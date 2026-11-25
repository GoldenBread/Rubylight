package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;

import com.limelight.R;
import com.limelight.binding.video.DisplayFrameRatePolicy;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

import static com.limelight.preferences.SettingsKeysTest.RETIRED;
import static com.limelight.preferences.SettingsKeysTest.keysInXml;
import static com.limelight.preferences.SettingsKeysTest.prefs;
import static org.junit.Assert.*;

/** Preference migrations, composite picker choices, and Reset-all clearing behavior. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsMigrationsAndChoicesTest {
    @Before
    public void aScreenWithArr() {
        DisplayFrameRatePolicy.vrrOfferedOverride = true;
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
        DisplayFrameRatePolicy.vrrOfferedOverride = null;
    }

    @Test
    public void renamedSettingsKeepTheirStoredValueUnderTheNewName() {
        for (String[] rename : PreferenceConfiguration.RENAMED_PREFS) {
            assertTrue(rename[0], RETIRED.contains(rename[0]));
        }
        prefs().edit().putBoolean("checkbox_auto_bitrate", true).putBoolean("adaptive_bitrate", false).commit();
        assertTrue(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).autoBitrate);
        assertFalse(prefs().contains("checkbox_auto_bitrate"));
        assertFalse(prefs().contains("adaptive_bitrate"));
        assertEquals(AutoBitrateChoice.SOONER, prefs().getString(AutoBitrateChoice.SETTINGS_KEY, null));
        assertTrue(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).autoBitrate);
        prefs().edit().putBoolean("checkbox_auto_bitrate", false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).autoBitrate);
        assertFalse(prefs().contains("checkbox_auto_bitrate"));
        assertEquals(AutoBitrateChoice.PC, prefs().getString(AutoBitrateChoice.SETTINGS_KEY, null));
    }

    @Test
    public void theThemeMovesFromItsTwoSwitchesToOneValue() {
        Object[][] cases = {
                {false, false, "system"}, {true, false, "dark"}, {true, true, "oled_black"},
                {false, true, "oled_black"},
        };
        for (Object[] row : cases) {
            prefs().edit().clear().putBoolean("checkbox_always_dark", (Boolean) row[0])
                    .putBoolean("checkbox_oled_black", (Boolean) row[1]).commit();
            PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
            assertEquals(row[2], prefs().getString(com.limelight.ui.AppTheme.THEME, null));
            assertFalse(prefs().contains("checkbox_always_dark"));
            assertFalse(prefs().contains("checkbox_oled_black"));
        }
        prefs().edit().putString(com.limelight.ui.AppTheme.THEME, "oled_black").putBoolean("checkbox_always_dark", false).commit();
        assertEquals("system", com.limelight.ui.AppTheme.theme(prefs()));
        PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
        assertEquals("system", prefs().getString(com.limelight.ui.AppTheme.THEME, null));
        assertFalse(com.limelight.ui.AppTheme.isDark(RuntimeEnvironment.getApplication()));
    }

    @Test
    public void theControllerStyleIsGoneAndOnlyAModernChoiceSkipsTheClassicCarryOver() {
        Application app = RuntimeEnvironment.getApplication();
        SharedPreferences controller = app.getSharedPreferences("osc_modern", android.content.Context.MODE_PRIVATE);
        controller.edit().clear().commit();
        prefs().edit().putString("list_osc_style", "classic").commit();
        PreferenceConfiguration.readPreferences(app);
        assertFalse(prefs().contains("list_osc_style"));
        assertFalse(controller.contains("classic-migrated"));
        assertTrue(controller.getBoolean("classic-style", false));

        controller.edit().clear().commit();
        prefs().edit().putString("list_osc_style", "modern").commit();
        PreferenceConfiguration.readPreferences(app);
        assertFalse(controller.contains("classic-style"));
        assertFalse(prefs().contains("list_osc_style"));
        assertTrue(controller.getBoolean("classic-migrated", false));

        controller.edit().clear().commit();
        prefs().edit().putInt("list_osc_style", 1).commit();
        PreferenceConfiguration.readPreferences(app);
        PreferenceConfiguration.readPreferences(app);
        assertFalse(prefs().contains("list_osc_style"));
        assertFalse(controller.contains("classic-migrated"));
        assertTrue(controller.getBoolean("classic-style", false));
    }

    @Test
    public void renderThroughOpenGlUsersMoveToVulkanOnce() {
        SharedPreferences p = prefs();
        p.edit().putBoolean("checkbox_gl_render", true).commit();
        assertTrue(PreferenceConfiguration.migrateRetiredGlRender(p, 33));
        assertFalse(p.contains("checkbox_gl_render"));
        assertTrue(p.getBoolean("checkbox_vulkan_render", false));
        assertTrue(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vulkanRender);

        p.edit().putBoolean("checkbox_vulkan_render", false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vulkanRender);
    }

    @Test
    public void openGlUsersWithAnUpscalerOrAnOldAndroidDoNotGetVulkan() {
        SharedPreferences p = prefs();
        p.edit().putBoolean("checkbox_gl_render", true).putString("upscaling_mode", "fsr1").commit();
        assertFalse(PreferenceConfiguration.migrateRetiredGlRender(p, 33));
        assertFalse(p.contains("checkbox_gl_render"));
        assertFalse(p.contains("checkbox_vulkan_render"));

        p.edit().putBoolean("checkbox_gl_render", true).remove("upscaling_mode").commit();
        assertFalse(PreferenceConfiguration.migrateRetiredGlRender(p, 28));
        assertFalse(p.contains("checkbox_gl_render"));
        assertFalse(p.contains("checkbox_vulkan_render"));

        p.edit().putBoolean("checkbox_gl_render", true).putString("upscaling_mode", "off").commit();
        assertTrue(PreferenceConfiguration.migrateRetiredGlRender(p, 29));
        assertTrue(p.getBoolean("checkbox_vulkan_render", false));
    }

    @Test
    public void anExplicitVulkanChoiceBeatsTheOldOpenGlSwitch() {
        SharedPreferences p = prefs();
        p.edit().putBoolean("checkbox_gl_render", true).putBoolean("checkbox_vulkan_render", false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vulkanRender);
        assertFalse(p.contains("checkbox_gl_render"));
    }

    @Test
    public void anOffOpenGlSwitchLeavesVulkanUnsetAndIsDropped() {
        SharedPreferences p = prefs();
        p.edit().putBoolean("checkbox_gl_render", false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vulkanRender);
        assertFalse(p.contains("checkbox_gl_render"));
        assertFalse(p.contains("checkbox_vulkan_render"));
    }

    @Test
    public void touchModeKeepsTheOlderSwitchesAndAddsTheNewerModes() {
        Application app = RuntimeEnvironment.getApplication();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
        assertEquals(com.limelight.binding.input.touch.TouchMode.TRACKPAD, config.touchMode);
        assertTrue(config.touchscreenTrackpad);

        prefs().edit().putBoolean("checkbox_touchscreen_trackpad", false).putBoolean("checkbox_native_touch", true).commit();
        assertEquals(com.limelight.binding.input.touch.TouchMode.MULTI_TOUCH,
                PreferenceConfiguration.readPreferences(app).touchMode);

        for (int mode : new int[] {com.limelight.binding.input.touch.TouchMode.LOCAL_CURSOR,
                com.limelight.binding.input.touch.TouchMode.OFF, com.limelight.binding.input.touch.TouchMode.TOUCHPAD,
                com.limelight.binding.input.touch.TouchMode.DIRECT}) {
            SharedPreferences.Editor editor = prefs().edit();
            com.limelight.binding.input.touch.TouchMode.save(editor, mode);
            editor.commit();
            config = PreferenceConfiguration.readPreferences(app);
            assertEquals(mode, config.touchMode);
            assertEquals(mode != com.limelight.binding.input.touch.TouchMode.DIRECT,
                    prefs().getBoolean("checkbox_touchscreen_trackpad", true));
            assertEquals(mode != com.limelight.binding.input.touch.TouchMode.DIRECT, config.touchscreenTrackpad);
            assertFalse(prefs().getBoolean("checkbox_native_touch", true));
        }
    }

    @Test
    public void videoScalingKeepsTheOlderStretchSwitchAndAddsTheNewerModes() {
        Application app = RuntimeEnvironment.getApplication();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
        assertEquals(com.limelight.ui.VideoScaling.FIT, config.videoScaling);
        assertFalse(config.stretchVideo);
        assertEquals(com.limelight.ui.VideoScaling.CENTER, config.videoAlignment);

        prefs().edit().putBoolean("checkbox_stretch_video", true).commit();
        config = PreferenceConfiguration.readPreferences(app);
        assertEquals(com.limelight.ui.VideoScaling.STRETCH, config.videoScaling);
        assertTrue(config.stretchVideo);

        for (int mode = com.limelight.ui.VideoScaling.FIT; mode <= com.limelight.ui.VideoScaling.NATIVE; mode++) {
            SharedPreferences.Editor editor = prefs().edit();
            com.limelight.ui.VideoScaling.save(editor, mode);
            editor.commit();
            config = PreferenceConfiguration.readPreferences(app);
            assertEquals(mode, config.videoScaling);
            boolean stretch = mode == com.limelight.ui.VideoScaling.STRETCH;
            assertEquals(stretch, prefs().getBoolean("checkbox_stretch_video", !stretch));
            assertEquals(stretch, config.stretchVideo);
        }

        SharedPreferences.Editor editor = prefs().edit();
        com.limelight.ui.VideoScaling.save(editor, com.limelight.ui.VideoScaling.FILL);
        editor.putBoolean("checkbox_stretch_video", true).commit();
        assertEquals(com.limelight.ui.VideoScaling.STRETCH, PreferenceConfiguration.readPreferences(app).videoScaling);

        prefs().edit().putString("video_alignment", "top").commit();
        assertEquals(com.limelight.ui.VideoScaling.TOP, PreferenceConfiguration.readPreferences(app).videoAlignment);
    }

    @Test
    public void sharperColoredTextReadsEitherOldSwitchAsAutomatic() {
        Application app = RuntimeEnvironment.getApplication();
        Boolean[][] cases = {
                {null, null, false}, {false, null, false}, {null, false, false}, {false, false, false},
                {true, null, true}, {null, true, true}, {true, false, true}, {false, true, true}, {true, true, true},
        };
        for (Boolean[] c : cases) {
            SharedPreferences.Editor editor = prefs().edit().clear();
            if (c[0] != null) editor.putBoolean("checkbox_yuv444", c[0]);
            if (c[1] != null) editor.putBoolean("experimental_packed_444", c[1]);
            editor.commit();
            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
            String name = c[0] + "/" + c[1];
            assertEquals(name, c[2], config.enableYuv444);
            assertEquals(name, c[2], config.experimentalPacked444);
            assertEquals(name, c[2], PreferenceConfiguration.sharpColor(prefs()));
            assertEquals(name, c[0], prefs().contains("checkbox_yuv444") ? prefs().getBoolean("checkbox_yuv444", false) : null);
        }
        SharedPreferences.Editor editor = prefs().edit().clear();
        PreferenceConfiguration.saveSharpColor(editor, true);
        editor.commit();
        assertTrue(prefs().getBoolean("checkbox_yuv444", false) && prefs().getBoolean("experimental_packed_444", false));
        editor = prefs().edit();
        PreferenceConfiguration.saveSharpColor(editor, false);
        editor.commit();
        assertFalse(prefs().getBoolean("checkbox_yuv444", true) || prefs().getBoolean("experimental_packed_444", true));
        PreferenceConfiguration layered = PreferenceConfiguration.readPreferences(app);
        HostStreamProfile.deserialize("v2;yuv444=true").applyTo(layered);
        assertTrue(layered.enableYuv444 && layered.experimentalPacked444);
        HostStreamProfile.deserialize("v2;yuv444=false").applyTo(layered);
        assertFalse(layered.enableYuv444 || layered.experimentalPacked444);
    }

    @Test
    public void controllersReadTheOlderSwitchAsItStreamed() {
        Application app = RuntimeEnvironment.getApplication();
        assertEquals(ControllersChoice.PLAYER_EACH, ControllersChoice.read(prefs()));
        prefs().edit().putBoolean("checkbox_multi_controller", false).commit();
        assertEquals("an older off reads as One player", ControllersChoice.ONE_PLAYER, ControllersChoice.read(prefs()));
        assertTrue(PreferenceConfiguration.readPreferences(app).sendControllers);
        assertFalse(PreferenceConfiguration.readPreferences(app).multiController);

        SharedPreferences.Editor editor = prefs().edit();
        ControllersChoice.save(editor, ControllersChoice.OFF);
        editor.commit();
        assertEquals(ControllersChoice.OFF, ControllersChoice.read(prefs()));
        assertFalse(PreferenceConfiguration.readPreferences(app).sendControllers);
        assertFalse("Off keeps the player choice for when controllers come back",
                prefs().getBoolean("checkbox_multi_controller", true));

        editor = prefs().edit();
        ControllersChoice.save(editor, ControllersChoice.PLAYER_EACH);
        editor.commit();
        assertEquals(ControllersChoice.PLAYER_EACH, ControllersChoice.read(prefs()));
        assertTrue(PreferenceConfiguration.readPreferences(app).multiController);
        editor = prefs().edit();
        ControllersChoice.save(editor, ControllersChoice.ONE_PLAYER);
        editor.commit();
        assertEquals(ControllersChoice.ONE_PLAYER, ControllersChoice.read(prefs()));
    }

    @Test
    public void variableRefreshRateIsOneChoiceOverTheTwoSwitches() {
        Application app = RuntimeEnvironment.getApplication();
        Object[][] cases = {
                {null, null, VrrChoice.OFF}, {false, true, VrrChoice.OFF}, {false, false, VrrChoice.OFF},
                {true, true, VrrChoice.ON}, {true, false, VrrChoice.FASTEST}, {true, null, VrrChoice.ON}};
        for (Object[] c : cases) {
            SharedPreferences.Editor editor = prefs().edit().clear();
            if (c[0] != null) editor.putBoolean("checkbox_vrr", (Boolean) c[0]);
            if (c[1] != null) editor.putBoolean("checkbox_real_vrr", (Boolean) c[1]);
            editor.commit();
            String label = c[0] + "/" + c[1];
            assertEquals(label, c[2], VrrChoice.read(prefs()));
            assertEquals(label, c[2], VrrChoice.shown(prefs(), true));
            assertEquals("where VRR isn't offered every store reads as Off", VrrChoice.OFF, VrrChoice.shown(prefs(), false));
            assertEquals(label + " stays as stored", c[0], prefs().contains("checkbox_vrr") ? prefs().getBoolean("checkbox_vrr", false) : null);
            assertEquals(label + " stays as stored", c[1], prefs().contains("checkbox_real_vrr") ? prefs().getBoolean("checkbox_real_vrr", true) : null);
        }

        prefs().edit().clear().commit();
        SharedPreferences.Editor editor = prefs().edit();
        VrrChoice.save(editor, VrrChoice.FASTEST);
        editor.commit();
        assertTrue(prefs().getBoolean("checkbox_vrr", false));
        assertFalse(prefs().getBoolean("checkbox_real_vrr", true));
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
        assertTrue(config.vrr && !config.realVrr && !config.hostFrameTimes);

        editor = prefs().edit();
        VrrChoice.save(editor, VrrChoice.ON);
        editor.commit();
        assertTrue(prefs().getBoolean("checkbox_vrr", false));
        assertTrue(prefs().getBoolean("checkbox_real_vrr", false));
        config = PreferenceConfiguration.readPreferences(app);
        assertTrue(config.vrr && config.realVrr && config.hostFrameTimes);

        for (int before : new int[] {VrrChoice.ON, VrrChoice.FASTEST}) {
            editor = prefs().edit();
            VrrChoice.save(editor, before);
            editor.commit();
            editor = prefs().edit();
            VrrChoice.save(editor, VrrChoice.OFF);
            editor.commit();
            assertFalse(prefs().getBoolean("checkbox_vrr", true));
            assertEquals(before == VrrChoice.ON, prefs().getBoolean("checkbox_real_vrr", before != VrrChoice.ON));
            assertEquals(VrrChoice.OFF, VrrChoice.read(prefs()));
            assertFalse(PreferenceConfiguration.readPreferences(app).vrr);
        }
        PreferenceConfiguration layered = PreferenceConfiguration.readPreferences(app);
        layered.realVrr = true;
        HostStreamProfile.deserialize("v2;vrr=true;real_vrr=false").applyTo(layered);
        layered.resolveHostFrameTimes();
        assertTrue(layered.vrr && !layered.realVrr && !layered.hostFrameTimes);
        PreferenceConfiguration older = PreferenceConfiguration.readPreferences(app);
        older.realVrr = false;
        HostStreamProfile.deserialize("v2;vrr=true").applyTo(older);
        assertTrue(older.vrr && !older.realVrr);

        assertEquals(Arrays.asList(VrrChoice.VALUES),
                Arrays.asList(app.getResources().getStringArray(R.array.vrr_choice_values)));
        assertEquals(VrrChoice.VALUES.length, app.getResources().getStringArray(R.array.vrr_choice_names).length);
    }

    @Test
    public void whenNightjarOpensReadsTheStoredPairAsOneChoice() {
        String[][] cases = {
                {null, null, StartScreenChoice.HOME}, {"pcs", "true", StartScreenChoice.HOME},
                {"pcs", "false", StartScreenChoice.PCS}, {null, "false", StartScreenChoice.PCS},
                {"last", "true", StartScreenChoice.LAST}, {"last", "false", StartScreenChoice.LAST},
                {"chosen", "true", StartScreenChoice.CHOSEN}, {"chosen", "false", StartScreenChoice.CHOSEN},
                {"a newer screen", "false", StartScreenChoice.PCS}, {"a newer screen", "true", StartScreenChoice.HOME},
        };
        for (String[] c : cases) {
            SharedPreferences.Editor editor = prefs().edit().clear();
            if (c[0] != null) editor.putString("list_open_on_start", c[0]);
            if (c[1] != null) editor.putBoolean("checkbox_console_home", Boolean.parseBoolean(c[1]));
            editor.commit();
            String name = c[0] + "/" + c[1];
            assertEquals(name, c[2], StartScreenChoice.read(prefs()));
            PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
            assertEquals(name, c[0], prefs().getString("list_open_on_start", null));
        }
        prefs().edit().clear().putBoolean("checkbox_console_home", true).commit();
        SharedPreferences.Editor editor = prefs().edit();
        StartScreenChoice.save(prefs(), editor, StartScreenChoice.LAST);
        editor.commit();
        assertEquals("last", prefs().getString("list_open_on_start", null));
        assertTrue("a game choice keeps Home as it was", prefs().getBoolean("checkbox_console_home", false));
        editor = prefs().edit();
        StartScreenChoice.save(prefs(), editor, StartScreenChoice.PCS);
        editor.commit();
        assertEquals("pcs", prefs().getString("list_open_on_start", null));
        assertFalse(prefs().getBoolean("checkbox_console_home", true));
        assertEquals(StartScreenChoice.PCS, StartScreenChoice.read(prefs()));
    }

    @Test
    public void resetAllPutsThePickerChoicesBack() {
        prefs().edit().putBoolean("checkbox_yuv444", true).putBoolean("experimental_packed_444", true)
                .putBoolean("checkbox_send_controllers", false).putBoolean("checkbox_multi_controller", false)
                .putBoolean("checkbox_console_home", false).putString("list_open_on_start", "last").commit();
        StreamSettings.resetAllSettings(RuntimeEnvironment.getApplication());
        assertFalse(PreferenceConfiguration.sharpColor(prefs()));
        assertEquals(ControllersChoice.PLAYER_EACH, ControllersChoice.read(prefs()));
        assertEquals(StartScreenChoice.HOME, StartScreenChoice.read(prefs()));
    }

    @Test
    public void resetAllClearsEveryKeyThePickersWrite() throws Exception {
        prefs().edit().clear().commit();
        StreamSettings.resetAllSettings(RuntimeEnvironment.getApplication());
        Map<String, ?> untouched = new HashMap<>(prefs().getAll());
        prefs().edit().clear().commit();
        SharedPreferences.Editor editor = prefs().edit();
        for (int i = 0; i < 3; i++) {
            ControllersChoice.save(editor, i);
            LowerRefreshChoice.save(editor, i);
            VrrChoice.save(editor, i);
        }
        for (int i = 0; i < 4; i++) ThemeChoice.save(editor, i);
        for (int i = 0; i <= OverlayChoice.MODES.length; i++) OverlayChoice.save(editor, i);
        for (int i = 0; i < DisplayArrangementChoice.ARRANGEMENTS.length; i++) DisplayArrangementChoice.saveMain(editor, i);
        for (String screen : StartScreenChoice.SCREENS) StartScreenChoice.save(prefs(), editor, screen);
        for (SwitchPairChoice pair : SwitchPairChoice.values()) {
            for (int i = 0; i < 3; i++) pair.save(editor, i);
        }
        for (String choice : FramePacingChoice.VALUES) FramePacingChoice.save(editor, choice);
        editor.commit();
        Set<String> pickerOnly = new HashSet<>(prefs().getAll().keySet());
        pickerOnly.removeAll(keysInXml().keySet());
        assertTrue(pickerOnly.contains(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING));
        assertEquals(FramePacingChoice.HOST_TIMES, FramePacingChoice.read(prefs()));

        StreamSettings.resetAllSettings(RuntimeEnvironment.getApplication());

        for (String key : pickerOnly) {
            assertEquals("Reset all settings keeps " + key, untouched.get(key), prefs().getAll().get(key));
        }
        assertEquals(FramePacingChoice.of(PreferenceConfiguration.DEFAULT_FRAME_PACING, true, false),
                FramePacingChoice.read(prefs()));
    }

    @Test
    public void lowLatencyDecodingMovesItsThreeSwitchesTogether() {
        SharedPreferences prefs = prefs();
        PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
        assertEquals(3, PreferenceConfiguration.lowLatencyDecodingCount(prefs));
        PreferenceConfiguration.setLowLatencyDecoding(prefs, false);
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
        assertFalse(config.codecLowLatency);
        assertFalse(config.vendorLowLatency);
        assertFalse(config.phonePerformanceHints);
        assertTrue(config.dropLateFrames && config.unbatchedInput && config.networkPriority && config.codecPerformance);
        PreferenceConfiguration.setLowLatencyDecoding(prefs, true);
        assertEquals(3, PreferenceConfiguration.lowLatencyDecodingCount(prefs));
        for (String key : PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS) {
            assertTrue(Arrays.asList(StreamPreset.LATENCY_SWITCHES).contains(key));
        }
    }
}
