package com.limelight.binding.input;

import android.annotation.SuppressLint;
import android.annotation.TargetApi;
import android.app.Activity;
import android.content.Context;
import android.hardware.BatteryState;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.hardware.display.DisplayManager;
import android.hardware.input.InputManager;
import android.hardware.lights.Light;
import android.hardware.lights.LightState;
import android.hardware.lights.LightsManager;
import android.hardware.lights.LightsRequest;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbManager;
import android.media.AudioAttributes;
import android.os.Build;
import android.os.CombinedVibration;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.VibrationAttributes;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.os.VibratorManager;
import android.view.InputDevice;
import android.view.Display;
import android.view.InputEvent;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.Surface;
import android.widget.Toast;

import com.limelight.LimeLog;
import com.limelight.Game;
import com.limelight.R;
import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.binding.input.driver.DualSenseController;
import com.limelight.binding.input.driver.UsbDriverListener;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.binding.input.keyboard.KeyboardFocus;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarInputJni;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.GameGestures;
import com.limelight.utils.Vector2d;

import org.cgutman.shieldcontrollerextensions.SceChargingState;
import org.cgutman.shieldcontrollerextensions.SceConnectionType;
import org.cgutman.shieldcontrollerextensions.SceManager;

import java.lang.reflect.InvocationTargetException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicIntegerArray;

public class ControllerHandler implements InputManager.InputDeviceListener, UsbDriverListener {

    private static final int START_DOWN_TIME_MOUSE_MODE_MS = 750;

    private static final int MINIMUM_BUTTON_DOWN_TIME_MS = 25;

    private static final int EMULATING_SPECIAL = 0x1;
    private static final int EMULATING_SELECT = 0x2;
    private static final int EMULATING_TOUCHPAD = 0x4;

    private static final short MAX_GAMEPADS = 16; // Limited by bits in activeGamepadMask

    private static final int BATTERY_RECHECK_INTERVAL_MS = 120 * 1000;

    private static volatile int[] androidButtonKeycodes;

    private static int[] androidButtonKeycodes() {
        int[] codes = androidButtonKeycodes;
        if (codes == null) {
            codes = NightjarInputJni.androidButtonKeycodes();
            androidButtonKeycodes = codes;
        }
        return codes;
    }

    private final Object fuseLock = new Object();
    private final int[] axisSetOut = new int[8];
    private final int[] remapOut = new int[1];
    private final int[] buttonStepOut = new int[5];
    private final int[] comboOut = new int[2];
    private int[] fuseScratch = new int[MAX_GAMEPADS * 6];
    private final int[] fuseOut = new int[7];
    private long slotStateHandle;

    private final ConcurrentHashMap<Integer, InputDeviceContext> inputDeviceContexts = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<Integer, UsbDeviceContext> usbDeviceContexts = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<Short, AbstractController> waveformControllers = new ConcurrentHashMap<>();
    // Attached USB devices whose driver could not start; launch preparation stops waiting for them.
    private final Set<Integer> failedUsbDevices = ConcurrentHashMap.newKeySet();
    // A late USB attachment must not block releases from an existing shared slot.
    private final AtomicIntegerArray controllerSlotsWithInput = new AtomicIntegerArray(MAX_GAMEPADS);

    private final NvConnection conn;
    private final Activity activityContext;
    private final double stickDeadzone;
    private final float triggerDeadzone;
    private final InputDeviceContext defaultContext = new InputDeviceContext();
    // Gamepad controls of the per-game touch layout, fused into the default context's slot.
    private volatile boolean layoutGamepadUsed;
    private int layoutInputMap;
    private short layoutLeftStickX, layoutLeftStickY, layoutRightStickX, layoutRightStickY;
    private byte layoutLeftTrigger, layoutRightTrigger;
    private final GameGestures gestures;
    private final InputManager inputManager;
    private final Vibrator deviceVibrator;
    // The phone's vibrator is standing in for a player 1 pad without a motor; it can run for
    // minutes on its own, so removing that pad has to stop it.
    private volatile boolean deviceVibratorForPlayerOne;
    // Vibrator calls are Binder calls, which can stall, and the main thread takes this handler's lock
    // (quitting, key events, device changes), so rumble keeps to a lock of its own (nightjar#73).
    private final Object rumbleLock = new Object();
    // An Xbox pad's rumble is a USB write that can take 100 ms, and every controller packet takes this handler's
    // lock for the active controllers. So a rumble only picks its USB pads under that lock and writes under this
    // one, which stop() takes too: no write follows the stop the pads get there.
    private final Object usbRumbleLock = new Object();
    // Under rumbleLock: while the stream has no focus the motors stay off, and the host's latest
    // levels per controller number ({low, high, left trigger, right trigger}) come back with focus.
    private boolean rumblePaused;
    // Concurrent: a pad removed clears its slot's levels without the rumble lock, whose holders take
    // the handler's lock (usbPadsOf) the other way round. All other access is under rumbleLock.
    private final Map<Short, short[]> rumbleLevels = new java.util.concurrent.ConcurrentHashMap<>();
    private final VibratorManager deviceVibratorManager;
    private final SensorManager deviceSensorManager;
    private final SceManager sceManager;
    private final Handler mainThreadHandler;
    private final HandlerThread backgroundHandlerThread;
    private final Handler backgroundThreadHandler;
    // Motion sensors report a few hundred times a second, so their events arrive on a thread of
    // their own instead of the main thread, where they would compete with input and drawing.
    private final MotionSensorThread motionSensorThread = new MotionSensorThread();
    // The activity display's rotation for device-orientation motion, read per sample without a Display lookup.
    private final DeviceRotation displayRotation = new DeviceRotation();
    // A sensor event sends its report under this lock, and so does the stationary gyroscope report
    // that ends a pad's motion. That report is sent after whatever ended the motion was set, so no
    // sample already on the sensor thread can reach the host behind it and leave the camera turning.
    private final Object motionSendLock = new Object();
    private boolean hasGameController;
    private boolean connectionStarted;
    private volatile boolean stopped = false;
    private volatile boolean sensorsEnabled = true;
    private volatile boolean inputEnabled = true;
    // Main thread: the sensors were turned off because the stream lost focus (not by picture-in-picture),
    // so focus coming back turns them on again.
    private boolean sensorsOffForFocus;

    private final PreferenceConfiguration prefConfig;
    /**
     * Whether this session's player 1 arrives as a PlayStation pad for the on-screen Touchpad
     * button. Fixed at launch: the stream menu turns the on-screen controls on and off mid-stream,
     * and the pad's type must not flip mid-game.
     */
    private final boolean touchpadButtonSession;
    /**
     * Whether the on-screen controls are their own player this session: controller 1 stays theirs
     * and every pad, built-in ones too, takes controller 2 and up. Fixed at launch for the reason
     * above: the stream menu's on and off must not renumber pads mid-game.
     */
    private final boolean onscreenOwnPlayerSession;
    /**
     * Whether the on-screen controls have the Steam Controller controls this session, fixed at
     * launch like {@link #touchpadButtonSession}: their pad arrives with a touchpad, grip and stick
     * touch, and the back buttons.
     */
    private final boolean steamControlsSession;
    /** Where the last physical touchpad finger on player 1 was across the pad; NaN until one touches. */
    private volatile float steamPadTouchX = Float.NaN;
    /** The slot whose pad may use this device's motors and sensors: the first one a pad can have. */
    private final short deviceFallbackSlot;
    /**
     * Input with no pad of its own: a handheld's system Back as Guide and events from no device. The
     * default context, unless the on-screen controls have controller 1 to themselves; then a context
     * of its own on the built-in pad's number (see {@link #handheldContext()}), so Guide chords with
     * the built-in buttons still reach one pad.
     */
    private final InputDeviceContext handheldContext;
    private final boolean rumbleAllowed;
    private final RumblePolicy.Feel rumbleFeel;
    private short currentControllers, initialControllers;

    public ControllerHandler(Activity activityContext, NvConnection conn, GameGestures gestures, PreferenceConfiguration prefConfig) {
        this.activityContext = activityContext;
        this.conn = conn;
        this.gestures = gestures;
        this.prefConfig = prefConfig;
        this.touchpadButtonSession = prefConfig.showTouchpadButton && !prefConfig.onlyL3R3;
        this.onscreenOwnPlayerSession = onscreenOwnsPlayerOne(prefConfig);
        // Both on-screen controller styles offer the Steam Controller controls.
        this.steamControlsSession = prefConfig.oscSteamControls;
        this.deviceFallbackSlot = (short) (onscreenOwnPlayerSession ? 1 : 0);
        this.rumbleAllowed = RumblePolicy.allows(prefConfig.disableRumble);
        this.rumbleFeel = RumblePolicy.Feel.fromKey(prefConfig.rumbleFeel);
		
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            this.deviceVibratorManager = (VibratorManager) activityContext.getSystemService(Context.VIBRATOR_MANAGER_SERVICE);
            this.deviceVibrator = (Vibrator) this.deviceVibratorManager.getDefaultVibrator();
        }
        else {
            this.deviceVibratorManager = null;
            this.deviceVibrator = (Vibrator) activityContext.getSystemService(Context.VIBRATOR_SERVICE);
        }
		
        this.deviceSensorManager = (SensorManager) activityContext.getSystemService(Context.SENSOR_SERVICE);
        this.inputManager = (InputManager) activityContext.getSystemService(Context.INPUT_SERVICE);
        this.mainThreadHandler = new Handler(Looper.getMainLooper());

        // Create a HandlerThread to process battery state updates. These can be slow enough
        // that they lead to ANRs if we do them on the main thread.
        this.backgroundHandlerThread = new HandlerThread("ControllerHandler");
        this.backgroundHandlerThread.start();
        this.backgroundThreadHandler = new Handler(backgroundHandlerThread.getLooper());

        this.sceManager = new SceManager(activityContext);
        this.sceManager.start();

        int deadzonePercentage = prefConfig.deadzonePercentage;

        int[] ids = InputDevice.getDeviceIds();
        for (int id : ids) {
            InputDevice dev = InputDevice.getDevice(id);
            if (dev == null) {
                // This device was removed during enumeration
                continue;
            }
            if ((dev.getSources() & InputDevice.SOURCE_JOYSTICK) != 0 ||
                    (dev.getSources() & InputDevice.SOURCE_GAMEPAD) != 0) {
                // This looks like a gamepad, but we'll check X and Y to be sure
                if (getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_X) != null &&
                    getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_Y) != null) {
                    // This is a gamepad
                    hasGameController = true;
                }
            }
        }

        // Negative values are an anti-deadzone, see applyStickDeadzone()
        this.stickDeadzone = (double)deadzonePercentage / 100.0;
        this.triggerDeadzone = prefConfig.triggerDeadzonePercentage / 100f;

        // Initialize the default context for events with no device
        initNoDeviceContext(defaultContext, (short) 0);
        // Controllers set to Off: the built-in pad gets a context of its own too, which never sends
        // (see sendControllerInputPacket), so it is not the on-screen gamepad's.
        handheldContext = onscreenOwnPlayerSession || !prefConfig.sendControllers ? new InputDeviceContext() : defaultContext;
        if (handheldContext != defaultContext) initNoDeviceContext(handheldContext, deviceFallbackSlot);

        // Get the initially attached set of gamepads. As each gamepad receives
        // its initial InputEvent, we will move these from this set onto the
        // currentControllers set which will allow them to properly unplug
        // if they are removed.
        initialControllers = getAttachedControllerMask(activityContext);
        if (onscreenOwnPlayerSession) currentControllers |= 1;
        slotStateHandle = NightjarInputJni.slotStateCreate(
                initialControllers, onscreenOwnPlayerSession, touchpadButtonSession, steamControlsSession);

        // Register ourselves for input device notifications
        inputManager.registerInputDeviceListener(this, null);
    }

    private void initNoDeviceContext(InputDeviceContext context, short controllerNumber) {
        context.leftStickXAxis = MotionEvent.AXIS_X;
        context.leftStickYAxis = MotionEvent.AXIS_Y;
        context.leftStickDeadzoneRadius = (float) stickDeadzone;
        context.rightStickXAxis = MotionEvent.AXIS_Z;
        context.rightStickYAxis = MotionEvent.AXIS_RZ;
        context.rightStickDeadzoneRadius = (float) stickDeadzone;
        context.triggerDeadzone = triggerDeadzone;
        context.leftTriggerAxis = MotionEvent.AXIS_BRAKE;
        context.rightTriggerAxis = MotionEvent.AXIS_GAS;
        context.hatXAxis = MotionEvent.AXIS_HAT_X;
        context.hatYAxis = MotionEvent.AXIS_HAT_Y;
        context.controllerNumber = controllerNumber;
        context.assignedControllerNumber = true;
        context.external = false;

        // Some devices (GPD XD) have a back button which sends input events
        // with device ID == 0. This hits the default context which would normally
        // consume these. Instead, let's ignore them since that's probably the
        // most likely case.
        context.ignoreBack = true;
    }

    private static InputDevice.MotionRange getMotionRangeForJoystickAxis(InputDevice dev, int axis) {
        InputDevice.MotionRange range;

        // First get the axis for SOURCE_JOYSTICK
        range = dev.getMotionRange(axis, InputDevice.SOURCE_JOYSTICK);
        if (range == null) {
            // Now try the axis for SOURCE_GAMEPAD
            range = dev.getMotionRange(axis, InputDevice.SOURCE_GAMEPAD);
        }

        return range;
    }

    @Override
    public synchronized void onInputDeviceAdded(int deviceId) {
        if (connectionStarted && !stopped && conn.canSendControllerInput()) {
            InputDeviceContext context = prepareInputDevice(deviceId);
            if (context != null) sendControllerInputPacket(context);
        }
        announcePresence();
        if (!stopped && isController(inputManager.getInputDevice(deviceId))) controllerAdded();
    }

    @Override
    public synchronized void onInputDeviceRemoved(int deviceId) {
        LimeLog.info("Bluetooth or Android controller detached");
        InputDeviceContext context = inputDeviceContexts.get(deviceId);
        if (context != null) {
            LimeLog.info("Removed controller: "+context.name+" ("+deviceId+")");
            // Out of the map first, so a built-in device no longer holds player 1 in the mask.
            inputDeviceContexts.remove(deviceId);
            releaseControllerNumber(context);
            context.destroy();
            if (context.external && context.assignedControllerNumber) announceBuiltInControls(context.controllerNumber);
        }
        announcePresence();
    }

    /** Tells a listener when a physical controller comes or goes, so the on-screen controls can step aside. */
    public interface PresenceListener {
        void onControllerPresenceChanged(boolean connected);
    }

    private PresenceListener presenceListener;
    private boolean externalPresent;

    public synchronized void setPresenceListener(PresenceListener listener) {
        presenceListener = listener;
        externalPresent = hasExternalControllers();
    }

    /** An external gamepad is connected: Bluetooth or USB, not the handheld's own built-in controls. */
    public synchronized boolean hasExternalControllers() {
        for (InputDeviceContext context : inputDeviceContexts.values()) {
            // A handheld's own pad is always there; counting it would keep the touch controls off for good. A remote
            // or an ordinary keyboard is not a controller either.
            InputDevice device = context.inputDevice;
            if (device != null && countsAsExternalController(context.external, isGamepadDevice(device),
                    isRemoteControlDevice(device), isAlphabeticKeyboard(device), isHandheldControls(device))) {
                return true;
            }
        }
        if (!usbDeviceContexts.isEmpty()) return true;
        // Before the first input event, look at what is attached rather than waiting for it to send one.
        if (inputManager == null) return false;
        for (int id : inputManager.getInputDeviceIds()) {
            InputDevice device = inputManager.getInputDevice(id);
            if (device != null && countsAsExternalController(isExternal(device), isGamepadDevice(device),
                    isRemoteControlDevice(device), isAlphabeticKeyboard(device), isHandheldControls(device))) {
                return true;
            }
        }
        return false;
    }

    /**
     * Whether an attached device counts as a physical controller beside the touch controls: Android lists it as
     * external, it has gamepad buttons or sticks, it is neither a remote nor an ordinary keyboard, and it is not the
     * handheld's own pad.
     */
    static boolean countsAsExternalController(boolean external, boolean gamepad, boolean remote, boolean keyboard,
                                              boolean handheldControls) {
        return NightjarInputJni.countsAsExternalController(external, gamepad, remote, keyboard, handheldControls);
    }

    /**
     * Whether the device is a controller at all: gamepad buttons or sticks. A fight stick or an NES-style pad with
     * only a hat and buttons has no X/Y sticks and still counts.
     */
    private static boolean isGamepadDevice(InputDevice device) {
        return hasJoystickAxes(device) || hasGamepadButtons(device);
    }

    /**
     * Whether this is an ordinary keyboard rather than a pad that also types: its layout is alphabetic and it has
     * neither gamepad buttons nor the gamepad's A button.
     */
    private static boolean isAlphabeticKeyboard(InputDevice device) {
        return device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC &&
                !hasGamepadButtons(device) && !device.hasKeys(KeyEvent.KEYCODE_BUTTON_A)[0];
    }

    private void announcePresence() {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            mainThreadHandler.post(this::announcePresence);
            return;
        }
        boolean now = hasExternalControllers();
        if (now == externalPresent) return;
        externalPresent = now;
        if (presenceListener != null) presenceListener.onControllerPresenceChanged(now);
    }

    // This can happen when gaining/losing input focus with some devices.
    // Input devices that have a trackpad may gain/lose AXIS_RELATIVE_X/Y.
    @Override
    public synchronized void onInputDeviceChanged(int deviceId) {
        InputDevice device = InputDevice.getDevice(deviceId);
        if (device == null) {
            return;
        }

        // If we don't have a context for this device, we don't need to update anything
        InputDeviceContext existingContext = inputDeviceContexts.get(deviceId);
        if (existingContext == null) {
            return;
        }

        LimeLog.info("Device changed: "+existingContext.name+" ("+deviceId+")");

        // Migrate the existing context into this new one by moving any stateful elements
        InputDeviceContext newContext = createInputDeviceContextForDevice(device);
        newContext.migrateContext(existingContext);
        inputDeviceContexts.put(deviceId, newContext);
    }

    public synchronized void stop() {
        if (stopped) {
            return;
        }

        // Stop new device contexts from being created or used
        stopped = true;
        presenceListener = null;

        // Unregister our input device callbacks
        inputManager.unregisterInputDeviceListener(this);

        for (InputDeviceContext deviceContext : inputDeviceContexts.values()) {
            deviceContext.destroy();
        }

        // Waits for a USB rumble write under way; one that comes after sees the handler stopped.
        synchronized (usbRumbleLock) {
            for (UsbDeviceContext deviceContext : usbDeviceContexts.values()) {
                deviceContext.device.stop();
                deviceContext.destroy();
            }
        }

        usbDeviceContexts.clear();
        inputDeviceContexts.clear();
        defaultContext.destroy();
        if (handheldContext != defaultContext) handheldContext.destroy();
        deviceVibratorForPlayerOne = false;
        deviceVibrator.cancel();
    }

    public void destroy() {
        if (!stopped) {
            stop();
        }

        mainThreadHandler.removeCallbacksAndMessages(null);
        backgroundThreadHandler.removeCallbacksAndMessages(null);
        sceManager.stop();
        backgroundHandlerThread.quit();
        // stop() unregistered every motion listener, and an event the thread is still handing to one
        // finds the stream stopped. The thread drops what it had queued and ends.
        motionSensorThread.quit();
        if (slotStateHandle != 0L) {
            NightjarInputJni.slotStateDestroy(slotStateHandle);
            slotStateHandle = 0L;
        }
    }

    /** Stops a pad's rotation on the host, ordered after any sensor sample being sent (see {@link #motionSendLock}). */
    private void sendGyroAtRest(short controllerNumber) {
        synchronized (motionSendLock) {
            conn.sendControllerMotionEvent((byte) controllerNumber, MoonBridge.LI_MOTION_TYPE_GYRO, 0, 0, 0);
        }
    }

    /** Unregisters a motion listener so that no sample of it reaches the host from here on. */
    private void closeMotionListener(SensorManager sensors, MotionListener listener) {
        // Closed first: a sample already on the sensor thread finds the listener closed, since
        // unregistering alone does not hold back one that was delivered just before.
        listener.closed = true;
        sensors.unregisterListener(listener);
        displayRotation.release(listener);
    }

    /** Registers a motion listener; one that maps device orientation keeps the rotation current while it is registered. */
    private boolean registerMotionListener(SensorManager sensors, Sensor sensor, MotionListener listener,
                                           int samplingPeriodUs) {
        // Acquired before registering, so the first sample already finds the rotation read.
        if (listener.needsDeviceOrientationCorrection) displayRotation.acquire(listener);
        boolean registered = motionSensorThread.register(sensors, sensor, listener, samplingPeriodUs);
        if (!registered) displayRotation.release(listener);
        return registered;
    }

    /**
     * The activity display's rotation. Held by the motion listeners that map device orientation:
     * read when the first one registers, kept current by a display listener that lives only while
     * one is registered. Display callbacks run on the main thread; the cached value is volatile.
     */
    private final class DeviceRotation implements DisplayManager.DisplayListener {
        private volatile int rotation;
        private volatile int displayId;
        private int holders; // guarded by this

        synchronized void acquire(MotionListener listener) {
            if (listener.holdsRotation) return;
            listener.holdsRotation = true;
            if (holders++ == 0) {
                Display display = activityContext.getWindowManager().getDefaultDisplay();
                displayId = display.getDisplayId();
                rotation = display.getRotation();
                displayManager().registerDisplayListener(this, mainThreadHandler);
            }
        }

        synchronized void release(MotionListener listener) {
            if (!listener.holdsRotation) return;
            listener.holdsRotation = false;
            if (--holders == 0) displayManager().unregisterDisplayListener(this);
        }

        int rotation() {
            return rotation;
        }

        private DisplayManager displayManager() {
            return (DisplayManager) activityContext.getSystemService(Context.DISPLAY_SERVICE);
        }

        @Override
        public void onDisplayAdded(int addedDisplayId) {
        }

        @Override
        public void onDisplayRemoved(int removedDisplayId) {
        }

        @Override
        public void onDisplayChanged(int changedDisplayId) {
            // Read the activity's display again, not only the one cached: the window may have
            // moved to another display (Dual screen, a foldable's cover screen).
            Display display = activityContext.getWindowManager().getDefaultDisplay();
            if (changedDisplayId == displayId || display.getDisplayId() != displayId) {
                displayId = display.getDisplayId();
                rotation = display.getRotation();
            }
        }
    }

    /** A controller the USB driver owns: Android lists no input device for it. */
    public boolean hasUsbControllers() {
        return !usbDeviceContexts.isEmpty();
    }

    /**
     * Takes over the buttons and left stick of USB-driver controllers (Android sees no key events for
     * them) while the on-screen PC keyboard is open. Called on the USB thread.
     */
    public interface PadCapture {
        /** A button, as a {@code ControllerPacket} flag, went down or up. */
        void button(int flag, boolean down);
        /** The left stick of the USB controller with this id, y growing downwards like Android reports it. */
        void stick(int controllerId, float x, float y);
    }

    private volatile PadCapture padCapture;
    private final java.util.concurrent.atomic.AtomicInteger captureGeneration = new java.util.concurrent.atomic.AtomicInteger();

    /**
     * The keyboard forgot what the pads were doing (a layout switch released everything) while it stays
     * open: each USB pad reports its held stick direction afresh on its next report.
     */
    public void resyncPadCapture() {
        captureGeneration.incrementAndGet();
    }

    /**
     * While set, USB-driver controllers send nothing to the game; null gives the input back. Set it
     * before anything else changes: the first report of each USB pad after that sends its neutral state
     * from the USB thread, so it is ordered after any report already on its way.
     */
    public void setPadCapture(PadCapture capture) {
        padCapture = capture;
        if (capture != null) {
            releaseInput(false);
        }
    }

    public void setInputEnabled(boolean enabled) {
        inputEnabled = enabled;
        if (!enabled) {
            releaseInput();
            // Every sample would be thrown away: unregister the motion sensors until focus returns.
            // releaseInput() has just told the PC the pads are at rest; it is not told twice.
            if (sensorsEnabled && !stopped) {
                stopSensors(false);
                sensorsOffForFocus = true;
            }
        }
        else if (sensorsOffForFocus) {
            sensorsOffForFocus = false;
            enableSensors();
        }
        // Vibrator and USB writes can stall, so not on the main thread.
        backgroundThreadHandler.post(() -> pauseRumble(!enabled));
    }

    /**
     * Without focus the pads and the phone stop vibrating instead of running on until the game sends
     * a zero; with focus back the host's latest levels play again.
     */
    private void pauseRumble(boolean paused) {
        synchronized (rumbleLock) {
            if (rumblePaused == paused || stopped) {
                return;
            }
            rumblePaused = paused;
            if (paused) {
                for (InputDeviceContext deviceContext : inputDeviceContexts.values()) {
                    deviceContext.cancelMotors();
                }
                for (short controllerNumber : rumbleLevels.keySet()) {
                    List<AbstractController> usbPads = usbPadsOf(controllerNumber);
                    if (usbPads == null || usbPads.isEmpty()) continue;
                    synchronized (usbRumbleLock) {
                        if (stopped) return;
                        for (AbstractController pad : usbPads) {
                            pad.rumble((short) 0, (short) 0);
                            pad.rumbleTriggers((short) 0, (short) 0);
                        }
                    }
                }
                deviceVibrator.cancel();
            }
            else {
                for (Map.Entry<Short, short[]> levels : new ArrayList<>(rumbleLevels.entrySet())) {
                    short[] l = levels.getValue();
                    if (l[0] != 0 || l[1] != 0) rumble(levels.getKey(), l[0], l[1]);
                    if (l[2] != 0 || l[3] != 0) rumbleTriggers(levels.getKey(), l[2], l[3]);
                }
            }
        }
    }

    private short[] levelsOf(short controllerNumber) {
        short[] levels = rumbleLevels.get(controllerNumber);
        if (levels == null) {
            levels = new short[4];
            rumbleLevels.put(controllerNumber, levels);
        }
        return levels;
    }

    public void releaseInput() {
        releaseInput(true);
    }

    private void releaseInput(boolean includeUsbPads) {
        if (stopped) {
            return;
        }
        for (InputDeviceContext context : inputDeviceContexts.values()) {
            releaseInput(context);
            if (context.assignedControllerNumber && context.gyroListener != null) {
                // Motion stops with the rest of the input; a held angular velocity keeps turning the camera.
                sendGyroAtRest(context.controllerNumber);
            }
        }
        if (includeUsbPads) {
            for (UsbDeviceContext context : usbDeviceContexts.values()) {
                releaseUsbInput(context);
            }
        }
        layoutInputMap = 0;
        layoutLeftTrigger = layoutRightTrigger = 0;
        layoutLeftStickX = layoutLeftStickY = layoutRightStickX = layoutRightStickY = 0;
        releaseInput(defaultContext);
        if (handheldContext != defaultContext) releaseInput(handheldContext);
    }

    private void releaseUsbInput(UsbDeviceContext context) {
        releaseInput(context);
        // A pad whose capabilities have not reached the host yet has nothing to release there,
        // nor has any physical pad with Controllers Off.
        if (!prefConfig.sendControllers || !context.assignedControllerNumber || context.arrivalPending) {
            return;
        }
        if ((context.capabilities() & MoonBridge.LI_CCAP_TOUCHPAD) != 0) {
            conn.sendControllerTouchEvent((byte) context.controllerNumber, MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL,
                    0, 0, 0, 0);
        }
        if (prefConfig.gamepadMotionSensors && (context.capabilities() & MoonBridge.LI_CCAP_GYRO) != 0) {
            sendGyroAtRest(context.controllerNumber);
        }
    }

    private void releaseInput(GenericControllerContext context) {
        context.inputMap = 0;
        context.leftTrigger = context.rightTrigger = 0;
        context.leftStickX = context.leftStickY = 0;
        context.rightStickX = context.rightStickY = 0;
        context.releaseMouseButtons();
        // A quit combo held as input went away (focus lost, the menu opened) is no longer held.
        context.quitComboPending = false;
        context.quitButtons = 0;
        // So is a Guide waiting to open the stream menu: no menu or tap follows, and like one that opened
        // the menu it stays held back until let go.
        if (context.guideHold.cancelPending()) {
            mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
        }
        if (context.guideTapEnd != null) {
            mainThreadHandler.removeCallbacks(context.guideTapEnd);
            context.guideTapEnd = null;
        }
        context.guideTapping = false;
        if (context instanceof InputDeviceContext) {
            InputDeviceContext inputContext = (InputDeviceContext) context;
            inputContext.pressedButtons.clear();
            for (Runnable r : inputContext.pendingButtonReleases.values()) {
                mainThreadHandler.removeCallbacks(r);
            }
            inputContext.pendingButtonReleases.clear();
            inputContext.emulatingButtonFlags = 0;
            inputContext.pendingExit = false;
        }
        if (context.assignedControllerNumber && NightjarInputJni.slotHasInput(slotStateHandle, context.controllerNumber) &&
                (prefConfig.sendControllers || context == defaultContext)) {
            // Bypass fusion while the other contexts sharing this number are being cleared.
            // Only a slot the host has seen input on can hold anything: a neutral report for
            // any other slot makes the host create a default pad there, which then ignores
            // the real controller's arrival (type, touchpad and motion) for the session.
            conn.sendControllerInput(context.controllerNumber, getActiveControllerMask(),
                    0, (byte) 0, (byte) 0, (short) 0, (short) 0, (short) 0, (short) 0);
        }
    }

    private boolean hasPendingControllerArrival(short controllerNumber) {
        for (UsbDeviceContext context : usbDeviceContexts.values()) {
            if (context.assignedControllerNumber && context.controllerNumber == controllerNumber && context.arrivalPending) {
                return true;
            }
        }
        return false;
    }

    /** Picture-in-picture: the sensors stay off until enableSensors(), whatever the focus does meanwhile. */
    public void disableSensors() {
        sensorsOffForFocus = false;
        stopSensors(true);
    }

    /** atRest: also tell the PC each gyro is still, unless the caller's release has just done so. */
    private void stopSensors(boolean atRest) {
        sensorsEnabled = false;
        for (InputDeviceContext deviceContext : inputDeviceContexts.values()) {
            deviceContext.disableSensors(atRest);
        }
        for (UsbDeviceContext context : usbDeviceContexts.values()) {
            context.unregisterDeviceMotion();
            if (atRest && context.assignedControllerNumber && (context.capabilities() & MoonBridge.LI_CCAP_GYRO) != 0) {
                sendGyroAtRest(context.controllerNumber);
            }
        }
    }

    public void enableSensors() {
        if (stopped) {
            return;
        }

        sensorsEnabled = true;

        for (InputDeviceContext deviceContext : inputDeviceContexts.values()) {
            deviceContext.enableSensors();
        }
        for (UsbDeviceContext context : usbDeviceContexts.values()) {
            context.registerDeviceMotion(MoonBridge.LI_MOTION_TYPE_ACCEL);
            context.registerDeviceMotion(MoonBridge.LI_MOTION_TYPE_GYRO);
        }
    }

    /**
     * Whether Android input device {@code device} counts as a controller, as the launch's gamepad mask
     * counts them. The controllers this app's USB driver claims are not input devices: the mask counts
     * them apart, and {@link #deviceAdded} reports them.
     */
    static boolean isController(InputDevice device) {
        return device != null && hasJoystickAxes(device);
    }

    private Runnable controllerAddedListener;

    /**
     * Runs {@code listener} on the main thread for each controller connected while this handler
     * lives: an input device that counts as one, or one this app's USB driver claims.
     */
    public synchronized void setControllerAddedListener(Runnable listener) {
        controllerAddedListener = listener;
    }

    private void controllerAdded() {
        Runnable listener = controllerAddedListener;
        if (listener == null) return;
        // Input devices are reported on the main thread; the USB driver reports from its own.
        if (Looper.myLooper() == Looper.getMainLooper()) listener.run();
        else mainThreadHandler.post(listener);
    }

    private static boolean hasJoystickAxes(InputDevice device) {
        return (device.getSources() & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK &&
                getMotionRangeForJoystickAxis(device, MotionEvent.AXIS_X) != null &&
                getMotionRangeForJoystickAxis(device, MotionEvent.AXIS_Y) != null;
    }

    private static boolean hasGamepadButtons(InputDevice device) {
        return (device.getSources() & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD;
    }

    /** Fire TV and similar remotes have a D-pad, but no gamepad buttons or joystick axes. */
    private static boolean isRemoteControlDevice(InputDevice device) {
        return device != null && device.supportsSource(InputDevice.SOURCE_DPAD) &&
                device.getKeyboardType() != InputDevice.KEYBOARD_TYPE_ALPHABETIC &&
                !hasGamepadButtons(device) && !hasJoystickAxes(device);
    }

    public static boolean isRemoteControlEvent(KeyEvent event) {
        InputDevice device = event.getDevice();
        if (device != null) return isRemoteControlDevice(device);
        // A remote can disappear between the press and release; preserve its event source.
        return event.isFromSource(InputDevice.SOURCE_DPAD) &&
                !event.isFromSource(InputDevice.SOURCE_GAMEPAD) &&
                !event.isFromSource(InputDevice.SOURCE_JOYSTICK);
    }

    public static boolean isGameControllerDevice(InputDevice device) {
        if (device == null) {
            return true;
        }

        if (isRemoteControlDevice(device)) return false;

        if (hasJoystickAxes(device) || hasGamepadButtons(device)) {
            // Has real joystick axes or gamepad buttons
            return true;
        }

        // HACK for https://issuetracker.google.com/issues/163120692
        if (Build.VERSION.SDK_INT == Build.VERSION_CODES.R) {
            if (device.getId() == -1) {
                // This "virtual" device could be input from any of the attached devices.
                // Look to see if any gamepads are connected.
                int[] ids = InputDevice.getDeviceIds();
                for (int id : ids) {
                    InputDevice dev = InputDevice.getDevice(id);
                    if (dev == null) {
                        // This device was removed during enumeration
                        continue;
                    }

                    // If there are any gamepad devices connected, we'll
                    // report that this virtual device is a gamepad.
                    if (hasJoystickAxes(dev) || hasGamepadButtons(dev)) {
                        return true;
                    }
                }
            }
        }

        // Otherwise, we'll try anything that claims to be a non-alphabetic keyboard
        return device.getKeyboardType() != InputDevice.KEYBOARD_TYPE_ALPHABETIC;
    }

    public static short getAttachedControllerMask(Context context) {
        int count = 0;
        short mask = 0;

        // Count all input devices that are gamepads
        InputManager im = (InputManager) context.getSystemService(Context.INPUT_SERVICE);
        for (int id : im.getInputDeviceIds()) {
            InputDevice dev = im.getInputDevice(id);
            if (dev == null) {
                continue;
            }

            if (hasJoystickAxes(dev)) {
                LimeLog.info("Counting InputDevice: "+dev.getName());
                mask |= 1 << count++;
            }
        }

        // Count all USB devices that match our drivers
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(context);
        if (config.usbDriver || config.usbDualSense || config.usbSwitchPro) {
            UsbManager usbManager = (UsbManager) context.getSystemService(Context.USB_SERVICE);
            if (usbManager != null) {
                for (UsbDevice dev : usbManager.getDeviceList().values()) {
                    // We explicitly check not to claim devices that appear as InputDevices
                    // otherwise we will double count them.
                    if (UsbDriverService.shouldClaimDevice(dev, config) &&
                            !UsbDriverService.isRecognizedInputDevice(dev)) {
                        LimeLog.info("Counting UsbDevice: "+dev.getDeviceName());
                        mask |= 1 << count++;
                    }
                }
            }
        }

        if (onscreenOwnsPlayerOne(config)) {
            // The pads counted above arrive from controller 2 up.
            mask = (short) (mask << 1);
        }

        if (config.onscreenController) {
            LimeLog.info("Counting OSC gamepad");
            mask |= 1;
        }

        LimeLog.info("Enumerated "+count+" gamepads");
        return mask;
    }

    /**
     * Whether the on-screen controls get controller 1 to themselves. Needs Multiple controllers,
     * where each pad has its own number, and more than the L3 and R3 that exist to join a pad.
     */
    static boolean onscreenOwnsPlayerOne(PreferenceConfiguration config) {
        return NightjarInputJni.onscreenOwnsPlayerOne(
                config.onscreenOwnPlayer, config.multiController, config.onscreenController, config.onlyL3R3);
    }

    /**
     * The built-in pad, the built-in device with sticks, numbered; null without one. While the on-screen
     * controls have controller 1, the handheld's other built-in devices and its system Back join it. With
     * {@code prepare}, a built-in pad without a number gets one first.
     */
    private InputDeviceContext builtInPad(boolean prepare) {
        for (InputDeviceContext context : inputDeviceContexts.values()) {
            if (!context.external && context.hasJoystickAxes && context.assignedControllerNumber) return context;
        }
        if (prepare) {
            for (int id : InputDevice.getDeviceIds()) {
                InputDevice device = InputDevice.getDevice(id);
                if (device == null || isExternal(device) || !hasJoystickAxes(device)) continue;
                InputDeviceContext context = prepareInputDevice(id);
                if (context != null && context.assignedControllerNumber) return context;
            }
        }
        return null;
    }

    /**
     * The context for input with no pad of its own. With the on-screen controls on controller 1 it takes
     * the number of the built-in pad, or of any built-in device without one, or else the first a pad can
     * have; it changes only while nothing is held on it, so a press never splits.
     */
    private synchronized InputDeviceContext handheldContext() {
        if (handheldContext != defaultContext && handheldContext.inputMap == 0 && handheldContext.guideTapEnd == null) {
            InputDeviceContext builtIn = builtInPad(false);
            for (InputDeviceContext context : inputDeviceContexts.values()) {
                if (builtIn == null && !context.external && context.assignedControllerNumber) builtIn = context;
            }
            handheldContext.controllerNumber = builtIn != null ? builtIn.controllerNumber : deviceFallbackSlot;
        }
        return handheldContext;
    }

    private void releaseControllerNumber(GenericControllerContext context) {
        if (deviceVibratorForPlayerOne && context.assignedControllerNumber && context.controllerNumber == deviceFallbackSlot) {
            deviceVibratorForPlayerOne = false;
            deviceVibrator.cancel();
        }

        // If we reserved a controller number, remove that reservation
        if (context.reservedControllerNumber) {
            LimeLog.info("Controller number "+context.controllerNumber+" is now available");
            currentControllers &= ~(1 << context.controllerNumber);
        }

        int released = NightjarInputJni.releaseReservedSlot(
                slotStateHandle, context.controllerNumber, context.reservedControllerNumber,
                context.assignedControllerNumber, prefConfig.sendControllers, prefConfig.multiController,
                prefConfig.onscreenController, layoutGamepadUsed, hasBuiltInPlayerOne(),
                hasPendingControllerArrival(context.controllerNumber));
        short activeControllerMask = (short) (released & 0xFFFF);
        if ((released & (1 << 16)) != 0) {
            // A cleared slot can retire a pre-launch prepared pad even before arrival.
            // A new active shared slot waits for its capabilities, without retrying on removal.
            conn.sendControllerInput(context.controllerNumber, activeControllerMask,
                    (short) 0,
                    (byte) 0, (byte) 0,
                    (short) 0, (short) 0,
                    (short) 0, (short) 0);
        }
        if ((released & (1 << 17)) != 0) {
            controllerSlotsWithInput.set(context.controllerNumber, 0);
        }
        if (context.assignedControllerNumber && !slotStillAssigned(context, context.controllerNumber)) {
            // The next pad in this slot must not play the old pad's levels when rumble resumes.
            rumbleLevels.remove(context.controllerNumber);
        }
    }

    /** Whether another pad still plays as this controller slot, after {@code released} goes. */
    private boolean slotStillAssigned(GenericControllerContext released, int controllerNumber) {
        for (InputDeviceContext other : inputDeviceContexts.values()) {
            if (other != released && other.assignedControllerNumber && other.controllerNumber == controllerNumber) {
                return true;
            }
        }
        for (UsbDeviceContext other : usbDeviceContexts.values()) {
            if (other != released && other.assignedControllerNumber && other.controllerNumber == controllerNumber) {
                return true;
            }
        }
        return false;
    }

    private boolean isAssociatedJoystick(InputDevice originalDevice, InputDevice possibleAssociatedJoystick) {
        if (possibleAssociatedJoystick == null) {
            return false;
        }

        // This can't be an associated joystick if it's not a joystick
        if ((possibleAssociatedJoystick.getSources() & InputDevice.SOURCE_JOYSTICK) != InputDevice.SOURCE_JOYSTICK) {
            return false;
        }

        // Make sure the device names *don't* match in order to prevent us from accidentally matching
        // on another of the exact same device.
        if (possibleAssociatedJoystick.getName().equals(originalDevice.getName())) {
            return false;
        }

        // Make sure the descriptor matches. This can match in cases where two of the exact same
        // input device are connected, so we perform the name check to exclude that case.
        if (!possibleAssociatedJoystick.getDescriptor().equals(originalDevice.getDescriptor())) {
            return false;
        }

        return true;
    }

    // Called before sending input but after we've determined that this
    // is definitely a controller (not a keyboard, mouse, or something else)
    private void assignControllerNumberIfNeeded(GenericControllerContext context) {
        if (context.assignedControllerNumber) {
            return;
        }
        // Input on the main thread and launch preparation on the connection worker can both
        // assign a pad. Without one lock a pad could take two slots, or two pads share one.
        synchronized (this) {
            if (!context.assignedControllerNumber) {
                assignControllerNumber(context);
            }
        }
    }

    private void assignControllerNumber(GenericControllerContext context) {
        if (context instanceof InputDeviceContext) {
            InputDeviceContext devContext = (InputDeviceContext) context;

            LimeLog.info(devContext.name+" ("+context.id+") needs a controller number assigned");
            InputDeviceContext builtInPad = onscreenOwnPlayerSession && !devContext.external && !devContext.hasJoystickAxes ?
                    builtInPad(true) : null;
            int builtInSlot = builtInPad != null ? builtInPad.controllerNumber : -1;
            boolean hasGamepadBtns = devContext.inputDevice != null && hasGamepadButtons(devContext.inputDevice);
            boolean hasAdjacentAssoc = prefConfig.multiController && !devContext.hasJoystickAxes && hasGamepadBtns &&
                    (isAssociatedJoystick(devContext.inputDevice, InputDevice.getDevice(devContext.id + 1)) ||
                            isAssociatedJoystick(devContext.inputDevice, InputDevice.getDevice(devContext.id - 1)));
            int decision = NightjarInputJni.assignInputDeviceSlot(
                    slotStateHandle, devContext.external, devContext.hasJoystickAxes,
                    hasGamepadBtns, hasAdjacentAssoc, prefConfig.multiController, builtInSlot);
            if ((decision & (1 << 10)) != 0) {
                context.controllerNumber = 0;
                InputDevice associatedDevice = InputDevice.getDevice(devContext.id + 1);
                if (!isAssociatedJoystick(devContext.inputDevice, associatedDevice)) {
                    associatedDevice = InputDevice.getDevice(devContext.id - 1);
                    if (!isAssociatedJoystick(devContext.inputDevice, associatedDevice)) {
                        LimeLog.info("No associated joystick device found");
                        associatedDevice = null;
                    }
                }
                if (associatedDevice != null) {
                    InputDeviceContext associatedDeviceContext = inputDeviceContexts.get(associatedDevice.getId());
                    if (associatedDeviceContext == null) {
                        associatedDeviceContext = createInputDeviceContextForDevice(associatedDevice);
                        inputDeviceContexts.put(associatedDevice.getId(), associatedDeviceContext);
                    }
                    if (!associatedDeviceContext.assignedControllerNumber) {
                        assignControllerNumberIfNeeded(associatedDeviceContext);
                        if (!associatedDeviceContext.assignedControllerNumber) {
                            return;
                        }
                    }
                    context.controllerNumber = associatedDeviceContext.controllerNumber;
                    LimeLog.info("Propagated controller number from "+associatedDeviceContext.name);
                }
                else {
                    int fb = NightjarInputJni.resolveAssociatedFallbackSlot(slotStateHandle);
                    if ((fb & (1 << 9)) == 0) {
                        return;
                    }
                    context.controllerNumber = (short) (fb & 0xFF);
                    if ((fb & (1 << 8)) != 0) {
                        context.reservedControllerNumber = true;
                        currentControllers |= (1 << context.controllerNumber);
                        initialControllers &= ~(1 << context.controllerNumber);
                    }
                }
            }
            else if ((decision & (1 << 9)) == 0) {
                return;
            }
            else {
                context.controllerNumber = (short) (decision & 0xFF);
                if ((decision & (1 << 8)) != 0) {
                    context.reservedControllerNumber = true;
                    currentControllers |= (1 << context.controllerNumber);
                    initialControllers &= ~(1 << context.controllerNumber);
                }
            }

            // If the gamepad doesn't have motion sensors, use the on-device sensors as a fallback for player 1
            if (prefConfig.gamepadMotionSensorsFallbackToDevice && context.controllerNumber == deviceFallbackSlot && devContext.sensorManager == null &&
                    !dropsMotionAndTouchpad(devContext.buttonMap) && !deviceSensorsInUse()) {
                devContext.sensorManager = deviceSensorManager;
            }
        }
        else {
            int decision = NightjarInputJni.assignUsbDeviceSlot(slotStateHandle, prefConfig.multiController);
            if ((decision & (1 << 9)) == 0) {
                return;
            }
            context.controllerNumber = (short) (decision & 0xFF);
            if ((decision & (1 << 8)) != 0) {
                context.reservedControllerNumber = true;
                currentControllers |= (1 << context.controllerNumber);
                initialControllers &= ~(1 << context.controllerNumber);
            }
        }

        LimeLog.info("Assigned as controller "+context.controllerNumber);
        context.assignedControllerNumber = true;

        if (context instanceof UsbDeviceContext) {
            UsbDeviceContext usbContext = (UsbDeviceContext) context;
            // A USB pad without motion sensors uses this device's for player 1, as an input device does
            if (deviceSensorManager != null && !deviceSensorsInUse() &&
                    takesDeviceMotion(prefConfig.gamepadMotionSensorsFallbackToDevice, (short) (context.controllerNumber - deviceFallbackSlot),
                            usbContext.device.getCapabilities(), usbContext.buttonMap)) {
                usbContext.deviceSensors = deviceSensorManager;
                if (motionOverridesType(usbContext.device.getType(), true, usbContext.buttonMap)) {
                    mainThreadHandler.post(() -> Toast.makeText(activityContext,
                            R.string.toast_controller_type_changed, Toast.LENGTH_LONG).show());
                }
            }
        }

        // Report attributes of this new controller to the host
        context.sendControllerArrival();
    }

    private UsbDeviceContext createUsbDeviceContextForDevice(AbstractController device) {
        UsbDeviceContext context = new UsbDeviceContext();

        context.id = device.getControllerId();
        context.device = device;
        context.external = true;

        context.vendorId = device.getVendorId();
        context.productId = device.getProductId();
        context.buttonMap = ControllerButtonMap.load(activityContext,
                ControllerButtonMap.deviceKey(context.vendorId, context.productId, null));
        context.axisMap = ControllerAxisMap.load(activityContext,
                ControllerButtonMap.deviceKey(context.vendorId, context.productId, null));
        context.gyroMapping = GyroMappingPreferences.transformerForModel(activityContext,
                ControllerButtonMap.deviceKey(context.vendorId, context.productId, null));

        context.leftStickDeadzoneRadius = (float) stickDeadzone;
        context.rightStickDeadzoneRadius = (float) stickDeadzone;
        context.triggerDeadzone = triggerDeadzone;

        return context;
    }

    private static boolean hasButtonUnderTouchpad(InputDevice dev, byte type) {
        // It has to have a touchpad to have a button under it
        if ((dev.getSources() & InputDevice.SOURCE_TOUCHPAD) != InputDevice.SOURCE_TOUCHPAD) {
            return false;
        }

        // Landroid/view/InputDevice;->hasButtonUnderPad()Z is blocked after O
        if (Build.VERSION.SDK_INT <= Build.VERSION_CODES.O) {
            try {
                return (Boolean) dev.getClass().getMethod("hasButtonUnderPad").invoke(dev);
            } catch (NoSuchMethodException e) {
                e.printStackTrace();
            } catch (IllegalAccessException e) {
                e.printStackTrace();
            } catch (InvocationTargetException e) {
                e.printStackTrace();
            } catch (ClassCastException e) {
                e.printStackTrace();
            }
        }

        // We can't use the platform API, so we'll have to just guess based on the gamepad type.
        // If this is a PlayStation controller with a touchpad, we know it has a clickpad.
        return type == MoonBridge.LI_CTYPE_PS;
    }

    private static boolean isExternal(InputDevice dev) {
        int override = NightjarInputJni.isExternalOverride(Build.MODEL, dev.getName());
        if (override >= 0) {
            return override == 1;
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            // Landroid/view/InputDevice;->isExternal()Z is officially public on Android Q
            return dev.isExternal();
        }
        else {
            try {
                // Landroid/view/InputDevice;->isExternal()Z is on the light graylist in Android P
                return (Boolean)dev.getClass().getMethod("isExternal").invoke(dev);
            } catch (NoSuchMethodException e) {
                e.printStackTrace();
            } catch (IllegalAccessException e) {
                e.printStackTrace();
            } catch (InvocationTargetException e) {
                e.printStackTrace();
            } catch (ClassCastException e) {
                e.printStackTrace();
            }
        }

        // Answer true if we don't know
        return true;
    }

    private boolean shouldIgnoreBack(InputDevice dev) {
        boolean external = isExternal(dev);
        boolean internalGamepad = false;
        boolean internalSelect = false;
        if (!external) {
            boolean[] internal = findInternalGamepadAndSelect();
            internalGamepad = internal[0];
            internalSelect = internal[1];
        }
        return NightjarInputJni.shouldIgnoreBack(
                dev.getName(), hasJoystickAxes(dev), hasGamepadButtons(dev),
                external, internalGamepad, internalSelect);
    }

    /** Back sends Guide when the option is on, except the Back with modifiers some remote apps type for B. */
    static boolean isBackAsGuide(boolean backAsGuide, int keyCode, int flags) {
        return NightjarInputJni.isBackAsGuide(backAsGuide, keyCode, flags);
    }

    /**
     * Back as Guide on any gamepad, built-in or external, as the setting asks: also a Back that is
     * the pad's Select, which the user then gives up. Only a pad that uses Back as Start keeps it.
     */
    static boolean isPadBackAsGuide(boolean backAsGuide, int keyCode, int flags, boolean backIsStart) {
        return NightjarInputJni.isPadBackAsGuide(backAsGuide, keyCode, flags, backIsStart);
    }

    /**
     * A Back from a handheld's system button that is not on its gamepad device: one its firmware
     * injects (Android's virtual keyboard device) or a built-in key device such as gpio-keys.
     * Never the navigation bar or gesture, a fallback key, a mouse's right click, an external or
     * alphabetic keyboard, a gamepad (handleButtonDown/Up), or a device without a gamepad whose
     * Back is spare (see backIsSpareButton).
     */
    public static boolean isHandheldSystemBack(int keyCode, int flags, int source, boolean virtualDevice,
                                               boolean external, boolean alphabeticKeyboard, boolean gamepad,
                                               boolean backIsSpareButton) {
        return NightjarInputJni.isHandheldSystemBack(
                keyCode, flags, source, virtualDevice, external, alphabeticKeyboard, gamepad, backIsSpareButton);
    }

    /**
     * Whether a gamepad is attached, and its system Back is not its Select: a built-in gamepad
     * without its own Select (Shield Portable) uses that Back as Select (shouldIgnoreBack). Built-in
     * pads Android lists as external, such as a Retroid's, count like any attached gamepad.
     */
    static boolean backIsSpareButton(boolean internalGamepad, boolean internalSelect, boolean anyGamepad) {
        return NightjarInputJni.backIsSpareButton(internalGamepad, internalSelect, anyGamepad);
    }

    /**
     * Whether this device has a built-in gamepad and a built-in Select button, on any internal
     * device, and whether any gamepad, internal or external, is attached.
     */
    protected boolean[] findInternalGamepadAndSelect() {
        InputManager im = (InputManager) activityContext.getSystemService(Context.INPUT_SERVICE);

        boolean foundInternalGamepad = false;
        boolean foundInternalSelect = false;
        boolean foundGamepad = false;
        for (int id : im.getInputDeviceIds()) {
            InputDevice currentDev = im.getInputDevice(id);
            if (currentDev == null) {
                continue;
            }
            if (!currentDev.isVirtual() && hasGamepadButtons(currentDev)) {
                foundGamepad = true;
            }

            // Ignore external devices
            if (isExternal(currentDev)) {
                continue;
            }

            // Note that we are explicitly NOT excluding the device a Back came from here,
            // since the other gamepad buttons may be on that device and that's fine.
            if (currentDev.hasKeys(KeyEvent.KEYCODE_BUTTON_SELECT)[0]) {
                foundInternalSelect = true;
            }

            // We don't check KEYCODE_BUTTON_A here, since the Shield Android TV has a
            // virtual mouse device that claims to have KEYCODE_BUTTON_A. Instead, we rely
            // on the SOURCE_GAMEPAD flag to be set on gamepad devices.
            if (hasGamepadButtons(currentDev)) {
                foundInternalGamepad = true;
            }
        }
        return new boolean[] {foundInternalGamepad, foundInternalSelect, foundGamepad};
    }

    private boolean backIsSpareButton() {
        boolean[] found = findInternalGamepadAndSelect();
        return backIsSpareButton(found[0], found[1], found[2]);
    }

    /**
     * Back as Guide for a handheld's system button (see isHandheldSystemBack): Guide goes out on
     * controller 0, where built-in buttons always sit. Returns whether the event was such a Back.
     */
    public boolean handleHandheldBackAsGuide(KeyEvent event) {
        if (!isBackAsGuide(prefConfig.backAsGuide, event.getKeyCode(), event.getFlags())) {
            return false;
        }
        InputDevice dev = event.getDevice();
        boolean virtualDevice = dev == null || dev.isVirtual();
        if (!isHandheldSystemBack(event.getKeyCode(), event.getFlags(), event.getSource(), virtualDevice,
                !virtualDevice && isExternal(dev),
                !virtualDevice && dev.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC,
                !virtualDevice && (hasGamepadButtons(dev) || hasJoystickAxes(dev)), backIsSpareButton())) {
            return false;
        }
        if (stopped) {
            return true;
        }

        // The built-in pad's own Guide: with the on-screen controls on controller 1, its number, not theirs
        InputDeviceContext context = handheldContext();
        if (event.getAction() == KeyEvent.ACTION_DOWN) {
            boolean repeat = event.getRepeatCount() > 0;
            if (!repeat) {
                endGuideTap(context);
                // No key event to replay: a chord sends the flag itself (sendHeldGuideOn)
                context.guideHeldDown = null;
            }
            if (holdGuide(context, repeat)) {
                return true;
            }
            context.inputMap |= ControllerPacket.SPECIAL_BUTTON_FLAG;
        }
        else if (event.getAction() == KeyEvent.ACTION_UP) {
            GuideHold.Release release = releaseGuide(context);
            if (release == GuideHold.Release.TAP) {
                tapDefaultGuide(context);
            }
            if (release != GuideHold.Release.PASS) {
                return true;
            }
            // Firmware injects the press and release together; hold Guide long enough for games to see it
            long remaining = MINIMUM_BUTTON_DOWN_TIME_MS - (event.getEventTime() - event.getDownTime());
            if (remaining > 0) {
                scheduleDefaultGuideRelease(context, remaining);
                return true;
            }
            context.inputMap &= ~ControllerPacket.SPECIAL_BUTTON_FLAG;
        }
        sendControllerInputPacket(context);
        return true;
    }

    private InputDeviceContext createInputDeviceContextForDevice(InputDevice dev) {
        InputDeviceContext context = new InputDeviceContext();
        String devName = dev.getName();

        LimeLog.info("Creating controller context for device: "+devName);
        LimeLog.info("Vendor ID: " + dev.getVendorId());
        LimeLog.info("Product ID: "+dev.getProductId());
        LimeLog.info(dev.toString());

        context.inputDevice = dev;
        context.name = devName;
        context.id = dev.getId();
        context.external = isExternal(dev);

        context.vendorId = dev.getVendorId();
        context.productId = dev.getProductId();
        context.buttonMap = ControllerButtonMap.load(activityContext,
                ControllerButtonMap.deviceKey(context.vendorId, context.productId, devName));
        context.axisMap = ControllerAxisMap.load(activityContext,
                ControllerButtonMap.deviceKey(context.vendorId, context.productId, devName));

        // These aren't always present in the Android key layout files, so they won't show up
        // in our normal InputDevice.hasKeys() probing.
        context.hasPaddles = MoonBridge.guessControllerHasPaddles(context.vendorId, context.productId);
        context.hasShare = MoonBridge.guessControllerHasShareButton(context.vendorId, context.productId);

        // Try to use the InputDevice's associated vibrators first
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && hasQuadAmplitudeControlledRumbleVibrators(dev.getVibratorManager())) {
            context.vibratorManager = dev.getVibratorManager();
            context.quadVibrators = true;
        }
        else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && hasDualAmplitudeControlledRumbleVibrators(dev.getVibratorManager())) {
            context.vibratorManager = dev.getVibratorManager();
            context.quadVibrators = false;
        }
        else if (dev.getVibrator().hasVibrator()) {
            context.vibrator = dev.getVibrator();
        }
        else if (!context.external) {
            // If this is an internal controller, try to use the device's vibrator
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && hasQuadAmplitudeControlledRumbleVibrators(deviceVibratorManager)) {
                context.vibratorManager = deviceVibratorManager;
                context.quadVibrators = true;
            }
            else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && hasDualAmplitudeControlledRumbleVibrators(deviceVibratorManager)) {
                context.vibratorManager = deviceVibratorManager;
                context.quadVibrators = false;
            }
            else if (deviceVibrator.hasVibrator()) {
                context.vibrator = deviceVibrator;
            }
        }

        // On Android 12, we can try to use the InputDevice's sensors. This may not work if the
        // Linux kernel version doesn't have motion sensor support, which is common for third-party
        // gamepads.
        //
        // Android 12 has a bug that causes InputDeviceSensorManager to cause a NPE on a background
        // thread due to bad error checking in InputListener callbacks. InputDeviceSensorManager is
        // created upon the first call to InputDevice.getSensorManager(), so we avoid calling this
        // on Android 12 unless we have a gamepad that could plausibly have motion sensors.
        // https://cs.android.com/android/_/android/platform/frameworks/base/+/8970010a5e9f3dc5c069f56b4147552accfcbbeb
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            if ((Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU || context.vendorId == 0x054c || context.vendorId == 0x057e) && // Sony or Nintendo
                    prefConfig.gamepadMotionSensors && !dropsMotionAndTouchpad(context.buttonMap)) {
                if (dev.getSensorManager().getDefaultSensor(Sensor.TYPE_ACCELEROMETER) != null || dev.getSensorManager().getDefaultSensor(Sensor.TYPE_GYROSCOPE) != null) {
                    context.sensorManager = dev.getSensorManager();
                }
            }
        }

        // Check if this device has a usable RGB LED and cache that result
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            for (Light light : dev.getLightsManager().getLights()) {
                if (light.hasRgbControl()) {
                    context.hasRgbLed = true;
                    break;
                }
            }

            // Light.hasRgbControl() was totally broken prior to Android 14.
            // It always returned true because LIGHT_CAPABILITY_RGB was defined as 0,
            // so we will just guess RGB is supported if it's a PlayStation controller.
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE && context.vendorId != 0x054c) {
                context.hasRgbLed = false;
            }
        }

        // Detect if the gamepad has Mode and Select buttons according to the Android key layouts.
        // We do this first because other codepaths below may override these defaults.
        boolean[] buttons = dev.hasKeys(KeyEvent.KEYCODE_BUTTON_MODE, KeyEvent.KEYCODE_BUTTON_SELECT, KeyEvent.KEYCODE_BACK, 0);
        context.hasMode = buttons[0];
        context.hasSelect = buttons[1] || buttons[2];

        context.touchpadXRange = dev.getMotionRange(MotionEvent.AXIS_X, InputDevice.SOURCE_TOUCHPAD);
        context.touchpadYRange = dev.getMotionRange(MotionEvent.AXIS_Y, InputDevice.SOURCE_TOUCHPAD);
        context.touchpadPressureRange = dev.getMotionRange(MotionEvent.AXIS_PRESSURE, InputDevice.SOURCE_TOUCHPAD);

        context.leftStickXAxis = MotionEvent.AXIS_X;
        context.leftStickYAxis = MotionEvent.AXIS_Y;
        if (getMotionRangeForJoystickAxis(dev, context.leftStickXAxis) != null &&
                getMotionRangeForJoystickAxis(dev, context.leftStickYAxis) != null) {
            // This is a gamepad
            hasGameController = true;
            context.hasJoystickAxes = true;
        }

        InputDevice.MotionRange leftTriggerRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_LTRIGGER);
        InputDevice.MotionRange rightTriggerRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_RTRIGGER);
        InputDevice.MotionRange brakeRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_BRAKE);
        InputDevice.MotionRange gasRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_GAS);
        InputDevice.MotionRange throttleRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_THROTTLE);
        InputDevice.MotionRange rxRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_RX);
        InputDevice.MotionRange ryRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_RY);
        InputDevice.MotionRange zRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_Z);
        InputDevice.MotionRange rzRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_RZ);
        InputDevice.MotionRange hatXRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_HAT_X);
        InputDevice.MotionRange hatYRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_HAT_Y);
        if (hatXRange != null && hatYRange != null) {
            context.hatXAxis = MotionEvent.AXIS_HAT_X;
            context.hatYAxis = MotionEvent.AXIS_HAT_Y;
        }
        boolean hasRxRy = rxRange != null && ryRange != null;
        boolean hasButtonC = hasRxRy && devName != null && dev.getVendorId() == 0x054c &&
                dev.hasKeys(KeyEvent.KEYCODE_BUTTON_C)[0];
        boolean hasStartOrMenu = true;
        if (devName != null && devName.contains("ASUS Gamepad")) {
            boolean[] hasStartKey = dev.hasKeys(KeyEvent.KEYCODE_BUTTON_START, KeyEvent.KEYCODE_MENU, 0);
            hasStartOrMenu = hasStartKey[0] || hasStartKey[1];
        }
        context.ignoreBack = shouldIgnoreBack(dev);

        int[] classifyInts = new int[6];
        float[] classifyFloat = new float[1];
        NightjarInputJni.classifyInputDevice(
                context.vendorId, context.productId, devName,
                dev.getSources() == (InputDevice.SOURCE_KEYBOARD | InputDevice.SOURCE_MOUSE),
                leftTriggerRange != null && rightTriggerRange != null,
                brakeRange != null && gasRange != null,
                brakeRange != null && throttleRange != null,
                gasRange != null,
                hasRxRy,
                zRange != null && rzRange != null,
                hasButtonC,
                context.hatXAxis != -1 && context.hatYAxis != -1,
                hasStartOrMenu,
                context.hasSelect,
                context.hasMode,
                context.ignoreBack,
                triggerDeadzone,
                classifyInts,
                classifyFloat);
        context.rightStickXAxis = classifyInts[0];
        context.rightStickYAxis = classifyInts[1];
        context.leftTriggerAxis = classifyInts[2];
        context.rightTriggerAxis = classifyInts[3];
        int quirks = classifyInts[4];
        context.isDualShockStandaloneTouchpad = (quirks & NightjarInputJni.QUIRK_DS4_STANDALONE_TOUCHPAD) != 0;
        context.usesLinuxGamepadStandardFaceButtons = (quirks & NightjarInputJni.QUIRK_LINUX_STD_FACE) != 0;
        context.isNonStandardDualShock4 = (quirks & NightjarInputJni.QUIRK_NON_STD_DS4) != 0;
        context.isServal = (quirks & NightjarInputJni.QUIRK_SERVAL) != 0;
        context.isNonStandardXboxBtController = (quirks & NightjarInputJni.QUIRK_NON_STD_XBOX_BT) != 0;
        context.backIsStart = (quirks & NightjarInputJni.QUIRK_BACK_IS_START) != 0;
        context.modeIsSelect = (quirks & NightjarInputJni.QUIRK_MODE_IS_SELECT) != 0;
        context.searchIsMode = (quirks & NightjarInputJni.QUIRK_SEARCH_IS_MODE) != 0;
        int boolBits = classifyInts[5];
        context.triggersIdleNegative = (boolBits & 1) != 0;
        context.hasSelect = (boolBits & 2) != 0;
        context.hasMode = (boolBits & 4) != 0;
        context.triggerDeadzone = classifyFloat[0];

        if (context.leftStickXAxis != -1 && context.leftStickYAxis != -1) {
            context.leftStickDeadzoneRadius = (float) stickDeadzone;
        }
        if (context.rightStickXAxis != -1 && context.rightStickYAxis != -1) {
            context.rightStickDeadzoneRadius = (float) stickDeadzone;
        }

        LimeLog.info("Analog stick deadzone: "+context.leftStickDeadzoneRadius+" "+context.rightStickDeadzoneRadius);
        LimeLog.info("Trigger deadzone: "+context.triggerDeadzone);

        return context;
    }

    private InputDeviceContext getContextForEvent(InputEvent event) {
        // Don't return a context if we're stopped
        if (stopped) {
            return null;
        }
        else if (event.getDeviceId() == 0) {
            // Unknown devices use the default context, or the built-in pad's with the on-screen controls on controller 1
            return handheldContext();
        }
        else if (event.getDevice() == null) {
            // During device removal, sometimes we can get events after the
            // input device has been destroyed. In this case we'll see a
            // != 0 device ID but no device attached.
            return null;
        }

        // HACK for https://issuetracker.google.com/issues/163120692
        if (Build.VERSION.SDK_INT == Build.VERSION_CODES.R) {
            if (event.getDeviceId() == -1) {
                return handheldContext();
            }
        }

        // Return the existing context if it exists
        InputDeviceContext context = inputDeviceContexts.get(event.getDeviceId());
        if (context != null) {
            return context;
        }

        // Otherwise create a new context
        context = createInputDeviceContextForDevice(event.getDevice());
        inputDeviceContexts.put(event.getDeviceId(), context);

        return context;
    }

    private static byte maxByMagnitude(byte a, byte b) {
        return NightjarInputJni.maxByMagnitudeByte(a, b);
    }

    private static short maxByMagnitude(short a, short b) {
        return NightjarInputJni.maxByMagnitudeShort(a, b);
    }

    // Slot assignment changes both masks under this lock, on the main thread or the connection
    // worker. Read them under it too, so input never carries a mask from mid-assignment.
    private boolean sharesSlotWithExternalPad(GenericControllerContext builtIn) {
        for (InputDeviceContext context : inputDeviceContexts.values()) {
            if (context != builtIn && context.external && context.assignedControllerNumber &&
                    context.controllerNumber == builtIn.controllerNumber) return true;
        }
        for (UsbDeviceContext context : usbDeviceContexts.values()) {
            if (context.assignedControllerNumber && context.controllerNumber == builtIn.controllerNumber) return true;
        }
        return false;
    }

    // An external pad sharing the slot left: built-in controls there take over the slot's profile.
    private void announceBuiltInControls(short slot) {
        if (!connectionStarted || stopped || !conn.canSendControllerInput()) return;
        for (InputDeviceContext context : inputDeviceContexts.values()) {
            if (!context.external && context.assignedControllerNumber && context.controllerNumber == slot) {
                context.sendControllerArrival();
                return;
            }
        }
    }

    private boolean hasBuiltInPlayerOne() {
        for (InputDeviceContext context : inputDeviceContexts.values()) {
            if (context.assignedControllerNumber && !context.external && context.controllerNumber == 0) return true;
        }
        return false;
    }

    /** The buttons the on-screen controller has, for its arrival. */
    static final int ONSCREEN_PAD_BUTTONS = ControllerPacket.A_FLAG | ControllerPacket.B_FLAG |
            ControllerPacket.X_FLAG | ControllerPacket.Y_FLAG | ControllerPacket.UP_FLAG |
            ControllerPacket.DOWN_FLAG | ControllerPacket.LEFT_FLAG | ControllerPacket.RIGHT_FLAG |
            ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG | ControllerPacket.LS_CLK_FLAG |
            ControllerPacket.RS_CLK_FLAG | ControllerPacket.BACK_FLAG | ControllerPacket.PLAY_FLAG |
            ControllerPacket.SPECIAL_BUTTON_FLAG | ControllerPacket.TOUCHPAD_FLAG;

    /**
     * Whether the on-screen controls can show their Touchpad button this session: the setting was
     * on at launch, whether or not the controls show right now (see {@link #touchpadButtonSession}).
     */
    private boolean showsTouchpadButton() {
        return touchpadButtonSession;
    }

    /**
     * Whether built-in controls arrive as a PlayStation pad for the on-screen Touchpad button they
     * share player 1 with: hosts make a DualShock 4 or DualSense for this type, the pads with a
     * touchpad click. A type the user pinned for the pad wins.
     */
    private boolean carriesTouchpadButton(InputDeviceContext context) {
        if (!showsTouchpadButton() || context.controllerNumber != 0 || context.buttonMap.isConnectAsPinned()) return false;
        if (!context.external) return true;
        // Decided once, so the pad's type does not flip when another pad joins later.
        if (context.handheldControls == null) context.handheldControls = isHandheldControls(context.inputDevice);
        return context.handheldControls;
    }

    /** Whether a gamepad Android lists as external is the handheld's own controls (see {@link #looksLikeHandheldControls}). */
    private boolean isHandheldControls(InputDevice device) {
        if (device == null) return false;
        int gamepads = 0;
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice other = InputDevice.getDevice(id);
            if (other != null && !other.isVirtual() && hasJoystickAxes(other)) gamepads++;
        }
        boolean attachedUsb = false;
        UsbManager usbManager = (UsbManager) activityContext.getSystemService(Context.USB_SERVICE);
        if (usbManager != null) {
            for (UsbDevice usb : usbManager.getDeviceList().values()) {
                if (usb.getVendorId() == device.getVendorId() && usb.getProductId() == device.getProductId()) attachedUsb = true;
            }
        }
        return looksLikeHandheldControls(device.getName(), device.getVendorId(), gamepads == 1, attachedUsb);
    }

    /**
     * Whether a gamepad Android lists as external is a handheld's built-in controls. A Retroid Pocket's is
     * "Xbox Wireless Controller" with its maker's vendor ID (0x2022:0x3002) on an internal bus. So: named Xbox
     * without Microsoft's vendor ID, the only gamepad, and absent from the USB device list, which is where an
     * attached pad appears. A real Xbox pad has Microsoft's ID, and a USB pad is in the list.
     */
    static boolean looksLikeHandheldControls(String name, int vendorId, boolean onlyGamepad, boolean attachedUsb) {
        return NightjarInputJni.looksLikeHandheldControls(name, vendorId, onlyGamepad, attachedUsb);
    }

    /**
     * Whether the on-screen controller announces itself as a PlayStation pad. Only for its
     * Touchpad button, as {@link #carriesTouchpadButton} does for built-in controls. Without the
     * button it stays quiet and the host makes its default pad as before; a pad in player 1's
     * slot announces player 1 instead.
     */
    private boolean announcesOnscreenPad() {
        boolean hasSlotZeroDevice = false;
        if (prefConfig.sendControllers) {
            for (InputDeviceContext context : inputDeviceContexts.values()) {
                if (context.assignedControllerNumber && context.controllerNumber == 0) {
                    hasSlotZeroDevice = true;
                    break;
                }
            }
            if (!hasSlotZeroDevice) {
                for (UsbDeviceContext context : usbDeviceContexts.values()) {
                    if (context.assignedControllerNumber && context.controllerNumber == 0) {
                        hasSlotZeroDevice = true;
                        break;
                    }
                }
            }
        }
        return NightjarInputJni.announcesOnscreenPad(
                slotStateHandle, prefConfig.sendControllers, prefConfig.onscreenController,
                layoutGamepadUsed, hasSlotZeroDevice);
    }

    private void sendOnscreenPadArrival() {
        conn.sendControllerArrivalEvent((byte) 0, getActiveControllerMask(), onscreenPadType(),
                onscreenPadButtons(), onscreenPadCapabilities(steamControlsSession));
    }

    // Its own player without the Touchpad button: the host's default pad, announced for its number.
    private byte onscreenPadType() {
        return NightjarInputJni.onscreenPadType(showsTouchpadButton());
    }

    private int onscreenPadButtons() {
        return onscreenPadButtons(showsTouchpadButton(), steamControlsSession);
    }

    /** The on-screen controller's buttons for its arrival: the Touchpad button and the back buttons with their settings. */
    static int onscreenPadButtons(boolean touchpadButton, boolean steamControls) {
        return NightjarInputJni.onscreenPadButtons(touchpadButton, steamControls);
    }

    /** The on-screen controller's capabilities for its arrival: none but the Steam Controller controls'. */
    static short onscreenPadCapabilities(boolean steamControls) {
        return NightjarInputJni.onscreenPadCapabilities(steamControls);
    }

    /**
     * Whether a pad in this slot carries the on-screen Steam Controller controls in its arrival:
     * player 1's, when the on-screen controls share it rather than being their own player.
     */
    static boolean carriesSteamControls(boolean steamControls, boolean onscreenOwnPlayer, short controllerNumber) {
        return NightjarInputJni.carriesSteamControls(steamControls, onscreenOwnPlayer, controllerNumber);
    }

    /** A pad's arrival capabilities with the on-screen Steam Controller controls it carries; a pin to Xbox still drops the touchpad. */
    private short withSteamControls(short controllerNumber, short capabilities, ControllerButtonMap map) {
        return NightjarInputJni.withSteamControlsCapabilities(
                steamControlsSession, onscreenOwnPlayerSession, controllerNumber, capabilities, map.connectAs());
    }

    private int withSteamControlButtons(short controllerNumber, int buttons) {
        return NightjarInputJni.withSteamControlsButtons(
                steamControlsSession, onscreenOwnPlayerSession, controllerNumber, buttons);
    }

    private synchronized short getActiveControllerMask() {
        return NightjarInputJni.activeControllerMask(
                slotStateHandle, prefConfig.sendControllers, prefConfig.multiController,
                prefConfig.onscreenController, layoutGamepadUsed, hasBuiltInPlayerOne());
    }

    /**
     * What the user's button map sends for this key on its pad: a target key code,
     * {@link ControllerButtonMap#DISABLED}, or {@link ControllerButtonMap#UNMAPPED}.
     */
    public synchronized int userMappedKey(KeyEvent event) {
        InputDeviceContext context = inputDeviceContexts.get(event.getDeviceId());
        return context == null ? ControllerButtonMap.UNMAPPED : context.buttonMap.map(event.getKeyCode());
    }

    /**
     * The type a pad arrives as. A type the user pinned for the pad wins. Otherwise only
     * PlayStation pads have a touchpad click, so a mapping to it asks the host for one: hosts
     * choose a DualShock 4 or DualSense for this type.
     */
    static byte arrivalType(byte type, ControllerButtonMap map) {
        return NightjarInputJni.arrivalType(type, map.connectAs(), map.targetButtonFlags());
    }

    /**
     * The type an Android input device arrives as. With Automatic, a non-PlayStation pad with
     * motion arrives as unknown so the host creates a pad that has motion; a pinned type wins.
     */
    static byte inputDeviceArrivalType(byte detected, boolean hasMotion, ControllerButtonMap map) {
        return NightjarInputJni.inputDeviceArrivalType(detected, hasMotion, map.connectAs(), map.targetButtonFlags());
    }

    /**
     * Whether a pad pinned as Xbox leaves out motion and the touchpad. An Xbox pad has neither, and
     * hosts such as Rubylight turn any pad that reports them into a DualSense, which would undo the pin.
     */
    static boolean dropsMotionAndTouchpad(ControllerButtonMap map) {
        return NightjarInputJni.dropsMotionAndTouchpad(map.connectAs());
    }

    /** The capabilities a pad arrives with: without motion and touchpad when pinned as Xbox. */
    static short arrivalCapabilities(short capabilities, ControllerButtonMap map) {
        return NightjarInputJni.arrivalCapabilities(capabilities, map.connectAs());
    }

    /**
     * Whether a USB pad streams this device's motion sensors: player 1 with "Use device motion
     * sensors" on, when the pad has none of its own and isn't pinned as Xbox.
     */
    static boolean takesDeviceMotion(boolean fallbackToDevice, short controllerNumber, short padCapabilities,
                                     ControllerButtonMap map) {
        return NightjarInputJni.takesDeviceMotion(fallbackToDevice, controllerNumber, padCapabilities, map.connectAs());
    }

    /** Whether a pad already streams this device's motion sensors, which only one pad may. */
    private boolean deviceSensorsInUse() {
        if (deviceSensorManager == null) return false;
        for (InputDeviceContext context : inputDeviceContexts.values()) {
            if (context.sensorManager == deviceSensorManager) return true;
        }
        for (UsbDeviceContext context : usbDeviceContexts.values()) {
            if (context.deviceSensors != null) return true;
        }
        return false;
    }

    /** Whether motion changes the type the pad arrives as, which the user is told about. */
    static boolean motionOverridesType(byte detected, boolean hasMotion, ControllerButtonMap map) {
        return NightjarInputJni.motionOverridesType(detected, hasMotion, map.connectAs());
    }

    /**
     * Whether Select+LB stands in for the touchpad click on a pad faked as PlayStation for motion.
     * A button mapped to the touchpad already clicks it, so the combo would only swallow Select and LB.
     */
    static boolean needsClickpadEmulation(ControllerButtonMap map) {
        return NightjarInputJni.needsClickpadEmulation(map.targetButtonFlags());
    }

    static boolean updateMappedButton(Map<Integer, Integer> pressedButtons, int physicalKey, int target, boolean down) {
        int normalized = NightjarInputJni.normalizeMappedTarget(target);
        if (normalized == 0) {
            return true;
        }
        if (down) {
            boolean alreadyPressed = pressedButtons.containsValue(normalized);
            pressedButtons.put(physicalKey, normalized);
            return !alreadyPressed;
        }
        pressedButtons.remove(physicalKey);
        return !pressedButtons.containsValue(normalized);
    }

    /**
     * The buttons held on a key-event pad, like the USB path's raw report: the clickpad emulation
     * swaps Select+LB for the touchpad in inputMap, so this swaps them back from pressedButtons.
     */
    static int heldButtons(InputDeviceContext context) {
        if ((context.emulatingButtonFlags & EMULATING_TOUCHPAD) == 0) {
            return context.inputMap;
        }
        return NightjarInputJni.heldButtons(
                context.inputMap, context.emulatingButtonFlags,
                context.pressedButtons.containsValue(ControllerButtonMap.TARGET_TOUCHPAD),
                context.pressedButtons.containsValue(KeyEvent.KEYCODE_BUTTON_SELECT),
                context.pressedButtons.containsValue(KeyEvent.KEYCODE_BUTTON_L1));
    }

    private static boolean areBatteryCapacitiesEqual(float first, float second) {
        // With no NaNs involved, it is a simple equality comparison.
        if (!Float.isNaN(first) && !Float.isNaN(second)) {
            return first == second;
        }
        else {
            // If we have a NaN in one or both positions, compare NaN-ness instead.
            // Equality comparisons will always return false for NaN.
            return Float.isNaN(first) == Float.isNaN(second);
        }
    }

    /** What a controller's battery reported, in BatteryState's terms. */
    private static final class BatteryReading {
        final int status;
        final float capacity;

        BatteryReading(int status, float capacity) {
            this.status = status;
            this.capacity = capacity;
        }
    }

    // A Binder call that can block for seconds (long enough for an ANR on a OnePlus Pad 2), so
    // never on the main thread and never holding a lock the main thread takes. Null: no battery.
    @SuppressLint("InlinedApi")
    private BatteryReading readControllerBattery(InputDeviceContext context) {
        int currentBatteryStatus;
        float currentBatteryCapacity;

        // Use the BatteryState object introduced in Android S, if it's available and present.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            BatteryState batteryState = context.inputDevice.getBatteryState();
            if (batteryState.isPresent()) {
                return new BatteryReading(batteryState.getStatus(), batteryState.getCapacity());
            }
        }

        if (sceManager.isRecognizedDevice(context.inputDevice)) {
            // On the SHIELD Android TV, we can use a proprietary API to access battery/charge state.
            // We will convert it to the same form used by BatteryState to share code.
            int batteryPercentage = sceManager.getBatteryPercentage(context.inputDevice);
            if (batteryPercentage < 0) {
                currentBatteryCapacity = Float.NaN;
            }
            else {
                currentBatteryCapacity = batteryPercentage / 100.f;
            }

            SceConnectionType connectionType = sceManager.getConnectionType(context.inputDevice);
            SceChargingState chargingState = sceManager.getChargingState(context.inputDevice);

            // We can make some assumptions about charge state based on the connection type
            if (connectionType == SceConnectionType.WIRED || connectionType == SceConnectionType.BOTH) {
                if (batteryPercentage == 100) {
                    currentBatteryStatus = BatteryState.STATUS_FULL;
                }
                else if (chargingState == SceChargingState.NOT_CHARGING) {
                    currentBatteryStatus = BatteryState.STATUS_NOT_CHARGING;
                }
                else {
                    currentBatteryStatus = BatteryState.STATUS_CHARGING;
                }
            }
            else if (connectionType == SceConnectionType.WIRELESS) {
                if (chargingState == SceChargingState.CHARGING) {
                    currentBatteryStatus = BatteryState.STATUS_CHARGING;
                }
                else {
                    currentBatteryStatus = BatteryState.STATUS_DISCHARGING;
                }
            }
            else {
                // If connection type is unknown, just use the charge state
                if (batteryPercentage == 100) {
                    currentBatteryStatus = BatteryState.STATUS_FULL;
                }
                else if (chargingState == SceChargingState.NOT_CHARGING) {
                    currentBatteryStatus = BatteryState.STATUS_DISCHARGING;
                }
                else if (chargingState == SceChargingState.CHARGING) {
                    currentBatteryStatus = BatteryState.STATUS_CHARGING;
                }
                else {
                    currentBatteryStatus = BatteryState.STATUS_UNKNOWN;
                }
            }
        }
        else {
            return null;
        }
        return new BatteryReading(currentBatteryStatus, currentBatteryCapacity);
    }

    @SuppressLint("InlinedApi")
    private void sendControllerBatteryPacket(InputDeviceContext context, BatteryReading battery) {
        // Controllers Off: the PC has no pad for it.
        if (!prefConfig.sendControllers) return;
        int currentBatteryStatus = battery.status;
        float currentBatteryCapacity = battery.capacity;

        if (currentBatteryStatus != context.lastReportedBatteryStatus ||
                !areBatteryCapacitiesEqual(currentBatteryCapacity, context.lastReportedBatteryCapacity)) {
            byte state;
            byte percentage;

            switch (currentBatteryStatus) {
                case BatteryState.STATUS_UNKNOWN:
                    state = MoonBridge.LI_BATTERY_STATE_UNKNOWN;
                    break;

                case BatteryState.STATUS_CHARGING:
                    state = MoonBridge.LI_BATTERY_STATE_CHARGING;
                    break;

                case BatteryState.STATUS_DISCHARGING:
                    state = MoonBridge.LI_BATTERY_STATE_DISCHARGING;
                    break;

                case BatteryState.STATUS_NOT_CHARGING:
                    state = MoonBridge.LI_BATTERY_STATE_NOT_CHARGING;
                    break;

                case BatteryState.STATUS_FULL:
                    state = MoonBridge.LI_BATTERY_STATE_FULL;
                    break;

                default:
                    return;
            }

            if (Float.isNaN(currentBatteryCapacity)) {
                percentage = MoonBridge.LI_BATTERY_PERCENTAGE_UNKNOWN;
            }
            else {
                percentage = (byte)(currentBatteryCapacity * 100);
            }

            conn.sendControllerBatteryEvent((byte)context.controllerNumber, state, percentage);

            context.lastReportedBatteryStatus = currentBatteryStatus;
            context.lastReportedBatteryCapacity = currentBatteryCapacity;
        }
    }

    private int packFusionContext(int count, int buttons, byte lt, byte rt, short lx, short ly, short rx, short ry) {
        int base = count * 6;
        if (base + 6 > fuseScratch.length) {
            fuseScratch = java.util.Arrays.copyOf(fuseScratch, fuseScratch.length * 2);
        }
        fuseScratch[base] = buttons;
        fuseScratch[base + 1] = (lt & 0xFF) | ((rt & 0xFF) << 8);
        fuseScratch[base + 2] = lx;
        fuseScratch[base + 3] = ly;
        fuseScratch[base + 4] = rx;
        fuseScratch[base + 5] = ry;
        return count + 1;
    }

    private void sendControllerInputPacket(GenericControllerContext originalContext) {
        // Controllers set to Off: no physical controller reaches the PC (the on-screen gamepad still does).
        boolean physical = prefConfig.sendControllers;
        if (!physical && originalContext != defaultContext) return;
        assignControllerNumberIfNeeded(originalContext);
        if (!originalContext.assignedControllerNumber) {
            return;
        }
        for (UsbDeviceContext context : physical ? usbDeviceContexts.values() : Collections.<UsbDeviceContext>emptyList()) {
            if (context.assignedControllerNumber && context.controllerNumber == originalContext.controllerNumber &&
                    !context.ensureControllerArrival() && (context == originalContext ||
                    !NightjarInputJni.slotHasInput(slotStateHandle, originalContext.controllerNumber))) {
                // A new slot must wait for its USB capabilities. A context already
                // using a shared slot must still be able to release its held input.
                return;
            }
        }

        // Take the context's controller number and fuse all inputs with the same number
        short controllerNumber = originalContext.controllerNumber;
        int inputMap;
        byte leftTrigger;
        byte rightTrigger;
        short leftStickX;
        short leftStickY;
        short rightStickX;
        short rightStickY;

        synchronized (fuseLock) {
            int count = 0;
            for (GenericControllerContext context : physical ? inputDeviceContexts.values() : Collections.<GenericControllerContext>emptyList()) {
                if (context.assignedControllerNumber &&
                        context.controllerNumber == controllerNumber &&
                        context.mouseEmulationActive == originalContext.mouseEmulationActive) {
                    context.axisMap.apply(context.inputMap, context.leftTrigger, context.rightTrigger,
                            context.leftStickX, context.leftStickY, context.rightStickX, context.rightStickY, context.mappedReport);
                    ControllerAxisMap.Report mapped = context.mappedReport;
                    count = packFusionContext(count, mapped.buttons, mapped.lt, mapped.rt,
                            mapped.lx, mapped.ly, mapped.rx, mapped.ry);
                }
            }
            for (UsbDeviceContext context : physical ? usbDeviceContexts.values() : Collections.<UsbDeviceContext>emptyList()) {
                if (context.assignedControllerNumber && !context.arrivalPending &&
                        context.controllerNumber == controllerNumber &&
                        context.mouseEmulationActive == originalContext.mouseEmulationActive) {
                    context.axisMap.apply(context.inputMap, context.leftTrigger, context.rightTrigger,
                            context.leftStickX, context.leftStickY, context.rightStickX, context.rightStickY, context.mappedReport);
                    ControllerAxisMap.Report mapped = context.mappedReport;
                    count = packFusionContext(count, mapped.buttons, mapped.lt, mapped.rt,
                            mapped.lx, mapped.ly, mapped.rx, mapped.ry);
                }
            }
            if (defaultContext.controllerNumber == controllerNumber) {
                count = packFusionContext(count, defaultContext.inputMap, defaultContext.leftTrigger, defaultContext.rightTrigger,
                        defaultContext.leftStickX, defaultContext.leftStickY, defaultContext.rightStickX, defaultContext.rightStickY);
                count = packFusionContext(count, layoutInputMap, layoutLeftTrigger, layoutRightTrigger,
                        layoutLeftStickX, layoutLeftStickY, layoutRightStickX, layoutRightStickY);
            }
            if (physical && handheldContext != defaultContext && handheldContext.controllerNumber == controllerNumber) {
                count = packFusionContext(count, handheldContext.inputMap, handheldContext.leftTrigger, handheldContext.rightTrigger,
                        handheldContext.leftStickX, handheldContext.leftStickY, handheldContext.rightStickX, handheldContext.rightStickY);
            }
            NightjarInputJni.fuseControllerInputs(
                    fuseScratch, count, steamControlsSession, controllerNumber,
                    steamPadTouchX, originalContext.mouseEmulationActive, fuseOut);
            inputMap = fuseOut[0];
            leftTrigger = (byte) fuseOut[1];
            rightTrigger = (byte) fuseOut[2];
            leftStickX = (short) fuseOut[3];
            leftStickY = (short) fuseOut[4];
            rightStickX = (short) fuseOut[5];
            rightStickY = (short) fuseOut[6];
        }

        short activeControllerMask = getActiveControllerMask();
        if (originalContext.mouseEmulationActive) {
            int changedMask = inputMap ^  originalContext.mouseEmulationLastInputMap;

            boolean aDown = (inputMap & ControllerPacket.A_FLAG) != 0;
            boolean bDown = (inputMap & ControllerPacket.B_FLAG) != 0;

            originalContext.mouseEmulationLastInputMap = inputMap;

            if (prefConfig.mouseEmulationFixedStep) {
                originalContext.mouseEmulationFixedStepHeld = (inputMap & ControllerPacket.X_FLAG) != 0;
                if ((changedMask & inputMap & ControllerPacket.Y_FLAG) != 0) {
                    originalContext.mouseEmulationStep = FixedStepMouse.nextStep(originalContext.mouseEmulationStep);
                    Toast.makeText(activityContext, activityContext.getString(R.string.stream_mouse_step,
                            originalContext.mouseEmulationStep), Toast.LENGTH_SHORT).show();
                }
            }

            if ((changedMask & ControllerPacket.A_FLAG) != 0) {
                if (aDown) {
                    conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_LEFT);
                }
                else {
                    conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
                }
            }
            if ((changedMask & ControllerPacket.B_FLAG) != 0) {
                if (bDown) {
                    conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
                }
                else {
                    conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
                }
            }
            if ((changedMask & ControllerPacket.UP_FLAG) != 0) {
                if ((inputMap & ControllerPacket.UP_FLAG) != 0) {
                    conn.sendMouseScroll((byte) 1);
                }
            }
            if ((changedMask & ControllerPacket.DOWN_FLAG) != 0) {
                if ((inputMap & ControllerPacket.DOWN_FLAG) != 0) {
                    conn.sendMouseScroll((byte) -1);
                }
            }
            if ((changedMask & ControllerPacket.RIGHT_FLAG) != 0) {
                if ((inputMap & ControllerPacket.RIGHT_FLAG) != 0) {
                    conn.sendMouseHScroll((byte) 1);
                }
            }
            if ((changedMask & ControllerPacket.LEFT_FLAG) != 0) {
                if ((inputMap & ControllerPacket.LEFT_FLAG) != 0) {
                    conn.sendMouseHScroll((byte) -1);
                }
            }

            conn.sendControllerInput(controllerNumber, activeControllerMask,
                    (short)0, (byte)0, (byte)0, (short)0, (short)0, (short)0, (short)0);
        }
        else {
            conn.sendControllerInput(controllerNumber, activeControllerMask,
                    inputMap,
                    leftTrigger, rightTrigger,
                    leftStickX, leftStickY,
                    rightStickX, rightStickY);
        }
        if (conn.canSendControllerInput() && (activeControllerMask & (1 << controllerNumber)) != 0) {
            NightjarInputJni.markSlotHasInput(slotStateHandle, controllerNumber);
            controllerSlotsWithInput.set(controllerNumber, 1);
        }
    }

    private final int REMAP_CONSUME = -2;

    // Return a valid keycode, -2 to consume, or -1 to not consume the event
    // Device MAY BE NULL
    private int handleRemapping(InputDeviceContext context, KeyEvent event) {
        int quirks = 0;
        if (context.ignoreBack) quirks |= NightjarInputJni.QUIRK_IGNORE_BACK;
        if (context.hasShare) quirks |= NightjarInputJni.QUIRK_HAS_SHARE;
        if (context.isDualShockStandaloneTouchpad) quirks |= NightjarInputJni.QUIRK_DS4_STANDALONE_TOUCHPAD;
        if (context.usesLinuxGamepadStandardFaceButtons) quirks |= NightjarInputJni.QUIRK_LINUX_STD_FACE;
        if (context.isNonStandardDualShock4) quirks |= NightjarInputJni.QUIRK_NON_STD_DS4;
        if (context.isServal) quirks |= NightjarInputJni.QUIRK_SERVAL;
        if (context.isNonStandardXboxBtController) quirks |= NightjarInputJni.QUIRK_NON_STD_XBOX_BT;
        if (context.hatXAxis == -1 && context.hatYAxis == -1) quirks |= NightjarInputJni.QUIRK_NO_HAT;
        if (context.backIsStart) quirks |= NightjarInputJni.QUIRK_BACK_IS_START;
        if (context.modeIsSelect) quirks |= NightjarInputJni.QUIRK_MODE_IS_SELECT;
        if (context.searchIsMode) quirks |= NightjarInputJni.QUIRK_SEARCH_IS_MODE;

        int mapped = NightjarInputJni.remapKeyEvent(
                prefConfig.backAsGuide, event.getKeyCode(), event.getScanCode(),
                event.getFlags(), event.hasNoModifiers(), context.vendorId, context.productId,
                Build.VERSION.SDK_INT, quirks, remapOut);
        if ((remapOut[0] & 1) != 0) {
            context.backIsStart = false;
        }
        if ((remapOut[0] & 2) != 0) {
            context.modeIsSelect = false;
        }
        return mapped;
    }

    private int handleFlipFaceButtons(int keyCode) {
        return NightjarInputJni.flipFaceButtonKeyCode(keyCode);
    }

    // A fixed 1% centre zone always remains so a resting stick reads as neutral and packet
    // batching keeps working.
    static final double MIN_STICK_DEADZONE = 0.01;

    // Trigger deadzone from the settings (2% by default, enough for a resting trigger to read as
    // released so packet batching keeps working). Moonlight used a fixed 13% (XInput's threshold
    // for treating a trigger as a button), which cut off the first part of the travel. Like the
    // stick deadzone, travel past it is sent raw: the host and game apply their own.
    // Pads whose triggers sit far forward and false-press never go below 30%.
    static final float QUIRK_TRIGGER_DEADZONE = 0.30f;

    static float applyTriggerDeadzone(float value, float deadzone) {
        return NightjarInputJni.applyTriggerDeadzone(value, deadzone);
    }

    // Positive radius: inputs inside it are dropped. We're not normalizing here because we let the
    // computer handle the deadzones. Normalizing can make the deadzones larger than they should
    // be after the computer also evaluates the deadzone.
    // Zero or negative radius (anti-deadzone, as in Artemis): the magnitude is remapped from
    // [0.01, 1] to [|radius|, 1], so small movements jump past a controller's or game's own deadzone.
    static void applyStickDeadzone(Vector2d stickVector, float deadzoneRadius) {
        float[] out = new float[2];
        NightjarInputJni.applyStickDeadzone(stickVector.getX(), stickVector.getY(), deadzoneRadius, out);
        stickVector.initialize(out[0], out[1]);
    }

    private void handleAxisSet(InputDeviceContext context, float lsX, float lsY, float rsX,
                               float rsY, float lt, float rt, float hatX, float hatY) {
        NightjarInputJni.normalizeAxisSet(
                lsX, lsY, rsX, rsY, lt, rt, hatX, hatY,
                context.leftStickXAxis != -1 && context.leftStickYAxis != -1,
                context.rightStickXAxis != -1 && context.rightStickYAxis != -1,
                context.leftTriggerAxis != -1 && context.rightTriggerAxis != -1,
                context.hatXAxis != -1 && context.hatYAxis != -1,
                context.leftStickDeadzoneRadius,
                context.rightStickDeadzoneRadius,
                context.triggerDeadzone,
                context.triggersIdleNegative,
                context.leftTriggerAxisUsed,
                context.rightTriggerAxisUsed,
                context.hatXAxisUsed,
                context.hatYAxisUsed,
                context.inputMap,
                axisSetOut);
        context.inputMap = axisSetOut[0];
        context.leftTrigger = (byte) axisSetOut[1];
        context.rightTrigger = (byte) axisSetOut[2];
        context.leftStickX = (short) axisSetOut[3];
        context.leftStickY = (short) axisSetOut[4];
        context.rightStickX = (short) axisSetOut[5];
        context.rightStickY = (short) axisSetOut[6];
        int usedBits = axisSetOut[7];
        context.leftTriggerAxisUsed = (usedBits & 1) != 0;
        context.rightTriggerAxisUsed = (usedBits & 2) != 0;
        context.hatXAxisUsed = (usedBits & 4) != 0;
        context.hatYAxisUsed = (usedBits & 8) != 0;

        sendControllerInputPacket(context);
    }

    // Normalize the given raw float value into a 0.0-1.0f range
    private float normalizeRawValueWithRange(float value, InputDevice.MotionRange range) {
        value = Math.max(value, range.getMin());
        value = Math.min(value, range.getMax());

        value -= range.getMin();

        return value / range.getRange();
    }

    private boolean sendTouchpadEventForPointer(InputDeviceContext context, MotionEvent event, byte touchType, int pointerIndex) {
        float normalizedX = normalizeRawValueWithRange(event.getX(pointerIndex), context.touchpadXRange);
        float normalizedY = normalizeRawValueWithRange(event.getY(pointerIndex), context.touchpadYRange);
        float normalizedPressure = context.touchpadPressureRange != null ?
                normalizeRawValueWithRange(event.getPressure(pointerIndex), context.touchpadPressureRange)
                : 0;

        if (steamControlsSession && context.controllerNumber == 0) steamPadTouchX = normalizedX;
        return conn.sendControllerTouchEvent((byte)context.controllerNumber, touchType,
                event.getPointerId(pointerIndex),
                normalizedX, normalizedY, normalizedPressure) != MoonBridge.LI_ERR_UNSUPPORTED;
    }

    public boolean tryHandleTouchpadEvent(MotionEvent event) {
        // Bail if this is not a touchpad or mouse event
        if (event.getSource() != InputDevice.SOURCE_TOUCHPAD &&
                event.getSource() != InputDevice.SOURCE_MOUSE) {
            return false;
        }

        // Only get a context if one already exists. We want to ensure we don't report non-gamepads.
        InputDeviceContext context = inputDeviceContexts.get(event.getDeviceId());
        if (context == null) {
            return false;
        }

        // When we're working with a mouse source instead of a touchpad, we're quite limited in
        // what useful input we can provide via the controller API. The ABS_X/ABS_Y values are
        // screen coordinates rather than touchpad coordinates. For now, we will just support
        // the clickpad button and nothing else.
        if (event.getSource() == InputDevice.SOURCE_MOUSE) {
            // Unlike the touchpad where down and up refer to individual touches on the touchpad,
            // down and up on a mouse indicates the state of the left mouse button.
            switch (event.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    context.inputMap |= ControllerPacket.TOUCHPAD_FLAG;
                    sendControllerInputPacket(context);
                    break;
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL:
                    context.inputMap &= ~ControllerPacket.TOUCHPAD_FLAG;
                    sendControllerInputPacket(context);
                    break;
                default:
                    break;
            }

            return !prefConfig.gamepadTouchpadAsMouse;
        }

        byte touchType;
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN:
                touchType = MoonBridge.LI_TOUCH_EVENT_DOWN;
                break;

            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
                if ((event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) {
                    touchType = MoonBridge.LI_TOUCH_EVENT_CANCEL;
                }
                else {
                    touchType = MoonBridge.LI_TOUCH_EVENT_UP;
                }
                break;

            case MotionEvent.ACTION_MOVE:
                touchType = MoonBridge.LI_TOUCH_EVENT_MOVE;
                break;

            case MotionEvent.ACTION_CANCEL:
                // ACTION_CANCEL applies to *all* pointers in the gesture, so it maps to CANCEL_ALL
                // rather than CANCEL. For a single pointer cancellation, that's indicated via
                // FLAG_CANCELED on a ACTION_POINTER_UP.
                // https://developer.android.com/develop/ui/views/touch-and-input/gestures/multi
                touchType = MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL;
                break;

            case MotionEvent.ACTION_BUTTON_PRESS:
                if (event.getActionButton() == MotionEvent.BUTTON_PRIMARY) {
                    context.inputMap |= ControllerPacket.TOUCHPAD_FLAG;
                    sendControllerInputPacket(context);
                    return !prefConfig.gamepadTouchpadAsMouse; // Report as unhandled event to trigger mouse handling
                }
                return false;

            case MotionEvent.ACTION_BUTTON_RELEASE:
                if (event.getActionButton() == MotionEvent.BUTTON_PRIMARY) {
                    context.inputMap &= ~ControllerPacket.TOUCHPAD_FLAG;
                    sendControllerInputPacket(context);
                    return !prefConfig.gamepadTouchpadAsMouse; // Report as unhandled event to trigger mouse handling
                }
                return false;

            default:
                return false;
        }

        // Bail if the user wants gamepad touchpads to control the mouse
        //
        // NB: We do this after processing ACTION_BUTTON_PRESS and ACTION_BUTTON_RELEASE
        // because we want to still send the touchpad button via the gamepad even when
        // configured to use the touchpad for mouse control.
        if (prefConfig.gamepadTouchpadAsMouse) {
            return false;
        }

        // A pad pinned as Xbox told the host it has no touchpad
        if (dropsMotionAndTouchpad(context.buttonMap) || !prefConfig.sendControllers) {
            return true;
        }

        // If we don't have X and Y ranges, we can't process this event
        if (context.touchpadXRange == null || context.touchpadYRange == null) {
            return false;
        }

        if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            // Move events may impact all active pointers
            for (int i = 0; i < event.getPointerCount(); i++) {
                if (!sendTouchpadEventForPointer(context, event, touchType, i)) {
                    // Controller touch events are not supported by the host
                    return false;
                }
            }
            return true;
        }
        else if (event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
            // Cancel impacts all active pointers
            return conn.sendControllerTouchEvent((byte)context.controllerNumber, MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL,
                    0, 0, 0, 0) != MoonBridge.LI_ERR_UNSUPPORTED;
        }
        else {
            // Down and Up events impact the action index pointer
            return sendTouchpadEventForPointer(context, event, touchType, event.getActionIndex());
        }
    }

    public boolean handleMotionEvent(MotionEvent event) {
        InputDeviceContext context = getContextForEvent(event);
        if (context == null) {
            return true;
        }

        float lsX = 0, lsY = 0, rsX = 0, rsY = 0, rt = 0, lt = 0, hatX = 0, hatY = 0;

        // We purposefully ignore the historical values in the motion event as it makes
        // the controller feel sluggish for some users.

        if (context.leftStickXAxis != -1 && context.leftStickYAxis != -1) {
            lsX = event.getAxisValue(context.leftStickXAxis);
            lsY = event.getAxisValue(context.leftStickYAxis);
        }

        if (context.rightStickXAxis != -1 && context.rightStickYAxis != -1) {
            rsX = event.getAxisValue(context.rightStickXAxis);
            rsY = event.getAxisValue(context.rightStickYAxis);
        }

        if (context.leftTriggerAxis != -1 && context.rightTriggerAxis != -1) {
            lt = event.getAxisValue(context.leftTriggerAxis);
            rt = event.getAxisValue(context.rightTriggerAxis);
        }

        if (context.hatXAxis != -1 && context.hatYAxis != -1) {
            hatX = event.getAxisValue(MotionEvent.AXIS_HAT_X);
            hatY = event.getAxisValue(MotionEvent.AXIS_HAT_Y);
        }

        if (hatX != 0 || hatY != 0) {
            // A D-pad chord with a waiting Guide (Guide+D-pad) sends Guide first
            sendHeldGuide(context);
        }

        handleAxisSet(context, lsX, lsY, rsX, rsY, lt, rt, hatX, hatY);

        return true;
    }

    private Vector2d convertRawStickAxisToPixelMovement(short stickX, short stickY) {
        float[] out = new float[2];
        NightjarInputJni.mouseStickCurve(stickX, stickY, out);
        Vector2d vector = new Vector2d();
        vector.initialize(out[0], out[1]);
        return vector;
    }

    // fixedStep is 0 for movement that follows how far the stick is pushed.
    private void sendEmulatedMouseMove(short x, short y, int fixedStep) {
        Vector2d vector = convertRawStickAxisToPixelMovement(x, y);
        if (vector.getMagnitude() >= 1) {
            if (fixedStep > 0) {
                int dx = FixedStepMouse.delta(vector.getX(), fixedStep);
                int dy = FixedStepMouse.delta(-vector.getY(), fixedStep);
                if (dx != 0 || dy != 0) {
                    conn.sendMouseMove((short)dx, (short)dy);
                }
            }
            else {
                conn.sendMouseMove((short)vector.getX(), (short)-vector.getY());
            }
        }
    }

    private void sendEmulatedMouseScroll(short x, short y) {
        Vector2d vector = convertRawStickAxisToPixelMovement(x, y);
        if (vector.getMagnitude() >= 1) {
            conn.sendMouseHighResScroll((short)vector.getY());
            conn.sendMouseHighResHScroll((short)vector.getX());
        }
    }

    @TargetApi(31)
    private boolean hasDualAmplitudeControlledRumbleVibrators(VibratorManager vm) {
        int[] vibratorIds = vm.getVibratorIds();

        // There must be exactly 2 vibrators on this device
        if (vibratorIds.length != 2) {
            return false;
        }

        // Both vibrators must have amplitude control
        for (int vid : vibratorIds) {
            if (!vm.getVibrator(vid).hasAmplitudeControl()) {
                return false;
            }
        }

        return true;
    }

    // This must only be called if hasDualAmplitudeControlledRumbleVibrators() is true!
    @TargetApi(31)
    private void rumbleDualVibrators(VibratorManager vm, short lowFreqMotor, short highFreqMotor) {
        lowFreqMotor = feel(lowFreqMotor);
        highFreqMotor = feel(highFreqMotor);
        // Normalize motor values to 0-255 amplitudes for VibrationManager
        highFreqMotor = (short)((highFreqMotor >> 8) & 0xFF);
        lowFreqMotor = (short)((lowFreqMotor >> 8) & 0xFF);

        // If they're both zero, we can just call cancel().
        if (lowFreqMotor == 0 && highFreqMotor == 0) {
            vm.cancel();
            return;
        }

        // There's no documentation that states that vibrators for FF_RUMBLE input devices will
        // always be enumerated in this order, but it seems consistent between Xbox Series X (USB),
        // PS3 (USB), and PS4 (USB+BT) controllers on Android 12 Beta 3.
        int[] vibratorIds = vm.getVibratorIds();
        int[] vibratorAmplitudes = new int[] { highFreqMotor, lowFreqMotor };

        CombinedVibration.ParallelCombination combo = CombinedVibration.startParallel();

        for (int i = 0; i < vibratorIds.length; i++) {
            // It's illegal to create a VibrationEffect with an amplitude of 0.
            // Simply excluding that vibrator from our ParallelCombination will turn it off.
            if (vibratorAmplitudes[i] != 0) {
                combo.addVibrator(vibratorIds[i], VibrationEffect.createOneShot(60000, vibratorAmplitudes[i]));
            }
        }

        VibrationAttributes.Builder vibrationAttributes = new VibrationAttributes.Builder();

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            vibrationAttributes.setUsage(VibrationAttributes.USAGE_MEDIA);
        }

        vm.vibrate(combo.combine(), vibrationAttributes.build());
    }

    @TargetApi(31)
    private boolean hasQuadAmplitudeControlledRumbleVibrators(VibratorManager vm) {
        int[] vibratorIds = vm.getVibratorIds();

        // There must be exactly 4 vibrators on this device
        if (vibratorIds.length != 4) {
            return false;
        }

        // All vibrators must have amplitude control
        for (int vid : vibratorIds) {
            if (!vm.getVibrator(vid).hasAmplitudeControl()) {
                return false;
            }
        }

        return true;
    }

    // This must only be called if hasQuadAmplitudeControlledRumbleVibrators() is true!
    @TargetApi(31)
    private void rumbleQuadVibrators(VibratorManager vm, short lowFreqMotor, short highFreqMotor, short leftTrigger, short rightTrigger) {
        lowFreqMotor = feel(lowFreqMotor);
        highFreqMotor = feel(highFreqMotor);
        leftTrigger = feel(leftTrigger);
        rightTrigger = feel(rightTrigger);
        // Normalize motor values to 0-255 amplitudes for VibrationManager
        highFreqMotor = (short)((highFreqMotor >> 8) & 0xFF);
        lowFreqMotor = (short)((lowFreqMotor >> 8) & 0xFF);
        leftTrigger = (short)((leftTrigger >> 8) & 0xFF);
        rightTrigger = (short)((rightTrigger >> 8) & 0xFF);

        // If they're all zero, we can just call cancel().
        if (lowFreqMotor == 0 && highFreqMotor == 0 && leftTrigger == 0 && rightTrigger == 0) {
            vm.cancel();
            return;
        }

        // This is a guess based upon the behavior of FF_RUMBLE, but untested due to lack of Linux
        // support for trigger rumble!
        int[] vibratorIds = vm.getVibratorIds();
        int[] vibratorAmplitudes = new int[] { highFreqMotor, lowFreqMotor, leftTrigger, rightTrigger };

        CombinedVibration.ParallelCombination combo = CombinedVibration.startParallel();

        for (int i = 0; i < vibratorIds.length; i++) {
            // It's illegal to create a VibrationEffect with an amplitude of 0.
            // Simply excluding that vibrator from our ParallelCombination will turn it off.
            if (vibratorAmplitudes[i] != 0) {
                combo.addVibrator(vibratorIds[i], VibrationEffect.createOneShot(60000, vibratorAmplitudes[i]));
            }
        }

        VibrationAttributes.Builder vibrationAttributes = new VibrationAttributes.Builder();

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            vibrationAttributes.setUsage(VibrationAttributes.USAGE_MEDIA);
        }

        vm.vibrate(combo.combine(), vibrationAttributes.build());
    }

    /** A motor level after the chosen rumble feel. */
    private short feel(short level) {
        return RumblePolicy.shapeMotor(level, rumbleFeel);
    }

    private void rumbleSingleVibrator(Vibrator vibrator, short lowFreqMotor, short highFreqMotor) {
        rumbleSingleVibrator(vibrator, lowFreqMotor, highFreqMotor, 100);
    }

    private void rumbleSingleVibrator(Vibrator vibrator, short lowFreqMotor, short highFreqMotor, int strengthPercent) {
        // Since we can only use a single amplitude value, scale each motor by the strength, mix them
        // (80% of the big one, 33% of the small one) and shape the result by the rumble feel.
        // NB: This value is 0-255 as required by VibrationEffect.
        int simulatedAmplitude = RumblePolicy.singleMotorAmplitude(lowFreqMotor, highFreqMotor, rumbleFeel, strengthPercent);

        if (simulatedAmplitude == 0) {
            // This case is easy - just cancel the current effect and get out.
            // NB: We cannot simply check lowFreqMotor == highFreqMotor == 0
            // because our simulatedAmplitude could be 0 even though our inputs
            // are not (ex: lowFreqMotor == 0 && highFreqMotor == 1).
            vibrator.cancel();
            return;
        }

        // Attempt to use amplitude-based control if we're on Oreo and the device
        // supports amplitude-based vibration control.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            if (vibrator.hasAmplitudeControl()) {
                VibrationEffect effect = VibrationEffect.createOneShot(60000, simulatedAmplitude);
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    VibrationAttributes vibrationAttributes = new VibrationAttributes.Builder()
                            .setUsage(VibrationAttributes.USAGE_MEDIA)
                            .build();
                    vibrator.vibrate(effect, vibrationAttributes);
                }
                else {
                    AudioAttributes audioAttributes = new AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_GAME)
                            .build();
                    vibrator.vibrate(effect, audioAttributes);
                }
                return;
            }
        }

        // If we reach this point, we don't have amplitude controls available, so
        // we must emulate it by PWMing the vibration. Ick.
        long[] pwm = RumblePolicy.pwmTimings(simulatedAmplitude, rumbleFeel);
        long onTime = pwm[0];
        long offTime = pwm[1];
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            VibrationAttributes vibrationAttributes = new VibrationAttributes.Builder()
                    .setUsage(VibrationAttributes.USAGE_MEDIA)
                    .build();
            vibrator.vibrate(VibrationEffect.createWaveform(new long[]{0, onTime, offTime}, 0), vibrationAttributes);
        }
        else {
            AudioAttributes audioAttributes = new AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_GAME)
                    .build();
            vibrator.vibrate(new long[]{0, onTime, offTime}, 0, audioAttributes);
        }
    }

    public void handleRumble(short controllerNumber, short lowFreqMotor, short highFreqMotor) {
        synchronized (rumbleLock) {
            rumble(controllerNumber, lowFreqMotor, highFreqMotor);
        }
    }

    private void rumble(short controllerNumber, short lowFreqMotor, short highFreqMotor) {
        boolean foundMatchingDevice = false;
        boolean vibrated = false;

        if (stopped || !rumbleAllowed) {
            return;
        }
        short[] levels = levelsOf(controllerNumber);
        levels[0] = lowFreqMotor;
        levels[1] = highFreqMotor;

        for (InputDeviceContext deviceContext : inputDeviceContexts.values()) {

            if (deviceContext.assignedControllerNumber && deviceContext.controllerNumber == controllerNumber) {
                foundMatchingDevice = true;

                // Also while rumble is paused: a trigger update right after it resumes re-sends both
                // motor levels, and must not bring back the ones from before the pause.
                deviceContext.lowFreqMotor = lowFreqMotor;
                deviceContext.highFreqMotor = highFreqMotor;
                if (rumblePaused) continue;

                // Prefer the documented Android 12 rumble API which can handle dual vibrators on PS/Xbox controllers
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && deviceContext.vibratorManager != null) {
                    vibrated = true;
                    if (deviceContext.quadVibrators) {
                        rumbleQuadVibrators(deviceContext.vibratorManager,
                                deviceContext.lowFreqMotor, deviceContext.highFreqMotor,
                                deviceContext.leftTriggerMotor, deviceContext.rightTriggerMotor);
                    }
                    else {
                        rumbleDualVibrators(deviceContext.vibratorManager,
                                deviceContext.lowFreqMotor, deviceContext.highFreqMotor);
                    }
                }
                // On Shield devices, we can use their special API to rumble Shield controllers
                else if (sceManager.rumble(deviceContext.inputDevice, feel(deviceContext.lowFreqMotor), feel(deviceContext.highFreqMotor))) {
                    vibrated = true;
                }
                // If all else fails, we have to try the old Vibrator API
                else if (deviceContext.vibrator != null) {
                    vibrated = true;
                    rumbleSingleVibrator(deviceContext.vibrator, deviceContext.lowFreqMotor, deviceContext.highFreqMotor);
                }
                // stop() does not wait for these calls, and one it ran across would keep a motor on for a minute.
                if (stopped) deviceContext.cancelMotors();
            }
        }
        if (rumblePaused) return;

        List<AbstractController> usbPads = usbPadsOf(controllerNumber);
        if (usbPads == null) {
            return;
        }
        if (!usbPads.isEmpty()) {
            foundMatchingDevice = vibrated = true;
            synchronized (usbRumbleLock) {
                if (stopped) {
                    return;
                }
                for (AbstractController pad : usbPads) {
                    pad.rumble(feel(lowFreqMotor), feel(highFreqMotor));
                }
            }
        }

        // We may decide to rumble the device for player 1, or the first pad after on-screen controls of their own
        if (controllerNumber == 0 || controllerNumber == deviceFallbackSlot) {
            // If we didn't find a matching device, it must be the on-screen
            // controls that triggered the rumble. Vibrate the device if
            // the user has requested that behavior.
            if (controllerNumber == 0 && !foundMatchingDevice && prefConfig.onscreenController && !prefConfig.onlyL3R3 && prefConfig.vibrateOsc) {
                rumbleSingleVibrator(deviceVibrator, lowFreqMotor, highFreqMotor);
            }
            else if (controllerNumber == deviceFallbackSlot && foundMatchingDevice && !vibrated && prefConfig.vibrateFallbackToDevice) {
                // We found a device to vibrate but it didn't have rumble support. The user
                // has requested us to vibrate the device in this case.

                deviceVibratorForPlayerOne = true;
                rumbleSingleVibrator(deviceVibrator, lowFreqMotor, highFreqMotor, prefConfig.vibrateFallbackToDeviceStrength);
            }
            if (stopped) deviceVibrator.cancel();
        }
    }

    public void handleRumbleTriggers(short controllerNumber, short leftTrigger, short rightTrigger) {
        synchronized (rumbleLock) {
            rumbleTriggers(controllerNumber, leftTrigger, rightTrigger);
        }
    }

    private void rumbleTriggers(short controllerNumber, short leftTrigger, short rightTrigger) {
        if (stopped || !rumbleAllowed) {
            return;
        }
        short[] levels = levelsOf(controllerNumber);
        levels[2] = leftTrigger;
        levels[3] = rightTrigger;

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            for (InputDeviceContext deviceContext : inputDeviceContexts.values()) {

                if (deviceContext.assignedControllerNumber && deviceContext.controllerNumber == controllerNumber) {
                    deviceContext.leftTriggerMotor = leftTrigger;
                    deviceContext.rightTriggerMotor = rightTrigger;
                    if (rumblePaused) continue;

                    if (deviceContext.quadVibrators) {
                        rumbleQuadVibrators(deviceContext.vibratorManager,
                                deviceContext.lowFreqMotor, deviceContext.highFreqMotor,
                                deviceContext.leftTriggerMotor, deviceContext.rightTriggerMotor);
                        if (stopped) deviceContext.cancelMotors();
                    }
                }
            }
        }
        if (rumblePaused) {
            return;
        }

        List<AbstractController> usbPads = usbPadsOf(controllerNumber);
        if (usbPads == null || usbPads.isEmpty()) {
            return;
        }
        synchronized (usbRumbleLock) {
            if (stopped) {
                return;
            }
            for (AbstractController pad : usbPads) {
                pad.rumbleTriggers(feel(leftTrigger), feel(rightTrigger));
            }
        }
    }

    /** The USB pads that play as this controller, or null once the handler has stopped. */
    private synchronized List<AbstractController> usbPadsOf(short controllerNumber) {
        if (stopped) {
            return null;
        }
        List<AbstractController> pads = java.util.Collections.emptyList();
        for (UsbDeviceContext deviceContext : usbDeviceContexts.values()) {
            if (deviceContext.assignedControllerNumber && deviceContext.controllerNumber == controllerNumber) {
                if (pads.isEmpty()) {
                    pads = new ArrayList<>(1);
                }
                pads.add(deviceContext.device);
            }
        }
        return pads;
    }

    // Called only on the connection worker. Give the USB service time to finish its
    // permission/driver handover before the host starts a game that caches devices.
    public String prepareControllersForLaunch() {
        if (!conn.canSendControllerInput()) return "";
        long deadline = android.os.SystemClock.uptimeMillis() + 2500;
        do {
            int attachedDualSenses = prefConfig.usbDualSenseHaptics ? countAttachedDualSenses() : 0;
            synchronized (this) {
                if (stopped || !conn.canSendControllerInput()) return "";
                int handedOver = 0;
                for (UsbDeviceContext context : usbDeviceContexts.values()) {
                    if (isDualSense(context.vendorId, context.productId)) handedOver++;
                }
                // Only a DualSense the USB service has not handed over yet can still become ready.
                if (handedOver >= attachedDualSenses) break;
            }
            try { Thread.sleep(50); }
            catch (InterruptedException e) { Thread.currentThread().interrupt(); return ""; }
        } while (android.os.SystemClock.uptimeMillis() < deadline);
        synchronized (this) {
            if (stopped || !conn.canSendControllerInput()) return "";
            prepareInputDevices();
            String[] controllers = new String[MAX_GAMEPADS];
            // Controllers Off: no physical pad is announced, so the PC makes no pad that never sends.
            boolean physical = prefConfig.sendControllers;
            for (InputDeviceContext context : physical ? inputDeviceContexts.values() : Collections.<InputDeviceContext>emptyList()) {
                // Built-in controls share player 1 with an external pad; the external pad's type wins.
                if (context.assignedControllerNumber &&
                        (context.external || controllers[context.controllerNumber] == null)) {
                    controllers[context.controllerNumber] = launchController(context.controllerNumber,
                            context.reportedType, context.reportedCapabilities, context.supportedButtonFlags);
                }
            }
            for (UsbDeviceContext context : usbDeviceContexts.values()) {
                assignControllerNumberIfNeeded(context);
                if (!physical || !context.assignedControllerNumber) continue;
                short caps = context.capabilities();
                if (!prefConfig.gamepadMotionSensors) caps &= ~(MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO);
                controllers[context.controllerNumber] = launchController(context.controllerNumber,
                        context.arrivalType(), withSteamControls(context.controllerNumber, caps, context.buttonMap),
                        withSteamControlButtons(context.controllerNumber,
                                context.device.getSupportedButtonFlags() | context.buttonMap.targetButtonFlags()));
            }
            if (controllers[0] == null && announcesOnscreenPad()) {
                controllers[0] = launchController((short) 0, onscreenPadType(),
                        onscreenPadCapabilities(steamControlsSession), onscreenPadButtons());
            }
            StringBuilder result = new StringBuilder();
            for (String controller : controllers) {
                if (controller == null) continue;
                if (result.length() != 0) result.append(',');
                result.append(controller);
            }
            return result.toString();
        }
    }

    private static String launchController(short slot, byte type, short capabilities, int buttons) {
        return NightjarInputJni.formatLaunchController(slot, type, capabilities, buttons);
    }

    private InputDeviceContext prepareInputDevice(int deviceId) {
        InputDevice device = InputDevice.getDevice(deviceId);
        if (device == null || (!hasJoystickAxes(device) && !hasGamepadButtons(device))) return null;
        // Built-in or virtual devices that merely advertise gamepad keys (#29, #30) get a
        // number when they first send gamepad input, never ahead of the real pads.
        if (!hasJoystickAxes(device) && !isExternal(device)) return null;
        InputDeviceContext context = inputDeviceContexts.get(deviceId);
        if (context == null) {
            context = createInputDeviceContextForDevice(device);
            inputDeviceContexts.put(deviceId, context);
        }
        assignControllerNumberIfNeeded(context);
        return context;
    }

    private void prepareInputDevices() {
        // External pads with sticks first, then button-only ones (a remote advertising gamepad
        // keys lands after any real pad), then USB pads, then built-in controls, which share
        // player 1 and so must find the external pads already in place.
        for (int pass = 0; pass < 3; pass++) {
            if (pass == 2) {
                for (UsbDeviceContext context : usbDeviceContexts.values()) assignControllerNumberIfNeeded(context);
            }
            for (int id : InputDevice.getDeviceIds()) {
                InputDevice device = InputDevice.getDevice(id);
                if (device == null) continue;
                boolean external = isExternal(device);
                boolean sticks = hasJoystickAxes(device);
                if (NightjarInputJni.launchPassForInputDevice(external, sticks) == pass) {
                    prepareInputDevice(id);
                }
            }
        }
    }

    private static boolean isDualSense(int vendorId, int productId) {
        return vendorId == 0x054c && (productId == 0x0ce6 || productId == 0x0df2);
    }

    private int countAttachedDualSenses() {
        UsbManager usbManager = (UsbManager) activityContext.getSystemService(Context.USB_SERVICE);
        if (usbManager == null) return 0;
        int count = 0;
        for (UsbDevice device : usbManager.getDeviceList().values()) {
            // A pad still at the USB permission prompt, or whose driver failed, cannot arrive in time.
            if (DualSenseController.canClaimDevice(device) && usbManager.hasPermission(device) &&
                    !failedUsbDevices.contains(device.getDeviceId())) count++;
        }
        return count;
    }

    @Override
    public void deviceStartFailed(int usbDeviceId) {
        failedUsbDevices.add(usbDeviceId);
    }

    public synchronized void resendControllerArrivals() {
        if (stopped || !conn.canSendControllerInput()) return;
        connectionStarted = true;
        if (onscreenOwnPlayerSession) {
            // Before any pad, so the PC's own numbering starts with it too. The neutral state is
            // for hosts that plug a controller at its first input instead of at its arrival.
            sendOnscreenPadArrival();
            sendControllerInputPacket(defaultContext);
        }
        for (InputDeviceContext context : inputDeviceContexts.values()) {
            if (context.assignedControllerNumber) context.sendControllerArrival();
        }
        for (UsbDeviceContext context : usbDeviceContexts.values()) {
            if (context.assignedControllerNumber) context.sendControllerArrival();
        }
        // Newly assigned pads announce themselves here.
        prepareInputDevices();
        if (!onscreenOwnPlayerSession && announcesOnscreenPad()) sendOnscreenPadArrival();
        for (InputDeviceContext context : inputDeviceContexts.values()) {
            if (context.assignedControllerNumber) sendControllerInputPacket(context);
        }
        for (UsbDeviceContext context : usbDeviceContexts.values()) {
            sendControllerInputPacket(context);
        }
    }

    public void handleControllerAudio(int kind, byte[] payload) {
        if (stopped || !conn.canSendControllerInput() || payload == null || payload.length < 4) return;
        short id = (short)((payload[2] & 255) | ((payload[3] & 255) << 8));
        AbstractController controller = waveformControllers.get(id);
        if (controller != null) controller.controllerAudio(kind, payload);
    }
    // The audio renderer's headset route; it reports itself inactive until a DualSense with USB
    // audio is announced, so ordinary streams skip the volume lookups entirely.
    public AndroidAudioRenderer.HeadphoneSink headphoneAudioSink() {
        return new AndroidAudioRenderer.HeadphoneSink() {
            @Override public boolean write(short[] pcm, int length, float volume) {
                return routeHeadphoneAudio(pcm, length, volume);
            }

            @Override public boolean active() {
                return !stopped && !waveformControllers.isEmpty() && conn.canSendControllerInput();
            }
        };
    }

    public boolean routeHeadphoneAudio(short[] samples, int length, float volume) {
        if (stopped || !conn.canSendControllerInput()) return false;
        // A single headset owns normal game sound; controller-specific effects remain per-pad.
        for (short id = 0; id < 16; id++) {
            AbstractController controller = waveformControllers.get(id);
            if (controller != null && controller.headphoneAudio(samples, length, volume)) return true;
        }
        return false;
    }
    public void handleControllerHaptics(short controllerNumber, int sequence, byte[] pcm) {
        if (stopped || !rumbleAllowed || !conn.canSendControllerInput()) return;
        // Avoid the input monitor and HID transfers on the control receive thread.
        AbstractController controller = waveformControllers.get(controllerNumber);
        if (controller != null) controller.controllerHaptics(sequence, pcm);
    }

    public synchronized void handleSetAdaptiveTriggers(short controllerNumber, byte eventFlags, byte typeLeft,
                                          byte typeRight, byte[] left, byte[] right) {
        if (stopped) {
            return;
        }
        for (UsbDeviceContext context : usbDeviceContexts.values()) {
            if (context.assignedControllerNumber && context.controllerNumber == controllerNumber) {
                context.device.setAdaptiveTriggers(eventFlags, typeLeft, typeRight, left, right);
            }
        }
    }

    // Player 1's controller gyroscope gets a liveness check when its demand may move to the device
    // gyroscope, the way Moonlight V+ does it.
    boolean checksGyroLiveness(short controllerNumber, SensorManager controllerSensors) {
        return controllerNumber == deviceFallbackSlot && controllerSensors != deviceSensorManager &&
                prefConfig.gamepadMotionSensorsFallbackToDevice && deviceSensorManager != null &&
                deviceSensorManager.getDefaultSensor(Sensor.TYPE_GYROSCOPE) != null;
    }

    // A controller gyroscope that reports only zeros hands both motion sensors to the device's own.
    // The new context keeps the device sensors through a reconfiguration, like the plain fallback.
    void fallBackToDeviceSensors(InputDeviceContext context) {
        if (stopped || context.destroyed || context.sensorManager == deviceSensorManager) {
            return;
        }
        LimeLog.warning("Gyroscope of " + context.name + " reports only zeros; using the device's motion sensors");
        context.disableSensors();
        context.sensorManager = deviceSensorManager;
        context.enableSensorRunnable.run();
    }

    private MotionListener createSensorListener(final short controllerNumber, final byte motionType,
                                                final boolean needsDeviceOrientationCorrection,
                                                final InputDeviceContext livenessContext) {
        return createSensorListener(controllerNumber, motionType, needsDeviceOrientationCorrection, livenessContext, null);
    }

    /** {@code gyroMapping} is a controller's own gyroscope's mapping; null for accelerometers and this device's sensors. */
    private MotionListener createSensorListener(final short controllerNumber, final byte motionType,
                                                final boolean needsDeviceOrientationCorrection,
                                                final InputDeviceContext livenessContext,
                                                final GyroAxisTransformer gyroMapping) {
        return new MotionListener(controllerNumber, motionType, needsDeviceOrientationCorrection, livenessContext,
                gyroMapping);
    }

    /**
     * One motion sensor of one pad. Its events arrive on {@link #motionSensorThread}, in the order
     * the sensor produced them, so everything here that the main thread also touches is either
     * volatile or under {@link #motionSendLock}.
     */
    final class MotionListener implements SensorEventListener {
        private final short controllerNumber;
        private final byte motionType;
        private final boolean needsDeviceOrientationCorrection;
        private final InputDeviceContext livenessContext;
        // Settings › Gyroscope mapping for a controller's gyroscope, fixed when the listener starts.
        private final GyroAxisTransformer gyroMapping;
        // Only the thread delivering the events reads and writes these.
        private final float[] lastValues = new float[3];
        private final float[] mapped = new float[3];
        private final GyroLivenessTracker liveness;
        // Set by closeMotionListener() on the thread that unregisters this listener.
        volatile boolean closed;
        // Whether this listener holds the display rotation; guarded by the DeviceRotation monitor.
        boolean holdsRotation;

        MotionListener(short controllerNumber, byte motionType, boolean needsDeviceOrientationCorrection,
                       InputDeviceContext livenessContext, GyroAxisTransformer gyroMapping) {
            this.controllerNumber = controllerNumber;
            this.motionType = motionType;
            this.needsDeviceOrientationCorrection = needsDeviceOrientationCorrection;
            this.livenessContext = livenessContext;
            this.gyroMapping = gyroMapping;
            this.liveness = livenessContext != null ? new GyroLivenessTracker() : null;
        }

        @Override
        public void onSensorChanged(SensorEvent sensorEvent) {
            if (closed || stopped || !sensorsEnabled || !inputEnabled || !prefConfig.sendControllers) {
                return;
            }
            // Before the duplicate check, which drops the repeated zeros this looks for.
            if (liveness != null && liveness.onSample(sensorEvent.values[0], sensorEvent.values[1],
                    sensorEvent.values[2], sensorEvent.timestamp)) {
                mainThreadHandler.post(() -> fallBackToDeviceSensors(livenessContext));
            }
            // Android will invoke our callback any time we get a new reading,
            // even if the values are the same as last time. Don't report a
            // duplicate set of values to save bandwidth.
            if (sensorEvent.values[0] == lastValues[0] &&
                    sensorEvent.values[1] == lastValues[1] &&
                    sensorEvent.values[2] == lastValues[2]) {
                return;
            }
            else {
                lastValues[0] = sensorEvent.values[0];
                lastValues[1] = sensorEvent.values[1];
                lastValues[2] = sensorEvent.values[2];
            }

            int x = 0;
            int y = 1;
            int z = 2;
            int xFactor = 1;
            int yFactor = 1;
            int zFactor = 1;

            if (needsDeviceOrientationCorrection) {
                int deviceRotation = displayRotation.rotation();
                switch (deviceRotation) {
                    case Surface.ROTATION_0:
                    case Surface.ROTATION_180:
                        x = 0;
                        y = 2;
                        z = 1;
                        break;

                    case Surface.ROTATION_90:
                    case Surface.ROTATION_270:
                        x = 1;
                        y = 2;
                        z = 0;
                        break;
                }

                switch (deviceRotation) {
                    case Surface.ROTATION_0:
                        zFactor = -1;
                        break;
                    case Surface.ROTATION_90:
                        xFactor = -1;
                        zFactor = -1;
                        break;
                    case Surface.ROTATION_180:
                        xFactor = -1;
                        break;
                    case Surface.ROTATION_270:
                        break;
                }
            }

            synchronized (motionSendLock) {
                // Asked again under the lock: the main thread may have ended this pad's motion and
                // sent its stationary report since the check above, and this sample must not follow it.
                if (closed || stopped || !sensorsEnabled || !inputEnabled || !prefConfig.sendControllers) {
                    return;
                }
                if (motionType == MoonBridge.LI_MOTION_TYPE_GYRO) {
                    // Convert from rad/s to deg/s
                    float gx = sensorEvent.values[x] * xFactor * 57.2957795f;
                    float gy = sensorEvent.values[y] * yFactor * 57.2957795f;
                    float gz = sensorEvent.values[z] * zFactor * 57.2957795f;
                    GyroAxisTransformer mapping = gyroMapping == null ? null
                            : GyroDiagnosticOverlay.isFlydigiForced() ? GyroMappingPreferences.FLYDIGI_TRANSFORM
                            : gyroMapping;
                    if (mapping != null && !mapping.isIdentity()) {
                        mapping.transform(gx, gy, gz, mapped);
                        gx = mapped[0];
                        gy = mapped[1];
                        gz = mapped[2];
                    }
                    if (GyroDiagnosticOverlay.isEnabled()) {
                        GyroDiagnosticOverlay.report("sensor", controllerNumber, motionType, gx, gy, gz);
                    }
                    conn.sendControllerMotionEvent((byte) controllerNumber, motionType, gx, gy, gz);
                }
                else {
                    // Pass m/s^2 directly without conversion
                    float ax = sensorEvent.values[x] * xFactor;
                    float ay = sensorEvent.values[y] * yFactor;
                    float az = sensorEvent.values[z] * zFactor;
                    if (GyroDiagnosticOverlay.isEnabled()) {
                        GyroDiagnosticOverlay.report("sensor", controllerNumber, motionType, ax, ay, az);
                    }
                    conn.sendControllerMotionEvent((byte) controllerNumber, motionType, ax, ay, az);
                }
            }
        }

        @Override
        public void onAccuracyChanged(Sensor sensor, int accuracy) {}
    }

    public void handleSetMotionEventState(final short controllerNumber, final byte motionType, short reportRateHz) {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            final short rate = reportRateHz;
            mainThreadHandler.post(() -> handleSetMotionEventState(controllerNumber, motionType, rate));
            return;
        }
        if (stopped) {
            return;
        }

        for (UsbDeviceContext context : usbDeviceContexts.values()) {
            if (context.assignedControllerNumber && context.controllerNumber == controllerNumber) {
                short rate = prefConfig.gamepadMotionSensors && !dropsMotionAndTouchpad(context.buttonMap) ? reportRateHz : 0;
                if (context.deviceSensors != null) {
                    // Report rate is restricted to <= 200 Hz without the HIGH_SAMPLING_RATE_SENSORS permission
                    context.setDeviceMotion(motionType, (short) Math.min(200, rate & 0xFFFF));
                }
                else {
                    context.device.setMotionEventState(motionType, rate);
                }
            }
        }

        // Report rate is restricted to <= 200 Hz without the HIGH_SAMPLING_RATE_SENSORS permission
        reportRateHz = (short) Math.min(200, reportRateHz & 0xFFFF);

        for (InputDeviceContext deviceContext : inputDeviceContexts.values()) {

            if (deviceContext.assignedControllerNumber && deviceContext.controllerNumber == controllerNumber) {
                // Store the desired report rate even if we don't have sensors. In some cases,
                // input devices can be reconfigured at runtime which results in a change where
                // sensors disappear and reappear. By storing the desired report rate, we can
                // reapply the desired motion sensor configuration after they reappear.
                switch (motionType) {
                    case MoonBridge.LI_MOTION_TYPE_ACCEL:
                        deviceContext.accelReportRateHz = reportRateHz;
                        break;
                    case MoonBridge.LI_MOTION_TYPE_GYRO:
                        deviceContext.gyroReportRateHz = reportRateHz;
                        break;
                }

                mainThreadHandler.removeCallbacks(deviceContext.enableSensorRunnable);

                SensorManager sm = deviceContext.sensorManager;
                if (sm == null) {
                    continue;
                }

                switch (motionType) {
                    case MoonBridge.LI_MOTION_TYPE_ACCEL:
                        if (deviceContext.accelListener != null) {
                            closeMotionListener(sm, deviceContext.accelListener);
                            deviceContext.accelListener = null;
                        }

                        // Enable the accelerometer if requested
                        Sensor accelSensor = sm.getDefaultSensor(Sensor.TYPE_ACCELEROMETER);
                        if (sensorsEnabled && reportRateHz != 0 && accelSensor != null) {
                            deviceContext.accelListener = createSensorListener(controllerNumber, motionType, sm == deviceSensorManager, null);
                            registerMotionListener(sm, accelSensor, deviceContext.accelListener, 1000000 / reportRateHz);
                        }
                        break;
                    case MoonBridge.LI_MOTION_TYPE_GYRO:
                        if (deviceContext.gyroListener != null) {
                            closeMotionListener(sm, deviceContext.gyroListener);
                            deviceContext.gyroListener = null;
                        }

                        // Enable the gyroscope if requested
                        Sensor gyroSensor = sm.getDefaultSensor(Sensor.TYPE_GYROSCOPE);
                        if (sensorsEnabled && reportRateHz != 0 && gyroSensor != null) {
                            deviceContext.gyroListener = createSensorListener(controllerNumber, motionType, sm == deviceSensorManager,
                                    checksGyroLiveness(controllerNumber, sm) ? deviceContext : null,
                                    sm == deviceSensorManager ? null
                                            : GyroMappingPreferences.transformer(activityContext, deviceContext.inputDevice));
                            registerMotionListener(sm, gyroSensor, deviceContext.gyroListener, 1000000 / reportRateHz);
                        }
                        break;
                }
                break;
            }
        }
    }

    public synchronized void handleSetControllerLED(short controllerNumber, byte r, byte g, byte b) {
        if (stopped) {
            return;
        }

        for (UsbDeviceContext context : usbDeviceContexts.values()) {
            if (context.assignedControllerNumber && context.controllerNumber == controllerNumber) {
                context.device.setLed(r, g, b);
            }
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            for (InputDeviceContext deviceContext : inputDeviceContexts.values()) {

                // Ignore input devices without an RGB LED
                if (deviceContext.assignedControllerNumber && deviceContext.controllerNumber == controllerNumber && deviceContext.hasRgbLed) {
                    // Convert the RGB components into the integer value that LightState uses
                    deviceContext.ledArgbValue = 0xFF000000 | ((r << 16) & 0xFF0000) | ((g << 8) & 0xFF00) | (b & 0xFF);

                    backgroundThreadHandler.removeCallbacks(deviceContext.setLedStateRunnable);
                    backgroundThreadHandler.post(deviceContext.setLedStateRunnable);
                }
            }
        }
    }

    public boolean handleButtonUp(KeyEvent event) {
        InputDeviceContext context = getContextForEvent(event);
        if (context == null) {
            return true;
        }

        int keyCode = handleRemapping(context, event);
        if (keyCode < 0) {
            return (keyCode == REMAP_CONSUME);
        }

        context.quitButtons &= ~quitButton(keyCode);
        releaseQuitCombo(context, context.quitButtons);

        if (keyCode == KeyEvent.KEYCODE_BUTTON_MODE && !context.guideTapping) {
            GuideHold.Release release = releaseGuide(context);
            if (release == GuideHold.Release.TAP) {
                tapGuide(context, event);
            }
            if (release != GuideHold.Release.PASS) {
                return true;
            }
        }

        // A user mapping is keyed by the physical key code and replaces face-button flipping
        int userTarget = context.buttonMap.map(event.getKeyCode());
        if (userTarget == ControllerButtonMap.DISABLED) {
            return true;
        }
        else if (userTarget != ControllerButtonMap.UNMAPPED) {
            keyCode = userTarget;
        }
        else if (prefConfig.flipFaceButtons) {
            keyCode = handleFlipFaceButtons(keyCode);
        }

        int physicalKey = event.getScanCode() != 0 ? event.getScanCode() : ~event.getKeyCode();
        if (!updateMappedButton(context.pressedButtons, physicalKey, keyCode, false)) {
            return true;
        }

        int finalKeyCode = keyCode;
        long eventTime = event.getEventTime();
        int scanCode = event.getScanCode();
        Runnable previous = context.pendingButtonReleases.remove(finalKeyCode);
        if (previous != null) {
            mainThreadHandler.removeCallbacks(previous);
        }
        long remaining = MINIMUM_BUTTON_DOWN_TIME_MS - (eventTime - event.getDownTime());
        if (remaining > 0 && isHandledButtonUp(context, finalKeyCode, scanCode)) {
            Runnable release = new Runnable() {
                @Override
                public void run() {
                    if (context.pendingButtonReleases.get(finalKeyCode) == this) {
                        context.pendingButtonReleases.remove(finalKeyCode);
                    }
                    if (context.destroyed || stopped) {
                        return;
                    }
                    finishButtonUp(context, finalKeyCode, eventTime, scanCode);
                }
            };
            context.pendingButtonReleases.put(finalKeyCode, release);
            mainThreadHandler.postDelayed(release, remaining);
            return true;
        }

        return finishButtonUp(context, finalKeyCode, eventTime, scanCode);
    }

    private static boolean isHandledButtonUp(InputDeviceContext context, int keyCode, int scanCode) {
        return NightjarInputJni.isHandledGamepadKey(keyCode, scanCode, context.hasPaddles);
    }

    private boolean finishButtonUp(InputDeviceContext context, int keyCode, long eventTime, int scanCode) {
        NightjarInputJni.buttonUpStep(
                context.inputMap, context.leftTrigger, context.rightTrigger,
                context.hasPaddles, context.hatXAxisUsed, context.hatYAxisUsed,
                context.leftTriggerAxisUsed, context.rightTriggerAxisUsed,
                keyCode, scanCode, buttonStepOut);
        int status = buttonStepOut[0];
        if (status == 1) {
            return true;
        }
        if (status == 2) {
            return false;
        }
        int stepFlags = buttonStepOut[4];
        if ((stepFlags & 1) != 0 &&
                eventTime - context.startDownTime > ControllerHandler.START_DOWN_TIME_MOUSE_MODE_MS &&
                prefConfig.mouseEmulation) {
            context.toggleMouseEmulation();
        }
        context.inputMap = buttonStepOut[1];
        context.leftTrigger = (byte) buttonStepOut[2];
        context.rightTrigger = (byte) buttonStepOut[3];
        if ((stepFlags & 2) != 0) {
            context.lastLbUpTime = eventTime;
        }
        if ((stepFlags & 4) != 0) {
            context.lastRbUpTime = eventTime;
        }

        if ((context.emulatingButtonFlags & (EMULATING_SELECT | EMULATING_SPECIAL | EMULATING_TOUCHPAD)) != 0) {
            boolean hasTargetTouchpad = (context.emulatingButtonFlags & EMULATING_TOUCHPAD) != 0 &&
                    context.pressedButtons.containsValue(ControllerButtonMap.TARGET_TOUCHPAD);
            boolean hasSelectHeld = (context.emulatingButtonFlags & EMULATING_TOUCHPAD) != 0 &&
                    context.pressedButtons.containsValue(KeyEvent.KEYCODE_BUTTON_SELECT);
            boolean hasLbHeld = (context.emulatingButtonFlags & EMULATING_TOUCHPAD) != 0 &&
                    context.pressedButtons.containsValue(KeyEvent.KEYCODE_BUTTON_L1);
            NightjarInputJni.applyEmulatedCombosOnUp(
                    context.inputMap, context.emulatingButtonFlags,
                    hasTargetTouchpad, hasSelectHeld, hasLbHeld, comboOut);
            context.inputMap = comboOut[0];
            context.emulatingButtonFlags = comboOut[1];
        }

        sendControllerInputPacket(context);

        // pressedButtons too: after a clickpad press, a Select or LB still held is not in inputMap.
        if (context.pendingExit && context.inputMap == 0 && context.pressedButtons.isEmpty()) {
            context.pendingExit = false;
            if (activityContext instanceof Game) {
                ((Game) activityContext).showStreamMenuFromController();
            }
        }

        return true;
    }

    /**
     * Guide went down on this controller: returns whether Hold Guide to open the stream menu holds
     * the press back, starting the hold time for a new press (not for a key repeat).
     */
    private boolean holdGuide(GenericControllerContext context, boolean repeat) {
        boolean wasActive = context.guideHold.active();
        if (!context.guideHold.down(prefConfig.guideMenuHoldMs, repeat)) {
            mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
            return false;
        }
        if (!(repeat && wasActive)) {
            mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
            mainThreadHandler.postDelayed(context.guideHoldRunnable, prefConfig.guideMenuHoldMs);
        }
        return true;
    }

    private GuideHold.Release releaseGuide(GenericControllerContext context) {
        mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
        return context.guideHold.up();
    }

    // A held-back Guide let go before the hold time: the press goes out now through the usual
    // mapping, and its release after the minimum press so the PC sees it.
    private void tapGuide(InputDeviceContext context, KeyEvent up) {
        KeyEvent release = new KeyEvent(up);
        replayGuide(context, KeyEvent.changeAction(up, KeyEvent.ACTION_DOWN), true);
        Runnable end = new Runnable() {
            @Override
            public void run() {
                // Only this tap's own release: a newer tap has its own.
                if (context.guideTapEnd != this) return;
                context.guideTapEnd = null;
                if (!stopped) replayGuide(context, release, false);
            }
        };
        context.guideTapEnd = end;
        mainThreadHandler.postDelayed(end, MINIMUM_BUTTON_DOWN_TIME_MS);
    }

    // Sends a held-back Guide press or release through the usual path without holding it back again.
    private void replayGuide(InputDeviceContext context, KeyEvent event, boolean down) {
        context.guideTapping = true;
        try {
            if (down) handleButtonDown(event);
            else handleButtonUp(event);
        } finally {
            context.guideTapping = false;
        }
    }

    // The same for a handheld's system Back as Guide, which has no mapping.
    private void tapDefaultGuide(InputDeviceContext context) {
        context.inputMap |= ControllerPacket.SPECIAL_BUTTON_FLAG;
        sendControllerInputPacket(context);
        scheduleDefaultGuideRelease(context, MINIMUM_BUTTON_DOWN_TIME_MS);
    }

    private void scheduleDefaultGuideRelease(InputDeviceContext context, long delayMs) {
        Runnable end = new Runnable() {
            @Override
            public void run() {
                if (context.guideTapEnd != this) return;
                context.guideTapEnd = null;
                if (stopped) return;
                context.inputMap &= ~ControllerPacket.SPECIAL_BUTTON_FLAG;
                sendControllerInputPacket(context);
            }
        };
        context.guideTapEnd = end;
        mainThreadHandler.postDelayed(end, delayMs);
    }

    // A new Guide press while the last tap is still going out: that tap's release goes first, so the
    // new press is held back on its own and the old release cannot cut it short.
    private void endGuideTap(GenericControllerContext context) {
        Runnable end = context.guideTapEnd;
        if (end != null) {
            mainThreadHandler.removeCallbacks(end);
            end.run();
        }
    }

    /**
     * Another button or the D-pad went down while Guide waits for the hold time: a chord such as
     * Steam's Guide+RB. Guide goes to the host now, before that button, on this pad and on a
     * handheld's system Back as Guide (controller 0, or the built-in pad's with the on-screen controls on it).
     */
    private void sendHeldGuide(InputDeviceContext context) {
        sendHeldGuideOn(context);
        if (context != handheldContext) sendHeldGuideOn(handheldContext);
    }

    private void sendHeldGuideOn(InputDeviceContext context) {
        if (!context.guideHold.chord()) return;
        mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
        KeyEvent down = context.guideHeldDown;
        context.guideHeldDown = null;
        if (down != null) {
            replayGuide(context, down, true);
        }
        else {
            // A handheld's system Back as Guide, which has no mapping
            context.inputMap |= ControllerPacket.SPECIAL_BUTTON_FLAG;
            sendControllerInputPacket(context);
        }
    }

    /**
     * Hold Guide to open the stream menu on a pad the USB driver owns, whose reports carry Guide as
     * a flag: Guide stays out of them while held back, and a short press goes out as a tap. Not while
     * the stream menu or a capture has the pad: there Guide does what it always did.
     */
    private int guideHoldButtons(UsbDeviceContext context, int buttonFlags) {
        boolean guide = (buttonFlags & ControllerPacket.SPECIAL_BUTTON_FLAG) != 0;
        if (guide != context.guideDown) {
            context.guideDown = guide;
            if (guide) {
                if (inputEnabled && padCapture == null) holdGuide(context, false);
            }
            else if (releaseGuide(context) == GuideHold.Release.TAP) {
                // Ended by reportControllerState, on the driver's thread like every report
                context.guideTapping = true;
            }
        }
        // Guide pressed together with another button is a chord: both go out now. A button pressed
        // after Guide was already waiting got Guide sent first, by reportControllerState.
        if (guide && (buttonFlags & ~ControllerPacket.SPECIAL_BUTTON_FLAG) != 0 && context.guideHold.chord()) {
            mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
        }
        if (context.guideHold.active()) buttonFlags &= ~ControllerPacket.SPECIAL_BUTTON_FLAG;
        if (context.guideTapping) buttonFlags |= ControllerPacket.SPECIAL_BUTTON_FLAG;
        return buttonFlags;
    }

    public boolean handleButtonDown(KeyEvent event) {
        InputDeviceContext context = getContextForEvent(event);
        if (context == null) {
            return true;
        }

        int keyCode = handleRemapping(context, event);
        if (keyCode < 0) {
            return (keyCode == REMAP_CONSUME);
        }

        // Like the desktop's, the quit combo is the physical buttons, even remapped or disabled.
        context.quitButtons |= quitButton(keyCode);
        holdQuitCombo(context, context.quitButtons);

        // Hold Guide to open the stream menu, on the physical Guide (or Back as Guide) like the quit combo
        if (keyCode == KeyEvent.KEYCODE_BUTTON_MODE && !context.guideTapping) {
            boolean repeat = event.getRepeatCount() > 0;
            if (!repeat) endGuideTap(context);
            boolean wasActive = context.guideHold.active();
            if (holdGuide(context, repeat)) {
                if (!(repeat && wasActive)) context.guideHeldDown = new KeyEvent(event);
                context.hasMode = true;
                return true;
            }
        }
        else if (keyCode != KeyEvent.KEYCODE_BUTTON_MODE) {
            // A chord with a waiting Guide sends Guide first
            sendHeldGuide(context);
        }

        // A user mapping is keyed by the physical key code and replaces face-button flipping
        int userTarget = context.buttonMap.map(event.getKeyCode());
        if (userTarget == ControllerButtonMap.DISABLED) {
            return true;
        }
        else if (userTarget != ControllerButtonMap.UNMAPPED) {
            keyCode = userTarget;
        }
        else if (prefConfig.flipFaceButtons) {
            keyCode = handleFlipFaceButtons(keyCode);
        }

        Runnable pendingRelease = context.pendingButtonReleases.remove(keyCode);
        if (pendingRelease != null) {
            mainThreadHandler.removeCallbacks(pendingRelease);
            pendingRelease.run();
        }

        int physicalKey = event.getScanCode() != 0 ? event.getScanCode() : ~event.getKeyCode();
        if (!updateMappedButton(context.pressedButtons, physicalKey, keyCode, true)) {
            return true;
        }

        NightjarInputJni.buttonDownStep(
                context.inputMap, context.leftTrigger, context.rightTrigger,
                context.hasPaddles, context.hatXAxisUsed, context.hatYAxisUsed,
                context.leftTriggerAxisUsed, context.rightTriggerAxisUsed,
                keyCode, event.getScanCode(), buttonStepOut);
        int status = buttonStepOut[0];
        if (status == 1) {
            return true;
        }
        if (status == 2) {
            return false;
        }
        context.inputMap = buttonStepOut[1];
        context.leftTrigger = (byte) buttonStepOut[2];
        context.rightTrigger = (byte) buttonStepOut[3];
        int stepFlags = buttonStepOut[4];
        if ((stepFlags & 1) != 0) {
            context.hasSelect = true;
        }
        if ((stepFlags & 2) != 0) {
            context.hasMode = true;
        }
        if ((stepFlags & 4) != 0 && event.getRepeatCount() == 0) {
            context.startDownTime = event.getEventTime();
        }

        // Wait for the menu combo to lift so it cannot activate a menu item. It goes by the
        // held buttons: a clickpad press before Start+RB took Select and LB out of inputMap.
        if (heldButtons(context) == (ControllerPacket.BACK_FLAG | ControllerPacket.PLAY_FLAG |
                                     ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG)) {
            context.pendingExit = true;
        }

        NightjarInputJni.applyEmulatedCombosOnDown(
                context.inputMap, context.emulatingButtonFlags,
                context.hasSelect, context.hasMode, context.needsClickpadEmulation,
                event.getEventTime(), context.lastLbUpTime, context.lastRbUpTime, comboOut);
        context.inputMap = comboOut[0];
        context.emulatingButtonFlags = comboOut[1];

        // We don't need to send repeat key down events, but the platform
        // sends us events that claim to be repeats but they're from different
        // devices, so we just send them all and deal with some duplicates.
        sendControllerInputPacket(context);
        return true;
    }

    public void reportOscState(int buttonFlags,
                               short leftStickX, short leftStickY,
                               short rightStickX, short rightStickY,
                               byte leftTrigger, byte rightTrigger) {
        if (stopped) {
            return;
        }
        defaultContext.leftStickX = leftStickX;
        defaultContext.leftStickY = leftStickY;

        defaultContext.rightStickX = rightStickX;
        defaultContext.rightStickY = rightStickY;

        defaultContext.leftTrigger = leftTrigger;
        defaultContext.rightTrigger = rightTrigger;

        defaultContext.inputMap = buttonFlags;

        sendControllerInputPacket(defaultContext);
    }

    /**
     * A finger on one of the on-screen Steam Controller trackpads: touchpad 0 of the on-screen
     * controls' pad, which a PC with the Steam Controller type splits into its two pads.
     */
    public void reportOscTouch(byte eventType, int pointerId, float x, float y, float pressure) {
        if (stopped) {
            return;
        }
        conn.sendControllerTouchEvent((byte) defaultContext.controllerNumber, eventType, pointerId, x, y, pressure);
    }

    /**
     * The gamepad controls of the per-game touch layout. They join controller 1 the way the fixed
     * on-screen gamepad does, fused with it and with any pad sharing the slot.
     */
    public void reportLayoutGamepadState(int buttonFlags,
                                         short leftStickX, short leftStickY,
                                         short rightStickX, short rightStickY,
                                         byte leftTrigger, byte rightTrigger) {
        if (stopped) {
            return;
        }
        layoutGamepadUsed = true;
        layoutInputMap = buttonFlags;
        layoutLeftStickX = leftStickX;
        layoutLeftStickY = leftStickY;
        layoutRightStickX = rightStickX;
        layoutRightStickY = rightStickY;
        layoutLeftTrigger = leftTrigger;
        layoutRightTrigger = rightTrigger;
        sendControllerInputPacket(defaultContext);
    }

    @Override
    public void reportControllerState(int controllerId, int buttonFlags,
                                      float leftStickX, float leftStickY,
                                      float rightStickX, float rightStickY,
                                      float leftTrigger, float rightTrigger) {
        UsbDeviceContext context = usbDeviceContexts.get(controllerId);
        if (context != null) {
            synchronized (context) {
                if (context.guideTapping && (buttonFlags & ControllerPacket.SPECIAL_BUTTON_FLAG) != 0) {
                    // A new Guide press arrived while the previous tap is still held: end that tap first
                    // so its delayed release cannot cut the new press short and the host sees a distinct press.
                    Runnable end = context.guideTapEnd;
                    context.guideTapEnd = null;
                    context.guideTapping = false;
                    if (end != null) {
                        mainThreadHandler.removeCallbacks(end);
                    }
                    reportUsbControllerState(controllerId, buttonFlags & ~ControllerPacket.SPECIAL_BUTTON_FLAG,
                            leftStickX, leftStickY, rightStickX, rightStickY, leftTrigger, rightTrigger);
                }
                if (context.guideDown && (buttonFlags & ControllerPacket.SPECIAL_BUTTON_FLAG) != 0 &&
                        (buttonFlags & ~ControllerPacket.SPECIAL_BUTTON_FLAG) != 0 && context.guideHold.chord()) {
                    // A button joined a waiting Guide (a chord such as Guide+RB): Guide goes out first, with
                    // the buttons held before, then this report.
                    mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
                    reportUsbControllerState(controllerId, context.quitButtons, leftStickX, leftStickY,
                            rightStickX, rightStickY, leftTrigger, rightTrigger);
                }
                reportUsbControllerState(controllerId, buttonFlags, leftStickX, leftStickY, rightStickX, rightStickY,
                        leftTrigger, rightTrigger);
                if (context.guideTapping && context.guideTapEnd == null && !stopped) {
                    // A short Guide tap went out with this report: schedule its release after the
                    // minimum press without blocking the driver's input thread.
                    Runnable end = new Runnable() {
                        @Override
                        public void run() {
                            synchronized (context) {
                                if (context.guideTapEnd != this) {
                                    return;
                                }
                                context.guideTapEnd = null;
                                context.guideTapping = false;
                                if (stopped || usbDeviceContexts.get(controllerId) != context) {
                                    return;
                                }
                                reportUsbControllerState(controllerId, context.lastRawButtons,
                                        context.lastRawLeftStickX, context.lastRawLeftStickY,
                                        context.lastRawRightStickX, context.lastRawRightStickY,
                                        context.lastRawLeftTrigger, context.lastRawRightTrigger);
                            }
                        }
                    };
                    context.guideTapEnd = end;
                    mainThreadHandler.postDelayed(end, MINIMUM_BUTTON_DOWN_TIME_MS);
                }
            }
            return;
        }
        reportUsbControllerState(controllerId, buttonFlags, leftStickX, leftStickY, rightStickX, rightStickY,
                leftTrigger, rightTrigger);
    }

    private void reportUsbControllerState(int controllerId, int buttonFlags,
                                          float leftStickX, float leftStickY,
                                          float rightStickX, float rightStickY,
                                          float leftTrigger, float rightTrigger) {
        UsbDeviceContext context = usbDeviceContexts.get(controllerId);
        if (stopped || context == null) {
            return;
        }
        context.lastRawButtons = buttonFlags;
        context.lastRawLeftStickX = leftStickX;
        context.lastRawLeftStickY = leftStickY;
        context.lastRawRightStickX = rightStickX;
        context.lastRawRightStickY = rightStickY;
        context.lastRawLeftTrigger = leftTrigger;
        context.lastRawRightTrigger = rightTrigger;
        // The driver's own flags are the physical buttons, before mapping and face-button flipping;
        // the quit combo arms as it comes together, not again for a chord held from the menu.
        boolean quitChanged = buttonFlags != context.quitButtons;
        context.quitButtons = buttonFlags;
        buttonFlags = guideHoldButtons(context, buttonFlags);
        int buttons = context.buttonMap.mapButtonFlags(buttonFlags, prefConfig.flipFaceButtons);
        if (!inputEnabled) {
            // Android sees no key events from a pad this driver owns: it steers the menu from here.
            steerMenu(context, buttons);
            return;
        }
        PadCapture capture = padCapture;
        if (capture != null) {
            capturePad(context, capture, buttons, leftStickX, leftStickY);
            return;
        }
        if (context.steeringMenu) {
            // Buttons still held from the menu reach the host only after they were let go.
            context.steeringMenu = false;
            context.captureStick = null;
            context.heldFromMenu = context.menuButtons;
        }
        context.heldFromMenu &= buttons;
        buttons &= ~context.heldFromMenu;
        context.lastButtons = buttons;

        NightjarInputJni.normalizeUsbAxes(
                leftStickX, leftStickY, rightStickX, rightStickY, leftTrigger, rightTrigger,
                context.leftStickDeadzoneRadius, context.rightStickDeadzoneRadius,
                context.triggerDeadzone, context.usbAxesOut);
        context.leftStickX = (short) context.usbAxesOut[0];
        context.leftStickY = (short) context.usbAxesOut[1];
        context.rightStickX = (short) context.usbAxesOut[2];
        context.rightStickY = (short) context.usbAxesOut[3];
        context.leftTrigger = (byte) context.usbAxesOut[4];
        context.rightTrigger = (byte) context.usbAxesOut[5];

        context.inputMap = context.clickpadButtons(buttons);

        // The same menu combination as on an Android-mapped pad, opened once all four are let go so
        // it cannot activate a menu item.
        if (buttons == (ControllerPacket.BACK_FLAG | ControllerPacket.PLAY_FLAG |
                ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG)) {
            context.menuComboPending = true;
        }
        if (quitChanged) holdQuitCombo(context, buttonFlags);

        sendControllerInputPacket(context);

        if (context.menuComboPending && buttons == 0) {
            context.menuComboPending = false;
            if (activityContext instanceof Game) {
                ((Game) activityContext).showStreamMenuFromController();
            }
        }
        releaseQuitCombo(context, buttonFlags);
    }

    /**
     * The buttons of the controller quit combo chosen in Settings, or 0 when it is off (the default:
     * Start+Select+LB+RB opens the stream menu here). The choices are Moonlight V+ PC's
     * (qiin2333/moonlight-qt, gamepad.cpp), less its default, which is the menu combo on Android.
     */
    static int quitComboMask(String combo) {
        int shoulders = ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG;
        switch (combo == null ? "" : combo) {
            case "select_lb_rb_y": return shoulders | ControllerPacket.BACK_FLAG | ControllerPacket.Y_FLAG;
            case "start_lb_rb_a": return shoulders | ControllerPacket.PLAY_FLAG | ControllerPacket.A_FLAG;
            case "start_lb_rb_b": return shoulders | ControllerPacket.PLAY_FLAG | ControllerPacket.B_FLAG;
            case "lb_rb_x_y": return shoulders | ControllerPacket.X_FLAG | ControllerPacket.Y_FLAG;
            case "lb_rb_a_b": return shoulders | ControllerPacket.A_FLAG | ControllerPacket.B_FLAG;
            default: return 0;
        }
    }

    // A physical button as the quit combo sees it; 0 for buttons no combo uses.
    static int quitButton(int keyCode) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_BUTTON_START: case KeyEvent.KEYCODE_MENU: return ControllerPacket.PLAY_FLAG;
            case KeyEvent.KEYCODE_BUTTON_SELECT: case KeyEvent.KEYCODE_BACK: return ControllerPacket.BACK_FLAG;
            case KeyEvent.KEYCODE_BUTTON_L1: return ControllerPacket.LB_FLAG;
            case KeyEvent.KEYCODE_BUTTON_R1: return ControllerPacket.RB_FLAG;
            case KeyEvent.KEYCODE_BUTTON_A: return ControllerPacket.A_FLAG;
            case KeyEvent.KEYCODE_BUTTON_B: return ControllerPacket.B_FLAG;
            case KeyEvent.KEYCODE_BUTTON_X: return ControllerPacket.X_FLAG;
            case KeyEvent.KEYCODE_BUTTON_Y: return ControllerPacket.Y_FLAG;
            case KeyEvent.KEYCODE_BUTTON_THUMBL: return ControllerPacket.LS_CLK_FLAG;
            case KeyEvent.KEYCODE_BUTTON_THUMBR: return ControllerPacket.RS_CLK_FLAG;
            case KeyEvent.KEYCODE_BUTTON_MODE: return ControllerPacket.SPECIAL_BUTTON_FLAG;
            case KeyEvent.KEYCODE_DPAD_UP: return ControllerPacket.UP_FLAG;
            case KeyEvent.KEYCODE_DPAD_DOWN: return ControllerPacket.DOWN_FLAG;
            case KeyEvent.KEYCODE_DPAD_LEFT: return ControllerPacket.LEFT_FLAG;
            case KeyEvent.KEYCODE_DPAD_RIGHT: return ControllerPacket.RIGHT_FLAG;
            default: return 0;
        }
    }

    private void holdQuitCombo(GenericControllerContext context, int buttons) {
        int mask = quitComboMask(prefConfig.gamepadQuitCombo);
        if (mask != 0 && buttons == mask) {
            context.quitComboPending = true;
        }
    }

    // Like the menu combo, the stream ends once every button is up, so none stays held on the PC.
    private void releaseQuitCombo(GenericControllerContext context, int buttons) {
        if (context.quitComboPending && buttons == 0) {
            context.quitComboPending = false;
            if (activityContext instanceof Game) {
                ((Game) activityContext).leaveStreamFromController();
            }
        }
    }

    // Every controller context the mouse switch covers: the default one and each pad.
    private List<GenericControllerContext> mouseContexts() {
        List<GenericControllerContext> contexts = new ArrayList<>();
        contexts.add(defaultContext);
        if (handheldContext != defaultContext) contexts.add(handheldContext);
        contexts.addAll(inputDeviceContexts.values());
        contexts.addAll(usbDeviceContexts.values());
        return contexts;
    }

    /** Any controller drives the mouse (mouse emulation). */
    public boolean isMouseEmulationActive() {
        for (GenericControllerContext context : mouseContexts()) {
            if (context.mouseEmulationActive) return true;
        }
        return false;
    }

    /**
     * A controller is connected and the mouse switch can do something, so the stream menu offers it: turn a mouse on
     * (Controllers on) or turn an active one off (also with Controllers Off, for a mouse the on-screen gamepad turned on).
     */
    public boolean hasControllers() {
        return (!inputDeviceContexts.isEmpty() || !usbDeviceContexts.isEmpty())
                && (prefConfig.sendControllers || isMouseEmulationActive());
    }

    /**
     * The stream menu's controller mouse switch, as in Moonlight V+ PC: every active controller
     * mouse turns off, or the first controller's turns on. It works with the Start long press off.
     */
    public void toggleMouseEmulationFromMenu() {
        boolean active = false;
        for (GenericControllerContext context : mouseContexts()) {
            if (context.mouseEmulationActive) { context.toggleMouseEmulation(); active = true; }
        }
        if (active) return;
        // The first controller is the lowest numbered pad, not whichever the map lists first.
        GenericControllerContext first = null;
        for (GenericControllerContext context : mouseContexts()) {
            if (context == defaultContext || context == handheldContext || !context.assignedControllerNumber) continue;
            if (first == null || context.controllerNumber < first.controllerNumber) first = context;
        }
        if (first != null) first.toggleMouseEmulation();
    }

    // The keyboard owns the pad: report each change to it, and treat what is still held on the way
    // out like buttons held from the menu, so they reach the game only after they were let go.
    private static final int[] CAPTURED_BUTTONS = {ControllerPacket.UP_FLAG, ControllerPacket.DOWN_FLAG,
            ControllerPacket.LEFT_FLAG, ControllerPacket.RIGHT_FLAG, ControllerPacket.A_FLAG, ControllerPacket.B_FLAG,
            ControllerPacket.X_FLAG, ControllerPacket.Y_FLAG, ControllerPacket.LB_FLAG, ControllerPacket.RB_FLAG,
            ControllerPacket.PLAY_FLAG};

    private void capturePad(UsbDeviceContext context, PadCapture capture, int buttons, float stickX, float stickY) {
        if (!context.steeringMenu) {
            context.steeringMenu = true;
            // Everything held as the keyboard takes over, such as the A that chose it in the menu, is no
            // press for the keyboard and stays away from the game until it is let go.
            context.menuButtons = buttons | context.lastButtons;
            context.menuComboPending = false;
            context.quitComboPending = false;
            context.captureStick = null;
            context.captureGeneration = captureGeneration.get();
            // The game's last state is cleared here, on the thread that sends this pad's reports.
            releaseUsbInput(context);
        }
        int generation = captureGeneration.get();
        if (context.captureGeneration != generation) {
            context.captureGeneration = generation;
            context.captureStick = null;
        }
        int changed = buttons ^ context.menuButtons;
        context.menuButtons = buttons;
        for (int flag : CAPTURED_BUTTONS) {
            if ((changed & flag) != 0) capture.button(flag, (buttons & flag) != 0);
        }
        // The stick is reported on every report; the keyboard only needs to hear when its direction changes.
        KeyboardFocus.Direction direction = PcKeyboardController.stickDirection(stickX, stickY, context.captureStick);
        if (direction != context.captureStick) {
            context.captureStick = direction;
            capture.stick(context.id, stickX, stickY);
        }
    }

    // While the stream menu or one of its dialogs has focus, a newly pressed D-pad direction, A or B
    // of a USB-driver pad moves, chooses or goes back there.
    private void steerMenu(UsbDeviceContext context, int buttons) {
        // The menu took the pad from the keyboard (if it had it), which let go of the stick.
        context.captureStick = null;
        if (!context.steeringMenu) {
            context.steeringMenu = true;
            context.menuButtons = context.lastButtons;
            context.menuComboPending = false;
            context.quitComboPending = false;
        }
        int pressed = buttons & ~context.menuButtons;
        context.menuButtons = buttons;
        if (pressed == 0 || !(activityContext instanceof Game)) {
            return;
        }
        Game game = (Game) activityContext;
        int[][] keys = {
                {ControllerPacket.UP_FLAG, KeyEvent.KEYCODE_DPAD_UP},
                {ControllerPacket.DOWN_FLAG, KeyEvent.KEYCODE_DPAD_DOWN},
                {ControllerPacket.LEFT_FLAG, KeyEvent.KEYCODE_DPAD_LEFT},
                {ControllerPacket.RIGHT_FLAG, KeyEvent.KEYCODE_DPAD_RIGHT},
                {ControllerPacket.A_FLAG, KeyEvent.KEYCODE_DPAD_CENTER},
                {ControllerPacket.B_FLAG, KeyEvent.KEYCODE_BUTTON_B},
                // The stream menu's controller buttons: Y keyboard, X mouse, LB shortcuts, RB overlay.
                {ControllerPacket.Y_FLAG, KeyEvent.KEYCODE_BUTTON_Y},
                {ControllerPacket.X_FLAG, KeyEvent.KEYCODE_BUTTON_X},
                {ControllerPacket.LB_FLAG, KeyEvent.KEYCODE_BUTTON_L1},
                {ControllerPacket.RB_FLAG, KeyEvent.KEYCODE_BUTTON_R1}};
        for (int[] key : keys) {
            if ((pressed & key[0]) != 0) game.steerStreamMenu(key[1]);
        }
    }

    @Override
    public void reportControllerTouch(int controllerId, byte eventType, int pointerId, float x, float y, float pressure) {
        UsbDeviceContext context = usbDeviceContexts.get(controllerId);
        if (!stopped && inputEnabled && prefConfig.sendControllers && context != null && context.assignedControllerNumber &&
                (context.capabilities() & MoonBridge.LI_CCAP_TOUCHPAD) != 0) {
            conn.sendControllerTouchEvent((byte)context.controllerNumber, eventType, pointerId, x, y, pressure);
        }
    }

    @Override
    public void reportControllerMotion(int controllerId, byte motionType, float x, float y, float z) {
        UsbDeviceContext context = usbDeviceContexts.get(controllerId);
        if (!stopped && inputEnabled && sensorsEnabled && prefConfig.sendControllers && prefConfig.gamepadMotionSensors && context != null &&
                context.assignedControllerNumber && !dropsMotionAndTouchpad(context.buttonMap)) {
            if (motionType == MoonBridge.LI_MOTION_TYPE_GYRO) {
                GyroAxisTransformer mapping = GyroDiagnosticOverlay.isFlydigiForced()
                        ? GyroMappingPreferences.FLYDIGI_TRANSFORM : context.gyroMapping;
                if (!mapping.isIdentity()) {
                    // One driver thread reports each pad, so its buffer is never shared.
                    mapping.transform(x, y, z, context.mappedMotion);
                    x = context.mappedMotion[0];
                    y = context.mappedMotion[1];
                    z = context.mappedMotion[2];
                }
            }
            if (GyroDiagnosticOverlay.isEnabled()) {
                GyroDiagnosticOverlay.report("usb", context.controllerNumber, motionType, x, y, z);
            }
            conn.sendControllerMotionEvent((byte)context.controllerNumber, motionType, x, y, z);
        }
    }

    @Override
    public void reportControllerBattery(int controllerId, byte state, byte percentage) {
        UsbDeviceContext context = usbDeviceContexts.get(controllerId);
        if (!stopped && prefConfig.sendControllers && context != null && context.assignedControllerNumber) {
            conn.sendControllerBatteryEvent((byte)context.controllerNumber, state, percentage);
        }
    }

    @Override
    public synchronized void deviceRemoved(AbstractController controller) {
        // Stream teardown retains host controller identities; only a physical removal
        // during a live stream should clear the active controller mask.
        if (stopped) return;
        UsbDeviceContext context = usbDeviceContexts.get(controller.getControllerId());
        if (context != null) {
            LimeLog.info("Removed controller: "+controller.getControllerId());
            // Unplugged while the PC keyboard was open: nothing will report what it held being let go.
            PadCapture capture = padCapture;
            if (capture != null && context.steeringMenu) {
                for (int flag : CAPTURED_BUTTONS) {
                    if ((context.menuButtons & flag) != 0) capture.button(flag, false);
                }
                context.menuButtons = 0;
                if (context.captureStick != null) {
                    context.captureStick = null;
                    capture.stick(context.id, 0f, 0f);
                }
            }
            if (prefConfig.sendControllers && context.assignedControllerNumber &&
                    (context.capabilities() & MoonBridge.LI_CCAP_TOUCHPAD) != 0) {
                conn.sendControllerTouchEvent((byte)context.controllerNumber, MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL,
                        0, 0, 0, 0);
            }
            releaseControllerNumber(context);
            context.destroy();
            usbDeviceContexts.remove(controller.getControllerId());
            if (context.assignedControllerNumber) announceBuiltInControls(context.controllerNumber);
        }
        announcePresence();
    }

    @Override
    public synchronized void deviceAdded(AbstractController controller) {
        if (stopped) {
            controller.stop();
            return;
        }
        if (usbDeviceContexts.get(controller.getControllerId()) != null) {
            return;
        }

        UsbDeviceContext context = createUsbDeviceContextForDevice(controller);
        usbDeviceContexts.put(controller.getControllerId(), context);
        if (connectionStarted && conn.canSendControllerInput()) sendControllerInputPacket(context);
        announcePresence();
        controllerAdded();
    }

    class GenericControllerContext {
        public int id;
        public boolean external;

        public int vendorId;
        public int productId;

        public float leftStickDeadzoneRadius;
        public float rightStickDeadzoneRadius;
        public float triggerDeadzone;

        public volatile boolean assignedControllerNumber;
        public boolean reservedControllerNumber;
        public short controllerNumber;

        public ControllerAxisMap axisMap = new ControllerAxisMap();
        public final ControllerAxisMap.Report mappedReport = new ControllerAxisMap.Report();
        public int inputMap = 0;
        public byte leftTrigger = 0x00;
        public byte rightTrigger = 0x00;
        public short rightStickX = 0x0000;
        public short rightStickY = 0x0000;
        public short leftStickX = 0x0000;
        public short leftStickY = 0x0000;

        public boolean mouseEmulationActive;
        // The quit combo was held; the stream ends once every button is up.
        boolean quitComboPending;
        // The physical buttons held, before user mapping, face-button flipping and the Select,
        // clickpad and guide emulation: the quit combo is matched against these.
        int quitButtons;
        // Hold Guide to open the stream menu: the held-back press, and the timer that opens the menu.
        final GuideHold guideHold = new GuideHold();
        final Runnable guideHoldRunnable = () -> {
            if (!stopped && guideHold.expire() && activityContext instanceof Game) {
                ((Game) activityContext).showStreamMenuFromController();
            }
        };
        // A held-back Guide is being replayed to the host (a tap or a chord), not pressed again.
        boolean guideTapping;
        // The release that ends a short Guide tap, until it has run; a new Guide press runs it first.
        Runnable guideTapEnd;
        // The held-back Guide press of an Android-mapped pad, replayed when a chord sends it.
        KeyEvent guideHeldDown;
        public int mouseEmulationLastInputMap;
        public boolean mouseEmulationFixedStepHeld;
        public int mouseEmulationStep = FixedStepMouse.MIN_STEP;
        public final int mouseEmulationReportPeriod = 50;

        public final Runnable mouseEmulationRunnable = new Runnable() {
            @Override
            public void run() {
                if (!mouseEmulationActive) {
                    return;
                }
                if (mouseSilenced()) {
                    return;
                }

                int fixedStep = mouseEmulationFixedStepHeld ? mouseEmulationStep : 0;

                // Send mouse events from analog sticks
                if (prefConfig.analogStickForScrolling == PreferenceConfiguration.AnalogStickForScrolling.RIGHT) {
                    sendEmulatedMouseMove(leftStickX, leftStickY, fixedStep);
                    sendEmulatedMouseScroll(rightStickX, rightStickY);
                }
                else if (prefConfig.analogStickForScrolling == PreferenceConfiguration.AnalogStickForScrolling.LEFT) {
                    sendEmulatedMouseMove(rightStickX, rightStickY, fixedStep);
                    sendEmulatedMouseScroll(leftStickX, leftStickY);
                }
                else {
                    sendEmulatedMouseMove(leftStickX, leftStickY, fixedStep);
                    sendEmulatedMouseMove(rightStickX, rightStickY, fixedStep);
                }

                // Requeue the callback
                mainThreadHandler.postDelayed(this, mouseEmulationReportPeriod);
            }
        };

        // Controllers Off: a physical pad sends nothing, its mouse included (the on-screen gamepad still does).
        private boolean mouseSilenced() {
            return !prefConfig.sendControllers && this != defaultContext;
        }

        public void toggleMouseEmulation() {
            // Mouse mode does not turn on for a pad whose mouse is silenced; turning it off always works.
            if (!mouseEmulationActive && mouseSilenced()) {
                return;
            }
            mainThreadHandler.removeCallbacks(mouseEmulationRunnable);
            releaseMouseButtons();
            mouseEmulationActive = !mouseEmulationActive;
            Toast.makeText(activityContext, mouseEmulationActive ? R.string.stream_mouse_mode_on :
                    R.string.stream_mouse_mode_off, Toast.LENGTH_SHORT).show();

            if (mouseEmulationActive) {
                mainThreadHandler.postDelayed(mouseEmulationRunnable, mouseEmulationReportPeriod);
            }
        }

        public void destroy() {
            releaseMouseButtons();
            mouseEmulationActive = false;
            mainThreadHandler.removeCallbacks(mouseEmulationRunnable);
            mainThreadHandler.removeCallbacks(guideHoldRunnable);
            guideHold.reset();
            if (guideTapEnd != null) {
                mainThreadHandler.removeCallbacks(guideTapEnd);
                guideTapEnd = null;
            }
            guideTapping = false;
        }

        private void releaseMouseButtons() {
            if ((mouseEmulationLastInputMap & ControllerPacket.A_FLAG) != 0) {
                conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
            }
            if ((mouseEmulationLastInputMap & ControllerPacket.B_FLAG) != 0) {
                conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
            }
            mouseEmulationLastInputMap = 0;
            mouseEmulationFixedStepHeld = false;
        }

        public void sendControllerArrival() {}
    }

    class InputDeviceContext extends GenericControllerContext {
        public String name;
        private byte reportedType;
        private short reportedCapabilities;
        private int supportedButtonFlags;
        public ControllerButtonMap buttonMap = new ControllerButtonMap();
        /** Whether this external-listed pad is a handheld's own controls; null until first asked. */
        Boolean handheldControls;
        public final Map<Integer, Integer> pressedButtons = new HashMap<>();
        public final Map<Integer, Runnable> pendingButtonReleases = new HashMap<>();
        public VibratorManager vibratorManager;
        public Vibrator vibrator;
        public boolean quadVibrators;
        public short lowFreqMotor, highFreqMotor;
        public short leftTriggerMotor, rightTriggerMotor;

        public SensorManager sensorManager;
        public MotionListener gyroListener;
        public short gyroReportRateHz;
        public MotionListener accelListener;
        public short accelReportRateHz;

        public InputDevice inputDevice;

        public boolean hasRgbLed;
        public LightsManager.LightsSession lightsSession;
        public int ledArgbValue;

        // These are BatteryState values, not Moonlight values
        public int lastReportedBatteryStatus;
        public float lastReportedBatteryCapacity;

        public int leftStickXAxis = -1;
        public int leftStickYAxis = -1;

        public int rightStickXAxis = -1;
        public int rightStickYAxis = -1;

        public int leftTriggerAxis = -1;
        public int rightTriggerAxis = -1;
        public boolean triggersIdleNegative;
        public boolean leftTriggerAxisUsed, rightTriggerAxisUsed;

        public int hatXAxis = -1;
        public int hatYAxis = -1;
        public boolean hatXAxisUsed, hatYAxisUsed;

        InputDevice.MotionRange touchpadXRange;
        InputDevice.MotionRange touchpadYRange;
        InputDevice.MotionRange touchpadPressureRange;

        public boolean isNonStandardDualShock4;
        public boolean usesLinuxGamepadStandardFaceButtons;
        public boolean isNonStandardXboxBtController;
        public boolean isServal;
        public boolean backIsStart;
        public boolean modeIsSelect;
        public boolean searchIsMode;
        public boolean ignoreBack;
        public boolean hasJoystickAxes;
        public boolean pendingExit;
        public boolean isDualShockStandaloneTouchpad;

        public int emulatingButtonFlags = 0;
        public boolean hasSelect;
        public boolean hasMode;
        public boolean hasPaddles;
        public boolean hasShare;
        public boolean needsClickpadEmulation;

        // Used for OUYA bumper state tracking since they force all buttons
        // up when the OUYA button goes down. We watch the last time we get
        // a bumper up and compare that to our maximum delay when we receive
        // a Start button press to see if we should activate one of our
        // emulated button combos.
        public long lastLbUpTime = 0;
        public long lastRbUpTime = 0;

        public long startDownTime = 0;

        private boolean destroyed;

        public final Runnable batteryStateUpdateRunnable = new Runnable() {
            @Override
            public void run() {
                synchronized (InputDeviceContext.this) {
                    if (destroyed || stopped || !assignedControllerNumber) {
                        return;
                    }
                }
                // Outside the lock: destroy() takes it on the main thread, and this read can block.
                BatteryReading battery = readControllerBattery(InputDeviceContext.this);
                synchronized (InputDeviceContext.this) {
                    if (destroyed || stopped) {
                        return;
                    }
                    if (battery != null) {
                        sendControllerBatteryPacket(InputDeviceContext.this, battery);
                    }

                    // Requeue the callback
                    backgroundThreadHandler.postDelayed(this, BATTERY_RECHECK_INTERVAL_MS);
                }
            }
        };

        public final Runnable enableSensorRunnable = new Runnable() {
            @Override
            public void run() {
                if (destroyed || stopped || !sensorsEnabled) {
                    return;
                }
                // Turn back on any sensors that should be reporting but are currently unregistered
                if (accelReportRateHz != 0 && accelListener == null) {
                    handleSetMotionEventState(controllerNumber, MoonBridge.LI_MOTION_TYPE_ACCEL, accelReportRateHz);
                }
                if (gyroReportRateHz != 0 && gyroListener == null) {
                    handleSetMotionEventState(controllerNumber, MoonBridge.LI_MOTION_TYPE_GYRO, gyroReportRateHz);
                }
            }
        };

        public final Runnable setLedStateRunnable = new Runnable() {
            @TargetApi(Build.VERSION_CODES.S)
            @Override
            public void run() {
                LightsManager.LightsSession session;
                synchronized (InputDeviceContext.this) {
                    if (destroyed || stopped) {
                        return;
                    }
                    session = lightsSession;
                }
                // Binder calls like the battery read, so outside the lock destroy() takes.
                try {
                    LightState lightState = new LightState.Builder().setColor(ledArgbValue).build();

                    // Set the RGB value for each RGB-controllable LED on the device
                    boolean hasRgbLight = false;
                    LightsRequest.Builder lightsRequestBuilder = new LightsRequest.Builder();
                    for (Light light : inputDevice.getLightsManager().getLights()) {
                        if (light.hasRgbControl()) {
                            lightsRequestBuilder.addLight(light, lightState);
                            hasRgbLight = true;
                        }
                    }

                    if (hasRgbLight) {
                        // Create a new light session if one doesn't already exist
                        if (session == null) {
                            session = inputDevice.getLightsManager().openSession();
                            boolean destroyedMeanwhile;
                            synchronized (InputDeviceContext.this) {
                                destroyedMeanwhile = destroyed || stopped;
                                if (!destroyedMeanwhile) {
                                    lightsSession = session;
                                }
                            }
                            if (destroyedMeanwhile) {
                                session.close();
                                return;
                            }
                        }

                        // Apply the LED changes
                        session.requestLights(lightsRequestBuilder.build());
                    }
                } catch (RuntimeException e) {
                    LimeLog.warning("Controller lights unavailable after detach");
                }
            }
        };

        void cancelMotors() {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && vibratorManager != null) {
                vibratorManager.cancel();
            }
            else if (vibrator != null) {
                vibrator.cancel();
            }
        }

        @Override
        public synchronized void destroy() {
            destroyed = true;
            for (Runnable r : pendingButtonReleases.values()) {
                mainThreadHandler.removeCallbacks(r);
            }
            pendingButtonReleases.clear();
            super.destroy();

            cancelMotors();

            mainThreadHandler.removeCallbacks(enableSensorRunnable);
            backgroundThreadHandler.removeCallbacks(setLedStateRunnable);

            if (gyroListener != null) {
                closeMotionListener(sensorManager, gyroListener);
            }
            if (accelListener != null) {
                closeMotionListener(sensorManager, accelListener);
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                if (lightsSession != null) {
                    try {
                        lightsSession.close();
                    } catch (RuntimeException e) {
                        LimeLog.warning("Error closing LightsSession: " + e.getMessage());
                    }
                }
            }

            backgroundThreadHandler.removeCallbacks(batteryStateUpdateRunnable);
        }

        @Override
        public void sendControllerArrival() {
            byte type;
            switch (inputDevice.getVendorId()) {
                case 0x045e: // Microsoft
                    type = MoonBridge.LI_CTYPE_XBOX;
                    break;
                case 0x054c: // Sony
                    type = MoonBridge.LI_CTYPE_PS;
                    break;
                case 0x057e: // Nintendo
                    type = MoonBridge.LI_CTYPE_NINTENDO;
                    break;
                default:
                    // Consult SDL's controller type list to see if it knows
                    type = MoonBridge.guessControllerType(inputDevice.getVendorId(), inputDevice.getProductId());
                    break;
            }

            int[] keyCodes = androidButtonKeycodes();
            boolean[] hasKeys = new boolean[keyCodes.length];
            for (int i = 0; i < keyCodes.length; i++) {
                hasKeys[i] = inputDevice.hasKeys(keyCodes[i])[0];
            }
            boolean hasHatX = getMotionRangeForJoystickAxis(inputDevice, MotionEvent.AXIS_HAT_X) != null;
            boolean hasHatY = getMotionRangeForJoystickAxis(inputDevice, MotionEvent.AXIS_HAT_Y) != null;
            boolean hasTouchpadSource =
                    (inputDevice.getSources() & InputDevice.SOURCE_TOUCHPAD) == InputDevice.SOURCE_TOUCHPAD;
            boolean hasClickpad = hasTouchpadSource && hasButtonUnderTouchpad(inputDevice, type);
            int baseSupportedButtons = NightjarInputJni.computeBaseSupportedButtons(
                    hasKeys, hasPaddles, hasShare, buttonMap.targetButtonFlags(),
                    hasHatX, hasHatY, hasClickpad);

            short capabilities = 0;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                if (quadVibrators) {
                    capabilities |= MoonBridge.LI_CCAP_RUMBLE | MoonBridge.LI_CCAP_TRIGGER_RUMBLE;
                }
                else if (vibratorManager != null || vibrator != null) {
                    capabilities |= MoonBridge.LI_CCAP_RUMBLE;
                }
                if (external) {
                    capabilities |= MoonBridge.LI_CCAP_BATTERY_STATE;
                }
                if (hasRgbLed) {
                    capabilities |= MoonBridge.LI_CCAP_RGB_LED;
                }
            }
            if (leftTriggerAxis != -1 || rightTriggerAxis != -1) {
                capabilities |= MoonBridge.LI_CCAP_ANALOG_TRIGGERS;
            }
            if (sensorManager != null && sensorManager.getDefaultSensor(Sensor.TYPE_ACCELEROMETER) != null) {
                capabilities |= MoonBridge.LI_CCAP_ACCEL;
            }
            if (sensorManager != null && sensorManager.getDefaultSensor(Sensor.TYPE_GYROSCOPE) != null) {
                capabilities |= MoonBridge.LI_CCAP_GYRO;
            }
            if (vibrator != null) {
                capabilities |= MoonBridge.LI_CCAP_RUMBLE;
            }
            if (sceManager.isRecognizedDevice(inputDevice)) {
                capabilities |= MoonBridge.LI_CCAP_RUMBLE | MoonBridge.LI_CCAP_BATTERY_STATE;
            }
            if (hasTouchpadSource) {
                capabilities |= MoonBridge.LI_CCAP_TOUCHPAD;
            }

            int[] arrivalOut = new int[4];
            NightjarInputJni.computeInputDeviceArrival(
                    type, baseSupportedButtons, capabilities, sensorManager != null,
                    buttonMap.connectAs(), buttonMap.targetButtonFlags(),
                    carriesTouchpadButton(this), steamControlsSession, onscreenOwnPlayerSession,
                    controllerNumber, arrivalOut);
            reportedType = (byte) arrivalOut[0];
            reportedCapabilities = (short) arrivalOut[1];
            supportedButtonFlags = arrivalOut[2];
            int arrivalFlags = arrivalOut[3];
            if ((arrivalFlags & 1) != 0) {
                mainThreadHandler.post(() -> Toast.makeText(activityContext,
                        R.string.toast_controller_type_changed, Toast.LENGTH_LONG).show());
            }
            if ((arrivalFlags & 2) != 0) {
                needsClickpadEmulation = (arrivalFlags & 4) != 0;
            }
            capabilities = reportedCapabilities;
            // A built-in device sharing player 1 with an external pad stays quiet, so the host
            // keeps the external pad's type, touchpad and motion.
            if (!external && sharesSlotWithExternalPad(this)) return;
            if (!prefConfig.sendControllers) return;
            // Which controller it is goes first, so the host can choose its pad on arrival.
            conn.sendControllerIdentityEvent((byte)controllerNumber, vendorId, productId);
            conn.sendControllerArrivalEvent((byte)controllerNumber, getActiveControllerMask(),
                    reportedType, supportedButtonFlags, capabilities);

            // After reporting arrival to the host, send initial battery state and begin monitoring
            backgroundThreadHandler.removeCallbacks(batteryStateUpdateRunnable);
            backgroundThreadHandler.post(batteryStateUpdateRunnable);
        }

        public void migrateContext(InputDeviceContext oldContext) {
            for (Runnable r : new ArrayList<>(oldContext.pendingButtonReleases.values())) {
                mainThreadHandler.removeCallbacks(r);
                r.run();
            }
            reportedType = oldContext.reportedType;
            reportedCapabilities = oldContext.reportedCapabilities;
            supportedButtonFlags = oldContext.supportedButtonFlags;
            handheldControls = oldContext.handheldControls;
            this.inputMap = oldContext.inputMap;
            this.leftTrigger = oldContext.leftTrigger;
            this.rightTrigger = oldContext.rightTrigger;
            this.leftStickX = oldContext.leftStickX;
            this.leftStickY = oldContext.leftStickY;
            this.rightStickX = oldContext.rightStickX;
            this.rightStickY = oldContext.rightStickY;
            this.pressedButtons.putAll(oldContext.pressedButtons);
            this.startDownTime = oldContext.startDownTime;
            this.lastLbUpTime = oldContext.lastLbUpTime;
            this.lastRbUpTime = oldContext.lastRbUpTime;
            this.emulatingButtonFlags = oldContext.emulatingButtonFlags;
            this.pendingExit = oldContext.pendingExit;
            this.hasSelect |= oldContext.hasSelect;
            this.hasMode |= oldContext.hasMode;
            this.leftTriggerAxisUsed = oldContext.leftTriggerAxisUsed && leftTriggerAxis == oldContext.leftTriggerAxis;
            this.rightTriggerAxisUsed = oldContext.rightTriggerAxisUsed && rightTriggerAxis == oldContext.rightTriggerAxis;
            this.hatXAxisUsed = oldContext.hatXAxisUsed && hatXAxis == oldContext.hatXAxis;
            this.hatYAxisUsed = oldContext.hatYAxisUsed && hatYAxis == oldContext.hatYAxis;
            this.mouseEmulationActive = oldContext.mouseEmulationActive;
            this.mouseEmulationLastInputMap = oldContext.mouseEmulationLastInputMap;
            this.mouseEmulationFixedStepHeld = oldContext.mouseEmulationFixedStepHeld;
            this.mouseEmulationStep = oldContext.mouseEmulationStep;
            oldContext.mouseEmulationLastInputMap = 0;
            synchronized (oldContext) {
                // Take ownership of the sensor and light sessions
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                    this.lightsSession = oldContext.lightsSession;
                    oldContext.lightsSession = null;
                }
                this.ledArgbValue = oldContext.ledArgbValue;
                this.gyroReportRateHz = oldContext.gyroReportRateHz;
                this.accelReportRateHz = oldContext.accelReportRateHz;

                // Don't release the controller number, because we will carry it over if it is present.
                // We also want to make sure the change is invisible to the host PC to avoid an add/remove
                // cycle for the gamepad which may break some games.
                oldContext.destroy();

            }

            // Copy over existing controller number state
            this.assignedControllerNumber = oldContext.assignedControllerNumber;
            this.reservedControllerNumber = oldContext.reservedControllerNumber;
            this.controllerNumber = oldContext.controllerNumber;

            if (mouseEmulationActive) {
                mainThreadHandler.postDelayed(mouseEmulationRunnable, mouseEmulationReportPeriod);
            }

            // We may have set this device to use the built-in sensor manager. If so, do that again.
            if (oldContext.sensorManager == deviceSensorManager) {
                this.sensorManager = deviceSensorManager;
            }

            // Copy state initialized in reportControllerArrival()
            this.needsClickpadEmulation = oldContext.needsClickpadEmulation;

            // Re-enable sensors on the new context
            enableSensors();

            // Refresh the LED state if it's set. We don't know if there was a pending LED state
            // callback when the context was migrated, so we'll reset it just to be safe. The old
            // context may still have been opening its session, which it closes when it finds itself
            // destroyed, so a colour without a session counts too.
            if (this.lightsSession != null || this.ledArgbValue != 0) {
                backgroundThreadHandler.post(setLedStateRunnable);
            }

            // Refresh battery state and start the battery state polling again
            backgroundThreadHandler.post(batteryStateUpdateRunnable);
        }

        public void disableSensors() {
            disableSensors(true);
        }

        void disableSensors(boolean atRest) {
            // Stop any pending enablement
            mainThreadHandler.removeCallbacks(enableSensorRunnable);

            // Unregister all sensor listeners
            if (gyroListener != null) {
                closeMotionListener(sensorManager, gyroListener);
                gyroListener = null;

                // Send a gyro event to ensure the virtual controller is stationary
                if (atRest) {
                    sendGyroAtRest(controllerNumber);
                }
            }
            if (accelListener != null) {
                closeMotionListener(sensorManager, accelListener);
                accelListener = null;

                // We leave the acceleration as-is to preserve the attitude of the controller
            }
        }

        public void enableSensors() {
            // We allow 1 second for the input device to settle before re-enabling sensors.
            // Pointer capture can cause the input device to change, which can cause
            // InputDeviceSensorManager to crash due to missing null checks on the InputDevice.
            mainThreadHandler.postDelayed(enableSensorRunnable, 1000);
        }
    }

    class UsbDeviceContext extends GenericControllerContext {
        public AbstractController device;
        // Empty until createUsbDeviceContextForDevice loads the stored one, like InputDeviceContext's
        public ControllerButtonMap buttonMap = new ControllerButtonMap();
        // Settings › Gyroscope mapping for this model; USB pads have no input descriptor.
        GyroAxisTransformer gyroMapping = GyroAxisTransformer.identity();
        final float[] mappedMotion = new float[3];
        private boolean arrivalPending;
        private long arrivalRetryAt;
        // Start+Back+LB+RB was held; the stream menu opens once every button is up.
        boolean menuComboPending;
        // Guide was down in the driver's last report (see guideHoldButtons).
        boolean guideDown;
        // The pad steers the stream menu while the stream has no input focus.
        boolean steeringMenu;
        KeyboardFocus.Direction captureStick;
        int captureGeneration;
        int menuButtons, heldFromMenu, lastButtons;

        // Select+LB held as the touchpad click: those of the two not let go yet, which stay silent until released
        int clickpadSwallowed;
        final int[] usbAxesOut = new int[6];
        final int[] clickpadOut = new int[2];
        // Last raw driver report, used when a deferred Guide tap release fires after MINIMUM_BUTTON_DOWN_TIME_MS.
        int lastRawButtons;
        float lastRawLeftStickX, lastRawLeftStickY, lastRawRightStickX, lastRawRightStickY;
        float lastRawLeftTrigger, lastRawRightTrigger;

        // This device's motion sensors, standing in for a pad that has none as player 1
        SensorManager deviceSensors;
        MotionListener gyroListener, accelListener;
        short gyroReportRateHz, accelReportRateHz;

        void setDeviceMotion(byte motionType, short reportRateHz) {
            if (motionType == MoonBridge.LI_MOTION_TYPE_GYRO) gyroReportRateHz = reportRateHz;
            else if (motionType == MoonBridge.LI_MOTION_TYPE_ACCEL) accelReportRateHz = reportRateHz;
            else return;
            registerDeviceMotion(motionType);
        }

        /** Registers the device sensor for the motion type at the rate the host asked for, if any. */
        void registerDeviceMotion(byte motionType) {
            if (deviceSensors == null) return;
            boolean gyro = motionType == MoonBridge.LI_MOTION_TYPE_GYRO;
            MotionListener old = gyro ? gyroListener : accelListener;
            if (old != null) closeMotionListener(deviceSensors, old);
            MotionListener listener = null;
            short rate = gyro ? gyroReportRateHz : accelReportRateHz;
            Sensor sensor = deviceSensors.getDefaultSensor(gyro ? Sensor.TYPE_GYROSCOPE : Sensor.TYPE_ACCELEROMETER);
            if (!stopped && sensorsEnabled && rate != 0 && sensor != null) {
                listener = createSensorListener(controllerNumber, motionType, true, null);
                registerMotionListener(deviceSensors, sensor, listener, 1000000 / rate);
            }
            if (gyro) gyroListener = listener;
            else accelListener = listener;
        }

        void unregisterDeviceMotion() {
            if (deviceSensors == null) return;
            if (gyroListener != null) closeMotionListener(deviceSensors, gyroListener);
            if (accelListener != null) closeMotionListener(deviceSensors, accelListener);
            gyroListener = null;
            accelListener = null;
        }

        /** The type the pad arrives as: unknown with Automatic when this device's sensors give it motion. */
        byte arrivalType() {
            return inputDeviceArrivalType(device.getType(), deviceSensors != null, buttonMap);
        }

        /**
         * The buttons sent for a report. On a pad pinned as PlayStation without being one, Select+LB
         * pressed together and alone clicks the touchpad it lacks while both stay held, as on an
         * Android-mapped pad.
         */
        int clickpadButtons(int buttons) {
            NightjarInputJni.applyUsbClickpad(
                    buttons, clickpadSwallowed, device.getType(), buttonMap.connectAs(),
                    deviceSensors != null, needsClickpadEmulation(buttonMap), clickpadOut);
            clickpadSwallowed = clickpadOut[1];
            return clickpadOut[0];
        }

        /** The driver's capabilities as the host is told them: without motion and touchpad when pinned as Xbox. */
        short capabilities() {
            short capabilities = arrivalCapabilities(device.getCapabilities(), buttonMap);
            if (deviceSensors != null && deviceSensors.getDefaultSensor(Sensor.TYPE_ACCELEROMETER) != null) {
                capabilities |= MoonBridge.LI_CCAP_ACCEL;
            }
            if (deviceSensors != null && deviceSensors.getDefaultSensor(Sensor.TYPE_GYROSCOPE) != null) {
                capabilities |= MoonBridge.LI_CCAP_GYRO;
            }
            return capabilities;
        }

        boolean ensureControllerArrival() {
            if (arrivalPending && android.os.SystemClock.uptimeMillis() >= arrivalRetryAt) {
                sendControllerArrival();
            }
            return !arrivalPending;
        }

        @Override
        public void destroy() {
            super.destroy();

            unregisterDeviceMotion();
            waveformControllers.remove((short)controllerNumber, device);
        }

        @Override
        public void sendControllerArrival() {
            if (!conn.canSendControllerInput()) {
                // The slot is already reserved. Keep its capabilities ahead of any
                // input if permission is restored, with the same bounded retry rate.
                arrivalPending = true;
                arrivalRetryAt = android.os.SystemClock.uptimeMillis() + 1000;
                return;
            }
            short capabilities = capabilities();
            if ((capabilities & MoonBridge.LI_CCAP_HAPTICS_PCM) != 0) {
                waveformControllers.put((short)controllerNumber, device);
            }
            if (!prefConfig.gamepadMotionSensors) {
                capabilities &= ~(MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO);
            }
            byte type = arrivalType();
            if (!prefConfig.sendControllers) return;
            conn.sendControllerIdentityEvent((byte)controllerNumber, vendorId, productId);
            int result = conn.sendControllerArrivalEvent((byte)controllerNumber, getActiveControllerMask(),
                    type, withSteamControlButtons((short) controllerNumber,
                            device.getSupportedButtonFlags() | buttonMap.targetButtonFlags()),
                    withSteamControls((short) controllerNumber, capabilities, buttonMap));
            if (result != 0) {
                if (!arrivalPending) {
                    LimeLog.warning("USB controller announcement deferred: slot=" + controllerNumber + ", rc=" + result);
                }
                arrivalPending = true;
                arrivalRetryAt = android.os.SystemClock.uptimeMillis() + 1000;
                return;
            }
            arrivalPending = false;
            LimeLog.info("USB controller announcement queued: slot=" + controllerNumber +
                    ", type=" + type + ", capabilities=0x" + Integer.toHexString(capabilities & 0xffff));
            device.setPlayerNumber(controllerNumber);
            device.bindAudio(conn, controllerNumber);
        }
    }
}
