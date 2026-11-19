package com.limelight;

import android.app.Application;
import android.media.MediaFormat;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamOrientation;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

/**
 * A UI action that met a decoder rebuild (a turn, a quality tick, the quality dialog's choice) runs
 * once the rebuild is over, in the order asked for, and a request the PC never got leaves the decoder
 * as it was.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = GameReconfigureRetryTest.ReconfigureBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni", qualifiers = "w800dp-h400dp-land")
public class GameReconfigureRetryTest {
    @Before public void reset() {
        ReconfigureBridge.requests.clear();
        ReconfigureBridge.accepts = true;
    }

    // Launched in a portrait window; the phone is in landscape now, so the stream asks to turn.
    private static Game turnedStream(Renderer[] renderer) {
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.fps = 60;
        prefs.width = 1080;
        prefs.height = 1920;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        renderer[0] = new Renderer(activity, prefs, 1080, 1920);
        ReflectionHelpers.setField(activity, "decoderRenderer", renderer[0]);
        ReflectionHelpers.setField(activity, "streamOrientation", StreamOrientation.ROTATE);
        ReflectionHelpers.setField(activity, "hostShapesStream", true);
        ReflectionHelpers.setField(activity.videoOutput, "launchWidth", 1080);
        ReflectionHelpers.setField(activity.videoOutput, "launchHeight", 1920);
        ReflectionHelpers.setField(activity, "streamFps", 60);
        ReflectionHelpers.setField(activity, "connected", true);
        return activity;
    }

    @Test
    public void aTurnDuringADecoderRebuildIsCarriedOutAfterIt() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        renderer[0].rebuilding = true;
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(ReconfigureBridge.requests.isEmpty());
        assertFalse("no decoder-limit notice for a decoder being rebuilt",
                ReflectionHelpers.<Boolean>getField(activity.videoOutput, "orientationNoticeShown"));

        // The rebuild configured its decoder and published what it can take.
        renderer[0].rebuilding = false;
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertEquals(1, ReconfigureBridge.requests.size());
        assertArrayEquals(new int[] {1920, 1080, 60000}, ReconfigureBridge.requests.get(0));
    }

    // A rebuild follows a decoder failure, just when automatic quality steps down: the tick waits for
    // it, keeps automatic quality on, and its step is taken after it.
    @Test
    public void aQualityStepDuringADecoderRebuildKeepsAutomaticQualityAndIsTakenAfterIt() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        // Battery above 60 fps steps down at once.
        com.limelight.nvstream.AdaptiveQualityController quality = new com.limelight.nvstream.AdaptiveQualityController(
                com.limelight.nvstream.AdaptiveQualityController.Mode.BATTERY, 1080, 1920, 120, 360, 30, 2, 2, -100000);
        List<com.limelight.nvstream.AdaptiveQualityController.Target> levels = ReflectionHelpers.getField(quality, "levels");
        ReflectionHelpers.setField(activity, "adaptiveQuality", quality);
        ReflectionHelpers.setField(activity, "foreground", true);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "listenForRebuilds");
        renderer[0].rebuilding = true;
        activity.updateQuality.run();
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertSame(quality, ReflectionHelpers.getField(activity, "adaptiveQuality"));
        assertNull("no error shown", ReflectionHelpers.getField(activity, "qualityStatus"));
        assertTrue(ReconfigureBridge.requests.isEmpty());

        renderer[0].rebuilding = false;
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper();
        assertEquals(1, ReconfigureBridge.requests.size());
        assertArrayEquals(levels.get(1).request(), ReconfigureBridge.requests.get(0));
        assertSame(levels.get(1), ReflectionHelpers.getField(activity, "pendingQualityTarget"));
    }

    // The window turned while the decoder was rebuilt, with automatic quality on: the new ceiling
    // is taken only once the PC was asked, after the rebuild.
    @Test
    public void aWindowChangeDuringARebuildWithAutomaticQualityIsTakenAfterIt() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        ReflectionHelpers.setField(activity, "adaptiveQuality", new com.limelight.nvstream.AdaptiveQualityController(
                com.limelight.nvstream.AdaptiveQualityController.Mode.DETAIL, 1080, 1920, 60, 360, 30, 2, 2, 0));
        renderer[0].rebuilding = true;
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(ReconfigureBridge.requests.isEmpty());
        assertNotEquals("no ceiling taken while waiting", 1920, (int) ReflectionHelpers.getField(activity, "qualityMaxWidth"));

        renderer[0].rebuilding = false;
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertEquals(1, ReconfigureBridge.requests.size());
        assertArrayEquals(new int[] {1920, 1080, 60000}, ReconfigureBridge.requests.get(0));
        assertEquals(1920, (int) ReflectionHelpers.getField(activity, "qualityMaxWidth"));
        assertEquals(1080, (int) ReflectionHelpers.getField(activity, "qualityMaxHeight"));
    }

    // After a reconnect automatic quality returns once to the step it had reached: landing in a
    // rebuild, that step is taken after it.
    @Test
    public void aResumeStepDuringARebuildIsTakenAfterIt() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        com.limelight.nvstream.AdaptiveQualityController quality = new com.limelight.nvstream.AdaptiveQualityController(
                com.limelight.nvstream.AdaptiveQualityController.Mode.DETAIL, 1080, 1920, 60, 360, 30, 2, 2, 0);
        List<com.limelight.nvstream.AdaptiveQualityController.Target> levels = ReflectionHelpers.getField(quality, "levels");
        com.limelight.nvstream.AdaptiveQualityController.Target step = levels.get(1);
        quality.resumeAt(step.width, step.height, step.fps);
        ReflectionHelpers.setField(activity, "adaptiveQuality", quality);
        ReflectionHelpers.setField(activity, "foreground", true);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "listenForRebuilds");
        renderer[0].rebuilding = true;
        activity.updateQuality.run();
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(ReconfigureBridge.requests.isEmpty());

        renderer[0].rebuilding = false;
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper();
        assertEquals(1, ReconfigureBridge.requests.size());
        assertArrayEquals(step.request(), ReconfigureBridge.requests.get(0));
        assertSame(step, quality.current());
    }

    // Automatic quality one step down on a plain stream (no native resolution, no rotation, a fixed
    // frame rate); the player turns it Off while the decoder is rebuilt: the stream returns to its
    // ceiling once the rebuild is over, asked for once.
    @Test
    public void turningAutomaticQualityOffDuringARebuildReturnsToTheCeilingAfterIt() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        ReflectionHelpers.setField(activity, "streamOrientation", StreamOrientation.LANDSCAPE);
        hosted(activity);
        renderer[0].size = new int[] {1280, 720};
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(ReconfigureBridge.requests.isEmpty());
        ReflectionHelpers.setField(activity, "qualityMaxWidth", 1920);
        ReflectionHelpers.setField(activity, "qualityMaxHeight", 1080);
        ReflectionHelpers.setField(activity, "qualityMaxFrameRateMilliHz", 60000);
        ReflectionHelpers.setField(activity, "adaptiveQuality", new com.limelight.nvstream.AdaptiveQualityController(
                com.limelight.nvstream.AdaptiveQualityController.Mode.DETAIL, 1920, 1080, 60, 360, 30, 2, 2, 0));
        ReflectionHelpers.<PreferenceConfiguration>getField(activity, "prefConfig").adaptiveQualityMode =
                com.limelight.nvstream.AdaptiveQualityController.Mode.OFF;
        renderer[0].rebuilding = renderer[0].limitsCleared = true;
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyQualityChoice");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(ReconfigureBridge.requests.isEmpty());

        renderer[0].rebuilding = renderer[0].limitsCleared = false;
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertEquals(1, ReconfigureBridge.requests.size());
        assertArrayEquals(new int[] {1920, 1080, 60000}, ReconfigureBridge.requests.get(0));
        // A second rebuild asks for nothing more.
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertEquals(1, ReconfigureBridge.requests.size());
    }

    // A game streamed from a PC that takes no bitrate changes.
    private static void hosted(Game activity) {
        com.limelight.nvstream.http.NvApp app = new com.limelight.nvstream.http.NvApp("Game", 73, false);
        ReflectionHelpers.setField(activity, "app", app);
        com.limelight.nvstream.NvConnection connection = new com.limelight.nvstream.NvConnection(null,
                new com.limelight.nvstream.http.ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                new com.limelight.nvstream.StreamConfiguration.Builder().setApp(app).build(), null, null) {
            @Override public void stop() {}
        };
        ReflectionHelpers.setField(connection, "hostDetails", new com.limelight.nvstream.http.ComputerDetails());
        ReflectionHelpers.setField(activity, "conn", connection);
    }

    // A turned Rotate stream (ceiling 1920x1080 at 60) with automatic quality on and stepped down in size
    // and frame rate, at no decoder rebuild; requests made so far are cleared.
    private static com.limelight.nvstream.AdaptiveQualityController.Target steppedDown(Game activity, Renderer renderer,
            com.limelight.nvstream.AdaptiveQualityController.Mode mode) {
        hosted(activity);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertEquals(1, ReconfigureBridge.requests.size());
        assertArrayEquals(new int[] {1920, 1080, 60000}, ReconfigureBridge.requests.get(0));
        renderer.size = new int[] {1920, 1080};
        com.limelight.nvstream.AdaptiveQualityController quality = new com.limelight.nvstream.AdaptiveQualityController(
                mode, 1920, 1080, 60, 360, 30, 2, 2, 0);
        List<com.limelight.nvstream.AdaptiveQualityController.Target> levels = ReflectionHelpers.getField(quality, "levels");
        com.limelight.nvstream.AdaptiveQualityController.Target step = null;
        for (com.limelight.nvstream.AdaptiveQualityController.Target level : levels) {
            if (level.width < 1920 && level.fps < 60) { step = level; break; }
        }
        assertNotNull(step);
        ReflectionHelpers.setField(activity, "adaptiveQuality", quality);
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyAdaptiveQuality",
                ReflectionHelpers.ClassParameter.from(com.limelight.nvstream.AdaptiveQualityController.Target.class, step),
                ReflectionHelpers.ClassParameter.from(long.class, 1000L));
        assertEquals(2, ReconfigureBridge.requests.size());
        renderer.size = new int[] {step.width, step.height};
        ReflectionHelpers.setField(activity, "pendingQualityTarget", null);
        ReconfigureBridge.requests.clear();
        return step;
    }

    // Test 1: automatic quality one step down in size and frame rate on a Rotate stream; Off is chosen
    // during a rebuild: once it is over the stream returns to the full size at the ceiling's frame rate.
    @Test
    public void offDuringARebuildOnARotateStreamReturnsToTheFullCeiling() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        steppedDown(activity, renderer[0], com.limelight.nvstream.AdaptiveQualityController.Mode.BATTERY);
        ReflectionHelpers.<PreferenceConfiguration>getField(activity, "prefConfig").adaptiveQualityMode =
                com.limelight.nvstream.AdaptiveQualityController.Mode.OFF;
        renderer[0].rebuilding = renderer[0].limitsCleared = true;
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyQualityChoice");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(ReconfigureBridge.requests.isEmpty());

        renderer[0].rebuilding = renderer[0].limitsCleared = false;
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertEquals(1, ReconfigureBridge.requests.size());
        assertArrayEquals(new int[] {1920, 1080, 60000}, ReconfigureBridge.requests.get(0));
        assertEquals(1920, (int) ReflectionHelpers.getField(activity, "qualityMaxWidth"));
        assertEquals(1080, (int) ReflectionHelpers.getField(activity, "qualityMaxHeight"));
        assertEquals(60, (int) ReflectionHelpers.getField(activity, "qualityMaxFps"));
        assertEquals(60000, (int) ReflectionHelpers.getField(activity, "qualityMaxFrameRateMilliHz"));
    }

    // Test 2: Motion chosen while the decoder is rebuilt: no "unavailable" notice, and automatic quality
    // runs once the rebuild is over.
    @Test
    public void motionChosenDuringARebuildStartsAfterIt() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        hosted(activity);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        renderer[0].size = new int[] {1920, 1080};
        ReconfigureBridge.requests.clear();
        ReflectionHelpers.<PreferenceConfiguration>getField(activity, "prefConfig").adaptiveQualityMode =
                com.limelight.nvstream.AdaptiveQualityController.Mode.MOTION;
        renderer[0].rebuilding = renderer[0].limitsCleared = true;
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyQualityChoice");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertNull("no unavailable notice", ReflectionHelpers.getField(activity, "qualityStatus"));

        renderer[0].rebuilding = renderer[0].limitsCleared = false;
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertNull(ReflectionHelpers.getField(activity, "qualityStatus"));
        assertNotNull("automatic quality runs", ReflectionHelpers.getField(activity, "adaptiveQuality"));
    }

    // Test 3: Off waits for a rebuild, and the window turns just before the rebuild ends. Off runs as it
    // would have when chosen (the ceiling at its frame rate, in the shape it had then); the turn is asked
    // for only once the window has settled, after it, and the stream ends in the new shape.
    @Test
    public void offWaitingWithAWindowChangeEndsInTheNewShape() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        steppedDown(activity, renderer[0], com.limelight.nvstream.AdaptiveQualityController.Mode.BATTERY);
        ReflectionHelpers.<PreferenceConfiguration>getField(activity, "prefConfig").adaptiveQualityMode =
                com.limelight.nvstream.AdaptiveQualityController.Mode.OFF;
        renderer[0].rebuilding = renderer[0].limitsCleared = true;
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyQualityChoice");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(ReconfigureBridge.requests.isEmpty());

        org.robolectric.RuntimeEnvironment.setQualifiers("w400dp-h800dp-port");
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "scheduleReconfigureCheck");
        renderer[0].rebuilding = renderer[0].limitsCleared = false;
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper();
        assertEquals("Off, before the window settled", 1, ReconfigureBridge.requests.size());
        assertArrayEquals(new int[] {1920, 1080, 60000}, ReconfigureBridge.requests.get(0));
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertEquals(2, ReconfigureBridge.requests.size());
        assertArrayEquals(new int[] {1080, 1920, 60000}, ReconfigureBridge.requests.get(1));
        assertEquals(1080, (int) ReflectionHelpers.getField(activity, "qualityMaxWidth"));
        assertEquals(60000, (int) ReflectionHelpers.getField(activity, "qualityMaxFrameRateMilliHz"));
    }

    private static java.util.LinkedHashMap<String, Runnable> waiting(Game activity) {
        return ReflectionHelpers.getField(activity.videoOutput, "decoderActions");
    }

    // Test 6: a turn, then Off, both during one rebuild, run in that order once it is over; the turn's
    // check, asked twice, waits once.
    @Test
    public void actionsDuringARebuildRunInTheOrderAskedAndChecksWaitOnce() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        hosted(activity);
        steppedDown(activity, renderer[0], com.limelight.nvstream.AdaptiveQualityController.Mode.BATTERY);
        renderer[0].rebuilding = renderer[0].limitsCleared = true;
        org.robolectric.RuntimeEnvironment.setQualifiers("w400dp-h800dp-port");
        activity.checkReconfigure.run();
        ReflectionHelpers.<PreferenceConfiguration>getField(activity, "prefConfig").adaptiveQualityMode =
                com.limelight.nvstream.AdaptiveQualityController.Mode.OFF;
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyQualityChoice");
        activity.checkReconfigure.run();
        assertEquals(java.util.Arrays.asList("check", "choice"), new ArrayList<>(waiting(activity).keySet()));
        assertTrue(ReconfigureBridge.requests.isEmpty());

        renderer[0].rebuilding = renderer[0].limitsCleared = false;
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(waiting(activity).isEmpty());
        assertFalse(ReconfigureBridge.requests.isEmpty());
        // The turn first (in the new shape), then Off at the turned ceiling; nothing in the old shape.
        for (int[] request : ReconfigureBridge.requests) assertTrue(request[0] < request[1]);
        assertArrayEquals(new int[] {1080, 1920, 60000},
                ReconfigureBridge.requests.get(ReconfigureBridge.requests.size() - 1));
        assertNull(ReflectionHelpers.getField(activity, "adaptiveQuality"));
    }

    // Test 8: the stream stops with actions waiting: nothing runs after the rebuild, no notice, no request.
    @Test
    public void nothingWaitingRunsOnceTheStreamStopped() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        hosted(activity);
        ReflectionHelpers.setField(activity, "foreground", true);
        renderer[0].rebuilding = renderer[0].limitsCleared = true;
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ReflectionHelpers.<PreferenceConfiguration>getField(activity, "prefConfig").adaptiveQualityMode =
                com.limelight.nvstream.AdaptiveQualityController.Mode.MOTION;
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyQualityChoice");
        activity.checkReconfigure.run();
        assertEquals(3, waiting(activity).size());

        ReflectionHelpers.setField(activity, "connected", false);
        renderer[0].rebuilding = renderer[0].limitsCleared = false;
        ReflectionHelpers.callInstanceMethod(renderer[0], "notifyReconfigureLimits");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(ReconfigureBridge.requests.isEmpty());
        assertNull(ReflectionHelpers.getField(activity, "adaptiveQuality"));
        assertNull(ReflectionHelpers.getField(activity, "qualityStatus"));
        assertNull(org.robolectric.shadows.ShadowToast.getLatestToast());
        // Asked again after the stop: nothing is kept for later either.
        activity.checkReconfigure.run();
        assertTrue(waiting(activity).isEmpty());
    }

    // Test 9: a rebuild that never ends: what waits stays waiting and the main looper stays free.
    @Test
    public void aRebuildThatNeverEndsKeepsActionsWaitingWithoutHoldingTheLooper() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        hosted(activity);
        com.limelight.nvstream.AdaptiveQualityController quality = new com.limelight.nvstream.AdaptiveQualityController(
                com.limelight.nvstream.AdaptiveQualityController.Mode.BATTERY, 1080, 1920, 120, 360, 30, 2, 2, -100000);
        ReflectionHelpers.setField(activity, "adaptiveQuality", quality);
        ReflectionHelpers.setField(activity, "foreground", true);
        renderer[0].rebuilding = renderer[0].limitsCleared = true;
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        activity.updateQuality.run();
        ReflectionHelpers.callInstanceMethod(activity.adaptation, "applyQualityChoice");
        ShadowLooper.idleMainLooper(60, TimeUnit.SECONDS);
        assertEquals(java.util.Arrays.asList("track", "tick", "choice"), new ArrayList<>(waiting(activity).keySet()));
        assertTrue(ReconfigureBridge.requests.isEmpty());
        assertTrue("nothing left posted that spins", ShadowLooper.shadowMainLooper().isIdle());
    }

    // Test 10: a menu opened during a rebuild still offers automatic quality.
    @Test
    public void aMenuOpenedDuringARebuildStillOffersAutomaticQuality() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        hosted(activity);
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(activity.adaptation, "showsAdaptiveQuality"));
        renderer[0].rebuilding = renderer[0].limitsCleared = true;
        assertTrue("offered while the limits are cleared for a moment",
                (boolean) ReflectionHelpers.callInstanceMethod(activity.adaptation, "showsAdaptiveQuality"));
        renderer[0].rebuilding = false;
        assertFalse("a decoder that cannot change size is not offered",
                (boolean) ReflectionHelpers.callInstanceMethod(activity.adaptation, "showsAdaptiveQuality"));
    }

    @Test
    public void aRequestThatWasNotSentIsTakenBackFromTheDecoder() {
        ReconfigureBridge.accepts = false;
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertFalse(ReconfigureBridge.requests.isEmpty());
        assertEquals(renderer[0].prepared, renderer[0].cancelled);
    }

    private static class Renderer extends MediaCodecDecoderRenderer {
        private final int width, height;
        int[] size;
        volatile boolean rebuilding, limitsCleared;
        int prepared, cancelled;

        Renderer(Game activity, PreferenceConfiguration prefs, int width, int height) {
            super(activity, prefs, null, 0, false, false, "", null);
            this.width = width;
            this.height = height;
            ReflectionHelpers.setField(this, "configuredFormat", MediaFormat.createVideoFormat("video/avc", width, height));
        }

        @Override public int[] getStreamSize() {
            return size != null ? size : new int[] {width, height};
        }

        @Override public boolean hasRecentVideoFrames(long nowMs) {
            return true;
        }

        // Read just before the rebuild cleared them: the size check passes, the decoder then refuses.
        @Override public int[] getReconfigureLimits() {
            return limitsCleared ? null : new int[] {4096, 4096, 2, 2};
        }

        @Override public boolean prepareForReconfigure(int width, int height) {
            if (rebuilding) return false;
            prepared++;
            return true;
        }

        @Override public boolean reconfigureLimitsPending() {
            return rebuilding;
        }

        @Override public void cancelReconfigure() {
            cancelled++;
        }
    }

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class ReconfigureBridge {
        static final List<int[]> requests = new ArrayList<>();
        static boolean accepts = true;

        @Implementation protected static void __staticInitializer__() { }

        @Implementation public static boolean hostSupportsControlMessage(int messageType) {
            return true;
        }

        @Implementation public static long getEstimatedRttInfo() {
            return -1;
        }

        @Implementation public static boolean requestReconfigure(int width, int height, int fpsMillihz) {
            requests.add(new int[] {width, height, fpsMillihz});
            return accepts;
        }
    }
}
