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
 * A turn the decoder could not take at that moment (it was being rebuilt) is carried out once it can,
 * and a request the PC never got leaves the decoder as it was.
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

    // A rebuild follows a decoder failure, just when automatic quality steps down: the step waits for it.
    @Test
    public void aQualityStepDuringADecoderRebuildKeepsAutomaticQualityAndIsTakenAfterIt() {
        Renderer[] renderer = new Renderer[1];
        Game activity = turnedStream(renderer);
        com.limelight.nvstream.AdaptiveQualityController quality = new com.limelight.nvstream.AdaptiveQualityController(
                com.limelight.nvstream.AdaptiveQualityController.Mode.DETAIL, 1080, 1920, 60, 360, 30, 2, 2, 0);
        ReflectionHelpers.setField(activity, "adaptiveQuality", quality);
        renderer[0].rebuilding = true;
        assertEquals("the quality tick goes on", GameVideoOutput.Reconfigure.PENDING,
                activity.adaptation.applyAdaptiveQuality(quality.current(), 1000));
        assertSame(quality, ReflectionHelpers.getField(activity, "adaptiveQuality"));
        assertNull("no error shown", ReflectionHelpers.getField(activity, "qualityStatus"));
        assertTrue(ReconfigureBridge.requests.isEmpty());

        // The next tick, once the decoder is set up again, asks the PC.
        renderer[0].rebuilding = false;
        assertEquals(GameVideoOutput.Reconfigure.SENT, activity.adaptation.applyAdaptiveQuality(quality.current(), 2000));
        assertEquals(1, ReconfigureBridge.requests.size());
        assertSame(quality.current(), ReflectionHelpers.getField(activity, "pendingQualityTarget"));
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
        assertEquals("no ceiling taken while waiting", 1080, (int) ReflectionHelpers.getField(activity, "qualityMaxWidth"));

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
        renderer[0].rebuilding = true;
        activity.updateQuality.run();
        assertTrue(ReconfigureBridge.requests.isEmpty());

        renderer[0].rebuilding = false;
        activity.updateQuality.run();
        assertEquals(1, ReconfigureBridge.requests.size());
        assertArrayEquals(step.request(), ReconfigureBridge.requests.get(0));
        assertSame(step, quality.current());
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
        volatile boolean rebuilding;
        int prepared, cancelled;

        Renderer(Game activity, PreferenceConfiguration prefs, int width, int height) {
            super(activity, prefs, null, 0, false, false, "", null);
            this.width = width;
            this.height = height;
            ReflectionHelpers.setField(this, "configuredFormat", MediaFormat.createVideoFormat("video/avc", width, height));
        }

        @Override public int[] getStreamSize() {
            return new int[] {width, height};
        }

        @Override public boolean hasRecentVideoFrames(long nowMs) {
            return true;
        }

        // Read just before the rebuild cleared them: the size check passes, the decoder then refuses.
        @Override public int[] getReconfigureLimits() {
            return new int[] {4096, 4096, 2, 2};
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
