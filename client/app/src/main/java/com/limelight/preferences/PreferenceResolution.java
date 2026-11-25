package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.hardware.display.DisplayManager;
import android.preference.PreferenceManager;
import android.view.Display;

import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.nvstream.PyroWaveBitrateFloor;

import static com.limelight.preferences.SettingsCatalogGen.*;

/**
 * Resolution, display-shape, bitrate, and stored-value parsing helpers for {@link PreferenceConfiguration}.
 */
final class PreferenceResolution {
    private PreferenceResolution() {}

    static boolean isNativeResolution(int width, int height) {
        if (width == 640 && height == 360) {
            return false;
        }
        else if (width == 854 && height == 480) {
            return false;
        }
        else if (width == 1280 && height == 720) {
            return false;
        }
        else if (width == 1920 && height == 1080) {
            return false;
        }
        else if (width == 2560 && height == 1440) {
            return false;
        }
        else if (width == 3840 && height == 2160) {
            return false;
        }
        return true;
    }

    static int[][] panelSizes(int[][] modes) {
        java.util.List<int[]> sizes = new java.util.ArrayList<>();
        for (int[] mode : modes) {
            if (mode[0] <= 0 || mode[1] <= 0) continue;
            int shape = -1;
            for (int i = 0; i < sizes.size() && shape < 0; i++) {
                if (sameShape(mode[0], mode[1], sizes.get(i)[0], sizes.get(i)[1], 0.01)) shape = i;
            }
            if (shape < 0) sizes.add(mode);
            else if (area(mode) > area(sizes.get(shape))) sizes.set(shape, mode);
        }
        return sizes.toArray(new int[0][]);
    }

    static boolean isSquarishScreen(int width, int height) {
        float longDim = Math.max(width, height);
        float shortDim = Math.min(width, height);
        return longDim / shortDim < 1.3f;
    }

    static boolean isSquarishScreen(Display display) {
        int width = display.getMode().getPhysicalWidth();
        int height = display.getMode().getPhysicalHeight();
        return isSquarishScreen(width, height);
    }

    static String convertFromLegacyResolutionString(String resString) {
        if (resString.equalsIgnoreCase("360p")) {
            return RES_360P;
        }
        else if (resString.equalsIgnoreCase("480p")) {
            return RES_480P;
        }
        else if (resString.equalsIgnoreCase("720p")) {
            return RES_720P;
        }
        else if (resString.equalsIgnoreCase("1080p")) {
            return RES_1080P;
        }
        else if (resString.equalsIgnoreCase("1440p")) {
            return RES_1440P;
        }
        else if (resString.equalsIgnoreCase("4K")) {
            return RES_4K;
        }
        else {
            return RES_720P;
        }
    }

    static int getWidthFromResolutionString(String resString) {
        return Integer.parseInt(resString.split("x")[0]);
    }

    static int getHeightFromResolutionString(String resString) {
        return Integer.parseInt(resString.split("x")[1]);
    }

    static String getResolutionText(int width, int height) {
        return width + "x" + height;
    }

    static String getResolutionString(int width, int height) {
        switch (height) {
            case 360:
                return RES_360P;
            case 480:
                return RES_480P;
            default:
            case 720:
                return RES_720P;
            case 1080:
                return RES_1080P;
            case 1440:
                return RES_1440P;
            case 2160:
                return RES_4K;
        }
    }

    static int getDefaultBitrate(String resString, String fpsString) {
        int width = getWidthFromResolutionString(resString);
        int height = getHeightFromResolutionString(resString);
        int fps = Integer.parseInt(fpsString);

        // This logic is shamelessly stolen from Moonlight Qt:
        // https://github.com/moonlight-stream/moonlight-qt/blob/master/app/settings/streamingpreferences.cpp
        double frameRateFactor = (fps <= 60 ? fps : (Math.sqrt(fps / 60.f) * 60.f)) / 30.f;

        int[] pixelVals = {
            640 * 360,
            854 * 480,
            1280 * 720,
            1920 * 1080,
            2560 * 1440,
            3840 * 2160,
            -1,
        };
        int[] factorVals = {
            1,
            2,
            5,
            10,
            20,
            40,
            -1
        };

        float resolutionFactor;
        int pixels = width * height;
        for (int i = 0; ; i++) {
            if (pixels == pixelVals[i]) {
                resolutionFactor = factorVals[i];
                break;
            }
            else if (pixels < pixelVals[i]) {
                if (i == 0) {
                    resolutionFactor = factorVals[i];
                }
                else {
                    resolutionFactor = ((float)(pixels - pixelVals[i-1]) / (pixelVals[i] - pixelVals[i-1])) * (factorVals[i] - factorVals[i-1]) + factorVals[i-1];
                }
                break;
            }
            else if (pixelVals[i] == -1) {
                resolutionFactor = factorVals[i-1];
                break;
            }
        }

        return (int)Math.round(resolutionFactor * frameRateFactor) * 1000;
    }

    static boolean getDefaultSmallMode(Context context) {
        PackageManager manager = context.getPackageManager();
        if (manager != null) {
            if (manager.hasSystemFeature(PackageManager.FEATURE_TELEVISION)) {
                return false;
            }
            if (manager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)) {
                return false;
            }
        }
        return context.getResources().getConfiguration().smallestScreenWidthDp < 500;
    }

    static boolean isBitrateUserSet(SharedPreferences prefs) {
        if (prefs.contains(BITRATE_USER_SET_PREF_STRING)) {
            return prefs.getBoolean(BITRATE_USER_SET_PREF_STRING, false);
        }
        return prefs.contains(BITRATE_PREF_STRING) && prefs.getInt(BITRATE_PREF_STRING, 0) != getDefaultBitrate(
                prefs.getString(RESOLUTION_PREF_STRING, DEFAULT_RESOLUTION), prefs.getString(FPS_PREF_STRING, DEFAULT_FPS));
    }

    static int getShownDefaultBitrate(SharedPreferences prefs, String res, String fps, String codec) {
        if (res == null) res = prefs.getString(RESOLUTION_PREF_STRING, DEFAULT_RESOLUTION);
        if (fps == null) fps = prefs.getString(FPS_PREF_STRING, DEFAULT_FPS);
        if (codec == null) codec = prefs.getString(VIDEO_FORMAT_PREF_STRING, DEFAULT_VIDEO_FORMAT);
        if ("forcepyrowave".equals(codec)) {
            return PyroWaveBitrateFloor.defaultKbps(getWidthFromResolutionString(res),
                    getHeightFromResolutionString(res), Integer.parseInt(fps), 0);
        }
        return getDefaultBitrate(res, fps);
    }

    static int pyroWaveMinimumAboveChosenKbps(PreferenceConfiguration config) {
        if (config.videoFormat != PreferenceConfiguration.FormatOption.FORCE_PYROWAVE || config.automaticBitrate) {
            return 0;
        }
        int minimum = PyroWaveBitrateFloor.minimumKbps(config.width, config.height, config.fps);
        return config.bitrate < minimum ? minimum : 0;
    }

    static String pyroWaveBelowMinimumWarning(Context context, PreferenceConfiguration config) {
        int minimum = pyroWaveMinimumAboveChosenKbps(config);
        if (minimum == 0) return null;
        return context.getString(com.limelight.R.string.summary_bitrate_pyrowave_below_minimum,
                (minimum + 999) / 1000, config.width, config.height, config.fps);
    }

    static PreferenceConfiguration.FormatOption getVideoFormatValue(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        String str = prefs.getString(VIDEO_FORMAT_PREF_STRING, DEFAULT_VIDEO_FORMAT);
        if (str.equals("auto")) {
            return PreferenceConfiguration.FormatOption.AUTO;
        }
        else if (str.equals("forceav1")) {
            return PreferenceConfiguration.FormatOption.FORCE_AV1;
        }
        else if (str.equals("forceh265")) {
            return PreferenceConfiguration.FormatOption.FORCE_HEVC;
        }
        else if (str.equals("neverh265")) {
            return PreferenceConfiguration.FormatOption.FORCE_H264;
        }
        else if (str.equals("forcepyrowave")) {
            return PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
        }
        else {
            return PreferenceConfiguration.FormatOption.AUTO;
        }
    }

    static int getFramePacingValue(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        if (prefs.contains(LEGACY_DISABLE_FRAME_DROP_PREF_STRING)) {
            boolean legacyNeverDropFrames = prefs.getBoolean(LEGACY_DISABLE_FRAME_DROP_PREF_STRING, false);
            prefs.edit()
                    .remove(LEGACY_DISABLE_FRAME_DROP_PREF_STRING)
                    .putString(FRAME_PACING_PREF_STRING, legacyNeverDropFrames ? "balanced" : "latency")
                    .apply();
        }
        return framePacingValue(prefs.getString(FRAME_PACING_PREF_STRING, DEFAULT_FRAME_PACING));
    }

    static int decodeQueueFrames(String stored) {
        try {
            int frames = Integer.parseInt(stored.trim());
            return frames >= 1 && frames <= DEFAULT_DECODE_QUEUE_FRAMES ? frames : DEFAULT_DECODE_QUEUE_FRAMES;
        } catch (NumberFormatException e) {
            return DEFAULT_DECODE_QUEUE_FRAMES;
        }
    }

    static int mobileBitrateLimit(String stored) {
        try {
            int kbps = Integer.parseInt(stored.trim());
            return kbps < 500 ? 0 : Math.min(kbps, HostStreamProfile.MAX_BITRATE_KBPS);
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    static boolean isVirtualDisplayDpi(int percent) {
        for (int offered : VIRTUAL_DISPLAY_DPI_VALUES) {
            if (offered == percent) return true;
        }
        return false;
    }

    static int virtualDisplayDpi(String stored) {
        try {
            int percent = Integer.parseInt(stored.trim());
            return isVirtualDisplayDpi(percent) ? percent : 0;
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    static int framePacingValue(String str) {
        if (str.equals("latency")) {
            return FRAME_PACING_MIN_LATENCY;
        }
        else if (str.equals("balanced")) {
            return FRAME_PACING_BALANCED;
        }
        else if (str.equals("cap-fps")) {
            return FRAME_PACING_CAP_FPS;
        }
        else if (str.equals("smoothness")) {
            return FRAME_PACING_MAX_SMOOTHNESS;
        }
        else {
            return FRAME_PACING_MIN_LATENCY;
        }
    }

    static PreferenceConfiguration.AnalogStickForScrolling getAnalogStickForScrollingValue(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        String str = prefs.getString(ANALOG_SCROLLING_PREF_STRING, DEFAULT_ANALOG_STICK_FOR_SCROLLING);
        if (str.equals("right")) {
            return PreferenceConfiguration.AnalogStickForScrolling.RIGHT;
        }
        else if (str.equals("left")) {
            return PreferenceConfiguration.AnalogStickForScrolling.LEFT;
        }
        else {
            return PreferenceConfiguration.AnalogStickForScrolling.NONE;
        }
    }

    static int guideMenuHoldMs(String stored) {
        if (stored == null) return DEFAULT_GUIDE_MENU_HOLD_MS;
        switch (stored) {
            case "0": case "500": case "1000": case "1500": case "2000": return Integer.parseInt(stored);
            default: return DEFAULT_GUIDE_MENU_HOLD_MS;
        }
    }

    static int listChoice(String stored, int[] choices, int fallback) {
        try {
            int value = Integer.parseInt(stored.trim());
            for (int choice : choices) {
                if (choice == value) return value;
            }
        } catch (NumberFormatException | NullPointerException ignored) {
        }
        return fallback;
    }

    static String nativeResolution(Context context) {
        return nativeResolution(defaultDisplay(context));
    }

    static String nativeResolution(Display display) {
        if (display == null) {
            return null;
        }
        android.graphics.Point real = new android.graphics.Point();
        display.getRealSize(real);
        Display.Mode[] supported = display.getSupportedModes();
        int[][] modes = new int[supported.length][];
        for (int i = 0; i < supported.length; i++) {
            modes[i] = new int[] {supported[i].getPhysicalWidth(), supported[i].getPhysicalHeight()};
        }
        int[] size = panelSize(real.x, real.y, modes);
        if (size == null || size[0] <= 0 || size[1] <= 0) {
            return null;
        }
        return Math.max(size[0], size[1]) + "x" + Math.min(size[0], size[1]);
    }

    static int[] panelSize(int realWidth, int realHeight, int[][] modes) {
        int[] largest = null, matching = null;
        for (int[] mode : modes) {
            if (mode[0] <= 0 || mode[1] <= 0) continue;
            if (largest == null || area(mode) > area(largest)) largest = mode;
            if (sameShape(mode[0], mode[1], realWidth, realHeight, 0.01) && (matching == null || area(mode) > area(matching))) {
                matching = mode;
            }
        }
        return matching != null ? matching : largest;
    }

    private static long area(int[] size) {
        return (long) size[0] * size[1];
    }

    static boolean hasPanelShape(Context context, int width, int height, double tolerance) {
        String panel = nativeResolution(context);
        return panel != null && sameShape(width, height, getWidthFromResolutionString(panel),
                getHeightFromResolutionString(panel), tolerance);
    }

    static boolean sameShape(int width, int height, int otherWidth, int otherHeight, double tolerance) {
        if (width <= 0 || height <= 0 || otherWidth <= 0 || otherHeight <= 0) return false;
        double ratio = (double) Math.max(width, height) / Math.min(width, height);
        double other = (double) Math.max(otherWidth, otherHeight) / Math.min(otherWidth, otherHeight);
        return Math.abs(ratio - other) <= other * tolerance;
    }

    static float panelMaxRefreshRate(Context context) {
        Display display = defaultDisplay(context);
        return display == null ? 60 : DisplayFrameRatePolicy.maxRefreshRate(display);
    }

    static Display defaultDisplay(Context context) {
        DisplayManager displays = (DisplayManager) context.getSystemService(Context.DISPLAY_SERVICE);
        return displays == null ? null : displays.getDisplay(Display.DEFAULT_DISPLAY);
    }

    static String streamResolution(Context context, SharedPreferences prefs) {
        String saved = prefs.getString(RESOLUTION_PREF_STRING, DEFAULT_RESOLUTION);
        if (!saved.contains("x")) {
            saved = convertFromLegacyResolutionString(saved);
        }
        return effectiveResolution(prefs, saved, nativeResolution(context));
    }

    static String effectiveResolution(SharedPreferences prefs, String saved, String nativeResolution) {
        return prefs.getBoolean(RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false) && nativeResolution != null ?
                nativeResolution : saved;
    }
}
