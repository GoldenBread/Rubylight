package com.limelight.preferences;

import android.content.Context;
import android.preference.PreferenceManager;
import android.util.AttributeSet;

import com.limelight.console.HomeSettings;

/**
 * Console mode: Nightjar opens on Home, the full-screen library a controller can navigate, instead of the
 * list of PCs or a game countdown. It is the existing Home start screen (HomeSettings.PREF_KEY and
 * OpenPlay.SETTING), not a second launcher.
 *
 * <p>Binding the row never writes: SwitchPreference persists the first value it reads, so only a tap
 * changes the two stored keys.
 */
public class ConsoleModePreference extends MaterialSwitchPreference {
    private boolean binding;

    public ConsoleModePreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    protected void onSetInitialValue(boolean restoreValue, Object defaultValue) {
        binding = true;
        super.onSetInitialValue(restoreValue, defaultValue);
        binding = false;
    }

    @Override
    protected boolean getPersistedBoolean(boolean defaultReturnValue) {
        return StartScreenChoice.read(PreferenceManager.getDefaultSharedPreferences(getContext()))
                .equals(StartScreenChoice.HOME);
    }

    @Override
    protected boolean persistBoolean(boolean value) {
        if (binding) return true;
        boolean stored = PreferenceManager.getDefaultSharedPreferences(getContext()).edit()
                .putBoolean(HomeSettings.PREF_KEY, value)
                // Both directions name the screen this row is; Turn off opens the PC list, not a game.
                .putString(com.limelight.console.OpenPlay.SETTING,
                        StartScreenChoice.openFor(value ? StartScreenChoice.HOME : StartScreenChoice.PCS))
                .commit();
        if (!stored) com.limelight.LimeLog.warning("Console mode: the choice could not be saved");
        return stored;
    }
}
