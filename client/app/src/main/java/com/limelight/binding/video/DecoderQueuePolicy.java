package com.limelight.binding.video;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

/**
 * Decoder queue depth, buffer limits, residency governor, display gate, debt recovery, and
 * in-flight compositor tracking ({@code nightjar_policy::video_queue}).
 */
final class DecoderQueuePolicy {
    /**
     * Frames the decoder holds: submitted less returned since the last flush, whose frames are gone
     * (base is the difference at that moment). Never negative.
     */
    static int depth(int framesIn, int framesOut, int base) {
        return NightjarPolicyJni.decoderQueueDepth(framesIn, framesOut, base);
    }

    static int outputLimit(String decoderName, boolean advertisedLowLatency, boolean dropLateFrames,
                           boolean recovered) {
        return NightjarPolicyJni.decoderOutputLimit(decoderName, advertisedLowLatency, dropLateFrames, recovered);
    }

    /**
     * Whether [Debt] watches the display's queue on this decoder. Every capture behind those rules
     * comes from a Shield TV (Nightjar issue 7), so they stay on its decoder family.
     */
    static boolean watchesDisplayQueue(String decoderName) {
        return NightjarPolicyJni.decoderWatchesDisplayQueue(decoderName);
    }

    /**
     * Residency governor (`nightjar_policy::video_queue::Residency`).
     */
    static final class Residency implements AutoCloseable {
        static final int FRAMES = 30;
        static final int PERIOD = 4;
        static final int WINDOWS = 10;
        static final long WINDOW_NS = 1_000_000_000L;

        private final long handle = NightjarPolicyJni.queueSlabCreate(NightjarPolicyJni.QUEUE_SLAB_RESIDENCY);
        private final HandleReaper.Token reaper = HandleReaper.register(
                this,
                handle,
                h -> NightjarPolicyJni.queueSlabDestroy(NightjarPolicyJni.QUEUE_SLAB_RESIDENCY, h));

        long handle() {
            return handle;
        }

        @Override
        public void close() {
            reaper.close();
        }

        long onOutput(long inputToOutputNs, long nowNs) {
            return NightjarPolicyJni.residencyOp(0, handle, inputToOutputNs, nowNs);
        }

        void track(long excessNs, long contentIntervalNs) {
            NightjarPolicyJni.residencyOp(1, handle, excessNs, contentIntervalNs);
        }

        boolean shouldDrop(int framesBehind) {
            return NightjarPolicyJni.residencyOp(2, handle, framesBehind, 0) != 0;
        }

        void onDropped() {
            NightjarPolicyJni.residencyOp(3, handle, 0, 0);
        }

        void reset() {
            NightjarPolicyJni.residencyOp(4, handle, 0, 0);
        }
    }

    /**
     * Whether the display, not the input, paces the decoder's outputs
     * (`nightjar_policy::video_queue::DisplayGate`).
     */
    static final class DisplayGate implements AutoCloseable {
        static final int BUCKETS = 10;
        static final long BUCKET_NS = 1_000_000_000L;
        static final double MAX_SLOPE = 0.3;
        static final int OVER_PERCENT = 80;
        static final int MAX_BEHIND_PERCENT = 10;
        static final long MIN_SPREAD_NS = 100_000;

        private final long handle = NightjarPolicyJni.queueSlabCreate(NightjarPolicyJni.QUEUE_SLAB_DISPLAY_GATE);
        private final HandleReaper.Token reaper = HandleReaper.register(
                this,
                handle,
                h -> NightjarPolicyJni.queueSlabDestroy(NightjarPolicyJni.QUEUE_SLAB_DISPLAY_GATE, h));

        @Override
        public void close() {
            reaper.close();
        }

        void onOutput(long inputToOutputNs, long outputNs, long contentIntervalNs, boolean over, boolean behind) {
            NightjarPolicyJni.displayGateOp(0, handle, inputToOutputNs, outputNs, contentIntervalNs, over, behind);
        }

        boolean held() {
            return NightjarPolicyJni.displayGateOp(1, handle, 0, 0, 0, false, false) != 0;
        }

        boolean stillHeld(int minBuckets, double maxSlope) {
            return NightjarPolicyJni.displayGateStillHeld(handle, minBuckets, maxSlope);
        }

        void clear() {
            NightjarPolicyJni.displayGateOp(2, handle, 0, 0, 0, false, false);
        }

        void reset() {
            NightjarPolicyJni.displayGateOp(3, handle, 0, 0, 0, false, false);
        }
    }

    /**
     * Low percentile of wait times over the last minute (`nightjar_policy::video_queue::RecentLevel`).
     */
    static final class RecentLevel implements AutoCloseable {
        static final int SECONDS = 60;
        static final long SECOND_NS = 1_000_000_000L;

        private final long handle = NightjarPolicyJni.queueSlabCreate(NightjarPolicyJni.QUEUE_SLAB_RECENT_LEVEL);
        private final HandleReaper.Token reaper = HandleReaper.register(
                this,
                handle,
                h -> NightjarPolicyJni.queueSlabDestroy(NightjarPolicyJni.QUEUE_SLAB_RECENT_LEVEL, h));

        @Override
        public void close() {
            reaper.close();
        }

        void add(long valueNs, long nowNs) {
            NightjarPolicyJni.recentLevelOp(0, handle, valueNs, nowNs);
        }

        long lowNs() {
            return NightjarPolicyJni.recentLevelOp(1, handle, 0, 0);
        }

        long lastNs() {
            return NightjarPolicyJni.recentLevelOp(2, handle, 0, 0);
        }

        void reset() {
            NightjarPolicyJni.recentLevelOp(3, handle, 0, 0);
        }
    }

    /**
     * Decoder debt recovery in Smoothest and the frame rate limit (`nightjar_policy::video_queue::Debt`).
     */
    static final class Debt implements AutoCloseable {
        static final long WINDOW_NS = 1_500_000_000L;
        static final int SHARE_PERCENT = 80;
        static final int MIN_OUTPUTS = 20;
        static final int SETTLE = 3;
        static final int MAX_DROPS = 8;
        static final long WAIT_US = 2_000;
        static final int MAX_WAITS = 60;
        static final int MAX_LEARNED_FRAMES = 3;
        static final long GATE_EXCESS_NS = 1_000_000;
        static final long GATE_CHECK_NS = 5_000_000_000L;
        static final long GATE_BACKOFF_NS = 600_000_000_000L;
        static final int MAX_GATE_DROPS = 24;

        private final long handle = NightjarPolicyJni.queueSlabCreate(NightjarPolicyJni.QUEUE_SLAB_DEBT);
        private final HandleReaper.Token reaper = HandleReaper.register(
                this,
                handle,
                h -> NightjarPolicyJni.queueSlabDestroy(NightjarPolicyJni.QUEUE_SLAB_DEBT, h));

        @Override
        public void close() {
            reaper.close();
        }

        void watchDisplay(boolean watch) {
            NightjarPolicyJni.debtOp(0, handle, 0, 0, 0, 0, 0, watch);
        }

        boolean watchesDisplay() {
            return NightjarPolicyJni.debtOp(1, handle, 0, 0, 0, 0, 0, false) != 0;
        }

        long onOutput(long inputToOutputNs, long nowNs, long contentIntervalNs) {
            return onOutput(inputToOutputNs, 0, nowNs, contentIntervalNs, 0);
        }

        long onOutput(long inputToOutputNs, long submitToOutputNs, long nowNs, long contentIntervalNs, int framesBehind) {
            return NightjarPolicyJni.debtOp(2, handle, inputToOutputNs, submitToOutputNs, nowNs, contentIntervalNs,
                    framesBehind, false);
        }

        boolean shouldDrop() {
            return NightjarPolicyJni.debtOp(3, handle, 0, 0, 0, 0, 0, false) != 0;
        }

        void onDropped() {
            NightjarPolicyJni.debtOp(4, handle, 0, 0, 0, 0, 0, false);
        }

        void onNoNewer() {
            NightjarPolicyJni.debtOp(5, handle, 0, 0, 0, 0, 0, false);
        }

        String takeEvent() {
            return NightjarPolicyJni.debtTakeEvent(handle);
        }

        void reset() {
            NightjarPolicyJni.debtOp(6, handle, 0, 0, 0, 0, 0, false);
        }
    }

    /**
     * Frames released to the compositor and not yet shown (`nightjar_policy::video_queue::InFlight`).
     */
    static final class InFlight implements AutoCloseable {
        static final int MAX = 2;

        private final long handle = NightjarPolicyJni.queueSlabCreate(NightjarPolicyJni.QUEUE_SLAB_IN_FLIGHT);
        private final HandleReaper.Token reaper = HandleReaper.register(
                this,
                handle,
                h -> NightjarPolicyJni.queueSlabDestroy(NightjarPolicyJni.QUEUE_SLAB_IN_FLIGHT, h));

        @Override
        public void close() {
            reaper.close();
        }

        synchronized void onReleased(long codecPtsUs, long dueAtNs) {
            NightjarPolicyJni.inFlightOp(0, handle, codecPtsUs, dueAtNs);
        }

        synchronized void onRendered(long codecPtsUs) {
            NightjarPolicyJni.inFlightOp(1, handle, codecPtsUs, 0);
        }

        synchronized int count(long nowNs) {
            return (int) NightjarPolicyJni.inFlightOp(2, handle, nowNs, 0);
        }

        synchronized boolean full(long nowNs) {
            return NightjarPolicyJni.inFlightOp(3, handle, nowNs, 0) != 0;
        }

        synchronized void reset() {
            NightjarPolicyJni.inFlightOp(4, handle, 0, 0);
        }
    }
}
