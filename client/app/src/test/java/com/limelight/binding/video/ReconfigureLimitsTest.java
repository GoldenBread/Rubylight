package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaFormat;
import android.view.Surface;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.MediaCodecInfoBuilder.CodecCapabilitiesBuilder;
import org.robolectric.shadows.ShadowMediaCodecList;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.fail;

/**
 * The UI reads the sizes a decoder can switch to, and asks it for a new size or frame rate, without
 * waiting for a reset that holds the decoder.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {ReconfigureLimitsTest.ObservingCodec.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ReconfigureLimitsTest {
    private ActivityController<Activity> activity;
    private SurfaceTexture texture;
    private Surface surface;
    private MediaCodecDecoderRenderer renderer;
    private MediaCodecInfo previousInfo;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecHelper.forgetCodecList();
        MediaFormat capabilities = MediaFormat.createVideoFormat("video/hevc", 3840, 2160);
        capabilities.setFeatureEnabled(CodecCapabilities.FEATURE_AdaptivePlayback, true);
        // The level sets the sizes and rates the decoder takes.
        MediaCodecInfo.CodecProfileLevel level = new MediaCodecInfo.CodecProfileLevel();
        level.profile = MediaCodecInfo.CodecProfileLevel.HEVCProfileMain;
        level.level = MediaCodecInfo.CodecProfileLevel.HEVCMainTierLevel51;
        MediaCodecInfo info = MediaCodecInfoBuilder.newBuilder().setName("c2.acme.hevc.decoder")
                .setIsHardwareAccelerated(true).setCapabilities(CodecCapabilitiesBuilder.newBuilder()
                        .setMediaFormat(capabilities)
                        .setProfileLevels(new MediaCodecInfo.CodecProfileLevel[] {level})
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
        ShadowMediaCodecList.addCodec(info);
        previousInfo = ObservingCodec.info;
        ObservingCodec.info = info;
        activity = Robolectric.buildActivity(Activity.class).create();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        prefs.width = 1920;
        prefs.height = 1080;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        texture = new SurfaceTexture(0);
        surface = new Surface(texture);
        ReflectionHelpers.setField(renderer, "renderTarget", surface);
    }

    @After
    public void tearDown() {
        renderer.stop();
        renderer.cleanup();
        surface.release();
        texture.release();
        ObservingCodec.info = previousInfo;
        ObservingCodec.observed = null;
        ObservingCodec.duringConfigure = null;
        ObservingCodec.queuedFlags.clear();
        ObservingCodec.queuedMaxWidths.clear();
        MediaCodecHelper.forgetCodecList();
        activity.close();
    }

    @Test
    public void theUiReadsTheLimitsWhileAResetHoldsTheDecoder() throws Exception {
        if (renderer.setup(MoonBridge.VIDEO_FORMAT_H265, 1920, 1080, 60) != 0) fail("setup failed");
        int[] limits = renderer.getReconfigureLimits();
        assertNotNull("this decoder can change size in place", limits);

        // A reset (an HDR change's restart, or one that hangs while the stream ends) holds the monitor.
        Object monitor = ReflectionHelpers.getField(renderer, "codecRecoveryMonitor");
        CountDownLatch held = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        ExecutorService threads = Executors.newFixedThreadPool(2);
        try {
            threads.submit(() -> {
                synchronized (monitor) {
                    held.countDown();
                    release.await();
                }
                return null;
            });
            org.junit.Assert.assertTrue("the reset holds the monitor", held.await(5, TimeUnit.SECONDS));
            Future<int[]> ui = threads.submit(renderer::getReconfigureLimits);
            int[] read;
            try {
                read = ui.get(2, TimeUnit.SECONDS);
            } catch (TimeoutException e) {
                throw new AssertionError("The UI waited for the reset's monitor");
            }
            assertArrayEquals(limits, read);

            // A turn, fold or automatic quality step asks the decoder for a new size and frame
            // rate: the UI is answered at once and the decoder takes the change on its next frame.
            Future<Boolean> resize = threads.submit(() -> renderer.prepareForReconfigure(2560, 1440));
            try {
                org.junit.Assert.assertTrue(resize.get(2, TimeUnit.SECONDS));
            } catch (TimeoutException e) {
                throw new AssertionError("The UI waited for the reset's monitor to prepare a new size");
            }
            Future<?> rate = threads.submit(() -> renderer.setStreamFrameRateMilliHz(120_000));
            try {
                rate.get(2, TimeUnit.SECONDS);
            } catch (TimeoutException e) {
                throw new AssertionError("The UI waited for the reset's monitor to set a new frame rate");
            }
            org.junit.Assert.assertFalse("a size the decoder cannot take is refused at once",
                    threads.submit(() -> renderer.prepareForReconfigure(8192, 8192)).get(2, TimeUnit.SECONDS));
            org.junit.Assert.assertEquals(120_000, renderer.getStreamFrameRateMilliHz());

            // The reset ends; the next frame applies both under the monitor, and the restart the
            // larger size needs runs before that frame is queued, which then decodes.
            release.countDown();
            quiesceRenderThreads();
            ObservingCodec.observed = renderer;
            org.junit.Assert.assertEquals(MoonBridge.DR_OK, submit(1, MoonBridge.BUFFER_TYPE_PICDATA));
            org.junit.Assert.assertFalse("the frame was queued", ObservingCodec.queuedMaxWidths.isEmpty());
            org.junit.Assert.assertEquals("the new maximum was in place when the frame was queued", 2560,
                    (int) ObservingCodec.queuedMaxWidths.get(ObservingCodec.queuedMaxWidths.size() - 1));
            org.junit.Assert.assertEquals(2560, (int) ReflectionHelpers.getField(renderer, "expectedWidth"));
            org.junit.Assert.assertEquals(1440, (int) ReflectionHelpers.getField(renderer, "expectedHeight"));
            org.junit.Assert.assertTrue(ReflectionHelpers.<Boolean>getField(renderer, "reconfigureRequested"));
            org.junit.Assert.assertEquals(1440, (int) ReflectionHelpers.getField(renderer, "maxDecodeHeight"));
            MediaFormat configured = ReflectionHelpers.getField(renderer, "configuredFormat");
            org.junit.Assert.assertEquals(2560, configured.getInteger(MediaFormat.KEY_MAX_WIDTH));
            org.junit.Assert.assertEquals(120, configured.getInteger(MediaFormat.KEY_FRAME_RATE));

            // Once the stream is ending, it offers no size change, still without waiting.
            renderer.prepareForStop();
            assertNull(threads.submit(renderer::getReconfigureLimits).get(2, TimeUnit.SECONDS));
        } finally {
            release.countDown();
            threads.shutdown();
            threads.awaitTermination(5, TimeUnit.SECONDS);
        }
    }

    // As the render and Choreographer threads do when a recovery is due, so the input thread runs it.
    private void quiesceRenderThreads() {
        ReflectionHelpers.setField(renderer, "codecRecoveryThreadQuiescedFlags",
                (int) ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_RENDER_THREAD"));
    }

    private int submit(int frame, int type) {
        return renderer.submitDecodeUnit(new byte[64], 64, type, frame, MoonBridge.FRAME_TYPE_IDR, (char) 0,
                frame * 16_000L, frame * 16_000L, frame * 16_000_000L, frame * 16_000L);
    }

    private int submitIdr(int frame) {
        for (int type : new int[] {MoonBridge.BUFFER_TYPE_VPS, MoonBridge.BUFFER_TYPE_SPS, MoonBridge.BUFFER_TYPE_PPS}) {
            int result = submit(frame, type);
            if (result != MoonBridge.DR_OK) return result;
        }
        return submit(frame, MoonBridge.BUFFER_TYPE_PICDATA);
    }

    private static MediaCodecInfo decoder(String name, int width, int height) {
        MediaFormat capabilities = MediaFormat.createVideoFormat("video/hevc", width, height);
        capabilities.setFeatureEnabled(CodecCapabilities.FEATURE_AdaptivePlayback, true);
        MediaCodecInfo.CodecProfileLevel level = new MediaCodecInfo.CodecProfileLevel();
        level.profile = MediaCodecInfo.CodecProfileLevel.HEVCProfileMain;
        level.level = MediaCodecInfo.CodecProfileLevel.HEVCMainTierLevel51;
        return MediaCodecInfoBuilder.newBuilder().setName(name).setIsHardwareAccelerated(true)
                .setCapabilities(CodecCapabilitiesBuilder.newBuilder().setMediaFormat(capabilities)
                        .setProfileLevels(new MediaCodecInfo.CodecProfileLevel[] {level})
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
    }

    // A still screen: no frame between the request and the PC's answer, whose first unit is the
    // new IDR's VPS. The restart runs before that IDR's parameter sets are kept, so it decodes.
    @Test
    public void theFirstIdrAtTheNewSizeDecodesOnAStillScreen() {
        if (renderer.setup(MoonBridge.VIDEO_FORMAT_H265, 1920, 1080, 60) != 0) fail("setup failed");
        org.junit.Assert.assertEquals(MoonBridge.DR_OK, submitIdr(1));
        org.junit.Assert.assertTrue(renderer.prepareForReconfigure(2560, 1440));
        quiesceRenderThreads();
        ObservingCodec.observed = renderer;
        ObservingCodec.queuedFlags.clear();
        org.junit.Assert.assertEquals("no second IDR is asked for", MoonBridge.DR_OK, submitIdr(2));
        org.junit.Assert.assertEquals("the parameter sets, then the picture",
                java.util.Arrays.asList(android.media.MediaCodec.BUFFER_FLAG_CODEC_CONFIG, android.media.MediaCodec.BUFFER_FLAG_KEY_FRAME),
                ObservingCodec.queuedFlags);
        org.junit.Assert.assertEquals(2560, (int) ObservingCodec.queuedMaxWidths.get(ObservingCodec.queuedMaxWidths.size() - 1));
    }

    // A request the old decoder took, then a rebuild onto a decoder that cannot take that size:
    // the request is dropped, not configured into the new decoder.
    @Test
    public void aRequestTheRebuiltDecoderCannotTakeIsDropped() {
        if (renderer.setup(MoonBridge.VIDEO_FORMAT_H265, 1920, 1080, 60) != 0) fail("setup failed");
        org.junit.Assert.assertTrue(renderer.prepareForReconfigure(2560, 1440));
        ObservingCodec.info = decoder("c2.acme.hevc.decoder", 1920, 1080);
        Object steps = ReflectionHelpers.getField(renderer, "recoveryCodec");
        ReflectionHelpers.callInstanceMethod(steps, "rebuild", ReflectionHelpers.ClassParameter.from(boolean.class, false));
        int maxWidth = ReflectionHelpers.getField(renderer, "maxDecodeWidth");
        org.junit.Assert.assertTrue(maxWidth < 2560);
        quiesceRenderThreads();
        submit(1, MoonBridge.BUFFER_TYPE_PICDATA);
        org.junit.Assert.assertEquals("the maximum stays the new decoder's", maxWidth,
                (int) ReflectionHelpers.getField(renderer, "maxDecodeWidth"));
        org.junit.Assert.assertFalse(ReflectionHelpers.<Boolean>getField(renderer, "reconfigureRequested"));
        org.junit.Assert.assertEquals(1920, (int) ReflectionHelpers.getField(renderer, "expectedWidth"));
    }

    // A request the PC got (A), then one that could not be sent (B): the decoder keeps A's maximum.
    @Test
    public void aCancelledRequestGivesBackTheOneItReplaced() {
        if (renderer.setup(MoonBridge.VIDEO_FORMAT_H265, 1920, 1080, 60) != 0) fail("setup failed");
        org.junit.Assert.assertTrue(renderer.prepareForReconfigure(2560, 1440));
        org.junit.Assert.assertTrue(renderer.prepareForReconfigure(1920, 1080));
        renderer.cancelReconfigure();
        quiesceRenderThreads();
        submit(1, MoonBridge.BUFFER_TYPE_PICDATA);
        org.junit.Assert.assertEquals(2560, (int) ReflectionHelpers.getField(renderer, "maxDecodeWidth"));
        org.junit.Assert.assertEquals(2560, (int) ReflectionHelpers.getField(renderer, "expectedWidth"));
    }

    // A rebuild sets the pending state, its successful configure try clears it and tells the
    // listener once, after the try is complete; a setup or a recovery's configure tells it nothing.
    @Test
    public void onlyARebuildsCompletedConfigureTellsTheListener() {
        java.util.List<Object> told = new java.util.ArrayList<>();
        renderer.setReconfigureLimitsListener(() -> told.add(ReflectionHelpers.getField(renderer, "activeDecoderName")));
        if (renderer.setup(MoonBridge.VIDEO_FORMAT_H265, 1920, 1080, 60) != 0) fail("setup failed");
        Object steps = ReflectionHelpers.getField(renderer, "recoveryCodec");
        ReflectionHelpers.callInstanceMethod(steps, "configure");
        org.junit.Assert.assertTrue(told.isEmpty());
        org.junit.Assert.assertFalse(renderer.reconfigureLimitsPending());

        ReflectionHelpers.setField(renderer, "activeDecoderName", null);
        ObservingCodec.observed = renderer;
        ReflectionHelpers.callInstanceMethod(steps, "rebuild", ReflectionHelpers.ClassParameter.from(boolean.class, false));
        org.junit.Assert.assertTrue("pending while the rebuild configures", ObservingCodec.pendingWhileConfiguring);
        org.junit.Assert.assertFalse(renderer.reconfigureLimitsPending());
        org.junit.Assert.assertEquals("told once, after the configure try finished",
                java.util.Collections.singletonList("c2.acme.hevc.decoder"), told);
    }

    // The PC was never asked (the request could not be sent): the decoder keeps its size and does not restart.
    @Test
    public void aCancelledRequestIsNotApplied() {
        if (renderer.setup(MoonBridge.VIDEO_FORMAT_H265, 1920, 1080, 60) != 0) fail("setup failed");
        org.junit.Assert.assertTrue(renderer.prepareForReconfigure(2560, 1440));
        ReflectionHelpers.callInstanceMethod(renderer, "cancelReconfigure");
        quiesceRenderThreads();
        org.junit.Assert.assertEquals(MoonBridge.DR_OK, submit(1, MoonBridge.BUFFER_TYPE_PICDATA));
        org.junit.Assert.assertEquals(1920, (int) ReflectionHelpers.getField(renderer, "maxDecodeWidth"));
        org.junit.Assert.assertEquals(1920, (int) ReflectionHelpers.getField(renderer, "expectedWidth"));
        org.junit.Assert.assertFalse(ReflectionHelpers.<Boolean>getField(renderer, "reconfigureRequested"));
    }

    @Test
    public void aRebuildOffersNoSizeChangeUntilItsNewDecoderIsConfigured() {
        if (renderer.setup(MoonBridge.VIDEO_FORMAT_H265, 1920, 1080, 60) != 0) fail("setup failed");
        assertNotNull(renderer.getReconfigureLimits());
        // The rebuild after a codec failure may move to another decoder: while it sets that one
        // up, the old decoder's limits must not be offered.
        ObservingCodec.observed = renderer;
        ObservingCodec.configures = 0;
        try {
            Object steps = ReflectionHelpers.getField(renderer, "recoveryCodec");
            ReflectionHelpers.callInstanceMethod(steps, "rebuild", ReflectionHelpers.ClassParameter.from(boolean.class, false));
        } finally {
            ObservingCodec.observed = null;
        }
        org.junit.Assert.assertEquals(1, ObservingCodec.configures);
        assertNull("limits offered during the rebuild", ObservingCodec.seen);
        assertNotNull("and offered again once it is configured", renderer.getReconfigureLimits());
    }

    // Test 7: no rebuild starts or ends while a UI action runs through runUnlessRebuilding. A rebuild
    // that begins on its own thread during an action is marked only once the action has ended, and an
    // action asked for while a rebuild is under way does not run in between its start and its end.
    @Test
    public void noRebuildStartsOrEndsWhileAnActionRuns() throws Exception {
        if (renderer.setup(MoonBridge.VIDEO_FORMAT_H265, 1920, 1080, 60) != 0) fail("setup failed");
        Object steps = ReflectionHelpers.getField(renderer, "recoveryCodec");
        java.util.List<String> told = new java.util.concurrent.CopyOnWriteArrayList<>();
        renderer.setReconfigureLimitsListener(() -> told.add("rebuilt"));
        Object monitor = ReflectionHelpers.getField(renderer, "codecRecoveryMonitor");
        CountDownLatch go = new CountDownLatch(1);
        Thread decoder = new Thread(() -> {
            try {
                go.await();
            } catch (InterruptedException e) {
                return;
            }
            // As the decoder does: the rebuild runs under the recovery monitor, then takes the gate.
            synchronized (monitor) {
                ReflectionHelpers.callInstanceMethod(steps, "rebuild", ReflectionHelpers.ClassParameter.from(boolean.class, false));
            }
        });
        decoder.start();
        boolean[] seenInAction = new boolean[2];
        org.junit.Assert.assertTrue(renderer.runUnlessRebuilding(() -> {
            go.countDown();
            // Released from inside the action: the rebuild waits at the gate until the action ends.
            long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
            while (decoder.getState() != Thread.State.BLOCKED && decoder.isAlive() && System.nanoTime() < deadline) {
                Thread.yield();
            }
            seenInAction[0] = decoder.getState() == Thread.State.BLOCKED;
            seenInAction[1] = renderer.reconfigureLimitsPending() || renderer.getReconfigureLimits() == null;
        }));
        decoder.join(TimeUnit.SECONDS.toMillis(5));
        org.junit.Assert.assertFalse(decoder.isAlive());
        org.junit.Assert.assertTrue("the rebuild waited for the action", seenInAction[0]);
        org.junit.Assert.assertFalse("no rebuild marked while the action ran", seenInAction[1]);
        org.junit.Assert.assertEquals(java.util.Collections.singletonList("rebuilt"), told);

        // During a rebuild's configure (its start marked, its end not yet) an action is refused, and
        // runs once the rebuild is over.
        boolean[] ranDuring = {false};
        boolean[] answered = {true};
        ObservingCodec.duringConfigure = () -> answered[0] = renderer.runUnlessRebuilding(() -> ranDuring[0] = true);
        ReflectionHelpers.callInstanceMethod(steps, "rebuild", ReflectionHelpers.ClassParameter.from(boolean.class, false));
        ObservingCodec.duringConfigure = null;
        org.junit.Assert.assertFalse(answered[0]);
        org.junit.Assert.assertFalse(ranDuring[0]);
        org.junit.Assert.assertTrue(renderer.runUnlessRebuilding(() -> ranDuring[0] = true));
        org.junit.Assert.assertTrue(ranDuring[0]);
    }

    /** Records what the UI would read while the renderer configures a decoder. */
    @org.robolectric.annotation.Implements(android.media.MediaCodec.class)
    public static class ObservingCodec extends KeyframeSafetyNetWiringTest.QueueingCodec {
        static volatile MediaCodecDecoderRenderer observed;
        static volatile int configures;
        static volatile int[] seen;
        static volatile boolean pendingWhileConfiguring;
        static volatile Runnable duringConfigure;
        /** For each buffer queued while observing: its flags, and the renderer's decode maximum then. */
        static final java.util.List<Integer> queuedFlags = new java.util.concurrent.CopyOnWriteArrayList<>();
        static final java.util.List<Integer> queuedMaxWidths = new java.util.concurrent.CopyOnWriteArrayList<>();
        @Override @org.robolectric.annotation.Implementation
        public void configure(MediaFormat format, Surface surface, android.media.MediaCrypto crypto, int flags) {
            MediaCodecDecoderRenderer renderer = observed;
            if (renderer != null) {
                configures++;
                seen = renderer.getReconfigureLimits();
                pendingWhileConfiguring = renderer.reconfigureLimitsPending();
            }
            Runnable during = duringConfigure;
            if (during != null) during.run();
        }
        @Override @org.robolectric.annotation.Implementation
        public void queueInputBuffer(int index, int offset, int size, long timeUs, int flags) {
            MediaCodecDecoderRenderer renderer = observed;
            if (renderer != null) {
                queuedFlags.add(flags);
                queuedMaxWidths.add(ReflectionHelpers.<Integer>getField(renderer, "maxDecodeWidth"));
            }
            super.queueInputBuffer(index, offset, size, timeUs, flags);
        }
    }
}
