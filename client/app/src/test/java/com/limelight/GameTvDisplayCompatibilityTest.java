package com.limelight;

import android.app.Application;
import android.graphics.drawable.ColorDrawable;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.ViewGroup;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.CompositorHeartbeat;
import com.limelight.ui.StreamView;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Settings › TV display compatibility in the stream: the dot runs in the window that holds the video while the
 * stream shows, read from the stored value (so a phone runs it too), and stops on pause, at the stream's end and
 * with the screen.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameTvDisplayCompatibilityTest {
    private ActivityController<Game> controller;

    @After
    public void clear() {
        if (controller != null) controller.destroy();
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    /** A stream screen whose stream has started, on a phone, with the setting stored as given. */
    private Game streaming(boolean on) {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean("checkbox_tv_display_compatibility", on).commit();
        controller = Robolectric.buildActivity(Game.class).create();
        Game game = controller.get();
        game.setTheme(R.style.StreamTheme);
        game.setContentView(R.layout.activity_game);
        StreamView stream = game.findViewById(R.id.surfaceView);
        game.streamView = stream;
        game.renderView = stream;
        game.prefConfig = PreferenceConfiguration.readPreferences(game);
        NvApp app = new NvApp("Desktop", 73, false);
        ReflectionHelpers.setField(game, "app", app);
        ReflectionHelpers.setField(game, "conn", new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "test", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
            @Override public void stop() {}
        });
        game.foreground = true;
        game.connected = true;
        // As GameConnection.connectionStarted does once the stream is up.
        game.videoOutput.updateCompositorHeartbeat();
        shadowOf(Looper.getMainLooper()).idle();
        return game;
    }

    private static void idle(long ms) {
        shadowOf(Looper.getMainLooper()).idleFor(ms, TimeUnit.MILLISECONDS);
    }

    private static int color(View dot) {
        return ((ColorDrawable) dot.getBackground()).getColor();
    }

    private static View dot(Game game) {
        CompositorHeartbeat heartbeat = game.videoOutput.compositorHeartbeat;
        return heartbeat == null ? null : heartbeat.dot();
    }

    @Test
    public void onTheDotShowsOverTheVideoAndAlternatesEvery100Ms() {
        Game game = streaming(true);
        View dot = dot(game);
        assertNotNull("running during the stream", dot);
        assertSame("in the window that holds the video", game.renderView.getParent(), dot.getParent());
        assertEquals(View.VISIBLE, dot.getVisibility());
        int first = color(dot);
        idle(100);
        int second = color(dot);
        assertNotEquals(first, second);
        idle(100);
        assertEquals(first, color(dot));
        assertFalse(dot.isFocusable());
        assertFalse(dot.isClickable());
        assertEquals(View.IMPORTANT_FOR_ACCESSIBILITY_NO, dot.getImportantForAccessibility());
    }

    @Test
    public void pauseStopsAndRemovesItAndResumeBringsItBack() {
        Game game = streaming(true);
        View dot = dot(game);
        controller.pause();
        assertNull(game.videoOutput.compositorHeartbeat);
        assertNull("gone on pause", dot.getParent());
        int paused = color(dot);
        idle(300);
        assertEquals("no tick while paused", paused, color(dot));
        controller.resume();
        assertNotNull("back with the stream", dot(game));
    }

    @Test
    public void theStreamsEndStopsAndRemovesIt() {
        Game game = streaming(true);
        View dot = dot(game);
        ReflectionHelpers.callInstanceMethod(game.streamConnection, "stopConnection");
        assertNull(game.videoOutput.compositorHeartbeat);
        assertNull("gone at the stream's end", dot.getParent());
        int ended = color(dot);
        idle(300);
        assertEquals(ended, color(dot));
    }

    @Test
    public void nothingIsLeftWhenTheScreenGoes() {
        Game game = streaming(true);
        View dot = dot(game);
        controller.destroy();
        controller = null;
        assertNull(game.videoOutput.compositorHeartbeat);
        assertNull(dot.getParent());
    }

    @Test
    public void offThereIsNoDotAndNoTicking() {
        Game game = streaming(false);
        ViewGroup window = (ViewGroup) game.renderView.getParent();
        int children = window.getChildCount();
        assertNull(game.videoOutput.compositorHeartbeat);
        idle(300);
        assertEquals(children, window.getChildCount());
        assertNull(game.videoOutput.compositorHeartbeat);
    }
}
