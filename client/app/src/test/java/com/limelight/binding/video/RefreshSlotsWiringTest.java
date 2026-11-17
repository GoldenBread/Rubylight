package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static com.limelight.preferences.PreferenceConfiguration.FRAME_PACING_BALANCED;
import static org.junit.Assert.*;

/**
 * Which release time Balanced's direct path hands to the display: the next free refresh for a
 * stream at the panel's rate (RefreshSlots), and the release time itself ("now") for a faster
 * stream, for a panel that dropped below it, and with debug.nightjar.refresh_slots=0.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class RefreshSlotsWiringTest {
    private static final long INTERVAL = 8_333_333L;
    private static final long LEAD = 10_330_000L;
    /** The last vsync the display's callback reported. */
    private static final long VSYNC = 1_000_000_000L;
    /** Released 3 ms after that vsync; the first refresh it can make is two on. */
    private static final long RELEASE = VSYNC + 3_000_000L;
    private static final long SLOT = VSYNC + 2 * INTERVAL;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
    }

    private MediaCodecDecoderRenderer renderer(int fpsMilliHz, long refreshIntervalNs, boolean slots) {
        ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.framePacing = FRAME_PACING_BALANCED;
        prefs.dropLateFrames = true;
        MediaCodecDecoderRenderer renderer =
                new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        renderer.setStreamFrameRateMilliHz(fpsMilliHz);
        ReflectionHelpers.setField(renderer, "outputVsyncAvailable", true);
        ReflectionHelpers.setField(renderer, "vsyncTimeNs", VSYNC);
        ReflectionHelpers.setField(renderer, "vsyncIntervalNs", refreshIntervalNs);
        ReflectionHelpers.setField(renderer, "vsyncPresentationDeadlineNs", LEAD);
        ReflectionHelpers.setField(renderer, "refreshSlotsEnabled", slots);
        return renderer;
    }

    private static long plan(MediaCodecDecoderRenderer renderer) {
        return ReflectionHelpers.callInstanceMethod(renderer, "planRefreshSlot",
                ReflectionHelpers.ClassParameter.from(long.class, RELEASE));
    }

    @Test public void aStreamAtThePanelRateIsStampedForItsRefresh() {
        assertEquals(SLOT - INTERVAL / 2, plan(renderer(120_000, INTERVAL, true)));
    }

    @Test public void aFasterStreamKeepsTheReleaseTime() {
        // 144 fps on 120 Hz has surplus frames at every refresh: a claimed refresh would only hold
        // them a refresh or more behind.
        assertEquals(RELEASE, plan(renderer(144_000, INTERVAL, true)));
        assertEquals(RELEASE, plan(renderer(240_000, INTERVAL, true)));
    }

    @Test public void aPanelBelowTheStreamRateKeepsTheReleaseTime() {
        // 120 fps after the panel dropped to 90 or 60 Hz.
        assertEquals(RELEASE, plan(renderer(120_000, 1_000_000_000L / 90, true)));
        assertEquals(RELEASE, plan(renderer(120_000, 16_666_667L, true)));
    }

    @Test public void theDebugSwitchKeepsTheReleaseTime() {
        // debug.nightjar.refresh_slots=0: the A/B arm without the slot stamp.
        assertEquals(RELEASE, plan(renderer(120_000, INTERVAL, false)));
    }

    @Test public void withoutAVsyncSampleTheReleaseTimeIsUsed() {
        MediaCodecDecoderRenderer renderer = renderer(120_000, INTERVAL, true);
        ReflectionHelpers.setField(renderer, "vsyncTimeNs", 0L);
        assertEquals(RELEASE, plan(renderer));
    }
}
