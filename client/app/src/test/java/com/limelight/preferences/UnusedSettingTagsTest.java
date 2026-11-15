package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.Preference;
import android.preference.PreferenceManager;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/** A setting that works only in some setups says why where it does nothing (client/AGENTS.md). */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class UnusedSettingTagsTest {
    private SharedPreferences prefs;

    @Before
    public void clear() {
        prefs = PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
        prefs.edit().clear().commit();
    }

    private String reason(String key) {
        return SettingRules.unusedReason(RuntimeEnvironment.getApplication(), key, prefs);
    }

    @Test
    public void compatibilityVideoViewNamesWhatKeepsTheStreamsOwnSurface() {
        assertNull(reason("checkbox_texture_view"));
        prefs.edit().putBoolean("checkbox_enable_hdr", true).commit();
        assertEquals("Not used with HDR", reason("checkbox_texture_view"));
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).useTextureView);
        prefs.edit().putBoolean("checkbox_enable_hdr", false).putBoolean("checkbox_enable_sdr_10bit", true).commit();
        assertTrue(reason("checkbox_texture_view").startsWith("Not used with "));
        prefs.edit().putBoolean("checkbox_enable_sdr_10bit", false).putString("video_format", "forcepyrowave").commit();
        assertEquals("Not used with PyroWave", reason("checkbox_texture_view"));
    }

    @Test
    public void thermalProtectionNamesTheThreeItActsThrough() {
        assertEquals("Not used without Lower bitrate, Automatic quality or Boost GPU performance for PyroWave",
                reason(PreferenceConfiguration.THERMAL_PROTECTION_PREF_STRING));
        prefs.edit().putBoolean(AutoBitrateChoice.SETTINGS_KEY, true).commit();
        assertNull(reason(PreferenceConfiguration.THERMAL_PROTECTION_PREF_STRING));
        prefs.edit().clear().putString("adaptive_quality_mode", "motion").commit();
        assertNull(reason(PreferenceConfiguration.THERMAL_PROTECTION_PREF_STRING));
        // The boost acts only on PyroWave.
        prefs.edit().clear().putBoolean(PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING, true).commit();
        assertNotNull(reason(PreferenceConfiguration.THERMAL_PROTECTION_PREF_STRING));
        prefs.edit().putString("video_format", "forcepyrowave").commit();
        assertNull(reason(PreferenceConfiguration.THERMAL_PROTECTION_PREF_STRING));
    }

    @Test
    public void rowsThatAnotherSwitchOverrulesNameIt() {
        prefs.edit().putString("adaptive_quality_mode", "motion").commit();
        assertNull(reason("adaptive_quality_mode"));
        prefs.edit().putString("video_format", "forcepyrowave").commit();
        assertEquals("Not used with PyroWave", reason("adaptive_quality_mode"));
        prefs.edit().putString("adaptive_quality_mode", "off").commit();
        assertNull("Off needs no reason", reason("adaptive_quality_mode"));

        assertNull(reason("checkbox_onscreen_own_player"));
        prefs.edit().putBoolean("checkbox_only_show_L3R3", true).commit();
        assertTrue(reason("checkbox_onscreen_own_player").startsWith("Not used with "));

        assertNull(reason("checkbox_audio_passthrough"));
        prefs.edit().putBoolean("checkbox_mute_audio", true).commit();
        assertTrue(reason("checkbox_audio_passthrough").startsWith("Not used with "));
    }

    @Test
    public void theRowShowsItsReason() {
        prefs.edit().putBoolean("checkbox_enable_hdr", true).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            screen.moreOpen.add("advanced");
            Preference view = SettingsStructureTest.open(screen, "advanced").findPreference("checkbox_texture_view");
            assertEquals("Not used with HDR", String.valueOf(view.getSummary()));
        }
    }
}
