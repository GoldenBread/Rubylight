package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.preference.Preference;
import android.preference.PreferenceFragment;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;
import android.preference.TwoStatePreference;

import com.limelight.R;
import com.limelight.binding.input.touch.TouchMode;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.function.BiConsumer;

import static com.limelight.preferences.SettingsStructureTest.keys;
import static com.limelight.preferences.SettingsStructureTest.open;
import static com.limelight.preferences.SettingsStructureTest.resultGroup;
import static com.limelight.preferences.SettingsStructureTest.search;
import static com.limelight.preferences.SettingsStructureTest.settle;
import static com.limelight.preferences.SettingsStructureTest.shown;
import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Settings shows the rows that can do something: Debug only with Show debug settings or a changed Debug setting, and the
 * rows of SettingRules.WHEN_ROWS only in the mode or on the device they act in, or while they hold a changed value.
 * Search finds every one of them.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, qualifiers = "en", application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsHideTest {
    /** A handheld with nothing attached, that does not fold, on a chip Chipset tuning leaves alone. */
    static final SettingDevices NOTHING_ATTACHED = new Devices();

    /** Device facts a test switches. */
    static final class Devices implements SettingDevices {
        boolean touchpad, mouse, keyboard, folds, tunedChip;
        @Override public boolean touchpad() { return touchpad; }
        @Override public boolean mouse() { return mouse; }
        @Override public boolean keyboard() { return keyboard; }
        @Override public boolean folds() { return folds; }
        @Override public boolean tunedChip() { return tunedChip; }
    }

    private final Devices devices = new Devices();

    private static Context context() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(context());
    }

    private static SharedPreferences gameInfoKeys() {
        return com.limelight.console.GameInfo.keyPrefs(context());
    }

    @Before
    public void touchscreen() {
        shadowOf(context().getPackageManager()).setSystemFeature(PackageManager.FEATURE_TOUCHSCREEN, true);
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
        gameInfoKeys().edit().clear().commit();
        context().getSharedPreferences("SettingsScreenState", Context.MODE_PRIVATE).edit().clear().commit();
        for (String name : new String[] {StreamProfiles.PREFERENCES, "AppStreamSettings", "HostStreamProfiles"}) {
            context().getSharedPreferences(name, Context.MODE_PRIVATE).edit().clear().commit();
        }
    }

    private ActivityController<StreamSettings> start() {
        ActivityController<StreamSettings> built = Robolectric.buildActivity(StreamSettings.class);
        built.get().devices = devices;
        built.get().savedPcReader = task -> { };
        return built.setup();
    }

    /** The keys of the rows a section's list shows now. */
    private static List<String> visible(StreamSettings.SettingsFragment fragment) {
        List<String> keys = new ArrayList<>();
        PreferenceScreen screen = fragment.getPreferenceScreen();
        for (int i = 0; i < screen.getPreferenceCount(); i++) {
            PreferenceGroup group = (PreferenceGroup) screen.getPreference(i);
            assertTrue(group.getKey() + " shows with no row", group.getPreferenceCount() > 0);
            keys.addAll(keys(group));
        }
        return keys;
    }

    /** Whether the list of sections (the overview, with More settings open) shows this row. */
    private static boolean listed(StreamSettings screen, String row) {
        return ((PreferenceFragment) shown(screen)).getPreferenceScreen().findPreference(row) != null;
    }

    private static Preference result(StreamSettings.SettingsFragment fragment, String key) {
        return fragment.getPreferenceScreen().findPreference("search_result_" + key);
    }

    @Test
    public void showDebugSettingsIsAnAppSwitchOffByDefaultAndItsValueIsKept() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment app = open(screen, "app");
            Preference row = app.findPreference(SettingRules.SHOW_DEBUG);
            assertTrue("a row of App", visible(app).contains(SettingRules.SHOW_DEBUG));
            assertTrue("behind App's More row", StreamSettings.foldsInSection(context(), "app", SettingRules.SHOW_DEBUG));
            assertFalse(((TwoStatePreference) row).isChecked());
            assertFalse(prefs().getBoolean(SettingRules.SHOW_DEBUG, true));
            ((TwoStatePreference) row).setChecked(true);
            assertTrue("stored", prefs().getBoolean(SettingRules.SHOW_DEBUG, false));
        }
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings.SettingsFragment app = open(controller.get(), "app");
            assertTrue("read back", ((TwoStatePreference) app.findPreference(SettingRules.SHOW_DEBUG)).isChecked());
        }
    }

    @Test
    public void debugIsListedOnlyOnRequestOrWhileOneOfItsSettingsIsChanged() {
        StreamSettings.rememberRail(context(), true);
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            settle(screen);
            assertTrue("More settings is open", listed(screen, "advanced"));
            assertFalse("not by default", listed(screen, "debug"));
            prefs().edit().putBoolean(SettingRules.SHOW_DEBUG, true).commit();
            settle(screen);
            assertTrue("with the switch, at once", listed(screen, "debug"));
            prefs().edit().putBoolean(SettingRules.SHOW_DEBUG, false).commit();
            settle(screen);
            assertFalse(listed(screen, "debug"));
            prefs().edit().putBoolean("checkbox_link_probe", true).commit();
            settle(screen);
            assertTrue("a changed Debug setting keeps it in sight", listed(screen, "debug"));
            prefs().edit().putBoolean("checkbox_link_probe", false).commit();
            settle(screen);
            assertFalse("back at its default", listed(screen, "debug"));
        }
    }

    @Test
    public void searchFindsADebugSettingAndALinkStillOpensDebug() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment results = search(screen, "Test the link");
            assertNotNull("found with the switch off", resultGroup(results, "checkbox_link_probe"));
            result(results, "checkbox_link_probe").getOnPreferenceClickListener().onPreferenceClick(result(results, "checkbox_link_probe"));
            settle(screen);
            StreamSettings.SettingsFragment debug = (StreamSettings.SettingsFragment) shown(screen);
            assertTrue("the result opens Debug", visible(debug).contains("checkbox_link_probe"));
            assertTrue("listed while it shows", screen.debugListed());
        }
        Intent link = StreamSettings.intent(context(), null, "checkbox_link_probe");
        ActivityController<StreamSettings> built = Robolectric.buildActivity(StreamSettings.class, link);
        built.get().devices = devices;
        built.get().savedPcReader = task -> { };
        try (ActivityController<StreamSettings> controller = built.setup()) {
            settle(controller.get());
            assertTrue("a link into Debug opens it", visible((StreamSettings.SettingsFragment) shown(controller.get())).contains("checkbox_link_probe"));
        }
    }

    /** A row of SettingRules.WHEN_ROWS: its section, what makes it do nothing, what makes it act, and a changed value. */
    private static final class Row {
        final String key, section;
        final BiConsumer<SharedPreferences.Editor, Devices> off, on, changed;

        Row(String key, String section, BiConsumer<SharedPreferences.Editor, Devices> off,
                BiConsumer<SharedPreferences.Editor, Devices> on, BiConsumer<SharedPreferences.Editor, Devices> changed) {
            this.key = key;
            this.section = section;
            this.off = off;
            this.on = on;
            this.changed = changed;
        }
    }

    private static final BiConsumer<SharedPreferences.Editor, Devices> NOTHING = (e, d) -> { };

    private static BiConsumer<SharedPreferences.Editor, Devices> touch(int mode) {
        return (e, d) -> TouchMode.save(e, mode);
    }

    private static BiConsumer<SharedPreferences.Editor, Devices> on(String key) {
        return (e, d) -> e.putBoolean(key, true);
    }

    private static BiConsumer<SharedPreferences.Editor, Devices> set(String key, String value) {
        return (e, d) -> e.putString(key, value);
    }

    private static BiConsumer<SharedPreferences.Editor, Devices> number(String key, int value) {
        return (e, d) -> e.putInt(key, value);
    }

    private static final List<Row> ROWS = Arrays.asList(
            new Row("adaptive_quality_min_resolution", "video", NOTHING, set("adaptive_quality_mode", "motion"),
                    set("adaptive_quality_min_resolution", "540")),
            new Row("adaptive_quality_min_fps", "video", NOTHING, set("adaptive_quality_mode", "motion"),
                    set("adaptive_quality_min_fps", "60")),
            new Row(PreferenceConfiguration.THERMAL_PROTECTION_PREF_STRING, "video", NOTHING, set("adaptive_quality_mode", "motion"),
                    on(PreferenceConfiguration.THERMAL_PROTECTION_PREF_STRING)),
            new Row("upscaling_sharpness", "display", NOTHING, set("upscaling_mode", "fsr1"), number("upscaling_sharpness", 70)),
            new Row("list_rumble_feel", "input", on("checkbox_disable_rumble"), (e, d) -> e.putBoolean("checkbox_disable_rumble", false),
                    (e, d) -> { e.putBoolean("checkbox_disable_rumble", true); e.putString("list_rumble_feel", "strong"); }),
            new Row("seekbar_mouse_speed", "input", NOTHING, (e, d) -> d.mouse = true, number("seekbar_mouse_speed", 150)),
            new Row("checkbox_mouse_nav_buttons", "input", NOTHING, (e, d) -> d.mouse = true, on("checkbox_mouse_nav_buttons")),
            new Row("seekbar_trackpad_speed", "input", touch(TouchMode.DIRECT), touch(TouchMode.TRACKPAD),
                    (e, d) -> { TouchMode.save(e, TouchMode.DIRECT); e.putInt("seekbar_trackpad_speed", 150); }),
            new Row("seekbar_local_cursor_speed", "input", NOTHING, touch(TouchMode.LOCAL_CURSOR), number("seekbar_local_cursor_speed", 150)),
            new Row("checkbox_trackpad_acceleration_v2", "input", touch(TouchMode.DIRECT), touch(TouchMode.LOCAL_CURSOR),
                    (e, d) -> { TouchMode.save(e, TouchMode.DIRECT); e.putBoolean("checkbox_trackpad_acceleration_v2", false); }),
            new Row("checkbox_trackpad_momentum", "input", touch(TouchMode.DIRECT), touch(TouchMode.TOUCHPAD),
                    (e, d) -> { TouchMode.save(e, TouchMode.DIRECT); e.putBoolean("checkbox_trackpad_momentum", true); }),
            new Row("list_trackpad_area", "input", touch(TouchMode.DIRECT), touch(TouchMode.TRACKPAD),
                    (e, d) -> { TouchMode.save(e, TouchMode.DIRECT); e.putString("list_trackpad_area", "right"); }),
            new Row("checkbox_multi_touch_finger_taps", "input", NOTHING, touch(TouchMode.MULTI_TOUCH), on("checkbox_multi_touch_finger_taps")),
            new Row("seekbar_trackpad_sensitivity_x", "input", NOTHING, (e, d) -> d.touchpad = true, number("seekbar_trackpad_sensitivity_x", 150)),
            new Row("seekbar_trackpad_sensitivity_y", "input", NOTHING, (e, d) -> d.touchpad = true, number("seekbar_trackpad_sensitivity_y", 150)),
            new Row("checkbox_trackpad_swap_axis", "input", NOTHING, (e, d) -> d.touchpad = true, on("checkbox_trackpad_swap_axis")),
            new Row("checkbox_native_touchpad", "input", NOTHING, (e, d) -> d.touchpad = true, on("checkbox_native_touchpad")),
            new Row("checkbox_back_as_meta", "input", NOTHING, (e, d) -> d.keyboard = true, on("checkbox_back_as_meta")),
            new Row("performance_overlay_position", "streaming", NOTHING, (e, d) -> OverlayChoice.save(e, 1),
                    set("performance_overlay_position", "bottom_right")),
            new Row("list_resolution_screen2", "streaming", NOTHING, on("checkbox_dual_screen"), set("list_resolution_screen2", "1280x720")),
            new Row("checkbox_covering_decoder_size", "streaming", NOTHING, set(com.limelight.ui.StreamOrientation.PREF, "rotate"),
                    on("checkbox_covering_decoder_size")),
            new Row("checkbox_experimental_chipset_tuning", "advanced", NOTHING, (e, d) -> d.tunedChip = true,
                    on("checkbox_experimental_chipset_tuning")),
            new Row("game_info_igdb_client_secret", "advanced", NOTHING,
                    (e, d) -> gameInfoKeys().edit().putString(com.limelight.console.GameInfo.KEY_IGDB_ID, "id").commit(),
                    (e, d) -> gameInfoKeys().edit().putString(com.limelight.console.GameInfo.KEY_IGDB_SECRET, "secret").commit()),
            new Row("checkbox_pyrowave_preacquire", "debug", NOTHING, set("video_format", "forcepyrowave"), on("checkbox_pyrowave_preacquire")));

    private void reset(BiConsumer<SharedPreferences.Editor, Devices> state) {
        prefs().edit().clear().commit();
        gameInfoKeys().edit().clear().commit();
        devices.touchpad = devices.mouse = devices.keyboard = devices.folds = devices.tunedChip = false;
        SharedPreferences.Editor editor = prefs().edit();
        state.accept(editor, devices);
        editor.commit();
    }

    @Test
    public void everyWhenRowIsTested() {
        List<String> tested = new ArrayList<>();
        for (Row row : ROWS) tested.add(row.key);
        List<String> rows = new ArrayList<>(SettingRules.WHEN_ROWS);
        // Removed on a device without amplitude control, as Robolectric's vibrator is; its rule is Vibrate device for rumble.
        rows.remove("seekbar_vibrate_fallback_strength");
        assertEquals(rows.size(), tested.size());
        assertTrue(tested.containsAll(rows));
    }

    @Test
    public void aRowThatCanDoNothingLeavesTheListAndComesBackWhenItCan() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            for (Row row : ROWS) {
                reset(row.off);
                StreamSettings.SettingsFragment fragment = open(screen, row.section);
                assertNotNull(row.key, fragment.findPreference(row.key));
                assertFalse(row.key + " does nothing now", visible(fragment).contains(row.key));
                // Without leaving the screen: the setting it follows changes, or the device is attached.
                SharedPreferences.Editor editor = prefs().edit();
                row.on.accept(editor, devices);
                editor.commit();
                fragment.devicesChanged();
                settle(screen);
                assertTrue(row.key + " acts now", visible(fragment).contains(row.key));
            }
        }
    }

    @Test
    public void aRowHoldingAChangedValueStaysAndSaysWhy() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            for (Row row : ROWS) {
                reset(row.off);
                SharedPreferences.Editor editor = prefs().edit();
                row.changed.accept(editor, devices);
                editor.commit();
                StreamSettings.SettingsFragment fragment = open(screen, row.section);
                assertTrue(row.key + " holds a changed value", visible(fragment).contains(row.key));
                String reason = SettingRules.whenReason(context(), row.key, prefs(), devices);
                assertNotNull(row.key + " still does nothing", reason);
                CharSequence summary = fragment.findPreference(row.key).getSummary();
                assertTrue(row.key + ": " + summary, summary != null && summary.toString().contains(reason));
            }
        }
    }

    @Test
    public void searchFindsARowThatDoesNothingAndOpeningItShowsIt() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment results = search(screen, "Trackpad sensitivity X");
            Preference found = result(results, "seekbar_trackpad_sensitivity_x");
            assertNotNull("found without a touchpad", found);
            assertTrue(String.valueOf(found.getSummary()),
                    String.valueOf(found.getSummary()).endsWith(context().getString(R.string.tag_unused_no_touchpad)));
            found.getOnPreferenceClickListener().onPreferenceClick(found);
            settle(screen);
            assertTrue("shown for this visit", visible((StreamSettings.SettingsFragment) shown(screen)).contains("seekbar_trackpad_sensitivity_x"));
        }
    }

    @Test
    public void aRowSetBackToItsDefaultStaysForThisVisit() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            reset(on("checkbox_trackpad_swap_axis"));
            StreamSettings.SettingsFragment fragment = open(screen, "input");
            assertTrue(visible(fragment).contains("checkbox_trackpad_swap_axis"));
            prefs().edit().putBoolean("checkbox_trackpad_swap_axis", false).commit();
            settle(screen);
            assertTrue("the edit keeps its row", visible(fragment).contains("checkbox_trackpad_swap_axis"));
        }
        try (ActivityController<StreamSettings> controller = start()) {
            assertFalse("gone on the next visit", visible(open(controller.get(), "input")).contains("checkbox_trackpad_swap_axis"));
        }
    }

    @Test
    public void aSwitchsOwnRowLeavesWithTheSwitch() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            reset((e, d) -> { TouchMode.save(e, TouchMode.TRACKPAD); e.putBoolean("checkbox_trackpad_momentum", true); });
            StreamSettings.SettingsFragment fragment = open(screen, "input");
            assertTrue(visible(fragment).containsAll(Arrays.asList("list_trackpad_acceleration_curve", "list_trackpad_momentum_glide")));
            // Direct touch: Trackpad acceleration leaves, and its strength with it; the changed Glide after a flick stays.
            SharedPreferences.Editor editor = prefs().edit();
            TouchMode.save(editor, TouchMode.DIRECT);
            editor.commit();
            fragment.devicesChanged();
            settle(screen);
            List<String> rows = visible(fragment);
            assertFalse(rows.contains(PreferenceConfiguration.TRACKPAD_ACCELERATION_PREF_STRING));
            assertFalse("no strength without its switch", rows.contains("list_trackpad_acceleration_curve"));
            assertTrue("changed, so it stays", rows.contains("checkbox_trackpad_momentum"));
            assertTrue(rows.contains("list_trackpad_momentum_glide"));
        }
    }

    @Test
    public void chipsetTuningIsLeftOutOnlyOnAChipKnownNotToBeTuned() {
        assertTrue(SettingDevices.tunedChip("Mediatek", "mt6893"));
        assertTrue(SettingDevices.tunedChip("Samsung", "s5e9925"));
        assertTrue("Tensor may name its decoders c2.exynos", SettingDevices.tunedChip("Google", "gs201"));
        assertFalse(SettingDevices.tunedChip("QTI", "qcom"));
        assertTrue("before Android 12, by its hardware name", SettingDevices.tunedChip(null, "exynos2100"));
        assertFalse(SettingDevices.tunedChip(null, "qcom"));
        assertTrue("an unknown chip may be one", SettingDevices.tunedChip(null, "something"));
    }

    @Test
    public void theMoreCountLeavesOutRowsThatDoNothing() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment fragment = open(screen, "input");
            screen.moreOpen.remove("input");
            fragment.devicesChanged();
            String without = String.valueOf(fragment.findPreference("more_input").getTitle());
            devices.touchpad = true;
            fragment.devicesChanged();
            String with = String.valueOf(fragment.findPreference("more_input").getTitle());
            assertNotEquals("a touchpad adds its rows to the count", without, with);
        }
    }

    @Test
    public void aGamesSettingsShowSharpnessAndDebugByTheSameRule() {
        prefs().edit().putString("upscaling_mode", "off").commit();
        Intent intent = HostStreamSettings.intentForGame(context(), "pc", "PC", "game-uuid", 7, "Racer");
        try (ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup()) {
            PreferenceFragment fragment = (PreferenceFragment) controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
            assertNull("no upscaler that sharpens", fragment.findPreference("sharpness"));
            assertNull("Debug only on request", fragment.findPreference("full_range"));
        }
        AppStreamSettings.save(context(), "pc", "game-uuid", 7, HostStreamProfile.deserialize("v2;sharpness=70;full_range=true"));
        try (ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup()) {
            PreferenceFragment fragment = (PreferenceFragment) controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
            assertNotNull("set by the game", fragment.findPreference("sharpness"));
            assertNotNull("set by the game", fragment.findPreference("full_range"));
        }
        AppStreamSettings.save(context(), "pc", "game-uuid", 7, HostStreamProfile.deserialize("v2"));
        prefs().edit().putBoolean(SettingRules.SHOW_DEBUG, true).putString("upscaling_mode", "sgsr1").commit();
        try (ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup()) {
            PreferenceFragment fragment = (PreferenceFragment) controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
            assertNotNull("an upscaler that sharpens", fragment.findPreference("sharpness"));
            assertNotNull("Show debug settings", fragment.findPreference("full_range"));
        }
    }
}
