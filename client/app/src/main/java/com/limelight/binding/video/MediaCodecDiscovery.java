package com.limelight.binding.video;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileReader;
import java.util.Collections;
import java.util.LinkedList;
import java.util.Locale;

import android.annotation.SuppressLint;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaCodecList;
import android.os.Build;

import com.limelight.LimeLog;

/**
 * Cached {@link MediaCodecList} lookup, decoder discovery/selection, software/blacklist filtering,
 * and legacy Exynos 4 CPU detection supporting {@link MediaCodecHelper}.
 */
final class MediaCodecDiscovery {
    // Building a MediaCodecList is a blocking call to the media service that can stall for
    // seconds on some devices, and the list never changes within a process: read it once
    // (from V+ c519d37). A failed read throws and is not cached, so the next call tries again.
    private static final Object codecListLock = new Object();
    private static volatile MediaCodecInfo[] codecInfos;

    private MediaCodecDiscovery() {}

    @SuppressWarnings("deprecation")
    @SuppressLint("NewApi")
    static LinkedList<MediaCodecInfo> getMediaCodecList() {
        MediaCodecInfo[] infos = codecInfos;
        if (infos == null) {
            synchronized (codecListLock) {
                infos = codecInfos;
                if (infos == null) {
                    infos = new MediaCodecList(MediaCodecList.REGULAR_CODECS).getCodecInfos();
                    codecInfos = infos;
                }
            }
        }

        LinkedList<MediaCodecInfo> infoList = new LinkedList<>();
        Collections.addAll(infoList, infos);
        return infoList;
    }

    static void forgetCodecList() {
        codecInfos = null;
    }

    @SuppressWarnings("RedundantThrows")
    static String dumpDecoders() throws Exception {
        String str = "";
        for (MediaCodecInfo codecInfo : getMediaCodecList()) {
            // Skip encoders
            if (codecInfo.isEncoder()) {
                continue;
            }

            str += "Decoder: " + codecInfo.getName() + "\n";
            for (String type : codecInfo.getSupportedTypes()) {
                str += "\t" + type + "\n";
                CodecCapabilities caps = codecInfo.getCapabilitiesForType(type);

                for (CodecProfileLevel profile : caps.profileLevels) {
                    str += "\t\t" + profile.profile + " " + profile.level + "\n";
                }
            }
        }
        return str;
    }

    private static MediaCodecInfo findPreferredDecoder() {
        // This is a different algorithm than the other findXXXDecoder functions,
        // because we want to evaluate the decoders in our list's order
        // rather than MediaCodecList's order

        if (!MediaCodecHelper.isInitialized()) {
            throw new IllegalStateException("MediaCodecHelper must be initialized before use");
        }

        for (String preferredDecoder : MediaCodecCapabilities.preferredDecoders) {
            for (MediaCodecInfo codecInfo : getMediaCodecList()) {
                // Skip encoders
                if (codecInfo.isEncoder()) {
                    continue;
                }

                // Check for preferred decoders
                if (preferredDecoder.equalsIgnoreCase(codecInfo.getName())) {
                    LimeLog.info("Preferred decoder choice is " + codecInfo.getName());
                    return codecInfo;
                }
            }
        }

        return null;
    }

    /**
     * The decoder runs on the CPU: isSoftwareOnly() or not hardware accelerated on Android Q and
     * later, the stock software decoders' name prefixes below that.
     */
    static boolean isSoftwareDecoder(MediaCodecInfo codecInfo) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            return codecInfo.isSoftwareOnly() || !codecInfo.isHardwareAccelerated();
        }
        String name = codecInfo.getName().toLowerCase(Locale.ROOT);
        return name.startsWith("c2.android.") || name.startsWith("omx.google.") || name.startsWith("c2.google.");
    }

    private static boolean isCodecBlacklisted(MediaCodecInfo codecInfo) {
        // Use the new isSoftwareOnly() function on Android Q
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            if (!MediaCodecHelper.SHOULD_BYPASS_SOFTWARE_BLOCK && codecInfo.isSoftwareOnly()) {
                LimeLog.info("Skipping software-only decoder: " + codecInfo.getName());
                return true;
            }
        }

        // Check for explicitly blacklisted decoders
        if (MediaCodecHelper.isDecoderInList(MediaCodecCapabilities.blacklistedDecoderPrefixes, codecInfo.getName())) {
            LimeLog.info("Skipping blacklisted decoder: " + codecInfo.getName());
            return true;
        }

        return false;
    }

    static MediaCodecInfo findFirstDecoder(String mimeType) {
        for (MediaCodecInfo codecInfo : getMediaCodecList()) {
            // Skip encoders
            if (codecInfo.isEncoder()) {
                continue;
            }

            // Skip compatibility aliases on Q+
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                if (codecInfo.isAlias()) {
                    continue;
                }
            }

            // Find a decoder that supports the specified video format
            for (String mime : codecInfo.getSupportedTypes()) {
                if (mime.equalsIgnoreCase(mimeType)) {
                    // Skip blacklisted codecs
                    if (isCodecBlacklisted(codecInfo)) {
                        continue;
                    }

                    LimeLog.info("First decoder choice is " + codecInfo.getName());
                    return codecInfo;
                }
            }
        }

        return null;
    }

    /**
     * A hardware video/dolby-vision decoder for {@code profile}, or null: profile 8 (DvheSt) takes
     * an HEVC Main10 stream with Dolby Vision RPUs (NAL type 62), profile 10 (Dvav110) an AV1
     * Main10 stream with RPU metadata OBUs, and either maps it for the screen. Unlike
     * findProbableSafeDecoder, neither the preferred decoder names nor a capabilities exception
     * can hand back a decoder without that profile.
     */
    static MediaCodecInfo findDolbyVisionDecoder(int profile) {
        for (MediaCodecInfo codecInfo : getMediaCodecList()) {
            if (codecInfo.isEncoder()) {
                continue;
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && codecInfo.isAlias()) {
                continue;
            }
            for (String mime : codecInfo.getSupportedTypes()) {
                if (!mime.equalsIgnoreCase(HdrFormatPolicy.MIME_DOLBY_VISION) ||
                        isCodecBlacklisted(codecInfo) || isSoftwareDecoder(codecInfo)) {
                    continue;
                }
                try {
                    for (CodecProfileLevel level : codecInfo.getCapabilitiesForType(mime).profileLevels) {
                        if (level.profile == profile) {
                            return codecInfo;
                        }
                    }
                    LimeLog.info("Dolby Vision decoder " + codecInfo.getName() + " lacks profile " +
                            dolbyVisionProfileName(profile));
                } catch (IllegalArgumentException e) {
                    LimeLog.warning("Dolby Vision decoder " + codecInfo.getName() + " reports no capabilities: " + e);
                }
            }
        }
        return null;
    }

    /** "8" for DvheSt, "10" for Dvav110: the Dolby Vision profile findDolbyVisionDecoder looks for, for logs. */
    static String dolbyVisionProfileName(int profile) {
        return profile == CodecProfileLevel.DolbyVisionProfileDvav110 ? "10" :
                profile == CodecProfileLevel.DolbyVisionProfileDvheSt ? "8" : "0x" + Integer.toHexString(profile);
    }

    static MediaCodecInfo findProbableSafeDecoder(String mimeType, int requiredProfile) {
        // First look for a preferred decoder by name
        MediaCodecInfo info = findPreferredDecoder();
        if (info != null) {
            return info;
        }

        // Now look for decoders we know are safe
        try {
            // If this function completes, it will determine if the decoder is safe
            return findKnownSafeDecoder(mimeType, requiredProfile);
        } catch (Exception e) {
            // Some buggy devices seem to throw exceptions
            // from getCapabilitiesForType() so we'll just assume
            // they're okay and go with the first one we find
            return findFirstDecoder(mimeType);
        }
    }

    // We declare this method as explicitly throwing Exception
    // since some bad decoders can throw IllegalArgumentExceptions unexpectedly
    // and we want to be sure all callers are handling this possibility
    @SuppressWarnings("RedundantThrows")
    private static MediaCodecInfo findKnownSafeDecoder(String mimeType, int requiredProfile) throws Exception {
        // Some devices (Exynos devces, at least) have two sets of decoders.
        // The first set of decoders are C2 which do not support FEATURE_LowLatency,
        // but the second set of OMX decoders do support FEATURE_LowLatency. We want
        // to pick the OMX decoders despite the fact that C2 is listed first.
        // On some Qualcomm devices (like Pixel 4), there are separate low latency decoders
        // (like c2.qti.hevc.decoder.low_latency) that advertise FEATURE_LowLatency while
        // the standard ones (like c2.qti.hevc.decoder) do not. Like Exynos, the decoders
        // with FEATURE_LowLatency support are listed after the standard ones.
        for (int i = 0; i < 2; i++) {
            for (MediaCodecInfo codecInfo : getMediaCodecList()) {
                // Skip encoders
                if (codecInfo.isEncoder()) {
                    continue;
                }

                // Skip compatibility aliases on Q+
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    if (codecInfo.isAlias()) {
                        continue;
                    }
                }

                // Find a decoder that supports the requested video format
                for (String mime : codecInfo.getSupportedTypes()) {
                    if (mime.equalsIgnoreCase(mimeType)) {
                        LimeLog.info("Examining decoder capabilities of " + codecInfo.getName() + " (round " + (i + 1) + ")");

                        // Skip blacklisted codecs
                        if (isCodecBlacklisted(codecInfo)) {
                            continue;
                        }

                        CodecCapabilities caps = codecInfo.getCapabilitiesForType(mime);

                        if (i == 0 && !MediaCodecCapabilities.decoderSupportsAndroidRLowLatency(codecInfo, mime)) {
                            LimeLog.info("Skipping decoder that lacks FEATURE_LowLatency for round 1");
                            continue;
                        }

                        if (requiredProfile != -1) {
                            for (CodecProfileLevel profile : caps.profileLevels) {
                                if (profile.profile == requiredProfile) {
                                    LimeLog.info("Decoder " + codecInfo.getName() + " supports required profile");
                                    return codecInfo;
                                }
                            }

                            LimeLog.info("Decoder " + codecInfo.getName() + " does NOT support required profile");
                        } else {
                            return codecInfo;
                        }
                    }
                }
            }
        }

        return null;
    }

    static String readCpuinfo() throws Exception {
        StringBuilder cpuInfo = new StringBuilder();
        try (final BufferedReader br = new BufferedReader(new FileReader(new File("/proc/cpuinfo")))) {
            for (;;) {
                int ch = br.read();
                if (ch == -1)
                    break;
                cpuInfo.append((char) ch);
            }

            return cpuInfo.toString();
        }
    }

    private static boolean stringContainsIgnoreCase(String string, String substring) {
        return string.toLowerCase(Locale.ENGLISH).contains(substring.toLowerCase(Locale.ENGLISH));
    }

    static boolean isExynos4Device() {
        try {
            // Try reading CPU info too look for
            String cpuInfo = readCpuinfo();

            // SMDK4xxx is Exynos 4
            if (stringContainsIgnoreCase(cpuInfo, "SMDK4")) {
                LimeLog.info("Found SMDK4 in /proc/cpuinfo");
                return true;
            }

            // If we see "Exynos 4" also we'll count it
            if (stringContainsIgnoreCase(cpuInfo, "Exynos 4")) {
                LimeLog.info("Found Exynos 4 in /proc/cpuinfo");
                return true;
            }
        } catch (Exception e) {
            e.printStackTrace();
        }

        try {
            File systemDir = new File("/sys/devices/system");
            File[] files = systemDir.listFiles();
            if (files != null) {
                for (File f : files) {
                    if (stringContainsIgnoreCase(f.getName(), "exynos4")) {
                        LimeLog.info("Found exynos4 in /sys/devices/system");
                        return true;
                    }
                }
            }
        } catch (Exception e) {
            e.printStackTrace();
        }

        return false;
    }
}
