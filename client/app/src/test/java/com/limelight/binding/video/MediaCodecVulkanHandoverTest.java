package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaCodec;
import android.media.MediaCrypto;
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
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowBuild;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

/**
 * Regression tests for {@link MediaCodecVulkanHelper#handOverFromVulkan()} (T8-2): when Vulkan
 * fails to stop within the handover timeout, {@link MediaCodecSession#configureAndStartDecoder}
 * must abort cleanly without configuring or starting MediaCodec on the contended surface, and
 * {@link MediaCodecRecovery} must preserve {@code vulkanFailure}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {MediaCodecVulkanHandoverTest.TrackingCodec.class,
                MediaCodecVulkanHandoverTest.StuckVulkanBridge.class,
                com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
public class MediaCodecVulkanHandoverTest {
    @Implements(MediaCodec.class)
    public static class TrackingCodec extends ShadowMediaCodec {
        static final AtomicInteger configureCalls = new AtomicInteger();
        static final AtomicInteger startCalls = new AtomicInteger();

        @Implementation public void configure(MediaFormat format, Surface surface, MediaCrypto crypto, int flags) {
            configureCalls.incrementAndGet();
        }
        @Implementation public void start() {
            startCalls.incrementAndGet();
        }
        @Implementation public void stop() {}
        @Implementation public void reset() {}
        @Implementation public void release() {}
    }

    @Implements(value = VulkanRendererBridge.class, isInAndroidSdk = false)
    public static class StuckVulkanBridge {
        static boolean stopResult = false;
        static final AtomicInteger stopCalls = new AtomicInteger();

        @Implementation public boolean stop() {
            stopCalls.incrementAndGet();
            return stopResult;
        }
    }

    private ActivityController<Activity> activity;
    private SurfaceTexture texture;
    private Surface surface;
    private MediaCodecDecoderRenderer renderer;

    @Before
    public void setUp() throws Exception {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        TrackingCodec.configureCalls.set(0);
        TrackingCodec.startCalls.set(0);
        StuckVulkanBridge.stopCalls.set(0);
        StuckVulkanBridge.stopResult = false;
        activity = Robolectric.buildActivity(Activity.class).create();
        texture = new SurfaceTexture(0);
        surface = new Surface(texture);
        ShadowBuild.setModel("Pixel 8");
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        prefs.bitrate = 20_000;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        renderer.renderTarget = surface;
        renderer.videoFormat = MoonBridge.VIDEO_FORMAT_H265;
        renderer.configuredFormat = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        renderer.videoDecoder = MediaCodec.createByCodecName("c2.android.hevc.decoder");
        renderer.vulkanRenderer = ReflectionHelpers.callConstructor(
                VulkanRendererBridge.class, ClassParameter.from(long.class, 1L));
        ReflectionHelpers.setField(renderer.vulkanHelper, "vulkanHandoverPending", true);
    }

    @After
    public void tearDown() {
        surface.release();
        texture.release();
        activity.close();
    }

    @Test
    public void timedOutVulkanStopAbortsConfigureAndStartAndSetsStopping() {
        assertFalse(renderer.session.configureAndStartDecoder(renderer.configuredFormat));
        assertEquals(1, StuckVulkanBridge.stopCalls.get());
        assertEquals("MediaCodec.configure must not be called when Vulkan stop timed out",
                0, TrackingCodec.configureCalls.get());
        assertEquals("MediaCodec.start must not be called when Vulkan stop timed out",
                0, TrackingCodec.startCalls.get());
        assertTrue("renderer must enter stopping state", renderer.stopping);
        assertFalse("handover flag must be cleared",
                ReflectionHelpers.<Boolean>getField(renderer.vulkanHelper, "vulkanHandoverPending"));
        assertNotNull("vulkanFailure must be recorded", renderer.vulkanHelper.getVulkanFailure());
    }

    @Test
    public void recoveryAbortsCleanlyAndPreservesVulkanFailureOnHandoverTimeout() {
        renderer.codecRecoveryType.set(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART);
        renderer.restartReason = "HDR shown as SDR";
        renderer.codecRecoveryThreadQuiescedFlags =
                MediaCodecDecoderRenderer.CR_FLAG_ALL & ~MediaCodecDecoderRenderer.CR_FLAG_INPUT_THREAD;

        assertTrue(renderer.recovery.doCodecRecoveryIfRequired(MediaCodecDecoderRenderer.CR_FLAG_INPUT_THREAD));
        assertEquals(0, TrackingCodec.configureCalls.get());
        assertEquals(0, TrackingCodec.startCalls.get());
        assertTrue(renderer.stopping);
        assertNotNull("vulkanFailure preserved", renderer.vulkanHelper.getVulkanFailure());
        assertNull("decoderFailure must not overwrite vulkanFailure", renderer.getDecoderFailure());
    }

    @Test
    public void alreadyStoppingRendererAbortsHandoverWithoutSpuriousVulkanFailure() {
        renderer.stopping = true;
        assertFalse(renderer.vulkanHelper.handOverFromVulkan());
        assertEquals(0, StuckVulkanBridge.stopCalls.get());
        assertFalse(ReflectionHelpers.<Boolean>getField(renderer.vulkanHelper, "vulkanHandoverPending"));
        assertNull(renderer.vulkanHelper.getVulkanFailure());
    }
}
