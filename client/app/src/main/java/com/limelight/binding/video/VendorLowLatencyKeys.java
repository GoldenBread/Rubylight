package com.limelight.binding.video;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * Vendor decoder keys that cut decode latency, matched against what a codec actually exposes.
 * Android 12+ lists a codec's vendor parameters, so only keys the decoder declares are sent.
 */
final class VendorLowLatencyKeys {
    // Known keys and the value that enables the low-latency behaviour.
    static final Map<String, Integer> KNOWN;
    // Keys sent to NVIDIA decoders by name, declared or not.
    static final Map<String, Integer> TEGRA;
    // The whole MediaTek preset, set on MediaTek decoders by name. Experimental: no MediaTek device here.
    static final Map<String, Integer> MEDIATEK;
    // The MediaTek preset for the first configure try: the same keys with shorter queues and waits, no preload.
    static final Map<String, Integer> MEDIATEK_SHORT;
    // The MediaTek preset for AV1: the low-latency, boost and clock switches only, no queue,
    // wait or preload keys.
    static final Map<String, Integer> MEDIATEK_AV1;
    // The Samsung Exynos preset, set on Exynos decoders by name. Experimental: no Exynos device here.
    static final Map<String, Integer> EXYNOS;
    // Google Codec2 frame rate key, read by Android 16 (API 36) and newer, the level Moonlight X gates it on. Experimental: no Tensor device here.
    static final String GOOGLE_FRAME_RATE = "vendor.google.frame-rate.value";
    // MediaTek switches whose presence on a decoder shows it handles low-latency streams (reference
    // frame invalidation); they are detected for HEVC RFI only, never set. Delay and timeout
    // knobs (buffer.fetch.timeout.ms, the output delay frame counts) are no evidence of that.
    static final String[] MEDIATEK_DETECT_ONLY = {
            "vendor.mtk.vdec.low-latency.strict.enable", "vendor.mtk.vdec.early.output.enable",
            "vendor.mtk.vdec.no.reorder.enable", "vendor.mtk.vdec.disable.reorder",
            "vendor.mtk.vdec.no.clock.adjust"};
    // Snapdragon keys from Moonlight V+ that are sent only when the decoder declares them
    // (Android 12 and newer), never by name, and on the first configure try only: no Snapdragon
    // here has confirmed them, and a Snapdragon key the decoder accepted has hung the Retroid
    // before (picture-order = 0).
    static final Collection<String> QUALCOMM_DECLARED_ONLY = Collections.unmodifiableList(Arrays.asList(
            "vendor.qti-ext-dec-info-misr.disable", "vendor.qti-ext-dec-instant-decode.enable",
            "vendor.qti-ext-dec-error-correction.conceal"));
    // Qualcomm's extension keys apply only to its own decoders, matched by codec-name prefix. A
    // decoder of another vendor can list them (the Dolby Vision decoder on the Lenovo Legion Tab
    // Y700 declares picture-order and the output-fence pair) but does not implement them: those
    // keys there broke the picture into white line segments (issue 129).
    private static final String QUALCOMM_EXT_PREFIX = "vendor.qti-ext";
    // The prefixes of MediaCodecHelper.qualcommDecoderPrefixes, repeated here so the filter is
    // independent of that class's initialization.
    private static final String[] QUALCOMM_DECODER_PREFIXES = {"omx.qcom", "c2.qti", "c2.qcom"};

    static {
        Map<String, Integer> known = new LinkedHashMap<>();
        // Qualcomm (OMX and Codec2): low-latency mode, decode-order output.
        known.put("vendor.qti-ext-dec-low-latency.enable", 1);
        known.put("vendor.qti-ext-dec-picture-order.enable", 1);
        // Snapdragon 8 Gen 2 and newer hold output behind a fence; a software fence releases it sooner.
        known.put("vendor.qti-ext-output-sw-fence-enable.value", 1);
        known.put("vendor.qti-ext-output-fence.enable", 1);
        known.put("vendor.qti-ext-output-fence.fence_type", 1);
        // Moonlight V+ (qiin2333/moonlight-vplus, GPL-3.0) also asks Snapdragon decoders to skip
        // the per-frame MISR checksum, to decode without output batching and to conceal a lost
        // slice instead of waiting for an IDR. Unlike V+ they go only to decoders that declare
        // them (QUALCOMM_DECLARED_ONLY), so they are never sent by name.
        known.put("vendor.qti-ext-dec-info-misr.disable", 1);
        known.put("vendor.qti-ext-dec-instant-decode.enable", 1);
        known.put("vendor.qti-ext-dec-error-correction.conceal", 1);
        // Samsung Exynos and Google Tensor.
        known.put("vendor.rtc-ext-dec-low-latency.enable", 1);
        // HiSilicon Kirin.
        known.put("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-req", 1);
        // Amlogic and other generic vendor stacks.
        known.put("vendor.low-latency.enable", 1);
        // MediaTek Codec2.
        known.put("vendor.mtk.vdec.low-latency.mode", 1);
        known.put("vendor.mtk.vdec.low-latency.mode.value", 1);
        // Boost and clock level 1, the mild one: Artemis and the derflacco fork both send 1 (the
        // fork withdrew its stronger profile, which used 2).
        known.put("vendor.mtk.vdec.cpu.boost.mode", 1);
        known.put("vendor.mtk.vdec.cpu.boost.mode.value", 1);
        // Keep the clocks up and let the app pace output; both from Artemis's MediaTek preset.
        known.put("vendor.mtk.vdec.dvfs.mode", 1);
        known.put("vendor.mtk.vdec.dvfs.level", 1);
        known.put("vendor.mtk.vdec.disable-idle", 1);
        known.put("vendor.mtk.vdec.vsync.adjust.enable", 0);
        // NVIDIA Tegra.
        known.put("vendor.nvidia.disable-output-reorder", 1);
        known.put("vendor.nvidia.disable-output-reorder.value", 1);
        KNOWN = Collections.unmodifiableMap(known);

        // Tegra (Shield) decoders take these at configure time without listing them, so they are
        // set by decoder name. Artemis sets the same four.
        Map<String, Integer> tegra = new LinkedHashMap<>();
        tegra.put("media.low-latency.enable", 1);
        tegra.put("vendor.low-latency.enable", 1);
        tegra.put("disable-output-reorder", 1);
        tegra.put("vendor.nvidia.disable-output-reorder", 1);
        TEGRA = Collections.unmodifiableMap(tegra);

        // The MediaTek preset of Artemis (ClassicOldSong) with the keys of the derflacco
        // moonlight-noir fork (GPL-3.0): mild boost, light prebuffering, short queues, output
        // paced by the app. The picture-quality and frame skip or drop keys (pq, nvop.skip,
        // skip.mode, drop.nonref.frame, frame-drop.policy) are not sent: the fork removed them.
        Map<String, Integer> mtk = new LinkedHashMap<>();
        for (Map.Entry<String, Integer> entry : known.entrySet()) {
            if (entry.getKey().startsWith("vendor.mtk.")) {
                mtk.put(entry.getKey(), entry.getValue());
            }
        }
        MEDIATEK_AV1 = Collections.unmodifiableMap(new LinkedHashMap<>(mtk));
        mtk.put("vendor.mtk.vdec.preload.frame.count", 1);
        mtk.put("vendor.mtk.vdec.buffer.fetch.timeout.ms", 4);
        mtk.put("vendor.mtk.vdec.bq.guard.interval.time", 4);
        mtk.put("vendor.mtk.vdec.input.max.queue.depth", 3);
        mtk.put("vendor.mtk.vdec.output.max.queue.depth", 3);
        MEDIATEK = Collections.unmodifiableMap(mtk);

        // Moonlight V+ (qiin2333/moonlight-vplus, GPL-3.0) tiers this preset by configure try, not
        // by SoC: queue depth 2, 2 ms fetch and guard waits and no preload once a try has failed,
        // depth 1 after that. Each of its fallbacks is stricter than the try before; here the ladder
        // goes from the most to the least risky format, so the shorter queues go first, on try 0,
        // and try 1 relaxes to the full preset with the same keys.
        Map<String, Integer> mtkShort = new LinkedHashMap<>(mtk);
        mtkShort.put("vendor.mtk.vdec.preload.frame.count", 0);
        mtkShort.put("vendor.mtk.vdec.buffer.fetch.timeout.ms", 2);
        mtkShort.put("vendor.mtk.vdec.bq.guard.interval.time", 2);
        mtkShort.put("vendor.mtk.vdec.input.max.queue.depth", 2);
        mtkShort.put("vendor.mtk.vdec.output.max.queue.depth", 2);
        MEDIATEK_SHORT = Collections.unmodifiableMap(mtkShort);

        // Moonlight X (MoreOrLessSoftware's mlsoft-x branch, GPL-3.0) sends all three to every
        // Exynos decoder, H.264, HEVC and AV1: the low-latency switch, the shortest output queue
        // and no output delay. The switch is declared by H.264 decoders only (issue 26), so the
        // preset goes by name like the MediaTek one.
        Map<String, Integer> exynos = new LinkedHashMap<>();
        exynos.put("vendor.rtc-ext-dec-low-latency.enable", 1);
        exynos.put("vendor.rtc-ext-dec-output-queue-depth.value", 2);
        exynos.put("vendor.sec-dec-output.delay", 0);
        EXYNOS = Collections.unmodifiableMap(exynos);
    }

    private VendorLowLatencyKeys() {}

    /**
     * Keys to set for a decoder exposing these integer vendor parameters: every known key it
     * declares, plus any other integer switch whose name says it enables low latency. Qualcomm's
     * extension keys are left out unless the decoder name is Qualcomm's, even when the decoder
     * declares them (issue 129).
     */
    static Map<String, Integer> forSupported(String decoderName, Collection<String> integerParams) {
        boolean qualcomm = isQualcommDecoder(decoderName);
        Map<String, Integer> keys = new LinkedHashMap<>();
        for (Map.Entry<String, Integer> known : KNOWN.entrySet()) {
            if ((qualcomm || !isQualcommExt(known.getKey())) &&
                    containsIgnoreCase(integerParams, known.getKey())) {
                keys.put(known.getKey(), known.getValue());
            }
        }
        for (String param : integerParams) {
            if ((qualcomm || !isQualcommExt(param)) && !containsIgnoreCase(keys.keySet(), param)
                    && looksLikeLowLatencySwitch(param)) {
                keys.put(param, 1);
            }
        }
        return keys;
    }

    /** Whether a codec name is one of Qualcomm's, matched by prefix like the rest of the code. */
    static boolean isQualcommDecoder(String decoderName) {
        if (decoderName == null) {
            return false;
        }
        String lower = decoderName.toLowerCase(Locale.ROOT);
        for (String prefix : QUALCOMM_DECODER_PREFIXES) {
            if (lower.startsWith(prefix)) {
                return true;
            }
        }
        return false;
    }

    private static boolean isQualcommExt(String key) {
        return key.toLowerCase(Locale.ROOT).startsWith(QUALCOMM_EXT_PREFIX);
    }

    /**
     * Every known Snapdragon key except QUALCOMM_DECLARED_ONLY, set on Qualcomm decoders whether
     * or not they declare it.
     */
    static Map<String, Integer> qualcommKeys() {
        Map<String, Integer> keys = new LinkedHashMap<>();
        for (Map.Entry<String, Integer> known : KNOWN.entrySet()) {
            if (known.getKey().startsWith("vendor.qti-ext-") && !QUALCOMM_DECLARED_ONLY.contains(known.getKey())) {
                keys.put(known.getKey(), known.getValue());
            }
        }
        return keys;
    }

    /** The QUALCOMM_DECLARED_ONLY keys among a decoder's integer vendor parameters, for the log. */
    static List<String> declaredQualcommExtras(Collection<String> integerParams) {
        List<String> declared = new ArrayList<>();
        for (String key : QUALCOMM_DECLARED_ONLY) {
            if (containsIgnoreCase(integerParams, key)) {
                declared.add(key);
            }
        }
        return declared;
    }

    /**
     * The whole MediaTek preset for a configure try, set by name whether or not the decoder lists
     * it: the shorter queues on try 0, the full preset after that. AV1 gets only the switches
     * (MEDIATEK_AV1) on every try: a MediaTek AV1 decoder on a Fire TV showed rainbow colours for
     * whole streams with the shorter queues and again with the full preset, without reporting a
     * single error and with no loss on the PC's side, so no fallback try ran (issue 56).
     */
    static Map<String, Integer> mediaTekKeys(int tryNumber, String mimeType) {
        if ("video/av01".equals(mimeType)) {
            return MEDIATEK_AV1;
        }
        return tryNumber == 0 ? MEDIATEK_SHORT : MEDIATEK;
    }

    static boolean isMediaTekDetectOnly(Collection<String> integerParams) {
        for (String key : MEDIATEK_DETECT_ONLY) {
            if (containsIgnoreCase(integerParams, key)) {
                return true;
            }
        }
        return false;
    }

    static boolean isKnown(String key) {
        return containsIgnoreCase(KNOWN.keySet(), key);
    }

    /** Keys that only switch the low-latency mode itself, kept on the last fallback try. */
    static boolean isCoreSwitch(String key) {
        String lower = key.toLowerCase(Locale.ROOT);
        return lower.contains("low-latency") || lower.contains("lowlatency") || lower.contains("low_latency");
    }

    static boolean looksLikeLowLatencySwitch(String param) {
        String lower = param.toLowerCase(Locale.ROOT);
        if (!lower.startsWith("vendor.") || !isCoreSwitch(lower)) {
            return false;
        }
        // Skip inverted, status and ultra modes; ultra-low-latency trades stability for little gain.
        if (lower.contains("disable") || lower.contains("rdy") || lower.contains("ultra")
                || lower.contains("status") || lower.contains("support")) {
            return false;
        }
        return lower.endsWith(".enable") || lower.endsWith(".value") || lower.endsWith(".mode")
                || lower.endsWith(".req") || lower.endsWith("-req");
    }

    private static boolean containsIgnoreCase(Collection<String> values, String wanted) {
        for (String value : values) {
            if (value.equalsIgnoreCase(wanted)) {
                return true;
            }
        }
        return false;
    }
}
