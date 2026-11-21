package com.limelight.binding.video;

import com.limelight.nvstream.jni.MoonBridge;

import java.util.ArrayDeque;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.BooleanSupplier;
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
 * and the hang detection the in-place path has. A decoder that keeps up never leaves the in-place
 * path, so nothing changes for it.
 *
 * <p>What the decoder never gets stays off it up to a key frame, as when the engine itself is told
 * {@code DR_NEED_IDR}: a unit that does not fit in the bound drops the queued ones with it, and a
 * queued unit that fails drops the rest of the queue. Either way the units that follow are dropped
 * (they would reference frames the decoder never saw) until the first unit of a key frame, and the
 * engine is asked for one: at once on overflow, with the next unit otherwise. A queued unit's status
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

        Entry(U unit, int frames) {
            this.unit = unit;
            this.frames = frames;
        }
    }

    private final Object lock = new Object();
    private final ArrayDeque<Entry<U>> queue = new ArrayDeque<>();
    private final Worker<U> worker;
    private final int bound;
    private final String threadName;
    private final Runnable threadInit;
    private final AtomicLong drops = new AtomicLong();
    // Frames the engine dropped while the decoder waited for a key frame: not network loss.
    private final AtomicBoolean keyGap = new AtomicBoolean();
    // All below under lock.
    private Thread thread;
    private boolean working;
    private boolean closed;
    // Units are dropped until the first of a key frame.
    private boolean awaitingKey;
    // The engine has yet to be told to ask for it.
    private boolean reportIdr;

    /**
     * At most {@code bound} units wait for the worker, which is {@code worker}'s thread;
     * {@code threadInit} runs on it first (its priority).
     */
    SubmitHandoff(int bound, String threadName, Runnable threadInit, Worker<U> worker) {
        this.bound = Math.max(1, bound);
        this.threadName = threadName;
        this.threadInit = threadInit;
        this.worker = worker;
    }

    /**
     * Submits one unit from the receive thread, which must be the only caller.
     *
     * @param inputReady waits at most the receive bound for the decoder buffers the unit needs; true when it has them
     * @param inPlace submits the unit now, on this thread, and returns its status
     * @param copy the unit as the worker thread takes it: the caller's buffers are reused after return
     * @param frames how many frames the unit counts as when it is dropped (0 for a parameter set)
     * @param keyUnit whether the unit belongs to a key frame
     * @return the status to give the engine
     */
    int submit(BooleanSupplier inputReady, IntSupplier inPlace, Supplier<U> copy, int frames, boolean keyUnit) {
        synchronized (lock) {
            Integer refused = gate(frames, keyUnit);
            if (refused != null) return refused;
            if (!(queue.isEmpty() && !working)) return enqueue(copy.get(), frames, keyUnit);
        }
        if (inputReady.getAsBoolean()) return inPlace.getAsInt();
        return enqueue(copy.get(), frames, keyUnit);
    }

    /** Under lock: the status for a unit dropped while the decoder waits for a key frame, else null. */
    private Integer gate(int frames, boolean keyUnit) {
        if (closed) return MoonBridge.DR_OK;
        if (!awaitingKey) return null;
        if (keyUnit) {
            awaitingKey = false;
            reportIdr = false;
            return null;
        }
        drops.addAndGet(frames);
        boolean report = reportIdr;
        reportIdr = false;
        return report ? MoonBridge.DR_NEED_IDR : MoonBridge.DR_OK;
    }

    /** Under lock: drops what waits, counting its frames, and waits for a key frame. */
    private void dropQueued(long frames) {
        for (Entry<U> waiting : queue) frames += waiting.frames;
        queue.clear();
        drops.addAndGet(frames);
        awaitingKey = true;
        keyGap.set(true);
    }

    private int enqueue(U unit, int frames, boolean keyUnit) {
        synchronized (lock) {
            // The worker may have failed since the caller looked.
            Integer refused = gate(frames, keyUnit);
            if (refused != null) return refused;
            if (queue.size() >= bound) {
                // The decoder fell behind by the whole bound: drop what waits, as the engines' decode
                // unit queue does, and ask for a key frame.
                dropQueued(frames);
                reportIdr = false;
                return MoonBridge.DR_NEED_IDR;
            }
            queue.add(new Entry<>(unit, frames));
            if (thread == null) {
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
            }
            int status = MoonBridge.DR_NEED_IDR;
            try {
                status = worker.submit(entry.unit);
            } finally {
                synchronized (lock) {
                    // A failure and its flags are one step: the receive thread never sees an idle
                    // worker that has yet to say the decoder lost a frame.
                    if (status != MoonBridge.DR_OK) {
                        dropQueued(0);
                        reportIdr = true;
                    }
                    working = false;
                }
            }
        }
    }

    /** Frames dropped for the decoder: refused by a full queue, or queued behind a failure. */
    long drops() {
        return drops.get();
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

    /**
     * Drops what waits and stops the worker once its unit in hand is done; waits for it at most
     * {@code joinMs} (0: not at all). Whether the worker has ended.
     */
    boolean close(long joinMs) {
        Thread joined;
        synchronized (lock) {
            closed = true;
            queue.clear();
            joined = thread;
            lock.notifyAll();
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
