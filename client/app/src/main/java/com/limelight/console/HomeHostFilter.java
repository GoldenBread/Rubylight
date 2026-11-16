package com.limelight.console;

import android.content.Context;
import android.preference.PreferenceManager;

/**
 * Which PC's games Home shows: one PC's, or every PC's. Kept in the app's preferences, every PC's until chosen;
 * a PC that is removed or unknown falls back to every PC's in {@link HomeModel}.
 */
public final class HomeHostFilter {
    static final String PREF_KEY = "console_home_only_pc";

    private HomeHostFilter() {
    }

    /** The chosen PC's UUID, or null for every PC's. */
    public static String chosen(Context context) {
        String uuid = PreferenceManager.getDefaultSharedPreferences(context).getString(PREF_KEY, null);
        return uuid == null || uuid.isEmpty() ? null : uuid;
    }

    /** Shows only this PC's games ({@code uuid}), or every PC's (null). */
    public static void set(Context context, String uuid) {
        if (uuid == null || uuid.isEmpty()) {
            PreferenceManager.getDefaultSharedPreferences(context).edit().remove(PREF_KEY).apply();
        } else {
            PreferenceManager.getDefaultSharedPreferences(context).edit().putString(PREF_KEY, uuid).apply();
        }
    }
}
