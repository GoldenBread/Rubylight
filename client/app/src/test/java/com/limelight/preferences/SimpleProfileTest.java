package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.console.HomeSettings;
import com.limelight.console.OpenPlay;

import org.json.JSONException;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

/**
 * The simple layer on Android: a choice writes the existing settings, a changed advanced setting reads as
 * Custom, Back to recommended restores it, and console mode reuses Home. The mapping itself is held by
 * SimpleProfileModelTest and nightjar-policy's tests; this class uses a stand-in model so it runs in a JVM.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = android.app.Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SimpleProfileTest {
    private Context context;
    private SharedPreferences prefs;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        prefs = PreferenceManager.getDefaultSharedPreferences(context);
        prefs.edit().clear().commit();
        SimpleProfile.forgetCapabilities();
        SimpleProfileModel.use(SimpleProfileTest::fake);
    }

    @After
    public void tearDown() {
        prefs.edit().clear().commit();
        SimpleProfileModel.use(null);
    }

    /** A small stand-in for the policy: the choices' headline values, enough for the app-side wiring. */
    private static String fake(String request) {
        try {
            JSONObject ask = new JSONObject(request);
            String op = ask.getString("op");
            if ("recommended".equals(op)) {
                return "{\"ok\":{\"smoothness\":\"auto\",\"picture\":\"auto\",\"hdr\":\"auto\"}}";
            }
            if ("chosen".equals(op)) {
                JSONObject current = ask.getJSONObject("current");
                boolean balanced = "latency".equals(current.isNull("pacing") ? null : current.getString("pacing"))
                        && !current.getBoolean("vrr")
                        && current.isNull("bitrate_milli") && "auto".equals(current.getString("codec"))
                        && current.getBoolean("hdr") && !current.getBoolean("adaptive_bitrate")
                        && "off".equals(current.isNull("adaptive_quality") ? null : current.getString("adaptive_quality"));
                return balanced ? "{\"ok\":{\"choices\":{\"smoothness\":\"auto\",\"picture\":\"auto\",\"hdr\":\"auto\"}}}"
                        : "{\"ok\":{\"choices\":null}}";
            }
            String smoothness = ask.optString("smoothness", "auto");
            String picture = ask.optString("picture", "auto");
            String hdr = ask.optString("hdr", "auto");
            JSONObject values = new JSONObject().put("native_resolution", true)
                    .put("fps", JSONObject.NULL)
                    .put("pacing", "smoothest".equals(smoothness) ? "smoothest" : "latency")
                    .put("vrr", "balanced".equals(smoothness) && ask.optJSONObject("display") != null
                            && ask.getJSONObject("display").getBoolean("vrr"))
                    .put("latency_switches", true)
                    .put("drop_late_frames", true).put("host_frame_times", false)
                    .put("real_vrr", JSONObject.NULL).put("reduce_refresh", false).put("content_rate", false)
                    .put("bitrate", "best".equals(picture) ? new JSONObject().put("mode", "factor").put("milli", 1500)
                            : "save-data".equals(picture) ? new JSONObject().put("mode", "factor").put("milli", 600)
                            : new JSONObject().put("mode", "default"))
                    .put("codec", "auto")
                    .put("hdr", !"off".equals(hdr))
                    .put("adaptive_bitrate", "save-data".equals(picture))
                    .put("adaptive_quality", "save-data".equals(picture) ? "save-power" : "off");
            return new JSONObject().put("ok", values).toString();
        } catch (JSONException e) {
            throw new AssertionError(e);
        }
    }

    @Test
    public void aChoiceWritesTheExistingSettingsAndKeepsTheOthers() {
        assertTrue(SimpleProfile.apply(context, SimpleProfile.SMOOTHNESS, "smoothest"));
        assertEquals("smoothness", prefs.getString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, null));
        assertFalse(prefs.getBoolean(PreferenceConfiguration.VRR_PREF_STRING, true));
        assertEquals("smoothest", prefs.getString(SimpleProfile.SMOOTHNESS, null));

        // Picture quality changes the bitrate and codec and leaves the pacing as the first choice made it.
        assertTrue(SimpleProfile.apply(context, SimpleProfile.PICTURE, "save-data"));
        assertEquals("auto", prefs.getString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, null));
        assertTrue(prefs.getBoolean(AutoBitrateChoice.SETTINGS_KEY, false));
        assertEquals("battery", prefs.getString("adaptive_quality_mode", null));
        assertEquals("save-data", prefs.getString(SimpleProfile.PICTURE, null));
        assertEquals("smoothest", prefs.getString(SimpleProfile.SMOOTHNESS, null));
        assertTrue("save data picks a lower bitrate than the default",
                prefs.getBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, false));
    }

    @Test
    public void aChangedAdvancedSettingReadsAsCustomAndBackRestoresTheProfile() {
        assertTrue(SimpleProfile.applyRecommended(context));
        assertEquals("auto", prefs.getString(SimpleProfile.SMOOTHNESS, null));
        assertEquals("auto", prefs.getString(SimpleProfile.PICTURE, null));
        assertEquals("auto", prefs.getString(SimpleProfile.HDR, null));
        assertFalse(SimpleProfile.custom(context, SimpleProfile.SMOOTHNESS));

        prefs.edit().putString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, "cap-fps").commit();
        assertTrue(SimpleProfile.custom(context, SimpleProfile.SMOOTHNESS));
        assertTrue(SimpleProfile.custom(context, SimpleProfile.PICTURE));

        assertTrue(SimpleProfile.applyRecommended(context));
        assertFalse(SimpleProfile.custom(context, SimpleProfile.SMOOTHNESS));
        assertEquals("latency", prefs.getString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, null));
    }

    @Test
    public void neverOpeningTheSimpleLayerLeavesEveryDefaultAlone() {
        assertNull(prefs.getString(SimpleProfile.SMOOTHNESS, null));
        assertFalse(prefs.contains(PreferenceConfiguration.RESOLUTION_PREF_STRING));
        assertFalse(prefs.contains(PreferenceConfiguration.FRAME_PACING_PREF_STRING));
        assertFalse(prefs.contains(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING));
    }

    @Test
    public void theCurrentSettingsReadAsTheChoicesTheyAre() {
        assertTrue(SimpleProfile.applyRecommended(context));
        SimpleProfileModel.Current current = SimpleProfile.current(context);
        assertTrue(current.nativeResolution);
        assertNull("the screen's rate is the unstored default", current.fps);
        assertEquals("latency", current.pacing);
        assertFalse(current.vrr);
        assertNull(current.bitrateMilli);
        assertEquals("auto", current.codec);
        assertFalse(current.adaptiveBitrate);
        assertEquals("off", current.adaptiveQuality);
    }

    @Test
    @Config(sdk = 28, application = android.app.Application.class,
            shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
            instrumentedPackages = "com.limelight.nvstream.jni")
    public void openingTheRecommendedSectionChangesNoAdvancedKey() {
        prefs.edit()
                .putString(SimpleProfile.SMOOTHNESS, "balanced")
                .putString(SimpleProfile.PICTURE, "best")
                .putString(SimpleProfile.HDR, "off")
                .putString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, "latency")
                .putString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, "forceh265")
                .putInt(PreferenceConfiguration.BITRATE_PREF_STRING, 12_345)
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, true)
                .putBoolean(PreferenceConfiguration.ENABLE_HDR_PREF_STRING, true)
                .putString(OpenPlay.SETTING, "last")
                .putBoolean(HomeSettings.PREF_KEY, true)
                .commit();
        prefs.edit().putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING,
                PreferenceConfiguration.FOLLOW_SCREEN_VERSION).commit();
        java.util.Map<String, ?> before = new java.util.HashMap<>(prefs.getAll());
        org.robolectric.android.controller.ActivityController<StreamSettings> controller =
                org.robolectric.Robolectric.buildActivity(StreamSettings.class).setup();
        try {
            SettingsStructureTest.open(controller.get(), "simple");
            for (String key : new String[] {PreferenceConfiguration.FRAME_PACING_PREF_STRING,
                    PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, PreferenceConfiguration.BITRATE_PREF_STRING,
                    PreferenceConfiguration.ENABLE_HDR_PREF_STRING, OpenPlay.SETTING, HomeSettings.PREF_KEY}) {
                assertEquals(key, before.get(key), prefs.getAll().get(key));
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void bindingSimpleRowsNeverWritesAProfileOrTheStartScreen() {
        prefs.edit()
                .putString(SimpleProfile.SMOOTHNESS, "balanced")
                .putString(SimpleProfile.PICTURE, "best")
                .putString(SimpleProfile.HDR, "off")
                .putString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, "latency")
                .putString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, "forceh265")
                .putInt(PreferenceConfiguration.BITRATE_PREF_STRING, 12_345)
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, true)
                .putBoolean(PreferenceConfiguration.ENABLE_HDR_PREF_STRING, true)
                .putString(OpenPlay.SETTING, "last")
                .putBoolean(HomeSettings.PREF_KEY, true)
                .commit();
        prefs.edit().putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING,
                PreferenceConfiguration.FOLLOW_SCREEN_VERSION).commit();
        java.util.Map<String, ?> before = new java.util.HashMap<>(prefs.getAll());
        for (String key : new String[] {SimpleProfile.SMOOTHNESS, SimpleProfile.PICTURE, SimpleProfile.HDR}) {
            SimpleProfilePreference row = new SimpleProfilePreference(context, null);
            row.setKey(key);
            row.onSetInitialValue(true, null);
        }
        ConsoleModePreference console = new ConsoleModePreference(context, null);
        console.setKey(HomeSettings.PREF_KEY);
        console.onSetInitialValue(true, true);
        for (String key : new String[] {PreferenceConfiguration.FRAME_PACING_PREF_STRING,
                PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, PreferenceConfiguration.BITRATE_PREF_STRING,
                PreferenceConfiguration.ENABLE_HDR_PREF_STRING, OpenPlay.SETTING, HomeSettings.PREF_KEY}) {
            assertEquals(key, before.get(key), prefs.getAll().get(key));
        }
    }

    @Test
    public void aMeasuredCapSnapsToAnOfferedFrameRate() {
        assertEquals(90, SimpleProfile.offeredFps(context, 109));
        assertEquals(60, SimpleProfile.offeredFps(context, 60));
        assertEquals(30, SimpleProfile.offeredFps(context, 24));
        assertEquals(16667, SimpleProfile.benchmarkUsAtNative(1920, 1080, 16667));
        assertTrue("a 1440p screen decodes slower than the 1080p clip",
                SimpleProfile.benchmarkUsAtNative(2560, 1440, 16667) > 16667);
    }

    @Test
    public void balancedRestoresTheVrrSmoothestTurnedOff() {
        prefs.edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true).commit();
        assertTrue(SimpleProfile.apply(context, SimpleProfile.SMOOTHNESS, "smoothest"));
        assertFalse(prefs.getBoolean(PreferenceConfiguration.VRR_PREF_STRING, true));
        assertTrue("the player's opt-in is kept", prefs.getBoolean(SimpleProfile.VRR_OPT_IN, false));
        assertTrue(SimpleProfile.apply(context, SimpleProfile.SMOOTHNESS, "balanced"));
        assertTrue("Balanced restores it", prefs.getBoolean(PreferenceConfiguration.VRR_PREF_STRING, false));
    }

    @Test
    public void aPcOverrideReadsAsCustom() {
        HostStreamProfile profile = new HostStreamProfile();
        profile.hdr = true;
        context.getSharedPreferences("HostStreamProfiles", Context.MODE_PRIVATE).edit()
                .putString("pc", profile.serialize()).commit();
        assertNull("a PC's own value wins over the simple layer", SimpleProfile.chosen(context));
        context.getSharedPreferences("HostStreamProfiles", Context.MODE_PRIVATE).edit().clear().commit();
    }

    @Test
    public void consoleModeReusesHomeAndThePcList() {
        ConsoleModePreference console = new ConsoleModePreference(context, null);
        console.setKey(HomeSettings.PREF_KEY);
        prefs.edit().putString(OpenPlay.SETTING, "last").commit();

        assertTrue(console.persistBoolean(true));
        assertTrue(HomeSettings.enabled(context));
        assertEquals("pcs", prefs.getString(OpenPlay.SETTING, null));

        assertTrue(console.persistBoolean(false));
        assertFalse(HomeSettings.enabled(context));
        assertEquals("pcs", prefs.getString(OpenPlay.SETTING, null));
    }

    @Test
    public void bindingConsoleModeKeepsLastAndChosen() {
        for (String screen : new String[] {"last", "chosen"}) {
            prefs.edit().putString(OpenPlay.SETTING, screen).putBoolean(HomeSettings.PREF_KEY, true).commit();
            ConsoleModePreference console = new ConsoleModePreference(context, null);
            console.setKey(HomeSettings.PREF_KEY);
            console.onSetInitialValue(true, true);
            assertEquals(screen, prefs.getString(OpenPlay.SETTING, null));
            assertTrue(prefs.getBoolean(HomeSettings.PREF_KEY, false));
        }
    }
}
