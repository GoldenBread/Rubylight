package com.limelight;

import android.app.Application;
import android.hardware.input.InputManager;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;

import com.limelight.GameKeyboardOverlayTest.Capture;
import com.limelight.GameKeyboardOverlayTest.RecordingConnection;
import com.limelight.binding.input.ControllerButtonMap;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.keyboard.PcKey;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.binding.input.keyboard.PcKeyboardLayout;
import com.limelight.binding.input.keyboard.PcModifier;
import com.limelight.nvstream.input.KeyboardPacket;
import com.limelight.ui.PcKeyboardOverlay;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.Map;

import static com.limelight.GameKeyboardOverlayTest.*;
import static org.junit.Assert.*;

/**
 * Tests the on-screen PC keyboard overlay, WASD stick key ref-counting, controller steering,
 * and focus-loss key releases in {@link Game}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GamePcKeyboardOverlayTest {
    /** The PC keyboard takes Vibrate on press when it opens. */
    @Test
    public void thePcKeyboardTakesVibrateOnPress() {
        for (boolean vibrate : new boolean[] {true, false}) {
            Game game = pcKeyboardGame(new RecordingConnection());
            game.prefConfig.vibrateOsc = vibrate;
            ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
            PcKeyboardOverlay overlay = ReflectionHelpers.getField(game, "pcKeyboard");
            Object keys = ReflectionHelpers.getField(overlay, "keys");
            assertEquals(vibrate, (boolean) ReflectionHelpers.getField(keys, "haptics"));
        }
    }

    @Test
    public void pcKeyboardOpensAndClosesAndExcludesTheOtherKeyboards() {
        Game game = pcKeyboardGame(new RecordingConnection());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(pcKeyboardVisible(game));
        // The extra-keys panel takes over: only one keyboard at a time.
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "toggleKeyboardOverlay");
        assertFalse(pcKeyboardVisible(game));
        View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
        assertEquals(View.VISIBLE, panel.getVisibility());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(pcKeyboardVisible(game));
        assertEquals(View.GONE, panel.getVisibility());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertFalse(pcKeyboardVisible(game));
    }

    @Test
    public void closingThePcKeyboardLetsGoOfEveryHeldAndStickyKey() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        PcKeyboardController keyboard = pcKeyboardController(game);
        PcKeyboardLayout layout = keyboard.layout();
        long now = 1000;
        // Ctrl locked, Shift armed, and A held down.
        keyboard.engine().tapModifier(PcModifier.CTRL);
        keyboard.engine().tapModifier(PcModifier.CTRL);
        keyboard.engine().tapModifier(PcModifier.SHIFT);
        keyboard.touchDown(layout.key("A"), 1, now);
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        assertFalse(pressed.isEmpty());

        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");

        assertFalse(pcKeyboardVisible(game));
        assertTrue("Nothing stays pressed on the host", pressed.isEmpty());
        int ups = 0, downs = 0;
        for (int[] key : connection.keys) {
            if (key[1] == KeyboardPacket.KEY_DOWN) downs++; else ups++;
        }
        assertEquals("Every key sent down is sent up", downs, ups);
        assertTrue(downs >= 3);
    }

    /** nightjar#61: the modern controller's WASD stick sends keys the way the PC keyboard does, and none stays held. */
    @Test
    public void theWasdStickSendsItsKeysLikeThePcKeyboardAndTheyGoUpWithTheRest() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        // The on-screen controls' switch is off by default; this test is about the WASD stick they carry.
        game.prefConfig.onscreenController = true;
        ReflectionHelpers.setField(game, "app", new com.limelight.nvstream.http.NvApp("Game", 7, false));
        game.showOnscreenController();
        assertNotNull(game.modernController);
        // The controller's key sink, as its WASD stick uses it: W, then D for the diagonal.
        Object keys = ReflectionHelpers.getField(game.modernController, "keys");
        ReflectionHelpers.callInstanceMethod(keys, "key", ReflectionHelpers.ClassParameter.from(int.class, 0x57),
                ReflectionHelpers.ClassParameter.from(boolean.class, true));
        ReflectionHelpers.callInstanceMethod(keys, "key", ReflectionHelpers.ClassParameter.from(int.class, 0x44),
                ReflectionHelpers.ClassParameter.from(boolean.class, true));
        assertEquals(2, connection.keys.size());
        assertEquals("The PC keyboard's virtual-key form", 0x8057, connection.keys.get(0)[0]);
        assertEquals(KeyboardPacket.KEY_DOWN, connection.keys.get(0)[1]);
        assertEquals(0x8044, connection.keys.get(1)[0]);
        ReflectionHelpers.callInstanceMethod(keys, "key", ReflectionHelpers.ClassParameter.from(int.class, 0x44),
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
        assertEquals(0x8044, connection.keys.get(2)[0]);
        assertEquals(KeyboardPacket.KEY_UP, connection.keys.get(2)[1]);

        // Losing input lets go of W too, even before the stick itself hears of it.
        game.keyboardInput.releaseKeyboardInput();
        int[] last = connection.keys.get(connection.keys.size() - 1);
        assertEquals(0x8057, last[0]);
        assertEquals(KeyboardPacket.KEY_UP, last[1]);
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        assertTrue(pressed.isEmpty());
        game.modernController.destroy();
    }

    /** The WASD stick and a keyboard holding the same W: W goes up on the PC only once both let go, in either order. */
    @Test
    public void wHeldByTheStickAndAKeyboardStaysDownUntilBothLetGo() {
        int w = 0x8057;
        for (boolean physical : new boolean[] {true, false}) {
            for (boolean stickFirst : new boolean[] {true, false}) {
                RecordingConnection connection = new RecordingConnection();
                Game game = pcKeyboardGame(connection);
                String test = (physical ? "physical keyboard" : "PC keyboard") + (stickFirst ? ", stick first" : ", keyboard first");
                if (stickFirst) {
                    game.keyboardInput.sendOnScreenKey(0x57, true);
                    keyboardW(game, physical, true);
                } else {
                    keyboardW(game, physical, true);
                    game.keyboardInput.sendOnScreenKey(0x57, true);
                }
                assertEquals(test + ": W goes down once", 1, connection.keys.size());
                assertEquals(test, w, connection.keys.get(0)[0]);
                assertEquals(test, KeyboardPacket.KEY_DOWN, connection.keys.get(0)[1]);

                // Either one letting go leaves W down while the other holds it.
                if (stickFirst) game.keyboardInput.sendOnScreenKey(0x57, false); else keyboardW(game, physical, false);
                assertEquals(test + ": W stays down", 1, connection.keys.size());
                if (stickFirst) keyboardW(game, physical, false); else game.keyboardInput.sendOnScreenKey(0x57, false);
                assertEquals(test + ": W goes up once both let go", 2, connection.keys.size());
                assertEquals(test, w, connection.keys.get(1)[0]);
                assertEquals(test, KeyboardPacket.KEY_UP, connection.keys.get(1)[1]);
            }
        }
    }

    /** Losing input lets go of a W the stick and a keyboard both hold once, not twice. */
    @Test
    public void wHeldByTheStickAndAKeyboardGoesUpOnceWhenInputIsLost() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        game.keyboardInput.sendOnScreenKey(0x57, true);
        keyboardW(game, true, true);
        game.keyboardInput.sendOnScreenKey(0x41, true);
        game.keyboardInput.releaseKeyboardInput();
        int wUps = 0, aUps = 0;
        for (int[] key : connection.keys) {
            if (key[1] != KeyboardPacket.KEY_UP) continue;
            if (key[0] == 0x8057) wUps++;
            if (key[0] == 0x8041) aUps++;
        }
        assertEquals(1, wUps);
        assertEquals(1, aUps);
        // The stick lifting afterwards sends nothing more.
        int sent = connection.keys.size();
        game.keyboardInput.sendOnScreenKey(0x57, false);
        game.keyboardInput.sendOnScreenKey(0x41, false);
        assertEquals(sent, connection.keys.size());
    }

    /** W from a physical keyboard, or from the PC keyboard's own key path. */
    private static void keyboardW(Game game, boolean physical, boolean down) {
        if (physical) {
            KeyEvent event = keyboardKey(down ? KeyEvent.ACTION_DOWN : KeyEvent.ACTION_UP, KeyEvent.KEYCODE_W);
            if (down) game.handleKeyDown(event); else game.handleKeyUp(event);
        } else {
            ReflectionHelpers.callInstanceMethod(game.keyboardInput, "sendPcKeyboardKey",
                    ReflectionHelpers.ClassParameter.from(int.class, 0x57),
                    ReflectionHelpers.ClassParameter.from(boolean.class, down),
                    ReflectionHelpers.ClassParameter.from(byte.class, (byte) 0));
        }
    }

    @Test
    public void controllerKeysSteerTheKeyboardWhileOpenAndReachTheGameAfterwards() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_X);
        padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_X);
        assertTrue("A closed keyboard sends no keys", connection.keys.isEmpty());

        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        connection.keys.clear();
        // X is Backspace, Y is Space: pressed and released on the host, and consumed.
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_X));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_X));
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_Y));
        assertEquals(4, connection.keys.size());
        assertEquals(KeyboardPacket.KEY_DOWN, connection.keys.get(0)[1]);
        assertEquals(KeyboardPacket.KEY_UP, connection.keys.get(1)[1]);
        // Buttons without a job are swallowed too, so the game never sees them.
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_THUMBL));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_THUMBL));
        assertEquals(4, connection.keys.size());

        // B closes it. The release that follows still belongs to the keyboard.
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_B));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_B));
        assertFalse(pcKeyboardVisible(game));
        padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_X);
        padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_X);
        assertEquals("Closed again: the pad no longer types", 4, connection.keys.size());
    }

    @Test
    public void closeWithAButtonHeldStillSwallowsItsRelease() {
        Game game = pcKeyboardGame(new RecordingConnection());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertFalse(pcKeyboardVisible(game));
        assertTrue("The game never saw the press, so it must not see the release",
                padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_Y));
        assertEquals(0, heldPadKeys(game).size());
    }

    @Test
    public void noDeviceAndTheSyntheticDeviceAreNotControllers() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        connection.keys.clear();
        PcKey focused = pcKeyboardController(game).focus().focused();
        assertFalse(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y, null, PAD_ID)));
        InputDevice synthetic = gamepad();
        ReflectionHelpers.setField(synthetic, "mId", -1);
        assertFalse(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y, synthetic, -1)));
        InputDevice keys = gamepad();
        ReflectionHelpers.setField(keys, "mSources", InputDevice.SOURCE_KEYBOARD);
        assertFalse(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y, keys, PAD_ID)));
        assertTrue(connection.keys.isEmpty());
        assertSame(focused, pcKeyboardController(game).focus().focused());
        assertEquals(0, heldPadKeys(game).size());
        assertTrue(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y, gamepad(), PAD_ID)));
    }

    @Test
    public void aPressWhileClosedDropsAStaleEntryForTheSameKey() {
        Game game = pcKeyboardGame(new RecordingConnection());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        assertTrue(heldPadKeys(game).get(KeyEvent.KEYCODE_BUTTON_Y));
        // The keyboard closes and the pad never reports the release, then Y is pressed for the game.
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertFalse(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y, gamepad(), PAD_ID)));
        assertEquals(0, heldPadKeys(game).size());
        assertFalse("The release of the game's own press belongs to the game",
                takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_Y, gamepad(), PAD_ID)));
    }

    @Test
    public void releasingKeyboardInputAndUnpluggingThePadForgetHeldButtons() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        assertEquals(1, heldPadKeys(game).size());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "releaseKeyboardInput");
        assertEquals(0, heldPadKeys(game).size());

        // Space is held by the pad when it is unplugged: the key goes up and the entry goes away.
        connection.keys.clear();
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        assertEquals(KeyboardPacket.KEY_DOWN, lastKey(connection)[1]);
        InputManager.InputDeviceListener listener = ReflectionHelpers.getField(game, "pcKeyboardDeviceListener");
        listener.onInputDeviceRemoved(PAD_ID);
        assertEquals(0, heldPadKeys(game).size());
        assertEquals(KeyboardPacket.KEY_UP, lastKey(connection)[1]);
        assertEquals(0x8000 | 0x20, lastKey(connection)[0]);
    }

    @Test
    public void theUsersButtonMapReplacesFaceButtonFlippingOnTheKeyboard() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ControllerHandler handler = ReflectionHelpers.getField(game, "controllerHandler");
        Map<Integer, Object> contexts = ReflectionHelpers.getField(handler, "inputDeviceContexts");
        Object context = ReflectionHelpers.getField(handler, "defaultContext");
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(KeyEvent.KEYCODE_BUTTON_X, KeyEvent.KEYCODE_BUTTON_Y);
        ReflectionHelpers.setField(context, "buttonMap", map);
        contexts.put(PAD_ID, context);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        connection.keys.clear();
        // X was remapped to Y, which is Space.
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_X));
        assertEquals(0x8000 | 0x20, lastKey(connection)[0]);
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_X));
        // A disabled button does nothing but is still the keyboard's.
        map.put(KeyEvent.KEYCODE_BUTTON_Y, ControllerButtonMap.DISABLED);
        connection.keys.clear();
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_Y));
        assertTrue(connection.keys.isEmpty());
    }

    @Test
    public void menuIsStartAndEnterOnTheKeyboard() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        connection.keys.clear();
        assertTrue(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_MENU, gamepad(), PAD_ID)));
        assertTrue(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_MENU, gamepad(), PAD_ID)));
        assertEquals(2, connection.keys.size());
        assertEquals(0x8000 | 0x0D, connection.keys.get(0)[0]);
    }

    @Test
    public void switchingLayoutResyncsTheUsbPadSide() {
        Game game = pcKeyboardGame(new RecordingConnection());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        ControllerHandler handler = ReflectionHelpers.getField(game, "controllerHandler");
        java.util.concurrent.atomic.AtomicInteger generation = ReflectionHelpers.getField(handler, "captureGeneration");
        int before = generation.get();
        PcKeyboardController keyboard = pcKeyboardController(game);
        keyboard.setLayout(PcKeyboardLayout.byId(PcKeyboardLayout.DE));
        assertTrue(generation.get() > before);
    }

    @Test
    public void losingFocusLetsGoOfAHeldKey() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.setField(game, "inputCaptureProvider", new Capture());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        PcKeyboardController keyboard = pcKeyboardController(game);
        keyboard.touchDown(keyboard.layout().key("A"), 1, 1000);
        assertEquals(KeyboardPacket.KEY_DOWN, lastKey(connection)[1]);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "setInputGrabState",
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
        assertEquals(KeyboardPacket.KEY_UP, lastKey(connection)[1]);
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        assertTrue(pressed.isEmpty());
    }

    @Test
    public void losingFocusLetsGoOfALockedModifier() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.setField(game, "inputCaptureProvider", new Capture());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        PcKeyboardController keyboard = pcKeyboardController(game);
        keyboard.engine().tapModifier(PcModifier.CTRL);
        keyboard.engine().tapModifier(PcModifier.CTRL);
        keyboard.touchDown(keyboard.layout().key("A"), 1, 1000);
        keyboard.touchUp(1);
        connection.keys.clear();
        // Ctrl is still down on the host.
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        assertFalse(pressed.isEmpty());
        ReflectionHelpers.callInstanceMethod(game.touchInput, "setInputGrabState",
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
        assertTrue(pressed.isEmpty());
        assertFalse(connection.keys.isEmpty());
        for (int[] key : connection.keys) assertEquals(KeyboardPacket.KEY_UP, key[1]);
    }

    @Test
    public void dpadMovesFocusAndAPressesTheFocusedKey() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        PcKeyboardController keyboard = pcKeyboardController(game);
        PcKey start = keyboard.focus().focused();
        connection.keys.clear();
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_RIGHT));
        assertNotSame(start, keyboard.focus().focused());
        assertTrue("Moving focus presses nothing", connection.keys.isEmpty());
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_CENTER));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_CENTER));
        assertEquals(2, connection.keys.size());
        assertEquals(0x8000 | keyboard.focus().focused().vk, connection.keys.get(0)[0]);
    }
}
