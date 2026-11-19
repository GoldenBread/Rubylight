package com.limelight.binding.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.view.InputDevice;
import android.view.KeyEvent;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

/** With Controllers set to Off the built-in pad keeps its local actions; only what it would send is dropped. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllersOffLocalActionsTest {
    private static class Connection extends NvConnection {
        final List<String> events = new ArrayList<>();
        Connection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }
        @Override public int sendControllerArrivalEvent(byte slot, short mask, byte type, int buttons, short caps) {
            events.add("arrival"); return 0;
        }
        @Override public void sendControllerInput(short number, short mask, int buttons, byte leftTrigger,
                                                   byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            events.add("input " + number + " " + buttons);
        }
    }

    private static ControllerHandler handler(ControllerInputReleaseTest.MenuGame activity, Connection connection,
                                             PreferenceConfiguration prefs) {
        prefs.sendControllers = false;
        return new ControllerHandler(activity, connection, null, prefs) {
            @Override protected boolean[] findInternalGamepadAndSelect() { return new boolean[] {true, true, true}; }
        };
    }

    @Test
    public void holdingBackAsGuideStillOpensTheStreamMenuAndATapSendsNothing() {
        // The test runs the main looper; the SHIELD accessory service is not on this device.
        org.robolectric.Shadows.shadowOf(org.robolectric.RuntimeEnvironment.getApplication()).declareComponentUnbindable(
                new android.content.ComponentName("com.nvidia.blakepairing", "com.nvidia.blakepairing.AccessoryService"));
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        Connection connection = new Connection();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.backAsGuide = true;
        ControllerHandler handler = handler(activity, connection, prefs);
        int keyboard = InputDevice.SOURCE_KEYBOARD;
        try {
            assertTrue(handler.handleHandheldBackAsGuide(new KeyEvent(1000, 1000, KeyEvent.ACTION_DOWN,
                    KeyEvent.KEYCODE_BACK, 0, 0, -1, 0, 0, keyboard)));
            ShadowLooper.idleMainLooper(prefs.guideMenuHoldMs + 100, TimeUnit.MILLISECONDS);
            assertEquals("held: the stream menu opens", 1, activity.menuCount);
            assertTrue(handler.handleHandheldBackAsGuide(new KeyEvent(1000, 2200, KeyEvent.ACTION_UP,
                    KeyEvent.KEYCODE_BACK, 0, 0, -1, 0, 0, keyboard)));

            assertTrue(handler.handleHandheldBackAsGuide(new KeyEvent(3000, 3000, KeyEvent.ACTION_DOWN,
                    KeyEvent.KEYCODE_BACK, 0, 0, -1, 0, 0, keyboard)));
            assertTrue(handler.handleHandheldBackAsGuide(new KeyEvent(3000, 3050, KeyEvent.ACTION_UP,
                    KeyEvent.KEYCODE_BACK, 0, 0, -1, 0, 0, keyboard)));
            ShadowLooper.idleMainLooper(1000, TimeUnit.MILLISECONDS);
            assertEquals("a tap opens no menu", 1, activity.menuCount);
            assertEquals("and sends nothing", new ArrayList<String>(), connection.events);
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void aKeyThatIsNoGamepadButtonIsNotConsumedFromDeviceZero() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        Connection connection = new Connection();
        ControllerHandler handler = handler(activity, connection, new PreferenceConfiguration());
        try {
            for (int device : new int[] {0}) {
                assertFalse("down from device " + device, handler.handleButtonDown(new KeyEvent(1000, 1000,
                        KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DEL, 0, 0, device, 0)));
                assertFalse("up from device " + device, handler.handleButtonUp(new KeyEvent(1000, 1010,
                        KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DEL, 0, 0, device, 0)));
            }
            assertEquals(new ArrayList<String>(), connection.events);
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void theQuitComboStillLeavesTheStreamWithTheBuiltInPad() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        Connection connection = new Connection();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.gamepadQuitCombo = "lb_rb_a_b";
        ControllerHandler handler = handler(activity, connection, prefs);
        int[] keys = {KeyEvent.KEYCODE_BUTTON_L1, KeyEvent.KEYCODE_BUTTON_R1,
                KeyEvent.KEYCODE_BUTTON_A, KeyEvent.KEYCODE_BUTTON_B};
        try {
            for (int key : keys) {
                assertTrue(handler.handleButtonDown(new KeyEvent(1000, 1000, KeyEvent.ACTION_DOWN, key, 0, 0, 0, 0)));
            }
            for (int key : keys) {
                assertTrue(handler.handleButtonUp(new KeyEvent(1000, 1100, KeyEvent.ACTION_UP, key, 0, 0, 0, 0)));
            }
            assertEquals(1, activity.leaves);
            assertEquals(new ArrayList<String>(), connection.events);
        } finally {
            handler.destroy();
        }
    }
}
