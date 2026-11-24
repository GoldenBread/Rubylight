package com.limelight.binding.video;

import android.content.Context;
import android.os.Build;
import android.view.Display;
import android.view.Window;
import android.view.WindowManager;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import java.util.List;

public final class DisplayFrameRatePolicy {
    public static boolean useAdaptiveRefreshRate(int sdk, boolean vrr, boolean arrSupported) {
        return NightjarPolicyJni.displayIntGate(0, sdk, arrSupported ? 1 : 0, 0, 0, 0, 0, vrr, 0f) == 1;
    }

    /** Tests stand in for the device's screens with this; null asks Android. */
    @VisibleForTesting
    public static Boolean vrrOfferedOverride;
    /** Tests stand in for an adaptive refresh (ARR) screen with this; null asks Android. */
    @VisibleForTesting
    public static Boolean arrScreenOverride;

    public static boolean arrScreen(Context context) {
        if (arrScreenOverride != null) return arrScreenOverride;
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.BAKLAVA) return false;
        Display display = com.limelight.preferences.PreferenceConfiguration.defaultDisplay(context);
        return display != null && display.hasArrSupport();
    }

    public static boolean adaptiveInUse(boolean arrSupported, boolean vrr, boolean arrFixedTiming) {
        return NightjarPolicyJni.displayBoolGate(0, 0, arrSupported, vrr, arrFixedTiming, false, false, false);
    }

    public static boolean vrrOffered(Context context) {
        if (vrrOfferedOverride != null) return vrrOfferedOverride;
        return true;
    }

    public enum ArrState {
        /** VRR was not asked for. */
        NOT_REQUESTED,
        /** Android's automatic policy decides the refresh rate. */
        IN_USE,
        /** VRR was asked for on Android before 16, which has no adaptive refresh API. */
        ANDROID_TOO_OLD,
        /** Android 16 or newer, but this screen does not report ARR support. */
        PANEL_WITHOUT_ARR
    }

    /** Why the stream does or does not run on ARR, so the overlay can name the reason for the fallback. */
    public static ArrState arrState(int sdk, boolean vrr, boolean arrSupported) {
        switch (NightjarPolicyJni.displayIntGate(0, sdk, arrSupported ? 1 : 0, 0, 0, 0, 0, vrr, 0f)) {
            case 1: return ArrState.IN_USE;
            case 2: return ArrState.ANDROID_TOO_OLD;
            case 3: return ArrState.PANEL_WITHOUT_ARR;
            default: return ArrState.NOT_REQUESTED;
        }
    }

    public static boolean tracksContentRate(boolean enabled, boolean reduceRefreshRate, boolean vrr,
            boolean textureView) {
        return NightjarPolicyJni.displayBoolGate(1, 0, enabled, reduceRefreshRate, vrr, textureView, false, false);
    }

    public static boolean followsContentRate(int sdk, boolean enabled, boolean hostAnnouncesClockSync,
            boolean arr, boolean vrr, boolean textureView) {
        return NightjarPolicyJni.displayBoolGate(
                2, sdk, enabled, hostAnnouncesClockSync, arr, vrr, textureView, false);
    }

    public static float vrrContentVote(float contentHz, float panelMaxHz, boolean mayReduce, float[] seamlessHz) {
        return NightjarPolicyJni.displayRateCalc(
                0, contentHz, panelMaxHz, 0f, mayReduce, seamlessHz != null ? seamlessHz : new float[0]);
    }

    public static float[] seamlessRates(Display display) {
        if (display == null) return new float[0];
        Display.Mode mode = display.getMode();
        float[] alternatives = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ?
                mode.getAlternativeRefreshRates() : new float[0];
        float[] rates = new float[alternatives.length + 1];
        rates[0] = mode.getRefreshRate();
        System.arraycopy(alternatives, 0, rates, 1, alternatives.length);
        return rates;
    }

    public static float contentVote(float contentHz, float baseHz, boolean mayReduce, float panelMaxHz) {
        return NightjarPolicyJni.displayRateCalc(1, contentHz, baseHz, panelMaxHz, mayReduce, null);
    }

    public static float arrVote(float streamHz, float panelMaxHz) {
        return NightjarPolicyJni.displayRateCalc(2, streamHz, panelMaxHz, 0f, false, null);
    }

    /**
     * Under ARR, whether the compositor shows about half the frames the decoder rendered while
     * the display runs at about half the stream rate or less.
     */
    public static final class ArrHalfRateCheck implements AutoCloseable {
        public static final long HOLD_MS = 3000;
        /** Below this the counts are too coarse to call half. */
        public static final float MIN_RENDERED_FPS = 40;
        public static final float LOW = 0.4f;
        public static final float HIGH = 0.6f;

        private final long handle;
        private final HandleReaper.Token reaper;

        public ArrHalfRateCheck() {
            this.handle = NightjarPolicyJni.pacingSlabCreate(NightjarPolicyJni.PACING_SLAB_ARR_HALF_RATE);
            this.reaper = HandleReaper.register(
                    this,
                    handle,
                    h -> NightjarPolicyJni.pacingSlabDestroy(NightjarPolicyJni.PACING_SLAB_ARR_HALF_RATE, h));
        }

        @Override
        public void close() {
            reaper.close();
        }

        public static boolean isHalf(float renderedFps, float presentedFps, float displayHz, float streamHz) {
            return NightjarPolicyJni.arrHalfRateStep(0, -1, 0, renderedFps, presentedFps, displayHz, streamHz);
        }

        /** Takes one sample; returns whether the check is flagged now. */
        public boolean update(long nowMs, float renderedFps, float presentedFps, float displayHz, float streamHz) {
            return NightjarPolicyJni.arrHalfRateStep(
                    handle, 0, nowMs, renderedFps, presentedFps, displayHz, streamHz);
        }

        public boolean flagged() {
            return NightjarPolicyJni.arrHalfRateStep(handle, 1, 0, 0f, 0f, 0f, 0f);
        }

        /** A new surface, stream or stop: start over. */
        public void reset() {
            NightjarPolicyJni.arrHalfRateStep(handle, 2, 0, 0f, 0f, 0f, 0f);
        }
    }

    /**
     * Decides when a changed content rate becomes a new vote: a rate within 3% of the last vote
     * is no change, and votes are at least 2 s apart.
     */
    public static final class ContentRateVote implements AutoCloseable {
        public static final long MIN_INTERVAL_MS = 2000;
        public static final float HYSTERESIS = 0.03f;

        private final long handle;
        private final HandleReaper.Token reaper;

        public ContentRateVote() {
            this.handle = NightjarPolicyJni.pacingSlabCreate(NightjarPolicyJni.PACING_SLAB_CONTENT_RATE_VOTE);
            this.reaper = HandleReaper.register(
                    this,
                    handle,
                    h -> NightjarPolicyJni.pacingSlabDestroy(NightjarPolicyJni.PACING_SLAB_CONTENT_RATE_VOTE, h));
        }

        @Override
        public void close() {
            reaper.close();
        }

        /** The rate in Hz to vote now, or 0 for no new vote. */
        public float next(long nowMs, int contentMilliHz) {
            return NightjarPolicyJni.contentRateVoteStep(handle, 0, nowMs, contentMilliHz);
        }

        /** Whether a content vote should be withdrawn now. */
        public boolean withdraw(long nowMs, int contentMilliHz) {
            return NightjarPolicyJni.contentRateVoteStep(handle, 1, nowMs, contentMilliHz) != 0f;
        }

        /** Something else replaced the vote (a new surface, another stream rate): vote again. */
        public void invalidate() {
            NightjarPolicyJni.contentRateVoteStep(handle, 2, 0, 0);
        }
    }

    public static float maxRefreshRate(Display display) {
        return maxRefreshRate(display, 0, 0);
    }

    public static boolean pinsWindowDisplayMode(int sdk, boolean useArr, boolean useTextureView,
            boolean television, boolean externalDisplay, boolean modeChanged, boolean resolutionChanged) {
        return NightjarPolicyJni.displayBoolGate(
                3, sdk, useArr, useTextureView, television, externalDisplay, modeChanged, resolutionChanged);
    }

    public static boolean syncsWindowDisplayModeWithVote(boolean useArr, boolean useTextureView,
            boolean television, boolean externalDisplay) {
        return NightjarPolicyJni.displayBoolGate(
                4, 0, useArr, useTextureView, television, externalDisplay, false, false);
    }

    public static RefreshSwitchDiagnosis.PanelMode windowModeForVote(int sdk, float voteHz,
            boolean mayReduceRefreshRate, boolean seamlessOnly, Display display) {
        if (display == null) return null;
        return windowModeForVote(sdk, voteHz, mayReduceRefreshRate, seamlessOnly,
                RefreshSwitchMonitor.activeMode(display), RefreshSwitchMonitor.modes(display));
    }

    static RefreshSwitchDiagnosis.PanelMode windowModeForVote(int sdk, float voteHz,
            boolean mayReduceRefreshRate, boolean seamlessOnly,
            RefreshSwitchDiagnosis.PanelMode active, List<RefreshSwitchDiagnosis.PanelMode> modes) {
        if (voteHz <= 0 || active == null || modes == null || modes.isEmpty()) return null;
        int n = modes.size();
        int[] ids = new int[n];
        int[] ws = new int[n];
        int[] hs = new int[n];
        float[] hzs = new float[n];
        for (int i = 0; i < n; i++) {
            RefreshSwitchDiagnosis.PanelMode m = modes.get(i);
            ids[i] = m.id;
            ws[i] = m.width;
            hs[i] = m.height;
            hzs[i] = m.hz;
        }
        int flags = (mayReduceRefreshRate ? 1 : 0) | (seamlessOnly ? 2 : 0);
        int idx = NightjarPolicyJni.displayWindowModeForVote(
                sdk, voteHz, flags, new int[] {active.id, active.width, active.height},
                active.hz, active.alternatives, ids, ws, hs, hzs);
        return idx >= 0 && idx < n ? modes.get(idx) : null;
    }

    /**
     * Updates the video window's display mode, refresh rate and Android 15+ power-savings frame-rate
     * balance only when a field actually changes.
     */
    public static void applyWindowDisplayMode(Window window, int modeId, float refreshRate,
            boolean powerSavingsBalanced) {
        if (window == null) return;
        WindowManager.LayoutParams attributes = window.getAttributes();
        boolean changed = false;
        if (attributes.preferredDisplayModeId != modeId) {
            attributes.preferredDisplayModeId = modeId;
            changed = true;
        }
        if (Float.compare(attributes.preferredRefreshRate, refreshRate) != 0) {
            attributes.preferredRefreshRate = refreshRate;
            changed = true;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM &&
                attributes.isFrameRatePowerSavingsBalanced() != powerSavingsBalanced) {
            attributes.setFrameRatePowerSavingsBalanced(powerSavingsBalanced);
            changed = true;
        }
        if (changed) window.setAttributes(attributes);
    }

    /** Applies the chosen non-ARR display mode to {@code window} according to {@link #pinsWindowDisplayMode}. */
    public static void configureWindowDisplayMode(Window window, Display.Mode currentMode, Display.Mode bestMode,
            boolean useTextureView, boolean television, boolean externalDisplay) {
        if (window == null) return;
        WindowManager.LayoutParams attrs = window.getAttributes();
        boolean modeChanged = currentMode.getModeId() != bestMode.getModeId();
        boolean resolutionChanged = currentMode.getPhysicalWidth() != bestMode.getPhysicalWidth() ||
                currentMode.getPhysicalHeight() != bestMode.getPhysicalHeight();
        boolean pinWindowMode = pinsWindowDisplayMode(Build.VERSION.SDK_INT, false, useTextureView,
                television, externalDisplay, modeChanged, resolutionChanged);
        int targetModeId = pinWindowMode ? bestMode.getModeId() : attrs.preferredDisplayModeId;
        float targetHz = (pinWindowMode && !television && !externalDisplay) || useTextureView ?
                bestMode.getRefreshRate() : attrs.preferredRefreshRate;
        if (!pinWindowMode && modeChanged) {
            LimeLog.info("Using setFrameRate() instead of preferredDisplayModeId due to matching resolution");
        } else if (!pinWindowMode) {
            LimeLog.info("Current display mode is already the best display mode");
        }
        applyWindowDisplayMode(window, targetModeId, targetHz, false);
    }

    /**
     * Keeps the window's pinned display mode in step with surface frame-rate votes on internal
     * non-TV SurfaceView streams, and clears stale mode IDs after a foldable panel change.
     */
    public static void syncWindowDisplayMode(Window window, Display display, float targetHz,
            boolean mayReduceRefreshRate, boolean seamlessOnly, boolean useArr, boolean useTextureView,
            boolean television, boolean externalDisplay, boolean inPictureInPicture) {
        if (window == null || display == null || inPictureInPicture ||
                !syncsWindowDisplayModeWithVote(useArr, useTextureView, television, externalDisplay)) {
            return;
        }
        RefreshSwitchDiagnosis.PanelMode match = windowModeForVote(Build.VERSION.SDK_INT, targetHz,
                mayReduceRefreshRate, seamlessOnly, display);
        if (match != null) {
            applyWindowDisplayMode(window, match.id, match.hz, false);
            return;
        }
        int currentPin = window.getAttributes().preferredDisplayModeId;
        if (currentPin != 0 && !isSameSizeSupportedMode(display, currentPin)) {
            applyWindowDisplayMode(window, 0, 0f, false);
        }
    }

    private static boolean isSameSizeSupportedMode(Display display, int modeId) {
        Display.Mode active = display.getMode();
        for (Display.Mode mode : display.getSupportedModes()) {
            if (mode.getModeId() == modeId &&
                    mode.getPhysicalWidth() == active.getPhysicalWidth() &&
                    mode.getPhysicalHeight() == active.getPhysicalHeight()) {
                return true;
            }
        }
        return false;
    }

    public static float maxRefreshRate(Display display, int streamWidth, int streamHeight) {
        return maxRefreshRate(RefreshSwitchMonitor.activeMode(display), RefreshSwitchMonitor.modes(display),
                streamWidth, streamHeight);
    }

    static float maxRefreshRate(RefreshSwitchDiagnosis.PanelMode active, List<RefreshSwitchDiagnosis.PanelMode> modes,
                                int streamWidth, int streamHeight) {
        if (active == null) return 0f;
        if (modes == null || modes.isEmpty()) return active.hz;
        int n = modes.size();
        int[] ws = new int[n];
        int[] hs = new int[n];
        float[] hzs = new float[n];
        int[] altCounts = new int[n];
        int totalAlts = 0;
        for (int i = 0; i < n; i++) {
            RefreshSwitchDiagnosis.PanelMode m = modes.get(i);
            ws[i] = m.width;
            hs[i] = m.height;
            hzs[i] = m.hz;
            altCounts[i] = m.alternatives.length;
            totalAlts += m.alternatives.length;
        }
        float[] allAlts = new float[totalAlts];
        int offset = 0;
        for (int i = 0; i < n; i++) {
            float[] alts = modes.get(i).alternatives;
            System.arraycopy(alts, 0, allAlts, offset, alts.length);
            offset += alts.length;
        }
        return NightjarPolicyJni.displayMaxRefreshRate(
                active.width, active.height, active.hz, ws, hs, hzs, altCounts, allAlts,
                streamWidth, streamHeight);
    }

    public static boolean modeFitsStream(int modeWidth, int modeHeight, int currentWidth, int currentHeight,
                                         int streamWidth, int streamHeight) {
        return NightjarPolicyJni.displayIntGate(
                3, modeWidth, modeHeight, currentWidth, currentHeight, streamWidth, streamHeight, false, 0f) != 0;
    }

    public static int streamFrameRate(float panelMaxHz) {
        return NightjarPolicyJni.displayIntGate(1, 0, 0, 0, 0, 0, 0, false, panelMaxHz);
    }

    public static boolean fpsFollowsDisplay(boolean vrr, int fps, float panelMaxHzAnySize) {
        return NightjarPolicyJni.displayIntGate(2, fps, 0, 0, 0, 0, 0, vrr, panelMaxHzAnySize) != 0;
    }
}
