package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.preference.ListPreference;
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

    private static boolean on(String pc) {
        return AutoBitrateChoice.isOn(app(), pc);
    }

    private static SharedPreferences pcStore() {
        return app().getSharedPreferences(AutoBitrateChoice.PC_STORE, Context.MODE_PRIVATE);
    }

    private static SharedPreferences retiredStore(int index) {
        return app().getSharedPreferences(AutoBitrateChoice.RETIRED_STORES[index], Context.MODE_PRIVATE);
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
        pcStore().edit().clear().commit();
        retiredStore(0).edit().clear().commit();
        retiredStore(1).edit().clear().commit();
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
            assertEquals(app().getString(R.string.lower_bitrate_pc), String.valueOf(((ListPreference) lower).getEntry()));
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
    public void thePcsFollowTheSettingUntilTheStreamMenuChoosesForOne() {
        assertFalse(on("a"));
        assertFalse("No PC id: the default", on(null));
        prefs().edit().putString(AutoBitrateChoice.SETTINGS_KEY, AutoBitrateChoice.SOONER).commit();
        assertTrue(on("a"));
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app());
        assertTrue(config.autoBitrate);
        assertTrue(config.rateChangeBitrate);
        assertFalse(config.keepBitrate);

        // The stream menu chooses The PC decides for A only; a stream from A acts on it.
        AutoBitrateChoice.setForPc(app(), "a", AutoBitrateChoice.PC);
        assertFalse(on("a"));
        assertTrue(on("b"));
        PreferenceConfiguration forA = PreferenceConfiguration.readPreferences(app(), "a");
        assertFalse(forA.autoBitrate);
        assertFalse(forA.rateChangeBitrate);
        assertTrue(PreferenceConfiguration.readPreferences(app(), "b").rateChangeBitrate);
    }

    @Test
    public void theOneValueDrivesWhatTheThreeSwitchesDid() {
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.applyLowerBitrate(AutoBitrateChoice.PC);
        assertFalse(config.autoBitrate || config.rateChangeBitrate || config.keepBitrate);
        config.applyLowerBitrate(AutoBitrateChoice.SOONER);
        assertTrue(config.autoBitrate && config.rateChangeBitrate);
        assertFalse(config.keepBitrate);
        config.applyLowerBitrate(AutoBitrateChoice.KEEP);
        assertTrue(config.keepBitrate);
        assertFalse(config.autoBitrate || config.rateChangeBitrate);
        // An unknown stored value reads as the default.
        prefs().edit().putString(AutoBitrateChoice.SETTINGS_KEY, "busy").commit();
        assertEquals(AutoBitrateChoice.PC, AutoBitrateChoice.read(prefs()));
    }

    @Test
    public void choosingInSettingsMakesEveryPcFollowItAgain() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment video = open(controller.get(), "video");
            ListPreference lower = (ListPreference) video.findPreference(AutoBitrateChoice.SETTINGS_KEY);
            // Three choices: The PC decides, Lower it sooner, Never: keep my bitrate.
            assertArrayEquals(new CharSequence[] {AutoBitrateChoice.PC, AutoBitrateChoice.SOONER, AutoBitrateChoice.KEEP},
                    lower.getEntryValues());
            assertEquals(3, lower.getEntries().length);
            AutoBitrateChoice.setForPc(app(), "a", AutoBitrateChoice.SOONER);
            AutoBitrateChoice.setForPc(app(), "b", AutoBitrateChoice.KEEP);
            assertTrue(on("a"));

            pick(lower, AutoBitrateChoice.KEEP);
            assertEquals(AutoBitrateChoice.KEEP, prefs().getString(AutoBitrateChoice.SETTINGS_KEY, null));
            assertEquals(app().getString(R.string.lower_bitrate_keep), String.valueOf(lower.getEntry()));
            assertFalse("A follows Settings again", on("a"));
            assertFalse(pcStore().contains("a"));
            assertFalse(pcStore().contains("b"));
            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app());
            assertTrue(config.keepBitrate);
            assertFalse(config.autoBitrate);
            assertFalse(config.rateChangeBitrate);

            pick(lower, AutoBitrateChoice.SOONER);
            assertEquals(app().getString(R.string.lower_bitrate_lost), String.valueOf(lower.getEntry()));
            assertTrue(on("a"));
        }
    }

    /** Chooses a value in the row, as its list does. */
    private static void pick(ListPreference row, String value) {
        assertTrue(row.getOnPreferenceChangeListener().onPreferenceChange(row, value));
        row.setValue(value);
        ShadowLooper.idleMainLooper();
    }

    @Test
    public void aRemovedPcLeavesNoChoiceBehind() {
        prefs().edit().putString(AutoBitrateChoice.SETTINGS_KEY, AutoBitrateChoice.SOONER).commit();
        AutoBitrateChoice.setForPc(app(), "a", AutoBitrateChoice.PC);
        assertFalse(on("a"));
        AutoBitrateChoice.forget(app(), "a");
        assertTrue(on("a"));
    }

    @Test
    public void onlyAChoiceThatDiffersFromTheSettingIsStored() {
        // The PC decides in Settings: Lower it sooner for a PC is its own choice, The PC decides again is not.
        AutoBitrateChoice.setForPc(app(), "a", AutoBitrateChoice.SOONER);
        assertEquals(AutoBitrateChoice.SOONER, pcStore().getString("a", null));
        AutoBitrateChoice.setForPc(app(), "a", AutoBitrateChoice.PC);
        assertFalse(pcStore().contains("a"));
        // Lower it sooner in Settings: the other way round.
        prefs().edit().putString(AutoBitrateChoice.SETTINGS_KEY, AutoBitrateChoice.SOONER).commit();
        AutoBitrateChoice.setForPc(app(), "a", AutoBitrateChoice.SOONER);
        assertFalse(pcStore().contains("a"));
        AutoBitrateChoice.setForPc(app(), "a", AutoBitrateChoice.PC);
        assertEquals(AutoBitrateChoice.PC, pcStore().getString("a", null));
        assertFalse(on("a"));
    }

    /** Every combination of the three switches earlier versions stored, and what Lower bitrate holds after. */
    @Test
    public void theThreeSwitchesMoveIntoOneValueOnce() {
        for (int bits = 0; bits < 27; bits++) {
            prefs().edit().clear().commit();
            // Each switch absent, off or on.
            Boolean busy = state(bits % 3), lost = state(bits / 3 % 3), keep = state(bits / 9);
            SharedPreferences.Editor editor = prefs().edit();
            if (busy != null) editor.putBoolean(AutoBitrateChoice.RETIRED_BUSY, busy);
            if (lost != null) editor.putBoolean(AutoBitrateChoice.RETIRED_LOST, lost);
            if (keep != null) editor.putBoolean(AutoBitrateChoice.RETIRED_KEEP, keep);
            editor.commit();
            String expected = Boolean.TRUE.equals(keep) ? AutoBitrateChoice.KEEP
                    : Boolean.TRUE.equals(busy) || Boolean.TRUE.equals(lost) ? AutoBitrateChoice.SOONER : AutoBitrateChoice.PC;
            String label = busy + " " + lost + " " + keep;
            boolean anyStored = busy != null || lost != null || keep != null;
            assertEquals(label, expected, AutoBitrateChoice.read(prefs()));
            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app());
            assertEquals(label, anyStored ? expected : null, prefs().getString(AutoBitrateChoice.SETTINGS_KEY, null));
            assertFalse(label, prefs().contains(AutoBitrateChoice.RETIRED_BUSY));
            assertFalse(label, prefs().contains(AutoBitrateChoice.RETIRED_LOST));
            assertFalse(label, prefs().contains(AutoBitrateChoice.RETIRED_KEEP));
            assertEquals(label, AutoBitrateChoice.KEEP.equals(expected), config.keepBitrate);
            assertEquals(label, AutoBitrateChoice.SOONER.equals(expected), config.rateChangeBitrate);
            assertEquals(label, AutoBitrateChoice.SOONER.equals(expected), config.autoBitrate);
            // A second read changes nothing.
            PreferenceConfiguration.readPreferences(app());
            assertEquals(label, anyStored ? expected : null, prefs().getString(AutoBitrateChoice.SETTINGS_KEY, null));
        }
    }

    private static Boolean state(int index) {
        return index == 0 ? null : index == 2;
    }

    @Test
    public void aPcsOwnChoiceOfTheBusyControllerBecomesItsOwnLowerBitrate() {
        // The PC decides in Settings. A: on for conventional codecs, off for PyroWave (the codecs' choice wins).
        // B: on for PyroWave only. C: off, as Settings is, so nothing of its own. D: already moved, stays.
        retiredStore(0).edit().putBoolean("a", true).putBoolean("c", false).putBoolean("d", false).commit();
        retiredStore(1).edit().putBoolean("a", false).putBoolean("b", true).commit();
        pcStore().edit().putString("d", AutoBitrateChoice.KEEP).commit();
        AutoBitrateChoice.migrate(app());
        assertEquals(AutoBitrateChoice.SOONER, pcStore().getString("a", null));
        assertEquals(AutoBitrateChoice.SOONER, pcStore().getString("b", null));
        assertFalse(pcStore().contains("c"));
        assertEquals(AutoBitrateChoice.KEEP, pcStore().getString("d", null));
        assertTrue(retiredStore(0).getAll().isEmpty());
        assertTrue(retiredStore(1).getAll().isEmpty());

        // Lower it sooner in Settings (from its switches): a PC switched off gets The PC decides of its own.
        prefs().edit().putBoolean(AutoBitrateChoice.RETIRED_BUSY, true).commit();
        retiredStore(0).edit().putBoolean("e", false).putBoolean("f", true).commit();
        AutoBitrateChoice.migrate(app());
        assertEquals(AutoBitrateChoice.SOONER, prefs().getString(AutoBitrateChoice.SETTINGS_KEY, null));
        assertEquals(AutoBitrateChoice.PC, pcStore().getString("e", null));
        assertFalse(pcStore().contains("f"));
        assertFalse(on("e"));
        assertTrue(on("f"));
    }
}
