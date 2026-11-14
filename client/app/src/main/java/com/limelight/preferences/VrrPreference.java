package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.AttributeSet;

/**
 * The Variable refresh rate row: Off, On or Fastest, drawn as a list but stored in the two switches
 * {@code checkbox_vrr} and {@code checkbox_real_vrr} (VrrChoice), never as a string under its key.
 */
public class VrrPreference extends StoredChoicePreference {
    public VrrPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    String readChoice(SharedPreferences prefs) {
        return VrrChoice.VALUES[VrrChoice.read(prefs)];
    }

    @Override
    void saveChoice(SharedPreferences.Editor editor, String value) {
        VrrChoice.save(editor, VrrChoice.ofValue(value));
    }
}
