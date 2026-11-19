package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

/**
 * Whether Lower bitrate when the network is busy is on for a PC. The Settings value (Lower bitrate's
 * busy switch, SwitchPairChoice) is the default for every PC; the stream menu's toggle saves a choice
 * for one PC (separately for conventional codecs and PyroWave, which have their own controllers), and
 * that choice wins over the default. Changing the Settings value drops the PCs' own choices, so it
 * takes effect everywhere at once.
 */
public final class AutoBitrateChoice {
    /** The Settings switch, stored with the main settings; checkbox_auto_bitrate before (RENAMED_PREFS). */
    public static final String SETTINGS_KEY = "adaptive_bitrate";
    public static final String CODEC_STORE = "AdaptiveBitrate";
    public static final String PYROWAVE_STORE = "PyroWaveBitrate";
    private static final String[] STORES = {CODEC_STORE, PYROWAVE_STORE};

    private AutoBitrateChoice() {
    }

    public static boolean isOn(Context context, String store, String pcUuid) {
        SharedPreferences saved = context.getSharedPreferences(store, Context.MODE_PRIVATE);
        if (pcUuid != null && saved.contains(pcUuid)) {
            return saved.getBoolean(pcUuid, false);
        }
        return byDefault(context);
    }

    /** The Settings switch, which reads as off while "Never: keep my bitrate" is chosen. */
    private static boolean byDefault(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        return !prefs.getBoolean(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING, false) && prefs.getBoolean(SETTINGS_KEY, false);
    }

    /** The stream menu's toggle: this PC only. Only a choice that differs from the Settings switch is kept. */
    public static void setForPc(Context context, String store, String pcUuid, boolean on) {
        boolean byDefault = byDefault(context);
        SharedPreferences.Editor edit = context.getSharedPreferences(store, Context.MODE_PRIVATE).edit();
        if (on == byDefault) {
            edit.remove(pcUuid);
        } else {
            edit.putBoolean(pcUuid, on);
        }
        edit.apply();
    }

    /**
     * The stream menu's Keep my bitrate: Lower bitrate's Never: keep my bitrate when on, The PC decides when off, written
     * as choosing them in Settings writes them (SwitchPairChoice). Only these keys: as the performance overlay's switch, it
     * leaves every other stored value, PCs' own choices included.
     */
    public static void storeKeepBitrate(SharedPreferences.Editor editor, boolean keep) {
        SwitchPairChoice.LOWER_BITRATE.save(editor, keep ? SwitchPairChoice.KEEP : 0);
    }

    /** The Settings switch changed: every PC follows it again. */
    public static void clearPcChoices(Context context) {
        for (String store : STORES) {
            context.getSharedPreferences(store, Context.MODE_PRIVATE).edit().clear().apply();
        }
    }

    public static void forget(Context context, String pcUuid) {
        for (String store : STORES) {
            context.getSharedPreferences(store, Context.MODE_PRIVATE).edit().remove(pcUuid).apply();
        }
    }
}
