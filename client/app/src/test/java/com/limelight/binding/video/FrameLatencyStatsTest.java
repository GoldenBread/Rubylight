package com.limelight.binding.video;

import com.limelight.nvstream.DecodeTiming;

import org.junit.Test;

import java.io.StringWriter;

import static org.junit.Assert.*;

public class FrameLatencyStatsTest {
    @Test public void highFrameRateAllowsTwoFramesOfPipelineLatencyButStillCountsStalls() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 120; i++) {
            timedOutput(stats, i, 1_000_000_000L + i * 8_000_000L,
                    i % 10 == 0 ? 20_000_000L : 12_000_000L, 0, -1);
        }
        DecodeTiming timing = stats.takeDecodeTiming(2_000_000_000L, 120);
        assertEquals(120, timing.samples);
        assertEquals(12, timing.lateFrames);
    }

    static void timedOutput(FrameLatencyStats stats, int number, long outputNs,
                                    long decodeNs, long queueNs, long displayNs) {
        long inputNs = outputNs - decodeNs;
        stats.onDecoderInput(number, number, inputNs - queueNs, inputNs - queueNs, inputNs, (char) 0);
        stats.onDecoderQueued(number, inputNs);
        stats.onDecoderOutput(0, number, outputNs);
        stats.onOutputReleased(0, outputNs, displayNs >= 0, displayNs >= 0);
        if (displayNs >= 0) stats.onFrameRendered(number, outputNs + displayNs);
    }

    // A frame received at 1 ms, queued at 2 and decoded at 5 ms.
    private static void decodedAndReleased(FrameLatencyStats stats, long ptsUs) {
        stats.onFrameReceived(1_000_000);
        stats.onDecoderInput((int) ptsUs, ptsUs, 1_000_000, 1_000_000, 2_000_000, (char) 0);
        stats.onDecoderQueued(ptsUs, 2_000_000);
        stats.onDecoderOutput(0, ptsUs, 5_000_000);
    }

    @Test public void submitToOutputStartsBeforeTheQueueCallAndInputToOutputAfterIt() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onFrameReceived(100);
        stats.onDecoderInput(1, 1, 100, 150, 200, (char) 0);
        stats.onDecoderQueued(1, 203);
        stats.onDecoderOutput(5, 1, 210);
        assertEquals("what the overlay and the CSV report", 7, stats.getInputToOutputNs(5));
        assertEquals("what the display gate reads", 10, stats.getSubmitToOutputNs(5));
    }

    @Test public void readOutputTimesGivesWhatTheThreeGettersGive() {
        FrameLatencyStats stats = new FrameLatencyStats();
        FrameLatencyStats.OutputTimes times = new FrameLatencyStats.OutputTimes();
        stats.readOutputTimes(5, times);
        assertEquals(0, times.inputToOutputNs + times.submitToOutputNs + times.outputNs);
        stats.onFrameReceived(100);
        stats.onDecoderInput(1, 1, 100, 150, 200, (char) 0);
        stats.onDecoderQueued(1, 203);
        stats.onDecoderOutput(5, 1, 210);
        stats.readOutputTimes(5, times);
        assertEquals(stats.getInputToOutputNs(5), times.inputToOutputNs);
        assertEquals(stats.getSubmitToOutputNs(5), times.submitToOutputNs);
        assertEquals(stats.getOutputNs(5), times.outputNs);
        assertEquals(7, times.inputToOutputNs);
        assertEquals(10, times.submitToOutputNs);
        assertEquals(210, times.outputNs);
        stats.readOutputTimes(6, times);
        assertEquals("an untracked output reads zeros, not the previous values", 0,
                times.inputToOutputNs + times.submitToOutputNs + times.outputNs);
    }

    @Test public void displayWaitAndClientLatencyEndAtTheHandOffAndTheScreenTimeGoesToItsOwnStage() {
        FrameLatencyStats stats = new FrameLatencyStats();
        decodedAndReleased(stats, 1);
        stats.onOutputHandedOff(0, 6_000_000, 6_000_000, true);
        // The compositor and the panel take 20 ms more: not display wait, not client latency.
        stats.onFrameRendered(1, 26_000_000);
        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {1, 1, 1, 1}, summary[2], 0);
        assertArrayEquals(new double[] {1, 5, 5, 5}, summary[3], 0);
        assertArrayEquals(new double[] {1, 21, 21, 21}, summary[FrameLatencyStats.SCREEN_STAGE], 0);
        // The render callback still counts the frame as shown.
        assertTrue(stats.getFrameRates(26_000_001)[FrameLatencyStats.PRESENTED_STAGE] > 0);
        assertNotNull(stats.takeScanoutLatency());
    }

    @Test public void aHandOffCountsTheTimeNightjarAskedTheFrameToBeShownWithinASecond() {
        assertEquals(10, FrameLatencyStats.handoffNs(10, 0));
        assertEquals(10, FrameLatencyStats.handoffNs(10, 5));
        assertEquals(8_000_010, FrameLatencyStats.handoffNs(10, 8_000_010));
        assertEquals(10, FrameLatencyStats.handoffNs(10, 1_000_000_010L));

        FrameLatencyStats stats = new FrameLatencyStats();
        decodedAndReleased(stats, 1);
        // Held for the PC's frame time: shown 8 ms after the release.
        stats.onOutputHandedOff(0, 6_000_000, 14_000_000, true);
        assertArrayEquals(new double[] {1, 9, 9, 9}, stats.summarize()[2], 0);
    }

    @Test public void framesWithoutRenderCallbacksStillHaveADisplayWaitAtTheHandOff() {
        FrameLatencyStats stats = new FrameLatencyStats();
        decodedAndReleased(stats, 1);
        stats.onOutputHandedOff(0, 7_000_000, 0, false);
        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {1, 2, 2, 2}, summary[2], 0);
        assertEquals(0, summary[FrameLatencyStats.SCREEN_STAGE][0], 0);
    }

    @Test public void vulkanCountsDisplayWaitToItsQueuedPresentAndTheScanoutToTheScreenStage() {
        FrameLatencyStats stats = new FrameLatencyStats();
        decodedAndReleased(stats, 1);
        stats.onOutputReleased(0, 5_000_000, true, true);
        stats.onFrameShown(1, 7_000_000, 30_000_000, 30_100_000);
        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {1, 2, 2, 2}, summary[2], 0);
        assertArrayEquals(new double[] {1, 25, 25, 25}, summary[FrameLatencyStats.SCREEN_STAGE], 0);
    }

    @Test public void pyroWaveCountsDisplayWaitToItsQueuedPresentAndItsDisplayTimeToTheScreenStage() {
        FrameLatencyStats stats = new FrameLatencyStats();
        decodedAndReleased(stats, 1);
        stats.onPresentQueued(0, 8_000_000, true, true);
        stats.onFrameRendered(1, 30_000_000);
        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {1, 3, 3, 3}, summary[2], 0);
        assertArrayEquals(new double[] {1, 7, 7, 7}, summary[3], 0);
        assertArrayEquals(new double[] {1, 25, 25, 25}, summary[FrameLatencyStats.SCREEN_STAGE], 0);
        // A replaced frame was never shown: no hand-off.
        decodedAndReleased(stats, 2);
        stats.onPresentQueued(0, 9_000_000, false, true);
        assertEquals(1, stats.summarize()[2][0], 0);
    }

    @Test public void aFailedReleaseTakesBackItsHandOffSamples() {
        FrameLatencyStats stats = new FrameLatencyStats();
        decodedAndReleased(stats, 1);
        stats.onOutputHandedOff(0, 6_000_000, 6_000_000, true);
        stats.onReleaseFailed(6_000_000);
        double[][] summary = stats.summarize();
        assertEquals(0, summary[2][0], 0);
        assertEquals(0, summary[3][0], 0);
        // In a full window the sample it replaced comes back.
        FrameLatencyStats full = new FrameLatencyStats();
        for (int i = 1; i <= FrameLatencyStats.WINDOW_SIZE; i++) {
            decodedAndReleased(full, i);
            full.onOutputHandedOff(0, 7_000_000, 0, false);
        }
        decodedAndReleased(full, 1000);
        full.onOutputHandedOff(0, 25_000_000, 0, true);
        assertEquals((599 * 2 + 20) / 600.0, full.summarize()[2][1], 0.00001);
        full.onReleaseFailed(25_000_000);
        assertArrayEquals(new double[] {600, 2, 2, 2}, full.summarize()[2], 0.00001);
        assertArrayEquals(new double[] {600, 6, 6, 6}, full.summarize()[3], 0.00001);
    }

    @Test public void theOverlaySummaryAddsTheMedian() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long[] waits = {1, 2, 3, 4, 20};
        for (int i = 0; i < waits.length; i++) {
            decodedAndReleased(stats, i + 1);
            stats.onOutputHandedOff(0, 5_000_000 + waits[i] * 1_000_000, 0, false);
        }
        double[] display = stats.summarizeWithMedian()[2];
        assertEquals(5, display.length);
        assertEquals(6, display[1], 0.00001);
        assertEquals(3, display[4], 0.00001);
        assertEquals(20, display[2], 0.00001);
        assertEquals(4, stats.summarize()[2].length);
    }

    @Test public void decodeTailCountsRecurringSpikesEvenWhenTheMeanIsLowAndFramesAreDropped() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 60; i++) {
            timedOutput(stats, i, 1_000_000_000L + i * 16_000_000L,
                    i % 10 == 0 ? 40_000_000L : 2_000_000L, 0, -1);
        }
        DecodeTiming timing = stats.takeDecodeTiming(2_000_000_000L, 60);
        assertEquals(60, timing.samples);
        assertEquals(6, timing.lateFrames);
        assertEquals(5.8f, stats.takeDecodeTimeMs(), 0.001f);
        assertEquals(60, stats.summarize()[1][0], 0);
        // Reading the same ring again cannot count that burst as a second pressure interval.
        assertEquals(0, stats.takeDecodeTiming(2_500_000_000L, 60).samples);
        timedOutput(stats, 61, 2_800_000_000L, 2_000_000L, 0, 0);
        DecodeTiming next = stats.takeDecodeTiming(3_000_000_000L, 60);
        assertEquals(1, next.samples);
        assertEquals(0, next.lateFrames);
        assertEquals(2_500_000_000L, next.windowStartNs);
    }

    @Test public void networkQueueAndDisplayWaitDoNotBecomeLateDecodeSamples() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 60; i++) {
            timedOutput(stats, i, 1_000_000_000L + i * 16_000_000L,
                    2_000_000L, 80_000_000L, 100_000_000L);
        }
        DecodeTiming timing = stats.takeDecodeTiming(2_000_000_000L, 60);
        assertEquals(60, timing.samples);
        assertEquals(0, timing.lateFrames);
        assertEquals(80, stats.summarize()[0][1], 0);
        assertEquals(100, stats.summarize()[2][1], 0);
    }

    @Test public void decodeTailBoundsHistoryAndUsesTheExactRequestedFrameBudget() {
        FrameLatencyStats stats = new FrameLatencyStats();
        timedOutput(stats, 1, 1_000_000_000L, 50_000_000L, 0, 0);
        // At 50 FPS the two-frame decode budget is 40 ms; only durations above it are late.
        timedOutput(stats, 2, 4_000_000_000L, 40_000_000L, 0, 0);
        timedOutput(stats, 3, 4_020_000_000L, 40_000_001L, 0, 0);
        DecodeTiming timing = stats.takeDecodeTiming(5_000_000_000L, 50);
        assertEquals(2, timing.samples);
        assertEquals(1, timing.lateFrames);
        for (int i = 0; i < FrameLatencyStats.WINDOW_SIZE + 100; i++) {
            timedOutput(stats, 10 + i, 6_000_000_000L + i * 1_000_000L, 1_000_000L, 0, 0);
        }
        assertEquals(FrameLatencyStats.WINDOW_SIZE, stats.takeDecodeTiming(7_000_000_000L, 60).samples);
    }

    @Test public void decoderRecoveryAndQualityResetExcludeThePreviousWorkloadWithoutClearingDiagnostics() {
        FrameLatencyStats stats = new FrameLatencyStats();
        timedOutput(stats, 1, 1_000_000_000L, 40_000_000L, 0, 0);
        DecodeTiming before = stats.takeDecodeTiming(1_100_000_000L, 60);
        timedOutput(stats, 2, 1_200_000_000L, 40_000_000L, 0, 0);
        stats.discardPending("codec_reset");
        timedOutput(stats, 3, 1_300_000_000L, 2_000_000L, 0, 0);
        DecodeTiming after = stats.takeDecodeTiming(1_400_000_000L, 60);
        assertNotEquals(before.generation, after.generation);
        assertEquals(1, after.samples);
        assertEquals(0, after.lateFrames);
        assertEquals(3, stats.summarize()[1][0], 0);
        stats.resetDecodeTiming();
        assertEquals(0, stats.takeDecodeTiming(1_500_000_000L, 120).samples);
    }

    @Test public void invalidAndFutureDecodeTimestampsCannotCountAsPressure() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1, 1, -1, (char) 0);
        stats.onDecoderOutput(0, 1, 10);
        stats.onOutputReleased(0, 10, false, false);
        stats.onDecoderInput(2, 2, 10, 10, 20, (char) 0);
        stats.onDecoderOutput(0, 2, 19);
        stats.onOutputReleased(0, 19, false, false);
        timedOutput(stats, 3, 5_000_000_000L, 40_000_000L, 0, 0);
        assertEquals(0, stats.takeDecodeTiming(4_000_000_000L, 60).samples);
    }

    @Test public void outputQueueAgeIsAvailableOnlyWhileTheTrackedBufferIsOwned() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 100, 150, 200, (char) 0);
        stats.onDecoderOutput(7, 1, 300);
        assertEquals(150, stats.getEnqueueTimeNsForOutput(7));
        assertEquals(0, stats.getEnqueueTimeNsForOutput(8));
        stats.onOutputReleased(7, 400, false, false);
        assertEquals(0, stats.getEnqueueTimeNsForOutput(7));
        stats.onDecoderInput(2, 2, 100, 350, 200, (char) 0);
        stats.onDecoderOutput(7, 2, 300);
        assertEquals(0, stats.getEnqueueTimeNsForOutput(7));
        stats.discardPending("codec_reset");
        assertEquals(0, stats.getEnqueueTimeNsForOutput(7));
    }

    @Test
    public void pyroWaveQueuePreservesPreciseTimingsWithoutMediaCodecCounter() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(7, 123, 1000000, 1000001, 2000000, (char) 0);
        stats.onPyroWaveDecoderQueued(123, 2100011);
        stats.onDecoderOutput(0, 123, 7000033);
        stats.onArtemisDecodeCounted(123);
        assertEquals(0, stats.getArtemisDecodeMs());
        assertArrayEquals(new double[] {1, 1.100010, 1.100010, 1.100010}, stats.summarize()[0], 0.0000001);
        assertArrayEquals(new double[] {1, 4.900022, 4.900022, 4.900022}, stats.summarize()[1], 0.0000001);
        stats.onOutputReleased(0, 8000044, false, false);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals("4.900022", fields[9]);
        assertEquals("1.100010", fields[25]);
    }

    @Test
    public void pyroWaveQueueDoesNotFillOrEvictTheMediaCodecCounter() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderQueued(1, 1000000);
        for (long pts = 2; pts < 3000; pts++) {
            stats.onPyroWaveDecoderQueued(pts, 2000000);
        }
        // PyroWave entries must not consume the bounded legacy map, even without pending frames.
        stats.onDecoderOutput(0, 2999, 9000000);
        stats.onArtemisDecodeCounted(2999);
        assertEquals(0, stats.getArtemisDecodeMs());
        stats.onDecoderOutput(1, 1, 5900000);
        stats.onArtemisDecodeCounted(1);
        assertEquals(4, stats.getArtemisDecodeMs());
    }

    @Test
    public void pyroWaveCompletionBeforeQueueKeepsItsOriginalStart() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 123, 1000000, 1500000, 2000000, (char) 0);
        stats.onDecoderOutput(0, 123, 3000000);
        stats.onPyroWaveDecoderQueued(123, 4000000);
        assertArrayEquals(new double[] {1, 0.5, 0.5, 0.5}, stats.summarize()[0], 0);
        assertArrayEquals(new double[] {1, 1, 1, 1}, stats.summarize()[1], 0);
    }

    // R10c2: PyroWave passes its truncated presentation time as the output index (about 786
    // million on the Retroid); the index array grew to it and threw OutOfMemoryError (3.1 GB).
    @Test
    public void pyroWavePresentationTimeIndicesNeverSizeAnAllocation() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        int initialSlots = stats.outputSlotCount();
        // The Retroid's value, across 2^31 us (the index turns negative) and from the largest int.
        long[] starts = {786_000_000L, (1L << 31) - 2 * 16_667L, Integer.MAX_VALUE - 16_667L};
        int number = 0;
        for (long start : starts) {
            for (long pts = start; pts < start + 300 * 16_667L; pts += 16_667L) {
                long t = 1_000_000_000L + number * 16_667_000L;
                stats.onDecoderInput(++number, pts, t, t + 1_000, t + 2_000, (char) 0);
                stats.onPyroWaveDecoderQueued(pts, t + 2_000);
                stats.onDecoderOutput((int) pts, pts, t + 5_000_000);
                assertEquals(4_998_000, stats.getInputToOutputNs((int) pts));
                stats.onOutputReleased((int) pts, t + 6_000_000, true, true);
                assertEquals(0, stats.getInputToOutputNs((int) pts));
                stats.onFrameRendered(pts, t + 7_000_000);
            }
        }
        assertTrue(stats.outputSlotCount() - initialSlots <= 16384);
        stats.expire(Long.MAX_VALUE / 2);
        assertEquals(0, stats.getPresentDrops());
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().trim().split("\n");
        assertEquals(900, rows.length);
        for (String row : rows) assertEquals(row, "rendered", row.split(",", -1)[7]);
    }

    @Test
    public void manyPyroWaveOutputsInFlightAreEachReleasedByTheirOwnIndex() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        int frames = 2000;
        for (int i = 0; i < frames; i++) {
            long pts = 786_000_000L + i * 16_667L;
            stats.onDecoderInput(i + 1, pts, 1000, 1001, 1002, (char) 0);
            stats.onDecoderOutput((int) pts, pts, 2000 + i);
        }
        // Released in a scattered order, so removals shift the probe chains of entries still held.
        for (int i = 0; i < frames; i++) {
            int k = (int) ((i * 7919L) % frames);
            long pts = 786_000_000L + k * 16_667L;
            assertEquals(998 + k, stats.getInputToOutputNs((int) pts));
            stats.onOutputReleased((int) pts, 3_000_000, false, false);
            assertEquals(0, stats.getInputToOutputNs((int) pts));
        }
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().trim().split("\n");
        assertEquals(frames, rows.length);
        for (String row : rows) assertEquals(row, "dropped", row.split(",", -1)[7]);
    }

    @Test
    public void mediaCodecIndicesStayInTheArrayAndAReusedIndexMovesToTheNewFrame() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 100, 150, 200, (char) 0);
        stats.onDecoderOutput(1000, 1, 300);
        assertEquals(100, stats.getInputToOutputNs(1000));
        assertEquals(1001, stats.outputSlotCount());
        stats.onDecoderInput(2, 2, 100, 150, 250, (char) 0);
        stats.onDecoderOutput(1000, 2, 400);
        assertEquals(150, stats.getInputToOutputNs(1000));
        // Finishing the first frame must not take the index from the second.
        stats.discard(1, "test");
        assertEquals(150, stats.getInputToOutputNs(1000));
    }

    @Test
    public void recentSummaryExpiresEachStageEvenWhenOtherStagesKeepReceivingFrames() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1000000000, 1000000000, 1001000000, (char) 300);
        stats.onDecoderQueued(1, 1001000000);
        stats.onDecoderOutput(0, 1, 1003000000);
        stats.onOutputReleased(0, 1004000000, true, true);
        stats.onFrameRendered(1, 1040000000);
        // Fresh frames have decode measurements, but no host timing or render callback.
        stats.onDecoderInput(2, 2, 5000000000L, 5000000000L, 5001000000L, (char) 0);
        stats.onDecoderQueued(2, 5001000000L);
        stats.onDecoderOutput(0, 2, 5004000000L);
        stats.onOutputReleased(0, 5005000000L, true, false);

        double[][] recent = stats.summarizeRecent(6000000000L, 2000000000L);
        assertArrayEquals(new double[] {1, 3, 3, 3}, recent[1], 0);
        assertEquals(0, recent[2][0], 0);
        assertEquals(0, recent[4][0], 0);
        // The existing last-N-frame overlay remains independent of diagnostic freshness.
        assertEquals(1, stats.summarize()[2][0], 0);
        assertEquals(1, stats.summarize()[4][0], 0);
    }

    @Test
    public void recentSummaryHonorsTimeBoundariesAndBoundsTheSampleRing() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 799, 799, 799, (char) 100);
        stats.onDecoderInput(2, 2, 800, 800, 800, (char) 20);
        stats.onDecoderInput(3, 3, 1000, 1000, 1000, (char) 40);
        stats.onDecoderInput(4, 4, 1001, 1001, 1001, (char) 100);
        assertArrayEquals(new double[] {2, 3, 4, 4}, stats.summarizeRecent(1000, 200)[4], 0);

        for (int i = 0; i < FrameLatencyStats.WINDOW_SIZE + 10; i++) {
            stats.onDecoderInput(i + 10, i + 10, 1100, 1100, 1100, (char) 10);
        }
        assertArrayEquals(new double[] {600, 1, 1, 1}, stats.summarizeRecent(1100, 200)[4], 0);
        assertEquals(0, stats.summarizeRecent(1301, 200)[4][0], 0);
    }

    @Test
    public void surfaceRecreationDoesNotCountCancelledPresentationsAsDrops() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 123, 1000000, 1500000, 2000000, (char) 0);
        stats.onDecoderOutput(0, 123, 3000000);
        stats.onOutputReleased(0, 4000000, true, true);
        stats.discard(123, "surface_changed");
        stats.expire(10000000000L);
        assertEquals(0, stats.getPresentDrops());
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains(",surface_changed,"));
    }

    @Test
    public void csvDurationsKeepNanosecondPrecisionWithoutLocaleFormatting() throws Exception {
        java.util.Locale previous = java.util.Locale.getDefault();
        try {
            java.util.Locale.setDefault(java.util.Locale.GERMANY);
            FrameLatencyStats stats = new FrameLatencyStats();
            stats.onDecoderInput(1, 123, 1000000, 1000001, 1000011, (char) 1);
            stats.onDecoderOutput(0, 123, 1234567890);
            stats.onOutputReleased(0, 1234567891, false, false);
            StringWriter csv = new StringWriter();
            stats.writeCsv(csv);
            String[] fields = csv.toString().trim().split(",", -1);
            assertEquals("0.000011", fields[8]);
            assertEquals("1233.567879", fields[9]);
            assertEquals("0.100000", fields[13]);
            assertEquals("0.000010", fields[25]);
        } finally {
            java.util.Locale.setDefault(previous);
        }
    }

    @Test
    public void aFrameTheRendererReplacedCountsAsADropAtOnceAndOnlyOnce() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 123, 1000000, 1500000, 2000000, (char) 0);
        stats.onDecoderOutput(0, 123, 3000000);
        stats.onOutputReleased(0, 4000000, true, true);
        stats.onFrameReplaced(123);
        assertEquals(1, stats.getPresentDrops());
        // A second report, or the timeout, finds the frame finished
        stats.onFrameReplaced(123);
        stats.expire(10000000000L);
        assertEquals(1, stats.getPresentDrops());
    }

    // Regression: the overlay's display drops were a running total for the whole stream, so one bad
    // moment read as an ongoing fault for the rest of it. Then (review) the windowed share divided
    // drops noticed at the 5 s callback timeout by the last 2 s of frames, so a 120 fps game's drops
    // read as a huge share of a 30 fps menu that followed it.
    @Test
    public void estimatedDisplayDropsAreAShareOfOneSettledWindowAcrossAFrameRateChange() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long startNs = 1_000_000_000L;
        stats.onFrameReceived(startNs);
        int pts = 0;
        // Six seconds at 120 fps where every tenth frame gets no render callback, then eight
        // seconds of a 30 fps menu where every frame does.
        for (long offset = 8_333_333L; offset <= 6_000_000_000L; offset += 8_333_333L) {
            frame(stats, ++pts, startNs + offset, pts % 10 != 0);
        }
        for (long offset = 6_033_333_333L; offset <= 14_000_000_000L; offset += 33_333_333L) {
            frame(stats, ++pts, startNs + offset, true);
        }
        // Not before a window's frames are all decided.
        stats.expire(startNs + 5_000_000_000L);
        assertEquals(-1, stats.getDisplayDropPercent(startNs + 5_000_000_000L), 0);
        // The drops are only known through the timeout, polled once a second as the renderer does.
        for (long t = 6_000_000_000L; t <= 8_000_000_000L; t += 1_000_000_000L) stats.expire(startNs + t);
        assertEquals(10f, stats.getDisplayDropPercent(startNs + 8_400_000_000L), 0.5f);
        for (long t = 9_000_000_000L; t <= 13_000_000_000L; t += 1_000_000_000L) stats.expire(startNs + t);
        stats.expire(startNs + 13_500_000_000L);
        // The menu's window (6.5 to 8.5 s) had no drops, though the game's were noticed within 2 s
        // before it was read.
        assertEquals(0, stats.getDisplayDropPercent(startNs + 13_600_000_000L), 0);
        assertEquals(72, stats.getPresentDrops());
        stats.discardPending("codec_reset");
        assertEquals(-1, stats.getDisplayDropPercent(startNs + 13_400_000_000L), 0);
        assertEquals(72, stats.getPresentDrops());
    }

    private static void frame(FrameLatencyStats stats, int pts, long t, boolean rendered) {
        stats.onFrameReceived(t);
        stats.onDecoderInput(pts, pts, t, t, t, (char) 0);
        stats.onDecoderOutput(0, pts, t + 1_000_000L);
        stats.onOutputReleased(0, t + 2_000_000L, true, true);
        if (rendered) stats.onFrameRendered(pts, t + 5_000_000L);
    }

    @Test
    public void aFrameTheRendererReplacedCountsInTheWindowOfItsRelease() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long startNs = 1_000_000_000L;
        stats.onFrameReceived(startNs);
        for (int i = 1; i <= 240; i++) {
            long t = startNs + i * 16_666_667L;
            stats.onFrameReceived(t);
            stats.onDecoderInput(i, i, t, t, t, (char) 0);
            stats.onDecoderOutput(0, i, t + 1_000_000L);
            stats.onOutputReleased(0, t + 2_000_000L, true, true);
            // Three of the first two seconds' 120 frames never reach the screen.
            if (i <= 120 && i % 40 == 0) stats.onFrameReplaced(i);
            else stats.onFrameRendered(i, t + 5_000_000L);
        }
        stats.expire(startNs + 7_010_000_000L);
        assertEquals(2.5f, stats.getDisplayDropPercent(startNs + 7_010_000_000L), 0.001f);
        stats.expire(startNs + 9_010_000_000L);
        assertEquals(0, stats.getDisplayDropPercent(startNs + 9_010_000_000L), 0);
        assertEquals(3, stats.getPresentDrops());
    }

    @Test
    public void displayDropsAreNotMeasuredWithoutRenderCallbacks() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onFrameReceived(1_000_000_000L);
        stats.onDecoderInput(1, 1, 1_000_000_000L, 1_000_000_000L, 1_000_000_000L, (char) 0);
        stats.onDecoderOutput(0, 1, 1_004_000_000L);
        stats.onOutputReleased(0, 1_005_000_000L, true, false);
        assertEquals(-1, stats.getDisplayDropPercent(1_500_000_000L), 0);
    }

    @Test
    public void aReportedFrameIsNotADropWhenItsTimeoutPasses() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 123, 1000000, 1500000, 2000000, (char) 0);
        stats.onDecoderOutput(0, 123, 3000000);
        stats.onOutputReleased(0, 4000000, true, true);
        stats.onFrameRendered(123, 9000000, 9100000);
        stats.onFrameReplaced(123);
        stats.expire(10000000000L);
        assertEquals(0, stats.getPresentDrops());
    }

    @Test
    public void vulkanDisplayWaitEndsWhenThePresentIsQueuedAndScanoutIsOnlyLogged() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 123, 1000000, 15000000, 20000000, (char) 0);
        stats.onDecoderQueued(123, 20000000);
        stats.onDecoderOutput(0, 123, 22750000);
        stats.onOutputReleased(0, 23000000, true, true);
        // Queued 0.75 ms after decoding, on the screen 20.5 ms after decoding
        stats.onFrameShown(123, 23500000, 43250000, 44000000);

        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {1, 0.75, 0.75, 0.75}, summary[2], 0);
        String line = stats.takeScanoutLatency();
        assertTrue(line, line.contains("Scanout latency (decoded to on screen)") && line.contains("avg 20.5 ms") && line.contains("1 frames"));
        assertNull(stats.takeScanoutLatency());
    }

    @Test
    public void aFrameTheScreenCannotTimeStillHasItsDisplayWaitButNoScanoutLatency() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 123, 1000000, 15000000, 20000000, (char) 0);
        stats.onDecoderQueued(123, 20000000);
        stats.onDecoderOutput(0, 123, 22750000);
        stats.onOutputReleased(0, 23000000, true, true);
        stats.onFrameShown(123, 23500000, -1, 24000000);

        assertArrayEquals(new double[] {1, 0.75, 0.75, 0.75}, stats.summarize()[2], 0);
        assertNull(stats.takeScanoutLatency());
    }

}
