package com.limelight;

import android.app.Application;
import android.content.pm.PackageManager;
import android.graphics.SurfaceTexture;
import android.media.MediaFormat;
import android.view.Display;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.View;
import android.view.WindowManager;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.binding.video.RefreshSwitchMonitorTest;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowSurfaceView;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;

import java.time.Duration;

import static org.junit.Assert.*;

/**
 * Window-level display mode pinning, Android 15 frame-rate power-savings opt-out and video view
 * requested frame rate across phones, Android TV, seamless content-rate steps, Android 16 ARR and
 * foldable panel switches.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 35, application = Application.class, shadows = GameFrameRateTest.ReconfigureBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameWindowRefreshModeTest {
    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void android15PhonePinsWindowModeAndDisablesPowerSavingsThrottleEvenWhenAlreadyAt120Hz() {
        // Issue #168: realme RMX3867 (API 35, 60/120.00001 Hz) starts at mode 2 (120 Hz) from touch
        // boost; leaving preferredDisplayModeId=0 and power-savings balanced=true let the panel drop
        // back to 60 Hz 1 s after UI views went idle.
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f, 120.00001f);
        Display.Mode m120 = RefreshSwitchMonitorTest.mode(2, 120.00001f, 60f);
        try {
            for (Display.Mode initial : new Display.Mode[] {m120, m60}) {
                RefreshSwitchMonitorTest.FakePanel.set(initial, m60, m120);
                Game activity = Robolectric.buildActivity(Game.class).get();
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.width = 2400;
                prefs.height = 1080;
                prefs.fps = 120;
                prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
                StreamView view = new StreamView(activity);
                view.initializeSurface(false);
                ReflectionHelpers.setField(activity, "prefConfig", prefs);
                ReflectionHelpers.setField(activity, "streamView", view);
                ReflectionHelpers.setField(activity, "renderView", view);

                ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");
                WindowManager.LayoutParams attrs = activity.getWindow().getAttributes();
                assertEquals("initial mode " + initial.getModeId(), 2, attrs.preferredDisplayModeId);
                assertEquals("initial mode " + initial.getModeId(), 120.00001f, attrs.preferredRefreshRate, 0f);
                assertFalse("initial mode " + initial.getModeId(), attrs.isFrameRatePowerSavingsBalanced());

                SurfaceTexture texture = new SurfaceTexture(0);
                Surface surface = new Surface(texture);
                try {
                    GameFrameRateTest.FrameRateSurface vote = Shadow.extract(surface);
                    vote(activity, surface);
                    assertEquals(120.00001f, vote.frameRate, 0f);
                    assertEquals(Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE, vote.compatibility);
                    assertEquals(Surface.CHANGE_FRAME_RATE_ALWAYS, vote.strategy);
                    assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
                    assertEquals(120.00001f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);
                    View surfaceChild = view.getChildAt(0);
                    assertEquals(120.00001f, surfaceChild.getRequestedFrameRate(), 0f);
                } finally {
                    surface.release();
                    texture.release();
                }
            }
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
        }
    }

    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void seamlessContentRateVotesUpdateThePinnedWindowModeWithoutBlankingNonSeamlessPanels() {
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f, 90f, 120f);
        Display.Mode m90 = RefreshSwitchMonitorTest.mode(2, 90f, 60f, 120f);
        Display.Mode m120 = RefreshSwitchMonitorTest.mode(3, 120f, 60f, 90f);
        RefreshSwitchMonitorTest.FakePanel.set(m120, m60, m90, m120);
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.width = 2400;
        prefs.height = 1080;
        prefs.fps = 120;
        prefs.vrr = true;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ContentRateRenderer renderer = new ContentRateRenderer(activity, prefs);
        StreamView view = new StreamView(activity);
        view.initializeSurface(false);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 120f);
        ReflectionHelpers.setField(activity, "streamView", view);
        ReflectionHelpers.setField(activity, "renderView", view);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");
        assertEquals(3, activity.getWindow().getAttributes().preferredDisplayModeId);

        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            ReflectionHelpers.setField(activity, "videoSurface", surface);
            ReflectionHelpers.setField(activity, "surfaceCreated", true);
            vote(activity, surface);
            assertEquals(3, activity.getWindow().getAttributes().preferredDisplayModeId);

            // VRR without ARR steps down seamlessly to 90 Hz: the window pin moves with it.
            renderer.contentMilliHz = 90_000;
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "voteContentRate");
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertEquals(90f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);
            assertEquals(90f, view.getChildAt(0).getRequestedFrameRate(), 0f);

            // Withdrawing returns seamlessly to 120 Hz.
            RefreshSwitchMonitorTest.FakePanel.set(m90, m60, m90, m120);
            ShadowSystemClock.advanceBy(Duration.ofSeconds(3));
            renderer.contentMilliHz = 0;
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "voteContentRate");
            assertEquals(3, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertEquals(120f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);
            assertEquals(120f, view.getChildAt(0).getRequestedFrameRate(), 0f);

            // Non-seamless panel: Lower refresh rate content vote must not force a blanking mode pin.
            Display.Mode ns60 = RefreshSwitchMonitorTest.mode(1, 60f);
            Display.Mode ns120 = RefreshSwitchMonitorTest.mode(2, 120f);
            RefreshSwitchMonitorTest.FakePanel.set(ns120, ns60, ns120);
            prefs.vrr = false;
            prefs.reduceRefreshRate = true;
            prefs.contentRateFrameRate = true;
            vote(activity, surface);
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
            ShadowSystemClock.advanceBy(Duration.ofSeconds(3));
            renderer.contentMilliHz = 60_000;
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "voteContentRate");
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void androidTvKeepsSetFrameRateOnlyForSameResolutionRefreshSwitches() {
        Shadows.shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(PackageManager.FEATURE_LEANBACK, true);
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f);
        Display.Mode m120 = RefreshSwitchMonitorTest.mode(2, 120f);
        RefreshSwitchMonitorTest.FakePanel.set(m60, m60, m120);
        try {
            Game activity = Robolectric.buildActivity(Game.class).get();
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.width = 1920;
            prefs.height = 1080;
            prefs.fps = 120;
            ReflectionHelpers.setField(activity, "prefConfig", prefs);
            giveStreamView(activity);

            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");
            assertEquals(0, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertEquals(0f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);

            SurfaceTexture texture = new SurfaceTexture(0);
            Surface surface = new Surface(texture);
            try {
                vote(activity, surface);
                assertEquals(0, activity.getWindow().getAttributes().preferredDisplayModeId);
            } finally {
                surface.release();
                texture.release();
            }
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
        }
    }

    @Test
    @Config(sdk = 36, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void android16ArrClearsPinnedWindowModeAndKeepsPowerSavingsBalanced() {
        Display.Mode m60 = RefreshSwitchMonitorTest.mode(1, 60f, 120f);
        Display.Mode m120 = RefreshSwitchMonitorTest.mode(2, 120f, 60f);
        RefreshSwitchMonitorTest.FakePanel.set(m120, m60, m120);
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.vrr = true;
        prefs.fps = 120;
        StreamView view = new StreamView(activity);
        view.initializeSurface(false);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 120f);
        ReflectionHelpers.setField(activity, "streamView", view);
        ReflectionHelpers.setField(activity, "renderView", view);

        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            // Non-ARR vote first pins the window mode and disables power-savings throttling.
            vote(activity, surface);
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertFalse(activity.getWindow().getAttributes().isFrameRatePowerSavingsBalanced());
            assertEquals(120f, view.getChildAt(0).getRequestedFrameRate(), 0f);

            // Switching to ARR clears the window pin and restores ARR power-savings balance.
            ReflectionHelpers.setField(activity, "useArr", true);
            vote(activity, surface);
            assertEquals(0, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertEquals(0f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);
            assertTrue(activity.getWindow().getAttributes().isFrameRatePowerSavingsBalanced());
            assertEquals(View.REQUESTED_FRAME_RATE_CATEGORY_DEFAULT,
                    view.getChildAt(0).getRequestedFrameRate(), 0f);
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
            surface.release();
            texture.release();
        }
    }

    @Test
    @Config(sdk = 35, shadows = {GameFrameRateTest.FrameRateSurface.class, RefreshSwitchMonitorTest.FakePanel.class})
    public void panelChangeClearsStalePinnedModeIdWhenTheNewPanelLacksTheVotedRate() {
        Display.Mode inner60 = RefreshSwitchMonitorTest.mode(1, 60f, 120f);
        Display.Mode inner120 = RefreshSwitchMonitorTest.mode(2, 120f, 60f);
        RefreshSwitchMonitorTest.FakePanel.set(inner120, inner60, inner120);
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.width = 1920;
        prefs.height = 1080;
        prefs.fps = 120;
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        giveStreamView(activity);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "prepareDisplayForRendering");
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            vote(activity, surface);
            assertEquals(2, activity.getWindow().getAttributes().preferredDisplayModeId);

            // Fold to a 60 Hz-only cover screen whose mode list has only id=1 (no mode 2).
            Display.Mode cover60 = RefreshSwitchMonitorTest.mode(1, 60f);
            RefreshSwitchMonitorTest.FakePanel.set(cover60, cover60);
            ReflectionHelpers.callInstanceMethod(activity.videoOutput, "onDisplayModesChanged");
            assertEquals(0, activity.getWindow().getAttributes().preferredDisplayModeId);
            assertEquals(0f, activity.getWindow().getAttributes().preferredRefreshRate, 0f);
        } finally {
            RefreshSwitchMonitorTest.FakePanel.clear();
            surface.release();
            texture.release();
        }
    }

    private static void giveStreamView(Game activity) {
        SurfaceHolder holder = new ShadowSurfaceView.FakeSurfaceHolder();
        StreamView view = new StreamView(activity) {
            @Override public SurfaceHolder getHolder() { return holder; }
        };
        ReflectionHelpers.setField(activity, "streamView", view);
        ReflectionHelpers.setField(activity, "renderView", view);
    }

    private static void vote(Game activity, Surface surface) {
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "voteSurfaceFrameRate",
                ReflectionHelpers.ClassParameter.from(Surface.class, surface));
    }

    private static final class ContentRateRenderer extends MediaCodecDecoderRenderer {
        int contentMilliHz;

        ContentRateRenderer(Game activity, PreferenceConfiguration prefs) {
            super(activity, prefs, null, 0, false, false, "", null);
            ReflectionHelpers.setField(this, "refreshRate", prefs.fps);
            ReflectionHelpers.setField(this, "configuredFormat", MediaFormat.createVideoFormat("video/avc", 1280, 720));
        }

        @Override public int getContentRateMilliHz() {
            return contentMilliHz;
        }
    }
}
