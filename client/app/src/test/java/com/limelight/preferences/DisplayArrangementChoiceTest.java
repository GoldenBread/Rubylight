package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.content.SharedPreferences;
import android.preference.Preference;
import android.preference.PreferenceManager;

import androidx.appcompat.app.AlertDialog;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;

import static com.limelight.preferences.SettingsStructureTest.open;
import static org.junit.Assert.*;

/** Display arrangement with Virtual display folded in: stored pairs read as streamed, a choice writes both values. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, qualifiers = "en", application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
public class DisplayArrangementChoiceTest {
    private final Application app = RuntimeEnvironment.getApplication();

    private SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app);
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    @Test
    public void everyStoredPairShowsTheChoiceThatStreamsTheSame() {
        for (HostDisplayChoice arrangement : HostDisplayChoice.values()) {
            for (boolean virtualDisplay : new boolean[] {false, true}) {
                String pair = arrangement + "/" + virtualDisplay;
                // With display modes the arrangement is sent, and the switch only counts with the PC's own setting.
                int choice = DisplayArrangementChoice.choice(arrangement, virtualDisplay, true);
                assertEquals(pair, arrangement, DisplayArrangementChoice.ARRANGEMENTS[choice]);
                assertEquals(pair, arrangement.requestsVirtualDisplay(virtualDisplay),
                        DisplayArrangementChoice.ARRANGEMENTS[choice].requestsVirtualDisplay(DisplayArrangementChoice.virtualDisplay(choice)));
                // Without display modes only the switch is sent.
                assertEquals(pair, virtualDisplay ? DisplayArrangementChoice.VIRTUAL_DISPLAY : DisplayArrangementChoice.PC_OWN,
                        DisplayArrangementChoice.choice(arrangement, virtualDisplay, false));
            }
        }
        // An inconsistent pair stays stored as it is until a choice is made, so no stream changes by itself.
        prefs().edit().putString(HostDisplayChoice.MAIN_PREF, HostDisplayChoice.EXCLUSIVE.name())
                .putBoolean(DisplayArrangementChoice.VIRTUAL_DISPLAY_PREF, false).commit();
        assertEquals(2, DisplayArrangementChoice.readMain(prefs()));
        assertEquals(HostDisplayChoice.EXCLUSIVE.name(), prefs().getString(HostDisplayChoice.MAIN_PREF, null));
        assertFalse(prefs().getBoolean(DisplayArrangementChoice.VIRTUAL_DISPLAY_PREF, true));
    }

    @Test
    public void eachChoiceSavesItsArrangementAndOnlyThePcsOwnAndVirtualDisplaySetTheSwitch() {
        CharSequence[] names = DisplayArrangementChoice.names(app, true);
        assertEquals(8, names.length);
        assertEquals("The PC’s own setting", names[DisplayArrangementChoice.PC_OWN].toString());
        assertEquals("Virtual display", names[DisplayArrangementChoice.VIRTUAL_DISPLAY].toString());
        assertEquals(2, DisplayArrangementChoice.names(app, false).length);
        for (boolean before : new boolean[] {false, true}) {
            for (int i = 0; i < names.length; i++) {
                String pick = names[i] + " over " + before;
                SharedPreferences.Editor editor = prefs().edit().clear().putBoolean(DisplayArrangementChoice.VIRTUAL_DISPLAY_PREF, before);
                DisplayArrangementChoice.saveMain(editor, i);
                editor.commit();
                HostDisplayChoice arrangement = DisplayArrangementChoice.ARRANGEMENTS[i];
                assertEquals(pick, i, DisplayArrangementChoice.readMain(prefs()));
                assertEquals(arrangement, HostDisplayChoice.loadMain(app));
                assertEquals(arrangement != HostDisplayChoice.HOST_DEFAULT, prefs().contains(HostDisplayChoice.MAIN_PREF));
                Boolean set = DisplayArrangementChoice.switchFor(i);
                assertEquals(pick, set != null ? set : before, PreferenceConfiguration.readPreferences(app).virtualDisplay);
            }
        }
    }

    /** What a Remote Monitor launch from the main settings asks for: it keeps the PC's arrangement, so the switch alone. */
    private boolean remoteMonitorAsksForAVirtualDisplay() {
        com.limelight.nvstream.ConnectionContext context = new com.limelight.nvstream.ConnectionContext();
        context.hostCaps = com.limelight.nvstream.http.TestHosts.rubylight();
        context.streamConfig = new com.limelight.nvstream.StreamConfiguration.Builder()
                .setVirtualDisplay(PreferenceConfiguration.readPreferences(app).virtualDisplay, 100).build();
        context.hostDisplay = HostDisplayChoice.loadMain(app);
        return com.limelight.nvstream.http.NvHTTP.wantsVirtualDisplay(context, com.limelight.nvstream.http.NvApp.Role.REMOTE_MONITOR);
    }

    @Test
    public void anArrangementPickLeavesWhatRemoteMonitorAsksFor() {
        assertTrue(com.limelight.nvstream.http.TestHosts.rubylight().has(com.limelight.nvstream.http.HostCapabilities.Feature.HOST_DISPLAY_MODES));
        for (HostDisplayChoice stored : HostDisplayChoice.values()) {
            for (boolean virtualDisplay : new boolean[] {false, true}) {
                for (int i = DisplayArrangementChoice.VIRTUAL_DISPLAY + 1; i < DisplayArrangementChoice.ARRANGEMENTS.length; i++) {
                    String pick = stored + "/" + virtualDisplay + " -> " + DisplayArrangementChoice.ARRANGEMENTS[i];
                    SharedPreferences.Editor editor = prefs().edit().clear().putBoolean(DisplayArrangementChoice.VIRTUAL_DISPLAY_PREF, virtualDisplay);
                    if (stored != HostDisplayChoice.HOST_DEFAULT) editor.putString(HostDisplayChoice.MAIN_PREF, stored.name());
                    editor.commit();
                    boolean before = remoteMonitorAsksForAVirtualDisplay();
                    assertEquals(pick, virtualDisplay, before);
                    editor = prefs().edit();
                    DisplayArrangementChoice.saveMain(editor, i);
                    editor.commit();
                    assertEquals(pick, before, remoteMonitorAsksForAVirtualDisplay());
                }
            }
        }
    }

    @Test
    public void theMainRowWritesBothValuesAndShowsTheRenderResolutionForAVirtualDisplay() {
        prefs().edit().putString(HostDisplayChoice.MAIN_PREF, HostDisplayChoice.PHYSICAL.name())
                .putBoolean(DisplayArrangementChoice.VIRTUAL_DISPLAY_PREF, true).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment display = open(controller.get(), "display");
            Preference row = display.findPreference("host_display_location");
            assertTrue(row.getSummary().toString().startsWith("Physical display"));
            assertNull(display.findPreference(DisplayArrangementChoice.VIRTUAL_DISPLAY_PREF));
            row.getOnPreferenceClickListener().onPreferenceClick(row);
            AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
            assertEquals(8, picker.getListView().getCount());
            assertEquals(7, picker.getListView().getCheckedItemPosition());
            picker.getListView().performItemClick(null, DisplayArrangementChoice.VIRTUAL_DISPLAY, DisplayArrangementChoice.VIRTUAL_DISPLAY);
            ShadowLooper.idleMainLooper();
            assertFalse(prefs().contains(HostDisplayChoice.MAIN_PREF));
            assertTrue(prefs().getBoolean(DisplayArrangementChoice.VIRTUAL_DISPLAY_PREF, false));
            assertTrue(SettingRules.virtualDisplayScaleApplies(prefs()));
        }
        prefs().edit().putString(HostDisplayChoice.MAIN_PREF, HostDisplayChoice.PHYSICAL.name()).commit();
        assertFalse(SettingRules.virtualDisplayScaleApplies(prefs()));
    }

    @Test
    public void aPcPagePickWritesItsArrangementAndItsOwnSwitch() {
        // Settings ask for Exclusive; this PC picks a virtual display in its own arrangement.
        prefs().edit().putString(HostDisplayChoice.MAIN_PREF, HostDisplayChoice.EXCLUSIVE.name())
                .putBoolean(DisplayArrangementChoice.VIRTUAL_DISPLAY_PREF, true).commit();
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra("host_uuid", "fold-pc").putExtra("host_name", "PC");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings.SettingsFragment fragment = (HostStreamSettings.SettingsFragment)
                    controller.get().getFragmentManager().findFragmentById(com.limelight.R.id.host_profile_settings);
            Preference row = fragment.findPreference("host_display");
            // Without a choice of its own, the PC follows Settings, as its other rows say.
            String followsSettings = app.getString(com.limelight.R.string.host_profile_same_as_all_value,
                    app.getString(com.limelight.R.string.host_display_exclusive));
            assertEquals(followsSettings, row.getSummary().toString().split("\\n")[0]);
            row.getOnPreferenceClickListener().onPreferenceClick(row);
            AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
            // Same as all PCs first, then the choices.
            assertEquals(followsSettings, picker.getListView().getAdapter().getItem(0).toString());
            assertEquals(9, picker.getListView().getCount());
            assertEquals(0, picker.getListView().getCheckedItemPosition());
            int pcOwn = 1 + DisplayArrangementChoice.PC_OWN;
            picker.getListView().performItemClick(null, pcOwn, pcOwn);
            picker.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            ShadowLooper.idleMainLooper();
            // The PC's own setting stays its own although Settings ask for Exclusive.
            assertEquals(HostDisplayChoice.HOST_DEFAULT, HostDisplayChoice.load(app, "fold-pc"));
            // Saved with the arrangement, as the dialog says, without the page's other edits.
            assertEquals(Boolean.FALSE, HostStreamProfile.load(app, "fold-pc").virtualDisplay);
            assertTrue(org.robolectric.util.ReflectionHelpers.<String>getField(controller.get(), "savedProfile")
                    .equals(org.robolectric.util.ReflectionHelpers.<HostStreamProfile>getField(controller.get(), "overrides").serialize()));
            assertTrue(row.getSummary().toString().startsWith("The PC’s own setting"));
            // Settings themselves are untouched.
            assertEquals(HostDisplayChoice.EXCLUSIVE, HostDisplayChoice.loadMain(app));
            assertTrue(prefs().getBoolean(DisplayArrangementChoice.VIRTUAL_DISPLAY_PREF, false));
            // Same as all PCs undoes it: the PC follows Settings again, a later change included.
            row.getOnPreferenceClickListener().onPreferenceClick(row);
            picker = (AlertDialog) ShadowDialog.getLatestDialog();
            assertEquals(pcOwn, picker.getListView().getCheckedItemPosition());
            picker.getListView().performItemClick(null, 0, 0);
            picker.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            ShadowLooper.idleMainLooper();
            assertFalse(HostDisplayChoice.hasOwn(app, "fold-pc"));
            // Nothing of its own is left: no stored values at all.
            assertNull(HostStreamProfile.load(app, "fold-pc"));
            assertEquals(HostDisplayChoice.EXCLUSIVE, HostDisplayChoice.load(app, "fold-pc"));
            assertEquals(app.getString(com.limelight.R.string.host_profile_same_as_all_value,
                    app.getString(com.limelight.R.string.host_display_exclusive)), row.getSummary().toString().split("\n")[0]);
            SharedPreferences.Editor editor = prefs().edit();
            DisplayArrangementChoice.saveMain(editor, 3);
            editor.commit();
            assertEquals(HostDisplayChoice.EXTENDED, DisplayArrangementChoice.ARRANGEMENTS[3]);
            assertEquals(HostDisplayChoice.EXTENDED, HostDisplayChoice.load(app, "fold-pc"));
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
