package com.limelight.binding.video;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

/**
 * Lowest latency and Balanced: a buffer too many in the display's queue (nightjar#7), backed by
 * {@code nightjar_policy::video_queue::DisplayQueueWatch}.
 */
final class DisplayQueueWatch implements AutoCloseable {
    static final long SECOND_NS = 1_000_000_000L;
    static final int MIN_FRAMES = 10;
    static final int LEVEL_SECONDS = 3;
    static final int SUSTAIN_SECONDS = 3;
    static final long MARGIN_NS = 1_000_000;
    static final int STEADY_SECONDS = 5;
    static final double PROBE_REFRESHES = 2.5;
    static final long MIN_GAP_NS = 10 * SECOND_NS;
    static final int MAX_DROPS = 12;
    static final int MAX_MISSES = 2;
    static final long MAX_WAIT_NS = 500_000_000L;
    static final long REPLACED_LOG_NS = 10 * SECOND_NS;

    private final long handle = NightjarPolicyJni.queueSlabCreate(NightjarPolicyJni.QUEUE_SLAB_DISPLAY_WATCH);
    private final HandleReaper.Token reaper = HandleReaper.register(
            this,
            handle,
            h -> NightjarPolicyJni.queueSlabDestroy(NightjarPolicyJni.QUEUE_SLAB_DISPLAY_WATCH, h));

    @Override
    public void close() {
        reaper.close();
    }

    static boolean applies(boolean settingOn, boolean latencyOrBalanced, boolean scheduled, boolean renderCallbacks,
                           boolean vrr, int streamFrameRateMilliHz, long refreshNs) {
        return NightjarPolicyJni.displayQueueApplies(settingOn, latencyOrBalanced, scheduled, renderCallbacks, vrr,
                streamFrameRateMilliHz, refreshNs);
    }

    /** Called with each frame's state; a change of either starts over. */
    synchronized void configure(boolean enabled, long refreshNs) {
        NightjarPolicyJni.displayQueueOp(0, handle, refreshNs, 0, enabled);
    }

    /**
     * One frame the display showed: {@code waitNs} from its hand-off to its render time
     * {@code renderNs} (the platform's, so callback batching does not matter).
     */
    synchronized void onRendered(long waitNs, long renderNs) {
        NightjarPolicyJni.displayQueueOp(1, handle, waitNs, renderNs, false);
    }

    /** Whether the frame about to be shown is dropped instead; true once per decision. */
    synchronized boolean takeDrop(long nowNs) {
        return NightjarPolicyJni.displayQueueOp(2, handle, nowNs, 0, false) != 0;
    }

    /** The line to log about the last decision, once, or null. */
    synchronized String takeEvent() {
        return NightjarPolicyJni.displayQueueTakeEvent(handle);
    }
}
