package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import java.util.Map;

/**
 * Lower bitrate, the one adaptive bitrate setting: The PC decides ({@link #PC}), Lower it sooner ({@link #SOONER}: this
 * device lowers the bitrate as soon as frames go missing, on a PC that takes its rate changes, and runs its own
 * controller while the connection falls behind elsewhere) or Never: keep my bitrate ({@link #KEEP}). The Settings value
 * holds for every PC; the stream menu saves a choice for one PC, which wins over it. Changing the Settings value drops
 * the PCs' own choices, so it takes effect everywhere at once.
 *
 * <p>Earlier versions kept it in three switches ({@link #RETIRED_BUSY}, {@link #RETIRED_LOST}, {@link #RETIRED_KEEP})
 * and the PCs' own choices of the busy controller in two stores, for conventional codecs and PyroWave. {@link #migrate}
 * moves them here once; until it has run, stored switches win (an older version may have written them).
 */
public final class AutoBitrateChoice {
    /** The Settings value, stored with the main settings. */
    public static final String SETTINGS_KEY = "lower_bitrate";
    public static final String PC = "pc";
    public static final String SOONER = "sooner";
    public static final String KEEP = "keep";
    /** The PCs' own choices: a value of {@link #SETTINGS_KEY} by PC id. */
    public static final String PC_STORE = "LowerBitrate";

    /** Lower bitrate when the network is busy (checkbox_auto_bitrate before it): the busy controller. */
    static final String RETIRED_BUSY = "adaptive_bitrate";
    /** Lower bitrate when frames are lost: the device's own loss reports over rate changes. */
    static final String RETIRED_LOST = "checkbox_rate_change_bitrate";
    /** Never: keep my bitrate, which won over the two switches. */
    static final String RETIRED_KEEP = "keep_bitrate";
    /** The PCs' own choices of the busy controller, for conventional codecs (first) and PyroWave. */
    static final String[] RETIRED_STORES = {"AdaptiveBitrate", "PyroWaveBitrate"};

    private AutoBitrateChoice() {
    }

    /**
     * The choice three switches made: Never won; either switch on lowered the bitrate before the PC did (When the
     * network is busy, which only listed while stored, folds into Lower it sooner); otherwise the PC decided.
     */
    static String fromSwitches(boolean busy, boolean lost, boolean keep) {
        if (keep) return KEEP;
        return busy || lost ? SOONER : PC;
    }

    private static boolean switchesStored(SharedPreferences prefs) {
        return prefs.contains(RETIRED_BUSY) || prefs.contains(RETIRED_LOST) || prefs.contains(RETIRED_KEEP);
    }

    private static String valid(Object value) {
        return PC.equals(value) || SOONER.equals(value) || KEEP.equals(value) ? (String) value : null;
    }

    /** The Settings value. */
    public static String read(SharedPreferences prefs) {
        if (switchesStored(prefs)) {
            return fromSwitches(prefs.getBoolean(RETIRED_BUSY, false), prefs.getBoolean(RETIRED_LOST, false),
                    prefs.getBoolean(RETIRED_KEEP, false));
        }
        String stored = valid(prefs.getAll().get(SETTINGS_KEY));
        return stored != null ? stored : PC;
    }

    /** Moves the Settings value and the PCs' own choices from where earlier versions kept them; changes nothing a second time. */
    public static void migrate(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        migrateSettings(prefs);
        migratePcChoices(context, read(prefs));
    }

    static void migrateSettings(SharedPreferences prefs) {
        if (!switchesStored(prefs)) return;
        prefs.edit().putString(SETTINGS_KEY, read(prefs))
                .remove(RETIRED_BUSY).remove(RETIRED_LOST).remove(RETIRED_KEEP).apply();
    }

    /**
     * A PC's own choice of the busy controller becomes its own Lower bitrate: on is Lower it sooner, off The PC
     * decides; the conventional codecs' choice wins over PyroWave's. A choice that equals the Settings value is not
     * kept, and one the new store already holds stays.
     */
    static void migratePcChoices(Context context, String settings) {
        SharedPreferences target = context.getSharedPreferences(PC_STORE, Context.MODE_PRIVATE);
        SharedPreferences.Editor edit = null;
        java.util.Set<String> taken = new java.util.HashSet<>();
        for (String name : RETIRED_STORES) {
            SharedPreferences old = context.getSharedPreferences(name, Context.MODE_PRIVATE);
            Map<String, ?> all = old.getAll();
            if (all.isEmpty()) continue;
            for (Map.Entry<String, ?> entry : all.entrySet()) {
                String pc = entry.getKey();
                if (!(entry.getValue() instanceof Boolean) || target.contains(pc) || !taken.add(pc)) continue;
                String choice = (Boolean) entry.getValue() ? SOONER : PC;
                if (choice.equals(settings)) continue;
                edit = edit != null ? edit : target.edit();
                edit.putString(pc, choice);
            }
            old.edit().clear().apply();
        }
        if (edit != null) edit.apply();
    }

    /** Lower bitrate for a PC: its own choice, else the Settings value. */
    public static String forPc(Context context, String pcUuid) {
        migrate(context);
        if (pcUuid != null) {
            String own = valid(context.getSharedPreferences(PC_STORE, Context.MODE_PRIVATE).getAll().get(pcUuid));
            if (own != null) return own;
        }
        return read(PreferenceManager.getDefaultSharedPreferences(context));
    }

    /** Whether this device's busy controller is on for a PC: Lower it sooner. */
    public static boolean isOn(Context context, String pcUuid) {
        return SOONER.equals(forPc(context, pcUuid));
    }

    /** The stream menu's choice: this PC only. Only a choice that differs from the Settings value is kept. */
    public static void setForPc(Context context, String pcUuid, String choice) {
        if (pcUuid == null || valid(choice) == null) return;
        migrate(context);
        String settings = read(PreferenceManager.getDefaultSharedPreferences(context));
        SharedPreferences.Editor edit = context.getSharedPreferences(PC_STORE, Context.MODE_PRIVATE).edit();
        if (choice.equals(settings)) {
            edit.remove(pcUuid);
        } else {
            edit.putString(pcUuid, choice);
        }
        edit.apply();
    }

    /**
     * The stream menu's Keep my bitrate: Never: keep my bitrate when on, The PC decides when off, written as choosing
     * them in Settings writes them. Only this key: as the performance overlay's switch, it leaves every other stored
     * value.
     */
    public static void storeKeepBitrate(SharedPreferences.Editor editor, boolean keep) {
        editor.putString(SETTINGS_KEY, keep ? KEEP : PC)
                .remove(RETIRED_BUSY).remove(RETIRED_LOST).remove(RETIRED_KEEP);
    }

    /** The Settings value changed: every PC follows it again. */
    public static void clearPcChoices(Context context) {
        context.getSharedPreferences(PC_STORE, Context.MODE_PRIVATE).edit().clear().apply();
        for (String store : RETIRED_STORES) {
            context.getSharedPreferences(store, Context.MODE_PRIVATE).edit().clear().apply();
        }
    }

    public static void forget(Context context, String pcUuid) {
        context.getSharedPreferences(PC_STORE, Context.MODE_PRIVATE).edit().remove(pcUuid).apply();
        for (String store : RETIRED_STORES) {
            context.getSharedPreferences(store, Context.MODE_PRIVATE).edit().remove(pcUuid).apply();
        }
    }
}
