package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;
import android.util.AttributeSet;
import android.widget.Toast;

import com.limelight.R;

/**
 * A row of the simple settings layer: Stream smoothness, Picture quality or HDR. Its picker offers the
 * choices of {@link SimpleProfile}; the row shows "Custom" while the advanced settings match none of them,
 * and choosing one writes the whole profile into the existing settings (SimpleProfile.apply).
 *
 * <p>Binding the row never writes: ListPreference always persists the first value it reads, so only a
 * value chosen in the picker (through {@link #callChangeListener}) may apply the profile.
 */
public class SimpleProfilePreference extends MaterialListPreference {
    private boolean userPicked;

    public SimpleProfilePreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    public boolean callChangeListener(Object newValue) {
        // A pick the listener refuses is not a choice: it must not apply on the next bind.
        userPicked = super.callChangeListener(newValue);
        return userPicked;
    }

    /**
     * {@link android.preference.ListPreference#setValue} skips persisting a value equal to the one the row
     * remembers, so a pick that matches a stale remembered value would do nothing after the settings changed
     * under the row (Back to recommended, an advanced setting, a preset). A pick always applies here; the
     * framework still remembers the value afterwards.
     */
    @Override
    public void setValue(String value) {
        if (userPicked) {
            userPicked = false;
            if (!applyPick(value)) return;
        }
        super.setValue(value);
    }

    /** The list's own value is always read from the settings, never from XML's default. */
    @Override
    protected void onSetInitialValue(boolean restoreValue, Object defaultValue) {
        super.onSetInitialValue(true, defaultValue);
    }

    @Override
    protected String getPersistedString(String defaultReturnValue) {
        return shownChoice();
    }

    @Override
    public String getValue() {
        return noPreferences() ? super.getValue() : shownChoice();
    }

    @Override
    public CharSequence getEntry() {
        // The summary is read whenever the screen's values refresh, which is also when the settings can
        // have changed under the row: follow them before the framework's remembered value is used.
        String shown = shownChoice();
        if (!shown.equals(super.getValue())) {
            super.setValue(shown);
        }
        if (noPreferences() || SimpleProfile.custom(getContext(), getKey())) {
            return getContext().getString(R.string.settings_custom);
        }
        int index = findIndexOfValue(getValue());
        return index >= 0 ? getEntries()[index] : super.getEntry();
    }

    /** The choice the current settings are, so the row shows the profile they equal; the stored one while Custom. */
    private String shownChoice() {
        if (noPreferences()) return readStored();
        String[] chosen = SimpleProfile.chosen(getContext());
        int index = SimpleProfile.SMOOTHNESS.equals(getKey()) ? 0 : SimpleProfile.PICTURE.equals(getKey()) ? 1 : 2;
        return chosen != null ? chosen[index] : readStored();
    }

    private String readStored() {
        SharedPreferences prefs = getSharedPreferences();
        return SimpleProfile.read(prefs != null ? prefs : PreferenceManager.getDefaultSharedPreferences(getContext()),
                getKey());
    }

    private boolean noPreferences() {
        return !shouldPersist() || getSharedPreferences() == null;
    }

    @Override
    protected boolean persistString(String value) {
        // The framework persists the first value it binds; that is not a choice.
        if (!userPicked) return true;
        userPicked = false;
        return applyPick(value);
    }

    /** Applies a choice the player made; false when it could not be saved (a toast says why). */
    private boolean applyPick(String value) {
        if (noPreferences()) return false;
        if (!SimpleProfile.apply(getContext(), getKey(), value)) {
            Toast.makeText(getContext(), R.string.simple_profile_failed, Toast.LENGTH_LONG).show();
            return false;
        }
        // One choice writes every simple value; the screen's stored-value listener refreshes the other
        // rows' summaries with it.
        return true;
    }
}
