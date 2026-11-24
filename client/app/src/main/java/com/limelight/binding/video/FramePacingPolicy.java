package com.limelight.binding.video;

import com.limelight.preferences.PreferenceConfiguration;

public final class FramePacingPolicy {
    // Keep a renderer turn finite even if decoder output keeps arriving while we catch up.
    static final int MAX_OUTPUT_DRAIN = 64;

    /**
     * The PyroWave presenter's mode for Real VRR, beside the frame pacing values it takes as they
     * are: each frame at the time the shared present clock gives it (present_timing.h, HostTimes).
     */
    static final int PYROWAVE_HOST_TIMES = 4;

    /**
     * The mode the PyroWave presenter runs in. Variable refresh rate replaces frame pacing. With
     * Real VRR on a screen that refreshes when a frame comes (the conditions under which
     * HostFrameClock schedules MediaCodec frames for an adaptive refresh rate), each frame is
     * shown at its time on the PC plus a steady delay. Otherwise VRR shows frames as they arrive:
     * with Real VRR off, and on a screen with a fixed refresh, whose refresh times a frame's time
     * cannot move.
     *
     * @param hostFrameTimes the stream's host frame times switch (PreferenceConfiguration.hostFrameTimes),
     *                       which under VRR is Real VRR
     * @param adaptive       the stream's display refreshes when a frame comes (Android 16 ARR)
     */
    static int pyroWaveMode(int requestedMode, boolean vrr, boolean hostFrameTimes, boolean adaptive) {
        if (!vrr) return requestedMode;
        return hostFrameTimes && adaptive ? PYROWAVE_HOST_TIMES : PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
    }

    /**
     * Whether the PyroWave presenter needs the display's vsync callbacks: only for the modes that
     * put frames onto the vsync grid. Under VRR it shows frames as they arrive or at the PC's
     * frame times, on no grid.
     */
    static boolean usesPyroWaveDisplayClock(int requestedMode, boolean vrr, boolean localDisplay) {
        return !vrr && localDisplay && (requestedMode == PreferenceConfiguration.FRAME_PACING_BALANCED ||
                requestedMode == PreferenceConfiguration.FRAME_PACING_CAP_FPS ||
                requestedMode == PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS);
    }

    static boolean dropQueuedFrames(int mode) {
        return mode != PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS &&
                mode != PreferenceConfiguration.FRAME_PACING_CAP_FPS;
    }

    static boolean isFrameDue(long dueNs, long vsyncNs, long vsyncIntervalNs) {
        return vsyncNs + vsyncIntervalNs / 2 >= dueNs;
    }

    /**
     * Whether Balanced holds decoded frames for the display's vsync callback (issue 7). That aligns
     * a stream slower than the panel to its cadence (60 fps on 120 Hz). When the stream is about as
     * fast as the panel, or faster, every refresh is due anyway: outputs then land next to the
     * callback, jitter puts two between one pair of callbacks and none between the next, and one
     * of the two was dropped (6% at 120 fps on a 120 Hz Retroid, 10% at 60 on a 60 Hz Shield).
     * Those streams are released as they come out, as in Lowest latency, but only to a host that
     * takes phase lock reports: their slack moves the host's capture so outputs land ahead of the
     * latch. Any other host keeps the vsync callback and its queue, and so does Drop late frames
     * off, which asks for that queue. Without a known refresh interval the callback keeps pacing.
     */
    static boolean balancedPacesOnVsync(boolean hostPhaseLock, boolean dropLateFrames, int fpsMilliHz,
                                        long vsyncIntervalNs) {
        if (!hostPhaseLock || !dropLateFrames) return true;
        // A refresh rate of 0 gives no interval or an unbounded one; nothing refreshes slower than 1 Hz.
        if (vsyncIntervalNs <= 0 || vsyncIntervalNs > 1_000_000_000L) return true;
        return frameIntervalNs(fpsMilliHz) * 20 > vsyncIntervalNs * 21;
    }

    static long nextFrameTimeNs(long dueNs, long vsyncNs, int fps) {
        return nextFrameTimeNsMilliHz(dueNs, vsyncNs, fps * 1000);
    }

    static long nextFrameTimeNsMilliHz(long dueNs, long vsyncNs, int fpsMilliHz) {
        long intervalNs = frameIntervalNs(fpsMilliHz);
        if (dueNs == 0) return vsyncNs + intervalNs;
        return dueNs + ((vsyncNs - dueNs) / intervalNs + 1) * intervalNs;
    }

    static long frameIntervalNs(int fpsMilliHz) {
        if (fpsMilliHz < 1000 || fpsMilliHz > 1_000_000) {
            throw new IllegalArgumentException("Invalid stream frame rate");
        }
        return 1_000_000_000_000L / fpsMilliHz;
    }

    /**
     * The refresh Balanced with FPS limit judges its cap by. As in Moonlight that is the rate the screen
     * shows when the stream starts, or the asked mode's where that is lower (startHz, as
     * GameVideoOutput.prepareDisplayForRendering gives it). One case takes the rate of the mode the stream
     * asks for (askedHz) instead: an adaptive screen resting at the stream's own rate (60 Hz for 60 fps)
     * asked for 2 to 4 times it (120 Hz), where 59 fps would run at 120 Hz and show two frames a second
     * for three refreshes. Not where the screen may stay at rest: battery saver, critical heat, or a peak
     * refresh setting below the asked rate (RefreshSwitchMonitor.conditions; unknown is 0). Other reasons
     * it stays (milder heat, a maker's mode, a refused switch) can't be seen, so any other stream keeps
     * Moonlight's rule: 120 fps on a screen resting at 60 Hz goes to Balanced rather than running 119 fps
     * into a queue that fills if the screen stays at 60.
     */
    public static float limitRefreshHz(float askedHz, float startHz, int fps,
                                       RefreshSwitchDiagnosis.Conditions conditions) {
        boolean resting = Math.round(startHz) == fps && refreshesPerFrame(askedHz, startHz, false, true) > 1;
        boolean held = conditions.batterySaver || conditions.thermalCritical ||
                (conditions.peakSettingHz > 0 && askedHz > conditions.peakSettingHz + 0.5f);
        return resting && !held ? askedHz : startHz;
    }

    /** limitedFrameRate: the stream is over 3 fps faster than the screen, so Balanced drops the surplus. */
    public static final int LIMIT_ABOVE_SCREEN = -1;
    /** limitedFrameRate: a refresh of 49 Hz or less is taken as bogus, and Balanced shows the stream. */
    public static final int LIMIT_BOGUS_SCREEN = -2;

    /**
     * The frame rate Balanced with FPS limit asks the PC for, or LIMIT_ABOVE_SCREEN or LIMIT_BOGUS_SCREEN
     * where Balanced takes over. Like Smoothest video it shows every frame in turn, so a stream as fast
     * as the screen would fill the display's queue: it is capped one below the refresh (59 fps at 60 Hz).
     * refreshHz is the refresh limitRefreshHz judges by. From Moonlight Android's Game.onCreate (GPL-3.0,
     * Cameron Gutman and contributors).
     */
    public static int limitedFrameRate(int fps, float refreshHz) {
        int screen = Math.round(refreshHz);
        if (fps < screen) return fps;
        if (fps > screen + 3) return LIMIT_ABOVE_SCREEN;
        if (screen <= 49) return LIMIT_BOGUS_SCREEN;
        return screen - 1;
    }

    static int negotiatedFrameRateMilliHz(int requestedMilliHz, int negotiatedFps) {
        return requestedMilliHz >= 1000 && requestedMilliHz <= 1_000_000 &&
                (requestedMilliHz + 500) / 1000 == negotiatedFps ? requestedMilliHz : negotiatedFps * 1000;
    }

    /** Whether the present scheduler places frames: host frame times in latency mode only. */
    static boolean schedulesFrames(boolean hostFrameTimes, int framePacing) {
        return hostFrameTimes && framePacing != PreferenceConfiguration.FRAME_PACING_BALANCED &&
                framePacing != PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS &&
                framePacing != PreferenceConfiguration.FRAME_PACING_CAP_FPS;
    }

    /** Whether decoder debt is recovered (DecoderQueuePolicy.Debt): Smoothest and the frame rate limit. */
    static boolean recoversDecoderDebt(boolean hostFrameTimes, int framePacing) {
        return !schedulesFrames(hostFrameTimes, framePacing) && !dropQueuedFrames(framePacing);
    }

    static boolean shouldDrainOutput(int mode, long enqueueNs, long nowNs, int fpsMilliHz, int drained) {
        if (drained >= MAX_OUTPUT_DRAIN) return false;
        if (dropQueuedFrames(mode)) return true;
        // Keep one frame of jitter buffering, then catch up only if a newer decoded output is
        // available. A 50 ms floor left 30–40 ms of persistent decoder debt untouched at 60 FPS.
        // The low-rate ceiling stays finite. Never use unknown/future timestamps or discard
        // compressed reference frames; a stale last output still gets displayed.
        long maximumAgeNs = Math.min(250_000_000L, frameIntervalNs(fpsMilliHz));
        return enqueueNs > 0 && nowNs > enqueueNs && nowNs - enqueueNs > maximumAgeNs;
    }

    /** How far a refresh may be from a whole multiple of the stream's rate, per multiple. */
    static final double RATE_TOLERANCE_HZ = 0.5;
    /** The most refreshes fractional V-Sync holds a frame for. */
    static final int MAX_HELD_REFRESHES = 4;

    /**
     * Fractional V-Sync: the refreshes each frame stays on a display that refreshes at a whole
     * multiple of the stream's rate (2 for 60 fps on 120 Hz), else 1. VRR and the setting off take
     * 1 too; Smoothest video and Balanced with FPS limit pass fractional true whatever the setting
     * (MediaCodecDecoderRenderer.smoothDisplayNs). Java port of nightjar_policy::vsync::refreshes_per_frame
     * (client/protocol/policy).
     */
    public static int refreshesPerFrame(double refreshHz, double fps, boolean vrr, boolean fractional) {
        if (!fractional || vrr || !(refreshHz > 0) || !(fps > 0)) return 1;
        for (int k = 1; k <= MAX_HELD_REFRESHES; k++) {
            if (Math.abs(refreshHz - fps * k) <= RATE_TOLERANCE_HZ * k) return k;
        }
        return 1;
    }

    /**
     * The time to stamp a frame held for {@code refreshes} refreshes with (fractional V-Sync): the
     * refresh {@code refreshes} after the last frame's ({@code lastTargetNs}), or the first one it
     * can still make ({@code earliestNs}, the next refresh past the compositor's deadline) when it
     * comes later than that, which the next frames then count from. The compositor shows a frame
     * at the refresh its time asks for, so a frame that comes early waits for its turn instead of
     * cutting the last one's short. The turn is put on the refresh grid of {@code vsyncNs}, so a
     * rounded interval never drifts off it.
     *
     * A frame waits at most {@code refreshes - 1} refreshes past the first it can make, as long as a
     * frame that comes right after the last one was shown would: frames a burst brings a few ms
     * apart would otherwise each take the next turn, and the compositor, which never drops a frame
     * stamped for a later refresh, would keep that backlog (3 frames at 60 fps on 120 Hz: about 33 ms
     * more delay) until {@link #earlyForTurn} took it back, a refresh every two seconds. A later frame
     * of the burst takes the same refresh and replaces the earlier one there.
     */
    static long heldTargetNs(long earliestNs, long lastTargetNs, long vsyncNs, long intervalNs, int refreshes) {
        return heldTargetNs(earliestNs, lastTargetNs, vsyncNs, intervalNs, refreshes, false);
    }

    /**
     * As above; with {@code everyFrame} (Smoothest video and Balanced with FPS limit, which keep every
     * frame they can) a frame waits up to a whole stream frame past the first refresh it can make, a
     * refresh longer: jitter up to a frame keeps every frame on its own turn, and the delay stays near a
     * frame. A deeper backlog, as a burst brings, puts a frame on the refresh of the one before it, where
     * the compositor shows the newer; the desktop likewise drops a picture more than a frame old once a
     * newer one is due (present::Pacer::take_due). Unbounded, a burst's delay stayed until the turns
     * moved back a refresh every two seconds, as outputs released for later refreshes no longer age where
     * the catch-up of {@link #shouldDrainOutput} and DecoderQueuePolicy.Debt look.
     */
    static long heldTargetNs(long earliestNs, long lastTargetNs, long vsyncNs, long intervalNs, int refreshes,
                             boolean everyFrame) {
        if (refreshes <= 1 || intervalNs <= 0 || lastTargetNs == 0) return earliestNs;
        long turnNs = lastTargetNs + refreshes * intervalNs;
        long offsetNs = turnNs - vsyncNs;
        turnNs = vsyncNs + Math.floorDiv(offsetNs + intervalNs / 2, intervalNs) * intervalNs;
        long latestNs = earliestNs + (everyFrame ? refreshes : refreshes - 1) * intervalNs;
        return Math.max(earliestNs, Math.min(turnNs, latestNs));
    }

    /**
     * Fractional V-Sync: frames released {@link #EARLY_SPARE_NS} ahead of the deadline of the
     * refresh before their turn for {@link #EARLY_FOR_NS} in a row move the turns a refresh earlier,
     * once. A source whose clock runs ahead of the display brings them earlier and earlier, each
     * waiting longer, up to the whole hold, until a host's phase lock moves them later, and a host
     * without one never does. Longer than the lock takes (a report or two), so the two never take
     * turns. The desktop's phase::HeldAnchor::hold does the same.
     */
    static final long EARLY_FOR_NS = 2_000_000_000L;
    /** The longest period a phase report carries (24 Hz), as the protocol's Report decodes it. */
    static final long MAX_REPORT_PERIOD_NS = 41_700_000L;
    static final long EARLY_SPARE_NS = 3_000_000L;

    /**
     * Whether a frame released at {@code releaseNs} makes the refresh before its turn
     * ({@code turnNs}), whose compositor deadline comes {@code deadlineNs} ahead of it, with
     * {@link #EARLY_SPARE_NS} to spare.
     */
    static boolean earlyForTurn(long releaseNs, long turnNs, long intervalNs, long deadlineNs) {
        return intervalNs > 0 && turnNs - intervalNs - deadlineNs - releaseNs >= EARLY_SPARE_NS;
    }

    /**
     * The refresh grid a phase report measures slack to with fractional V-Sync: a refresh of the
     * grid ({@code anchorNs}) put {@code periods} grid periods into the past, so a report never
     * counts from a refresh still to come, and the grid's period; the display's own refresh and
     * interval without a hold, or with a hold longer than a report's period may be
     * ({@link #MAX_REPORT_PERIOD_NS}: 20 fps held 3 refreshes of 60 Hz), which the host would drop.
     * Returns {anchor, period}. The desktop's nightjar_policy::vsync::report_refreshes is the same rule.
     */
    static long[] heldReportGrid(long vsyncNs, long intervalNs, long anchorNs, int refreshes) {
        long[] out = new long[2];
        heldReportGrid(vsyncNs, intervalNs, anchorNs, refreshes, out);
        return out;
    }

    /** Zero-allocation {@link #heldReportGrid(long, long, long, int)} writing {anchor, period} into {@code out}. */
    static void heldReportGrid(long vsyncNs, long intervalNs, long anchorNs, int refreshes, long[] out) {
        if (refreshes <= 1 || anchorNs == 0 || intervalNs <= 0 || refreshes * intervalNs > MAX_REPORT_PERIOD_NS) {
            out[0] = vsyncNs;
            out[1] = intervalNs;
            return;
        }
        long periodNs = intervalNs * refreshes;
        out[0] = anchorNs - 4 * periodNs;
        out[1] = periodNs;
    }

    static boolean hasOutputDrainTimeRemaining(long elapsedNs, int fpsMilliHz) {
        // A slow discard can otherwise keep chasing newly decoded outputs without ever
        // presenting. One codec call can exceed this budget; after it returns, hand off the
        // output we already own instead of beginning another discard. Retain the count cap too.
        return elapsedNs >= 0 && elapsedNs < frameIntervalNs(fpsMilliHz);
    }
}
