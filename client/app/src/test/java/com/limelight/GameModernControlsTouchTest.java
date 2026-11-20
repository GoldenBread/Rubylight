package com.limelight;

import android.app.Application;
import android.content.ComponentName;
import android.content.Context;
import android.content.DialogInterface;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.graphics.Rect;
import android.os.Looper;
import android.os.SystemClock;
import android.preference.PreferenceManager;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;

import androidx.appcompat.app.AlertDialog;

import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.binding.input.TouchLayout;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * The modern on-screen controls on the real stream screen, built by its own onCreate, at the size of a Galaxy Z
 * Fold7's inner screen: a finger on any control the player sees reaches that control, whatever else the stream shows.
 * Touches go through the activity, as Android delivers them, at the middle of each visible control.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 36, application = Application.class, qualifiers = "w832dp-h750dp-land-420dpi",
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameModernControlsTouchTest {
    private static final String PC = "pc-uuid";
    private static final int APP = 11;
    private static final String PORTRAIT = "w750dp-h832dp-port-420dpi", LANDSCAPE = "w832dp-h750dp-land-420dpi";
    private ActivityController<StreamGame> controller;

    /** The stream screen without the Shield controller service, which has no binder here. */
    public static class StreamGame extends Game {
        @Override public boolean bindService(Intent service, ServiceConnection connection, int flags) {
            return false;
        }
    }

    /** A Desktop stream with the modern controls on, through the screen's onCreate up to its connection. */
    private Game stream(boolean steamControls, boolean touchLayout) {
        Context context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit()
                .putBoolean("checkbox_show_onscreen_controls", true)
                .putString("list_osc_style", "modern")
                .putBoolean("checkbox_osc_steam_controls", steamControls)
                .commit();
        if (touchLayout) TouchLayout.setEnabled(context, TouchLayout.profileKey(PC, null, APP), true);
        // Handles its own configuration changes, as Game does, so a rotation reaches onConfigurationChanged.
        try {
            ActivityInfo info = new ActivityInfo(context.getPackageManager()
                    .getActivityInfo(new ComponentName(context, Game.class), 0));
            info.name = StreamGame.class.getName();
            shadowOf(context.getPackageManager()).addOrUpdateActivity(info);
        } catch (PackageManager.NameNotFoundException e) {
            throw new AssertionError(e);
        }
        Intent intent = new Intent(context, StreamGame.class)
                .putExtra(Game.EXTRA_HOST, "192.0.2.1")
                .putExtra(Game.EXTRA_UNIQUEID, "device")
                .putExtra(Game.EXTRA_APP_ID, APP)
                .putExtra(Game.EXTRA_APP_NAME, "Desktop")
                .putExtra(Game.EXTRA_PC_UUID, PC);
        controller = Robolectric.buildActivity(StreamGame.class, intent).setup();
        idle();
        Game game = controller.get();
        assertNotNull("the controls were built", game.modernController);
        assertEquals("the touch layout as set", touchLayout ? View.VISIBLE : View.GONE,
                game.gameTouchControls.getVisibility());
        return game;
    }

    private static void idle() {
        shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
    }

    private void reconfigure(Game game, String qualifiers) {
        RuntimeEnvironment.setQualifiers(qualifiers);
        Configuration changed = RuntimeEnvironment.getApplication().getResources().getConfiguration();
        controller.configurationChange(new Configuration(changed));
        game.getWindow().getDecorView().requestLayout();
        idle();
    }

    private static ViewGroup frame(Game game) {
        return (ViewGroup) game.streamView.getParent();
    }

    private static Rect boundsIn(ViewGroup frame, View child) {
        Rect rect = new Rect(0, 0, child.getWidth(), child.getHeight());
        frame.offsetDescendantRectToMyCoords(child, rect);
        return rect;
    }

    /** The visible controls by their place in the frame, each with a recorder of the touches that reach it. */
    private static Map<Rect, List<MotionEvent>> recordControls(Game game) {
        ViewGroup frame = frame(game);
        List<Rect> visible = new ArrayList<>();
        game.modernController.collectVisibleBounds(visible);
        Map<Rect, List<MotionEvent>> reached = new HashMap<>();
        for (int i = 0; i < frame.getChildCount(); i++) {
            View child = frame.getChildAt(i);
            Rect rect = boundsIn(frame, child);
            if (child.getVisibility() != View.VISIBLE || !visible.contains(rect) || reached.containsKey(rect)) continue;
            List<MotionEvent> events = new ArrayList<>();
            reached.put(rect, events);
            // Looks only: the control still handles the touch itself.
            child.setOnTouchListener((view, event) -> {
                events.add(MotionEvent.obtain(event));
                return false;
            });
        }
        assertEquals("each visible control found in the frame", visible.size(), reached.size());
        return reached;
    }

    private static MotionEvent touch(long down, long time, int action, float[]... fingers) {
        MotionEvent.PointerProperties[] properties = new MotionEvent.PointerProperties[fingers.length];
        MotionEvent.PointerCoords[] coords = new MotionEvent.PointerCoords[fingers.length];
        for (int i = 0; i < fingers.length; i++) {
            properties[i] = new MotionEvent.PointerProperties();
            properties[i].id = i;
            properties[i].toolType = MotionEvent.TOOL_TYPE_FINGER;
            coords[i] = new MotionEvent.PointerCoords();
            coords[i].x = fingers[i][0];
            coords[i].y = fingers[i][1];
            coords[i].pressure = 1;
            coords[i].size = .1f;
        }
        return MotionEvent.obtain(down, time, action, fingers.length, properties, coords, 0, 0, 1, 1, 0, 0,
                InputDevice.SOURCE_TOUCHSCREEN, 0);
    }

    private static void dispatch(Game game, MotionEvent event) {
        try {
            game.dispatchTouchEvent(event);
        } finally {
            event.recycle();
        }
    }

    /** The middle of a control, in the window's coordinates, where Android delivers the touch. */
    private static float[] middle(Game game, Rect control) {
        int[] origin = new int[2];
        frame(game).getLocationInWindow(origin);
        return new float[] {origin[0] + control.exactCenterX(), origin[1] + control.exactCenterY()};
    }

    /** What took the touch instead, for the failure message. */
    private static String taker(ViewGroup frame) {
        try {
            Object target = ReflectionHelpers.getField(frame, "mFirstTouchTarget");
            View child = target == null ? null : ReflectionHelpers.getField(target, "child");
            return child == null ? "nothing" : child.getClass().getSimpleName();
        } catch (RuntimeException unknown) {
            return "unknown";
        }
    }

    private static String describe(ViewGroup frame) {
        StringBuilder text = new StringBuilder();
        for (int i = 0; i < frame.getChildCount(); i++) {
            View child = frame.getChildAt(i);
            text.append('\n').append(i).append(' ').append(child.getClass().getSimpleName())
                    .append(child.getVisibility() == View.VISIBLE ? " visible " : " hidden ")
                    .append(boundsIn(frame, child));
        }
        return text.toString();
    }

    /** A tap at the middle of every visible control: each must reach the control under the finger. */
    private static void assertEveryControlAnswers(Game game, String when) {
        Map<Rect, List<MotionEvent>> controls = recordControls(game);
        assertFalse(when + ": controls on the screen", controls.isEmpty());
        ViewGroup frame = frame(game);
        List<String> missed = new ArrayList<>();
        int buttons = 0;
        for (Map.Entry<Rect, List<MotionEvent>> control : controls.entrySet()) {
            float[] finger = middle(game, control.getKey());
            long now = SystemClock.uptimeMillis();
            dispatch(game, touch(now, now, MotionEvent.ACTION_DOWN, finger));
            String taker = taker(frame);
            buttons |= oscButtons(game);
            dispatch(game, touch(now, now + 100, MotionEvent.ACTION_UP, finger));
            List<MotionEvent> events = control.getValue();
            if (events.size() != 2 || events.get(0).getActionMasked() != MotionEvent.ACTION_DOWN) {
                missed.add(control.getKey() + " (taken by " + taker + ")");
            }
            for (MotionEvent event : events) event.recycle();
            events.clear();
        }
        // A grip stays latched after its tap.
        game.modernController.releaseInput();
        assertTrue(when + ": touches that missed their control " + missed + describe(frame), missed.isEmpty());
        assertNotEquals(when + ": the presses reached the PC's gamepad", 0, buttons);
    }

    /** The on-screen gamepad's buttons as the controller handler last sent them. */
    private static int oscButtons(Game game) {
        Object pad = ReflectionHelpers.getField(game.controllerHandler, "defaultContext");
        return ReflectionHelpers.getField(pad, "inputMap");
    }

    @Test public void withTheSteamControllerControlsEveryControlAnswers() {
        assertEveryControlAnswers(stream(true, false), "Steam controls");
    }

    @Test public void withoutTheSteamControllerControlsEveryControlAnswers() {
        assertEveryControlAnswers(stream(false, false), "no Steam controls");
    }

    /**
     * The game's touch layout covers the whole screen, so fingers beside its own buttons reach the stream. With Hide
     * while a controller is connected on, as it is by default, the modern controls were built under it, and not one
     * touch reached them: no pressed state, no vibration, nothing on the PC.
     */
    @Test public void aShownTouchLayoutLeavesTheModernControlsTheirTouches() {
        assertEveryControlAnswers(stream(true, true), "Steam controls with the touch layout");
    }

    @Test public void aShownTouchLayoutLeavesTheModernControlsTheirTouchesWithoutSteamControls() {
        assertEveryControlAnswers(stream(false, true), "touch layout");
    }

    @Test public void aTouchLayoutShownDuringTheStreamLeavesTheModernControlsTheirTouches() {
        Game game = stream(true, false);
        // As its menu's Show does.
        ReflectionHelpers.setField(game.gameTouchControls, "enabled", true);
        game.gameTouchControls.setVisibility(View.VISIBLE);
        idle();
        assertEveryControlAnswers(game, "touch layout shown during the stream");
    }

    @Test @Config(qualifiers = PORTRAIT)
    public void inPortraitEveryControlAnswers() {
        assertEveryControlAnswers(stream(true, false), "portrait");
    }

    @Test @Config(qualifiers = PORTRAIT)
    public void inPortraitAShownTouchLayoutLeavesTheModernControlsTheirTouches() {
        assertEveryControlAnswers(stream(true, true), "portrait with the touch layout");
    }

    @Test public void withThePerformanceOverlayEveryControlAnswers() {
        Game game = stream(true, false);
        game.prefConfig.enablePerfOverlay = true;
        game.performanceOverlayScroll.setVisibility(View.VISIBLE);
        game.overlays.updatePerformanceOverlay();
        idle();
        assertEveryControlAnswers(game, "performance overlay");
    }

    @Test public void afterTheLayoutEditorEveryControlAnswers() {
        Game game = stream(true, false);
        game.touchInput.showControllerLayoutDialog();
        idle();
        assertTrue(game.modernController.isEditing());
        game.modernController.finishEditing(false);
        idle();
        assertEveryControlAnswers(game, "after the editor's Cancel");
        game.touchInput.showControllerLayoutDialog();
        idle();
        game.modernController.finishEditing(true);
        idle();
        assertEveryControlAnswers(game, "after the editor's Done");
    }

    @Test public void afterTurningAndNarrowingEveryControlAnswers() {
        Game game = stream(true, false);
        reconfigure(game, PORTRAIT);
        assertTrue("turned", frame(game).getHeight() > frame(game).getWidth());
        assertEveryControlAnswers(game, "turned to portrait");
        reconfigure(game, LANDSCAPE);
        assertTrue("turned back", frame(game).getWidth() > frame(game).getHeight());
        assertEveryControlAnswers(game, "turned back");
        // Half the screen, as split screen gives it.
        reconfigure(game, "w416dp-h750dp-port-420dpi");
        assertEveryControlAnswers(game, "split screen");
    }

    @Test public void afterTheStreamMenuEveryControlAnswers() {
        Game game = stream(true, false);
        game.connected = true;
        game.showStreamMenu();
        idle();
        assertNotNull("the menu opened", game.streamMenu);
        game.streamMenu.dismiss();
        idle();
        assertNull(game.streamMenu);
        assertEveryControlAnswers(game, "after the stream menu");
    }

    @Test public void afterAControllerCameAndWentEveryControlAnswers() {
        Game game = stream(true, false);
        game.setPhysicalControllerConnected(true);
        idle();
        List<Rect> shown = new ArrayList<>();
        game.modernController.collectVisibleBounds(shown);
        assertTrue("hidden while the controller is here", shown.isEmpty());
        game.setPhysicalControllerConnected(false);
        idle();
        assertEveryControlAnswers(game, "after a controller came and went");
    }

    @Test public void afterTheWindowLostAndRegainedFocusEveryControlAnswers() {
        Game game = stream(true, false);
        game.onWindowFocusChanged(false);
        idle();
        game.onWindowFocusChanged(true);
        idle();
        assertEveryControlAnswers(game, "after focus came back");
    }

    @Test public void aSecondFingerReachesItsControlWhileTheFirstIsHeld() {
        Game game = stream(true, true);
        Map<Rect, List<MotionEvent>> controls = recordControls(game);
        List<Rect> places = new ArrayList<>(controls.keySet());
        float[] first = middle(game, places.get(0)), second = middle(game, places.get(1));
        long now = SystemClock.uptimeMillis();
        dispatch(game, touch(now, now, MotionEvent.ACTION_DOWN, first));
        int secondDown = MotionEvent.ACTION_POINTER_DOWN | (1 << MotionEvent.ACTION_POINTER_INDEX_SHIFT);
        dispatch(game, touch(now, now + 20, secondDown, first, second));
        assertFalse("the first finger's control", controls.get(places.get(0)).isEmpty());
        assertFalse("the second finger's control, beside the touch layout", controls.get(places.get(1)).isEmpty());
        assertEquals(MotionEvent.ACTION_DOWN, controls.get(places.get(1)).get(0).getActionMasked());
    }

    // Moving the game buttons. The entries of Controls for this game, in GameTouchControls.showMenu's order.
    private static final int SHOW_OR_HIDE = 0, MOVE_OR_SAVE = 2;
    private static final String BUTTONS_PROFILE = TouchLayout.profileKey(PC, null, 12);

    /** Counts the gamepad reports that leave for the PC. */
    private static final class CountingConnection extends NvConnection {
        int gamepadReports;

        CountingConnection() {
            super(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return true; }

        @Override public void sendControllerInput(short controllerNumber, short activeGamepadMask, int buttonFlags,
                                                  byte leftTrigger, byte rightTrigger, short leftStickX,
                                                  short leftStickY, short rightStickX, short rightStickY) {
            gamepadReports++;
        }
    }

    private static CountingConnection countGamepadReports(Game game) {
        CountingConnection counting = new CountingConnection();
        ReflectionHelpers.setField(game.controllerHandler, "conn", counting);
        return counting;
    }

    private static List<Rect> visibleControls(Game game) {
        List<Rect> visible = new ArrayList<>();
        game.modernController.collectVisibleBounds(visible);
        return visible;
    }

    private static Rect largestControl(Game game) {
        Rect largest = null;
        for (Rect rect : visibleControls(game)) {
            if (largest == null || rect.width() * rect.height() > largest.width() * largest.height()) largest = rect;
        }
        assertNotNull(largest);
        return largest;
    }

    /** One game button in the middle of a modern control, shown as switching to a game that has it does; its id. */
    private static String showGameButtonOver(Game game, Rect control) {
        ViewGroup frame = frame(game);
        float size = Math.min(control.width(), control.height()) * .6f;
        String id = UUID.randomUUID().toString();
        TouchLayout.Button button = new TouchLayout.Button(id, "Jump", KeyboardShortcuts.parseChord("SPACE"), 0, false,
                (control.exactCenterX() - size / 2) / frame.getWidth(),
                (control.exactCenterY() - size / 2) / frame.getHeight(),
                size / frame.getWidth(), size / frame.getHeight());
        Context context = RuntimeEnvironment.getApplication();
        new TouchLayout(Collections.singletonList(button)).save(context, BUTTONS_PROFILE);
        TouchLayout.setEnabled(context, BUTTONS_PROFILE, true);
        game.gameTouchControls.setProfile(BUTTONS_PROFILE);
        idle();
        assertTrue("it lies under the control", control.contains(boundsIn(frame, gameButton(game, id))));
        return id;
    }

    /** The game button's view as the overlay has it now: moving the buttons builds their views again. */
    private static View gameButton(Game game, String id) {
        View view = game.gameTouchControls.findViewWithTag(id);
        assertNotNull("the game button", view);
        return view;
    }

    /** Picks an entry of Controls for this game, as a tap on it does. */
    private static void chooseInGameButtonsMenu(Game game, int entry) {
        game.gameTouchControls.showMenu();
        idle();
        AlertDialog menu = (AlertDialog) game.streamMenu;
        assertNotNull("the menu opened", menu);
        menu.getListView().performItemClick(null, entry, entry);
        idle();
        // Moving explains itself in a dialog of its own.
        if (game.streamMenu != null) {
            game.streamMenu.dismiss();
            idle();
        }
        menuClosed(game);
    }

    /**
     * Input goes back to the PC, as a live stream does once a menu closes. Without it, the touch layout would let
     * touches through to whatever lies below it, as it does while input is not captured.
     */
    private static void menuClosed(Game game) {
        game.touchInput.setInputGrabState(true);
        idle();
        assertTrue(game.grabbedInput);
    }

    private static void startMovingTheButtons(Game game) {
        chooseInGameButtonsMenu(game, MOVE_OR_SAVE);
        assertTrue("moving the game buttons", game.gameTouchControls.isEditing());
        assertTrue("the modern controls stepped aside", visibleControls(game).isEmpty());
    }

    private static float[] middleOf(View view) {
        int[] at = new int[2];
        view.getLocationInWindow(at);
        return new float[] {at[0] + view.getWidth() / 2f, at[1] + view.getHeight() / 2f};
    }

    /**
     * Shown game buttons lie under the modern controls, which take touches first. While the buttons are moved the
     * controls leave, so a button under one of them can be grabbed and dragged away.
     */
    @Test public void aGameButtonUnderAModernControlCanBeDraggedWhileTheButtonsAreMoved() {
        Game game = stream(true, false);
        Rect control = largestControl(game);
        String id = showGameButtonOver(game, control);
        Map<Rect, List<MotionEvent>> controls = recordControls(game);
        float[] start = middleOf(gameButton(game, id));
        long now = SystemClock.uptimeMillis();
        dispatch(game, touch(now, now, MotionEvent.ACTION_DOWN, start));
        dispatch(game, touch(now, now + 100, MotionEvent.ACTION_UP, start));
        assertFalse("outside the editor the control above the button takes the touch", controls.get(control).isEmpty());

        startMovingTheButtons(game);
        View button = gameButton(game, id);
        assertArrayEquals("the button is where it was", start, middleOf(button), 1);
        float before = TouchLayout.load(RuntimeEnvironment.getApplication(), BUTTONS_PROFILE).buttons.get(0).x;
        float startX = button.getX();
        // Toward the middle of the screen, so the drag stays on it.
        float dx = start[0] < frame(game).getWidth() / 2f ? 300 : -300;
        float[] end = {start[0] + dx, start[1]};
        now = SystemClock.uptimeMillis();
        dispatch(game, touch(now, now, MotionEvent.ACTION_DOWN, start));
        dispatch(game, touch(now, now + 50, MotionEvent.ACTION_MOVE, new float[] {start[0] + dx / 2, start[1]}));
        dispatch(game, touch(now, now + 100, MotionEvent.ACTION_MOVE, end));
        dispatch(game, touch(now, now + 150, MotionEvent.ACTION_UP, end));
        assertEquals("the button followed the finger", startX + dx, button.getX(), 2);
        float after = TouchLayout.load(RuntimeEnvironment.getApplication(), BUTTONS_PROFILE).buttons.get(0).x;
        assertEquals("and stays where it was dropped", dx, (after - before) * frame(game).getWidth(), 2);
    }

    /** No drag or tap while the buttons are moved reaches the PC as gamepad input, wherever a control was. */
    @Test public void noGamepadReportLeavesWhileTheButtonsAreMoved() {
        Game game = stream(true, false);
        List<Rect> places = visibleControls(game);
        String id = showGameButtonOver(game, largestControl(game));
        CountingConnection pc = countGamepadReports(game);
        startMovingTheButtons(game);
        // Opening the menu told the PC its pads are at rest; from here on only a touch could send anything.
        pc.gamepadReports = 0;
        for (Rect place : places) {
            float[] finger = middle(game, place);
            long now = SystemClock.uptimeMillis();
            dispatch(game, touch(now, now, MotionEvent.ACTION_DOWN, finger));
            dispatch(game, touch(now, now + 50, MotionEvent.ACTION_MOVE, new float[] {finger[0] + 60, finger[1] + 60}));
            dispatch(game, touch(now, now + 100, MotionEvent.ACTION_UP, new float[] {finger[0] + 60, finger[1] + 60}));
        }
        float[] start = middleOf(gameButton(game, id));
        long now = SystemClock.uptimeMillis();
        dispatch(game, touch(now, now, MotionEvent.ACTION_DOWN, start));
        dispatch(game, touch(now, now + 100, MotionEvent.ACTION_MOVE, new float[] {start[0] + 200, start[1]}));
        dispatch(game, touch(now, now + 150, MotionEvent.ACTION_UP, new float[] {start[0] + 200, start[1]}));
        idle();
        assertEquals("no gamepad report while the buttons are moved", 0, pc.gamepadReports);
        assertEquals(0, oscButtons(game));

        // The count is real: once the editor closes, presses go out again.
        chooseInGameButtonsMenu(game, MOVE_OR_SAVE);
        pc.gamepadReports = 0;
        assertEveryControlAnswers(game, "after Save");
        assertTrue("presses after Save reach the PC", pc.gamepadReports > 0);
    }

    @Test public void theModernControlsAnswerAgainAfterTheMovedButtonsAreSaved() {
        Game game = stream(true, false);
        showGameButtonOver(game, largestControl(game));
        startMovingTheButtons(game);
        chooseInGameButtonsMenu(game, MOVE_OR_SAVE);
        assertFalse(game.gameTouchControls.isEditing());
        assertEquals("the game buttons stay shown", View.VISIBLE, game.gameTouchControls.getVisibility());
        assertEveryControlAnswers(game, "after Save");
    }

    /**
     * The editor has no Cancel of its own: the menu's Cancel leaves it open (each drag is kept as it ends), and Hide
     * game buttons is the other way out.
     */
    @Test public void theModernControlsAnswerAgainAfterTheButtonEditorIsLeftByHidingTheButtons() {
        Game game = stream(true, false);
        showGameButtonOver(game, largestControl(game));
        startMovingTheButtons(game);
        game.gameTouchControls.showMenu();
        idle();
        ((AlertDialog) game.streamMenu).getButton(DialogInterface.BUTTON_NEGATIVE).performClick();
        idle();
        menuClosed(game);
        assertTrue("Cancel keeps moving the buttons", game.gameTouchControls.isEditing());
        assertTrue("and the controls away", visibleControls(game).isEmpty());
        chooseInGameButtonsMenu(game, SHOW_OR_HIDE);
        assertFalse(game.gameTouchControls.isEditing());
        assertEquals(View.GONE, game.gameTouchControls.getVisibility());
        assertEveryControlAnswers(game, "after Hide game buttons");
    }

    @Test public void aControllerLeavingWhileTheButtonsAreMovedLeavesTheControlsAway() {
        Game game = stream(true, false);
        showGameButtonOver(game, largestControl(game));
        startMovingTheButtons(game);
        game.setPhysicalControllerConnected(true);
        idle();
        game.setPhysicalControllerConnected(false);
        idle();
        assertTrue("still away while the buttons are moved", visibleControls(game).isEmpty());
        chooseInGameButtonsMenu(game, MOVE_OR_SAVE);
        assertEveryControlAnswers(game, "after Save");
    }

    /** The modern controls' own editor, as Controller layout opens it. */
    private static void openTheControlsEditor(Game game) {
        game.touchInput.showControllerLayoutDialog();
        idle();
        assertTrue("the controls' editor is open", game.modernController.isEditing());
    }

    /**
     * Their editor opened while the game buttons are moved, and moving them ended first (Save): the controls come back
     * once their editor closes.
     */
    @Test public void theControlsComeBackWhenTheirEditorClosesAfterTheButtonsWereSaved() {
        Game game = stream(true, false);
        showGameButtonOver(game, largestControl(game));
        startMovingTheButtons(game);
        openTheControlsEditor(game);
        chooseInGameButtonsMenu(game, MOVE_OR_SAVE);
        assertFalse(game.gameTouchControls.isEditing());
        assertTrue("their editor stays open", game.modernController.isEditing());
        game.modernController.finishEditing(true);
        idle();
        assertEveryControlAnswers(game, "after their editor's Done");
    }

    /** The same with a reconnect switching to the running game's buttons, and the controls' editor left by Cancel. */
    @Test public void theControlsComeBackWhenTheirEditorClosesAfterAnotherGamesButtonsEndedTheMove() {
        Game game = stream(true, false);
        showGameButtonOver(game, largestControl(game));
        startMovingTheButtons(game);
        openTheControlsEditor(game);
        game.gameTouchControls.setProfile(TouchLayout.profileKey(PC, null, APP));
        idle();
        assertFalse(game.gameTouchControls.isEditing());
        assertTrue("their editor stays open", game.modernController.isEditing());
        game.modernController.finishEditing(false);
        idle();
        assertEveryControlAnswers(game, "after their editor's Cancel");
    }

    /** The same with Hide game buttons ending the move. */
    @Test public void theControlsComeBackWhenTheirEditorClosesAfterTheButtonsWereHidden() {
        Game game = stream(true, false);
        showGameButtonOver(game, largestControl(game));
        startMovingTheButtons(game);
        openTheControlsEditor(game);
        chooseInGameButtonsMenu(game, SHOW_OR_HIDE);
        assertFalse(game.gameTouchControls.isEditing());
        assertTrue("their editor stays open", game.modernController.isEditing());
        game.modernController.finishEditing(true);
        idle();
        assertEveryControlAnswers(game, "after their editor's Done");
    }

    /**
     * An open editor of the modern controls is the player's work: neither moving the game buttons nor what happens
     * meanwhile closes it, so its draft stays (closing without Done is what drops a draft).
     */
    @Test public void theControlsEditorKeepsItsDraftWhileTheButtonsAreMoved() {
        Game game = stream(true, false);
        showGameButtonOver(game, largestControl(game));
        openTheControlsEditor(game);
        startMovingTheButtons(game);
        assertTrue("moving the buttons kept it", game.modernController.isEditing());
        game.setPhysicalControllerConnected(true);
        idle();
        game.setPhysicalControllerConnected(false);
        idle();
        assertTrue("a controller coming and going kept it", game.modernController.isEditing());
        // The stream menu's switch.
        game.touchInput.setOnscreenControlsEnabled(true);
        idle();
        assertTrue("the controls' switch kept it", game.modernController.isEditing());
        game.modernController.finishEditing(true);
        idle();
        assertTrue("away while the buttons are still moved", visibleControls(game).isEmpty());
        chooseInGameButtonsMenu(game, MOVE_OR_SAVE);
        assertEveryControlAnswers(game, "after both editors closed");
    }

    /** The same with their editor opened after moving the buttons began, as the stream menu offers both. */
    @Test public void theControlsEditorOpenedWhileTheButtonsAreMovedKeepsItsDraft() {
        Game game = stream(true, false);
        showGameButtonOver(game, largestControl(game));
        startMovingTheButtons(game);
        openTheControlsEditor(game);
        game.setPhysicalControllerConnected(true);
        idle();
        game.setPhysicalControllerConnected(false);
        idle();
        assertTrue("a controller coming and going kept it", game.modernController.isEditing());
        game.touchInput.setOnscreenControlsEnabled(true);
        idle();
        assertTrue("the controls' switch kept it", game.modernController.isEditing());
        chooseInGameButtonsMenu(game, MOVE_OR_SAVE);
        assertTrue("saving the buttons kept it", game.modernController.isEditing());
        game.modernController.finishEditing(true);
        idle();
        assertEveryControlAnswers(game, "after both editors closed");
    }

    /**
     * A pad in hand keeps the controls off the screen (Hide while a controller is connected), yet their editor opens
     * on purpose. Moving the game buttons and saving them leaves that editor open, and its Done keeps the drag.
     */
    @Test public void withAPadInHandTheControlsEditorKeepsItsDraftAfterTheButtonsAreSaved() {
        Game game = stream(true, false);
        Rect control = largestControl(game);
        showGameButtonOver(game, control);
        game.setPhysicalControllerConnected(true);
        idle();
        assertTrue("a pad keeps the controls away", visibleControls(game).isEmpty());
        openTheControlsEditor(game);
        // The player drags that control toward the middle of the screen.
        ViewGroup frame = frame(game);
        float density = game.getResources().getDisplayMetrics().density;
        float dx = control.exactCenterX() < frame.getWidth() / 2f ? 40 * density : -40 * density;
        float dy = control.exactCenterY() < frame.getHeight() / 2f ? 40 * density : -40 * density;
        float[] from = middle(game, control);
        float[] to = {from[0] + dx, from[1] + dy};
        long now = SystemClock.uptimeMillis();
        dispatch(game, touch(now, now, MotionEvent.ACTION_DOWN, from));
        dispatch(game, touch(now, now + 50, MotionEvent.ACTION_MOVE, to));
        dispatch(game, touch(now, now + 100, MotionEvent.ACTION_UP, to));
        idle();
        startMovingTheButtons(game);
        chooseInGameButtonsMenu(game, MOVE_OR_SAVE);
        assertFalse(game.gameTouchControls.isEditing());
        assertTrue("saving the buttons kept their editor open", game.modernController.isEditing());
        game.modernController.finishEditing(true);
        idle();
        game.setPhysicalControllerConnected(false);
        idle();
        // Where the drag left it, give or take the editor's grid.
        boolean moved = false;
        for (Rect rect : visibleControls(game)) {
            moved |= Math.abs(rect.width() - control.width()) <= 2 && Math.abs(rect.height() - control.height()) <= 2
                    && Math.abs(rect.exactCenterX() - control.exactCenterX() - dx) <= 16 * density
                    && Math.abs(rect.exactCenterY() - control.exactCenterY() - dy) <= 16 * density;
        }
        assertTrue("the drag made in the editor was kept: " + control + " -> " + visibleControls(game), moved);
    }

    private static void layOut(View root) {
        root.measure(View.MeasureSpec.makeMeasureSpec(root.getWidth(), View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(root.getHeight(), View.MeasureSpec.EXACTLY));
        root.layout(0, 0, root.getWidth(), root.getHeight());
    }

    /**
     * Placing the game buttons asks for no further layout once they are in place. It asked after every layout of their
     * overlay, so the stream screen was laid out again on every frame while they showed. Laid out by hand here: the
     * main looper never went idle with that, so idling would hang instead of failing.
     */
    @Test public void shownGameButtonsLetTheLayoutSettle() {
        Game game = stream(true, false);
        Context context = RuntimeEnvironment.getApplication();
        String id = UUID.randomUUID().toString();
        TouchLayout.Button middle = new TouchLayout.Button(id, "Jump", KeyboardShortcuts.parseChord("SPACE"), 0, false,
                .45f, .45f, .1f, .1f);
        new TouchLayout(Collections.singletonList(middle)).save(context, BUTTONS_PROFILE);
        TouchLayout.setEnabled(context, BUTTONS_PROFILE, true);
        game.gameTouchControls.setProfile(BUTTONS_PROFILE);
        View decor = game.getWindow().getDecorView();
        // The first layout places the button; the next, as the next frame does, finds it in place.
        layOut(decor);
        View button = gameButton(game, id);
        game.gameTouchControls.requestLayout();
        layOut(decor);
        assertTrue("placed", button.getWidth() > 1);
        assertFalse("placing the buttons asked for another layout", button.isLayoutRequested());
        idle();
        assertFalse(button.isLayoutRequested());
    }
}
