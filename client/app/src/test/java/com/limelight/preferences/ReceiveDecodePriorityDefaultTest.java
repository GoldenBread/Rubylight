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

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ReceiveDecodePriorityDefaultTest {
    private static final String KEY = "checkbox_receive_decode_priority";

    private static Application app() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app());
    }

    @Test public void aNewInstallStartsWithItOn() {
        // What the PC list stores on its first launch.
        PreferenceManager.setDefaultValues(app(), R.xml.preferences, true);
        assertTrue(prefs().getBoolean(KEY, false));
        assertTrue(PreferenceConfiguration.readPreferences(app()).receiveDecodePriority);
    }

    @Test public void theOldStoredDefaultIsSwitchedOnOnceAndALaterChoiceIsKept() {
        // Installs whose first launch or Settings stored the old default, off.
        prefs().edit().putBoolean(KEY, false).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).receiveDecodePriority);
        assertTrue(prefs().getBoolean(KEY, false));

        // Turning it off in Settings is a real choice and survives later reads.
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).receiveDecodePriority);
        assertFalse(PreferenceConfiguration.readPreferences(app()).receiveDecodePriority);
        assertFalse(prefs().getBoolean(KEY, true));
    }

    @Test public void theMigrationRunsOnce() {
        prefs().edit().putBoolean(KEY, false).commit();
        assertTrue(PreferenceConfiguration.migrateReceiveDecodePriorityDefault(prefs()));
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.migrateReceiveDecodePriorityDefault(prefs()));
        assertFalse(prefs().getBoolean(KEY, true));
    }
}
