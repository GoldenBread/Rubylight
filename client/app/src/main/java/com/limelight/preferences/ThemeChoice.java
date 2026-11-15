package com.limelight.preferences;

import android.content.SharedPreferences;

import com.limelight.ui.AppTheme;

/**
 * The app theme as one choice: System, Dark or OLED black, stored under {@link AppTheme#THEME}. Before, it was two
 * switches ({@code checkbox_always_dark} and {@code checkbox_oled_black}); {@link AppTheme#theme} reads them while
 * they remain, so a store with only OLED black set reads as OLED black too.
 */
final class ThemeChoice {
    static final int SYSTEM = 0;
    static final int DARK = 1;
    static final int OLED_BLACK = 2;

    private ThemeChoice() { }

    static int read(SharedPreferences prefs) {
        String theme = AppTheme.theme(prefs);
        return AppTheme.THEME_OLED_BLACK.equals(theme) ? OLED_BLACK : AppTheme.THEME_DARK.equals(theme) ? DARK : SYSTEM;
    }

    static void save(SharedPreferences.Editor editor, int choice) {
        editor.putString(AppTheme.THEME, choice == OLED_BLACK ? AppTheme.THEME_OLED_BLACK :
                        choice == DARK ? AppTheme.THEME_DARK : AppTheme.THEME_SYSTEM)
                .remove(AppTheme.RETIRED_ALWAYS_DARK).remove(AppTheme.RETIRED_OLED_BLACK);
    }
}
