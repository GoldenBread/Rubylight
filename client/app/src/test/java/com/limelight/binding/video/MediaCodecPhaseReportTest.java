package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.media.MediaFormat;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.Field;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

import static org.junit.Assert.*;

/** The phase lock report with Real VRR (host frame times), as with lowest latency. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = {MediaCodecPhaseReportTest.SlackBridge.class, MediaCodecPhaseReportTest.OneFrameCodec.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class MediaCodecPhaseReportTest {
    private static final long VSYNC_NS = 1_000_000_000L;
    private static final long INTERVAL_NS = 10_000_000L;
    private static final long DEADLINE_NS = 2_000_000L;

    @Before public void resetBridge() {
        SlackBridge.slacks.clear();
        SlackBridge.intervals.clear();
    }

    @Test public void pcFrameTimesReportsThePhaseWithRoomBeforeTheLatch() {
        for (boolean hostFrameTimes : new boolean[] {false, true}) {
            try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
                MediaCodecDecoderRenderer renderer = renderer(controller.get(), hostFrameTimes);
                assertEquals("the scheduler places frames only with host frame times", hostFrameTimes,
                        FramePacingPolicy.schedulesFrames(hostFrameTimes, PreferenceConfiguration.FRAME_PACING_MIN_LATENCY));
                SlackBridge.slacks.clear();
                SlackBridge.intervals.clear();
                samplePhase(renderer);
                // Ready 5 ms after a vsync: 3 ms before the next latch deadline, and the scheduler
                // asks for 1 ms more room before it.
                report(renderer, VSYNC_NS + 5_000_000L);
                long expected = hostFrameTimes ? 3_000_000L - MediaCodecDecoderRenderer.SCHEDULED_LATCH_ROOM_NS : 3_000_000L;
                assertEquals(Collections.singletonList(expected), SlackBridge.slacks);
                assertEquals(Collections.singletonList(INTERVAL_NS), SlackBridge.intervals);
            }
        }
    }

    @Test public void pcFrameTimesStillSkipsAdaptiveRefreshUpscalingAndAMissingVsync() throws Exception {
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = renderer(controller.get(), true);
            samplePhase(renderer);

            ReflectionHelpers.setField(renderer, "outputHasArrSupport", true);
            report(renderer, VSYNC_NS + 5_000_000L);
            assertTrue("adaptive refresh has no fixed latch", SlackBridge.slacks.isEmpty());
            // Fixed timing on adaptive screens: with VRR off the ARR screen is timed as fixed and reports.
            PreferenceConfiguration config = ReflectionHelpers.getField(renderer, "prefs");
            config.arrFixedTiming = true;
            report(renderer, VSYNC_NS + 5_000_000L);
            assertEquals("fixed timing reports on an ARR screen", 1, SlackBridge.slacks.size());
            config.arrFixedTiming = false;
            SlackBridge.slacks.clear();
            SlackBridge.intervals.clear();
            ReflectionHelpers.setField(renderer, "outputHasArrSupport", false);

            ReflectionHelpers.setField(renderer, "upscaler", unconstructed(GlesUpscaler.class));
            report(renderer, VSYNC_NS + 5_000_000L);
            assertTrue("an upscaled frame is not display-ready", SlackBridge.slacks.isEmpty());
            ReflectionHelpers.setField(renderer, "upscaler", null);

            ReflectionHelpers.setField(renderer, "outputVsyncAvailable", false);
            report(renderer, VSYNC_NS + 5_000_000L);
            assertTrue("no vsync of the output display", SlackBridge.slacks.isEmpty());
            ReflectionHelpers.setField(renderer, "outputVsyncAvailable", true);

            ReflectionHelpers.setField(renderer, "vsyncTimeNs", 0L);
            report(renderer, VSYNC_NS + 5_000_000L);
            assertTrue("no phase before the first vsync", SlackBridge.slacks.isEmpty());

            samplePhase(renderer);
            report(renderer, VSYNC_NS + 5_000_000L);
            assertEquals("reported once nothing stops it",
                    Collections.singletonList(3_000_000L - MediaCodecDecoderRenderer.SCHEDULED_LATCH_ROOM_NS), SlackBridge.slacks);
        }
    }

    @Test public void aScheduledFrameReportsThePhaseOfItsReadyTimeNotOfItsDueTime() throws Exception {
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            ShadowSystemClock.advanceBy(Duration.ofSeconds(2));
            MediaCodecDecoderRenderer renderer = renderer(controller.get(), true);
            renderer.setStreamFrameRateMilliHz(60_000);
            OneFrameCodec.renderer = renderer;
            OneFrameCodec.stats = ReflectionHelpers.getField(renderer, "frameLatencyStats");
            OneFrameCodec.dequeues = OneFrameCodec.dropped = 0;
            OneFrameCodec.releasedAtNs = OneFrameCodec.dueNs = 0;
            MediaCodec codec = MediaCodec.createDecoderByType("video/avc");
            try {
                ReflectionHelpers.setField(renderer, "videoDecoder", codec);
                // Refreshes long enough that the test's own delays stay inside one. With no host
                // time the scheduler stamps the frame due at its ready time plus the deadline.
                long vsyncNs = System.nanoTime();
                long intervalNs = 10_000_000_000L;
                long deadlineNs = 4_000_000_000L;
                ReflectionHelpers.setField(renderer, "vsyncTimeNs", vsyncNs);
                ReflectionHelpers.setField(renderer, "vsyncIntervalNs", intervalNs);
                ReflectionHelpers.setField(renderer, "vsyncPresentationDeadlineNs", deadlineNs);
                ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
                Thread thread = ReflectionHelpers.getField(renderer, "rendererThread");
                thread.join(5000);
                long doneNs = System.nanoTime();
                assertFalse("Renderer did not finish its output turn", thread.isAlive());

                assertEquals(0, OneFrameCodec.dropped);
                assertTrue("released ahead of its due time",
                        OneFrameCodec.dueNs - OneFrameCodec.releasedAtNs > deadlineNs / 2);
                assertEquals(1, SlackBridge.slacks.size());
                long slackNs = SlackBridge.slacks.get(0);
                // Ready after the release call returned and before the renderer thread ended.
                long latchNs = deadlineNs + MediaCodecDecoderRenderer.SCHEDULED_LATCH_ROOM_NS;
                assertTrue("slack " + slackNs, slackNs <= MediaCodecDecoderRenderer.latchSlackNs(
                        OneFrameCodec.releasedAtNs, vsyncNs, intervalNs, latchNs));
                assertTrue("slack " + slackNs, slackNs >= MediaCodecDecoderRenderer.latchSlackNs(
                        doneNs, vsyncNs, intervalNs, latchNs));
            } finally {
                renderer.stop();
                codec.release();
            }
        }
    }

    @Test public void fractionalVsyncHoldsNothingWithVrrChosenOnADisplayWithoutArr() {
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = renderer(controller.get(), false);
            PreferenceConfiguration prefs = ReflectionHelpers.getField(renderer, "prefs");
            prefs.fractionalVsync = true;
            renderer.setStreamFrameRateMilliHz(60_000);
            ReflectionHelpers.setField(renderer, "vsyncIntervalNs", 8_333_333L);
            ReflectionHelpers.setField(renderer, "outputHasArrSupport", false);
            assertEquals("VRR off: 60 fps on 120 Hz holds 2 refreshes", 2, heldRefreshes(renderer));
            // VRR Fastest (Real VRR off) on a phone without ARR: the row is hidden, nothing holds.
            prefs.vrr = true;
            prefs.realVrr = false;
            assertEquals(1, heldRefreshes(renderer));
            prefs.vrr = false;
            ReflectionHelpers.setField(renderer, "outputHasArrSupport", true);
            assertEquals("ARR refreshes when a frame comes", 1, heldRefreshes(renderer));
            prefs.arrFixedTiming = true;
            assertEquals("ARR timed as a fixed refresh", 2, heldRefreshes(renderer));
            prefs.vrr = true;
            assertEquals("and VRR chosen", 1, heldRefreshes(renderer));
        }
    }

    /**
     * Smoothest video and Balanced with FPS limit at 60 fps on 120 Hz, Fractional V-Sync off: each frame is
     * stamped half a refresh ahead of its own turn, up to a stream frame past the first refresh it can make,
     * while Lowest latency's hold follows the setting and releases at once. At the stream's rate, with VRR or
     * before a vsync they are released with no time, which the compositor never drops, as before.
     */
    @Test public void smoothPacingHoldsAnEarlyFrameForItsTurnWhateverFractionalVsyncSays() {
        long interval = 8_333_333L;
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            for (int mode : new int[] {PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS,
                    PreferenceConfiguration.FRAME_PACING_CAP_FPS}) {
                String name = "mode " + mode;
                MediaCodecDecoderRenderer renderer = renderer(controller.get(), false);
                PreferenceConfiguration prefs = ReflectionHelpers.getField(renderer, "prefs");
                prefs.framePacing = mode;
                prefs.fractionalVsync = false;
                renderer.setStreamFrameRateMilliHz(60_000);
                ReflectionHelpers.setField(renderer, "outputHasArrSupport", false);
                ReflectionHelpers.setField(renderer, "vsyncTimeNs", VSYNC_NS);
                ReflectionHelpers.setField(renderer, "vsyncIntervalNs", interval);
                ReflectionHelpers.setField(renderer, "vsyncPresentationDeadlineNs", DEADLINE_NS);
                long release = VSYNC_NS + 3 * interval + 1_000_000L;
                long earliest = MediaCodecDecoderRenderer.nextVsyncTimeNs(release, VSYNC_NS, interval, DEADLINE_NS);
                assertEquals(name, earliest - interval / 2, smoothDisplayNs(renderer, release));
                assertEquals(name, earliest + 2 * interval - interval / 2, smoothDisplayNs(renderer, release + 1_000_000L));
                assertEquals(name + ": the third of a burst takes the second's refresh",
                        earliest + 2 * interval - interval / 2, smoothDisplayNs(renderer, release + 2_000_000L));
                assertEquals("the phase report aims at the turns", 2,
                        ReflectionHelpers.<Integer>getField(renderer, "heldAnchorRefreshes").intValue());
                long lowest = ReflectionHelpers.callInstanceMethod(renderer, "heldDisplayNs",
                        ReflectionHelpers.ClassParameter.from(long.class, release));
                assertEquals(name + ": Lowest latency's hold follows the setting", release, lowest);

                ReflectionHelpers.setField(renderer, "vsyncIntervalNs", 16_666_667L);
                assertEquals(name + " at 60 Hz", 0L, smoothDisplayNs(renderer, release));
                ReflectionHelpers.setField(renderer, "vsyncIntervalNs", interval);
                prefs.vrr = true;
                assertEquals(name + " with VRR", 0L, smoothDisplayNs(renderer, release));
                prefs.vrr = false;
                ReflectionHelpers.setField(renderer, "vsyncTimeNs", 0L);
                assertEquals(name + " before a vsync", 0L, smoothDisplayNs(renderer, release));
            }
        }
    }

    /** The release path stamps a Smoothest frame on a screen at twice the stream's rate, and none at its rate. */
    @Test public void smoothestReleasesTheFrameStampedForItsTurn() {
        long interval = 8_333_333L;
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            for (int fps : new int[] {60, 120}) {
                MediaCodecDecoderRenderer renderer = renderer(controller.get(), false);
                PreferenceConfiguration prefs = ReflectionHelpers.getField(renderer, "prefs");
                prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS;
                renderer.setStreamFrameRateMilliHz(fps * 1000);
                ReflectionHelpers.setField(renderer, "outputHasArrSupport", false);
                long vsyncNs = System.nanoTime();
                ReflectionHelpers.setField(renderer, "vsyncTimeNs", vsyncNs);
                ReflectionHelpers.setField(renderer, "vsyncIntervalNs", interval);
                ReflectionHelpers.setField(renderer, "vsyncPresentationDeadlineNs", DEADLINE_NS);
                OneFrameCodec.renderer = renderer;
                OneFrameCodec.stats = ReflectionHelpers.getField(renderer, "frameLatencyStats");
                OneFrameCodec.dequeues = OneFrameCodec.dropped = 0;
                OneFrameCodec.releasedAtNs = OneFrameCodec.dueNs = -1;
                MediaCodec codec;
                try {
                    codec = MediaCodec.createDecoderByType("video/avc");
                } catch (java.io.IOException e) {
                    throw new AssertionError(e);
                }
                try {
                    ReflectionHelpers.setField(renderer, "videoDecoder", codec);
                    ReflectionHelpers.callInstanceMethod(renderer, "releaseNewestOutput",
                            ReflectionHelpers.ClassParameter.from(int.class, 0),
                            ReflectionHelpers.ClassParameter.from(long.class, 1L),
                            ReflectionHelpers.ClassParameter.from(MediaCodec.BufferInfo.class, new MediaCodec.BufferInfo()),
                            ReflectionHelpers.ClassParameter.from(long.class, android.os.SystemClock.elapsedRealtimeNanos()));
                    assertEquals(0, OneFrameCodec.dropped);
                    if (fps == 120) {
                        assertEquals("at the screen's rate: no time", 0L, OneFrameCodec.dueNs);
                    } else {
                        long dueNs = OneFrameCodec.dueNs;
                        assertTrue("stamped " + dueNs, dueNs > 0);
                        assertEquals("half a refresh ahead of a refresh", 0,
                                Math.floorMod(dueNs + interval / 2 - vsyncNs, interval));
                        assertTrue("for a refresh to come", dueNs + interval / 2 > OneFrameCodec.releasedAtNs);
                    }
                } finally {
                    ReflectionHelpers.setField(renderer, "stopping", true);
                    codec.release();
                }
            }
        }
    }

    /**
     * A burst settles back: at 60 fps on 120 Hz frames 10 and 11 come late with 12. The third of them takes the
     * second's refresh, where the compositor shows it; the next frames keep a frame of delay, which the turns
     * give back a refresh at a time (FramePacingPolicy.earlyForTurn). No frame waits more than a stream frame
     * past the first refresh it can make, and no other frame is replaced.
     */
    @Test public void smoothPacingKeepsAFrameOfABurstAndGivesItBack() {
        long interval = 8_333_333L, frame = 16_666_667L;
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = smoothRenderer(controller.get(), interval);
            int frames = 13 + 400;
            long[] releases = new long[frames];
            for (int j = 0; j < frames; j++) releases[j] = VSYNC_NS + 3_000_000L + j * frame;
            releases[10] = releases[12];
            releases[11] = releases[12] + 500_000L;
            releases[12] += 1_000_000L;
            long[][] turns = smoothTurns(renderer, interval, releases);
            for (int j = 0; j < frames; j++) {
                long delay = turns[j][1] - turns[j][0];
                assertTrue("frame " + j + " held " + delay, delay >= 0 && delay <= 2 * interval);
                if (j == 12) {
                    assertEquals("the third of the burst takes the second's refresh", turns[11][1], turns[12][1]);
                } else if (j > 0) {
                    assertTrue("frame " + j + " has a refresh of its own", turns[j][1] > turns[j - 1][1]);
                }
            }
            assertEquals("a frame of the burst kept", 2 * interval, turns[13][1] - turns[13][0]);
            assertEquals("and given back", 0, turns[frames - 1][1] - turns[frames - 1][0]);
        }
    }

    /**
     * A PC slightly faster than the screen settles too: 60.000 fps on 119.88 Hz brings 0.12 refreshes a second
     * more than turns of two refreshes show. The turns move a refresh earlier now and then (one frame up for a
     * refresh, about 14 in two minutes), so no frame is replaced and none waits more than a stream frame.
     */
    @Test public void smoothPacingKeepsUpWithAPcSlightlyFasterThanTheScreen() {
        long interval = 8_341_675L, frame = 16_666_667L;
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = smoothRenderer(controller.get(), interval);
            int frames = 7200;
            long[] releases = new long[frames];
            for (int j = 0; j < frames; j++) releases[j] = VSYNC_NS + 3_000_000L + j * frame;
            long[][] turns = smoothTurns(renderer, interval, releases);
            int shortened = 0;
            for (int j = 0; j < frames; j++) {
                long delay = turns[j][1] - turns[j][0];
                assertTrue("frame " + j + " held " + delay, delay >= 0 && delay <= 2 * interval);
                if (j == 0) continue;
                long gap = turns[j][1] - turns[j - 1][1];
                assertTrue("frame " + j + " after " + gap, gap == interval || gap == 2 * interval);
                if (gap == interval) shortened++;
            }
            assertTrue("frames up for one refresh: " + shortened, shortened >= 12 && shortened <= 17);
        }
    }

    private static MediaCodecDecoderRenderer smoothRenderer(Activity activity, long interval) {
        MediaCodecDecoderRenderer renderer = renderer(activity, false);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(renderer, "prefs");
        prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS;
        renderer.setStreamFrameRateMilliHz(60_000);
        ReflectionHelpers.setField(renderer, "outputHasArrSupport", false);
        ReflectionHelpers.setField(renderer, "vsyncTimeNs", VSYNC_NS);
        ReflectionHelpers.setField(renderer, "vsyncIntervalNs", interval);
        ReflectionHelpers.setField(renderer, "vsyncPresentationDeadlineNs", DEADLINE_NS);
        return renderer;
    }

    /** Each release through smoothDisplayNs: {the first refresh it can make, the refresh it is stamped for}. */
    private static long[][] smoothTurns(MediaCodecDecoderRenderer renderer, long interval, long[] releasesNs) {
        long[][] turns = new long[releasesNs.length][];
        for (int j = 0; j < releasesNs.length; j++) {
            long earliest = MediaCodecDecoderRenderer.nextVsyncTimeNs(releasesNs[j], VSYNC_NS, interval, DEADLINE_NS);
            turns[j] = new long[] {earliest, smoothDisplayNs(renderer, releasesNs[j]) + interval / 2};
        }
        return turns;
    }

    private static long smoothDisplayNs(MediaCodecDecoderRenderer renderer, long releaseNs) {
        return ReflectionHelpers.callInstanceMethod(renderer, "smoothDisplayNs",
                ReflectionHelpers.ClassParameter.from(long.class, releaseNs));
    }

    private static int heldRefreshes(MediaCodecDecoderRenderer renderer) {
        return ReflectionHelpers.callInstanceMethod(renderer, "heldRefreshes");
    }

    private static MediaCodecDecoderRenderer renderer(Activity activity, boolean hostFrameTimes) {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.upscalingMode = UpscalingPolicy.Mode.OFF;
        prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
        prefs.hostFrameTimes = hostFrameTimes;
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity, prefs, null, 0, false, false, "",
                null, activity.getWindowManager().getDefaultDisplay());
        // The stream setup copies the preference.
        ReflectionHelpers.setField(renderer, "hostFrameTimes", hostFrameTimes);
        return renderer;
    }

    private static void samplePhase(MediaCodecDecoderRenderer renderer) {
        ReflectionHelpers.setField(renderer, "vsyncTimeNs", VSYNC_NS);
        ReflectionHelpers.setField(renderer, "vsyncIntervalNs", INTERVAL_NS);
        ReflectionHelpers.setField(renderer, "vsyncPresentationDeadlineNs", DEADLINE_NS);
    }

    private static void report(MediaCodecDecoderRenderer renderer, long readyNs) {
        ReflectionHelpers.callInstanceMethod(renderer, "reportPhaseSlack",
                ReflectionHelpers.ClassParameter.from(long.class, readyNs));
    }

    /** An instance whose constructor never ran (no GL thread): only its presence is read. */
    private static <T> T unconstructed(Class<T> type) throws Exception {
        Field theUnsafe = Class.forName("sun.misc.Unsafe").getDeclaredField("theUnsafe");
        theUnsafe.setAccessible(true);
        Object unsafe = theUnsafe.get(null);
        return type.cast(unsafe.getClass().getMethod("allocateInstance", Class.class).invoke(unsafe, type));
    }

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class SlackBridge {
        static final List<Long> slacks = new ArrayList<>();
        static final List<Long> intervals = new ArrayList<>();
        @Implementation protected static void __staticInitializer__() { }
        @Implementation protected static boolean reportFrameSlack(long slackNs, long intervalNs) {
            slacks.add(slackNs);
            intervals.add(intervalNs);
            return false;
        }
    }

    /** One decoded frame, then nothing: the renderer loop's next wait ends it. */
    @Implements(MediaCodec.class)
    public static class OneFrameCodec extends ShadowMediaCodec {
        static MediaCodecDecoderRenderer renderer;
        static FrameLatencyStats stats;
        static int dequeues, dropped;
        static long releasedAtNs, dueNs;

        @Implementation public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            if (dequeues++ > 0) {
                // A poll for a newer output (no timeout) finds none; the loop's wait stops it.
                if (timeoutUs > 0) ReflectionHelpers.setField(renderer, "stopping", true);
                return MediaCodec.INFO_TRY_AGAIN_LATER;
            }
            long enqueue = System.nanoTime() - 1_000_000L;
            info.presentationTimeUs = 1;
            stats.onDecoderInput(1, 1, enqueue, enqueue, enqueue, (char) 0);
            return 0;
        }

        @Implementation public MediaFormat getOutputFormat() {
            return MediaFormat.createVideoFormat("video/avc", 1920, 1080);
        }

        @Implementation public void releaseOutputBuffer(int index, boolean render) {
            dropped++;
        }

        @Implementation public void releaseOutputBuffer(int index, long timestampNs) {
            releasedAtNs = System.nanoTime();
            dueNs = timestampNs;
        }
    }
}
