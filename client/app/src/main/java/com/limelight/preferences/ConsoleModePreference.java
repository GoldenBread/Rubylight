package com.limelight.preferences;

import android.content.Context;
import android.preference.PreferenceManager;
import android.util.AttributeSet;

import com.limelight.console.HomeSettings;
import com.limelight.console.OpenPlay;

/**
 * Console mode: Nightjar opens on Home, the full-screen library a controller can navigate, instead of the
 * list of PCs or a game countdown. It is the existing Home start screen (HomeSettings.PREF_KEY and
 * OpenPlay.SETTING), not a second launcher.
 */
public class ConsoleModePreference extends MaterialSwitchPreference {
    public ConsoleModePreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    protected boolean getPersistedBoolean(boolean defaultReturnValue) {
        return HomeSettings.enabled(getContext());
    }

    @Override
    protected boolean persistBoolean(boolean value) {
        boolean stored = PreferenceManager.getDefaultSharedPreferences(getContext()).edit()
                .putBoolean(HomeSettings.PREF_KEY, value)
                // Console mode is Home, never a countdown into the last or a chosen game.
                .putString(OpenPlay.SETTING, value ? "pcs" : OpenPlay.DEFAULT)
                .commit();
        if (!stored) com.limelight.LimeLog.warning("Console mode: the choice could not be saved");
        return stored;
    }
}
