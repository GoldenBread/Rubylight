package com.limelight.binding.video;

/**
 * Primitive {@code int} FIFO ring buffer for queued MediaCodec output buffer indices.
 *
 * <p>Replaces {@code LinkedBlockingQueue<Integer>} where callers already synchronize on the queue
 * instance, avoiding {@link Integer} boxing, per-element node allocation, and a redundant internal
 * lock on every frame. Non-final with an overridable {@link #size()} so unit tests can coordinate
 * concurrency races on the queue.
 */
class IntRing {
    /** Sentinel returned by {@link #poll()} when the ring is empty (valid codec buffer indices are >= 0). */
    static final int EMPTY = -1;

    private int[] items;
    private int head;
    private int tail;
    private int count;

    IntRing() {
        this(8);
    }

    IntRing(int initialCapacity) {
        int cap = 1;
        while (cap < Math.max(4, initialCapacity)) cap <<= 1;
        this.items = new int[cap];
    }

    void add(int value) {
        if (count == items.length) grow();
        items[tail] = value;
        tail = (tail + 1) & (items.length - 1);
        count++;
    }

    int poll() {
        if (count == 0) return EMPTY;
        int value = items[head];
        head = (head + 1) & (items.length - 1);
        count--;
        return value;
    }

    int size() {
        return count;
    }

    boolean isEmpty() {
        return count == 0;
    }

    void clear() {
        head = 0;
        tail = 0;
        count = 0;
    }

    /** Test helper: copies queued elements in FIFO order without removing them. */
    java.util.List<Integer> toList() {
        java.util.ArrayList<Integer> out = new java.util.ArrayList<>(count);
        for (int i = 0; i < count; i++) out.add(items[(head + i) & (items.length - 1)]);
        return out;
    }

    private void grow() {
        int oldCap = items.length;
        int[] next = new int[oldCap << 1];
        int first = oldCap - head;
        System.arraycopy(items, head, next, 0, first);
        if (head > 0) System.arraycopy(items, 0, next, first, head);
        items = next;
        head = 0;
        tail = count;
    }
}
