package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.AttributeSet;

/**
 * The Lower refresh rate row: Off, On or On, matched to the game, stored in {@code checkbox_reduce_refresh_rate}
 * and {@code checkbox_content_rate_frame_rate} (LowerRefreshChoice), never as a string under its key.
 */
public class LowerRefreshPreference extends StoredChoicePreference {
    public LowerRefreshPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    String readChoice(SharedPreferences prefs) {
        return LowerRefreshChoice.VALUES[LowerRefreshChoice.read(prefs)];
    }

    @Override
    void saveChoice(SharedPreferences.Editor editor, String value) {
        LowerRefreshChoice.save(editor, LowerRefreshChoice.ofValue(value));
    }
}
