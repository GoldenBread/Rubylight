package com.limelight.binding.video;

import com.limelight.nvstream.DecodeTiming;

import java.util.Arrays;
import java.util.Locale;

/**
 * Rolling sample windows, frame-rate and display-drop counters, decode-timing quality polling,
 * Artemis-compatible decode accounting, and scanout latency summaries for {@link FrameLatencyStats}.
 * All instance methods run under the owning {@link FrameLatencyStats} monitor.
 */
final class FrameLatencyMetrics {
    private static final int WINDOW_SIZE = FrameLatencyStats.WINDOW_SIZE;
    private static final int STAGES = FrameLatencyStats.STAGES;
    private static final long CALLBACK_TIMEOUT_NS = FrameLatencyStats.CALLBACK_TIMEOUT_NS;
    private static final long FPS_WINDOW_NS = FrameLatencyStats.FPS_WINDOW_NS;
    private static final long MAX_PRESENT_REPORT_LAG_NS = FrameLatencyStats.MAX_PRESENT_REPORT_LAG_NS;

    private final long[][] samples = new long[STAGES][WINDOW_SIZE];
    private final long[][] sampleTimes = new long[STAGES][WINDOW_SIZE];
    private final int[] counts = new int[STAGES];
    private final int[] positions = new int[STAGES];
    // Each stage's sample its last one replaced, for removeLastSample.
    private final long[] overwritten = new long[STAGES];
    private final long[] overwrittenTimes = new long[STAGES];

    private long decodeTimeTotalNs;
    private int decodeTimeSamples;
    private long totalDecodeTimeNs;
    private long totalDecodedFrames;
    private long decodeTimingConsumedFrames, decodeTimingPollNs, decodeTimingGeneration;
    private long totalClientTimeNs;
    private long totalClientTimeSamples;

    private final long[][] frameTimes = new long[3][2048];
    private final int[] frameTimePositions = new int[3];
    private long fpsStartNs;
    private long latestPresentedNs;
    private boolean renderCallbacksAvailable;
    // The session's display drops for the CSV, and when each was counted for the overlay's window.
    private long presentDrops;
    private final long[] dropTimes = new long[2048];
    private int dropTimePosition;
    // The last expire(): frames released CALLBACK_TIMEOUT_NS before it are decided.
    private long lastExpireNs;

    // The decode time the app shows is counted the way Artemis counts it, so the two apps show the
    // same figure for the same decoder work: each frame Artemis would count is timed from
    // queueInputBuffer() returning to the decoder returning it, rounded down to whole milliseconds, and the
    // sum is later divided by every frame received. The precise timings above stay as they are
    // for adaptive bitrate and quality, the stream doctor, the stage breakdown and the CSV.
    private final FrameLatencyTables.PtsTimes artemisQueuedNs =
            new FrameLatencyTables.PtsTimes(FrameLatencyStats.MAX_ARTEMIS_QUEUED);
    // When the decoder returned each of those frames from dequeueOutputBuffer(). Kept apart from
    // Frame because the render callback can finish a frame before the renderer thread counts it.
    private final FrameLatencyTables.PtsTimes artemisOutputNs =
            new FrameLatencyTables.PtsTimes(FrameLatencyStats.MAX_ARTEMIS_QUEUED);
    private long artemisDecodeMs;

    // Decoded to on screen, from the codec's render callback or the Vulkan renderer's display timing; logged, not shown
    private long scanoutTotalNs;
    private long scanoutMaxNs;
    private int scanoutFrames;

    // Why outputs were dropped this stream (onDropReason), sorted for a stable report row; one
    // holder per reason, so counting allocates nothing after a reason's first drop.
    private final java.util.TreeMap<String, int[]> dropCounts = new java.util.TreeMap<>();

    void recordDropReason(String reason) {
        int[] count = dropCounts.get(reason);
        if (count == null) dropCounts.put(reason, count = new int[1]);
        count[0]++;
    }

    String describeDrops(long notShown) {
        StringBuilder text = new StringBuilder();
        long counted = 0;
        for (java.util.Map.Entry<String, int[]> drop : dropCounts.entrySet()) {
            text.append(text.length() == 0 ? "" : ", ").append(drop.getKey()).append(' ').append(drop.getValue()[0]);
            counted += drop.getValue()[0];
        }
        if (notShown > counted) text.append(text.length() == 0 ? "" : ", ").append("other ").append(notShown - counted);
        return text.length() == 0 ? "none" : text.toString();
    }

    void onFrameReceived(long receiveNs) {
        if (fpsStartNs == 0) fpsStartNs = receiveNs;
        recordFrameTime(0, receiveNs);
    }

    void onArtemisQueued(long ptsUs, long queuedNs, long existingOutputNs) {
        artemisQueuedNs.put(ptsUs, queuedNs);
        if (existingOutputNs != 0) {
            artemisOutputNs.put(ptsUs, existingOutputNs);
        }
    }

    void onArtemisOutput(long ptsUs, long outputNs) {
        // PyroWave never fills the map, so its outputs skip the lookup (and its boxing) entirely.
        if (!artemisQueuedNs.isEmpty() && artemisQueuedNs.contains(ptsUs)) {
            // Decode time ends here, when the decoder hands the frame back.
            artemisOutputNs.put(ptsUs, outputNs);
        }
    }

    void recordDecodedFrame(long inputNs, long outputNs, long receiveNs) {
        addSample(1, inputNs, outputNs);
        decodeTimeTotalNs += outputNs - inputNs;
        decodeTimeSamples++;
        totalDecodeTimeNs += outputNs - inputNs;
        totalDecodedFrames++;
        if (receiveNs > 0 && inputNs >= receiveNs) {
            totalClientTimeNs += outputNs - receiveNs;
            totalClientTimeSamples++;
        }
    }

    void onOutputRendered(long releaseNs, boolean hasRenderCallback) {
        renderCallbacksAvailable = hasRenderCallback;
        recordFrameTime(FrameLatencyStats.RENDERED_STAGE, releaseNs);
    }

    void undoLastRenderedFrameTime(long releaseNs) {
        long[] times = frameTimes[FrameLatencyStats.RENDERED_STAGE];
        int last = (frameTimePositions[FrameLatencyStats.RENDERED_STAGE] + times.length - 1) % times.length;
        if (times[last] == releaseNs) {
            times[last] = 0;
        }
    }

    void onFramePresented(long presentedNs) {
        recordFrameTime(FrameLatencyStats.PRESENTED_STAGE, presentedNs);
        if (presentedNs > latestPresentedNs) latestPresentedNs = presentedNs;
    }

    void recordScanout(long durationNs, long screenNs) {
        addDuration(FrameLatencyStats.SCREEN_STAGE, durationNs, screenNs);
        scanoutTotalNs += durationNs;
        scanoutMaxNs = Math.max(scanoutMaxNs, durationNs);
        scanoutFrames++;
    }

    String takeScanoutLatency() {
        if (scanoutFrames == 0) {
            return null;
        }
        String line = String.format(Locale.US, "Scanout latency (decoded to on screen): avg %.1f ms, max %.1f ms, %d frames",
                scanoutTotalNs / 1e6 / scanoutFrames, scanoutMaxNs / 1e6, scanoutFrames);
        scanoutTotalNs = 0;
        scanoutMaxNs = 0;
        scanoutFrames = 0;
        return line;
    }

    void markRenderCallbacksUnavailable() {
        renderCallbacksAvailable = false;
    }

    void onDiscardPending() {
        decodeTimeTotalNs = 0;
        decodeTimeSamples = 0;
        resetDecodeTiming();
        artemisQueuedNs.clear();
        artemisOutputNs.clear();
        fpsStartNs = 0;
        latestPresentedNs = 0;
        for (long[] times : frameTimes) Arrays.fill(times, 0);
        Arrays.fill(dropTimes, 0);
    }

    float takeDecodeTimeMs() {
        float result = decodeTimeSamples == 0 ? -1 : decodeTimeTotalNs / (decodeTimeSamples * 1000000.0f);
        decodeTimeTotalNs = 0;
        decodeTimeSamples = 0;
        return result;
    }

    DecodeTiming takeDecodeTiming(long nowNs, int fps) {
        long startNs = Math.max(decodeTimingPollNs, Math.max(0, nowNs - FPS_WINDOW_NS));
        int available = (int) Math.min(counts[1], totalDecodedFrames - decodeTimingConsumedFrames);
        int sampled = 0, late = 0;
        long first = 0, last = 0;
        if (nowNs > 0 && nowNs >= decodeTimingPollNs && fps > 0) {
            for (int offset = 1; offset <= available; offset++) {
                int index = (positions[1] - offset + WINDOW_SIZE) % WINDOW_SIZE;
                long observed = sampleTimes[1][index];
                if (observed <= startNs || observed > nowNs) continue;
                sampled++;
                if (samples[1][index] > DecodeTiming.decodeBudgetMs(fps) * 1_000_000d) late++;
                first = first == 0 ? observed : Math.min(first, observed);
                last = Math.max(last, observed);
            }
        }
        decodeTimingConsumedFrames = totalDecodedFrames;
        decodeTimingPollNs = Math.max(0, nowNs);
        return new DecodeTiming(decodeTimingGeneration, startNs, nowNs, first, last, fps, sampled, late);
    }

    void resetDecodeTiming() {
        decodeTimingGeneration++;
        decodeTimingConsumedFrames = totalDecodedFrames;
        decodeTimingPollNs = 0;
    }

    // A frame Artemis counts: the one it presents, or one it puts in the pacing queue without
    // dropping an older one. Its decode time runs from queueInputBuffer() returning to the
    // decoder returning the frame; release and pacing waits after that are not decode time. Like
    // Artemis, a frame is rounded down to whole milliseconds and a second or more is left out.
    void onArtemisDecodeCounted(long ptsUs) {
        long queuedNs = artemisQueuedNs.remove(ptsUs);
        long outputNs = artemisOutputNs.remove(ptsUs);
        if (queuedNs != FrameLatencyTables.PtsTimes.ABSENT &&
                outputNs != FrameLatencyTables.PtsTimes.ABSENT && outputNs >= queuedNs) {
            long ms = (outputNs - queuedNs) / 1000000L;
            if (ms < 1000) {
                artemisDecodeMs += ms;
            }
        }
    }

    long getArtemisDecodeMs() {
        return artemisDecodeMs;
    }

    long takeArtemisDecodeMs() {
        long result = artemisDecodeMs;
        artemisDecodeMs = 0;
        return result;
    }

    int getAverageDecoderLatency() {
        return totalDecodedFrames == 0 ? 0 : (int) (totalDecodeTimeNs / totalDecodedFrames / 1000000);
    }

    int getAverageEndToEndLatency() {
        return totalClientTimeSamples == 0 ? 0 : (int) (totalClientTimeNs / totalClientTimeSamples / 1000000);
    }

    private void recordFrameTime(int stage, long timeNs) {
        frameTimes[stage][frameTimePositions[stage]] = timeNs;
        frameTimePositions[stage] = (frameTimePositions[stage] + 1) % frameTimes[stage].length;
    }

    float[] getFrameRates(long nowNs) {
        float[] rates = new float[3];
        if (fpsStartNs != 0) {
            for (int stage = 0; stage < rates.length; stage++) {
                long endNs = nowNs;
                if (stage == FrameLatencyStats.PRESENTED_STAGE && latestPresentedNs > 0 && latestPresentedNs < nowNs) {
                    endNs = Math.max(latestPresentedNs, nowNs - MAX_PRESENT_REPORT_LAG_NS);
                }
                long startNs = Math.max(fpsStartNs, endNs - FPS_WINDOW_NS);
                if (endNs <= startNs) continue;
                int count = 0;
                for (long timestamp : frameTimes[stage]) {
                    if (timestamp > startNs && timestamp <= endNs) count++;
                }
                rates[stage] = (float) (count * 1000000000.0 / (endNs - startNs));
            }
        }
        if (!renderCallbacksAvailable) rates[2] = -1;
        return rates;
    }

    long getPresentDrops() {
        return presentDrops;
    }

    void countPresentDrop(long timeNs) {
        presentDrops++;
        dropTimes[dropTimePosition] = timeNs;
        dropTimePosition = (dropTimePosition + 1) % dropTimes.length;
    }

    void noteExpire(long nowNs) {
        lastExpireNs = Math.max(lastExpireNs, nowNs);
    }

    float getDisplayDropPercent(long nowNs) {
        long endNs = Math.min(nowNs, lastExpireNs) - CALLBACK_TIMEOUT_NS;
        long startNs = Math.max(fpsStartNs, endNs - FPS_WINDOW_NS);
        if (fpsStartNs == 0 || lastExpireNs == 0 || endNs <= startNs || !renderCallbacksAvailable) return -1;
        int submitted = countWithin(frameTimes[FrameLatencyStats.RENDERED_STAGE],
                frameTimePositions[FrameLatencyStats.RENDERED_STAGE], startNs, endNs);
        int drops = countWithin(dropTimes, dropTimePosition, startNs, endNs);
        if (submitted <= 0 || drops < 0) return -1;
        return Math.min(100f, 100f * drops / submitted);
    }

    // Timestamps in (startNs, endNs] of a ring whose next write goes to position; -1 when the ring
    // has wrapped past startNs, so part of the window is gone.
    private static int countWithin(long[] ring, int position, long startNs, long endNs) {
        long oldest = ring[position];
        if (oldest != 0 && oldest > startNs) return -1;
        int count = 0;
        for (long timestamp : ring) {
            if (timestamp > startNs && timestamp <= endNs) count++;
        }
        return count;
    }

    void addSample(int stage, long startNs, long endNs) {
        if (startNs == 0 || endNs < startNs) {
            return;
        }
        addDuration(stage, endNs - startNs, endNs);
    }

    void addDuration(int stage, long durationNs, long observedAtNs) {
        overwritten[stage] = samples[stage][positions[stage]];
        overwrittenTimes[stage] = sampleTimes[stage][positions[stage]];
        samples[stage][positions[stage]] = durationNs;
        sampleTimes[stage][positions[stage]] = observedAtNs;
        positions[stage] = (positions[stage] + 1) % WINDOW_SIZE;
        counts[stage] = Math.min(counts[stage] + 1, WINDOW_SIZE);
    }

    // Takes back the stage's last sample if it was observed at observedAtNs, restoring the one it
    // replaced in a full window.
    void removeLastSample(int stage, long observedAtNs) {
        int last = (positions[stage] + WINDOW_SIZE - 1) % WINDOW_SIZE;
        if (counts[stage] == 0 || sampleTimes[stage][last] != observedAtNs) return;
        positions[stage] = last;
        if (counts[stage] < WINDOW_SIZE) {
            counts[stage]--;
            samples[stage][last] = 0;
            sampleTimes[stage][last] = 0;
        } else {
            samples[stage][last] = overwritten[stage];
            sampleTimes[stage][last] = overwrittenTimes[stage];
        }
    }

    long[][] snapshotSamples(long nowNs, long windowNs, boolean recentOnly) {
        long[][] snapshot = new long[samples.length][];
        for (int stage = 0; stage < snapshot.length; stage++) {
            long[] values = new long[counts[stage]];
            int count = 0;
            for (int i = 0; i < counts[stage]; i++) {
                long observedAt = sampleTimes[stage][i];
                if (!recentOnly || (observedAt > 0 && observedAt <= nowNs && nowNs - observedAt <= windowNs)) {
                    values[count++] = samples[stage][i];
                }
            }
            snapshot[stage] = count == values.length ? values : Arrays.copyOf(values, count);
        }
        return snapshot;
    }

    static double[][] summarizeSnapshot(long[][] snapshot, boolean median) {
        // Per stage: samples, average, p95, p99 and, when asked for, median, in milliseconds.
        double[][] summary = new double[snapshot.length][median ? 5 : 4];
        for (int stage = 0; stage < snapshot.length; stage++) {
            long[] values = snapshot[stage];
            summary[stage][0] = values.length;
            if (values.length == 0) {
                continue;
            }
            Arrays.sort(values);
            double sum = 0;
            for (long value : values) {
                sum += value;
            }
            summary[stage][1] = sum / values.length / 1000000;
            summary[stage][2] = values[(int) Math.ceil(values.length * 0.95) - 1] / 1000000.0;
            summary[stage][3] = values[(int) Math.ceil(values.length * 0.99) - 1] / 1000000.0;
            if (median) summary[stage][4] = values[(int) Math.ceil(values.length * 0.5) - 1] / 1000000.0;
        }
        return summary;
    }
}
