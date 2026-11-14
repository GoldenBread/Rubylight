package com.limelight.preferences;

import android.content.SharedPreferences;

/**
 * Frame pacing as one choice: Lowest latency, Balanced, Balanced keeping every frame, Balanced with FPS limit,
 * Smoothest video, or the PC's frame times. It is stored as before, so older values and the stream
 * configuration keep working: the pacing in {@code frame_pacing} (one of its four values), Balanced's
 * "Keep only the newest frame" in {@code checkbox_drop_late_frames} (on by default) and the PC's frame times in
 * {@code checkbox_host_frame_times} (off by default). Choosing the PC's frame times leaves the pacing as it was,
 * so choosing a pacing again starts from it; the newest-frame choice is written only by the two Balanced choices.
 * Desktop Nightjar's row writes {@code frame_pacing}, {@code drop_late_frames} and {@code host_frame_times} the
 * same way.
 */
final class FramePacingChoice {
    static final String LATENCY = "latency";
    static final String BALANCED = "balanced";
    static final String BALANCED_EVERY_FRAME = "balanced-every";
    static final String CAP_FPS = "cap-fps";
    static final String SMOOTHNESS = "smoothness";
    static final String HOST_TIMES = "host";
    /** The values of {@code frame_pacing_choice_values}, in the order of the picker. */
    static final String[] VALUES = {LATENCY, BALANCED, BALANCED_EVERY_FRAME, CAP_FPS, SMOOTHNESS, HOST_TIMES};

    private FramePacingChoice() { }

    /** The choice for a stored pacing, newest-frame switch and PC's-frame-times switch. */
    static String of(String pacing, boolean dropLateFrames, boolean hostFrameTimes) {
        if (hostFrameTimes) return HOST_TIMES;
        if (BALANCED.equals(pacing)) return dropLateFrames ? BALANCED : BALANCED_EVERY_FRAME;
        for (String known : HostStreamProfile.PACING_VALUES) {
            if (known.equals(pacing)) return pacing;
        }
        // PreferenceConfiguration.framePacingValue reads anything else as Lowest latency.
        return LATENCY;
    }

    /** The stored choice. */
    static String read(SharedPreferences prefs) {
        return of(prefs.getString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, PreferenceConfiguration.DEFAULT_FRAME_PACING),
                prefs.getBoolean(SettingRules.DROP_LATE_FRAMES, true),
                prefs.getBoolean(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING, false));
    }

    /** The choice a stream's configuration runs, after VRR and every layer. */
    static String of(PreferenceConfiguration config) {
        if (config.hostFrameTimes && !config.vrr) return HOST_TIMES;
        switch (config.framePacing) {
            case PreferenceConfiguration.FRAME_PACING_BALANCED:
                return config.dropLateFrames ? BALANCED : BALANCED_EVERY_FRAME;
            case PreferenceConfiguration.FRAME_PACING_CAP_FPS:
                return CAP_FPS;
            case PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS:
                return SMOOTHNESS;
            default:
                return LATENCY;
        }
    }

    /** Whether {@code value} is one of {@link #VALUES}. */
    static boolean known(String value) {
        return index(value) >= 0;
    }

    /** The position of {@code value} in {@link #VALUES}, or -1. */
    static int index(String value) {
        for (int i = 0; i < VALUES.length; i++) {
            if (VALUES[i].equals(value)) return i;
        }
        return -1;
    }

    /** The pacing {@code frame_pacing} holds for a choice; null for the PC's frame times, which keeps it. */
    static String pacing(String choice) {
        if (HOST_TIMES.equals(choice)) return null;
        return BALANCED_EVERY_FRAME.equals(choice) ? BALANCED : choice;
    }

    /** The newest-frame switch a choice sets: Balanced on, Balanced keeping every frame off, null for the others. */
    static Boolean dropLateFrames(String choice) {
        if (BALANCED.equals(choice)) return true;
        if (BALANCED_EVERY_FRAME.equals(choice)) return false;
        return null;
    }

    /** Writes a choice of {@link #VALUES}; anything else writes nothing. */
    static void save(SharedPreferences.Editor editor, String choice) {
        if (!known(choice)) return;
        editor.putBoolean(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING, HOST_TIMES.equals(choice));
        String pacing = pacing(choice);
        if (pacing != null) editor.putString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, pacing);
        Boolean newest = dropLateFrames(choice);
        if (newest != null) editor.putBoolean(SettingRules.DROP_LATE_FRAMES, newest);
    }

    /**
     * Puts the choice on a PC, game or profile layer with its values together, or follows the layers below when
     * null. The PC's frame times keep Lowest latency as the layer's pacing (what the PC's frame times run, and
     * what an older build reads from it); the newest-frame switch is set only by the two Balanced choices.
     */
    static void save(HostStreamProfile own, String choice) {
        if (choice == null || !known(choice)) {
            own.pacing = null;
            own.hostFrameTimes = null;
            own.dropLateFrames = null;
            return;
        }
        String pacing = pacing(choice);
        own.pacing = pacing == null ? LATENCY : pacing;
        own.hostFrameTimes = HOST_TIMES.equals(choice);
        own.dropLateFrames = dropLateFrames(choice);
    }
}
