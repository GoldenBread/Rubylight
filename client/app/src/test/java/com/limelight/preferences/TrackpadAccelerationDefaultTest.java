package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/**
 * Trackpad acceleration used to start off and its stored false is the old default, not a choice.
 * It moved to a new key that starts on once; a later Off is a real choice.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class TrackpadAccelerationDefaultTest {
    private static final String KEY = "checkbox_trackpad_acceleration_v2";
    private static final String RETIRED = "checkbox_trackpad_acceleration";

    private static Application app() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app());
    }

    @Test
    public void aNewInstallStartsWithAccelerationOn() {
        // What the PC list stores on its first launch.
        PreferenceManager.setDefaultValues(app(), R.xml.preferences, true);
        assertTrue(prefs().getBoolean(KEY, false));
        assertTrue(PreferenceConfiguration.readPreferences(app()).trackpadAcceleration);
    }

    @Test
    public void theOldStoredOffValueGetsTheNewDefaultOnceAndALaterChoiceIsKept() {
        // An install that had the old default, or had turned the old feature off: it cannot be
        // told apart from the default, so it starts on.
        prefs().edit().putBoolean(RETIRED, false).commit();
        assertFalse(prefs().contains(KEY));
        assertTrue(PreferenceConfiguration.readPreferences(app()).trackpadAcceleration);
        assertTrue(prefs().getBoolean(KEY, false));
        assertFalse(prefs().contains(RETIRED));

        // Turning it off now is a real choice and survives later reads.
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).trackpadAcceleration);
        assertFalse(PreferenceConfiguration.readPreferences(app()).trackpadAcceleration);
        assertFalse(prefs().getBoolean(KEY, true));
    }

    @Test
    public void theOldOnValueKeepsAccelerationOnAndForgetsTheOldName() {
        prefs().edit().putBoolean(RETIRED, true).putString("list_trackpad_acceleration_curve", "strong").commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).trackpadAcceleration);
        assertTrue(prefs().getBoolean(KEY, false));
        assertFalse(prefs().contains(RETIRED));
        // The strength is its own setting and keeps its stored choice.
        assertEquals("strong", prefs().getString("list_trackpad_acceleration_curve", null));
        assertEquals("strong", PreferenceConfiguration.readPreferences(app()).trackpadAccelerationCurve);
    }

    @Test
    public void anInstallThatNeverStoredItReadsOn() {
        assertFalse(prefs().contains(KEY));
        assertFalse(prefs().contains(RETIRED));
        assertTrue(PreferenceConfiguration.readPreferences(app()).trackpadAcceleration);
        assertTrue(prefs().getBoolean(KEY, false));
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).trackpadAcceleration);
    }

    @Test
    public void theNewKeyWinsWhenSettingsAlreadyStoredIt() {
        // Settings may have stored the new XML default before the migration ran; the old off value
        // still must not win, and the old name is forgotten.
        prefs().edit().putBoolean(KEY, true).putBoolean(RETIRED, false).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).trackpadAcceleration);
        assertFalse(prefs().contains(RETIRED));
        assertTrue(prefs().getBoolean(KEY, false));
    }

    @Test
    public void theMigrationRunsOnce() {
        assertFalse(PreferenceConfiguration.migrateTrackpadAccelerationDefault(prefs()));
        assertTrue(prefs().getBoolean(KEY, false));
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.migrateTrackpadAccelerationDefault(prefs()));
        assertFalse(prefs().getBoolean(KEY, true));
    }

    @Test
    public void onlyTheOldOffValueIsReported() {
        prefs().edit().putBoolean(RETIRED, false).commit();
        assertTrue(PreferenceConfiguration.migrateTrackpadAccelerationDefault(prefs()));
        assertFalse(PreferenceConfiguration.migrateTrackpadAccelerationDefault(prefs()));
    }
}
