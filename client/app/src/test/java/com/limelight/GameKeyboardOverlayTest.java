package com.limelight;

import android.app.Application;
import android.content.ComponentName;
import android.content.Context;
import android.graphics.Insets;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.FrameLayout;

import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.KeyboardPacket;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.PcKeyboardOverlay;
import com.limelight.ui.StreamMenu;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowInputDevice;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Tests soft-keyboard IME visibility, extra-keys desktop toolbar interaction, keyboard shortcuts,
 * local chord handling, and shared keyboard test fixtures for {@link Game}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameKeyboardOverlayTest {
    @Test
    @Config(sdk = {29, 36}, qualifiers = "sw800dp-land")
    public void tabletStreamDoesNotAutomaticallyOpenSoftwareKeyboard() throws Exception {
        Game game = game(new RecordingConnection());
        int softInputMode = game.getPackageManager().getActivityInfo(
                new ComponentName(game, Game.class), 0).softInputMode;
        // StreamView must retain text-editor support for the explicit keyboard action, but
        // its initial focus must not trigger Android's large-screen automatic IME policy.
        assertEquals(WindowManager.LayoutParams.SOFT_INPUT_STATE_HIDDEN,
                softInputMode & WindowManager.LayoutParams.SOFT_INPUT_MASK_STATE);
        assertTrue(new StreamView(game).onCheckIsTextEditor());
    }

    @Test
    public void phoneKeyboardMenuActionHidesDesktopPanelAndStillSendsText() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        game.setTheme(R.style.AppTheme);
        StreamView stream = new StreamView(game);
        stream.setFocusableInTouchMode(true);
        stream.setInputCallbacks(game);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        game.setContentView(parent);
        ReflectionHelpers.setField(game, "streamView", stream);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "toggleKeyboardOverlay");
        View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
        assertEquals(View.VISIBLE, panel.getVisibility());
        InputMethodManager input = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
        assertFalse(shadowOf(input).isSoftInputVisible());

        ReflectionHelpers.callInstanceMethod(game.menuActions, "onStreamMenuItem",
                ReflectionHelpers.ClassParameter.from(StreamMenu.Item.class, StreamMenu.Item.KEYBOARD));
        Runnable action = ReflectionHelpers.getField(game, "streamMenuAction");
        assertNotNull(action);
        // The menu hands the selected action back after its window closes.
        action.run();

        assertEquals(View.GONE, panel.getVisibility());
        assertTrue(shadowOf(input).isSoftInputVisible());
        assertTrue(stream.hasFocus());
        InputConnection editor = stream.onCreateInputConnection(new EditorInfo());
        assertNotNull(editor);
        assertTrue(editor.commitText("Hello, world!", 1));
        assertEquals(java.util.Collections.singletonList("Hello, world!"), connection.text);
    }

    @Test
    public void closingPhoneKeyboardDoesNotHideDesktopPanel() {
        Game game = game(new RecordingConnection());
        game.setTheme(R.style.AppTheme);
        StreamView stream = new StreamView(game) {
            @Override public WindowInsets getRootWindowInsets() {
                // API 29 derives IME visibility from the bottom inset beyond stable bars.
                return new WindowInsets.Builder().setSystemWindowInsets(Insets.of(0, 0, 0, 300)).build();
            }
        };
        stream.setFocusableInTouchMode(true);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        game.setContentView(parent);
        ReflectionHelpers.setField(game, "streamView", stream);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "toggleKeyboardOverlay");
        View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
        InputMethodManager input = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
        input.showSoftInput(stream, 0);
        assertTrue(shadowOf(input).isSoftInputVisible());

        game.toggleKeyboard();

        assertFalse(shadowOf(input).isSoftInputVisible());
        assertEquals(View.VISIBLE, panel.getVisibility());
    }

    /**
     * A shortcut's keys stay down for a moment before they go up in reverse (#63): pressed and released in the same
     * instant, the Steam overlay's Shift + Tab and games reading the keys once a frame never saw the chord held.
     */
    @Test
    public void taskManagerShortcutReleasesKeysInReverseOrder() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        sendShortcut(game, KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_ESCAPE);
        KeyboardTranslator translator = new KeyboardTranslator();
        int ctrl = translator.translate(KeyEvent.KEYCODE_CTRL_LEFT, -1) & 0xffff;
        int shift = translator.translate(KeyEvent.KEYCODE_SHIFT_LEFT, -1) & 0xffff;
        int escape = translator.translate(KeyEvent.KEYCODE_ESCAPE, -1) & 0xffff;
        int modifiers = KeyboardPacket.MODIFIER_CTRL | KeyboardPacket.MODIFIER_SHIFT;
        assertEquals("the chord is pressed at once", 3, connection.keys.size());
        holdShortcut(GameKeyboardInput.SHORTCUT_HOLD_MS - 1);
        assertEquals("and still held just before the hold ends", 3, connection.keys.size());
        holdShortcut(1);
        assertEquals(6, connection.keys.size());
        assertArrayEquals(new int[] {ctrl, KeyboardPacket.KEY_DOWN, KeyboardPacket.MODIFIER_CTRL}, connection.keys.get(0));
        assertArrayEquals(new int[] {shift, KeyboardPacket.KEY_DOWN, modifiers}, connection.keys.get(1));
        assertArrayEquals(new int[] {escape, KeyboardPacket.KEY_DOWN, modifiers}, connection.keys.get(2));
        assertArrayEquals(new int[] {escape, KeyboardPacket.KEY_UP, modifiers}, connection.keys.get(3));
        assertArrayEquals(new int[] {shift, KeyboardPacket.KEY_UP, KeyboardPacket.MODIFIER_CTRL}, connection.keys.get(4));
        assertArrayEquals(new int[] {ctrl, KeyboardPacket.KEY_UP, 0}, connection.keys.get(5));
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        assertTrue(pressed.isEmpty());
    }

    /** A shortcut saved with Esc before Ctrl and Shift (tap order, or typed so in Settings) still sends Ctrl + Shift + Esc. */
    @Test
    public void aShortcutSavedEscFirstPressesItsModifiersFirst() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        sendShortcut(game, KeyEvent.KEYCODE_ESCAPE, KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT);
        holdShortcut(GameKeyboardInput.SHORTCUT_HOLD_MS);
        KeyboardTranslator translator = new KeyboardTranslator();
        int ctrl = translator.translate(KeyEvent.KEYCODE_CTRL_LEFT, -1) & 0xffff;
        int shift = translator.translate(KeyEvent.KEYCODE_SHIFT_LEFT, -1) & 0xffff;
        int escape = translator.translate(KeyEvent.KEYCODE_ESCAPE, -1) & 0xffff;
        int modifiers = KeyboardPacket.MODIFIER_CTRL | KeyboardPacket.MODIFIER_SHIFT;
        assertEquals(6, connection.keys.size());
        assertArrayEquals(new int[] {ctrl, KeyboardPacket.KEY_DOWN, KeyboardPacket.MODIFIER_CTRL}, connection.keys.get(0));
        assertArrayEquals(new int[] {shift, KeyboardPacket.KEY_DOWN, modifiers}, connection.keys.get(1));
        assertArrayEquals("Esc goes down with both modifiers held", new int[] {escape, KeyboardPacket.KEY_DOWN, modifiers}, connection.keys.get(2));
        assertArrayEquals(new int[] {ctrl, KeyboardPacket.KEY_UP, 0}, connection.keys.get(5));
    }

    /** A held shortcut goes up once: with the rest when input is let go, or before the next shortcut is pressed. */
    @Test
    public void aHeldShortcutGoesUpOnceWhenInputIsLetGoOrTheNextOneIsPressed() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        ReflectionHelpers.setField(game, "inputCaptureProvider", new Capture());
        KeyboardTranslator translator = new KeyboardTranslator();
        int alt = translator.translate(KeyEvent.KEYCODE_ALT_LEFT, -1) & 0xffff;
        int tab = translator.translate(KeyEvent.KEYCODE_TAB, -1) & 0xffff;
        int escape = translator.translate(KeyEvent.KEYCODE_ESCAPE, -1) & 0xffff;
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");

        sendShortcut(game, KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_TAB);
        game.touchInput.setInputGrabState(false);
        assertEquals("both keys went up with the rest", 4, connection.keys.size());
        holdShortcut(GameKeyboardInput.SHORTCUT_HOLD_MS);
        assertEquals("and the hold's end sends nothing more", 4, connection.keys.size());
        assertTrue(pressed.isEmpty());

        connection.keys.clear();
        game.touchInput.setInputGrabState(true);
        sendShortcut(game, KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_TAB);
        sendShortcut(game, KeyEvent.KEYCODE_ESCAPE);
        assertEquals(5, connection.keys.size());
        assertArrayEquals(new int[] {tab, KeyboardPacket.KEY_UP, KeyboardPacket.MODIFIER_ALT}, connection.keys.get(2));
        assertArrayEquals(new int[] {alt, KeyboardPacket.KEY_UP, 0}, connection.keys.get(3));
        assertArrayEquals("the next shortcut is pressed in full", new int[] {escape, KeyboardPacket.KEY_DOWN, 0}, connection.keys.get(4));
        holdShortcut(GameKeyboardInput.SHORTCUT_HOLD_MS);
        assertEquals(6, connection.keys.size());
        assertArrayEquals(new int[] {escape, KeyboardPacket.KEY_UP, 0}, connection.keys.get(5));
        assertTrue(pressed.isEmpty());
    }

    /**
     * A shortcut tile in the stream menu runs once the menu is gone. With input released before the menu opened it
     * was dropped without a word: now input is captured for it, and without the keyboard permission a toast says so.
     */
    @Test
    public void aMenuShortcutCapturesInputForItselfOrSaysWhyItWasNotSent() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        ReflectionHelpers.setField(game, "inputCaptureProvider", new Capture());
        ReflectionHelpers.setField(game, "grabbedInput", false);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "sendMenuShortcut",
                ReflectionHelpers.ClassParameter.from(int[].class, new int[] {KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_TAB}));
        holdShortcut(GameKeyboardInput.SHORTCUT_HOLD_MS);
        assertEquals(4, connection.keys.size());
        assertTrue(ReflectionHelpers.<Boolean>getField(game, "grabbedInput"));
        assertNull(org.robolectric.shadows.ShadowToast.getLatestToast());

        connection.keys.clear();
        connection.inputAllowed = false;
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "sendMenuShortcut",
                ReflectionHelpers.ClassParameter.from(int[].class, new int[] {KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_TAB}));
        assertTrue(connection.keys.isEmpty());
        assertEquals(game.getString(R.string.stream_shortcut_no_keyboard), org.robolectric.shadows.ShadowToast.getTextOfLatestToast());

        // Another dialog took the screen before the shortcut ran.
        connection.inputAllowed = true;
        ReflectionHelpers.setField(game, "streamMenu", new android.app.Dialog(game));
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "sendMenuShortcut",
                ReflectionHelpers.ClassParameter.from(int[].class, new int[] {KeyEvent.KEYCODE_ESCAPE}));
        assertTrue(connection.keys.isEmpty());
        assertEquals(game.getString(R.string.stream_shortcut_not_sent), org.robolectric.shadows.ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void aLocalChordKeepsItsModifierTapsFromBeingLoneTapsOnTheHost() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        int neutral = 0x8000 | com.limelight.binding.input.keyboard.PcKeyboardEngine.NEUTRAL_VK;
        int[] chord = {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT};
        for (int key : chord) game.handleKeyDown(keyboardKey(KeyEvent.ACTION_DOWN, key));
        assertEquals("the modifiers are sent as they go down", 3, connection.keys.size());
        assertTrue(game.handleKeyDown(keyboardKey(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_K)));
        assertEquals("the shortcut key itself never reaches the host", 5, connection.keys.size());
        assertEquals(neutral, connection.keys.get(3)[0]);
        assertEquals(KeyboardPacket.KEY_DOWN, connection.keys.get(3)[1]);
        assertEquals(neutral, connection.keys.get(4)[0]);
        assertEquals(KeyboardPacket.KEY_UP, connection.keys.get(4)[1]);
        assertTrue(game.handleKeyUp(keyboardKey(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_K)));
        for (int key : chord) game.handleKeyUp(keyboardKey(KeyEvent.ACTION_UP, key));
        assertTrue("the PC keyboard opened", pcKeyboardVisible(game));
        assertEquals("the modifier releases follow the neutral key", 8, connection.keys.size());
    }

    /** An old chord runs its action and says once, in the app's words, which chord it moved to. */
    @Test
    public void anOldChordSaysOnceWhereItMoved() {
        Game game = pcKeyboardGame(new RecordingConnection());
        int[] chord = {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_C};
        for (int press = 0; press < 2; press++) {
            org.robolectric.shadows.ShadowToast.reset();
            for (int key : chord) game.handleKeyDown(keyboardKey(KeyEvent.ACTION_DOWN, key));
            for (int key : chord) game.handleKeyUp(keyboardKey(KeyEvent.ACTION_UP, key));
            if (press == 0) {
                assertEquals("Ctrl + Alt + Shift + C is now Ctrl + Alt + Shift + P",
                        org.robolectric.shadows.ShadowToast.getTextOfLatestToast());
            } else {
                assertNull("said once", org.robolectric.shadows.ShadowToast.getLatestToast());
            }
        }
    }

    @Test
    public void aLocalChordSendsNothingExtraWhileInputIsNotGrabbed() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.setField(game, "grabbedInput", false);
        for (int key : new int[] {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_K}) {
            game.handleKeyDown(keyboardKey(KeyEvent.ACTION_DOWN, key));
        }
        assertEquals(0, connection.keys.size());
    }

    @Test
    public void shortcutsPreservePhysicalModifiersAndRespectInputGrab() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        short ctrl = new KeyboardTranslator().translate(KeyEvent.KEYCODE_CTRL_LEFT, -1);
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        pressed.put(ctrl, (byte) 0);
        ReflectionHelpers.setField(game, "modifierFlags", (int) KeyboardPacket.MODIFIER_CTRL);
        sendShortcut(game, KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_ESCAPE);
        holdShortcut(GameKeyboardInput.SHORTCUT_HOLD_MS);
        assertEquals(4, connection.keys.size());
        assertEquals(1, pressed.size());
        assertTrue(pressed.containsKey(ctrl));
        assertEquals(KeyboardPacket.MODIFIER_CTRL, connection.keys.get(3)[2]);
        ReflectionHelpers.setField(game, "grabbedInput", false);
        sendShortcut(game, KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_F4);
        assertEquals(4, connection.keys.size());
        ReflectionHelpers.setField(game, "grabbedInput", true);
        ReflectionHelpers.setField(game, "foreground", false);
        sendShortcut(game, KeyEvent.KEYCODE_ESCAPE);
        assertEquals(4, connection.keys.size());
        ReflectionHelpers.setField(game, "foreground", true);
        connection.inputAllowed = false;
        sendShortcut(game, KeyEvent.KEYCODE_ESCAPE);
        assertEquals(4, connection.keys.size());
    }

    @Test
    public void toolbarReleasesPointerCaptureAndRestoresCursorPreference() {
        for (boolean visible : new boolean[] {false, true}) {
            Game game = game(new RecordingConnection());
            game.setTheme(R.style.AppTheme);
            StreamView stream = new StreamView(game);
            FrameLayout parent = new FrameLayout(game);
            parent.addView(stream);
            ReflectionHelpers.setField(game, "streamView", stream);
            Capture capture = new Capture();
            ReflectionHelpers.setField(game, "inputCaptureProvider", capture);
            ReflectionHelpers.setField(game, "cursorVisible", visible);
            ReflectionHelpers.callInstanceMethod(game.keyboardInput, "toggleKeyboardOverlay");
            assertTrue(capture.visible());
            assertTrue((boolean) ReflectionHelpers.getField(game, "grabbedInput"));
            ReflectionHelpers.callInstanceMethod(game.touchInput, "setInputGrabState",
                    ReflectionHelpers.ClassParameter.from(boolean.class, true));
            assertTrue(capture.visible());
            ReflectionHelpers.callInstanceMethod(game.keyboardInput, "hideKeyboardOverlay");
            assertEquals(visible, capture.visible());
            assertTrue((boolean) ReflectionHelpers.getField(game, "grabbedInput"));
        }
    }

    static Game pcKeyboardGame(RecordingConnection connection) {
        Game game = game(connection);
        game.setTheme(R.style.AppTheme);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.pcKeyboardLayout = "us";
        prefs.pcKeyboardSize = 50;
        prefs.pcKeyboardOpacity = 90;
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "controllerHandler", new ControllerHandler(game, connection, game, prefs));
        StreamView stream = new StreamView(game);
        stream.setFocusableInTouchMode(true);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        game.setContentView(parent);
        ReflectionHelpers.setField(game, "streamView", stream);
        return game;
    }

    static boolean pcKeyboardVisible(Game game) {
        return (boolean) ReflectionHelpers.callInstanceMethod(game.keyboardInput, "isPcKeyboardVisible");
    }

    static PcKeyboardController pcKeyboardController(Game game) {
        PcKeyboardOverlay overlay = ReflectionHelpers.getField(game, "pcKeyboard");
        return overlay.controller();
    }

    static final int PAD_ID = 12;

    static InputDevice gamepad() {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed("Pad");
        ReflectionHelpers.setField(device, "mId", PAD_ID);
        ReflectionHelpers.setField(device, "mSources", InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD);
        ReflectionHelpers.setField(device, "mKeyboardType", InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC);
        ReflectionHelpers.setField(device, "mMotionRanges", new java.util.ArrayList<>());
        return device;
    }

    static KeyEvent padEvent(int action, int keyCode, InputDevice device, int deviceId) {
        KeyEvent event = new KeyEvent(0, 0, action, keyCode, 0, 0, deviceId, 0, 0, InputDevice.SOURCE_GAMEPAD);
        shadowOf(event).setDevice(device);
        return event;
    }

    static boolean padKey(Game game, int action, int keyCode) {
        KeyEvent event = padEvent(action, keyCode, gamepad(), PAD_ID);
        return action == KeyEvent.ACTION_DOWN ? game.handleKeyDown(event) : game.handleKeyUp(event);
    }

    static boolean takenByPcKeyboard(Game game, KeyEvent event) {
        return (boolean) ReflectionHelpers.callInstanceMethod(game.keyboardInput, "handlePcKeyboardKey",
                ReflectionHelpers.ClassParameter.from(KeyEvent.class, event));
    }

    static android.util.SparseBooleanArray heldPadKeys(Game game) {
        return ReflectionHelpers.getField(game.keyboardInput, "pcKeyboardKeys");
    }

    static int[] lastKey(RecordingConnection connection) {
        return connection.keys.get(connection.keys.size() - 1);
    }

    static class Capture extends InputCaptureProvider {
        boolean visible() { return isCursorVisible; }
    }

    static Game game(RecordingConnection connection) {
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.setField(game, "keyboardTranslator", new KeyboardTranslator());
        // Grabbing input reads Send system keys
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(game, "grabbedInput", true);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        return game;
    }

    /** A key of a physical keyboard (a device that is not a game controller). */
    static KeyEvent keyboardKey(int action, int keyCode) {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed("Keyboard");
        ReflectionHelpers.setField(device, "mId", 31);
        ReflectionHelpers.setField(device, "mSources", InputDevice.SOURCE_KEYBOARD);
        ReflectionHelpers.setField(device, "mKeyboardType", InputDevice.KEYBOARD_TYPE_ALPHABETIC);
        ReflectionHelpers.setField(device, "mMotionRanges", new ArrayList<>());
        KeyEvent event = new KeyEvent(0, 0, action, keyCode, 0, 0, 31, 0, 0, InputDevice.SOURCE_KEYBOARD);
        shadowOf(event).setDevice(device);
        return event;
    }

    private static void sendShortcut(Game game, int... keys) {
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "sendKeyboardShortcut",
                ReflectionHelpers.ClassParameter.from(int[].class, keys));
    }

    /** Lets a sent shortcut's hold run on for {@code ms}. */
    private static void holdShortcut(long ms) {
        shadowOf(android.os.Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(ms));
    }

    static class RecordingConnection extends NvConnection {
        final List<int[]> keys = new ArrayList<>();
        final List<String> text = new ArrayList<>();
        final List<Integer> controllerButtons = new ArrayList<>();
        boolean inputAllowed = true;

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return inputAllowed; }
        @Override public void sendUtf8Text(String value) { text.add(value); }
        @Override public void sendControllerInput(short controllerNumber, short activeGamepadMask, int buttonFlags,
                byte leftTrigger, byte rightTrigger, short leftStickX, short leftStickY,
                short rightStickX, short rightStickY) {
            controllerButtons.add(buttonFlags);
        }
        @Override public void sendKeyboardInput(short key, byte direction, byte modifiers, byte flags) {
            keys.add(new int[] {key & 0xffff, direction, modifiers});
        }
    }
}
