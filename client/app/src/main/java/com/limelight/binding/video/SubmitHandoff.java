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
 * and the hang detection the in-place path has. That is what the engines do for a decoder
 * without direct submit (the decode unit queue): a unit that does not fit in the bound drops the
 * queued ones with it and fails, which asks the engine for a key frame, and the failure of a
 * queued unit asks for one with the next unit. A decoder that keeps up never leaves the in-place
 * path, so nothing changes for it.
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
    private final AtomicLong drops = new AtomicLong();
    private final AtomicBoolean needIdr = new AtomicBoolean();
    private Thread thread;
    private boolean working;
    private boolean closed;

    /** At most {@code bound} units wait for the worker, which is {@code worker}'s thread. */
    SubmitHandoff(int bound, String threadName, Worker<U> worker) {
        this.bound = Math.max(1, bound);
        this.threadName = threadName;
        this.worker = worker;
    }

    /**
     * Submits one unit from the receive thread, which must be the only caller.
     *
     * @param inputReady waits at most the receive bound for a decoder buffer; true when it has one
     * @param inPlace submits the unit now, on this thread, and returns its status
     * @param copy the unit as the worker thread takes it: the caller's buffers are reused after return
     * @param frames how many frames the unit counts as when it is dropped (0 for a parameter set)
     * @return the status to give the engine
     */
    int submit(BooleanSupplier inputReady, IntSupplier inPlace, Supplier<U> copy, int frames) {
        int status;
        synchronized (lock) {
            if (closed) return MoonBridge.DR_OK;
        }
        if (isIdle() && inputReady.getAsBoolean()) {
            status = inPlace.getAsInt();
        } else {
            status = enqueue(copy.get(), frames);
        }
        // A queued unit that failed: the engine learns of it with the next unit it hands over.
        return needIdr.getAndSet(false) && status == MoonBridge.DR_OK ? MoonBridge.DR_NEED_IDR : status;
    }

    private boolean isIdle() {
        synchronized (lock) {
            return queue.isEmpty() && !working;
        }
    }

    private int enqueue(U unit, int frames) {
        synchronized (lock) {
            if (closed) return MoonBridge.DR_OK;
            if (queue.size() >= bound) {
                // The decoder fell behind by the whole bound: drop what waits, as the engines' decode
                // unit queue does, and ask for a key frame.
                long dropped = frames;
                for (Entry<U> waiting : queue) dropped += waiting.frames;
                queue.clear();
                drops.addAndGet(dropped);
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
            int status = MoonBridge.DR_OK;
            try {
                status = worker.submit(entry.unit);
            } finally {
                synchronized (lock) {
                    working = false;
                }
            }
            if (status != MoonBridge.DR_OK) {
                drops.addAndGet(entry.frames);
                needIdr.set(true);
            }
        }
    }

    /** Frames dropped for the decoder: refused by a full queue, or failed on the worker. */
    long drops() {
        return drops.get();
    }

    /** Drops what waits and stops the worker once its unit in hand is done, and waits for it at most {@code joinMs} (0: not at all). */
    void close(long joinMs) {
        Thread joined;
        synchronized (lock) {
            closed = true;
            queue.clear();
            joined = thread;
            lock.notifyAll();
        }
        if (joinMs > 0 && joined != null && joined != Thread.currentThread()) {
            try {
                joined.join(joinMs);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }
    }
}
