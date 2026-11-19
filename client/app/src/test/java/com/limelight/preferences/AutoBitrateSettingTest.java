package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.preference.Preference;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.SwitchPreference;

import com.limelight.R;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import static com.limelight.preferences.SettingsStructureTest.open;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AutoBitrateSettingTest {
    private static Application app() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app());
    }

    private static boolean on(String store, String pc) {
        return AutoBitrateChoice.isOn(app(), store, pc);
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
        for (String store : new String[] {AutoBitrateChoice.CODEC_STORE, AutoBitrateChoice.PYROWAVE_STORE}) {
            app().getSharedPreferences(store, Context.MODE_PRIVATE).edit().clear().commit();
        }
    }

    @Test
    public void theOldMinimumAndMaximumAreDeletedWhenSettingsLoad() {
        prefs().edit().putString("list_auto_bitrate_min_kbps", "20000")
                .putString("list_auto_bitrate_max_kbps", "50000").putString("list_fps", "60").commit();
        PreferenceConfiguration.readPreferences(app());
        assertFalse(prefs().contains("list_auto_bitrate_min_kbps"));
        assertFalse(prefs().contains("list_auto_bitrate_max_kbps"));
        assertEquals("Other settings stay", "60", prefs().getString("list_fps", null));
        // A second load has nothing left to do.
        PreferenceConfiguration.readPreferences(app());
        assertFalse(prefs().contains("list_auto_bitrate_min_kbps"));
    }

    @Test
    public void theOldListsAreNotInTheSettingsScreen() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment video = open(controller.get(), "video");
            assertNull(video.findPreference("list_auto_bitrate_min_kbps"));
            assertNull(video.findPreference("list_auto_bitrate_max_kbps"));
        }
    }

    @Test
    public void lowerBitrateSitsDirectlyUnderTheBitrateSliderAndIsOffByDefault() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            // The slider shows while Automatic bitrate is off (SettingRules.bitrateShown); after the first start's
            // move to this screen's defaults, which turns it on.
            prefs().edit().putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, true).commit();
            StreamSettings.SettingsFragment video = open(controller.get(), "video");
            Preference lower = video.findPreference(AutoBitrateChoice.SETTINGS_KEY);
            assertNotNull(lower);
            assertEquals(app().getString(R.string.title_lower_bitrate), lower.getTitle().toString());
            assertEquals(app().getString(R.string.lower_bitrate_pc), String.valueOf(lower.getSummary()));
            Preference slider = video.findPreference("seekbar_bitrate_kbps");
            PreferenceGroup group = (PreferenceGroup) video.getPreferenceScreen().getPreference(0);
            int at = -1;
            for (int i = 0; i < group.getPreferenceCount(); i++) {
                if (group.getPreference(i) == slider) at = i;
            }
            assertTrue(at >= 0);
            assertSame(lower, group.getPreference(at + 1));
            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app());
            assertFalse(config.autoBitrate);
            assertFalse(config.rateChangeBitrate);
        }
    }

    @Test
    public void automaticBitrateIsOnUntilTheSliderMovesAndTurnsBackToTheDefault() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment video = open(controller.get(), "video");
            SwitchPreference toggle = (SwitchPreference) video.findPreference(StreamSettings.AUTOMATIC_BITRATE_KEY);
            assertNotNull(toggle);
            assertEquals(app().getString(R.string.title_automatic_bitrate), toggle.getTitle().toString());
            assertTrue("On while the bitrate is not the user's", toggle.isChecked());

            // Moving the slider makes the bitrate the user's: the switch goes off and the bitrate stays.
            Preference slider = video.findPreference("seekbar_bitrate_kbps");
            int automatic = PreferenceConfiguration.getShownDefaultBitrate(prefs(), null, null, null);
            int chosenKbps = automatic + 7000;
            assertTrue(slider.getOnPreferenceChangeListener().onPreferenceChange(slider, chosenKbps));
            prefs().edit().putInt("seekbar_bitrate_kbps", chosenKbps).commit();
            ShadowLooper.idleMainLooper();
            assertFalse(toggle.isChecked());
            PreferenceConfiguration chosen = PreferenceConfiguration.readPreferences(app());
            assertFalse(chosen.automaticBitrate);
            assertEquals(chosenKbps, chosen.bitrate);

            // On again: back to the default for the mode, which it follows once more.
            assertTrue(toggle.getOnPreferenceChangeListener().onPreferenceChange(toggle, true));
            ShadowLooper.idleMainLooper();
            assertTrue(toggle.isChecked());
            assertEquals(automatic, prefs().getInt("seekbar_bitrate_kbps", 0));
            assertTrue(PreferenceConfiguration.readPreferences(app()).automaticBitrate);

            // Off without moving the slider keeps the bitrate shown, as the user's.
            assertTrue(toggle.getOnPreferenceChangeListener().onPreferenceChange(toggle, false));
            ShadowLooper.idleMainLooper();
            assertFalse(toggle.isChecked());
            assertFalse(PreferenceConfiguration.readPreferences(app()).automaticBitrate);
            assertEquals(automatic, prefs().getInt("seekbar_bitrate_kbps", 0));
        }
    }

    @Test
    public void thePcsFollowTheSwitchUntilTheStreamMenuChoosesForOne() {
        assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
        assertFalse("No PC id: the default", on(AutoBitrateChoice.CODEC_STORE, null));
        prefs().edit().putBoolean(AutoBitrateChoice.SETTINGS_KEY, true).commit();
        assertTrue(on(AutoBitrateChoice.CODEC_STORE, "a"));
        assertTrue(on(AutoBitrateChoice.PYROWAVE_STORE, "a"));
        assertTrue(PreferenceConfiguration.readPreferences(app()).autoBitrate);

        // The stream menu turns it off for A only, in the store of the stream's codec.
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", false);
        assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
        assertTrue(on(AutoBitrateChoice.PYROWAVE_STORE, "a"));
        assertTrue(on(AutoBitrateChoice.CODEC_STORE, "b"));
    }

    @Test
    public void choosingInSettingsMakesEveryPcFollowItAgain() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment video = open(controller.get(), "video");
            Preference lower = video.findPreference(AutoBitrateChoice.SETTINGS_KEY);
            AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", true);
            AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.PYROWAVE_STORE, "b", true);
            assertTrue(on(AutoBitrateChoice.CODEC_STORE, "a"));

            // Off in Settings: the PCs the stream menu had switched on follow it.
            pick(lower, 0);
            assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
            assertFalse(on(AutoBitrateChoice.PYROWAVE_STORE, "b"));

            // The list offers When the network is busy only while it is stored: The PC decides, Lower it sooner, Never.
            assertTrue(lower.getOnPreferenceClickListener().onPreferenceClick(lower));
            assertEquals(3, ((androidx.appcompat.app.AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog())
                    .getListView().getAdapter().getCount());
            ((androidx.appcompat.app.AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog()).dismiss();
            // A stored When the network is busy, then off again for one PC from the stream menu.
            prefs().edit().putBoolean(AutoBitrateChoice.SETTINGS_KEY, true)
                    .putBoolean(PreferenceConfiguration.RATE_CHANGE_BITRATE_PREF_STRING, false).commit();
            assertTrue(prefs().getBoolean(AutoBitrateChoice.SETTINGS_KEY, false));
            assertFalse(prefs().getBoolean(PreferenceConfiguration.RATE_CHANGE_BITRATE_PREF_STRING, true));
            assertTrue(on(AutoBitrateChoice.CODEC_STORE, "a"));
            assertTrue(on(AutoBitrateChoice.PYROWAVE_STORE, "b"));
            AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", false);
            assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
            assertTrue(on(AutoBitrateChoice.CODEC_STORE, "b"));

            // Lower it sooner (the second entry of four while When the network is busy is stored) keeps the busy network's controller for a PC without rate changes.
            pick(lower, 1);
            assertTrue(prefs().getBoolean(AutoBitrateChoice.SETTINGS_KEY, false));
            assertTrue(prefs().getBoolean(PreferenceConfiguration.RATE_CHANGE_BITRATE_PREF_STRING, false));
            assertFalse(prefs().getBoolean(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING, true));
            assertEquals(app().getString(R.string.lower_bitrate_lost), String.valueOf(lower.getSummary()));

            // Never: keep my bitrate leaves the two switches as they are, wins over them, and every PC follows it.
            AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", true);
            pick(lower, 2);
            assertTrue(prefs().getBoolean(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING, false));
            assertTrue(prefs().getBoolean(AutoBitrateChoice.SETTINGS_KEY, false));
            assertTrue(prefs().getBoolean(PreferenceConfiguration.RATE_CHANGE_BITRATE_PREF_STRING, false));
            assertEquals(app().getString(R.string.lower_bitrate_keep), String.valueOf(lower.getSummary()));
            assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app());
            assertTrue(config.keepBitrate);
            assertFalse(config.autoBitrate);
            assertFalse(config.rateChangeBitrate);

            // Any other choice writes the pair as before and clears it.
            pick(lower, 0);
            assertFalse(prefs().getBoolean(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING, true));
            assertFalse(prefs().getBoolean(AutoBitrateChoice.SETTINGS_KEY, true));
            assertFalse(prefs().getBoolean(PreferenceConfiguration.RATE_CHANGE_BITRATE_PREF_STRING, true));
        }
    }

    /** Opens the row's picker and picks the choice at index. */
    private static void pick(Preference row, int index) {
        assertTrue(row.getOnPreferenceClickListener().onPreferenceClick(row));
        androidx.appcompat.app.AlertDialog dialog = (androidx.appcompat.app.AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
        dialog.getListView().performItemClick(null, index, 0);
        ShadowLooper.idleMainLooper();
    }

    @Test
    public void aRemovedPcLeavesNoChoiceBehind() {
        prefs().edit().putBoolean(AutoBitrateChoice.SETTINGS_KEY, true).commit();
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", false);
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.PYROWAVE_STORE, "a", false);
        assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
        AutoBitrateChoice.forget(app(), "a");
        assertTrue(on(AutoBitrateChoice.CODEC_STORE, "a"));
        assertTrue(on(AutoBitrateChoice.PYROWAVE_STORE, "a"));
    }

    @Test
    public void onlyAChoiceThatDiffersFromTheSettingsSwitchIsStored() {
        SharedPreferences codec = app().getSharedPreferences(AutoBitrateChoice.CODEC_STORE, Context.MODE_PRIVATE);
        // Switch off: turning it on for a PC is an override, turning it off again is not.
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", true);
        assertTrue(codec.contains("a"));
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", false);
        assertFalse(codec.contains("a"));
        // Switch on: the other way round.
        prefs().edit().putBoolean(AutoBitrateChoice.SETTINGS_KEY, true).commit();
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", true);
        assertFalse(codec.contains("a"));
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", false);
        assertTrue(codec.contains("a"));
        assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
    }
}
