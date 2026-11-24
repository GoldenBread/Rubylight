package com.limelight.binding.video;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

/**
 * Deadline present scheduling for a display with a fixed refresh rate, on top of
 * [HostFrameClock] and backed by {@code nightjar_policy::video_queue::PresentSchedulerState}.
 */
final class PresentScheduler implements AutoCloseable {
    /** A frame is due this fraction of a vsync before its vsync. */
    static final int LEAD_DIVISOR = 4;
    static final int WINDOW = 128;
    static final int PERCENTILE = 97;
    static final long MARGIN_US = 500;
    /** No frame counts as late before this many frames and this much host time (the clock's warm-up). */
    static final int MIN_SAMPLES = 32;
    static final long WARMUP_US = 250_000;

    /** One planned frame. */
    static final class Plan {
        /** System.nanoTime() the buffer is due at (the timestamp for releaseOutputBuffer). */
        long releaseNs;
        /** The vsync the frame is meant for. */
        long vsyncNs;
        /** Time from the frame being ready to its release time. */
        long holdNs;
        /** The frame shares its vsync with the one before it. */
        boolean sharesVsync;
    }

    private final HostFrameClock clock;
    private final HostFrameClock.GridPlan grid = new HostFrameClock.GridPlan();
    private long lastLookupPtsUs = Long.MIN_VALUE;
    private long lastLookupHostUs = -1;
    private final long handle = NightjarPolicyJni.queueSlabCreate(NightjarPolicyJni.QUEUE_SLAB_PRESENT_SCHEDULER);
    private final HandleReaper.Token reaper = HandleReaper.register(
            this,
            handle,
            h -> NightjarPolicyJni.queueSlabDestroy(NightjarPolicyJni.QUEUE_SLAB_PRESENT_SCHEDULER, h));

    PresentScheduler(HostFrameClock clock) {
        this.clock = clock;
    }

    @Override
    public void close() {
        reaper.close();
    }

    /** Forget the lateness window, for example after a decoder flush; counters stay. */
    synchronized void reset() {
        lastLookupPtsUs = Long.MIN_VALUE;
        lastLookupHostUs = -1;
        NightjarPolicyJni.presentSchedulerOp(0, handle, false);
    }

    /**
     * Notes a decoded frame, ready at readyNs, in the lateness window and returns whether it is
     * behind the game's cadence by more than half of contentIntervalNs.
     */
    synchronized boolean isLate(long codecPtsUs, long readyNs, long contentIntervalNs) {
        long host = clock.hostUsFor(codecPtsUs);
        if (host < 0) {
            lastLookupPtsUs = Long.MIN_VALUE;
            lastLookupHostUs = -1;
            return false;
        }
        lastLookupPtsUs = codecPtsUs;
        lastLookupHostUs = host;
        return NightjarPolicyJni.presentSchedulerIsLate(handle, host, readyNs, contentIntervalNs);
    }

    /**
     * Plans the frame onto the grid of a panel whose last known vsync was at vsyncNs, with vsyncs
     * intervalNs apart and a presentation deadline of deadlineNs.
     */
    boolean plan(long codecPtsUs, long readyNs, long vsyncNs, long intervalNs, long deadlineNs, Plan out) {
        if (vsyncNs <= 0 || intervalNs <= 0) return false;
        long readyForGridNs = readyNs + Math.max(0, deadlineNs);
        long host;
        if (codecPtsUs == lastLookupPtsUs) {
            host = lastLookupHostUs;
            lastLookupPtsUs = Long.MIN_VALUE;
        } else {
            host = clock.hostUsFor(codecPtsUs);
        }
        if (!clock.presentOnGridForHost(host, readyForGridNs, vsyncNs, intervalNs, intervalNs / LEAD_DIVISOR,
                intervalNs / 2, grid)) {
            return false;
        }
        out.releaseNs = grid.releaseNs;
        out.vsyncNs = grid.vsyncNs;
        out.holdNs = grid.releaseNs - readyNs;
        out.sharesVsync = grid.sharesVsync;
        NightjarPolicyJni.presentSchedulerOp(1, handle, grid.sharesVsync);
        return true;
    }

    /**
     * A frame dropped for a newer one still teaches the clock its lateness, without taking a
     * vsync. readyNs is on the same basis as the frames that are planned.
     */
    void discard(long codecPtsUs, long readyNs) {
        clock.presentNs(codecPtsUs, readyNs);
    }

    /** Whether a late frame is replaced by the newer output already decoded. Never twice in a row. */
    static boolean dropLate(boolean late, boolean newerAvailable, boolean previousDropped) {
        return NightjarPolicyJni.presentSchedulerDropLate(late, newerAvailable, previousDropped);
    }

    void onLateDropped() { NightjarPolicyJni.presentSchedulerOp(2, handle, false); }
    void onGovernorDropped() { NightjarPolicyJni.presentSchedulerOp(3, handle, false); }
    void onCapDropped() { NightjarPolicyJni.presentSchedulerOp(4, handle, false); }
    void onCapHeld() { NightjarPolicyJni.presentSchedulerOp(5, handle, false); }

    long lateDropped() { return NightjarPolicyJni.presentSchedulerOp(6, handle, false); }
    long governorDropped() { return NightjarPolicyJni.presentSchedulerOp(7, handle, false); }
    long capDropped() { return NightjarPolicyJni.presentSchedulerOp(8, handle, false); }
    long capHeld() { return NightjarPolicyJni.presentSchedulerOp(9, handle, false); }

    /** One overlay line, or null before the first planned frame. */
    String overlayLine() {
        return NightjarPolicyJni.presentSchedulerOverlayLine(handle);
    }
}
