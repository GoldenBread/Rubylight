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
@Config(sdk = 29)
public class SimpleProfileTest {
    private Context context;
    private SharedPreferences prefs;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        prefs = PreferenceManager.getDefaultSharedPreferences(context);
        prefs.edit().clear().commit();
        SimpleProfileModel.use(SimpleProfileTest::fake);
    }

    @After
    public void tearDown() {
        prefs.edit().clear().commit();
    }

    /** A small stand-in for the policy: the choices' headline values, enough for the app-side wiring. */
    private static String fake(String request) {
        try {
            JSONObject ask = new JSONObject(request);
            String op = ask.getString("op");
            if ("recommended".equals(op)) {
                return "{\"ok\":{\"smoothness\":\"balanced\",\"picture\":\"auto\",\"hdr\":\"auto\"}}";
            }
            if ("chosen".equals(op)) {
                JSONObject current = ask.getJSONObject("current");
                boolean balanced = "balanced".equals(current.isNull("pacing") ? null : current.getString("pacing"))
                        && current.getBoolean("vrr")
                        && current.isNull("bitrate_milli") && "auto".equals(current.getString("codec"))
                        && current.getBoolean("hdr") && !current.getBoolean("adaptive_bitrate")
                        && "off".equals(current.isNull("adaptive_quality") ? null : current.getString("adaptive_quality"));
                return balanced ? "{\"ok\":{\"choices\":{\"smoothness\":\"balanced\",\"picture\":\"auto\",\"hdr\":\"auto\"}}}"
                        : "{\"ok\":{\"choices\":null}}";
            }
            String smoothness = ask.optString("smoothness", "balanced");
            String picture = ask.optString("picture", "auto");
            String hdr = ask.optString("hdr", "auto");
            JSONObject values = new JSONObject().put("native_resolution", true)
                    .put("fps", JSONObject.NULL)
                    .put("pacing", "smoothest".equals(smoothness) ? "smoothest"
                            : "lowest-delay".equals(smoothness) ? "latency" : "balanced")
                    .put("vrr", "balanced".equals(smoothness))
                    .put("latency_switches", true)
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
        assertEquals("balanced", prefs.getString(SimpleProfile.SMOOTHNESS, null));
        assertEquals("auto", prefs.getString(SimpleProfile.PICTURE, null));
        assertEquals("auto", prefs.getString(SimpleProfile.HDR, null));
        assertFalse(SimpleProfile.custom(context, SimpleProfile.SMOOTHNESS));

        prefs.edit().putString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, "cap-fps").commit();
        assertTrue(SimpleProfile.custom(context, SimpleProfile.SMOOTHNESS));
        assertTrue(SimpleProfile.custom(context, SimpleProfile.PICTURE));

        assertTrue(SimpleProfile.applyRecommended(context));
        assertFalse(SimpleProfile.custom(context, SimpleProfile.SMOOTHNESS));
        assertEquals("balanced", prefs.getString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, null));
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
        assertEquals("balanced", current.pacing);
        assertTrue(current.vrr);
        assertNull(current.bitrateMilli);
        assertEquals("auto", current.codec);
        assertFalse(current.adaptiveBitrate);
        assertEquals("off", current.adaptiveQuality);
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
}
