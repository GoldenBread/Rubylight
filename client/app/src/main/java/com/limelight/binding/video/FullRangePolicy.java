package com.limelight.binding.video;

import android.media.MediaFormat;

import com.limelight.nvstream.jni.MoonBridge;

import java.util.Locale;
import java.util.function.BooleanSupplier;

/**
 * Full-range SDR color handling on MediaCodec, apart from MediaCodec itself so each rule can be
 * unit tested.
 *
 * <p>The host encodes full-range YUV (0..255 / 0..1023) and signals {@code video_full_range_flag=1}
 * whenever Full color range is enabled. On direct SurfaceView output, however, black levels still
 * crush or wash out if (1) the hardware decoder ignores {@link MediaFormat#KEY_COLOR_RANGE} and
 * reports limited or unspecified range in its output format (causing Codec2 to stamp a limited-range
 * dataspace onto the surface), or (2) a MediaTek display pipeline ({@code c2.mtk.*} /
 * {@code omx.mtk.*}) mishandles full-range direct YUV/P010 layers on SDR streams even when the
 * codec decodes full-range samples. Presenting those SDR streams through {@link VulkanRendererBridge}
 * applies {@code VK_SAMPLER_YCBCR_RANGE_ITU_FULL} explicitly and outputs an RGB swapchain.
 *
 * <p>With Full color range off (the default), nothing here runs and no Vulkan probe is made. HDR,
 * Dolby Vision, PyroWave and packed 4:4:4 keep their own color paths untouched.
 */
public final class FullRangePolicy {
    private FullRangePolicy() {}

    /** Whether {@code decoderName} is a MediaTek hardware decoder ({@code c2.mtk.*} or {@code omx.mtk.*}). */
    public static boolean isMediaTekDecoder(String decoderName) {
        if (decoderName == null) return false;
        String lower = decoderName.toLowerCase(Locale.ROOT);
        return lower.startsWith("c2.mtk.") || lower.startsWith("omx.mtk.");
    }

    /**
     * Whether a stream should start on the Vulkan renderer unasked for full-range SDR accuracy:
     * only when Full color range is enabled, the stream is SDR (8-bit or 10-bit), and the selected
     * decoder is MediaTek's, whose direct YUV/P010 SurfaceView path mishandles full-range SDR.
     */
    public static boolean needsVulkanUpFront(boolean fullRange, boolean hdr, String decoderName) {
        return fullRange && !hdr && isMediaTekDecoder(decoderName);
    }

    /**
     * Whether an SDR stream running without the Vulkan renderer must switch to Vulkan based on the
     * decoder's reported {@link MediaFormat}, returning a short reason for the log or {@code null}
     * when the current path should be kept.
     */
    public static String outputFallbackReason(boolean fullRange, boolean hdr, boolean dolbyVision,
                                              boolean packed444, boolean pyroWave, String decoderName,
                                              MediaFormat outputFormat) {
        Integer range = outputFormat != null && outputFormat.containsKey(MediaFormat.KEY_COLOR_RANGE) ?
                outputFormat.getInteger(MediaFormat.KEY_COLOR_RANGE) : null;
        boolean hasColorAspects = outputFormat != null &&
                (outputFormat.containsKey(MediaFormat.KEY_COLOR_STANDARD) ||
                        outputFormat.containsKey(MediaFormat.KEY_COLOR_TRANSFER));
        return outputFallbackReason(fullRange, hdr, dolbyVision, packed444, pyroWave, decoderName,
                range, hasColorAspects);
    }

    /**
     * Pure form of {@link #outputFallbackReason(boolean, boolean, boolean, boolean, boolean, String, MediaFormat)}.
     *
     * @param outputColorRange {@link MediaFormat#KEY_COLOR_RANGE} from the output format, or null when absent
     * @param hasColorAspects  whether the output format reported {@code color-standard} or {@code color-transfer}
     */
    public static String outputFallbackReason(boolean fullRange, boolean hdr, boolean dolbyVision,
                                              boolean packed444, boolean pyroWave, String decoderName,
                                              Integer outputColorRange, boolean hasColorAspects) {
        if (!fullRange || hdr || dolbyVision || packed444 || pyroWave) {
            return null;
        }
        if (outputColorRange != null) {
            if (outputColorRange != MediaFormat.COLOR_RANGE_FULL) {
                return "reported " + rangeName(outputColorRange) + " color range for a full-range SDR stream";
            }
        } else if (hasColorAspects) {
            // Codec2 populated colour standard/transfer while leaving range unspecified, which
            // Android's ColorUtils maps to RANGE_LIMITED for BT.709/BT.601 YUV surfaces.
            return "omitted color range (defaults to limited) for a full-range SDR stream";
        }
        if (isMediaTekDecoder(decoderName)) {
            return "uses MediaTek direct output for a full-range SDR stream";
        }
        return null;
    }

    /** Whether the Vulkan renderer can take over on this API level and GPU; probes lazily. */
    public static boolean canUseVulkan(int sdkInt, BooleanSupplier vulkanSupported) {
        return sdkInt >= VideoRendererChoice.MIN_SDK && vulkanSupported.getAsBoolean();
    }

    /** Sets the initial colour keys on a decoder's base {@link MediaFormat}. */
    public static void applyBaseColor(MediaFormat format, int preferredColorRange, int preferredColorSpace,
                                      boolean assumesHdr) {
        format.setInteger(MediaFormat.KEY_COLOR_RANGE,
                preferredColorRange == MoonBridge.COLOR_RANGE_FULL ?
                        MediaFormat.COLOR_RANGE_FULL : MediaFormat.COLOR_RANGE_LIMITED);
        // If the stream is HDR-capable, the decoder detects transitions in colour standard/transfer.
        if (!assumesHdr) {
            format.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
            switch (preferredColorSpace) {
                case MoonBridge.COLORSPACE_REC_601:
                    format.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT601_NTSC);
                    break;
                case MoonBridge.COLORSPACE_REC_709:
                    format.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709);
                    break;
                case MoonBridge.COLORSPACE_REC_2020:
                    format.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT2020);
                    break;
            }
        }
    }

    /** Concise output-format summary for problem-report stream rows, including colour range. */
    public static String outputDescription(MediaFormat format) {
        return outputKey(format, MediaFormat.KEY_WIDTH) + "x" + outputKey(format, MediaFormat.KEY_HEIGHT) +
                " color " + outputKey(format, MediaFormat.KEY_COLOR_FORMAT) +
                " standard " + outputKey(format, MediaFormat.KEY_COLOR_STANDARD) +
                " transfer " + outputKey(format, MediaFormat.KEY_COLOR_TRANSFER) +
                " range " + outputKey(format, MediaFormat.KEY_COLOR_RANGE);
    }

    static String rangeName(Integer colorRange) {
        if (colorRange == null) return "unspecified";
        if (colorRange == MediaFormat.COLOR_RANGE_FULL) return "full (1)";
        if (colorRange == MediaFormat.COLOR_RANGE_LIMITED) return "limited (2)";
        return "unknown (" + colorRange + ")";
    }

    private static String outputKey(MediaFormat format, String key) {
        return format.containsKey(key) ? String.valueOf(format.getInteger(key)) : "?";
    }
}
