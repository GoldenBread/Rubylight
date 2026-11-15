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
    public void eachChoiceSavesBothValuesAndReadsBackAsItself() {
        CharSequence[] names = DisplayArrangementChoice.names(app, true);
        assertEquals(8, names.length);
        assertEquals("The PC’s own setting", names[DisplayArrangementChoice.PC_OWN].toString());
        assertEquals("Virtual display", names[DisplayArrangementChoice.VIRTUAL_DISPLAY].toString());
        assertEquals(2, DisplayArrangementChoice.names(app, false).length);
        for (int i = 0; i < names.length; i++) {
            SharedPreferences.Editor editor = prefs().edit().clear();
            DisplayArrangementChoice.saveMain(editor, i);
            editor.commit();
            HostDisplayChoice arrangement = DisplayArrangementChoice.ARRANGEMENTS[i];
            assertEquals(names[i].toString(), i, DisplayArrangementChoice.readMain(prefs()));
            assertEquals(arrangement, HostDisplayChoice.loadMain(app));
            assertEquals(arrangement != HostDisplayChoice.HOST_DEFAULT, prefs().contains(HostDisplayChoice.MAIN_PREF));
            boolean virtualDisplay = PreferenceConfiguration.readPreferences(app).virtualDisplay;
            assertEquals(names[i].toString(), i != DisplayArrangementChoice.PC_OWN && arrangement != HostDisplayChoice.PHYSICAL, virtualDisplay);
            // A PC without display modes gets the switch alone: a virtual display for every choice that asks for one.
            assertEquals(arrangement.requestsVirtualDisplay(virtualDisplay), HostDisplayChoice.HOST_DEFAULT.requestsVirtualDisplay(virtualDisplay));
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
            assertTrue(row.getSummary().toString().startsWith(app.getString(com.limelight.R.string.host_display_exclusive)));
            row.getOnPreferenceClickListener().onPreferenceClick(row);
            AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
            picker.getListView().performItemClick(null, DisplayArrangementChoice.PC_OWN, DisplayArrangementChoice.PC_OWN);
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
            // A removed PC follows Settings again if it comes back.
            HostDisplayChoice.forget(app, "fold-pc");
            assertEquals(HostDisplayChoice.EXCLUSIVE, HostDisplayChoice.load(app, "fold-pc"));
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
