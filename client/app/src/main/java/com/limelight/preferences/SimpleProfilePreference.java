package com.limelight.preferences;

import android.content.Context;
import android.util.AttributeSet;

import com.limelight.R;

/**
 * A row of the simple settings layer: Stream smoothness, Picture quality or HDR. Its picker offers the
 * choices of {@link SimpleProfile}; the row shows "Custom" while the advanced settings match none of them,
 * and choosing one writes the whole profile into the existing settings (SimpleProfile.apply).
 */
public class SimpleProfilePreference extends MaterialListPreference {
    public SimpleProfilePreference(Context context, AttributeSet attrs) {
        super(context, attrs);
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
        return shouldPersist() && getSharedPreferences() != null ? shownChoice() : super.getValue();
    }

    @Override
    public CharSequence getEntry() {
        if (SimpleProfile.custom(getContext(), getKey())) {
            return getContext().getString(R.string.settings_custom);
        }
        return super.getEntry();
    }

    /** The choice the current settings are, so the row shows the profile they equal; the stored one while Custom. */
    private String shownChoice() {
        String[] chosen = SimpleProfile.chosen(getContext());
        int index = SimpleProfile.SMOOTHNESS.equals(getKey()) ? 0 : SimpleProfile.PICTURE.equals(getKey()) ? 1 : 2;
        return chosen != null ? chosen[index] : SimpleProfile.read(getSharedPreferences(), getKey());
    }

    @Override
    protected boolean persistString(String value) {
        if (!SimpleProfile.apply(getContext(), getKey(), value)) return false;
        // One choice writes every simple value: the other rows' summaries change with it.
        for (String key : new String[] {SimpleProfile.SMOOTHNESS, SimpleProfile.PICTURE, SimpleProfile.HDR}) {
            android.preference.Preference row = getPreferenceManager().findPreference(key);
            if (row != null) row.notifyChanged();
        }
        return true;
    }
}
