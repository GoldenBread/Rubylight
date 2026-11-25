package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.hardware.usb.UsbManager;
import android.hardware.input.InputManager;
import android.view.InputDevice;
import android.view.MotionEvent;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.binding.input.driver.UsbPads;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.shadows.ShadowUsbManager;
import org.robolectric.shadows.ShadowInputDevice;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;
import com.limelight.nvstream.input.ControllerPacket;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import static org.junit.Assert.*;


/**
 * Shared Robolectric shadows, fake {@link NvConnection}/{@link AbstractController} harnesses,
 * and input-device fixtures for {@link ControllerPreparationTest} and its sibling suites.
 */
final class ControllerPreparationTestSupport {
    private ControllerPreparationTestSupport() {}

    static final int JOYPAD = InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD;

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class ControllerBridge {
        @Implementation protected static void __staticInitializer__() { }
        @Implementation protected static boolean guessControllerHasPaddles(int vendor, int product) { return false; }
        @Implementation protected static boolean guessControllerHasShareButton(int vendor, int product) { return false; }
        @Implementation protected static byte guessControllerType(int vendor, int product) { return MoonBridge.LI_CTYPE_UNKNOWN; }
    }

    static PreferenceConfiguration onscreenOwnPlayerPrefs(Activity activity) {
        // The attached mask reads the saved settings, the handler its session's.
        android.preference.PreferenceManager.getDefaultSharedPreferences(activity).edit()
                .putBoolean("checkbox_multi_controller", true)
                .putBoolean("checkbox_show_onscreen_controls", true)
                .putBoolean("checkbox_onscreen_own_player", true).commit();
        return PreferenceConfiguration.readPreferences(activity);
    }

    static java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts(ControllerHandler handler) {
        return ReflectionHelpers.getField(handler, "inputDeviceContexts");
    }

    static ControllerHandler handheld(Activity activity, Connection connection, PreferenceConfiguration prefs) {
        return new ControllerHandler(activity, connection, null, prefs) {
            @Override protected boolean[] findInternalGamepadAndSelect() { return new boolean[] {true, true, true}; }
        };
    }

    static android.view.KeyEvent systemBack(int action) {
        return new android.view.KeyEvent(1000, 1000, action, android.view.KeyEvent.KEYCODE_BACK, 0, 0, -1, 0, 0,
                InputDevice.SOURCE_KEYBOARD);
    }

    static void send(ControllerHandler handler, ControllerHandler.GenericControllerContext context) {
        ReflectionHelpers.callInstanceMethod(handler, "sendControllerInputPacket",
                ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, context));
    }

    /** Keeps what is posted to the main thread, to run it without idling the looper. */
    static class MainThreadPosts extends android.os.Handler {
        final List<Runnable> posted = new ArrayList<>();
        MainThreadPosts() { super(android.os.Looper.getMainLooper()); }
        @Override public boolean sendMessageAtTime(android.os.Message message, long uptimeMillis) {
            posted.add(message.getCallback()); return true;
        }
        void runAll() {
            List<Runnable> run = new ArrayList<>(posted);
            posted.clear();
            for (Runnable runnable : run) if (runnable != null) runnable.run();
        }
    }

    static void addInputDevice(Activity activity, int id, boolean external, int sources) {
        addInputDevice(activity, id, external, sources, "Controller " + id, 0, 0);
    }

    static void addInputDevice(Activity activity, int id, boolean external, int sources,
                                       String name, int vendor, int product) {
        addInputDevice(activity, id, external, sources, name, vendor, product, InputDevice.KEYBOARD_TYPE_NONE);
    }

    static void addInputDevice(Activity activity, int id, boolean external, int sources,
                                       String name, int vendor, int product, int keyboardType) {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed(name);
        ReflectionHelpers.setField(device, "mVendorId", vendor);
        ReflectionHelpers.setField(device, "mProductId", product);
        ReflectionHelpers.setField(device, "mId", id);
        ReflectionHelpers.setField(device, "mDescriptor", "controller-" + id);
        ReflectionHelpers.setField(device, "mIsExternal", external);
        ReflectionHelpers.setField(device, "mSources", sources);
        ReflectionHelpers.setField(device, "mKeyboardType", keyboardType);
        ReflectionHelpers.setField(device, "mMotionRanges", new ArrayList<InputDevice.MotionRange>());
        if ((sources & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK) {
            for (int axis : new int[] {MotionEvent.AXIS_X, MotionEvent.AXIS_Y}) {
                ReflectionHelpers.callInstanceMethod(device, "addMotionRange",
                        ReflectionHelpers.ClassParameter.from(int.class, axis),
                        ReflectionHelpers.ClassParameter.from(int.class, InputDevice.SOURCE_JOYSTICK),
                        ReflectionHelpers.ClassParameter.from(float.class, -1f),
                        ReflectionHelpers.ClassParameter.from(float.class, 1f),
                        ReflectionHelpers.ClassParameter.from(float.class, 0f),
                        ReflectionHelpers.ClassParameter.from(float.class, 0f),
                        ReflectionHelpers.ClassParameter.from(float.class, 0f));
            }
        }
        org.robolectric.Shadows.shadowOf((InputManager) activity.getSystemService(Context.INPUT_SERVICE))
                .addInputDevice(device);
    }

    static ShadowUsbManager usbManager(Activity activity) {
        return org.robolectric.Shadows.shadowOf((UsbManager) activity.getSystemService(Context.USB_SERVICE));
    }

    static ControllerHandler handler(Activity activity, Connection connection) {
        PreferenceConfiguration prefs = new PreferenceConfiguration(); prefs.usbDualSenseHaptics = true;
        return new ControllerHandler(activity, connection, null, prefs);
    }
    static class Connection extends NvConnection {
        int arrivals, inputs;
        short activeMask;
        byte arrivalType;
        short arrivalCaps;
        byte arrivalSlot;
        int arrivalButtons;
        final java.util.Map<Byte, Byte> typeBySlot = new java.util.HashMap<>();
        /** Each arrival as "arrival slot" and each input as "input slot buttons", in order. */
        final List<String> events = new ArrayList<>();
        Connection(int appId) {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().setApp(new NvApp("Test", appId, false)).build(), null, null);
        }
        /** Each identity sent, as {slot, vendor, product}; never part of {@link #events}. */
        final List<int[]> identities = Collections.synchronizedList(new ArrayList<>());
        @Override public int sendControllerIdentityEvent(byte slot, int vendorId, int productId) {
            identities.add(new int[] {slot, vendorId, productId}); return 0;
        }
        @Override public int sendControllerArrivalEvent(byte slot, short mask, byte type, int buttons, short caps) {
            arrivals++; arrivalType = type; arrivalCaps = caps; arrivalSlot = slot; arrivalButtons = buttons;
            typeBySlot.put(slot, type); events.add("arrival " + slot); return 0;
        }
        @Override public void sendControllerInput(short slot, short mask, int buttons, byte leftTrigger,
                                                 byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            inputs++; activeMask = mask; events.add("input " + slot + " " + buttons);
        }
        int batteries, mouseMoves;
        @Override public void sendControllerBatteryEvent(byte slot, byte state, byte percentage) { batteries++; }
        @Override public void sendMouseMove(short deltaX, short deltaY) { mouseMoves++; }
        int touches, motions;
        @Override public int sendControllerTouchEvent(byte slot, byte type, int pointer, float x, float y, float pressure) {
            touches++; return 0;
        }
        @Override public int sendControllerMotionEvent(byte slot, byte type, float x, float y, float z) {
            motions++; return 0;
        }
    }
    static class Pad extends AbstractController {
        int audioBindings, audioPackets, headphonePackets, waveforms;
        Pad() { this(MoonBridge.LI_CCAP_HAPTICS_PCM); }
        Pad(short caps) { this(42, caps); }
        Pad(int id, short caps) { super(id, null, 0x054c, 0x0ce6); capabilities = caps; }
        @Override public void bindAudio(NvConnection connection, int number) { audioBindings++; }
        @Override public void controllerAudio(int kind, byte[] payload) { audioPackets++; }
        @Override public void controllerHaptics(int sequence, byte[] pcm) { waveforms++; }
        @Override public boolean headphoneAudio(short[] samples, int length, float volume) { headphonePackets++; return true; }
        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short low, short high) { }
        @Override public void rumbleTriggers(short left, short right) { }
    }
}
