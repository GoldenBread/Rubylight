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
            held.await(5, TimeUnit.SECONDS);
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

            // The reset ends; the next frame applies both under the monitor, before it is queued.
            // (The decoder thread then waits in the restart for render threads this test does not run;
            // the stop below lets it go.)
            release.countDown();
            Thread input = new Thread(() -> renderer.submitDecodeUnit(new byte[64], 64, MoonBridge.BUFFER_TYPE_PICDATA,
                    1, MoonBridge.FRAME_TYPE_IDR, (char) 0, 16_000L, 16_000L, 16_000_000L, 16_000L));
            input.setDaemon(true);
            input.start();
            long deadline = System.nanoTime() + 5_000_000_000L;
            while (!ReflectionHelpers.<Boolean>getField(renderer, "reconfigureRequested") && System.nanoTime() < deadline) {
                Thread.sleep(5);
            }
            org.junit.Assert.assertEquals(2560, (int) ReflectionHelpers.getField(renderer, "expectedWidth"));
            org.junit.Assert.assertEquals(1440, (int) ReflectionHelpers.getField(renderer, "expectedHeight"));
            org.junit.Assert.assertTrue(ReflectionHelpers.<Boolean>getField(renderer, "reconfigureRequested"));
            org.junit.Assert.assertEquals(2560, (int) ReflectionHelpers.getField(renderer, "maxDecodeWidth"));
            org.junit.Assert.assertEquals(1440, (int) ReflectionHelpers.getField(renderer, "maxDecodeHeight"));
            MediaFormat configured = ReflectionHelpers.getField(renderer, "configuredFormat");
            org.junit.Assert.assertEquals(2560, configured.getInteger(MediaFormat.KEY_MAX_WIDTH));
            org.junit.Assert.assertEquals(120, configured.getInteger(MediaFormat.KEY_FRAME_RATE));
            org.junit.Assert.assertEquals("size change", ReflectionHelpers.getField(renderer, "restartReason"));

            // Once the stream is ending, it offers no size change, still without waiting.
            renderer.prepareForStop();
            assertNull(threads.submit(renderer::getReconfigureLimits).get(2, TimeUnit.SECONDS));
            input.join(5000);
            org.junit.Assert.assertFalse("the decoder thread left the restart once the stream stopped", input.isAlive());
        } finally {
            release.countDown();
            threads.shutdown();
            threads.awaitTermination(5, TimeUnit.SECONDS);
        }
    }

    /** Records what the UI would read while the renderer configures a decoder. */
    @org.robolectric.annotation.Implements(android.media.MediaCodec.class)
    public static class ObservingCodec extends KeyframeSafetyNetWiringTest.QueueingCodec {
        static volatile MediaCodecDecoderRenderer observed;
        static volatile int configures;
        static volatile int[] seen;
        @Override @org.robolectric.annotation.Implementation
        public void configure(MediaFormat format, Surface surface, android.media.MediaCrypto crypto, int flags) {
            MediaCodecDecoderRenderer renderer = observed;
            if (renderer != null) {
                configures++;
                seen = renderer.getReconfigureLimits();
            }
        }
    }
}
