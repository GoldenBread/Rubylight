package com.limelight.preferences;

import androidx.appcompat.app.AlertDialog;
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
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;

import static org.junit.Assert.*;

/** The rows that were two switches each: pairs saved before read as the stream used them, and a choice saves both. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SwitchPairChoiceTest {
    private SharedPreferences prefs;

    @Before
    public void clear() {
        prefs = PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
        prefs.edit().clear().commit();
    }

    private int read(SwitchPairChoice pair, boolean first, boolean second) {
        prefs.edit().putBoolean(pair.firstKey, first).putBoolean(pair.secondKey, second).commit();
        return pair.read(prefs);
    }

    @Test
    public void everyPairSavedBeforeReadsAsTheStreamUsedIt() {
        // Frame loss replaced the busy network wherever the PC took rate changes.
        assertEquals(0, read(SwitchPairChoice.LOWER_BITRATE, false, false));
        assertEquals(1, read(SwitchPairChoice.LOWER_BITRATE, true, false));
        assertEquals(2, read(SwitchPairChoice.LOWER_BITRATE, false, true));
        assertEquals(2, read(SwitchPairChoice.LOWER_BITRATE, true, true));
        // Never: keep my bitrate wins over every stored pair, and leaves it stored.
        prefs.edit().putBoolean(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING, true).commit();
        for (boolean first : new boolean[] {false, true}) {
            for (boolean second : new boolean[] {false, true}) {
                assertEquals(3, read(SwitchPairChoice.LOWER_BITRATE, first, second));
                assertEquals(first, prefs.getBoolean(SwitchPairChoice.LOWER_BITRATE.firstKey, !first));
            }
        }
        prefs.edit().remove(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING).commit();
        assertEquals("an older file without the key", 2, read(SwitchPairChoice.LOWER_BITRATE, true, true));
        // The row lists them as The PC decides, Lower it sooner, When the network is busy, Never.
        assertArrayEquals(new int[] {0, 2, 1, 3}, SwitchPairChoice.LOWER_BITRATE.displayOrder(4));
        // When the network is busy is listed only while it is the stored choice.
        android.content.Context context = org.robolectric.RuntimeEnvironment.getApplication();
        prefs.edit().putBoolean(SwitchPairChoice.LOWER_BITRATE.firstKey, true)
                .putBoolean(SwitchPairChoice.LOWER_BITRATE.secondKey, false).commit();
        assertEquals(4, SwitchPairChoice.LOWER_BITRATE.offered(context, prefs));
        prefs.edit().putBoolean(SwitchPairChoice.LOWER_BITRATE.secondKey, true).commit();
        assertEquals(3, SwitchPairChoice.LOWER_BITRATE.offered(context, prefs));
        assertArrayEquals(new int[] {0, 2, 3}, SwitchPairChoice.LOWER_BITRATE.displayOrder(3));
        prefs.edit().clear().commit();
        // With both on, the stream menu won.
        assertEquals(0, read(SwitchPairChoice.BACK_BUTTON, false, false));
        assertEquals(1, read(SwitchPairChoice.BACK_BUTTON, true, false));
        assertEquals(2, read(SwitchPairChoice.BACK_BUTTON, false, true));
        assertEquals(1, read(SwitchPairChoice.BACK_BUTTON, true, true));
        // The second switch only counted while the first was on.
        for (SwitchPairChoice pair : new SwitchPairChoice[] {SwitchPairChoice.SYSTEM_KEYS, SwitchPairChoice.ROAMING}) {
            assertEquals(0, read(pair, false, false));
            assertEquals(0, read(pair, false, true));
            assertEquals(1, read(pair, true, false));
            assertEquals(2, read(pair, true, true));
        }
    }

    @Test
    public void nothingStoredReadsAsEachDefault() {
        assertEquals(0, SwitchPairChoice.LOWER_BITRATE.read(prefs));
        assertEquals(0, SwitchPairChoice.BACK_BUTTON.read(prefs));
        assertEquals(1, SwitchPairChoice.SYSTEM_KEYS.read(prefs));
        assertEquals(1, SwitchPairChoice.ROAMING.read(prefs));
    }

    @Test
    public void everyChoiceSavesBothSwitchesAndReadsBack() {
        for (SwitchPairChoice pair : SwitchPairChoice.values()) {
            int names = RuntimeEnvironment.getApplication().getResources().getStringArray(pair.names).length;
            assertEquals(pair.name(), pair == SwitchPairChoice.LOWER_BITRATE ? 4 : 3, names);
            for (int choice = 0; choice < names; choice++) {
                SharedPreferences.Editor editor = prefs.edit().clear();
                pair.save(editor, choice);
                editor.commit();
                // Never, keep my bitrate stores its own key and leaves the pair.
                boolean keep = pair == SwitchPairChoice.LOWER_BITRATE && choice == SwitchPairChoice.KEEP;
                assertEquals(!keep, prefs.contains(pair.firstKey));
                assertEquals(!keep, prefs.contains(pair.secondKey));
                assertEquals(pair.name() + " " + choice, choice, pair.read(prefs));
                assertSame(pair, SwitchPairChoice.forKey(pair.key));
            }
        }
        // When frames are lost keeps the busy network's controller for a PC without rate changes.
        SharedPreferences.Editor editor = prefs.edit();
        SwitchPairChoice.LOWER_BITRATE.save(editor, 2);
        editor.commit();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
        assertTrue(config.autoBitrate);
        assertTrue(config.rateChangeBitrate);
    }

    @Test
    public void theRowsPickerWritesBothSwitches() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment input = SettingsStructureTest.open(controller.get(), "input");
            Preference back = input.findPreference(SwitchPairChoice.BACK_BUTTON.key);
            assertNotNull(back);
            assertTrue(back.getOnPreferenceClickListener().onPreferenceClick(back));
            ((AlertDialog) ShadowDialog.getLatestDialog()).getListView().performItemClick(null, 2, 0);
            ShadowLooper.idleMainLooper();
            assertFalse(prefs.getBoolean(PreferenceConfiguration.BACK_OPENS_MENU_PREF_STRING, true));
            assertTrue(prefs.getBoolean(PreferenceConfiguration.BACK_AS_GUIDE_PREF_STRING, false));
            assertEquals("Guide", String.valueOf(back.getSummary()));
            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
            assertFalse(config.backOpensMenu);
            assertTrue(config.backAsGuide);
        }
    }
}
