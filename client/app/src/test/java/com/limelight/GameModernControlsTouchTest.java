package com.limelight;

import android.app.Application;
import android.content.ComponentName;
import android.content.Context;
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

import com.limelight.binding.input.TouchLayout;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
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
}
