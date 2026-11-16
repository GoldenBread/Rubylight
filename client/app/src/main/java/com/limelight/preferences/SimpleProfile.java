package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.media.MediaCodecInfo;
import android.media.MediaCodecList;
import android.os.Build;
import android.preference.PreferenceManager;

import com.limelight.binding.video.DecoderBenchmark;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.HdrPolicy;

/**
 * The simple settings layer: what the stream should favour, how much picture quality it may spend, and
 * whether HDR may be used. The choices are read and written through nightjar-policy's {@code simple_profile}
 * (see {@link SimpleProfileModel}); this class gathers this device's measured capabilities, reads the concrete
 * settings the app already has, and writes the profile back into them. Nothing here changes a stream until a
 * choice is made, so a user who never opens Settings keeps the defaults they had.
 */
public final class SimpleProfile {
    /** What the stream should favour: smoothest, balanced or lowest-delay. */
    static final String SMOOTHNESS = "simple_smoothness";
    /** Picture quality: auto, best or save-data. */
    static final String PICTURE = "simple_picture";
    /** HDR: auto or off. */
    static final String HDR = "simple_hdr";
    /** The values a row can take, in the order of its picker. */
    static final String[] SMOOTHNESS_VALUES = {"smoothest", "balanced", "lowest-delay"};
    static final String[] PICTURE_VALUES = {"auto", "best", "save-data"};
    static final String[] HDR_VALUES = {"auto", "off"};

    private SimpleProfile() { }

    /** The choice stored for {@code key}, or the recommended one when nothing (valid) is stored. */
    static String read(SharedPreferences prefs, String key) {
        if (SMOOTHNESS.equals(key)) {
            return known(prefs.getString(key, "balanced"), SMOOTHNESS_VALUES, "balanced");
        }
        if (PICTURE.equals(key)) {
            return known(prefs.getString(key, "auto"), PICTURE_VALUES, "auto");
        }
        if (HDR.equals(key)) {
            return known(prefs.getString(key, "auto"), HDR_VALUES, "auto");
        }
        return "auto";
    }

    private static String known(String value, String[] values, String fallback) {
        for (String offered : values) if (offered.equals(value)) return value;
        return fallback;
    }

    /** What this device and its paired PCs measured, for the mapping. */
    static SimpleProfileModel.Capabilities capabilities(Context context) {
        android.view.Display display = PreferenceConfiguration.defaultDisplay(context);
        int refresh = display == null ? 60 : DisplayFrameRatePolicy.streamFrameRate(
                DisplayFrameRatePolicy.maxRefreshRate(display));
        String nativeResolution = PreferenceConfiguration.nativeResolution(context);
        int width = 1280, height = 720;
        if (nativeResolution != null) {
            width = PreferenceConfiguration.getWidthFromResolutionString(nativeResolution);
            height = PreferenceConfiguration.getHeightFromResolutionString(nativeResolution);
        }
        boolean hdr = display != null && HdrPolicy.hasHdr10(HdrPolicy.displayHdrTypes(display));
        // Variable refresh rate stays this device's own opt-in choice: the simple layer changes the
        // pacing, never a display mode the player has not turned on (VRR can flicker on some panels).
        boolean vrr = DisplayFrameRatePolicy.vrrOffered(context) && PreferenceManager
                .getDefaultSharedPreferences(context)
                .getBoolean(PreferenceConfiguration.VRR_PREF_STRING, false);
        Codecs codecs = codecs(context, width, height);
        // The PCs' HDR and codec offers are settled when a stream negotiates; the settings screen does
        // not know them, and leaving them unknown never rules anything out.
        return new SimpleProfileModel.Capabilities(width, height, refresh, hdr, vrr,
                true, codecs.hevc, codecs.av1, codecs.hdr, codecs.maxFpsAtNative, null, null, null);
    }

    private static final class Codecs {
        boolean hevc, av1, hdr;
        Integer maxFpsAtNative;
    }

    /**
     * The hardware decoders this device has, from what Android lists, and the benchmark's fastest measured
     * rate at the screen's size. No vendor or model is consulted; a codec counts only when a decoder that is
     * not software supports it, and HDR only when that decoder's profile is a 10-bit HDR one.
     */
    private static Codecs codecs(Context context, int width, int height) {
        Codecs codecs = new Codecs();
        long bestUs = 0;
        try {
            MediaCodecList list = new MediaCodecList(MediaCodecList.REGULAR_CODECS);
            for (MediaCodecInfo info : list.getCodecInfos()) {
                if (info.isEncoder() || !hardware(info)) continue;
                String[] types = info.getSupportedTypes();
                for (String type : types) {
                    if ("video/hevc".equals(type)) {
                        codecs.hevc = true;
                        if (main10(info, type)) codecs.hdr = true;
                    } else if ("video/av01".equals(type)) {
                        codecs.av1 = true;
                        if (main10(info, type)) codecs.hdr = true;
                    }
                    // The benchmark's clip is 1080p60: it can only cap a stream at or below that size.
                    if ((long) width * height <= 1920L * 1080 && hardware(info)) {
                        long us = DecoderBenchmark.result(context, info, type);
                        if (us > 0 && (bestUs == 0 || us < bestUs)) bestUs = us;
                    }
                }
            }
        } catch (RuntimeException e) {
            com.limelight.LimeLog.warning("Simple settings: the decoder list could not be read: " + e);
        }
        if (bestUs > 0) {
            codecs.maxFpsAtNative = (int) Math.min(1000, 1_000_000L / bestUs);
        }
        return codecs;
    }

    private static boolean hardware(MediaCodecInfo info) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) return info.isHardwareAccelerated();
        String name = info.getName().toLowerCase(java.util.Locale.ROOT);
        return !name.startsWith("omx.google.") && !name.startsWith("c2.android.") && !name.contains("software");
    }

    /** Whether a decoder's HEVC or AV1 profile carries 10-bit HDR (Main10 and its HDR variants). */
    private static boolean main10(MediaCodecInfo info, String mime) {
        try {
            MediaCodecInfo.CodecCapabilities capabilities = info.getCapabilitiesForType(mime);
            for (MediaCodecInfo.CodecProfileLevel level : capabilities.profileLevels) {
                if ("video/hevc".equals(mime)) {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N &&
                            (level.profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10 ||
                                    level.profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10 ||
                                    level.profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10Plus)) {
                        return true;
                    }
                } else if ("video/av01".equals(mime)) {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q &&
                            (level.profile == MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10 ||
                                    level.profile == MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10HDR10 ||
                                    level.profile == MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10HDR10Plus)) {
                        return true;
                    }
                }
            }
        } catch (RuntimeException ignored) {
            // A decoder that does not answer for this codec does not count for it.
        }
        return false;
    }

    /** The simple choices these settings are, {smoothness, picture, hdr}; null for Custom. */
    static String[] chosen(Context context) {
        return SimpleProfileModel.chosen(capabilities(context), current(context));
    }

    /** {@code key} is Custom when the settings match no choice of the row. */
    static boolean custom(Context context, String key) {
        return chosen(context) == null;
    }

    /** Applies {@code value} to {@code key}, keeping the other two choices. */
    static boolean apply(Context context, String key, String value) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        // The settings may equal a profile the stored choices do not name (a profile the player
        // matched by hand, or the app's defaults): a change starts from what the rows show.
        String[] shown = chosen(context);
        String smoothness = shown != null ? shown[0] : read(prefs, SMOOTHNESS);
        String picture = shown != null ? shown[1] : read(prefs, PICTURE);
        String hdr = shown != null ? shown[2] : read(prefs, HDR);
        if (SMOOTHNESS.equals(key)) smoothness = value;
        else if (PICTURE.equals(key)) picture = value;
        else if (HDR.equals(key)) hdr = value;
        else return false;
        return apply(context, prefs, smoothness, picture, hdr);
    }

    /** Writes the recommended profile: the one "Back to recommended" takes. */
    static boolean applyRecommended(Context context) {
        SimpleProfileModel.Capabilities caps = capabilities(context);
        String[] recommended = SimpleProfileModel.recommended(caps);
        if (recommended == null) return false;
        return apply(context, PreferenceManager.getDefaultSharedPreferences(context),
                recommended[0], recommended[1], recommended[2]);
    }

    private static boolean apply(Context context, SharedPreferences prefs, String smoothness, String picture,
                                 String hdr) {
        SimpleProfileModel.Capabilities caps = capabilities(context);
        SimpleProfileModel.Values values = SimpleProfileModel.values(smoothness, picture, hdr, caps);
        String nativeResolution = PreferenceConfiguration.nativeResolution(context);
        if (values == null || nativeResolution == null) return false;
        String fps = Integer.toString(values.fps == null ? caps.refreshHz : values.fps);
        int bitrate = PreferenceConfiguration.getDefaultBitrate(nativeResolution, fps) * values.bitrateMilli / 1000;
        // Android's stored value for the Smoothest pacing is "smoothness" (FramePacingChoice).
        String pacing = "smoothest".equals(values.pacing) ? "smoothness" : values.pacing;
        SharedPreferences.Editor editor = prefs.edit()
                .putString(SMOOTHNESS, smoothness)
                .putString(PICTURE, picture)
                .putString(HDR, hdr)
                .putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, nativeResolution)
                .putBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, true)
                .putString(PreferenceConfiguration.FPS_PREF_STRING, fps)
                .putInt(PreferenceConfiguration.BITRATE_PREF_STRING, bitrate)
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, values.bitrateMilli != 1000)
                .putString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, pacing)
                .putBoolean(SettingRules.DROP_LATE_FRAMES, true)
                .putBoolean(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING, false)
                .putBoolean(PreferenceConfiguration.VRR_PREF_STRING, values.vrr)
                .putBoolean("checkbox_reduce_refresh_rate", false)
                .putBoolean(PreferenceConfiguration.CONTENT_RATE_FRAME_RATE_PREF_STRING, false)
                .putString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, codec(values.codec))
                .putBoolean(PreferenceConfiguration.ENABLE_HDR_PREF_STRING, values.hdr)
                .putBoolean(AutoBitrateChoice.SETTINGS_KEY, values.adaptiveBitrate)
                .putString("adaptive_quality_mode", "save-power".equals(values.adaptiveQuality) ? "battery" : "off");
        if (values.vrr) editor.putBoolean(PreferenceConfiguration.REAL_VRR_PREF_STRING, true);
        if (values.latencySwitches) {
            for (String switchKey : StreamPreset.LATENCY_SWITCHES) {
                if (StreamPreset.setsLatencySwitch(switchKey)) editor.putBoolean(switchKey, true);
            }
        }
        if (!editor.commit()) {
            com.limelight.LimeLog.warning("Simple settings: the profile could not be saved");
            return false;
        }
        AutoBitrateChoice.clearPcChoices(context);
        return true;
    }

    private static String codec(String codec) {
        if ("hevc".equals(codec)) return "forceh265";
        if ("av1".equals(codec)) return "forceav1";
        if ("h264".equals(codec)) return "neverh265";
        return "auto";
    }

    /** The stream's current advanced settings, as the mapping compares them. */
    static SimpleProfileModel.Current current(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        android.view.Display display = PreferenceConfiguration.defaultDisplay(context);
        int panelRate = display == null ? 60 : DisplayFrameRatePolicy.streamFrameRate(
                DisplayFrameRatePolicy.maxRefreshRate(display));
        String nativeResolution = PreferenceConfiguration.nativeResolution(context);
        String storedResolution = prefs.getString(PreferenceConfiguration.RESOLUTION_PREF_STRING,
                PreferenceConfiguration.DEFAULT_RESOLUTION);
        boolean nativeSize = nativeResolution != null && (prefs.getBoolean(
                PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false) ||
                nativeResolution.equals(storedResolution));
        int storedFps;
        try {
            storedFps = Integer.parseInt(prefs.getString(PreferenceConfiguration.FPS_PREF_STRING,
                    PreferenceConfiguration.DEFAULT_FPS));
        } catch (NumberFormatException e) {
            storedFps = panelRate;
        }
        Integer fps = storedFps == panelRate ? null : storedFps;
        String pacing = pacing(FramePacingChoice.read(prefs));
        boolean latencySwitches = true;
        for (String key : StreamPreset.LATENCY_SWITCHES) {
            if (StreamPreset.setsLatencySwitch(key) && !prefs.getBoolean(key, true)) latencySwitches = false;
        }
        Integer bitrateMilli = null;
        if (prefs.getBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, false)) {
            String res = nativeResolution != null ? nativeResolution
                    : storedResolution.contains("x") ? storedResolution : PreferenceConfiguration.DEFAULT_RESOLUTION;
            int defaultBitrate = PreferenceConfiguration.getDefaultBitrate(res, Integer.toString(storedFps));
            int stored = prefs.getInt(PreferenceConfiguration.BITRATE_PREF_STRING, defaultBitrate);
            bitrateMilli = defaultBitrate <= 0 ? null : (int) Math.round(stored * 1000.0 / defaultBitrate);
        }
        String storedCodec = prefs.getString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING,
                PreferenceConfiguration.DEFAULT_VIDEO_FORMAT);
        // PyroWave has no simple profile: H.264 cannot match one either, so those settings read as Custom.
        String codec = "auto".equals(storedCodec) ? "auto" : "forceh265".equals(storedCodec) ? "hevc"
                : "forceav1".equals(storedCodec) ? "av1" : "h264";
        String qualityMode = prefs.getString("adaptive_quality_mode", "off");
        String quality = "battery".equals(qualityMode) ? "save-power" : "off".equals(qualityMode) ? "off" : null;
        return new SimpleProfileModel.Current(nativeSize, fps, pacing,
                prefs.getBoolean(PreferenceConfiguration.VRR_PREF_STRING, false), latencySwitches, bitrateMilli,
                codec, prefs.getBoolean(PreferenceConfiguration.ENABLE_HDR_PREF_STRING, false),
                prefs.getBoolean(AutoBitrateChoice.SETTINGS_KEY, false), quality);
    }

    private static String pacing(String choice) {
        switch (choice) {
            case FramePacingChoice.LATENCY: return "latency";
            case FramePacingChoice.BALANCED:
            case FramePacingChoice.BALANCED_EVERY_FRAME: return "balanced";
            case FramePacingChoice.SMOOTHNESS: return "smoothest";
            default: return null;
        }
    }
}
