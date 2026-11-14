package com.limelight.preferences;

import android.content.SharedPreferences;

/**
 * Lower refresh rate as one choice: Off, On (the lowest rate that fits the stream) or On, matched to the game (the
 * rate that fits the game's own frame rate). It is stored as before, in {@code checkbox_reduce_refresh_rate} and
 * {@code checkbox_content_rate_frame_rate}, both off by default, so older values and the stream configuration keep
 * working: the game's rate only ever acted with a lower refresh rate allowed. Off leaves the second switch as it
 * was, as Variable refresh rate's Off leaves Real VRR (VrrChoice).
 */
final class LowerRefreshChoice {
    static final int OFF = 0;
    static final int ON = 1;
    static final int MATCHED = 2;
    /** The values of {@code lower_refresh_choice_values}, in the order of the picker. */
    static final String[] VALUES = {"off", "on", "matched"};

    static final String REDUCE_REFRESH_RATE = "checkbox_reduce_refresh_rate";
    static final String CONTENT_RATE = PreferenceConfiguration.CONTENT_RATE_FRAME_RATE_PREF_STRING;

    private LowerRefreshChoice() { }

    static int of(boolean reduceRefreshRate, boolean contentRate) {
        return !reduceRefreshRate ? OFF : contentRate ? MATCHED : ON;
    }

    static int read(SharedPreferences prefs) {
        return of(prefs.getBoolean(REDUCE_REFRESH_RATE, false), prefs.getBoolean(CONTENT_RATE, false));
    }

    /** The choice for a value of {@link #VALUES}; an unknown one reads as Off. */
    static int ofValue(String value) {
        for (int i = 0; i < VALUES.length; i++) {
            if (VALUES[i].equals(value)) return i;
        }
        return OFF;
    }

    static void save(SharedPreferences.Editor editor, int choice) {
        editor.putBoolean(REDUCE_REFRESH_RATE, choice != OFF);
        if (choice != OFF) editor.putBoolean(CONTENT_RATE, choice == MATCHED);
    }
}
