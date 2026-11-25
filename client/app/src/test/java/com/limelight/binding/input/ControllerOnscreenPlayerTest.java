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
import com.limelight.binding.input.ControllerPreparationTestSupport.Pad;
import static com.limelight.binding.input.ControllerPreparationTestSupport.*;

/**
 * Verifies on-screen-controller-as-player-one slot isolation, handheld Back-as-Guide routing,
 * device sensor/vibrator fallback, and Controllers-Off behavior for {@link ControllerHandler}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {ControllerPreparationTestSupport.ControllerBridge.class, UsbPads.Connection.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllerOnscreenPlayerTest {
    @Test public void onscreenOwnPlayerPutsAttachedPadsAfterIt() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 4, true, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            // What the launch tells the host: controller 1 and the two pads behind it.
            assertEquals(7, ControllerHandler.getAttachedControllerMask(activity.get()));
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(3, prepared.length);
                assertTrue(prepared[0], prepared[0].startsWith("0:" + MoonBridge.LI_CTYPE_XBOX + ":0:"));
                assertEquals(0, Integer.parseUnsignedInt(prepared[0].split(":")[3]) & ControllerPacket.TOUCHPAD_FLAG);
                assertTrue(prepared[1], prepared[1].startsWith("1:"));
                assertTrue(prepared[2], prepared[2].startsWith("2:"));
                assertEquals(1, contexts(handler).get(2).controllerNumber);
                assertEquals(2, contexts(handler).get(4).controllerNumber);

                connection.events.clear();
                handler.resendControllerArrivals();
                // First to arrive and first with a state, or the PC's own numbering starts with a pad.
                assertEquals("arrival 0", connection.events.get(0));
                assertEquals("input 0 0", connection.events.get(1));
                assertEquals(7, connection.activeMask);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerKeepsItsNumberThroughAPadLeavingAndComingBack() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.resendControllerArrivals();
                assertEquals(1, connection.activeMask);

                // Connected after the stream started.
                addInputDevice(activity.get(), 5, true, JOYPAD);
                handler.onInputDeviceAdded(5);
                assertEquals(1, contexts(handler).get(5).controllerNumber);
                assertEquals(3, connection.activeMask);

                handler.onInputDeviceRemoved(5);
                assertEquals(1, connection.activeMask);

                // Back with a new id: the same player, and no pad ever on controller 1.
                addInputDevice(activity.get(), 6, true, JOYPAD);
                handler.onInputDeviceAdded(6);
                assertEquals(1, contexts(handler).get(6).controllerNumber);
                assertEquals(3, connection.activeMask);

                // The stream menu hiding the controls keeps every number.
                prefs.onscreenController = false;
                handler.reportOscState(0, (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                assertEquals(3, connection.activeMask);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerDoesNotMixWithBuiltInOrUsbPads() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, false, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.deviceAdded(new Pad());
                handler.resendControllerArrivals();
                ControllerHandler.InputDeviceContext builtIn = contexts(handler).get(2);
                assertNotEquals(0, builtIn.controllerNumber);
                assertTrue(builtIn.reservedControllerNumber);
                assertEquals(7, connection.activeMask & 7);

                connection.events.clear();
                handler.reportOscState(ControllerPacket.A_FLAG, (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                assertEquals(Collections.singletonList("input 0 " + ControllerPacket.A_FLAG), connection.events);

                // A pad's state stays on its own number, without the on-screen press.
                connection.events.clear();
                builtIn.inputMap = ControllerPacket.B_FLAG;
                ReflectionHelpers.callInstanceMethod(handler, "sendControllerInputPacket",
                        ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, builtIn));
                assertEquals(Collections.singletonList("input " + builtIn.controllerNumber + " " + ControllerPacket.B_FLAG),
                        connection.events);
            } finally { handler.destroy(); }
        }
    }

    @Test public void controllersOffStillSendsTheOnscreenGamepadButNotAPhysicalPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, false, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.sendControllers = false;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.resendControllerArrivals();
                ControllerHandler.InputDeviceContext builtIn = contexts(handler).get(2);
                connection.events.clear();
                handler.reportOscState(ControllerPacket.A_FLAG, (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                assertEquals(Collections.singletonList("input 0 " + ControllerPacket.A_FLAG), connection.events);

                connection.events.clear();
                builtIn.inputMap = ControllerPacket.B_FLAG;
                ReflectionHelpers.callInstanceMethod(handler, "sendControllerInputPacket",
                        ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, builtIn));
                assertEquals(Collections.emptyList(), connection.events);
            } finally { handler.destroy(); }
        }
    }

    private static String launchWithPhysicalPads(Activity activity, PreferenceConfiguration prefs) {
        Connection connection = new Connection(73);
        ControllerHandler handler = new ControllerHandler(activity, connection, null, prefs);
        try {
            handler.deviceAdded(new Pad(42, (short) 0));
            return handler.prepareControllersForLaunch();
        } finally { handler.destroy(); }
    }

    /** A controller connected during the stream is reported, also one the USB driver claims (Controllers Off notice). */
    @Test public void aControllerConnectedLaterIsReportedAlsoFromTheUsbDriver() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 9, true, InputDevice.SOURCE_DPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.sendControllers = false;
            ControllerHandler handler = new ControllerHandler(activity.get(), new Connection(73), null, prefs);
            try {
                int[] added = {0};
                handler.setControllerAddedListener(() -> added[0]++);
                handler.onInputDeviceAdded(9);
                assertEquals("a remote's D-pad is no controller", 0, added[0]);
                handler.onInputDeviceAdded(2);
                assertEquals(1, added[0]);
                handler.deviceAdded(new Pad(42, (short) 0));
                assertEquals("the USB driver's controller is no input device, and is reported too", 2, added[0]);
            } finally { handler.destroy(); }
        }
    }

    @Test public void controllersOffLaunchAnnouncesNoPhysicalPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 8, false, JOYPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            // On: the external, built-in and USB pads are announced, as before.
            String on = launchWithPhysicalPads(activity.get(), prefs);
            assertEquals(on, 2, on.split(",").length);
            assertTrue(on, on.startsWith("0:") && on.contains(",1:"));
            // Off: none of them.
            prefs.sendControllers = false;
            assertEquals("", launchWithPhysicalPads(activity.get(), prefs));
        }
    }

    @Test public void controllersOffLaunchStillAnnouncesTheOnscreenGamepad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            prefs.showTouchpadButton = true;
            String onscreen = "0:" + MoonBridge.LI_CTYPE_PS + ":0:" + ControllerHandler.ONSCREEN_PAD_BUTTONS;
            // On: the pad in player 1's slot keeps its type and the on-screen gamepad stays quiet.
            String on = launchWithPhysicalPads(activity.get(), prefs);
            assertTrue(on, on.startsWith("0:" + MoonBridge.LI_CTYPE_XBOX + ":"));
            // Off: only the on-screen gamepad, which the physical pads no longer push aside.
            prefs.sendControllers = false;
            assertEquals(onscreen, launchWithPhysicalPads(activity.get(), prefs));
            // Off, with the on-screen gamepad as its own player: still only it.
            PreferenceConfiguration own = onscreenOwnPlayerPrefs(activity.get());
            own.sendControllers = false;
            String ownList = launchWithPhysicalPads(activity.get(), own);
            assertEquals(ownList, 1, ownList.split(",").length);
            assertTrue(ownList, ownList.startsWith("0:"));
        }
    }

    @Test public void controllersOffWithTheOnscreenControlsOffAnnouncesNoOnscreenGamepad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            for (boolean steam : new boolean[] {false, true}) {
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.multiController = true;
                prefs.onscreenController = false;
                prefs.showTouchpadButton = !steam;
                prefs.oscSteamControls = steam;
                prefs.sendControllers = false;
                Connection connection = new Connection(73);
                ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
                try {
                    assertEquals("Steam controls " + steam, "", handler.prepareControllersForLaunch());
                    handler.resendControllerArrivals();
                    assertEquals("Steam controls " + steam, 0, connection.arrivals);
                } finally { handler.destroy(); }
            }
        }
    }

    @Test public void controllersOffSendsNoReleaseForAUsbPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            for (boolean send : new boolean[] {true, false}) {
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.multiController = true;
                prefs.gamepadMotionSensors = true;
                prefs.sendControllers = send;
                Connection connection = new Connection(73);
                ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
                try {
                    Pad pad = new Pad(42, (short) (MoonBridge.LI_CCAP_TOUCHPAD | MoonBridge.LI_CCAP_GYRO));
                    handler.deviceAdded(pad);
                    handler.prepareControllersForLaunch();
                    java.util.Map<Integer, ControllerHandler.UsbDeviceContext> usb =
                            ReflectionHelpers.getField(handler, "usbDeviceContexts");
                    ReflectionHelpers.setField(usb.get(42), "arrivalPending", false);
                    handler.releaseInput();
                    handler.deviceRemoved(pad);
                    assertEquals("touch releases with Controllers " + send, send ? 2 : 0, connection.touches);
                    assertEquals("gyro at rest with Controllers " + send, send ? 1 : 0, connection.motions);
                } finally { handler.destroy(); }
            }
        }
    }

    @Test public void controllersOffSendsNoPhysicalPadInTheActiveMask() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 4, true, JOYPAD);
            for (boolean own : new boolean[] {false, true}) {
                PreferenceConfiguration prefs = own ? onscreenOwnPlayerPrefs(activity.get()) : new PreferenceConfiguration();
                prefs.multiController = true;
                prefs.onscreenController = true;
                prefs.sendControllers = false;
                Connection connection = new Connection(73);
                ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
                try {
                    handler.prepareControllersForLaunch();
                    handler.resendControllerArrivals();
                    handler.reportOscState(ControllerPacket.A_FLAG, (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                    assertEquals("own player " + own, 1, connection.activeMask);
                } finally { handler.destroy(); }
            }
            // With Controllers On both pads stay in the mask.
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.prepareControllersForLaunch();
                handler.reportOscState(ControllerPacket.A_FLAG, (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                assertEquals(3, connection.activeMask & 3);
            } finally { handler.destroy(); }
        }
    }

    @Test public void controllersOffAnnouncesNoPadAttachedMidStreamAndSendsNoBatteryOrPadMouse() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            for (boolean send : new boolean[] {true, false}) {
                int first = send ? 2 : 12, midStream = first + 1;
                addInputDevice(activity.get(), first, true, JOYPAD);
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.multiController = true;
                prefs.sendControllers = send;
                Connection connection = new Connection(73);
                ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
                try {
                    handler.prepareControllersForLaunch();
                    handler.resendControllerArrivals();
                    int atStart = connection.arrivals;
                    assertEquals("arrivals at start with Controllers " + send, send, atStart > 0);
                    // Attached mid-stream: an Android pad and a USB pad.
                    addInputDevice(activity.get(), midStream, true, JOYPAD);
                    handler.onInputDeviceAdded(midStream);
                    handler.deviceAdded(new Pad(42, (short) 0));
                    assertEquals("mid-stream arrivals with Controllers " + send, send, connection.arrivals > atStart);

                    handler.reportControllerBattery(42, MoonBridge.LI_BATTERY_STATE_DISCHARGING, (byte) 50);
                    ControllerHandler.InputDeviceContext pad = contexts(handler).get(midStream);
                    Object reading = ReflectionHelpers.callConstructor(
                            Class.forName(ControllerHandler.class.getName() + "$BatteryReading"),
                            ReflectionHelpers.ClassParameter.from(int.class, android.hardware.BatteryState.STATUS_DISCHARGING),
                            ReflectionHelpers.ClassParameter.from(float.class, 0.5f));
                    ReflectionHelpers.callInstanceMethod(handler, "sendControllerBatteryPacket",
                            ReflectionHelpers.ClassParameter.from(ControllerHandler.InputDeviceContext.class, pad),
                            ReflectionHelpers.ClassParameter.from(reading.getClass(), reading));
                    assertEquals("battery events with Controllers " + send, send ? 2 : 0, connection.batteries);

                    pad.mouseEmulationActive = true;
                    pad.leftStickX = 32000;
                    pad.mouseEmulationRunnable.run();
                    pad.mouseEmulationActive = false;
                    assertEquals("pad mouse moves with Controllers " + send, send ? 1 : 0, connection.mouseMoves);
                } catch (ClassNotFoundException e) {
                    throw new AssertionError(e);
                } finally { handler.destroy(); }
            }
        }
    }

    @Test public void onscreenOwnPlayerAnnouncesTheTouchpadPadItsButtonNeeds() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertTrue(prepared[0], prepared[0].startsWith("0:" + MoonBridge.LI_CTYPE_PS + ":"));
                assertEquals(ControllerPacket.TOUCHPAD_FLAG,
                        Integer.parseUnsignedInt(prepared[0].split(":")[3]) & ControllerPacket.TOUCHPAD_FLAG);
                assertTrue(prepared[1], prepared[1].startsWith("1:" + MoonBridge.LI_CTYPE_XBOX + ":"));
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerNeedsOnscreenControlsMultipleControllersAndMoreThanL3R3() {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.onscreenOwnPlayer = prefs.multiController = prefs.onscreenController = true;
        assertTrue(ControllerHandler.onscreenOwnsPlayerOne(prefs));
        prefs.onlyL3R3 = true;
        assertFalse(ControllerHandler.onscreenOwnsPlayerOne(prefs));
        prefs.onlyL3R3 = false;
        prefs.multiController = false;
        assertFalse(ControllerHandler.onscreenOwnsPlayerOne(prefs));
        prefs.multiController = true;
        prefs.onscreenController = false;
        assertFalse(ControllerHandler.onscreenOwnsPlayerOne(prefs));
        prefs.onscreenController = true;
        prefs.onscreenOwnPlayer = false;
        assertFalse(ControllerHandler.onscreenOwnsPlayerOne(prefs));
    }

    @Test public void withoutTheOptionAPadStillSharesPlayerOneWithTheOnscreenControls() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.onscreenOwnPlayer = false;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                assertTrue(handler.prepareControllersForLaunch().startsWith("0:"));
                assertEquals(0, contexts(handler).get(2).controllerNumber);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerSendsAHandheldsBackAsGuideAndDeviceLessInputOnItsBuiltInPad() {
        org.robolectric.Shadows.shadowOf(org.robolectric.RuntimeEnvironment.getApplication()).declareComponentUnbindable(
                new android.content.ComponentName("com.nvidia.blakepairing", "com.nvidia.blakepairing.AccessoryService"));
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 8, false, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.backAsGuide = true;
            // Guide at once, not held back for Hold Guide to open the stream menu
            prefs.guideMenuHoldMs = 0;
            Connection connection = new Connection(73);
            ControllerHandler handler = handheld(activity.get(), connection, prefs);
            try {
                handler.prepareControllersForLaunch();
                handler.resendControllerArrivals();
                ControllerHandler.InputDeviceContext builtIn = contexts(handler).get(8);
                assertEquals(2, builtIn.controllerNumber);

                connection.events.clear();
                assertTrue(handler.handleHandheldBackAsGuide(systemBack(android.view.KeyEvent.ACTION_DOWN)));
                assertEquals(Collections.singletonList("input 2 " + ControllerPacket.SPECIAL_BUTTON_FLAG), connection.events);
                // A chord with the built-in buttons reaches the PC as one pad.
                builtIn.inputMap = ControllerPacket.RB_FLAG;
                send(handler, builtIn);
                assertEquals("input 2 " + (ControllerPacket.SPECIAL_BUTTON_FLAG | ControllerPacket.RB_FLAG),
                        connection.events.get(connection.events.size() - 1));
                builtIn.inputMap = 0;
                assertTrue(handler.handleHandheldBackAsGuide(systemBack(android.view.KeyEvent.ACTION_UP)));
                org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper())
                        .idleFor(25, java.util.concurrent.TimeUnit.MILLISECONDS);
                assertEquals("input 2 0", connection.events.get(connection.events.size() - 1));
                for (String event : connection.events) assertFalse(event, event.startsWith("input 0 "));

                // Events from no device join the built-in pad too, not the on-screen controls.
                ControllerHandler.InputDeviceContext noDevice = ReflectionHelpers.callInstanceMethod(handler, "getContextForEvent",
                        ReflectionHelpers.ClassParameter.from(android.view.InputEvent.class, new android.view.KeyEvent(0, 0,
                                android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_BUTTON_A, 0, 0, 0, 0)));
                assertNotSame(ReflectionHelpers.getField(handler, "defaultContext"), noDevice);
                assertEquals(2, noDevice.controllerNumber);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerSendsBackAsGuideOnTheFirstPadNumberWithoutABuiltInPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.backAsGuide = true;
            prefs.guideMenuHoldMs = 0;
            Connection connection = new Connection(73);
            ControllerHandler handler = handheld(activity.get(), connection, prefs);
            try {
                handler.resendControllerArrivals();
                connection.events.clear();
                assertTrue(handler.handleHandheldBackAsGuide(systemBack(android.view.KeyEvent.ACTION_DOWN)));
                assertEquals(Collections.singletonList("input 1 " + ControllerPacket.SPECIAL_BUTTON_FLAG), connection.events);
                assertTrue(handler.handleHandheldBackAsGuide(systemBack(android.view.KeyEvent.ACTION_UP)));
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerJoinsStickLessBuiltInButtonsToTheBuiltInPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 8, false, JOYPAD);
            // Start, Select and Home on a kernel device of their own, as on some handhelds.
            addInputDevice(activity.get(), 9, false, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                assertEquals(3, handler.prepareControllersForLaunch().split(",").length);
                handler.resendControllerArrivals();
                assertEquals(7, connection.activeMask);
                int arrivals = connection.arrivals;

                // What its first button press does (Robolectric's KeyEvent has no device).
                ControllerHandler.InputDeviceContext buttons = ReflectionHelpers.callInstanceMethod(handler,
                        "createInputDeviceContextForDevice",
                        ReflectionHelpers.ClassParameter.from(InputDevice.class, InputDevice.getDevice(9)));
                contexts(handler).put(9, buttons);
                buttons.inputMap = ControllerPacket.PLAY_FLAG;
                send(handler, buttons);
                assertEquals(contexts(handler).get(8).controllerNumber, buttons.controllerNumber);
                assertFalse(buttons.reservedControllerNumber);
                // No controller more on the PC.
                assertEquals(7, connection.activeMask);
                assertEquals("input 2 " + ControllerPacket.PLAY_FLAG, connection.events.get(connection.events.size() - 1));

                // Gone again, the built-in pad keeps its number.
                handler.onInputDeviceRemoved(9);
                assertEquals(7, connection.activeMask);
                assertTrue(connection.arrivals >= arrivals);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerGivesStickLessBuiltInButtonsANumberOfTheirOwnWithoutABuiltInPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 9, false, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.resendControllerArrivals();
                ControllerHandler.InputDeviceContext buttons = ReflectionHelpers.callInstanceMethod(handler,
                        "createInputDeviceContextForDevice",
                        ReflectionHelpers.ClassParameter.from(InputDevice.class, InputDevice.getDevice(9)));
                contexts(handler).put(9, buttons);
                buttons.inputMap = ControllerPacket.A_FLAG;
                send(handler, buttons);
                // Active on the PC, or hosts drop its packets.
                assertEquals(1, buttons.controllerNumber);
                assertTrue(buttons.reservedControllerNumber);
                assertEquals(3, connection.activeMask);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerGivesTheDevicesMotorsAndSensorsToTheFirstPadNotTheOnscreenControls() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            android.hardware.SensorManager sensors = (android.hardware.SensorManager)
                    activity.get().getSystemService(Context.SENSOR_SERVICE);
            for (int type : new int[] {android.hardware.Sensor.TYPE_ACCELEROMETER, android.hardware.Sensor.TYPE_GYROSCOPE}) {
                org.robolectric.Shadows.shadowOf(sensors).addSensor(org.robolectric.shadows.ShadowSensor.newInstance(type));
            }
            android.os.Vibrator vibrator = (android.os.Vibrator) activity.get().getSystemService(Context.VIBRATOR_SERVICE);
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 4, true, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.gamepadMotionSensors = true;
            prefs.gamepadMotionSensorsFallbackToDevice = true;
            prefs.vibrateFallbackToDevice = true;
            prefs.vibrateOsc = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.prepareControllersForLaunch();
                ControllerHandler.InputDeviceContext first = contexts(handler).get(2);
                ControllerHandler.InputDeviceContext second = contexts(handler).get(4);
                assertEquals(1, first.controllerNumber);
                assertEquals(2, second.controllerNumber);
                // Motion: the first pad streams the device's sensors, and its own gyroscope gets the liveness check.
                assertSame(sensors, first.sensorManager);
                assertNotSame(sensors, second.sensorManager);
                assertTrue(handler.checksGyroLiveness((short) 1, null));
                assertFalse(handler.checksGyroLiveness((short) 0, null));
                assertFalse(handler.checksGyroLiveness((short) 2, null));

                // Rumble: the second pad without a motor leaves the phone still.
                handler.handleRumble((short) 2, (short) 0x8000, (short) 0x8000);
                assertFalse(org.robolectric.Shadows.shadowOf(vibrator).isVibrating());
                // The first pad without a motor vibrates the phone, and its removal stops it.
                handler.handleRumble((short) 1, (short) 0x8000, (short) 0x8000);
                assertTrue(org.robolectric.Shadows.shadowOf(vibrator).isVibrating());
                assertTrue(ReflectionHelpers.<Boolean>getField(handler, "deviceVibratorForPlayerOne"));
                handler.onInputDeviceRemoved(2);
                assertFalse(org.robolectric.Shadows.shadowOf(vibrator).isVibrating());
                // Controller 1 has no pad: a game's rumble for it is the on-screen controls' own.
                handler.handleRumble((short) 0, (short) 0x8000, (short) 0x8000);
                assertTrue(org.robolectric.Shadows.shadowOf(vibrator).isVibrating());
                assertFalse(ReflectionHelpers.<Boolean>getField(handler, "deviceVibratorForPlayerOne"));
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerGivesTheDevicesSensorsToAUsbPadOnTheFirstPadNumber() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            android.hardware.SensorManager sensors = (android.hardware.SensorManager)
                    activity.get().getSystemService(Context.SENSOR_SERVICE);
            for (int type : new int[] {android.hardware.Sensor.TYPE_ACCELEROMETER, android.hardware.Sensor.TYPE_GYROSCOPE}) {
                org.robolectric.Shadows.shadowOf(sensors).addSensor(org.robolectric.shadows.ShadowSensor.newInstance(type));
            }
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.gamepadMotionSensors = true;
            prefs.gamepadMotionSensorsFallbackToDevice = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                Pad first = new Pad(42, (short) 0);
                Pad second = new Pad(43, (short) 0);
                handler.deviceAdded(first);
                handler.deviceAdded(second);
                handler.prepareControllersForLaunch();
                java.util.Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
                assertEquals(1, usb.get(42).controllerNumber);
                assertEquals(2, usb.get(43).controllerNumber);
                assertSame(sensors, ReflectionHelpers.getField(usb.get(42), "deviceSensors"));
                assertNull(ReflectionHelpers.getField(usb.get(43), "deviceSensors"));
            } finally { handler.destroy(); }
        }
    }
}
