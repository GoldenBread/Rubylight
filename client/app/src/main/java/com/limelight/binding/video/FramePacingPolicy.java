package com.limelight.binding.video;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

public final class FramePacingPolicy {
    /** Native fractional V-Sync hold stamp holder with deterministic close() and GC reclamation. */
    static final class HoldStamp implements AutoCloseable {
        private final long handle;
        private final HandleReaper.Token reaper;
        final long[] out = new long[3];

        HoldStamp() {
            this.handle = NightjarPolicyJni.pacingSlabCreate(NightjarPolicyJni.PACING_SLAB_HOLD_STAMP);
            this.reaper = HandleReaper.register(
                    this,
                    handle,
                    h -> NightjarPolicyJni.pacingSlabDestroy(NightjarPolicyJni.PACING_SLAB_HOLD_STAMP, h));
        }

        boolean step(long releaseNs, long vsyncNs, long intervalNs, long deadlineNs,
                     int refreshes, boolean everyFrame, boolean outputVsyncAvailable) {
            return NightjarPolicyJni.holdStampStep(
                    handle, releaseNs, vsyncNs, intervalNs, deadlineNs, refreshes,
                    everyFrame, outputVsyncAvailable, out);
        }

        @Override
        public void close() {
            reaper.close();
        }
    }
    // Keep a renderer turn finite even if decoder output keeps arriving while we catch up.
    static final int MAX_OUTPUT_DRAIN = 64;

    /**
     * The PyroWave presenter's mode for Real VRR, beside the frame pacing values it takes as they
     * are: each frame at the time the shared present clock gives it (present_timing.h, HostTimes).
     */
    static final int PYROWAVE_HOST_TIMES = 4;

    static int pyroWaveMode(int requestedMode, boolean vrr, boolean hostFrameTimes, boolean adaptive) {
        return NightjarPolicyJni.framePacingIntOp(
                0, requestedMode, vrr ? 1 : 0, hostFrameTimes ? 1 : 0, adaptive ? 1 : 0, 0f);
    }

    static boolean usesPyroWaveDisplayClock(int requestedMode, boolean vrr, boolean localDisplay) {
        return NightjarPolicyJni.framePacingBool(0, requestedMode, vrr, localDisplay, false, 0, 0, 0, 0);
    }

    static boolean dropQueuedFrames(int mode) {
        return NightjarPolicyJni.framePacingBool(1, mode, false, false, false, 0, 0, 0, 0);
    }

    static boolean isFrameDue(long dueNs, long vsyncNs, long vsyncIntervalNs) {
        return NightjarPolicyJni.framePacingBool(
                5, 0, false, false, false, dueNs, vsyncNs, vsyncIntervalNs, 0);
    }

    static boolean balancedPacesOnVsync(boolean hostPhaseLock, boolean dropLateFrames, int fpsMilliHz,
                                        long vsyncIntervalNs) {
        return NightjarPolicyJni.framePacingBool(
                6, 0, hostPhaseLock, dropLateFrames, false, 0, 0, vsyncIntervalNs, fpsMilliHz);
    }

    static long nextFrameTimeNs(long dueNs, long vsyncNs, int fps) {
        return nextFrameTimeNsMilliHz(dueNs, vsyncNs, fps * 1000);
    }

    static long nextFrameTimeNsMilliHz(long dueNs, long vsyncNs, int fpsMilliHz) {
        return NightjarPolicyJni.framePacingTimeNs(true, dueNs, vsyncNs, fpsMilliHz);
    }

    static long frameIntervalNs(int fpsMilliHz) {
        long intervalNs = NightjarPolicyJni.framePacingTimeNs(false, 0, 0, fpsMilliHz);
        if (intervalNs < 0) {
            throw new IllegalArgumentException("Invalid stream frame rate");
        }
        return intervalNs;
    }

    public static float limitRefreshHz(float askedHz, float startHz, int fps,
                                       RefreshSwitchDiagnosis.Conditions conditions) {
        return NightjarPolicyJni.framePacingLimitRefreshHz(
                askedHz, startHz, fps, conditions.batterySaver, conditions.thermalCritical,
                conditions.peakSettingHz);
    }

    /** limitedFrameRate: the stream is over 3 fps faster than the screen, so Balanced drops the surplus. */
    public static final int LIMIT_ABOVE_SCREEN = -1;
    /** limitedFrameRate: a refresh of 49 Hz or less is taken as bogus, and Balanced shows the stream. */
    public static final int LIMIT_BOGUS_SCREEN = -2;

    public static int limitedFrameRate(int fps, float refreshHz) {
        return NightjarPolicyJni.framePacingIntOp(1, fps, 0, 0, 0, refreshHz);
    }

    static int negotiatedFrameRateMilliHz(int requestedMilliHz, int negotiatedFps) {
        return NightjarPolicyJni.framePacingIntOp(2, requestedMilliHz, negotiatedFps, 0, 0, 0f);
    }

    static boolean schedulesFrames(boolean hostFrameTimes, int framePacing) {
        return NightjarPolicyJni.framePacingBool(2, framePacing, hostFrameTimes, false, false, 0, 0, 0, 0);
    }

    static boolean recoversDecoderDebt(boolean hostFrameTimes, int framePacing) {
        return NightjarPolicyJni.framePacingBool(3, framePacing, hostFrameTimes, false, false, 0, 0, 0, 0);
    }

    static boolean shouldDrainOutput(int mode, long enqueueNs, long nowNs, int fpsMilliHz, int drained) {
        return NightjarPolicyJni.framePacingBool(7, mode, false, false, false, enqueueNs, nowNs, drained, fpsMilliHz);
    }

    static boolean applyOutputPacingFallback(boolean isPyroWave, boolean forcePyroWave,
                                             boolean outputVsyncAvailable, int framePacing) {
        return NightjarPolicyJni.framePacingBool(
                4, framePacing, isPyroWave, forcePyroWave, outputVsyncAvailable, 0, 0, 0, 0);
    }

    /** How far a refresh may be from a whole multiple of the stream's rate, per multiple. */
    static final double RATE_TOLERANCE_HZ = 0.5;
    /** The most refreshes fractional V-Sync holds a frame for. */
    static final int MAX_HELD_REFRESHES = 4;

    public static int refreshesPerFrame(double refreshHz, double fps, boolean vrr, boolean fractional) {
        return NightjarPolicyJni.vsyncRefreshesPerFrame(refreshHz, fps, vrr, fractional);
    }

    static long heldTargetNs(long earliestNs, long lastTargetNs, long vsyncNs, long intervalNs, int refreshes) {
        return heldTargetNs(earliestNs, lastTargetNs, vsyncNs, intervalNs, refreshes, false);
    }

    static long heldTargetNs(long earliestNs, long lastTargetNs, long vsyncNs, long intervalNs, int refreshes,
                             boolean everyFrame) {
        return NightjarPolicyJni.vsyncTiming(
                4, earliestNs, lastTargetNs, vsyncNs, intervalNs, refreshes, everyFrame);
    }

    static final long EARLY_FOR_NS = 2_000_000_000L;
    static final long MAX_REPORT_PERIOD_NS = 41_700_000L;
    static final long EARLY_SPARE_NS = 3_000_000L;

    static boolean earlyForTurn(long releaseNs, long turnNs, long intervalNs, long deadlineNs) {
        return NightjarPolicyJni.vsyncTiming(3, releaseNs, turnNs, intervalNs, deadlineNs, 0, false) != 0;
    }

    static long[] heldReportGrid(long vsyncNs, long intervalNs, long anchorNs, int refreshes) {
        long[] out = new long[2];
        heldReportGrid(vsyncNs, intervalNs, anchorNs, refreshes, out);
        return out;
    }

    static void heldReportGrid(long vsyncNs, long intervalNs, long anchorNs, int refreshes, long[] out) {
        NightjarPolicyJni.vsyncHeldReportGrid(vsyncNs, intervalNs, anchorNs, refreshes, out);
    }

    static boolean hasOutputDrainTimeRemaining(long elapsedNs, int fpsMilliHz) {
        return NightjarPolicyJni.framePacingBool(8, 0, false, false, false, elapsedNs, 0, 0, fpsMilliHz);
    }
}
