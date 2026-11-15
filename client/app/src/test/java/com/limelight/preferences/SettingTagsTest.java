package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.R;
import com.limelight.binding.video.DisplayFrameRatePolicy;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/**
 * "Fixed timing on adaptive screens" (DisplayFrameRatePolicy.adaptiveInUse) and the tags rows give when they do
 * nothing (SettingRules.framePacingUnused, displayQueueRecoveryUnused).
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, qualifiers = "en", application = Application.class)
public class SettingTagsTest {
    private Context context;
    private SharedPreferences prefs;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        prefs = PreferenceManager.getDefaultSharedPreferences(context);
        prefs.edit().clear().commit();
    }

    @After
    public void tearDown() {
        DisplayFrameRatePolicy.arrScreenOverride = null;
    }

    @Test
    public void anArrScreenIsAdaptiveUnlessFixedTimingIsOnWithVrrOff() {
        // arr, vrr, fixed timing
        assertFalse("no ARR: a fixed refresh", DisplayFrameRatePolicy.adaptiveInUse(false, false, false));
        assertFalse(DisplayFrameRatePolicy.adaptiveInUse(false, false, true));
        assertFalse(DisplayFrameRatePolicy.adaptiveInUse(false, true, false));
        assertFalse(DisplayFrameRatePolicy.adaptiveInUse(false, true, true));
        assertTrue("ARR, VRR off, the default", DisplayFrameRatePolicy.adaptiveInUse(true, false, false));
        assertFalse("ARR, VRR off, fixed timing on", DisplayFrameRatePolicy.adaptiveInUse(true, false, true));
        assertTrue("VRR keeps ARR adaptive", DisplayFrameRatePolicy.adaptiveInUse(true, true, false));
        assertTrue("fixed timing does nothing under VRR", DisplayFrameRatePolicy.adaptiveInUse(true, true, true));
    }

    @Test
    public void fixedTimingShowsOnlyOnAnArrScreenWithVrrOff() {
        DisplayFrameRatePolicy.arrScreenOverride = true;
        assertTrue(SettingRules.arrFixedTimingShown(context, prefs));
        prefs.edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true).commit();
        assertFalse("hidden while VRR is on", SettingRules.arrFixedTimingShown(context, prefs));
        prefs.edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, false).commit();
        DisplayFrameRatePolicy.arrScreenOverride = false;
        assertFalse("hidden without ARR", SettingRules.arrFixedTimingShown(context, prefs));
    }

    @Test
    public void theTakeBackSaysWhyItDoesNothing() {
        DisplayFrameRatePolicy.arrScreenOverride = true;
        assertEquals(context.getString(R.string.tag_unused_adaptive_screen), SettingRules.displayQueueRecoveryUnused(context, prefs));
        prefs.edit().putBoolean(PreferenceConfiguration.ARR_FIXED_TIMING_PREF_STRING, true).commit();
        assertNull("fixed timing lets it run", SettingRules.displayQueueRecoveryUnused(context, prefs));
        prefs.edit().putBoolean(PreferenceConfiguration.ARR_FIXED_TIMING_PREF_STRING, false).commit();
        DisplayFrameRatePolicy.arrScreenOverride = false;
        assertNull(SettingRules.displayQueueRecoveryUnused(context, prefs));
        prefs.edit().putString(PreferenceConfiguration.UPSCALING_PREF_STRING, "fsr1").commit();
        assertEquals(context.getString(R.string.tag_unused_with, context.getString(R.string.title_upscaling)),
                SettingRules.displayQueueRecoveryUnused(context, prefs));
    }

    @Test
    @Config(sdk = 34)
    public void theVulkanRendererTagsTheTakeBackAndTheChoicesItIgnores() {
        DisplayFrameRatePolicy.arrScreenOverride = false;
        prefs.edit().putBoolean(PreferenceConfiguration.VULKAN_RENDER_PREF_STRING, true).commit();
        String vulkan = context.getString(R.string.title_vulkan_render);
        assertEquals(context.getString(R.string.tag_unused_with, vulkan), SettingRules.displayQueueRecoveryUnused(context, prefs));
        assertNull("it takes the pacing", SettingRules.framePacingUnused(context, prefs));
        prefs.edit().putString("frame_pacing", "balanced").putBoolean("checkbox_drop_late_frames", false).commit();
        assertEquals(context.getString(R.string.tag_every_frame_with, vulkan), SettingRules.framePacingUnused(context, prefs));
        prefs.edit().putBoolean(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING, true).commit();
        assertEquals(context.getString(R.string.tag_unused_with, vulkan), SettingRules.framePacingUnused(context, prefs));
    }

    @Test
    public void pyroWaveTagsThePcsFrameTimes() {
        prefs.edit().putString("video_format", "forcepyrowave").commit();
        assertNull("PyroWave takes the pacing", SettingRules.framePacingUnused(context, prefs));
        prefs.edit().putBoolean(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING, true).commit();
        assertEquals("Not used with PyroWave", SettingRules.framePacingUnused(context, prefs));
        prefs.edit().putString("video_format", "auto").commit();
        assertNull(SettingRules.framePacingUnused(context, prefs));
    }
}
