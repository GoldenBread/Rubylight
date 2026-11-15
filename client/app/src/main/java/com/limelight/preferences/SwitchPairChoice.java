package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.R;

/**
 * Rows that were two switches each and are one choice now, as on desktop Nightjar (nightjar_policy::choices). Each
 * is still saved in its two switches, so presets, older versions and everything that reads them keep working, and a
 * pair saved before reads as the stream used it. The row's key names it and stores nothing of its own beyond the
 * first switch.
 */
enum SwitchPairChoice {
    /**
     * Off, When the network is busy, When frames are lost. Frame loss took the place of the busy network wherever the
     * PC took its rate changes, so either pair with it on reads as When frames are lost; choosing that keeps the busy
     * controller on for a PC that takes no rate changes (GameAdaptation.startStreamBitrate).
     */
    LOWER_BITRATE(AutoBitrateChoice.SETTINGS_KEY, R.string.title_lower_bitrate, R.string.summary_lower_bitrate,
            R.array.lower_bitrate_names, AutoBitrateChoice.SETTINGS_KEY, false,
            PreferenceConfiguration.RATE_CHANGE_BITRATE_PREF_STRING, false) {
        @Override int choice(boolean busy, boolean lost) { return lost ? 2 : busy ? 1 : 0; }
        @Override boolean first(int choice) { return choice != 0; }
        @Override boolean second(int choice) { return choice == 2; }
    },
    /** Back, Stream menu, Guide. With both switches on the menu won (GameKeyboardInput.handlePhysicalBackMenuKey). */
    BACK_BUTTON("back_button", R.string.title_back_button, R.string.summary_back_button, R.array.back_button_names,
            PreferenceConfiguration.BACK_OPENS_MENU_PREF_STRING, false, PreferenceConfiguration.BACK_AS_GUIDE_PREF_STRING, false) {
        @Override int choice(boolean menu, boolean guide) { return menu ? 1 : guide ? 2 : 0; }
        @Override boolean first(int choice) { return choice == 1; }
        @Override boolean second(int choice) { return choice == 2; }
    },
    /** Off, Always, In full screen. Full screen only counted while the keys were sent (PreferenceConfiguration.sendsSystemKeys). */
    SYSTEM_KEYS(PreferenceConfiguration.CAPTURE_SYSTEM_KEYS_PREF_STRING, R.string.title_checkbox_capture_system_keys,
            R.string.summary_system_keys, R.array.system_keys_names, PreferenceConfiguration.CAPTURE_SYSTEM_KEYS_PREF_STRING, true,
            PreferenceConfiguration.SYSTEM_KEYS_FULLSCREEN_ONLY_PREF_STRING, false) {
        @Override int choice(boolean send, boolean fullScreen) { return !send ? 0 : fullScreen ? 2 : 1; }
        @Override boolean first(int choice) { return choice != 0; }
        @Override boolean second(int choice) { return choice == 2; }
    },
    /** Off, Except mobile data, Any network. Mobile data only counted while roaming was on (NvConnection). */
    ROAMING(PreferenceConfiguration.CLIENT_ROAMING_PREF_STRING, R.string.title_client_roaming, R.string.summary_roaming,
            R.array.roaming_names, PreferenceConfiguration.CLIENT_ROAMING_PREF_STRING, true,
            PreferenceConfiguration.ROAMING_METERED_PREF_STRING, false) {
        @Override int choice(boolean roaming, boolean metered) { return !roaming ? 0 : metered ? 2 : 1; }
        @Override boolean first(int choice) { return choice != 0; }
        @Override boolean second(int choice) { return choice == 2; }
    };

    /** The row's key. */
    final String key;
    final int title, help, names;
    final String firstKey, secondKey;
    final boolean firstDefault, secondDefault;

    SwitchPairChoice(String key, int title, int help, int names, String firstKey, boolean firstDefault,
                     String secondKey, boolean secondDefault) {
        this.key = key;
        this.title = title;
        this.help = help;
        this.names = names;
        this.firstKey = firstKey;
        this.firstDefault = firstDefault;
        this.secondKey = secondKey;
        this.secondDefault = secondDefault;
    }

    /** The choice, as its index in {@link #names}, that the two switches make. */
    abstract int choice(boolean first, boolean second);

    abstract boolean first(int choice);

    abstract boolean second(int choice);

    int read(SharedPreferences prefs) {
        return choice(prefs.getBoolean(firstKey, firstDefault), prefs.getBoolean(secondKey, secondDefault));
    }

    /**
     * How many of the choices the row offers, from the first: all of them, except roaming's Any network on a device
     * without mobile data, unless it is the stored choice (rows about mobile data, SettingRules.hasMobileData).
     */
    int offered(Context context, SharedPreferences prefs) {
        if (this == ROAMING && read(prefs) != 2 && !SettingRules.hasMobileData(context)) return 2;
        return 3;
    }

    void save(SharedPreferences.Editor editor, int choice) {
        editor.putBoolean(firstKey, first(choice)).putBoolean(secondKey, second(choice));
    }

    /** The choice a row's key names, or null. */
    static SwitchPairChoice forKey(String key) {
        for (SwitchPairChoice choice : values()) {
            if (choice.key.equals(key)) return choice;
        }
        return null;
    }
}
