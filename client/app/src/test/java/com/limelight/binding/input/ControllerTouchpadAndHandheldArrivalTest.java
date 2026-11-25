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

import com.limelight.binding.input.ControllerPreparationTestSupport.Connection;
import static com.limelight.binding.input.ControllerPreparationTestSupport.*;

/**
 * Verifies PlayStation touchpad-button arrivals, handheld built-in gamepad classification,
 * and external-controller presence detection for {@link ControllerHandler}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {ControllerPreparationTestSupport.ControllerBridge.class, UsbPads.Connection.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllerTouchpadAndHandheldArrivalTest {
    @Test public void theOnScreenTouchpadButtonAsksTheHostForAPlayStationPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            // Without the Touchpad button the on-screen pad stays quiet, as before: the host's default pad.
            Connection quiet = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), quiet, null, prefs);
            try {
                assertEquals("", handler.prepareControllersForLaunch());
                handler.resendControllerArrivals();
                assertEquals(0, quiet.arrivals);
            } finally { handler.destroy(); }

            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                assertEquals("0:" + MoonBridge.LI_CTYPE_PS + ":0:" + ControllerHandler.ONSCREEN_PAD_BUTTONS,
                        handler.prepareControllersForLaunch());
                handler.resendControllerArrivals();
                assertEquals(1, connection.arrivals);
                assertEquals(0, connection.arrivalSlot);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
                assertTrue((connection.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG) != 0);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aPadInPlayerOnesSlotKeepsItsTypeWithTheTouchpadButton() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(1, prepared.length);
                assertTrue(prepared[0].startsWith("0:1:"));
                int before = connection.arrivals;
                handler.resendControllerArrivals();
                assertEquals(before + 1, connection.arrivals);
                assertTrue(connection.arrivalType != MoonBridge.LI_CTYPE_PS);
            } finally { handler.destroy(); }
        }
    }

    @Test public void builtInControlsCarryTheTouchpadButtonAsAPlayStationPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 8, false, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            // Without the Touchpad button the handheld's controls keep their own type.
            Connection quiet = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), quiet, null, prefs);
            try {
                assertTrue(handler.prepareControllersForLaunch().startsWith("0:1:"));
                assertTrue(quiet.arrivalType != MoonBridge.LI_CTYPE_PS);
                assertEquals(0, quiet.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG);
            } finally { handler.destroy(); }

            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String prepared = handler.prepareControllersForLaunch();
                assertTrue(prepared, prepared.startsWith("0:" + MoonBridge.LI_CTYPE_PS + ":"));
                assertTrue((Integer.parseUnsignedInt(prepared.split(":")[3]) & ControllerPacket.TOUCHPAD_FLAG) != 0);
                handler.resendControllerArrivals();
                assertEquals(0, connection.arrivalSlot);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
                assertTrue((connection.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG) != 0);
                assertEquals("One pad for player 1, not a second on-screen one", 1, connection.typeBySlot.size());
            } finally { handler.destroy(); }
        }
    }

    @Test public void playerOnesTypeStaysForTheSessionWhenTheOnScreenControlsToggle() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 8, false, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            // Off at launch, turned on from the stream menu later to press Touchpad.
            prefs.onscreenController = false;
            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                assertTrue(handler.prepareControllersForLaunch().startsWith("0:" + MoonBridge.LI_CTYPE_PS + ":"));
                for (boolean shown : new boolean[] {true, false}) {
                    prefs.onscreenController = shown;
                    handler.resendControllerArrivals();
                    assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
                    assertTrue((connection.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG) != 0);
                }
            } finally { handler.destroy(); }
        }
    }

    @Test public void aHandheldsOwnPadListedAsExternalIsToldApartFromARealXboxPad() {
        // Retroid Pocket 6: "Xbox Wireless Controller", 0x2022:0x3002, external, on an internal bus and absent
        // from the USB device list, so the name and vendor check decides.
        assertTrue(ControllerHandler.looksLikeHandheldControls("Xbox Wireless Controller", 0x2022, true, false));
        assertFalse("A real Xbox pad", ControllerHandler.looksLikeHandheldControls("Xbox Wireless Controller",
                0x045e, true, false));
        assertFalse("Beside another pad", ControllerHandler.looksLikeHandheldControls("Xbox Wireless Controller",
                0x2022, false, false));
        assertFalse("Attached by USB", ControllerHandler.looksLikeHandheldControls("Xbox Wireless Controller",
                0x2022, true, true));
        assertFalse("Another kind of pad", ControllerHandler.looksLikeHandheldControls("8BitDo Pro 2",
                0x2dc8, true, false));
    }

    /** LOW: a fight stick with only a hat and buttons has no X/Y sticks, and is still a controller. */
    @Test public void aPadWithOnlyAHatAndButtonsCountsAsAController() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 7, true, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD,
                    "Fight Stick", 0x0f0d, 0x00c1);
            ControllerHandler handler = new ControllerHandler(activity.get(), new Connection(73), null,
                    new PreferenceConfiguration());
            try {
                assertTrue("Buttons and a hat are a pad", handler.hasExternalControllers());
            } finally {
                handler.destroy();
            }
        }
    }

    /** LOW: a TV remote's D-pad and an alphabetic keyboard, even with a pointer nub, are not controllers. */
    @Test public void aRemoteAndAnAlphabeticKeyboardAreNotControllers() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 7, true, InputDevice.SOURCE_DPAD, "Fire TV Remote", 0, 0);
            ControllerHandler handler = new ControllerHandler(activity.get(), new Connection(73), null,
                    new PreferenceConfiguration());
            try {
                assertFalse("A remote's D-pad is no pad", handler.hasExternalControllers());
                // An alphabetic keyboard whose pointer reports joystick axes: sticks alone do not make it a pad.
                addInputDevice(activity.get(), 8, true, InputDevice.SOURCE_KEYBOARD | InputDevice.SOURCE_JOYSTICK,
                        "Keyboard", 0, 0, InputDevice.KEYBOARD_TYPE_ALPHABETIC);
                assertFalse("A keyboard is no pad", handler.hasExternalControllers());
            } finally {
                handler.destroy();
            }
        }
    }

    @Test public void aHandheldsOwnPadCarriesTheTouchpadButton() {
        Connection connection = arrivalOfXboxNamedPad(0x2022);
        assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
        assertTrue((connection.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG) != 0);
    }

    /** Auto-hide must not treat a handheld's built-in pad as a controller that takes the touch controls away. */
    @Test public void aHandheldsOwnPadIsNotAnExternalController() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            // The handheld's only gamepad, listed as external on an internal bus, like a Retroid Pocket's.
            addInputDevice(activity.get(), 7, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD,
                    "Xbox Wireless Controller", 0x2022, 0x3002);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            ControllerHandler handler = new ControllerHandler(activity.get(), new Connection(73), null, prefs);
            try {
                assertFalse("The handheld's own controls never hide the touch controls",
                        handler.hasExternalControllers());
                // A real Xbox pad beside it is an external controller, whichever one is looked at.
                addInputDevice(activity.get(), 9, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD,
                        "Xbox Wireless Controller", 0x045e, 0x0b12);
                assertTrue(handler.hasExternalControllers());
            } finally {
                handler.destroy();
            }
        }
    }

    @Test public void aRealXboxPadKeepsItsTypeBesideTheTouchpadButton() {
        Connection connection = arrivalOfXboxNamedPad(0x045e);
        assertEquals(MoonBridge.LI_CTYPE_XBOX, connection.arrivalType);
        assertEquals(0, connection.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG);
    }

    /** The only gamepad, listed as external and named like a Retroid Pocket's, with the Touchpad button on. */
    private static Connection arrivalOfXboxNamedPad(int vendor) {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 7, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD,
                    "Xbox Wireless Controller", vendor, 0x3002);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String prepared = handler.prepareControllersForLaunch();
                handler.resendControllerArrivals();
                assertTrue(prepared, prepared.startsWith("0:" + connection.arrivalType + ":"));
                assertEquals(0, connection.arrivalSlot);
                return connection;
            } finally { handler.destroy(); }
        }
    }

    @Test public void anExternalPadKeepsItsTypeBesideTheTouchpadButton() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 4, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 8, false, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(2, prepared.length);
                assertTrue(prepared[0].startsWith("0:1:"));
                assertTrue(prepared[1].startsWith("1:1:"));
                handler.resendControllerArrivals();
                for (byte type : connection.typeBySlot.values()) assertTrue(type != MoonBridge.LI_CTYPE_PS);
            } finally { handler.destroy(); }
        }
    }
}
