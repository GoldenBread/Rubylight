package com.limelight.binding.input;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.hardware.input.InputManager;
import android.view.InputDevice;
import android.view.KeyEvent;

import com.limelight.R;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
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

import java.util.Map;

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
}
