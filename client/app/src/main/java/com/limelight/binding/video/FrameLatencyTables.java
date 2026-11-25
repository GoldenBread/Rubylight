package com.limelight.binding.video;

import com.limelight.nvstream.av.video.HostTimingReport;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Iterator;
import java.util.LinkedHashMap;

/**
 * Zero-allocation primitive-keyed lookup tables and bounded frame storage supporting
 * {@link FrameLatencyStats}: open-addressed pending-frame map in arrival order, bounded
 * PTS-to-timestamp ring map, dense-plus-sparse output-buffer index table, and frame
 * pool / completed-queue / early host-timing report storage.
 */
final class FrameLatencyTables {
    private FrameLatencyTables() {}

    static int hashSlot(long key, int mask) {
        long h = key * 0x9E3779B97F4A7C15L;
        return (int) (h ^ (h >>> 32)) & mask;
    }

    /** Frames by ptsUs in arrival order: an open-addressed table and a list through the frames. */
    static final class PendingFrames {
        private final FrameLatencyStats.Frame[] table;
        private final int mask;
        FrameLatencyStats.Frame oldest, newest;
        int size;

        PendingFrames(int capacity) {
            table = new FrameLatencyStats.Frame[Integer.highestOneBit(capacity * 2 - 1) << 1];
            mask = table.length - 1;
        }

        FrameLatencyStats.Frame get(long ptsUs) {
            for (int i = hashSlot(ptsUs, mask); table[i] != null; i = (i + 1) & mask) {
                if (table[i].ptsUs == ptsUs) return table[i];
            }
            return null;
        }

        /** Adds the newest frame; one already waiting with the same ptsUs is replaced, as a map would. */
        void put(FrameLatencyStats.Frame frame) {
            FrameLatencyStats.Frame previous = get(frame.ptsUs);
            if (previous != null) remove(previous);
            int i = hashSlot(frame.ptsUs, mask);
            while (table[i] != null) i = (i + 1) & mask;
            table[i] = frame;
            frame.older = newest;
            frame.newer = null;
            if (newest != null) newest.newer = frame; else oldest = frame;
            newest = frame;
            size++;
        }

        void remove(FrameLatencyStats.Frame frame) {
            int i = hashSlot(frame.ptsUs, mask);
            while (table[i] != null && table[i] != frame) i = (i + 1) & mask;
            if (table[i] == null) return;
            // Backward-shift deletion keeps every probe chain unbroken.
            int hole = i;
            for (int j = (i + 1) & mask; table[j] != null; j = (j + 1) & mask) {
                int home = hashSlot(table[j].ptsUs, mask);
                if (((j - home) & mask) >= ((j - hole) & mask)) {
                    table[hole] = table[j];
                    hole = j;
                }
            }
            table[hole] = null;
            if (frame.older != null) frame.older.newer = frame.newer; else oldest = frame.newer;
            if (frame.newer != null) frame.newer.older = frame.older; else newest = frame.older;
            frame.older = frame.newer = null;
            size--;
        }
    }

    /**
     * ptsUs to a time without boxing. A key is forgotten once {@code capacity} newer keys were
     * added, like a LinkedHashMap that drops its eldest entry; updating a key keeps its age.
     */
    static final class PtsTimes {
        static final long ABSENT = Long.MIN_VALUE;
        private final long[] keys;
        private final long[] values;
        private final int mask;
        private final long[] order;
        private int orderHead;
        private int size;

        PtsTimes(int capacity) {
            keys = new long[Integer.highestOneBit(capacity * 4 - 1) << 1];
            values = new long[keys.length];
            mask = keys.length - 1;
            order = new long[capacity];
            clear();
        }

        boolean isEmpty() {
            return size == 0;
        }

        private int find(long key) {
            for (int i = hashSlot(key, mask); keys[i] != ABSENT; i = (i + 1) & mask) {
                if (keys[i] == key) return i;
            }
            return -1;
        }

        boolean contains(long key) {
            return find(key) >= 0;
        }

        void put(long key, long value) {
            int i = find(key);
            if (i >= 0) {
                values[i] = value;
                return;
            }
            long eldest = order[orderHead];
            if (eldest != ABSENT) remove(eldest);
            order[orderHead] = key;
            orderHead = (orderHead + 1) % order.length;
            i = hashSlot(key, mask);
            while (keys[i] != ABSENT) i = (i + 1) & mask;
            keys[i] = key;
            values[i] = value;
            size++;
        }

        /** Removes the key and returns its time, or {@link #ABSENT}. */
        long remove(long key) {
            int i = find(key);
            if (i < 0) return ABSENT;
            long value = values[i];
            int hole = i;
            for (int j = (i + 1) & mask; keys[j] != ABSENT; j = (j + 1) & mask) {
                int home = hashSlot(keys[j], mask);
                if (((j - home) & mask) >= ((j - hole) & mask)) {
                    keys[hole] = keys[j];
                    values[hole] = values[j];
                    hole = j;
                }
            }
            keys[hole] = ABSENT;
            size--;
            return value;
        }

        void clear() {
            Arrays.fill(keys, ABSENT);
            Arrays.fill(order, ABSENT);
            orderHead = 0;
            size = 0;
        }
    }

    /**
     * Frames by output index. MediaCodec's buffer indices are small and live in a growing array;
     * any other index (PyroWave passes its presentation time truncated to an int, which can be
     * in the hundreds of millions or negative) goes to a fixed table keyed by Frame.outputIndex,
     * so no index can size an allocation (the array once grew to 3 GB and threw OutOfMemoryError).
     */
    static final class OutputIndexTable {
        private static final int MAX_DENSE_OUTPUTS = 1024;
        private final int maxSparseOutputs;
        private FrameLatencyStats.Frame[] outputs = new FrameLatencyStats.Frame[64];
        private FrameLatencyStats.Frame[] sparseOutputs; // allocated on the first index past the array
        private int sparseOutputCount;

        OutputIndexTable(int maxSparseOutputs) {
            this.maxSparseOutputs = maxSparseOutputs;
        }

        private static boolean isDenseOutput(int index) {
            return index >= 0 && index < MAX_DENSE_OUTPUTS;
        }

        // The table is never full (at most maxSparseOutputs of twice as many slots), so this ends.
        private int sparseOutputSlot(int index) {
            int mask = sparseOutputs.length - 1;
            int i = hashSlot(index, mask);
            while (sparseOutputs[i] != null && sparseOutputs[i].outputIndex != index) i = (i + 1) & mask;
            return i;
        }

        FrameLatencyStats.Frame get(int index) {
            if (isDenseOutput(index)) return index < outputs.length ? outputs[index] : null;
            return index == -1 || sparseOutputs == null ? null : sparseOutputs[sparseOutputSlot(index)];
        }

        // Sets frame.outputIndex; a frame already at that index loses it, as an overwritten slot would.
        void put(int index, FrameLatencyStats.Frame frame) {
            if (frame.outputIndex != -1 && frame.outputIndex != index) remove(frame.outputIndex);
            if (index == -1) return;
            FrameLatencyStats.Frame previous;
            if (isDenseOutput(index)) {
                if (index >= outputs.length) {
                    outputs = Arrays.copyOf(outputs,
                            Math.min(MAX_DENSE_OUTPUTS, Math.max(index + 1, outputs.length * 2)));
                }
                previous = outputs[index];
                outputs[index] = frame;
            } else {
                if (sparseOutputs == null) {
                    sparseOutputs = new FrameLatencyStats.Frame[Integer.highestOneBit(maxSparseOutputs * 2 - 1) << 1];
                }
                int i = sparseOutputSlot(index);
                previous = sparseOutputs[i];
                if (previous == null) {
                    // Entries are frames still tracked, so this needs about MAX_PENDING outputs in
                    // flight; the frame then finishes as output_unobserved in the CSV.
                    if (sparseOutputCount == maxSparseOutputs) return;
                    sparseOutputCount++;
                }
                sparseOutputs[i] = frame;
            }
            if (previous != null && previous != frame) previous.outputIndex = -1;
            frame.outputIndex = index;
        }

        FrameLatencyStats.Frame remove(int index) {
            FrameLatencyStats.Frame frame;
            if (isDenseOutput(index)) {
                if (index >= outputs.length || (frame = outputs[index]) == null) return null;
                outputs[index] = null;
            } else {
                if (index == -1 || sparseOutputs == null) return null;
                FrameLatencyStats.Frame[] table = sparseOutputs;
                int mask = table.length - 1;
                int hole = sparseOutputSlot(index);
                if ((frame = table[hole]) == null) return null;
                // Backward-shift deletion keeps every probe chain unbroken.
                for (int j = (hole + 1) & mask; table[j] != null; j = (j + 1) & mask) {
                    int home = hashSlot(table[j].outputIndex, mask);
                    if (((j - home) & mask) >= ((j - hole) & mask)) {
                        table[hole] = table[j];
                        hole = j;
                    }
                }
                table[hole] = null;
                sparseOutputCount--;
            }
            frame.outputIndex = -1;
            return frame;
        }

        int slotCount() {
            return outputs.length + (sparseOutputs == null ? 0 : sparseOutputs.length);
        }
    }

    /**
     * Recycled {@link FrameLatencyStats.Frame} pool, direct-mapped frame-number index,
     * completed-frame queue for CSV export, and bounded early host-timing report cache.
     */
    static final class FrameStore {
        private static final int MAX_COMPLETED = 4096;
        private static final int MAX_SPARE_FRAMES = 512;
        // Reports that arrive before their frame, and frames finished before their report, are matched
        // within these bounds; a report older than that is dropped.
        private static final int MAX_EARLY_REPORTS = 256;
        private static final int MAX_COMPLETED_SCAN = 512;
        // Direct-mapped O(1) lookup by frameNumber for onHostTiming (falls back to scan on slot collision).
        private static final int BY_NUMBER_SIZE = 512;
        private static final int BY_NUMBER_MASK = BY_NUMBER_SIZE - 1;

        private final ArrayDeque<FrameLatencyStats.Frame> spareFrames = new ArrayDeque<>();
        private final FrameLatencyStats.Frame[] byNumber = new FrameLatencyStats.Frame[BY_NUMBER_SIZE];
        private final ArrayDeque<FrameLatencyStats.Frame> completed = new ArrayDeque<>();
        private final LinkedHashMap<Integer, HostTimingReport.Record> earlyReports = new LinkedHashMap<>();
        private long csvRowsLost;

        FrameLatencyStats.Frame newFrame() {
            FrameLatencyStats.Frame frame = spareFrames.pollFirst();
            return frame != null ? frame : new FrameLatencyStats.Frame();
        }

        void indexByNumber(FrameLatencyStats.Frame frame) {
            byNumber[frame.number & BY_NUMBER_MASK] = frame;
        }

        // Most frames have no early report: skip the Integer the lookup would box for each.
        HostTimingReport.Record takeEarlyReport(int frameNumber) {
            return earlyReports.isEmpty() ? null : earlyReports.remove(frameNumber);
        }

        void putEarlyReport(int frameNumber, HostTimingReport.Record record) {
            if (earlyReports.size() == MAX_EARLY_REPORTS) {
                Iterator<Integer> oldest = earlyReports.keySet().iterator();
                oldest.next();
                oldest.remove();
            }
            earlyReports.put(frameNumber, record);
        }

        FrameLatencyStats.Frame findFrame(int frameNumber, PendingFrames pending) {
            FrameLatencyStats.Frame cached = byNumber[frameNumber & BY_NUMBER_MASK];
            if (cached != null && cached.number == frameNumber) {
                return cached;
            }
            for (FrameLatencyStats.Frame frame = pending.oldest; frame != null; frame = frame.newer) {
                if (frame.number == frameNumber) {
                    return frame;
                }
            }
            int scanned = 0;
            for (Iterator<FrameLatencyStats.Frame> newest = completed.descendingIterator();
                 newest.hasNext() && scanned < MAX_COMPLETED_SCAN; scanned++) {
                FrameLatencyStats.Frame frame = newest.next();
                if (frame.number == frameNumber) {
                    return frame;
                }
            }
            return null;
        }

        /** Adds frame to the completed queue; returns the evicted oldest frame if full, else null. */
        FrameLatencyStats.Frame addCompleted(FrameLatencyStats.Frame frame) {
            FrameLatencyStats.Frame evicted = null;
            if (completed.size() == MAX_COMPLETED) {
                evicted = completed.removeFirst();
                csvRowsLost++;
            }
            completed.addLast(frame);
            return evicted;
        }

        void recycle(FrameLatencyStats.Frame frame) {
            int slot = frame.number & BY_NUMBER_MASK;
            if (byNumber[slot] == frame) byNumber[slot] = null;
            if (spareFrames.size() < MAX_SPARE_FRAMES) {
                frame.reset();
                spareFrames.addFirst(frame);
            }
        }

        long getCsvRowsLost() {
            return csvRowsLost;
        }

        boolean hasCompletedFrames() {
            return !completed.isEmpty();
        }

        void drainCompleted(ArrayList<FrameLatencyStats.Frame> dest) {
            dest.addAll(completed);
            completed.clear();
            for (int i = 0; i < dest.size(); i++) {
                FrameLatencyStats.Frame f = dest.get(i);
                int slot = f.number & BY_NUMBER_MASK;
                if (byNumber[slot] == f) byNumber[slot] = null;
            }
        }
    }
}
