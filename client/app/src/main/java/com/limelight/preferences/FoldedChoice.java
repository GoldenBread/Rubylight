package com.limelight.preferences;

import android.content.SharedPreferences;

/**
 * Settings that were a switch and a list and are one list now, whose first value is Off: Vibration (Off, Subtle,
 * Standard, Strong), Trackpad acceleration (Off, Gentle, Medium, Strong) and Glide after a flick (Off, Short, Medium,
 * Long). The list keeps its key; the switch is retired. {@link #migrate} folds a stored switch into the list once: a
 * switch that was off becomes Off, one that was on keeps the list's stored value. Nothing changes in how a stream
 * behaves.
 */
final class FoldedChoice {
    static final String OFF = "off";

    /** Vibration: the switch was "Turn off vibration" (on means Off), the list "Rumble feel". */
    static final FoldedChoice VIBRATION = new FoldedChoice("checkbox_disable_rumble", true, false,
            "list_rumble_feel", "standard", "subtle", "standard", "strong");
    /** Trackpad acceleration: the switch was the setting (on means on), the list "Acceleration strength". */
    static final FoldedChoice ACCELERATION = new FoldedChoice("checkbox_trackpad_acceleration_v2", false, true,
            "list_trackpad_acceleration_curve", "medium", "gentle", "medium", "strong");
    /** Glide after a flick: the switch was the setting, the list "Glide distance". */
    static final FoldedChoice MOMENTUM = new FoldedChoice("checkbox_trackpad_momentum", false, false,
            "list_trackpad_momentum_glide", "medium", "short", "medium", "long");

    static final FoldedChoice[] ALL = {VIBRATION, ACCELERATION, MOMENTUM};

    /** The very first trackpad acceleration switch, forgotten: it could not tell an explicit off from its old default. */
    static final String RETIRED_ACCELERATION_V1 = "checkbox_trackpad_acceleration";

    final String switchKey, listKey;
    /** The switch's meaning: true when its On stood for Off. */
    final boolean switchOnIsOff;
    final boolean switchDefault;
    /** The list's value while the switch did not say Off, when none was stored. */
    final String listDefault;
    private final String[] values;

    private FoldedChoice(String switchKey, boolean switchOnIsOff, boolean switchDefault, String listKey,
                         String listDefault, String... values) {
        this.switchKey = switchKey;
        this.switchOnIsOff = switchOnIsOff;
        this.switchDefault = switchDefault;
        this.listKey = listKey;
        this.listDefault = listDefault;
        this.values = values;
    }

    private boolean valid(String value) {
        for (String own : values) {
            if (own.equals(value)) return true;
        }
        return false;
    }

    /** Whether the retired switch, as stored, said Off. */
    private boolean switchSaidOff(SharedPreferences prefs) {
        boolean on = prefs.getBoolean(switchKey, switchDefault);
        return switchOnIsOff == on;
    }

    /** The list's stored value while it is one of the choices (not Off), else the default one. */
    private String choice(SharedPreferences prefs) {
        String stored = prefs.getString(listKey, listDefault);
        return valid(stored) ? stored : listDefault;
    }

    /** The choice a stream uses: Off, or one of the values. Anything stored that is not a choice reads as the default. */
    String read(SharedPreferences prefs) {
        String stored = prefs.getString(listKey, null);
        if (stored == null) return this == MOMENTUM ? OFF : listDefault;
        if (OFF.equals(stored)) return OFF;
        return valid(stored) ? stored : (this == MOMENTUM ? OFF : listDefault);
    }

    boolean isOff(SharedPreferences prefs) {
        return OFF.equals(read(prefs));
    }

    /** The value to use while not Off. */
    String valueWhileOn(SharedPreferences prefs) {
        String value = read(prefs);
        return OFF.equals(value) ? listDefault : value;
    }

    /** Folds this setting's stored switch into its list and forgets the switch; returns whether it changed anything. */
    private boolean migrateOne(SharedPreferences prefs, SharedPreferences.Editor editor) {
        if (!prefs.contains(switchKey)) return false;
        editor.putString(listKey, switchSaidOff(prefs) ? OFF : choice(prefs)).remove(switchKey);
        return true;
    }

    /** Moves every stored switch into its list; changes nothing a second time. */
    static boolean migrate(SharedPreferences prefs) {
        SharedPreferences.Editor editor = prefs.edit();
        boolean changed = false;
        // The first acceleration switch is forgotten; it counted as On, whatever it held (the new default).
        if (prefs.contains(RETIRED_ACCELERATION_V1)) {
            editor.remove(RETIRED_ACCELERATION_V1);
            changed = true;
        }
        for (FoldedChoice folded : ALL) {
            changed |= folded.migrateOne(prefs, editor);
        }
        // The removed Temporal (SGSR 2) upscaling reads as SGSR.
        if ("sgsr2".equals(prefs.getString(PreferenceConfiguration.UPSCALING_PREF_STRING, null))) {
            editor.putString(PreferenceConfiguration.UPSCALING_PREF_STRING, "sgsr1");
            changed = true;
        }
        if (changed) editor.apply();
        return changed;
    }
}
