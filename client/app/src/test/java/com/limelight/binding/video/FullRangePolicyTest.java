package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;

import com.limelight.LimeLog;
import com.limelight.PcViewLifecycleTest;
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
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.io.IOException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

/** Full-range SDR colour policy and its wiring into MediaCodecDecoderRenderer (issue #153). */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"},
        shadows = {FullRangePolicyTest.FakeVulkanBridge.class, FullRangePolicyTest.FakeCodec.class,
                PcViewLifecycleTest.NoNativeMoonBridge.class})
public class FullRangePolicyTest {
    @Implements(value = VulkanRendererBridge.class, isInAndroidSdk = false)
    public static class FakeVulkanBridge {
        static boolean supported = true;
        static int probes;

        @Implementation public static synchronized boolean isSupported() {
            probes++;
            return supported;
        }
    }

    @Implements(MediaCodec.class)
    public static class FakeCodec extends ShadowMediaCodec {
        static final List<String> calls = new ArrayList<>();
        static MediaFormat output;

        @Implementation public MediaFormat getOutputFormat() {
            return output;
        }

        @Implementation public void stop() {
            calls.add("stop");
        }

        @Implementation public void reset() {
            calls.add("reset");
        }

        @Implementation public void release() {
            calls.add("release");
        }
    }

    @Before
    public void resetState() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        FakeVulkanBridge.supported = true;
        FakeVulkanBridge.probes = 0;
        FakeCodec.calls.clear();
        FakeCodec.output = null;
    }

    @Test
    public void mediaTekFullRangeSdrNeedsVulkanUpFrontWhileDefaultAndHdrDoNot() {
        assertTrue(FullRangePolicy.needsVulkanUpFront(true, false, "c2.mtk.hevc.decoder.lowlatency"));
        assertTrue(FullRangePolicy.needsVulkanUpFront(true, false, "C2.MTK.AVC.DECODER"));
        assertTrue(FullRangePolicy.needsVulkanUpFront(true, false, "omx.mtk.video.decoder.hevc"));
        // Default limited range: untouched.
        assertFalse(FullRangePolicy.needsVulkanUpFront(false, false, "c2.mtk.hevc.decoder.lowlatency"));
        // HDR and Dolby Vision stay on their own paths.
        assertFalse(FullRangePolicy.needsVulkanUpFront(true, true, "c2.mtk.hevc.decoder.lowlatency"));
        assertFalse(FullRangePolicy.needsVulkanUpFront(true, false, "c2.dolby.decoder.hevc"));
        // Other vendors stay on direct output unless their output format disagrees.
        for (String name : new String[] {"c2.qti.hevc.decoder", "c2.exynos.hevc.decoder",
                "c2.amlogic.hevc.decoder", "c2.google.hevc.decoder", null}) {
            assertFalse(String.valueOf(name), FullRangePolicy.needsVulkanUpFront(true, false, name));
        }
    }

    @Test
    public void outputFallbackDetectsLimitedOrUnspecifiedRangeAndMediaTekSdr() {
        // Full range off: never triggers.
        assertNull(FullRangePolicy.outputFallbackReason(false, false, false, false, false,
                "c2.qti.hevc.decoder", MediaFormat.COLOR_RANGE_LIMITED, true));
        // HDR, Dolby Vision, packed 4:4:4 and PyroWave stay on their own paths.
        assertNull(FullRangePolicy.outputFallbackReason(true, true, false, false, false,
                "c2.mtk.hevc.decoder.lowlatency", MediaFormat.COLOR_RANGE_LIMITED, true));
        assertNull(FullRangePolicy.outputFallbackReason(true, false, true, false, false,
                "c2.dolby.decoder.hevc", MediaFormat.COLOR_RANGE_LIMITED, true));
        assertNull(FullRangePolicy.outputFallbackReason(true, false, false, true, false,
                "c2.mtk.hevc.decoder.lowlatency", MediaFormat.COLOR_RANGE_LIMITED, true));
        assertNull(FullRangePolicy.outputFallbackReason(true, false, false, false, true,
                "c2.mtk.hevc.decoder.lowlatency", MediaFormat.COLOR_RANGE_LIMITED, true));

        // Non-MediaTek decoder honoring full range: stays direct.
        assertNull(FullRangePolicy.outputFallbackReason(true, false, false, false, false,
                "c2.qti.hevc.decoder", MediaFormat.COLOR_RANGE_FULL, true));
        // Non-MediaTek decoder with no colour keys at all: gives nothing to object to.
        assertNull(FullRangePolicy.outputFallbackReason(true, false, false, false, false,
                "c2.qti.hevc.decoder", null, false));

        // Any decoder reporting limited range for full-range SDR triggers fallback.
        String limited = FullRangePolicy.outputFallbackReason(true, false, false, false, false,
                "c2.qti.hevc.decoder", MediaFormat.COLOR_RANGE_LIMITED, true);
        assertNotNull(limited);
        assertTrue(limited, limited.contains("limited"));

        // Any decoder reporting standard/transfer while omitting range (defaults to limited in Codec2) triggers fallback.
        String omitted = FullRangePolicy.outputFallbackReason(true, false, false, false, false,
                "c2.exynos.hevc.decoder", null, true);
        assertNotNull(omitted);
        assertTrue(omitted, omitted.contains("omitted"));

        // MediaTek SDR triggers even if the decoder echoed COLOR_RANGE_FULL (e.g. after an HDR -> SDR switch).
        String mtk = FullRangePolicy.outputFallbackReason(true, false, false, false, false,
                "c2.mtk.hevc.decoder.lowlatency", MediaFormat.COLOR_RANGE_FULL, true);
        assertNotNull(mtk);
        assertTrue(mtk, mtk.contains("MediaTek"));
    }

    @Test
    public void canUseVulkanDoesNotProbeBelowApi29() {
        boolean[] probed = {false};
        assertFalse(FullRangePolicy.canUseVulkan(28, () -> probed[0] = true));
        assertFalse(probed[0]);
        assertTrue(FullRangePolicy.canUseVulkan(29, () -> {
            probed[0] = true;
            return true;
        }));
        assertTrue(probed[0]);
    }

    @Test
    public void outputDescriptionIncludesColorRangeOrQuestionMark() {
        MediaFormat full = MediaFormat.createVideoFormat("video/raw", 2944, 1840);
        full.setInteger(MediaFormat.KEY_COLOR_FORMAT, 2130708361);
        full.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709);
        full.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
        full.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_FULL);
        assertEquals("2944x1840 color 2130708361 standard 1 transfer 3 range 1",
                FullRangePolicy.outputDescription(full));

        MediaFormat withoutRange = MediaFormat.createVideoFormat("video/raw", 1920, 1080);
        withoutRange.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709);
        assertEquals("1920x1080 color ? standard 1 transfer ? range ?",
                FullRangePolicy.outputDescription(withoutRange));
    }

    private MediaCodecDecoderRenderer buildRenderer(Activity activity, boolean fullRange, boolean sdr10,
                                                    int videoFormat, String hevcDecoderName) {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        prefs.fullRange = fullRange;
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity, prefs,
                null, 0, false, false, "", null);
        renderer.setSdr10Requested(sdr10);
        ReflectionHelpers.setField(renderer, "videoFormat", videoFormat);
        ReflectionHelpers.setField(renderer, "refreshRate", 144);
        if (hevcDecoderName != null) {
            MediaCodecInfo info = MediaCodecInfoBuilder.newBuilder().setName(hevcDecoderName).build();
            ReflectionHelpers.setField(renderer, "hevcDecoder", info);
            ReflectionHelpers.setField(renderer, "hevcDecoder10", info);
            ReflectionHelpers.setField(renderer, "activeDecoderName", hevcDecoderName);
        }
        return renderer;
    }

    @Test
    public void setupUpscalingPicksVulkanForMediaTekFullRangeTenBitAndEightBitSdr() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            // Lenovo TB375FC scenario: HEVC 10-bit SDR + full range ON + c2.mtk.hevc.decoder.lowlatency
            MediaCodecDecoderRenderer sdr10 = buildRenderer(activity.get(), true, true,
                    MoonBridge.VIDEO_FORMAT_H265_MAIN10, "c2.mtk.hevc.decoder.lowlatency");
            ReflectionHelpers.callInstanceMethod(sdr10, "setupUpscaling");
            assertEquals(true, ReflectionHelpers.getField(sdr10, "wantVulkan"));

            // 8-bit HEVC SDR + full range ON + c2.mtk
            MediaCodecDecoderRenderer sdr8 = buildRenderer(activity.get(), true, false,
                    MoonBridge.VIDEO_FORMAT_H265, "c2.mtk.hevc.decoder.lowlatency");
            ReflectionHelpers.callInstanceMethod(sdr8, "setupUpscaling");
            assertEquals(true, ReflectionHelpers.getField(sdr8, "wantVulkan"));

            // Default (full range OFF): stays direct, never probes Vulkan.
            FakeVulkanBridge.probes = 0;
            MediaCodecDecoderRenderer limited = buildRenderer(activity.get(), false, true,
                    MoonBridge.VIDEO_FORMAT_H265_MAIN10, "c2.mtk.hevc.decoder.lowlatency");
            ReflectionHelpers.callInstanceMethod(limited, "setupUpscaling");
            assertEquals(false, ReflectionHelpers.getField(limited, "wantVulkan"));
            assertEquals(0, FakeVulkanBridge.probes);

            // HDR stream (10-bit without sdr10 -> assumesHdr): stays direct so HDR / Dolby Vision stays intact.
            MediaCodecDecoderRenderer hdr = buildRenderer(activity.get(), true, false,
                    MoonBridge.VIDEO_FORMAT_H265_MAIN10, "c2.mtk.hevc.decoder.lowlatency");
            ReflectionHelpers.callInstanceMethod(hdr, "setupUpscaling");
            assertEquals(false, ReflectionHelpers.getField(hdr, "wantVulkan"));
            assertEquals(0, FakeVulkanBridge.probes);

            // Non-MediaTek decoder starts direct and never probes Vulkan up front.
            MediaCodecDecoderRenderer qti = buildRenderer(activity.get(), true, true,
                    MoonBridge.VIDEO_FORMAT_H265_MAIN10, "c2.qti.hevc.decoder");
            ReflectionHelpers.callInstanceMethod(qti, "setupUpscaling");
            assertEquals(false, ReflectionHelpers.getField(qti, "wantVulkan"));
            assertEquals(0, FakeVulkanBridge.probes);
        }
    }

    @Test
    public void outputFormatWithLimitedOrOmittedRangeSwitchesNonMediaTekStreamToVulkanOnce() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = buildRenderer(activity.get(), true, true,
                    MoonBridge.VIDEO_FORMAT_H265_MAIN10, "c2.qti.hevc.decoder");
            MediaCodec codec = MediaCodec.createDecoderByType("video/avc");
            try {
                ReflectionHelpers.setField(renderer, "videoDecoder", codec);
                MediaFormat limited = MediaFormat.createVideoFormat("video/raw", 2944, 1840);
                limited.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709);
                limited.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
                limited.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED);
                FakeCodec.output = limited;

                ReflectionHelpers.callInstanceMethod(renderer, "onOutputFormatChanged");

                AtomicInteger recovery = ReflectionHelpers.getField(renderer, "codecRecoveryType");
                assertEquals((int) ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class,
                        "CR_RECOVERY_TYPE_RESTART"), recovery.get());
                assertEquals(true, ReflectionHelpers.getField(renderer, "wantVulkan"));
                assertEquals(true, ReflectionHelpers.getField(renderer, "rebuildForVulkan"));
                assertEquals(true, ReflectionHelpers.getField(renderer, "fullRangeFallbackAttempted"));

                // Running a second time (e.g., if Vulkan later declined) never loops.
                recovery.set(ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_RECOVERY_TYPE_NONE"));
                ReflectionHelpers.setField(renderer, "wantVulkan", false);
                ReflectionHelpers.setField(renderer, "rebuildForVulkan", false);
                ReflectionHelpers.callInstanceMethod(renderer, "onOutputFormatChanged");
                assertEquals((int) ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class,
                        "CR_RECOVERY_TYPE_NONE"), recovery.get());
                assertEquals(false, ReflectionHelpers.getField(renderer, "wantVulkan"));
            } finally {
                codec.release();
            }
        }
    }

    @Test
    public void outputFormatHonoringFullRangeOrUnsupportedVulkanOrHandedBackDoesNotRestart() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            // Case 1: decoder reports COLOR_RANGE_FULL -> stays direct, no restart.
            MediaCodecDecoderRenderer ok = buildRenderer(activity.get(), true, false,
                    MoonBridge.VIDEO_FORMAT_H265, "c2.qti.hevc.decoder");
            MediaCodec codec1 = MediaCodec.createDecoderByType("video/avc");
            try {
                ReflectionHelpers.setField(ok, "videoDecoder", codec1);
                MediaFormat full = MediaFormat.createVideoFormat("video/raw", 1920, 1080);
                full.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709);
                full.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
                full.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_FULL);
                FakeCodec.output = full;
                ReflectionHelpers.callInstanceMethod(ok, "onOutputFormatChanged");
                assertEquals(false, ReflectionHelpers.getField(ok, "wantVulkan"));
                assertEquals(0, ((AtomicInteger) ReflectionHelpers.getField(ok, "codecRecoveryType")).get());
            } finally {
                codec1.release();
            }

            // Case 2: decoder omits range while Vulkan is unsupported -> warns, no restart, never probes again.
            FakeVulkanBridge.supported = false;
            MediaCodecDecoderRenderer noVk = buildRenderer(activity.get(), true, false,
                    MoonBridge.VIDEO_FORMAT_H265, "c2.qti.hevc.decoder");
            MediaCodec codec2 = MediaCodec.createDecoderByType("video/avc");
            try {
                ReflectionHelpers.setField(noVk, "videoDecoder", codec2);
                MediaFormat omitted = MediaFormat.createVideoFormat("video/raw", 1920, 1080);
                omitted.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709);
                omitted.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
                FakeCodec.output = omitted;
                ReflectionHelpers.callInstanceMethod(noVk, "onOutputFormatChanged");
                assertEquals(false, ReflectionHelpers.getField(noVk, "wantVulkan"));
                assertEquals(true, ReflectionHelpers.getField(noVk, "fullRangeFallbackAttempted"));
                assertEquals(0, ((AtomicInteger) ReflectionHelpers.getField(noVk, "codecRecoveryType")).get());
                assertTrue(LimeLog.getRedactedLog().contains("Vulkan renderer is unavailable"));
            } finally {
                codec2.release();
            }

            // Case 3: stream already handed back from Vulkan (tone-mapped HDR) -> never ping-pongs back.
            FakeVulkanBridge.supported = true;
            MediaCodecDecoderRenderer handedBack = buildRenderer(activity.get(), true, false,
                    MoonBridge.VIDEO_FORMAT_H265, "c2.qti.hevc.decoder");
            ReflectionHelpers.setField(handedBack, "vulkanHandedBack", true);
            MediaCodec codec3 = MediaCodec.createDecoderByType("video/avc");
            try {
                ReflectionHelpers.setField(handedBack, "videoDecoder", codec3);
                MediaFormat limited = MediaFormat.createVideoFormat("video/raw", 1920, 1080);
                limited.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED);
                FakeCodec.output = limited;
                ReflectionHelpers.callInstanceMethod(handedBack, "onOutputFormatChanged");
                assertEquals(false, ReflectionHelpers.getField(handedBack, "wantVulkan"));
                assertEquals(0, ((AtomicInteger) ReflectionHelpers.getField(handedBack, "codecRecoveryType")).get());
            } finally {
                codec3.release();
            }
        }
    }

    @Test
    public void hdrOutputDoesNotConsumeFallbackBeforeLaterSwitchToSdr() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            // Starts as HDR (10-bit without sdr10 -> assumesHdr() is true).
            MediaCodecDecoderRenderer renderer = buildRenderer(activity.get(), true, false,
                    MoonBridge.VIDEO_FORMAT_H265_MAIN10, "c2.mtk.hevc.decoder.lowlatency");
            MediaCodec codec = MediaCodec.createDecoderByType("video/avc");
            try {
                ReflectionHelpers.setField(renderer, "videoDecoder", codec);
                MediaFormat fmt = MediaFormat.createVideoFormat("video/raw", 2944, 1840);
                fmt.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT2020);
                fmt.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_ST2084);
                fmt.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED);
                FakeCodec.output = fmt;

                // While HDR, fallback does not fire and stays armed.
                ReflectionHelpers.callInstanceMethod(renderer, "onOutputFormatChanged");
                assertEquals(false, ReflectionHelpers.getField(renderer, "fullRangeFallbackAttempted"));
                assertEquals(false, ReflectionHelpers.getField(renderer, "wantVulkan"));

                // Once the PC reports SDR, the next output format change switches to Vulkan.
                ReflectionHelpers.setField(renderer, "currentHdrMode", Boolean.FALSE);
                fmt.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709);
                fmt.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
                fmt.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_FULL);
                ReflectionHelpers.callInstanceMethod(renderer, "onOutputFormatChanged");
                assertEquals(true, ReflectionHelpers.getField(renderer, "fullRangeFallbackAttempted"));
                assertEquals(true, ReflectionHelpers.getField(renderer, "wantVulkan"));
                assertEquals(true, ReflectionHelpers.getField(renderer, "rebuildForVulkan"));
            } finally {
                codec.release();
            }
        }
    }

    @Test
    public void rebuildForVulkanReleasesOldCodecBeforeReinitializingAndClearsFlag() throws IOException {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = buildRenderer(activity.get(), true, false,
                    MoonBridge.VIDEO_FORMAT_H265, null);
            MediaCodec codec = MediaCodec.createDecoderByType("video/avc");
            ReflectionHelpers.setField(renderer, "videoDecoder", codec);
            ReflectionHelpers.setField(renderer, "wantVulkan", true);
            ReflectionHelpers.setField(renderer, "rebuildForVulkan", true);
            AtomicInteger recovery = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            recovery.set(ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_RECOVERY_TYPE_RESTART"));
            ReflectionHelpers.setField(renderer, "codecRecoveryThreadQuiescedFlags",
                    (int) ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_INPUT_THREAD"));

            // No HEVC decoder configured so initializeDecoder fails cleanly after release(), proving
            // rebuild() released the old codec directly instead of calling stop() or reset().
            ReflectionHelpers.callInstanceMethod(renderer, "doCodecRecoveryIfRequired",
                    ClassParameter.from(int.class,
                            (int) ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_RENDER_THREAD")));
            assertEquals(Collections.singletonList("release"), FakeCodec.calls);
            assertNotNull(renderer.getDecoderFailure());
        }
    }
}
