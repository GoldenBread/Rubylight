package com.limelight.binding.video;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.os.Build;

import com.limelight.LimeLog;
import com.limelight.preferences.PreferenceConfiguration;

/**
 * Low-latency MediaFormat option selection, vendor tuning fallback ladder, Google Codec2
 * operating-rate calculation, and tuning-key diagnostics supporting {@link MediaCodecHelper}.
 */
final class MediaCodecLowLatencyOptions {
    // Snapdragon decoders whose declared extras were logged, once each.
    private static final Set<String> snapdragonExtrasLogged = ConcurrentHashMap.newKeySet();

    private MediaCodecLowLatencyOptions() {}

    /**
     * Whether a decoder gets the vendor low-latency keys. Not a MediaTek decoder whose frames the
     * Vulkan renderer draws: on the TCL C6K (MT9689, Mali-G52) the GPU detiles its tuned output, and
     * after one to four frames the decoder takes no more input ("Dequeue input buffer ran long: 5000
     * ms", then DecoderHungException), in SDR and HDR, HEVC and AV1, mailbox and FIFO. Without the
     * keys the same stream ran its 45 s with no hang. Direct output keeps them.
     */
    static boolean usesVendorLowLatency(boolean vendorLowLatency, String decoderName, boolean vulkanOutput) {
        return vendorLowLatency && !(vulkanOutput && MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.mediaTekDecoderPrefixes, decoderName));
    }

    /**
     * Decoders whose vendor keys follow Artemis and the derflacco fork but were never run on
     * MediaTek, NVIDIA, Amlogic, Exynos or Tensor hardware by us; the stream log marks them Experimental.
     */
    static boolean isExperimentalVendorDecoder(String decoderName) {
        return MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.mediaTekDecoderPrefixes, decoderName)
                || MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.tegraDecoderPrefixes, decoderName)
                || MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.amlogicDecoderPrefixes, decoderName)
                || MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.exynosDecoderPrefixes, decoderName)
                || MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.googleDecoderPrefixes, decoderName)
                || decoderName.toLowerCase(Locale.ROOT).startsWith("c2.qcom");
    }

    static boolean decoderSupportsKnownVendorLowLatencyOption(String decoderName) {
        List<String> params = MediaCodecHelper.getIntegerVendorParameters(decoderName);
        if (params == null) {
            return false;
        }
        for (String key : VendorLowLatencyKeys.forSupported(decoderName, params).keySet()) {
            if (VendorLowLatencyKeys.isCoreSwitch(key)) {
                LimeLog.info(decoderName + " supports low latency option: " + key);
                return true;
            }
        }
        return false;
    }

    // MediaTek switches from the derflacco fork (moonlight-noir) that mark a decoder as able to
    // handle HEVC RFI. Only used for that; the AV1 whitelist keeps asking the known switches.
    // Experimental: not run on MediaTek hardware.
    static boolean decoderSupportsMediaTekRfiSwitch(String decoderName) {
        List<String> params = MediaCodecHelper.getIntegerVendorParameters(decoderName);
        if (params != null && VendorLowLatencyKeys.isMediaTekDetectOnly(params)) {
            LimeLog.info(decoderName + " supports a MediaTek low latency option");
            return true;
        }
        return false;
    }

    private static boolean decoderSupportsMaxOperatingRate(String decoderName) {
        // Operate at maximum rate to lower latency as much as possible on
        // some Qualcomm platforms. We could also set KEY_PRIORITY to 0 (realtime)
        // but that will actually result in the decoder crashing if it can't satisfy
        // our (ludicrous) operating rate requirement. This seems to cause reliable
        // crashes on the Xiaomi Mi 10 lite 5G and Redmi K30i 5G on Android 10, so
        // we'll disable it on Snapdragon 765G and all non-Qualcomm devices to be safe.
        //
        // NB: Even on Android 10, this optimization still provides significant
        // performance gains on Pixel 2.
        return MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.qualcommDecoderPrefixes, decoderName) &&
                !MediaCodecHelper.isAdreno620();
    }

    // "vdec-lowlatency" is read by the OMX layer of MediaTek and Amazon/Amlogic builds. Codec2
    // decoders and other vendors' OMX components never see it, so it is not sent to them.
    static boolean usesVdecLowLatency(String decoderName) {
        String name = decoderName.toLowerCase(Locale.ROOT);
        return name.startsWith("omx.") && !MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.qualcommDecoderPrefixes, decoderName) &&
                !MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.exynosDecoderPrefixes, decoderName) &&
                !MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.kirinDecoderPrefixes, decoderName);
    }

    private static boolean requiresHevcVdecLowLatency(String decoderName, String mime) {
        // Fire TV 3 otherwise accepts HEVC but outputs no frames (erratum 17). AFTN is its
        // Build.MODEL: https://developer.amazon.com/docs/device-specs/identify-fire-tv-devices.html
        return "video/hevc".equals(mime) && Build.MANUFACTURER.equalsIgnoreCase("Amazon") &&
                Build.MODEL.equalsIgnoreCase("AFTN") &&
                Build.VERSION.SDK_INT < Build.VERSION_CODES.O &&
                decoderName.toLowerCase(Locale.ROOT).startsWith("omx.amlogic");
    }

    /**
     * Whether a decoder goes without the Android and vendor low-latency flags under the HEVC
     * low-latency mode, Moonlight V+'s HevcLowLatencyPolicy (qiin2333/moonlight-vplus, GPL-3.0):
     * only HEVC is affected; Off leaves them out on every decoder, On on none, and Auto on the
     * decoders of {@link #autoOmitsHevcLowLatencyFlags}.
     */
    static boolean omitsHevcLowLatencyFlags(String mimeType, String decoderName, int hevcLowLatencyMode) {
        if (!"video/hevc".equals(mimeType)) return false;
        switch (hevcLowLatencyMode) {
            case PreferenceConfiguration.HEVC_LOW_LATENCY_OFF: return true;
            case PreferenceConfiguration.HEVC_LOW_LATENCY_ON: return false;
            default: return autoOmitsHevcLowLatencyFlags(decoderName);
        }
    }

    /**
     * Auto's exception: Codec2 Amlogic decoders, which configure fine with the flags and then show
     * about one frame a second (erratum 22, c2.amlogic.hevc.decoder; V+ issue 499,
     * moonlight-android 1584), which no configure fallback can catch. V+ also lists omx.amlogic,
     * but the reports are Codec2 only. Fire TV is left out: its Amlogic decoders run HEVC with
     * low-latency decoding (the Fire TV 3 shows no frames without it) and no report there shows
     * the stall.
     */
    static boolean autoOmitsHevcLowLatencyFlags(String decoderName) {
        return !MediaCodecHelper.isFireTv() && decoderName.toLowerCase(Locale.ROOT).startsWith("c2.amlogic");
    }

    /** Without logOverrides the Amlogic HEVC warnings stay out, for a format that is only compared. */
    static boolean setDecoderLowLatencyOptions(MediaFormat videoFormat, MediaCodecInfo decoderInfo, int tryNumber,
                                               boolean lowLatency, boolean vendorLowLatency, boolean performanceHints,
                                               int hevcLowLatencyMode, boolean chipsetTuning, boolean logOverrides) {
        // Options here should be tried in the order of most to least risky. The decoder will use
        // the first MediaFormat that doesn't fail in configure().

        boolean setNewOption = false;

        // Some TV firmware accepts these flags but displays HEVC as a slideshow (erratum 22). The
        // HEVC low-latency mode decides, independent of decoder performance hints.
        String mime = videoFormat.getString(MediaFormat.KEY_MIME);
        boolean disableLowLatency = omitsHevcLowLatencyFlags(mime, decoderInfo.getName(), hevcLowLatencyMode);
        boolean amlogic = "video/hevc".equals(mime) && autoOmitsHevcLowLatencyFlags(decoderInfo.getName());
        if (logOverrides && amlogic && tryNumber == 0 && (lowLatency || vendorLowLatency)) {
            if (hevcLowLatencyMode == PreferenceConfiguration.HEVC_LOW_LATENCY_AUTO) {
                LimeLog.warning("HEVC low-latency flags omitted on Amlogic decoder " + decoderInfo.getName() +
                        ": they stall its display (erratum 22)");
            } else if (hevcLowLatencyMode == PreferenceConfiguration.HEVC_LOW_LATENCY_ON) {
                LimeLog.warning("HEVC low-latency flags forced on Amlogic decoder " + decoderInfo.getName() +
                        ": if video updates about once a second, set HEVC low-latency flags to Auto (erratum 22)");
            }
        }
        boolean useVendorOptions = vendorLowLatency && !disableLowLatency;
        if (lowLatency && !disableLowLatency && tryNumber < 1) {
            // Official Android 11+ low latency option (KEY_LOW_LATENCY). Many decoders honour it
            // without advertising FEATURE_LowLatency, and decoders that don't know it ignore it.
            // Artemis (ClassicOldSong) sends it on every Android version, because some vendor
            // builds read the "low-latency" key before Android 11; older frameworks ignore it.
            // Vendor keys stay on as well: on Snapdragon 8 Gen 2 and newer the standard flag alone
            // leaves the output fence in place, which costs several milliseconds per frame.
            videoFormat.setInteger(MediaFormat.KEY_LOW_LATENCY, 1);
            setNewOption = true;
        }

        boolean requiredVdec = requiresHevcVdecLowLatency(decoderInfo.getName(), videoFormat.getString(MediaFormat.KEY_MIME));
        if (requiredVdec || (useVendorOptions && tryNumber < 2 && usesVdecLowLatency(decoderInfo.getName()))) {
            // MediaTek decoders don't use vendor-defined keys for low latency mode. Instead, they have a modified
            // version of AOSP's ACodec.cpp which supports the "vdec-lowlatency" option. This option is passed down
            // to the decoder as OMX.MTK.index.param.video.LowLatencyDecode.
            //
            // This option is also plumbed for Amazon Amlogic-based devices like the Fire TV 3. Not only does it
            // reduce latency on Amlogic, it fixes the HEVC bug that causes the decoder to not output any frames.
            // Unfortunately, it does the exact opposite for the Xiaomi MITV4-ANSM0, breaking it in the way that
            // Fire TV was broken prior to vdec-lowlatency :(
            //
            // On Fire TV 3, vdec-lowlatency is translated to OMX.amazon.fireos.index.video.lowLatencyDecode.
            //
            // https://github.com/yuan1617/Framwork/blob/master/frameworks/av/media/libstagefright/ACodec.cpp
            // https://github.com/iykex/vendor_mediatek_proprietary_hardware/blob/master/libomx/video/MtkOmxVdecEx/MtkOmxVdecEx.h
            videoFormat.setInteger("vdec-lowlatency", 1);
            // Keep mandatory compatibility settings even in the final fallback. They do not
            // count as another optional tuning attempt, so configure failures still terminate.
            setNewOption |= !requiredVdec || tryNumber == 0;
        }
        else if (chipsetTuning && useVendorOptions && tryNumber < 1 &&
                decoderInfo.getName().toLowerCase(Locale.ROOT).startsWith("c2.mtk")) {
            // Experimental: we have no Dimensity device to run this on.
            videoFormat.setInteger("vdec-lowlatency", 1);
            setNewOption = true;
        }

        // Without low-latency flags, fall back to an untuned format after the first rejected hint.
        if (performanceHints && tryNumber < (disableLowLatency ? 1 : 3)) {
            // Experimental: Artemis asks Tegra decoders for the top operating rate, and the derflacco fork
            // (moonlight-noir, GPL-3.0) MediaTek decoders too. They get the rate only, like Snapdragon:
            // the real-time priority next to a rate the decoder cannot meet is the pair that crashes
            // decoders after start(), outside the fallback ladder, so it is not sent to them (the fork
            // does send it to MediaTek).
            boolean rateOnly = useVendorOptions && (MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.tegraDecoderPrefixes, decoderInfo.getName()) ||
                    MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.mediaTekDecoderPrefixes, decoderInfo.getName()));
            if (rateOnly || decoderSupportsMaxOperatingRate(decoderInfo.getName())) {
                videoFormat.setInteger(MediaFormat.KEY_OPERATING_RATE, Short.MAX_VALUE);
                setNewOption = true;
            }
            else if (chipsetTuning && useVendorOptions && tryNumber < 1 &&
                    MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.exynosDecoderPrefixes, decoderInfo.getName())) {
                // Experimental: a rate the decoder declares it can meet, so real-time priority
                // does not come with an unattainable rate. We have no Exynos device to run this on.
                videoFormat.setInteger(MediaFormat.KEY_OPERATING_RATE, googleOperatingRate(videoFormat, decoderInfo));
                videoFormat.setInteger(MediaFormat.KEY_PRIORITY, 0);
                setNewOption = true;
            }
            else {
                videoFormat.setInteger(MediaFormat.KEY_PRIORITY, 0);
                setNewOption = true;
            }
        }

        // MediaCodec supports vendor-defined format keys using the "vendor.<extension name>.<parameter name>" syntax.
        // These allow access to functionality that is not exposed through documented MediaFormat.KEY_* values.
        // https://cs.android.com/android/platform/superproject/+/master:hardware/qcom/sdm845/media/mm-video-v4l2/vidc/common/inc/vidc_vendor_extensions.h;l=67
        //
        // MediaCodec vendor extension support was introduced in Android 8.0:
        // https://cs.android.com/android/_/android/platform/frameworks/av/+/01c10f8cdcd58d1e7025f426a72e6e75ba5d7fc2
        // NVIDIA Tegra decoders take their keys by name whether or not they list them (Artemis does
        // the same). They go first so the fallback tries drop them like every other vendor key.
        // Experimental: we have no Shield to run the keys on.
        if (useVendorOptions && tryNumber < 4 && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.tegraDecoderPrefixes, decoderInfo.getName())) {
            for (Map.Entry<String, Integer> key : VendorLowLatencyKeys.TEGRA.entrySet()) {
                videoFormat.setInteger(key.getKey(), key.getValue());
            }
            setNewOption = true;
        }

        // MediaTek decoders take the whole preset by name (Artemis and the derflacco fork do the
        // same), declared or not. The ladder for them, most to least risky: try 0 the preset with
        // Moonlight V+'s shorter queues and KEY_LOW_LATENCY, try 1 the full preset (longer queues
        // and waits, one preloaded frame) without KEY_LOW_LATENCY, try 2 and later only the plain
        // vendor switches below. Each try sends the keys of the one before or fewer, with values
        // that only relax toward the full preset (VendorLowLatencyKeys.mediaTekKeys). AV1 gets only
        // the switches, without queue, wait or preload keys, on every try.
        // Android 8 and newer, like Tegra: older builds (Fire OS 5 OMX.MTK on 5.1) have no vendor
        // key support.
        // Experimental: we have no MediaTek device to run the preset on.
        if (useVendorOptions && tryNumber < 2 && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.mediaTekDecoderPrefixes, decoderInfo.getName())) {
            for (Map.Entry<String, Integer> key : VendorLowLatencyKeys.mediaTekKeys(tryNumber, mime).entrySet()) {
                videoFormat.setInteger(key.getKey(), key.getValue());
            }
            setNewOption = true;
        }

        // Samsung Exynos decoders take the whole preset by name. On Android 12 and newer the
        // declared-keys path below sends only what a decoder lists, and Exynos H.264 decoders list
        // just the low-latency switch while HEVC and AV1 list none (issue 26). The preset leaves
        // after the second try; the low-latency switch alone stays through the fourth.
        // Experimental: we have no Exynos device to run the preset on.
        if (useVendorOptions && tryNumber < 4 && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.exynosDecoderPrefixes, decoderInfo.getName())) {
            for (Map.Entry<String, Integer> key : VendorLowLatencyKeys.EXYNOS.entrySet()) {
                if (tryNumber < 2 || VendorLowLatencyKeys.isCoreSwitch(key.getKey())) {
                    videoFormat.setInteger(key.getKey(), key.getValue());
                    setNewOption = true;
                }
            }
        }

        // Google Codec2 (Tensor) decoders run at the most they can do for the stream's size, with
        // real-time priority, and on Android 16 (API 36) and newer also get the frame rate by their
        // own key. The rate comes from the decoder's own capabilities, so it can meet it. Behind
        // the performance hints like the other priority-0 decoders.
        // Experimental: we have no Tensor device to run this on.
        if (useVendorOptions && performanceHints && tryNumber < 2 &&
                MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.googleDecoderPrefixes, decoderInfo.getName())) {
            int rate = googleOperatingRate(videoFormat, decoderInfo);
            videoFormat.setInteger(MediaFormat.KEY_OPERATING_RATE, rate);
            videoFormat.setInteger(MediaFormat.KEY_PRIORITY, 0);
            if (tryNumber == 0 && Build.VERSION.SDK_INT >= 36) {
                videoFormat.setInteger(VendorLowLatencyKeys.GOOGLE_FRAME_RATE, rate);
            }
            setNewOption = true;
        }

        List<String> vendorParams = useVendorOptions ? MediaCodecHelper.getIntegerVendorParameters(decoderInfo.getName()) : null;
        if (vendorParams != null && !vendorParams.isEmpty() && tryNumber < 5) {
            // Android 12+: set exactly the low-latency keys this decoder declares. The full set goes
            // first; the last vendor try keeps only the low-latency switches themselves. The
            // declared-only Snapdragon extras go on the first try alone, so a decoder that rejects
            // one of them still gets the confirmed Qualcomm keys on the next tries.
            if (tryNumber == 0 && MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.qualcommDecoderPrefixes, decoderInfo.getName()) &&
                    snapdragonExtrasLogged.add(decoderInfo.getName())) {
                LimeLog.info(decoderInfo.getName() + " declares Snapdragon extras: " +
                        VendorLowLatencyKeys.declaredQualcommExtras(vendorParams));
            }
            for (Map.Entry<String, Integer> key : VendorLowLatencyKeys.forSupported(decoderInfo.getName(), vendorParams).entrySet()) {
                if (tryNumber > 0 && VendorLowLatencyKeys.QUALCOMM_DECLARED_ONLY.contains(key.getKey())) {
                    continue;
                }
                if (tryNumber < 4 || VendorLowLatencyKeys.isCoreSwitch(key.getKey())) {
                    videoFormat.setInteger(key.getKey(), key.getValue());
                    setNewOption = true;
                }
            }
            // Snapdragon decoders take their low-latency and output-fence keys at configure time
            // without always listing them as vendor parameters; without the software fence each
            // frame waits several milliseconds longer. The first tries set the full Qualcomm set,
            // the later ones only what the decoder declares.
            if (tryNumber < 3 && MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.qualcommDecoderPrefixes, decoderInfo.getName())) {
                for (Map.Entry<String, Integer> key : VendorLowLatencyKeys.qualcommKeys().entrySet()) {
                    if (!videoFormat.containsKey(key.getKey())) {
                        videoFormat.setInteger(key.getKey(), key.getValue());
                        setNewOption = true;
                    }
                }
            }
        }
        else if (useVendorOptions && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            // Try vendor-specific low latency options
            //
            // Used below Android 12, where declared vendor parameters can't be listed.
            // NOTE: Update VendorLowLatencyKeys.KNOWN if you modify this code!
            if (MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.qualcommDecoderPrefixes, decoderInfo.getName())) {
                // Examples of Qualcomm's vendor extensions for Snapdragon 845:
                // https://cs.android.com/android/platform/superproject/+/master:hardware/qcom/sdm845/media/mm-video-v4l2/vidc/vdec/src/omx_vdec_extensions.hpp
                // https://cs.android.com/android/_/android/platform/hardware/qcom/sm8150/media/+/0621ceb1c1b19564999db8293574a0e12952ff6c
                //
                // We will first try both, then try vendor.qti-ext-dec-low-latency.enable alone if that fails
                if (tryNumber < 4) {
                    videoFormat.setInteger("vendor.qti-ext-dec-picture-order.enable", 1);
                    setNewOption = true;
                }
                if (tryNumber < 5) {
                    videoFormat.setInteger("vendor.qti-ext-dec-low-latency.enable", 1);
                    setNewOption = true;
                }
                if (tryNumber < 3) {
                    // Snapdragon 8 Gen 2 and newer: a software output fence releases frames sooner.
                    videoFormat.setInteger("vendor.qti-ext-output-sw-fence-enable.value", 1);
                    videoFormat.setInteger("vendor.qti-ext-output-fence.enable", 1);
                    videoFormat.setInteger("vendor.qti-ext-output-fence.fence_type", 1);
                }
            }
            else if (MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.kirinDecoderPrefixes, decoderInfo.getName())) {
                if (tryNumber < 4) {
                    // Kirin low latency options
                    // https://developer.huawei.com/consumer/cn/forum/topic/0202325564295980115
                    videoFormat.setInteger("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-req", 1);
                    videoFormat.setInteger("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-rdy", -1);
                    setNewOption = true;
                }
            }
            else if (MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.amlogicDecoderPrefixes, decoderInfo.getName())) {
                if (tryNumber < 4) {
                    // Amlogic low latency vendor extension
                    // https://github.com/codewalkerster/android_vendor_amlogic_common_prebuilt_libstagefrighthw/commit/41fefc4e035c476d58491324a5fe7666bfc2989e
                    videoFormat.setInteger("vendor.low-latency.enable", 1);
                    setNewOption = true;
                }
            }
        }

        // Snapdragon extras from the derflacco fork: frame drop on, and on the old OMX decoders a
        // smaller output delay. The first tries only, so a decoder that rejects them falls back.
        // The fork also sets picture-order to 0 after the keys above; on the Snapdragon 8 Gen 2
        // (c2.qti.hevc.decoder.low_latency) that made the decoder produce no output at all
        // (28 frames in, none out, DecoderHungException after 5 s), so it is not ported.
        if (useVendorOptions && tryNumber < 3 &&
                MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.qualcommDecoderPrefixes, decoderInfo.getName())) {
            // Off by default: the decoder may drop a picture the display then never sees.
            // debug.nightjar.qti_frame_drop 1 turns it on for A/B runs.
            if (MediaCodecCapabilities.debugSwitch("qti_frame_drop", false)) {
                videoFormat.setInteger("vendor.qti-ext-dec-frame-drop.enable", 1);
            }
            if (decoderInfo.getName().toLowerCase(Locale.ROOT).startsWith("omx.qcom")) {
                videoFormat.setInteger("vendor.qti-ext-dec-dpb-output-delay.enable", 0);
                videoFormat.setInteger("vendor.qti-ext-dec-picture-type.enable", 0);
            }
            setNewOption = true;
        }

        return setNewOption;
    }

    /**
     * Operating rate for a Google Codec2 decoder: the most it reports for the stream's size, at
     * most 960 and never above what it declares. Without capabilities for the size the rate is
     * the stream's frame rate, at least 120. Moonlight X (GPL-3.0) asks for the decoder's maximum
     * because a rate equal to the stream's makes each frame take most of a frame interval (about
     * 8 ms at 120 FPS on a Pixel 10).
     */
    static int googleOperatingRate(MediaFormat format, MediaCodecInfo decoderInfo) {
        int fps = format.containsKey(MediaFormat.KEY_FRAME_RATE) ? format.getInteger(MediaFormat.KEY_FRAME_RATE) : 60;
        try {
            MediaCodecInfo.VideoCapabilities caps = decoderInfo
                    .getCapabilitiesForType(format.getString(MediaFormat.KEY_MIME)).getVideoCapabilities();
            int width = format.getInteger(MediaFormat.KEY_WIDTH);
            int height = format.getInteger(MediaFormat.KEY_HEIGHT);
            if (caps != null && caps.isSizeSupported(width, height)) {
                return googleOperatingRate(fps, caps.getSupportedFrameRatesFor(width, height).getUpper());
            }
        } catch (Exception e) {
            // Keep the rate from the stream
        }
        return googleOperatingRate(fps, null);
    }

    /** The rate for a stream frame rate and the decoder's declared maximum at that size (null: none). */
    static int googleOperatingRate(int fps, Double capsUpper) {
        if (capsUpper == null) {
            return Math.max(120, fps);
        }
        // The 120 floor only applies to a decoder that declares at least that much.
        return (int) Math.max(1, Math.min(capsUpper, 960));
    }

    /**
     * Splits the requested low-latency keys into ones the decoder confirmed (echoed back, declared
     * as a vendor parameter, or advertised as FEATURE_LowLatency) and ones nothing confirms.
     * Element 2 is the mode: "android+vendor", "android", "vendor", "requested" (keys were asked
     * for and nothing can confirm them) or "off".
     */
    static String[] getDecoderLowLatencyOptions(MediaFormat requested, MediaFormat accepted,
                                                List<String> vendorParams, boolean featureLowLatency) {
        List<String> keys = tuningKeyNames(vendorParams);
        List<String> confirmed = new ArrayList<>();
        List<String> unconfirmed = new ArrayList<>();
        boolean android = false, vendor = false, asked = false;
        for (String key : keys) {
            if (!requested.containsKey(key)) {
                continue;
            }
            asked = true;
            int value = requested.getInteger(key);
            boolean echoed = accepted != null && accepted.containsKey(key) && value == accepted.getInteger(key);
            boolean ok = echoed || (key.equals(MediaFormat.KEY_LOW_LATENCY) ? featureLowLatency
                    : vendorParams != null && vendorParams.contains(key));
            (ok ? confirmed : unconfirmed).add(key + "=" + value);
            if (ok && key.equals(MediaFormat.KEY_LOW_LATENCY)) {
                android = true;
            } else if (ok && key.startsWith("vendor.")) {
                // Any confirmed chip-vendor setting counts, e.g. Snapdragon's output fence on
                // its dedicated low-latency decoders, which have no separate low-latency switch.
                vendor = true;
            }
        }
        // Sent but unconfirmed is no "off": below Android 11 no decoder can confirm anything, whatever
        // it does with the keys (a Fire TV's omx.mtk decoders on Android 9, #56).
        String mode = android && vendor ? "android+vendor" : android ? "android" : vendor ? "vendor" :
                asked ? "requested" : "off";
        return new String[] { confirmed.isEmpty() ? "none" : String.join(", ", confirmed),
                unconfirmed.isEmpty() ? "none" : String.join(", ", unconfirmed), mode };
    }

    /** Every format key this class can set for decoder tuning, plus the decoder's own vendor parameters. */
    private static List<String> tuningKeyNames(List<String> vendorParams) {
        List<String> keys = new ArrayList<>(VendorLowLatencyKeys.KNOWN.keySet());
        keys.add(0, MediaFormat.KEY_LOW_LATENCY);
        keys.add(1, "vdec-lowlatency");
        keys.add("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-rdy");
        for (String key : VendorLowLatencyKeys.TEGRA.keySet()) {
            if (!keys.contains(key)) {
                keys.add(key);
            }
        }
        for (String key : VendorLowLatencyKeys.MEDIATEK.keySet()) {
            if (!keys.contains(key)) {
                keys.add(key);
            }
        }
        for (String key : VendorLowLatencyKeys.EXYNOS.keySet()) {
            if (!keys.contains(key)) {
                keys.add(key);
            }
        }
        for (String key : new String[] {"vendor.qti-ext-dec-frame-drop.enable", "vendor.qti-ext-dec-dpb-output-delay.enable",
                "vendor.qti-ext-dec-picture-type.enable", VendorLowLatencyKeys.GOOGLE_FRAME_RATE}) {
            if (!keys.contains(key)) {
                keys.add(key);
            }
        }
        if (vendorParams != null) {
            for (String param : vendorParams) {
                if (!keys.contains(param)) {
                    keys.add(param);
                }
            }
        }
        return keys;
    }

    /** The tuning keys a format asks for, as "key=value" in a fixed order. */
    static List<String> requestedTuningKeys(MediaFormat format, List<String> vendorParams) {
        List<String> keys = tuningKeyNames(vendorParams);
        keys.add(MediaFormat.KEY_OPERATING_RATE);
        keys.add(MediaFormat.KEY_PRIORITY);
        List<String> requested = new ArrayList<>();
        for (String key : keys) {
            if (format.containsKey(key)) {
                requested.add(key + "=" + format.getInteger(key));
            }
        }
        return requested;
    }

    /**
     * The tuning keys of a configured try that the vendor low-latency setting added: those the same
     * try without the setting does not ask for. keyless is a base format the options are set on.
     */
    static List<String> vendorAddedKeys(List<String> configured, MediaFormat keyless, MediaCodecInfo decoderInfo,
                                        int tryNumber, boolean lowLatency, boolean performanceHints,
                                        int hevcLowLatencyMode, boolean chipsetTuning, List<String> vendorParams) {
        setDecoderLowLatencyOptions(keyless, decoderInfo, tryNumber, lowLatency, false, performanceHints,
                hevcLowLatencyMode, chipsetTuning, false);
        List<String> added = new ArrayList<>(configured);
        added.removeAll(requestedTuningKeys(keyless, vendorParams));
        return added;
    }

    /** Keys the first configure attempt asked for that the format finally configured no longer carries. */
    static List<String> droppedTuningKeys(List<String> firstTry, List<String> configured) {
        List<String> dropped = new ArrayList<>(firstTry);
        dropped.removeAll(configured);
        return dropped;
    }
}
