package com.limelight.binding.video;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;

import java.util.ArrayDeque;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.BooleanSupplier;
import java.util.function.Consumer;
import java.util.function.IntSupplier;
import java.util.function.Supplier;

/**
 * Keeps a decoder without a free input buffer off the thread that reads the video socket. With
 * direct submit the engine submits a frame from that thread, and the wait for an input buffer
 * (up to the hang limit, see {@link InputBufferWait}) held every datagram in the socket meanwhile:
 * a vendor codec's hiccup became a stall of the whole stream, frames arriving late in bursts.
 *
 * <p>A unit is submitted in place when the decoder has a buffer for it within
 * {@link InputBufferWait#RECEIVE_WAIT_MS}, and only if nothing is waiting before it. Otherwise
 * it is copied and queued for a worker thread, which submits the queue in order with the wait
 * and the hang detection the in-place path has. A decoder that keeps up stays on the in-place
 * path, except for an H.264/HEVC key frame whose parameter sets need buffers of their own: the
 * worker takes that whole frame, and the units after it queue until the worker has drained.
 *
 * <p>What the decoder never gets stays off it up to a key frame, as when the engine itself is told
 * {@code DR_NEED_IDR}: a unit that does not fit in the bound drops the queued ones with it, and a
 * queued unit that fails drops the rest of the queue. Either way the units that follow are dropped
 * (they would reference frames the decoder never saw) until the first unit of a key frame other than
 * one that was itself dropped, and the engine is asked for one: at once on overflow, with the next
 * unit otherwise. The engine stops submitting the rest of a frame after a unit's {@code DR_NEED_IDR}
 * (the Rust engine's renderers.rs returns at the first parameter set that is not OK), so a key frame cut short at the hand-off normally loses its remaining units
 * there; when a later unit of it arrives anyway (a failure on the worker has no status for the engine
 * yet) it is dropped here, as the picture alone would have no parameter sets. A queued unit's status
 * cannot reach the engine in its own call, so a key frame that was queued is counted as submitted
 * before the decoder has it.
 */
final class SubmitHandoff<U> {
    /** Submits a queued unit on the worker thread; returns the engine's DR_* status. */
    interface Worker<U> {
        int submit(U unit);
    }

    private static final class Entry<U> {
        final U unit;
        final int frames;
        final boolean keyUnit;
        final int frameNumber;

        Entry(U unit, int frames, boolean keyUnit, int frameNumber) {
            this.unit = unit;
            this.frames = frames;
            this.keyUnit = keyUnit;
            this.frameNumber = frameNumber;
        }
    }

    /** Return codes of {@link #tryGateInPlace} (disjoint from MoonBridge DR_* <= 0). */
    static final int GATE_IN_PLACE = 1;
    static final int GATE_QUEUE = 2;

    /** 3-slot reusable byte[] pool for units copied to the input worker outside the handoff lock. */
    static final class BytePool {
        private static final int SLOTS = 3;
        private final byte[][] slots = new byte[SLOTS][];
        private int count;

        synchronized byte[] acquire(int minLength) {
            int best = -1;
            int smallest = -1;
            for (int i = 0; i < count; i++) {
                int len = slots[i].length;
                if (len >= minLength && (best < 0 || len < slots[best].length)) best = i;
                if (smallest < 0 || len < slots[smallest].length) smallest = i;
            }
            if (best >= 0) {
                byte[] buf = slots[best];
                slots[best] = slots[--count];
                slots[count] = null;
                return buf;
            }
            if (count == SLOTS && smallest >= 0) {
                slots[smallest] = slots[--count];
                slots[count] = null;
            }
            return new byte[minLength];
        }

        synchronized void release(byte[] buf) {
            if (buf == null || buf.length == 0) return;
            if (count < SLOTS) {
                slots[count++] = buf;
                return;
            }
            int smallest = 0;
            for (int i = 1; i < SLOTS; i++) {
                if (slots[i].length < slots[smallest].length) smallest = i;
            }
            if (buf.length > slots[smallest].length) slots[smallest] = buf;
        }
    }

    private final Object lock = new Object();
    private final ArrayDeque<Entry<U>> queue = new ArrayDeque<>();
    private final Worker<U> worker;
    private final Consumer<U> recycler;
    private final int bound;
    private final String threadName;
    private final Runnable threadInit;
    private final AtomicLong drops = new AtomicLong();
    // Units the worker took from the queue (the others were submitted in place on the receive thread).
    private final AtomicLong handed = new AtomicLong();
    // Frames the engine dropped while the decoder waited for a key frame: not network loss.
    private final AtomicBoolean keyGap = new AtomicBoolean();
    // True iff !closed && !awaitingKey && !working && queue.isEmpty(): lets the hot in-place path skip lock.
    private volatile boolean fastBypass = true;
    // All below under lock.
    private Thread thread;
    private boolean working;
    private boolean closed;
    // Units are dropped until the first of a key frame.
    private boolean awaitingKey;
    // The engine has yet to be told to ask for it.
    private boolean reportIdr;
    // The last key frame (number) that lost a unit here, else -1: the rest of it is dropped too.
    private int lostKeyFrame = -1;

    /**
     * At most {@code bound} units wait for the worker, which is {@code worker}'s thread;
     * {@code threadInit} runs on it first (its priority).
     */
    SubmitHandoff(int bound, String threadName, Runnable threadInit, Worker<U> worker) {
        this(bound, threadName, threadInit, worker, null);
    }

    SubmitHandoff(int bound, String threadName, Runnable threadInit, Worker<U> worker, Consumer<U> recycler) {
        this.bound = Math.max(1, bound);
        this.threadName = threadName;
        this.threadInit = threadInit;
        this.worker = worker;
        this.recycler = recycler;
    }

    private void updateFastBypass() {
        fastBypass = !closed && !awaitingKey && !working && queue.isEmpty();
    }

    private void recycle(U unit) {
        if (recycler != null && unit != null) recycler.accept(unit);
    }

    /**
     * Zero-allocation fast gate on the receive thread: returns {@link #GATE_IN_PLACE} when the
     * caller may try in-place submission, {@link #GATE_QUEUE} when it must copy outside the lock
     * and call {@link #enqueue}, or a MoonBridge DR_* status (<= 0) when dropped.
     */
    int tryGateInPlace(int frames, boolean keyUnit, int frameNumber) {
        if (fastBypass) return GATE_IN_PLACE;
        synchronized (lock) {
            Integer refused = gate(frames, keyUnit, frameNumber);
            if (refused != null) return refused;
            return (queue.isEmpty() && !working) ? GATE_IN_PLACE : GATE_QUEUE;
        }
    }

    /**
     * Submits one unit from the receive thread, which must be the only caller.
     *
     * @param inputReady waits at most the receive bound for the decoder buffers the unit needs; true when it has them
     * @param inPlace submits the unit now, on this thread, and returns its status
     * @param copy the unit as the worker thread takes it: the caller's buffers are reused after return
     * @param frames how many frames the unit counts as when it is dropped (0 for a parameter set)
     * @param keyUnit whether the unit belongs to a key frame
     * @param frameNumber the unit's frame, which its other units share
     * @return the status to give the engine
     */
    int submit(BooleanSupplier inputReady, IntSupplier inPlace, Supplier<U> copy, int frames, boolean keyUnit,
               int frameNumber) {
        int gateResult = tryGateInPlace(frames, keyUnit, frameNumber);
        if (gateResult <= 0) return gateResult;
        if (gateResult == GATE_IN_PLACE && inputReady.getAsBoolean()) return inPlace.getAsInt();
        return enqueue(copy.get(), frames, keyUnit, frameNumber);
    }

    /** Under lock: the status for a unit dropped while the decoder waits for a key frame, else null. */
    private Integer gate(int frames, boolean keyUnit, int frameNumber) {
        if (closed) return MoonBridge.DR_OK;
        if (!awaitingKey) return null;
        if (keyUnit && frameNumber != lostKeyFrame) {
            awaitingKey = false;
            reportIdr = false;
            lostKeyFrame = -1;
            updateFastBypass();
            return null;
        }
        drops.addAndGet(frames);
        boolean report = reportIdr;
        reportIdr = false;
        return report ? MoonBridge.DR_NEED_IDR : MoonBridge.DR_OK;
    }

    /**
     * Under lock: drops what waits, counting its frames (and {@code frames} more), and waits for a key
     * frame other than the ones that lost a unit: the unit that failed ({@code failedKeyFrame}, or -1)
     * and those that waited.
     */
    private void dropQueued(long frames, int failedKeyFrame) {
        if (failedKeyFrame >= 0) lostKeyFrame = failedKeyFrame;
        for (Entry<U> waiting : queue) {
            frames += waiting.frames;
            if (waiting.keyUnit) lostKeyFrame = waiting.frameNumber;
            recycle(waiting.unit);
        }
        queue.clear();
        drops.addAndGet(frames);
        awaitingKey = true;
        updateFastBypass();
        keyGap.set(true);
    }

    int enqueue(U unit, int frames, boolean keyUnit, int frameNumber) {
        synchronized (lock) {
            // The worker may have failed since the caller looked.
            Integer refused = gate(frames, keyUnit, frameNumber);
            if (refused != null) {
                recycle(unit);
                return refused;
            }
            if (unit == null || queue.size() >= bound) {
                // No unit to queue (the caller could not copy it), or the decoder fell behind by the
                // whole bound: drop what waits, as the engines' decode unit queue does, and ask for a
                // key frame.
                recycle(unit);
                dropQueued(frames, -1);
                if (keyUnit) lostKeyFrame = frameNumber;
                reportIdr = false;
                return MoonBridge.DR_NEED_IDR;
            }
            queue.add(new Entry<>(unit, frames, keyUnit, frameNumber));
            updateFastBypass();
            if (thread == null || !thread.isAlive()) {
                thread = new Thread(this::run, threadName);
                thread.setDaemon(true);
                thread.start();
            }
            lock.notifyAll();
            return MoonBridge.DR_OK;
        }
    }

    private void run() {
        threadInit.run();
        while (true) {
            Entry<U> entry;
            synchronized (lock) {
                while (queue.isEmpty() && !closed) {
                    try {
                        lock.wait();
                    } catch (InterruptedException e) {
                        return;
                    }
                }
                if (closed) return;
                entry = queue.poll();
                working = true;
                updateFastBypass();
            }
            int status = MoonBridge.DR_NEED_IDR;
            try {
                handed.incrementAndGet();
                status = worker.submit(entry.unit);
            } catch (Throwable t) {
                // The unit failed like any other: the thread stays, or the stream would freeze with
                // units queueing behind a worker that is gone.
                LimeLog.severe("Input worker: submitting frame " + entry.frameNumber + " threw " + t);
            } finally {
                recycle(entry.unit);
                synchronized (lock) {
                    // A failure and its flags are one step: the receive thread never sees an idle
                    // worker that has yet to say the decoder lost a frame.
                    if (status != MoonBridge.DR_OK) {
                        dropQueued(0, entry.keyUnit ? entry.frameNumber : -1);
                        reportIdr = true;
                    }
                    working = false;
                    updateFastBypass();
                }
            }
        }
    }

    /**
     * Frames dropped here for the decoder: refused by a full queue, or queued or arriving behind a
     * failure. Not the unit that failed, which the decoder's path counted, and not what the engine
     * dropped itself (see {@link #takeKeyGap}).
     */
    long drops() {
        return drops.get();
    }

    /** Units handed to the worker so far. */
    long handedUnits() {
        return handed.get();
    }

    /**
     * Whether frames were dropped here since the last call and not yet answered by a key frame
     * unit's own gap: the gap before that key frame is the decoder's, not the network's.
     */
    boolean takeKeyGap() {
        return keyGap.getAndSet(false);
    }

    /** For tests: whether frames were dropped and not taken (without taking). */
    boolean takeKeyGapPeek() {
        return keyGap.get();
    }

    /** Whether a unit waits or is being submitted. */
    boolean busy() {
        synchronized (lock) {
            return !queue.isEmpty() || working;
        }
    }

    /** Whether the calling thread is the worker. */
    boolean onWorkerThread() {
        synchronized (lock) {
            return thread == Thread.currentThread();
        }
    }

    /** Whether {@link #close} has run: a closed hand-off passes every unit, so a new start needs a new one. */
    boolean isClosed() {
        synchronized (lock) {
            return closed;
        }
    }

    /**
     * Drops what waits and stops the worker once its unit in hand is done; waits for it at most
     * {@code joinMs} (0: not at all). Whether the worker has ended.
     */
    boolean close(long joinMs) {
        Thread joined;
        boolean first;
        synchronized (lock) {
            first = !closed;
            closed = true;
            for (Entry<U> waiting : queue) recycle(waiting.unit);
            queue.clear();
            updateFastBypass();
            joined = thread;
            lock.notifyAll();
        }
        if (first) {
            // Once per stream: whether the worker was used at all.
            LimeLog.info("Input hand-off: " + handed.get() + " units handed to the " + threadName + " worker, " +
                    drops.get() + " frames dropped there");
        }
        if (joined == null) return true;
        if (joinMs > 0 && joined != Thread.currentThread()) {
            try {
                joined.join(joinMs);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }
        return !joined.isAlive();
    }
}
