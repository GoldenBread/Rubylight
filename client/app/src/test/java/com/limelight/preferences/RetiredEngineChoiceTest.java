package com.limelight.preferences;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.content.Context;
import android.content.SharedPreferences;

import android.preference.PreferenceManager;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;

/** "New streaming engine" is gone with the classic engine: its stored values are cleared. */
@RunWith(RobolectricTestRunner.class)
public class RetiredEngineChoiceTest {
    @Test
    public void theStoredChoiceAndTheDefaultMigrationsMarkerAreCleared() {
        Context app = RuntimeEnvironment.getApplication();
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(app);
        prefs.edit().clear().putBoolean("checkbox_rust_engine", false).putBoolean("rust_engine_default_on", true)
                .putBoolean("checkbox_client_roaming", false).commit();
        PreferenceConfiguration.retireEngineChoice(prefs);
        assertFalse(prefs.contains("checkbox_rust_engine"));
        assertFalse(prefs.contains("rust_engine_default_on"));
        // Other settings are left as they were.
        assertTrue(prefs.contains("checkbox_client_roaming"));
    }
}
