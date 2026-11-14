package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.AttributeSet;

/**
 * The Frame pacing row: its four pacings, Balanced keeping every frame and the PC's frame times, stored in
 * {@code frame_pacing}, {@code checkbox_drop_late_frames} and {@code checkbox_host_frame_times}
 * (FramePacingChoice); {@code frame_pacing} only ever holds one of its four pacings.
 */
public class FramePacingPreference extends StoredChoicePreference {
    public FramePacingPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    String readChoice(SharedPreferences prefs) {
        return FramePacingChoice.read(prefs);
    }

    @Override
    void saveChoice(SharedPreferences.Editor editor, String value) {
        FramePacingChoice.save(editor, value);
    }
}
