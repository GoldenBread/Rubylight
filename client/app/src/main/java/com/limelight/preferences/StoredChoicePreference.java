package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.AttributeSet;

/**
 * A row drawn as a list whose choice is stored in other values, never as a string of its own under its key
 * (VrrChoice, FramePacingChoice, LowerRefreshChoice). The value is read from those values each time, so presets,
 * profiles and other writers show at once.
 */
abstract class StoredChoicePreference extends MaterialListPreference {
    StoredChoicePreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    /** The choice the stored values make, as one of the list's values. */
    abstract String readChoice(SharedPreferences prefs);

    /** Writes the stored values of a list value; an unknown one writes nothing. */
    abstract void saveChoice(SharedPreferences.Editor editor, String value);

    /**
     * Always the stored choice, never the XML default: the row's key may be unstored while the values it reads
     * are (the PC's frame times switched on with the pacing never chosen), and writing the default would undo them.
     */
    @Override
    protected void onSetInitialValue(boolean restoreValue, Object defaultValue) {
        super.onSetInitialValue(true, defaultValue);
    }

    @Override
    protected String getPersistedString(String defaultReturnValue) {
        SharedPreferences prefs = getSharedPreferences();
        if (!shouldPersist() || prefs == null) return defaultReturnValue;
        return readChoice(prefs);
    }

    @Override
    protected boolean persistString(String value) {
        SharedPreferences prefs = getSharedPreferences();
        if (!shouldPersist() || prefs == null) return false;
        // A row first stored its default too; an unchanged choice writes nothing else.
        if (readChoice(prefs).equals(value) && prefs.contains(getKey())) return true;
        SharedPreferences.Editor editor = prefs.edit();
        saveChoice(editor, value);
        editor.apply();
        return true;
    }

    @Override
    public void setValue(String value) {
        super.setValue(value);
        // The list remembers its last value; the stored values may have changed since without it.
        persistString(value);
    }

    @Override
    public String getValue() {
        SharedPreferences prefs = getSharedPreferences();
        return shouldPersist() && prefs != null ? readChoice(prefs) : super.getValue();
    }

    @Override
    public CharSequence getEntry() {
        int index = findIndexOfValue(getValue());
        CharSequence[] entries = getEntries();
        return index >= 0 && entries != null ? entries[index] : null;
    }
}
