package com.limelight.binding.video;

/**
 * Balanced with a stream about as fast as the panel, which releases each decoded output as it
 * comes out: the refresh of the display's vsync grid that the output is shown at.
 *
 * A frame released without a time is latched at whatever refresh follows the release. Two outputs
 * that come out inside one refresh then both ask for the same one and the compositor replaces the
 * earlier, so the screen keeps the frame before them for two refreshes: 8.9% of display intervals
 * at 120 fps on the Retroid's 120 Hz panel, with the renderer's own cadence even (arm a of the
 * 2026-11-16 pacing rig). A buffer stamped with the time of a refresh is shown at that refresh, so
 * this class gives each output a refresh of its own:
 * <ul>
 *   <li>the first refresh the output can still make, which is the one releasing it "now" would
 *   reach: the first vsync at least the display's presentation deadline after the release (Android
 *   shows a buffer queued that long before a vsync at it);</li>
 *   <li>or the refresh after the previous output's when both can make the same one: one frame per
 *   refresh;</li>
 *   <li>or the refresh already claimed, when its own is more than one refresh behind: the newest
 *   output of a burst is stamped with that refresh too, so the compositor replaces the older buffer
 *   there and the freshest frame is the one that shows.</li>
 * </ul>
 *
 * The stamp is the refresh less half a refresh, as fractional V-Sync stamps a held frame
 * (FramePacingPolicy.heldTargetNs), so the compositor rounds it to that refresh while the phase
 * estimate is right to within half a refresh.
 *
 * Only a stream within {@link #RATE_TOLERANCE_PERCENT}% of the panel's rate is planned: a faster
 * stream has surplus frames at every refresh, where a claimed refresh only queues them a refresh or
 * more behind. A slower stream is not planned either; Balanced below 95% of the panel keeps the
 * vsync callback (FramePacingPolicy.balancedPacesOnVsync) and never reaches this class.
 */
final class RefreshSlots {
    /** How far the stream's frame interval may be from the refresh interval and still be planned. */
    static final int RATE_TOLERANCE_PERCENT = 2;

    /** One output's place on the grid. */
    static final class Slot {
        /** The refresh the output is shown at; 0 when there is no grid to plan on. */
        long vsyncNs;
        /** The time to release it with, the refresh less half a refresh; 0 without a grid. */
        long targetNs;
        /** The output takes a refresh already claimed: the compositor replaces the buffer there. */
        boolean shared;
    }

    /** The last refresh claimed, 0 before the first; on the grid of {@link #intervalNs}. */
    private long lastVsyncNs;
    private long intervalNs;
    private long planned;
    private long shared;

    /** Forgets the grid, for a new display mode or stream. */
    synchronized void reset() {
        lastVsyncNs = 0;
        intervalNs = 0;
    }

    /**
     * Whether the stream runs at the panel's rate, so that one output per refresh keeps up:
     * frame and refresh intervals within {@link #RATE_TOLERANCE_PERCENT}%.
     */
    static boolean applies(long frameIntervalNs, long refreshIntervalNs) {
        if (frameIntervalNs <= 0 || refreshIntervalNs <= 0) return false;
        return Math.abs(frameIntervalNs - refreshIntervalNs) * 100 <= refreshIntervalNs * RATE_TOLERANCE_PERCENT;
    }

    /**
     * The first vsync a frame ready at readyNs can still make: the first that comes at least
     * leadNs after it, or the one after it when they fall together. Zero without a grid (no vsync
     * sample yet, or no refresh interval).
     */
    static long firstVsyncNs(long readyNs, long vsyncNs, long intervalNs, long leadNs) {
        if (vsyncNs <= 0 || intervalNs <= 0) return 0;
        long intervals = Math.max(1, (readyNs + Math.max(0, leadNs) - vsyncNs) / intervalNs + 1);
        return vsyncNs + intervals * intervalNs;
    }

    /**
     * Places an output released at releaseNs on the grid whose last vsync was at vsyncNs, with
     * vsyncs intervalNs apart and a presentation deadline (latch lead) of leadNs. Reentrant-safe
     * to call from one thread at a time; the reset from a display change may race it.
     */
    synchronized void plan(long releaseNs, long vsyncNs, long intervalNs, long leadNs, Slot out) {
        out.vsyncNs = 0;
        out.targetNs = 0;
        out.shared = false;
        if (vsyncNs <= 0 || intervalNs <= 0) {
            reset();
            return;
        }
        if (intervalNs != this.intervalNs) {
            // A new mode's refresh: the claimed refreshes belong to the old grid.
            lastVsyncNs = 0;
            this.intervalNs = intervalNs;
        }
        long earliestNs = firstVsyncNs(releaseNs, vsyncNs, intervalNs, leadNs);
        long vsync;
        if (lastVsyncNs == 0) {
            vsync = earliestNs;
        } else {
            long nextNs = lastVsyncNs + intervalNs;
            // Snap the carried claim back onto the live vsync grid so panel clock drift or
            // Choreographer jitter cannot ratchet lastVsyncNs off the display's refreshes.
            nextNs = vsyncNs + Math.floorDiv(nextNs - vsyncNs + intervalNs / 2, intervalNs) * intervalNs;
            vsync = Math.max(earliestNs, nextNs);
        }
        if (lastVsyncNs != 0 && vsync - earliestNs >= 2 * intervalNs) {
            // Two refreshes already claimed ahead of this output: stamp it with the refresh already
            // claimed (the previous output's), where the compositor replaces that older buffer with
            // this one, so the newest frame of the burst is what shows.
            shared++;
            out.vsyncNs = lastVsyncNs;
            out.targetNs = lastVsyncNs - intervalNs / 2;
            out.shared = true;
            return;
        }
        lastVsyncNs = vsync;
        planned++;
        out.vsyncNs = vsync;
        out.targetNs = vsync - intervalNs / 2;
    }

    /** Outputs given a refresh of their own and outputs sharing the previous one, for the report row. */
    synchronized long planned() {
        return planned;
    }

    synchronized long shared() {
        return shared;
    }
}
