package com.limelight;

import android.app.Application;
import android.content.Context;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.inputmethod.InputMethodManager;
import android.widget.FrameLayout;

import com.limelight.GameKeyboardOverlayTest.RecordingConnection;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.LegacyKeyboardInsets;
import com.limelight.ui.PcKeyboardOverlay;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowInputDevice;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Both keyboards with nothing but a TV remote (D-pad, OK and Back; no B, no touch). The keys go in where Android
 * delivers them, the stream view's pre-IME hook, which takes them before the system keyboard can.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameRemoteKeyboardTest {
    private static final int REMOTE_ID = 21;

    /** Counts the menus Back opens instead of building one. */
    public static class MenuGame extends Game {
        int menus;
        @Override public void showStreamMenu() { menus++; }
        @Override public void showStreamMenuFromController() { menus++; }
    }

    private static MenuGame game(RecordingConnection connection) {
        MenuGame game = Robolectric.buildActivity(MenuGame.class).get();
        game.setTheme(R.style.AppTheme);
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.setField(game, "keyboardTranslator", new KeyboardTranslator());
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.pcKeyboardLayout = "us";
        prefs.pcKeyboardSize = 50;
        prefs.pcKeyboardOpacity = 90;
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "grabbedInput", true);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "controllerHandler", new ControllerHandler(game, connection, game, prefs));
        StreamView stream = new StreamView(game);
        stream.setFocusableInTouchMode(true);
        stream.setInputCallbacks(game);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        game.setContentView(parent);
        ReflectionHelpers.setField(game, "streamView", stream);
        return game;
    }

    private static InputDevice remote() {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed("Remote");
        ReflectionHelpers.setField(device, "mId", REMOTE_ID);
        ReflectionHelpers.setField(device, "mSources", InputDevice.SOURCE_DPAD | InputDevice.SOURCE_KEYBOARD);
        ReflectionHelpers.setField(device, "mKeyboardType", InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC);
        ReflectionHelpers.setField(device, "mMotionRanges", new ArrayList<>());
        return device;
    }

    /** One press and release of a remote key, as the stream view gets it; whether the stream took both. */
    private static boolean press(Game game, int keyCode) {
        InputDevice device = remote();
        StreamView stream = ReflectionHelpers.getField(game, "streamView");
        boolean taken = true;
        for (int action : new int[] {KeyEvent.ACTION_DOWN, KeyEvent.ACTION_UP}) {
            KeyEvent event = new KeyEvent(0, 0, action, keyCode, 0, 0, REMOTE_ID, 0, 0, InputDevice.SOURCE_DPAD);
            shadowOf(event).setDevice(device);
            taken &= stream.onKeyPreIme(keyCode, event);
        }
        return taken;
    }

    private static void openPcKeyboard(Game game) {
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(GameKeyboardOverlayTest.pcKeyboardVisible(game));
    }

    @Test
    public void aRemotesBackClosesThePcKeyboardAndOpensNoMenu() {
        RecordingConnection connection = new RecordingConnection();
        MenuGame game = game(connection);
        openPcKeyboard(game);
        connection.keys.clear();
        assertTrue(press(game, KeyEvent.KEYCODE_BACK));
        assertFalse("Back closes the keyboard", GameKeyboardOverlayTest.pcKeyboardVisible(game));
        assertEquals(0, game.menus);
        assertTrue("Back never types", connection.keys.isEmpty());
        // The next Back is the stream's again: it opens the menu.
        assertTrue(press(game, KeyEvent.KEYCODE_BACK));
        assertEquals(1, game.menus);
    }

    @Test
    public void theDpadReachesTheCloseButtonAndOkClosesThePcKeyboard() {
        RecordingConnection connection = new RecordingConnection();
        MenuGame game = game(connection);
        openPcKeyboard(game);
        PcKeyboardController keyboard = GameKeyboardOverlayTest.pcKeyboardController(game);
        PcKeyboardOverlay overlay = ReflectionHelpers.getField(game, "pcKeyboard");
        connection.keys.clear();
        int rows = keyboard.layout().rowCount();
        for (int i = 0; i < rows; i++) assertTrue(press(game, KeyEvent.KEYCODE_DPAD_UP));
        assertTrue("Up past the top row selects the close button", keyboard.isCloseSelected());
        assertTrue("and the button shows it", (boolean) ReflectionHelpers.callInstanceMethod(overlay, "closeShownSelected"));
        assertTrue(press(game, KeyEvent.KEYCODE_DPAD_DOWN));
        assertFalse(keyboard.isCloseSelected());
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(overlay, "closeShownSelected"));
        assertTrue(press(game, KeyEvent.KEYCODE_DPAD_UP));
        assertTrue(press(game, KeyEvent.KEYCODE_DPAD_CENTER));
        assertFalse("OK on the close button closes the keyboard", GameKeyboardOverlayTest.pcKeyboardVisible(game));
        assertTrue("Nothing reached the PC", connection.keys.isEmpty());
        assertEquals(0, game.menus);
        // Opened again, it starts on a key.
        openPcKeyboard(game);
        assertFalse(keyboard.isCloseSelected());
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(overlay, "closeShownSelected"));
    }

    /**
     * Issue 142: a remote's Back opened the menu over the system keyboard, which came back each time the menu
     * closed, so a remote had no way to close it. Back now closes the keyboard and opens no menu.
     */
    @Test
    public void aRemotesBackClosesTheSystemKeyboardAndOpensNoMenu() {
        RecordingConnection connection = new RecordingConnection();
        MenuGame game = game(connection);
        InputMethodManager input = systemKeyboardOpen(game);
        assertTrue("The stream takes Back before the keyboard", press(game, KeyEvent.KEYCODE_BACK));
        assertFalse("Back closes the system keyboard", shadowOf(input).isSoftInputVisible());
        assertEquals("and opens no menu", 0, game.menus);
        assertTrue(connection.keys.isEmpty());
    }

    /** With Physical Back opens stream menu, a handheld's own Back closes the system keyboard first, as a remote's does. */
    @Test
    public void aHandheldsBackSetToOpenTheMenuClosesTheSystemKeyboardFirst() {
        RecordingConnection connection = new RecordingConnection();
        MenuGame game = game(connection);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.backOpensMenu = true;
        InputMethodManager input = systemKeyboardOpen(game);
        InputDevice pad = GameKeyboardOverlayTest.gamepad();
        assertTrue(game.handleKeyDown(GameKeyboardOverlayTest.padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, pad,
                GameKeyboardOverlayTest.PAD_ID)));
        assertTrue(game.handleKeyUp(GameKeyboardOverlayTest.padEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, pad,
                GameKeyboardOverlayTest.PAD_ID)));
        assertFalse(shadowOf(input).isSoftInputVisible());
        assertEquals(0, game.menus);
    }

    /** The system keyboard open as Android 10 reports it: through the legacy probe only. */
    private static InputMethodManager systemKeyboardOpen(Game game) {
        StreamView stream = ReflectionHelpers.getField(game, "streamView");
        LegacyKeyboardInsets legacy = new LegacyKeyboardInsets(game, (android.view.View) stream.getParent(), (visible, bottom) -> { });
        ReflectionHelpers.setField(legacy, "visible", true);
        ReflectionHelpers.setField(game, "legacyKeyboard", legacy);
        InputMethodManager input = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
        stream.requestFocus();
        input.showSoftInput(stream, 0);
        assertTrue(shadowOf(input).isSoftInputVisible());
        return input;
    }
}
