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
 *   <li>or nothing, when its own refresh is more than one refresh behind and the display already
 *   waits for the refresh after the next: a surplus output of a burst is dropped rather than held,
 *   so no frame waits more than one refresh past the first it could make, as the catch-up drain
 *   drops older outputs today.</li>
 * </ul>
 *
 * The stamp is the refresh less half a refresh, as fractional V-Sync stamps a held frame
 * (FramePacingPolicy.heldTargetNs), so the compositor rounds it to that refresh while the phase
 * estimate is right to within half a refresh.
 *
 * A stream slower than the panel keeps a refresh of its own and gains nothing: its next output
 * already comes after the previous refresh, so each takes the first refresh it can make, which is
 * where releasing it "now" would land. (Balanced at a rate more than 5% below the panel does not
 * come through here at all: FramePacingPolicy.balancedPacesOnVsync keeps the vsync callback.)
 */
final class RefreshSlots {
    /** One output's place on the grid. */
    static final class Slot {
        /** The refresh the output is shown at; 0 when there is no grid to plan on. */
        long vsyncNs;
        /** The time to release it with, the refresh less half a refresh; 0 without a grid. */
        long targetNs;
        /** The output is surplus: the display already waits for the two refreshes it could use. */
        boolean drop;
    }

    /** The last refresh claimed, 0 before the first; on the grid of {@link #intervalNs}. */
    private long lastVsyncNs;
    private long intervalNs;
    private long planned;
    private long dropped;

    /** Forgets the grid, for a new display mode or stream. */
    synchronized void reset() {
        lastVsyncNs = 0;
        intervalNs = 0;
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
        out.drop = false;
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
        long vsync = lastVsyncNs == 0 ? earliestNs : Math.max(earliestNs, lastVsyncNs + intervalNs);
        if (vsync - earliestNs >= 2 * intervalNs) {
            dropped++;
            out.drop = true;
            return;
        }
        lastVsyncNs = vsync;
        planned++;
        out.vsyncNs = vsync;
        out.targetNs = vsync - intervalNs / 2;
    }

    /** Outputs placed on the grid and surplus ones dropped, for the problem report's row. */
    synchronized long planned() {
        return planned;
    }

    synchronized long dropped() {
        return dropped;
    }
}
