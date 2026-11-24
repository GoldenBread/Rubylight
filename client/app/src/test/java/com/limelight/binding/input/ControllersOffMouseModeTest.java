package com.limelight.binding.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.hardware.input.InputManager;
import android.view.InputDevice;
import android.view.KeyEvent;

import com.limelight.R;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowInputDevice;
import org.robolectric.shadows.ShadowToast;
import org.robolectric.util.ReflectionHelpers;

import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

/** With Controllers Off a physical pad's mouse is silenced, so mouse mode does not turn on for it. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllersOffMouseModeTest {
    private static final int PAD_DEVICE = 5;

    private static class Connection extends NvConnection {
        Connection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }
    }

    private static ControllerHandler handler(ControllerInputReleaseTest.MenuGame activity, boolean sendControllers) {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.sendControllers = sendControllers;
        prefs.mouseEmulation = true;
        return new ControllerHandler(activity, new Connection(), null, prefs) {
            @Override protected boolean[] findInternalGamepadAndSelect() { return new boolean[] {true, true, true}; }
        };
    }

    private static ControllerHandler.InputDeviceContext physicalPad(ControllerHandler handler,
                                                                     ControllerInputReleaseTest.MenuGame activity) {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed("Pad");
        ReflectionHelpers.setField(device, "mId", PAD_DEVICE);
        ReflectionHelpers.setField(device, "mSources", InputDevice.SOURCE_GAMEPAD);
        ReflectionHelpers.setField(device, "mMotionRanges", new java.util.ArrayList<InputDevice.MotionRange>());
        org.robolectric.Shadows.shadowOf((InputManager) activity.getSystemService(Context.INPUT_SERVICE))
                .addInputDevice(device);
        Map<Integer, ControllerHandler.InputDeviceContext> devices = ReflectionHelpers.getField(handler, "inputDeviceContexts");
        ControllerHandler.InputDeviceContext pad = handler.new InputDeviceContext();
        pad.assignedControllerNumber = true;
        pad.buttonMap = new ControllerButtonMap();
        pad.inputDevice = device;
        devices.put(PAD_DEVICE, pad);
        return pad;
    }

    private static KeyEvent start(ControllerHandler handler, int action, long time, int deviceId) {
        KeyEvent event = new KeyEvent(1000, time, action, KeyEvent.KEYCODE_BUTTON_START, 0, 0, deviceId, 0, 0,
                InputDevice.SOURCE_GAMEPAD);
        if (deviceId == PAD_DEVICE) org.robolectric.Shadows.shadowOf(event).setDevice(InputDevice.getDevice(PAD_DEVICE));
        return event;
    }

    private static void startLongPress(ControllerHandler handler, int deviceId) {
        handler.handleButtonDown(start(handler, KeyEvent.ACTION_DOWN, 1000, deviceId));
        handler.handleButtonUp(start(handler, KeyEvent.ACTION_UP, 1850, deviceId));
    }

    private static boolean toastedMouseOn() {
        String on = RuntimeEnvironment.getApplication().getString(R.string.stream_mouse_mode_on);
        return ShadowToast.showedToast(on);
    }

    @Test
    public void controllersOffAPhysicalPadStartLongPressDoesNotTurnMouseModeOn() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        ControllerHandler handler = handler(activity, false);
        physicalPad(handler, activity);
        try {
            startLongPress(handler, PAD_DEVICE);
            assertFalse(handler.isMouseEmulationActive());
            assertFalse("no mouse mode on toast", toastedMouseOn());
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void controllersOffTheMenuSwitchDoesNotTurnAPhysicalPadMouseOn() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        ControllerHandler handler = handler(activity, false);
        physicalPad(handler, activity);
        try {
            handler.toggleMouseEmulationFromMenu();
            assertFalse(handler.isMouseEmulationActive());
            assertFalse("no mouse mode on toast", toastedMouseOn());
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void controllersOnAPhysicalPadStartLongPressTurnsMouseModeOn() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        ControllerHandler handler = handler(activity, true);
        ControllerHandler.InputDeviceContext pad = physicalPad(handler, activity);
        try {
            startLongPress(handler, PAD_DEVICE);
            assertTrue(pad.mouseEmulationActive);
            assertTrue(handler.isMouseEmulationActive());
            assertTrue(toastedMouseOn());
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void controllersOffTheOnScreenGamepadContextStillTurnsMouseModeOnAndOff() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        ControllerHandler handler = handler(activity, false);
        ControllerHandler.InputDeviceContext onscreen = ReflectionHelpers.getField(handler, "defaultContext");
        try {
            onscreen.toggleMouseEmulation();
            assertTrue(handler.isMouseEmulationActive());
            assertTrue(toastedMouseOn());
            handler.toggleMouseEmulationFromMenu();
            assertFalse("turning it off is allowed", handler.isMouseEmulationActive());
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void controllersOffTheMenuStillOffersTheSwitchToTurnAnActiveMouseOff() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        ControllerHandler handler = handler(activity, false);
        physicalPad(handler, activity);
        ControllerHandler.InputDeviceContext onscreen = ReflectionHelpers.getField(handler, "defaultContext");
        try {
            assertFalse("nothing to turn on or off yet", handler.hasControllers());
            onscreen.toggleMouseEmulation();
            assertTrue(handler.isMouseEmulationActive());
            assertTrue("the switch can turn the active mouse off", handler.hasControllers());
            handler.toggleMouseEmulationFromMenu();
            assertFalse(handler.isMouseEmulationActive());
            assertFalse("no longer offered once it is off", handler.hasControllers());
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void concurrentUsbReportAndResendArrivalsWithMouseModeActiveNeverDeadlock() throws Exception {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        List<Byte> mouseUps = new CopyOnWriteArrayList<>();
        NvConnection streamConn = new NvConnection(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989),
                47984, "test", new StreamConfiguration.Builder().build(), null, null) {
            @Override public boolean canSendControllerInput() { return true; }
            @Override public int sendControllerIdentityEvent(byte n, int vid, int pid) { return 0; }
            @Override public int sendControllerArrivalEvent(byte n, short mask, byte type, int btnFlags, short caps) { return 0; }
            @Override public void sendControllerInput(short n, short mask, int b, byte lt, byte rt, short lx, short ly, short rx, short ry) {}
            @Override public void sendMouseButtonDown(byte button) {}
            @Override public void sendMouseButtonUp(byte button) { mouseUps.add(button); }
        };
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.sendControllers = true;
        prefs.mouseEmulation = true;
        ControllerHandler handler = new ControllerHandler(activity, streamConn, null, prefs) {
            @Override protected boolean[] findInternalGamepadAndSelect() { return new boolean[] {true, true, true}; }
        };
        Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
        ControllerHandler.UsbDeviceContext pad = handler.new UsbDeviceContext();
        pad.device = new AbstractController(3, null, 0x054c, 0x0ce6) {
            @Override public boolean start() { return true; }
            @Override public void stop() {}
            @Override public void rumble(short low, short high) {}
            @Override public void rumbleTriggers(short left, short right) {}
        };
        pad.assignedControllerNumber = true;
        pad.buttonMap = new ControllerButtonMap();
        pad.mouseEmulationActive = true;
        usb.put(3, pad);

        final int iterations = 250;
        CountDownLatch ready = new CountDownLatch(2);
        CountDownLatch start = new CountDownLatch(1);
        AtomicBoolean done = new AtomicBoolean(false);
        AtomicReference<Throwable> error = new AtomicReference<>();

        Thread resendThread = new Thread(() -> {
            ready.countDown();
            try {
                start.await();
                for (int i = 0; i < iterations && !done.get(); i++) {
                    handler.resendControllerArrivals();
                }
            } catch (Throwable t) {
                error.compareAndSet(null, t);
            }
        }, "test-resend-arrivals");

        Thread usbThread = new Thread(() -> {
            ready.countDown();
            try {
                start.await();
                for (int i = 0; i < iterations && !done.get(); i++) {
                    int btn = (i & 1) == 0 ? ControllerPacket.A_FLAG : 0;
                    handler.reportControllerState(3, btn, 0f, 0f, 0f, 0f, 0f, 0f);
                }
            } catch (Throwable t) {
                error.compareAndSet(null, t);
            }
        }, "test-usb-report");

        try {
            resendThread.start();
            usbThread.start();
            assertTrue(ready.await(5, TimeUnit.SECONDS));
            start.countDown();

            resendThread.join(5000);
            usbThread.join(5000);
            done.set(true);

            assertFalse("resendThread timed out (state=" + resendThread.getState() + ")", resendThread.isAlive());
            assertFalse("usbThread timed out (state=" + usbThread.getState() + ")", usbThread.isAlive());
            assertNull(error.get());

            pad.mouseEmulationLastInputMap = ControllerPacket.A_FLAG | ControllerPacket.B_FLAG;
            pad.mouseEmulationFixedStepHeld = true;
            pad.toggleMouseEmulation();
            assertFalse(pad.mouseEmulationActive);
            assertFalse(pad.mouseEmulationFixedStepHeld);
            assertEquals(0, pad.mouseEmulationLastInputMap);
            assertTrue(mouseUps.contains(MouseButtonPacket.BUTTON_LEFT));
            assertTrue(mouseUps.contains(MouseButtonPacket.BUTTON_RIGHT));
        } finally {
            done.set(true);
            resendThread.interrupt();
            usbThread.interrupt();
            usb.clear();
            handler.destroy();
        }
    }
}
