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

/** The UI reads the sizes a decoder can switch to without waiting for a reset that holds the decoder. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {KeyframeSafetyNetWiringTest.QueueingCodec.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
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
        MediaCodecInfo info = MediaCodecInfoBuilder.newBuilder().setName("c2.acme.hevc.decoder")
                .setIsHardwareAccelerated(true).setCapabilities(CodecCapabilitiesBuilder.newBuilder()
                        .setMediaFormat(capabilities)
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
        ShadowMediaCodecList.addCodec(info);
        previousInfo = KeyframeSafetyNetWiringTest.QueueingCodec.info;
        KeyframeSafetyNetWiringTest.QueueingCodec.info = info;
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
        KeyframeSafetyNetWiringTest.QueueingCodec.info = previousInfo;
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

            // Once the stream is ending, it offers no size change, still without waiting.
            renderer.prepareForStop();
            assertNull(threads.submit(renderer::getReconfigureLimits).get(2, TimeUnit.SECONDS));
        } finally {
            release.countDown();
            threads.shutdown();
            threads.awaitTermination(5, TimeUnit.SECONDS);
        }
    }
}
