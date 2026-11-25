package com.limelight.binding.video;

import org.junit.Test;

import java.io.StringWriter;
import java.io.Writer;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;

import static com.limelight.binding.video.FrameLatencyStatsTest.timedOutput;
import static org.junit.Assert.*;

/**
 * Unit tests for {@link FrameLatencyStats} frame lifecycle tracking, Artemis decode accounting,
 * rendered FPS, expiry/overflow bounds, concurrent CSV I/O, and present jitter windows.
 */
public class FrameLatencyStatsLifecycleTest {
    @Test
    public void supportedThousandFpsStreamKeepsFramesUntilTheCallbackTimeout() {
        FrameLatencyStats stats = new FrameLatencyStats(true);
        for (int i = 1; i <= 8000; i++) {
            long now = 1000000000L + i * 1000000L;
            stats.onFrameReceived(now);
            stats.onDecoderInput(i, i, now, now, now, (char) 0);
            stats.onDecoderQueued(i, now);
            stats.onDecoderOutput(0, i, now);
            stats.onOutputReleased(0, now, true, true);
            if (i % 1000 == 0) stats.expire(now);
        }
        stats.expire(14000000000L);
        assertEquals(8000, stats.getPresentDrops());
    }

    @Test
    public void maximumStreamRateRetainsFramesUntilCallbackTimeout() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 0; i < 5000; i++) {
            long timeNs = 1000000000L + i * 1000000L;
            stats.onDecoderInput(i, i, timeNs, timeNs, timeNs, (char) 0);
            stats.onDecoderQueued(i, timeNs);
            stats.onDecoderOutput(0, i, timeNs);
            stats.onOutputReleased(0, timeNs, true, true);
        }
        stats.expire(11000000000L);
        assertEquals(5000, stats.getPresentDrops());
    }

    @Test
    public void timingSplitExcludesPacketAssemblyAndKeepsFractionalMilliseconds() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 123, 1000000, 15000000, 20000000, (char) 0);
        stats.onDecoderQueued(123, 20000000);
        stats.onDecoderOutput(0, 123, 22750000);
        stats.onOutputReleased(0, 23000000, true, true);
        stats.onFrameRendered(123, 27250000);

        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {1, 5, 5, 5}, summary[0], 0);
        assertArrayEquals(new double[] {1, 2.75, 2.75, 2.75}, summary[1], 0);
        assertArrayEquals(new double[] {1, 4.5, 4.5, 4.5}, summary[2], 0);
        // Client latency starts with the complete frame (15 ms), not its first packet (1 ms): queue wait,
        // decode and display wait together.
        assertEquals(5 + 2.75 + 4.5, summary[3][1], 0);
        assertEquals(2.75, stats.takeDecodeTimeMs(), 0);

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        assertEquals("19.000000", fields[8]);
        assertEquals("2.750000", fields[9]);
        assertEquals("4.500000", fields[10]);
        assertEquals("15000000", fields[24]);
        assertEquals("5.000000", fields[25]);
    }

    @Test
    public void drainedBatchCountsEveryOutputButOnlyObservedPresentations() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 4; i++) {
            stats.onFrameReceived(1000000);
            stats.onDecoderInput(i, i, 1000000, 2000000, 3000000, (char) 0);
            stats.onDecoderQueued(i, 3000000);
        }
        stats.onDecoderOutput(0, 1, 4000000);
        stats.onOutputReleased(0, 5000000, false, true);
        stats.onDecoderOutput(0, 2, 7000000);
        stats.onOutputReleased(0, 8000000, false, true);
        stats.onDecoderOutput(0, 3, 9000000);
        stats.onOutputReleased(0, 10000000, true, true);
        stats.onFrameRendered(3, 13000000);
        stats.discard(4, "output_unobserved");

        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {4, 1, 1, 1}, summary[0], 0);
        assertArrayEquals(new double[] {3, 11.0 / 3, 6, 6}, summary[1], 0.00001);
        assertArrayEquals(new double[] {1, 4, 4, 4}, summary[2], 0);
        assertEquals(11.0 / 3, stats.takeDecodeTimeMs(), 0.00001);
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
        assertEquals(3, stats.getAverageDecoderLatency());
        assertEquals(5, stats.getAverageEndToEndLatency());

        stats.discardPending("codec_reset");
        assertEquals(3, stats.getAverageDecoderLatency());
        stats.onDecoderInput(5, 5, 20000000, 21000000, 22000000, (char) 0);
        stats.onDecoderQueued(5, 22000000);
        stats.onDecoderOutput(0, 5, 31000000);
        assertEquals(9, stats.takeDecodeTimeMs(), 0);
        assertEquals(5, stats.getAverageDecoderLatency());
        assertEquals(7, stats.getAverageEndToEndLatency());
    }

    @Test
    public void artemisDecodeTimeRoundsEachCountedFrameDownToWholeMilliseconds() {
        FrameLatencyStats stats = new FrameLatencyStats();
        assertEquals(0, stats.takeArtemisDecodeMs());
        // 4.9 ms counts as 4, 2.999 ms as 2 and 0.6 ms as 0, as in Artemis.
        long[][] frames = {{1, 10000000, 14900000}, {2, 20000000, 22999000}, {3, 30000000, 30600000}};
        for (long[] frame : frames) {
            stats.onDecoderQueued(frame[0], frame[1]);
            stats.onDecoderOutput((int) frame[0], frame[0], frame[2]);
            stats.onArtemisDecodeCounted(frame[0]);
        }
        assertEquals(6, stats.getArtemisDecodeMs());
        assertEquals(6, stats.takeArtemisDecodeMs());
        assertEquals(0, stats.getArtemisDecodeMs());
        assertEquals(0, stats.takeArtemisDecodeMs());
    }

    @Test
    public void artemisDecodeTimeSkipsUncountedImpossibleAndSecondLongFrames() {
        FrameLatencyStats stats = new FrameLatencyStats();
        // Replaced before display: queued but never counted.
        stats.onDecoderQueued(1, 1000000);
        stats.onDecoderOutput(0, 1, 4000000);
        // Counted twice: only the first count has a start time.
        stats.onDecoderQueued(2, 2000000);
        stats.onDecoderOutput(0, 2, 5000000);
        stats.onArtemisDecodeCounted(2);
        stats.onArtemisDecodeCounted(2);
        // Never queued, never returned by the decoder, a second or longer, and a clock going backwards.
        stats.onDecoderOutput(0, 3, 9000000);
        stats.onArtemisDecodeCounted(3);
        stats.onDecoderQueued(9, 3000000);
        stats.onArtemisDecodeCounted(9);
        stats.onDecoderQueued(4, 10000000);
        stats.onDecoderOutput(0, 4, 1010000000);
        stats.onArtemisDecodeCounted(4);
        stats.onDecoderQueued(5, 20000000);
        stats.onDecoderOutput(0, 5, 18000000);
        stats.onArtemisDecodeCounted(5);
        stats.onDecoderQueued(8, 30000000);
        stats.onDecoderOutput(0, 8, 29999999);
        stats.onArtemisDecodeCounted(8);
        assertEquals(3, stats.takeArtemisDecodeMs());
        // A codec reset forgets queued frames, not the time already counted.
        stats.onDecoderQueued(6, 30000000);
        stats.onDecoderOutput(0, 6, 37000000);
        stats.onArtemisDecodeCounted(6);
        stats.onDecoderQueued(7, 40000000);
        stats.onDecoderOutput(0, 7, 45000000);
        stats.discardPending("codec_reset");
        stats.onArtemisDecodeCounted(7);
        assertEquals(7, stats.takeArtemisDecodeMs());
    }

    @Test
    public void artemisDecodeTimeStopsWhenTheDecoderReturnsTheFrameNotWhenItIsReleased() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long t0 = 10000000;
        stats.onDecoderInput(1, 1, t0 - 2000000, t0 - 1000000, t0 - 500000, (char) 0);
        stats.onDecoderQueued(1, t0);
        stats.onDecoderOutput(0, 1, t0 + 2400000);
        // Released and counted 5.9 ms after queueing: the 3.5 ms in between is pacing, not decode.
        stats.onOutputReleased(0, t0 + 5900000, true, false);
        stats.onArtemisDecodeCounted(1);
        assertEquals(2, stats.takeArtemisDecodeMs());
    }

    @Test
    public void artemisDecodeTimeUsesTheCountedCatchUpFramesOwnDequeueTime() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long t0 = 10000000;
        stats.onDecoderQueued(1, t0);
        stats.onDecoderQueued(2, t0 + 1000000);
        stats.onDecoderOutput(0, 1, t0 + 2400000);
        stats.onDecoderOutput(1, 2, t0 + 7100000);
        stats.onOutputReleased(0, t0 + 7200000, false, false);
        // Frame 1 was dropped by the catch-up drain; frame 2 is the one counted, 6.1 ms after
        // its own queueing, whatever time the release and bookkeeping then took.
        stats.onOutputReleased(1, t0 + 12000000, true, false);
        stats.onArtemisDecodeCounted(2);
        assertEquals(6, stats.takeArtemisDecodeMs());
    }

    @Test
    public void artemisDecodeTimeKeepsAnOutputThatBeatsTheQueueBookkeeping() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long t0 = 10000000;
        stats.onDecoderInput(1, 1, t0 - 2000000, t0 - 1000000, t0 - 500000, (char) 0);
        stats.onDecoderOutput(0, 1, t0 + 3300000);
        stats.onDecoderQueued(1, t0);
        stats.onArtemisDecodeCounted(1);
        assertEquals(3, stats.takeArtemisDecodeMs());
    }

    @Test
    public void artemisDecodeTimeLeavesThePreciseDecodeTimeAlone() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1000000, 1000000, 2000000, (char) 0);
        stats.onDecoderQueued(1, 2000000);
        stats.onDecoderOutput(0, 1, 6900000);
        stats.onArtemisDecodeCounted(1);
        assertEquals(4.9, stats.takeDecodeTimeMs(), 0.00001);
        assertEquals(4, stats.takeArtemisDecodeMs());
    }

    @Test
    public void renderedFpsCountsSubmittedFramesWithoutRenderCallbacks() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long startNs = 1000000000L;
        for (int i = 1; i <= 5; i++) {
            long base = startNs + i * 100000000L;
            stats.onFrameReceived(base);
            stats.onDecoderInput(i, i, base, base, base + 1000000, (char) 0);
            stats.onDecoderQueued(i, base + 1000000);
            stats.onDecoderOutput(i, i, base + 3000000);
        }
        // Frames 1 to 3 are released for rendering, 4 and 5 are dropped; no render callback arrives.
        for (int i = 1; i <= 3; i++) {
            stats.onOutputReleased(i, startNs + i * 100000000L + 4000000, true, false);
        }
        stats.onOutputReleased(4, startNs + 400000000L + 4000000, false, false);
        stats.onOutputReleased(5, startNs + 500000000L + 4000000, false, false);
        float[] rates = stats.getFrameRates(startNs + 1100000000L);
        // The window starts at the first received frame, which its own start excludes from the
        // received count; the release stamps all fall inside it.
        assertEquals(4, rates[0], 0.001f);
        assertEquals(3, rates[1], 0.001f);
        // Compositor-confirmed rate stays unavailable without callbacks.
        assertEquals(-1, rates[2], 0);
    }

    @Test
    public void renderedFpsTakesBackAFrameWhoseReleaseThrew() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long startNs = 1000000000L;
        for (int i = 1; i <= 3; i++) {
            long base = startNs + i * 100000000L;
            stats.onFrameReceived(base);
            stats.onDecoderInput(i, i, base, base, base + 1000000, (char) 0);
            stats.onDecoderQueued(i, base + 1000000);
            stats.onDecoderOutput(i, i, base + 3000000);
            long releaseNs = base + 4000000;
            stats.onOutputReleased(i, releaseNs, true, true);
            // releaseOutputBuffer() throws for frame 2 after it was counted as submitted.
            if (i == 2) stats.onReleaseFailed(releaseNs);
        }
        assertEquals(2, stats.getFrameRates(startNs + 1100000000L)[1], 0.001f);
        // A stale or repeated notification changes nothing.
        stats.onReleaseFailed(12345);
        stats.onReleaseFailed(startNs + 200000000L + 4000000);
        assertEquals(2, stats.getFrameRates(startNs + 1100000000L)[1], 0.001f);
    }

    @Test
    public void absentAndInvalidQueueTimestampsDoNotInventQueueWait() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (long enqueueNs : new long[] {0, 4000000}) {
            stats.onDecoderInput(1, 1, 1000000, enqueueNs, 3000000, (char) 0);
            stats.onDecoderQueued(1, 3000000);
            stats.onDecoderOutput(0, 1, 4000000);
            stats.onOutputReleased(0, 5000000, false, true);
        }
        assertEquals(0, stats.summarize()[0][0], 0);
        assertEquals(2, stats.summarize()[1][0], 0);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        for (String row : csv.toString().split("\n")) {
            assertEquals("", row.split(",", -1)[25]);
        }
    }

    @Test
    public void bitrateDecodeSamplesRequireCompletedValidFramesAndResetEachWindow() {
        FrameLatencyStats stats = new FrameLatencyStats();
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
        stats.onDecoderInput(1, 1000, 1000000, 1000000, 2000000, (char) 0);
        stats.onDecoderQueued(1000, 2000000);
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
        stats.onDecoderOutput(1, 1000, 6000000);
        stats.onDecoderInput(2, 2000, 7000000, 7000000, 8000000, (char) 0);
        stats.onDecoderQueued(2000, 8000000);
        stats.onDecoderOutput(2, 2000, 14000000);
        assertEquals(5, stats.takeDecodeTimeMs(), 0);
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
        stats.onDecoderInput(3, 3000, 15000000, 15000000, 16000000, (char) 0);
        stats.onDecoderQueued(3000, 16000000);
        stats.onDecoderOutput(3, 3000, 14000000);
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
        stats.onDecoderInput(4, 4000, 20000000, 20000000, 21000000, (char) 0);
        stats.onDecoderQueued(4000, 21000000);
        stats.onDecoderOutput(4, 4000, 22000000);
        stats.discardPending("stopped");
        assertEquals(-1, stats.takeDecodeTimeMs(), 0);
    }

    @Test
    public void reusedOutputIndexKeepsBothPendingRenderCallbacks() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 10, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(10, 200);
        stats.onDecoderOutput(0, 10, 300);
        stats.onOutputReleased(0, 400, true, true);
        stats.onDecoderInput(2, 20, 500, 500, 600, (char) 0);
        stats.onDecoderQueued(20, 600);
        stats.onDecoderOutput(0, 20, 700);
        stats.onFrameRendered(10, 450);
        stats.onOutputReleased(0, 800, true, true);
        stats.onFrameRendered(20, 900);

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains("1,10,100,200,300,450,400,rendered,"));
        assertTrue(csv.toString().contains("2,20,500,600,700,900,800,rendered,"));
        assertEquals(2, stats.summarize()[3][0], 0);
    }

    @Test
    public void lateCallbacksAfterCodecResetCannotCompleteNewBuffers() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 10, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(10, 200);
        stats.onDecoderOutput(0, 10, 300);
        stats.discardPending("codec_reset");
        stats.onDecoderInput(2, 20, 500, 500, 600, (char) 0);
        stats.onDecoderQueued(20, 600);
        stats.onDecoderOutput(0, 20, 700);
        stats.onFrameRendered(10, 800);
        stats.onOutputReleased(0, 900, false, true);
        stats.onFrameRendered(20, 1000);

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains("1,10,100,200,300,,,codec_reset,"));
        assertTrue(csv.toString().contains("2,20,500,600,700,,900,dropped,"));
        assertEquals(2, csv.toString().split("\n").length);
        assertEquals(0, stats.summarize()[3][0], 0);
    }

    @Test
    public void absentCallbacksExpireWithoutInventingRenderMeasurements() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 10, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(10, 200);
        stats.onDecoderOutput(0, 10, 300);
        stats.onOutputReleased(0, 400, true, true);
        stats.onDecoderInput(2, 20, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(20, 200);
        stats.expire(5000000400L);
        stats.onFrameRendered(10, 5000000300L);

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains("render_unobserved"));
        assertTrue(csv.toString().contains("output_unobserved"));
        assertEquals(2, csv.toString().split("\n").length);
        assertEquals(0, stats.summarize()[3][0], 0);
    }

    @Test
    public void invalidClockOrderIsExcludedFromPercentiles() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 10, 300, 300, 200, (char) 0);
        stats.onDecoderQueued(10, 200);
        stats.onDecoderOutput(0, 10, 100);
        stats.onOutputReleased(0, 400, true, true);
        stats.onFrameRendered(10, 500);
        for (int stage = 0; stage < 4; stage++) {
            assertEquals(0, stats.summarize()[stage][0], 0);
        }
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains("invalid_render_time"));
    }

    @Test
    public void stalledDecoderAndCsvWriterKeepTrackingBounded() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 0; i < 15000; i++) {
            stats.onDecoderInput(i, i, 100, 100, 200, (char) 0);
            stats.onDecoderQueued(i, 200);
        }
        StringWriter overflowCsv = new StringWriter();
        stats.writeCsv(overflowCsv);
        assertEquals(4096, overflowCsv.toString().split("\n").length);
        assertTrue(overflowCsv.toString().contains("tracking_overflow"));
        stats.discardPending("stream_ended");
        assertEquals(15000 - 2 * 4096, stats.getCsvRowsLost());
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals(4096, csv.toString().split("\n").length);
        assertTrue(csv.toString().contains("stream_ended"));
    }

    @Test
    public void blockedCsvIoDoesNotBlockDecoderCallbacks() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 10, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(10, 200);
        stats.discardPending("stream_ended");
        CountDownLatch writing = new CountDownLatch(1);
        CountDownLatch resume = new CountDownLatch(1);
        ExecutorService executor = Executors.newFixedThreadPool(2);
        try {
            Future<?> export = executor.submit(() -> {
                stats.writeCsv(new Writer() {
                    @Override
                    public void write(char[] data, int offset, int length) {
                        writing.countDown();
                        try {
                            assertTrue(resume.await(5, TimeUnit.SECONDS));
                        } catch (InterruptedException e) {
                            throw new AssertionError(e);
                        }
                    }

                    @Override
                    public void flush() { }

                    @Override
                    public void close() { }
                });
                return null;
            });
            assertTrue(writing.await(5, TimeUnit.SECONDS));
            executor.submit(() -> {
                stats.onDecoderInput(2, 20, 500, 500, 600, (char) 0);
                stats.onDecoderQueued(20, 600);
                stats.onDecoderOutput(0, 20, 700);
                stats.onOutputReleased(0, 800, true, true);
                stats.onFrameRendered(20, 900);
            }).get(5, TimeUnit.SECONDS);
            resume.countDown();
            export.get(5, TimeUnit.SECONDS);
            assertEquals(1, stats.summarize()[3][0], 0);
        } finally {
            resume.countDown();
            executor.shutdownNow();
        }
    }

    @Test
    public void completedGpuFenceDoesNotInventDisplayTiming() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(9, 9, 1000000, 1000000, 2000000, (char) 0);
        stats.onDecoderQueued(9, 2000000);
        stats.onDecoderOutput(0, 9, 3500000);
        stats.onOutputReleased(0, 6000000, true, false);
        assertArrayEquals(new double[] {1, 1.5, 1.5, 1.5}, stats.summarize()[1], 0.00001);
        assertEquals(0, stats.summarize()[2][0], 0);
        assertEquals(0, stats.summarize()[3][0], 0);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().contains("render_unavailable"));
    }


    // Decoded and released for display, waiting for its render or present report.
    private static void releasedForDisplay(FrameLatencyStats stats, int number, long outputNs) {
        long inputNs = outputNs - 3_000_000L;
        stats.onDecoderInput(number, number, inputNs, inputNs, inputNs, (char) 0);
        stats.onDecoderQueued(number, inputNs);
        stats.onDecoderOutput(0, number, outputNs);
        stats.onOutputReleased(0, outputNs, true, true);
    }

    @Test public void presentJitterCountsRenderedFramesOnlyWhileTheOverlayShowsIt() {
        long vsync = 16_666_667L;
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 10; i++) timedOutput(stats, i, 1_000_000_000L + i * vsync, 5_000_000L, 0, 2_000_000L);
        stats.setPresentJitterEnabled(true);
        assertNull(stats.presentJitter(vsync));
        for (int i = 11; i <= 31; i++) {
            // Frame 20 is held for an extra vsync.
            long outputNs = 1_000_000_000L + i * vsync + (i >= 20 ? vsync : 0);
            timedOutput(stats, i, outputNs, 5_000_000L, 0, 2_000_000L);
        }
        PresentJitter.Snapshot window = stats.presentJitter(vsync);
        assertEquals(20, window.count);
        assertEquals(19, window.histogram[1]);
        assertEquals(1, window.histogram[2]);

        // A render time before the decoder's output is not a present.
        releasedForDisplay(stats, 32, 1_000_000_000L + 33 * vsync);
        stats.onFrameRendered(32, 1_000_000_000L);
        assertEquals(20, stats.presentJitter(vsync).count);

        stats.setPresentJitterEnabled(false);
        assertNull(stats.presentJitter(vsync));
    }

    @Test public void presentJitterTakesReleaseTimesWithoutRenderCallbacks() {
        // The GL upscaler and Android 5 never report a render: the release is the closest time.
        long vsync = 16_666_667L;
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.setPresentJitterEnabled(true);
        for (int i = 1; i <= 11; i++) {
            long outputNs = 1_000_000_000L + i * vsync;
            stats.onDecoderInput(i, i, outputNs - 3_000_000L, outputNs - 3_000_000L, outputNs - 3_000_000L, (char) 0);
            stats.onDecoderQueued(i, outputNs - 3_000_000L);
            stats.onDecoderOutput(0, i, outputNs);
            stats.onOutputReleased(0, outputNs + 1_000_000L, true, false);
        }
        // A dropped frame is no present either way.
        stats.onDecoderInput(12, 12, 2_000_000_000L, 2_000_000_000L, 2_000_000_000L, (char) 0);
        stats.onDecoderQueued(12, 2_000_000_000L);
        stats.onDecoderOutput(0, 12, 2_000_000_000L);
        stats.onOutputReleased(0, 1_000_000_000L + 12 * vsync, false, false);
        PresentJitter.Snapshot window = stats.presentJitter(vsync);
        assertEquals(10, window.count);
        assertEquals(10, window.histogram[1]);
        assertTrue(window.releaseTime);

        // Render reports replace the release times; the window starts over with them.
        for (int i = 13; i <= 15; i++) {
            long outputNs = 1_000_000_000L + i * vsync;
            releasedForDisplay(stats, i, outputNs);
            stats.onFrameRendered(i, outputNs + 2_000_000L);
        }
        window = stats.presentJitter(vsync);
        assertEquals(2, window.count);
        assertFalse(window.releaseTime);
    }

    @Test public void presentJitterTakesVulkanQueueTimesAndForgetsTheLastPresentOnReset() {
        long vsync = 8_333_333L;
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.setPresentJitterEnabled(true);
        for (int i = 1; i <= 5; i++) {
            long outputNs = 1_000_000_000L + i * vsync;
            releasedForDisplay(stats, i, outputNs);
            stats.onFrameShown(i, outputNs + 1_000_000L, outputNs + 4_000_000L, outputNs + 5_000_000L);
        }
        assertEquals(4, stats.presentJitter(vsync).count);
        assertEquals(1, stats.presentJitter(vsync).modeUnits);

        stats.discardPending("codec_reset");
        assertNull(stats.presentJitter(vsync));
        // The first frame after the reset only starts the window again.
        timedOutput(stats, 6, 2_000_000_000L, 3_000_000L, 0, 1_000_000L);
        assertNull(stats.presentJitter(vsync));
    }
}
