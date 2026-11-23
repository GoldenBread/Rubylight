package com.limelight.binding.video;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.limelight.binding.video.UpscalingPolicy.Mode;
import com.limelight.binding.video.VideoRendererChoice.Reason;
import com.limelight.binding.video.VideoRendererChoice.Renderer;

import org.junit.Test;

public class VideoRendererChoiceTest {
    private static VideoRendererChoice choose(boolean vulkan, boolean pyroWave, int sdk, boolean supported,
                                              boolean packed, Mode mode) {
        return VideoRendererChoice.choose(vulkan, pyroWave, sdk, () -> supported, packed, mode);
    }

    @Test
    public void vulkanOffKeepsTheDirectPathAndNeverProbes() {
        VideoRendererChoice choice = VideoRendererChoice.choose(false, false, 34, () -> {
            throw new AssertionError("must not probe when Vulkan is off");
        }, false, Mode.OFF);
        assertEquals(Renderer.DIRECT, choice.renderer);
        assertEquals(Reason.NONE, choice.reason);
        assertFalse(choice.upscalingOverridden);
    }

    @Test
    public void vulkanOffWithUpscalingUsesGles() {
        VideoRendererChoice choice = choose(false, false, 34, true, false, Mode.FSR1);
        assertEquals(Renderer.GLES, choice.renderer);
        assertEquals(Reason.NONE, choice.reason);
    }

    @Test
    public void vulkanOnWhenSupported() {
        VideoRendererChoice choice = choose(true, false, 29, true, false, Mode.OFF);
        assertEquals(Renderer.VULKAN, choice.renderer);
        assertEquals(Reason.NONE, choice.reason);
        assertFalse(choice.upscalingOverridden);
    }

    @Test
    public void pyroWaveKeepsItsOwnRenderer() {
        VideoRendererChoice choice = choose(true, true, 34, true, false, Mode.OFF);
        assertEquals(Renderer.DIRECT, choice.renderer);
        assertEquals(Reason.PYROWAVE, choice.reason);
    }

    @Test
    public void androidNineAndOlderCannotUseVulkan() {
        VideoRendererChoice choice = choose(true, false, 28, true, false, Mode.OFF);
        assertEquals(Renderer.DIRECT, choice.renderer);
        assertEquals(Reason.API_LEVEL, choice.reason);
    }

    @Test
    public void unsupportedDeviceFallsBackWithAReason() {
        VideoRendererChoice choice = choose(true, false, 34, false, false, Mode.OFF);
        assertEquals(Renderer.DIRECT, choice.renderer);
        assertEquals(Reason.UNSUPPORTED, choice.reason);
    }

    @Test
    public void unsupportedDeviceWithUpscalingStillGetsTheUpscaler() {
        VideoRendererChoice choice = choose(true, false, 34, false, false, Mode.SGSR1);
        assertEquals(Renderer.GLES, choice.renderer);
        assertEquals(Reason.UNSUPPORTED, choice.reason);
    }

    @Test
    public void packedFullChromaUsesGlesAndSaysSo() {
        VideoRendererChoice choice = choose(true, false, 34, true, true, Mode.OFF);
        assertEquals(Renderer.GLES, choice.renderer);
        assertEquals(Reason.PACKED_444, choice.reason);
    }

    @Test
    public void vulkanWinsOverUpscalingAndFlagsIt() {
        VideoRendererChoice choice = choose(true, false, 34, true, false, Mode.FSR1);
        assertEquals(Renderer.VULKAN, choice.renderer);
        assertEquals(Reason.NONE, choice.reason);
        assertTrue(choice.upscalingOverridden);
    }

    @Test
    public void probeOnlyRunsOnceNothingElseRulesVulkanOut() {
        boolean[] probed = {false};
        VideoRendererChoice.choose(true, true, 34, () -> probed[0] = true, false, Mode.OFF);
        VideoRendererChoice.choose(true, false, 28, () -> probed[0] = true, false, Mode.OFF);
        VideoRendererChoice.choose(true, false, 34, () -> probed[0] = true, true, Mode.OFF);
        assertFalse(probed[0]);
    }

    private static VideoRendererChoice warp(boolean vulkan, int sdk, boolean supported, boolean packed, Mode mode,
                                            boolean hdr) {
        return VideoRendererChoice.choose(vulkan, false, sdk, () -> supported, packed, mode, true, hdr);
    }

    @Test
    public void withoutFrameWarpTheChoiceIsTheOneItAlwaysWas() {
        boolean[] bools = {false, true};
        int[] sdks = {28, 29, 34};
        for (boolean vulkan : bools) for (boolean pyroWave : bools) for (int sdk : sdks) for (boolean supported : bools)
            for (boolean packed : bools) for (Mode mode : Mode.values()) for (boolean hdr : bools) {
                VideoRendererChoice plain = choose(vulkan, pyroWave, sdk, supported, packed, mode);
                VideoRendererChoice off = VideoRendererChoice.choose(vulkan, pyroWave, sdk, () -> supported, packed,
                        mode, false, hdr);
                String at = vulkan + " " + pyroWave + " " + sdk + " " + supported + " " + packed + " " + mode + " " + hdr;
                assertEquals(at, plain.renderer, off.renderer);
                assertEquals(at, plain.reason, off.reason);
                assertEquals(at, plain.upscalingOverridden, off.upscalingOverridden);
                assertFalse(at, off.frameWarp);
                assertEquals(at, VideoRendererChoice.FrameWarpReason.NONE, off.frameWarpReason);
            }
    }

    @Test
    public void frameWarpUsesVulkanWhereItRunsEvenUnasked() {
        VideoRendererChoice choice = warp(false, 29, true, false, Mode.OFF, false);
        assertEquals(Renderer.VULKAN, choice.renderer);
        assertTrue(choice.frameWarp);
        assertEquals(Reason.NONE, choice.reason);
        assertFalse(choice.upscalingOverridden);
        VideoRendererChoice hdr = warp(true, 34, true, false, Mode.FSR1, true);
        assertEquals(Renderer.VULKAN, hdr.renderer);
        assertTrue("Vulkan draws HDR with the shift", hdr.frameWarp);
        assertTrue(hdr.upscalingOverridden);
    }

    @Test
    public void frameWarpWithoutVulkanDrawsThroughGlesWithoutUpscaling() {
        VideoRendererChoice old = warp(false, 28, true, false, Mode.SGSR1, false);
        assertEquals(Renderer.GLES, old.renderer);
        assertTrue(old.frameWarp);
        assertTrue("the shift replaces the upscaler", old.upscalingOverridden);
        assertEquals(Reason.NONE, old.reason);
        VideoRendererChoice unsupported = warp(true, 34, false, false, Mode.OFF, false);
        assertEquals(Renderer.GLES, unsupported.renderer);
        assertTrue(unsupported.frameWarp);
        assertFalse(unsupported.upscalingOverridden);
        assertEquals("Vulkan was asked for, so its reason stands", Reason.UNSUPPORTED, unsupported.reason);
    }

    @Test
    public void frameWarpIsOffForHdrWithoutVulkanAndForFullChroma() {
        VideoRendererChoice hdr = warp(false, 34, false, false, Mode.OFF, true);
        assertEquals(Renderer.DIRECT, hdr.renderer);
        assertFalse(hdr.frameWarp);
        assertEquals(VideoRendererChoice.FrameWarpReason.HDR, hdr.frameWarpReason);
        VideoRendererChoice packed = warp(true, 34, true, true, Mode.OFF, false);
        assertEquals(Renderer.GLES, packed.renderer);
        assertEquals(Reason.PACKED_444, packed.reason);
        assertFalse(packed.frameWarp);
        assertEquals(VideoRendererChoice.FrameWarpReason.FULL_CHROMA, packed.frameWarpReason);
    }

    @Test
    public void pyroWaveStreamsDrawFrameWarpInTheirOwnRenderer() {
        VideoRendererChoice choice = VideoRendererChoice.choose(false, true, 34, () -> {
            throw new AssertionError("PyroWave never probes for the MediaCodec renderer");
        }, false, Mode.OFF, true, false);
        assertEquals(Renderer.DIRECT, choice.renderer);
        assertTrue(choice.frameWarp);
        assertEquals(VideoRendererChoice.FrameWarpReason.NONE, choice.frameWarpReason);
        assertFalse(choice.upscalingOverridden);
        // HDR and its upscaling stay: PyroWave's renderer shifts the picture in each of its passes
        VideoRendererChoice asked = VideoRendererChoice.choose(true, true, 34, () -> true, false, Mode.SGSR1, true, true);
        assertEquals("the renderer is the one PyroWave always had", Reason.PYROWAVE, asked.reason);
        assertEquals(Renderer.DIRECT, asked.renderer);
        assertTrue(asked.frameWarp);
        assertFalse(asked.upscalingOverridden);
        assertEquals(VideoRendererChoice.FrameWarpReason.NONE, asked.frameWarpReason);
    }

    @Test
    public void pyroWaveWithoutFrameWarpIsTheChoiceItAlwaysWas() {
        VideoRendererChoice plain = VideoRendererChoice.choose(true, true, 34, () -> true, false, Mode.SGSR1);
        VideoRendererChoice offered = VideoRendererChoice.choose(true, true, 34, () -> true, false, Mode.SGSR1, false, true);
        assertEquals(plain.renderer, offered.renderer);
        assertEquals(plain.reason, offered.reason);
        assertEquals(plain.upscalingOverridden, offered.upscalingOverridden);
        assertFalse(offered.frameWarp);
        assertEquals(VideoRendererChoice.FrameWarpReason.NONE, offered.frameWarpReason);
    }

    @Test
    public void fullRangeNeedsVulkanPicksVulkanUnaskedForMediaCodecSdr() {
        VideoRendererChoice direct = VideoRendererChoice.choose(false, false, 34, () -> true, false,
                Mode.OFF, false, true, true);
        assertEquals(Renderer.VULKAN, direct.renderer);
        assertEquals(Reason.NONE, direct.reason);
        assertFalse(direct.upscalingOverridden);
        assertFalse(direct.frameWarp);

        // Upscaling is flagged as overridden when Vulkan replaces it.
        VideoRendererChoice upscaled = VideoRendererChoice.choose(false, false, 34, () -> true, false,
                Mode.FSR1, false, false, true);
        assertEquals(Renderer.VULKAN, upscaled.renderer);
        assertEquals(Reason.NONE, upscaled.reason);
        assertTrue(upscaled.upscalingOverridden);

        // Unsupported Vulkan or API < 29 keeps the non-Vulkan path with Reason.NONE (user did not ask for Vulkan).
        VideoRendererChoice unsupported = VideoRendererChoice.choose(false, false, 34, () -> false, false,
                Mode.OFF, false, false, true);
        assertEquals(Renderer.DIRECT, unsupported.renderer);
        assertEquals(Reason.NONE, unsupported.reason);

        VideoRendererChoice oldApi = VideoRendererChoice.choose(false, false, 28, () -> {
            throw new AssertionError("must not probe below API 29");
        }, false, Mode.SGSR1, false, false, true);
        assertEquals(Renderer.GLES, oldApi.renderer);
        assertEquals(Reason.NONE, oldApi.reason);

        // PyroWave and packed 4:4:4 keep their own renderers without probing Vulkan.
        VideoRendererChoice pyro = VideoRendererChoice.choose(false, true, 34, () -> {
            throw new AssertionError("PyroWave never probes Vulkan renderer");
        }, false, Mode.OFF, false, false, true);
        assertEquals(Renderer.DIRECT, pyro.renderer);
        assertEquals(Reason.NONE, pyro.reason);

        VideoRendererChoice packed = VideoRendererChoice.choose(false, false, 34, () -> {
            throw new AssertionError("packed 4:4:4 never probes Vulkan renderer");
        }, true, Mode.OFF, false, false, true);
        assertEquals(Renderer.GLES, packed.renderer);
        assertEquals(Reason.NONE, packed.reason);

        // When vulkanPreferred or frameWarp already probed and found Vulkan unsupported, do not probe twice.
        int[] probes = {0};
        VideoRendererChoice preferredUnsupported = VideoRendererChoice.choose(true, false, 34, () -> {
            probes[0]++;
            return false;
        }, false, Mode.OFF, false, false, true);
        assertEquals(Renderer.DIRECT, preferredUnsupported.renderer);
        assertEquals(Reason.UNSUPPORTED, preferredUnsupported.reason);
        assertEquals(1, probes[0]);
    }
}
