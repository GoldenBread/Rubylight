package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.pm.PackageManager;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;
import android.preference.TwoStatePreference;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

import static com.limelight.preferences.SettingsStructureTest.keys;
import static com.limelight.preferences.SettingsStructureTest.open;
import static com.limelight.preferences.SettingsStructureTest.resultGroup;
import static com.limelight.preferences.SettingsStructureTest.search;
import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Settings › Advanced › Compatibility › TV display compatibility: off by default, listed only on a TV
 * (FEATURE_LEANBACK) and behind Advanced's More row there, and read from the stored value on any device.
 * The preference part follows the derflacco fork's TvCompositorPreferenceTest (branch Artemide_Experimental_Async, GPL-3.0, commit 784b4eca).
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, qualifiers = "en", application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class TvDisplayCompatibilitySettingTest {
    private static final String KEY = "checkbox_tv_display_compatibility";

    private static Context context() {
        return RuntimeEnvironment.getApplication();
    }

    @Before
    public void touchscreen() {
        shadowOf(context().getPackageManager()).setSystemFeature(PackageManager.FEATURE_TOUCHSCREEN, true);
    }

    @After
    public void clear() {
        PreferenceManager.getDefaultSharedPreferences(context()).edit().clear().commit();
        context().getSharedPreferences("SettingsScreenState", Context.MODE_PRIVATE).edit().clear().commit();
    }

    private ActivityController<StreamSettings> start(boolean tv) {
        shadowOf(context().getPackageManager()).setSystemFeature(PackageManager.FEATURE_LEANBACK, tv);
        shadowOf(context().getPackageManager()).setSystemFeature(PackageManager.FEATURE_TOUCHSCREEN, !tv);
        ActivityController<StreamSettings> built = Robolectric.buildActivity(StreamSettings.class);
        built.get().devices = SettingsHideTest.NOTHING_ATTACHED;
        built.get().savedPcReader = task -> { };
        return built.setup();
    }

    private static List<String> visible(StreamSettings.SettingsFragment fragment) {
        List<String> rows = new ArrayList<>();
        PreferenceScreen screen = fragment.getPreferenceScreen();
        for (int i = 0; i < screen.getPreferenceCount(); i++) rows.addAll(keys((PreferenceGroup) screen.getPreference(i)));
        return rows;
    }

    @Test
    public void offByDefaultAndReadFromTheStoredValueOnAnyDevice() {
        assertFalse(PreferenceConfiguration.readPreferences(context()).tvDisplayCompatibility);
        PreferenceManager.getDefaultSharedPreferences(context()).edit().putBoolean(KEY, true).commit();
        assertTrue("a phone forced on through the preference", PreferenceConfiguration.readPreferences(context()).tvDisplayCompatibility);
    }

    @Test
    public void notOfferedOnAPhone() {
        try (ActivityController<StreamSettings> controller = start(false)) {
            StreamSettings screen = controller.get();
            assertFalse(visible(open(screen, "advanced")).contains(KEY));
            assertNull("search does not find it", resultGroup(search(screen, "TV display compatibility"), KEY));
        }
    }

    @Test
    public void offeredOnATvBehindAdvancedsMoreRowAndOff() {
        try (ActivityController<StreamSettings> controller = start(true)) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment advanced = open(screen, "advanced");
            assertTrue(visible(advanced).contains(KEY));
            assertTrue("a rarely needed row", StreamSettings.foldsInSection(context(), "advanced", KEY));
            assertFalse(((TwoStatePreference) advanced.findPreference(KEY)).isChecked());
            assertNotNull("search finds it", resultGroup(search(screen, "TV display compatibility"), KEY));
        }
    }
}
