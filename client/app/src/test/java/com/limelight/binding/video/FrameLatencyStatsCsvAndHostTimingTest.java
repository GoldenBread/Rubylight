package com.limelight.binding.video;

import com.limelight.nvstream.av.video.HostTimingReport;

import org.junit.Test;

import java.io.StringWriter;

import static org.junit.Assert.*;

/**
 * Unit tests for {@link FrameLatencyStats} host timing reports, VRR CSV intervals,
 * CSV column compatibility (present, host record, and scheduler columns), and clock-offset locking.
 */
public class FrameLatencyStatsCsvAndHostTimingTest {
    @Test
    public void hostDurationIsIndependentOfClientClockAndSurvivesCsvExport() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(42, 123, 1000000, 1000000, 2000000, (char) 123);
        stats.onDecoderQueued(123, 2000000);
        stats.onDecoderOutput(0, 123, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(123, 7000000);
        assertEquals(6.0, stats.summarize()[3][1], 0.00001);
        assertArrayEquals(new double[] {1, 12.3, 12.3, 12.3}, stats.summarize()[4], 0.00001);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals("12.300000", csv.toString().trim().split(",", -1)[13]);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, csv.toString().trim().split(",", -1).length);
    }

    private static HostTimingReport report(long[]... records) {
        byte[] data = new byte[4 + 20 * records.length];
        data[0] = 1;
        data[1] = (byte) records.length;
        for (int i = 0; i < records.length; i++) {
            for (int field = 0; field < 5; field++) {
                for (int b = 0; b < 4; b++) data[4 + 20 * i + 4 * field + b] = (byte) (records[i][field] >> (8 * b));
            }
        }
        return HostTimingReport.parse(data);
    }

    private static void renderFrame(FrameLatencyStats stats, int number, long ptsUs, char headerLatency) {
        stats.onDecoderInput(number, ptsUs, 1000000, 1000000, 2000000, headerLatency);
        stats.onDecoderQueued(ptsUs, 2000000);
        stats.onDecoderOutput(0, ptsUs, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(ptsUs, 7000000);
    }

    @Test
    public void hostTimingReportsFillTheirOwnStagesAndTheCsvColumns() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        // The report can arrive before or after its frame; the header says 5 ms (50 x 100 us).
        stats.onHostTiming(report(new long[] {42, 12000, 3000, 5000, 2500}), 1000000000L);
        renderFrame(stats, 42, 123, (char) 50);
        stats.onDecoderInput(43, 124, 1000000, 1000000, 2000000, (char) 50);
        stats.onHostTiming(report(new long[] {43, 8000, 1000, 2000, 3000}), 1000000100L);

        double[][] summary = stats.summarize();
        // Host processing (stage 4) is the two header values, 5 ms each; the reports' present to
        // last packet sent is stage 8.
        assertArrayEquals(new double[] {2, 5, 5, 5}, summary[4], 0);
        assertArrayEquals(new double[] {2, 10, 12, 12}, summary[FrameLatencyStats.HOST_SENT_STAGE], 0);
        assertEquals(2, summary[5][1], 0);
        assertEquals(3.5, summary[6][1], 0);
        assertEquals(2.75, summary[7][1], 0);

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().trim().split("\n");
        assertEquals(1, rows.length);
        String[] fields = rows[0].split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        assertEquals("5.000000", fields[13]);
        assertEquals("12.000000", fields[26]);
        assertEquals("3.000000", fields[27]);
        assertEquals("5.000000", fields[28]);
        assertEquals("2.500000", fields[29]);

        // A frame finished before its report arrived is still matched.
        stats.onDecoderOutput(1, 124, 5000000);
        stats.onOutputReleased(1, 6000000, true, true);
        stats.onFrameRendered(124, 7000000);
        csv = new StringWriter();
        stats.writeCsv(csv);
        fields = csv.toString().trim().split(",", -1);
        assertEquals("43", fields[0]);
        assertEquals("8.000000", fields[26]);
        assertEquals("3.000000", fields[29]);
    }

    @Test
    public void hostProcessingStaysTheHeaderValueWhateverTheReportsSay() {
        // Regression: host timing reports (present to last packet sent, with the network send) once
        // replaced the header's host processing in the overlay, so Nightjar showed 4.9 ms where
        // Artemis showed the header's 0.5 ms.
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1000000, 1000000, 2000000, (char) 5);
        stats.onHostTiming(report(new long[] {1, 5000, 100, 600, 4200}), 3000000000L);
        // Reports are fresh, and the header keeps counting.
        stats.onDecoderInput(2, 2, 3100000000L, 3100000000L, 3100000000L, (char) 5);
        double[][] summary = stats.summarize();
        assertArrayEquals(new double[] {2, 0.5, 0.5, 0.5}, summary[4], 0.00001);
        assertArrayEquals(new double[] {1, 5, 5, 5}, summary[FrameLatencyStats.HOST_SENT_STAGE], 0);
        // Long after the last report the header's average is still the same stage.
        stats.onDecoderInput(3, 3, 9000000000L, 9000000000L, 9000000000L, (char) 5);
        assertArrayEquals(new double[] {3, 0.5, 0.5, 0.5}, stats.summarize()[4], 0.00001);
    }

    @Test
    public void reportsAloneLeaveHostProcessingEmpty() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onHostTiming(report(new long[] {1, 5000, 100, 600, 4200}), 3000000000L);
        assertEquals(0, stats.summarize()[4][0], 0);
        assertEquals(1, stats.summarize()[FrameLatencyStats.HOST_SENT_STAGE][0], 0);
    }

    @Test
    public void unmatchedReportsStayBoundedAndHeaderOnlyStreamsHaveBlankSplitColumns() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 0; i < 1000; i++) {
            stats.onHostTiming(report(new long[] {i, 10, 1, 1, 1}), 1000L + i);
        }
        // The oldest were dropped, so frame 0 gets nothing but frame 999 does.
        renderFrame(stats, 0, 1, (char) 0);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals("", fields[26]);
        assertEquals("", fields[29]);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);

        renderFrame(stats, 999, 2, (char) 0);
        csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals("0.010000", csv.toString().trim().split(",", -1)[26]);
    }

    @Test
    public void missingHostTimingIsBlankNotZeroLatency() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(1, 200);
        stats.discardPending("stopped");
        assertEquals(0, stats.summarize()[4][0], 0);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals("", csv.toString().trim().split(",", -1)[13]);
    }

    @Test
    public void wireTimingIsUnsignedAndWindowIsBounded() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 100, 100, 200, (char) 65535);
        stats.onDecoderQueued(1, 200);
        assertEquals(6553.5, stats.summarize()[4][1], 0.00001);
        stats.discardPending("stopped");
        for (int i = 0; i < FrameLatencyStats.WINDOW_SIZE; i++) {
            stats.onDecoderInput(i, i, 100, 100, 200, (char) 10);
            stats.onDecoderQueued(i, 200);
            stats.discardPending("stopped");
        }
        assertArrayEquals(new double[] {600, 1, 1, 1}, stats.summarize()[4], 0.00001);
    }

    @Test
    public void hostPercentilesUseObservedSamplesOnly() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 0; i <= 100; i++) {
            stats.onDecoderInput(i, i, 100, 100, 200, (char) i);
            stats.discardPending("stopped");
        }
        assertArrayEquals(new double[] {100, 5.05, 9.5, 9.9}, stats.summarize()[4], 0.00001);
    }

    @Test
    public void presentColumnsAreAppendedWithoutChangingExistingColumnOrder() throws Exception {
        assertEquals("frame_number,pts_us,receive_ns,decoder_input_ns,decoder_output_ns," +
                "render_ns,release_ns,status,receive_to_input_ms,input_to_output_ms,output_to_render_ms," +
                "receive_to_render_ms,csv_rows_lost,host_processing_ms,decode_to_present_ms,receive_to_present_ms," +
                "decode_to_release_ms,decode_interval_ms,release_interval_ms," +
                "fps_sample_ns,received_fps,released_fps,shown_fps,present_drops,enqueue_ns,queue_wait_ms," +
                "host_present_to_sent_ms,host_queue_ms,host_encode_ms,host_send_ms," +
                "callback_received_ns,callback_delay_ms," +
                "rtp_host_us,host_present_us,host_acquired_us,host_submitted_us,host_encoded_us," +
                "host_first_sent_us,host_last_sent_us,clock_offset_us,clock_rtt_us",
                FrameLatencyStats.CSV_HEADER.substring(0, FrameLatencyStats.CSV_HEADER.indexOf(",sched_vsync_ns")));
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(42, 123, 1000000, 1000000, 2000000, (char) 123);
        stats.onDecoderQueued(123, 2000000);
        stats.onDecoderOutput(0, 123, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(123, 9000000);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertTrue(csv.toString().startsWith("42,123,1000000,2000000,5000000,9000000,6000000,rendered," +
                "1.000000,3.000000,4.000000,8.000000,0,12.300000,4.000000,8.000000,,,,"));
        assertEquals(41 + SCHEDULER_COLUMNS, csv.toString().trim().split(",", -1).length);
    }

    @Test
    public void batchedOutOfOrderCallbacksUseFrameTimestampsOnce() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 2; i++) {
            long start = i * 10000000L;
            stats.onDecoderInput(i, i, start, start, start + 1000000, (char) 0);
            stats.onDecoderQueued(i, start + 1000000);
            stats.onDecoderOutput(0, i, start + 3000000);
            stats.onOutputReleased(0, start + 4000000, true, true);
        }
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals("", csv.toString());
        stats.onFrameRendered(2, 29000000, 100000000);
        stats.onFrameRendered(1, 15000000, 100000000);
        stats.onFrameRendered(2, 99000000, 190000000);
        assertArrayEquals(new double[] {2, 4, 6, 6}, stats.summarize()[2], 0.00001);
        assertArrayEquals(new double[] {2, 7, 9, 9}, stats.summarize()[3], 0.00001);
        stats.writeCsv(csv);
        String[] rows = csv.toString().split("\n");
        assertEquals(2, rows.length);
        assertTrue(rows[0].contains(",6.000000,9.000000,,,,"));
        assertTrue(rows[1].contains(",2.000000,5.000000,,,,"));
        assertEquals("100000000", rows[0].split(",", -1)[30]);
        assertEquals("71.000000", rows[0].split(",", -1)[31]);
        assertEquals("85.000000", rows[1].split(",", -1)[31]);
    }

    @Test
    public void unavailableOrReversedCallbackClocksDoNotInventDeliveryDelays() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 2; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i, 2000000);
            stats.onDecoderOutput(0, i, 3000000);
            stats.onOutputReleased(0, 4000000, true, true);
            stats.onFrameRendered(i, 5000000, i == 1 ? 0 : 4999999);
        }
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().trim().split("\n");
        assertEquals("", rows[0].split(",", -1)[30]);
        for (String row : rows) {
            assertEquals("", row.split(",", -1)[31]);
        }
        assertEquals(2, stats.summarize()[2][1], 0.00001);
    }

    @Test
    public void unobservedDroppedAndUnsupportedFramesLeavePresentColumnsBlank() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 3; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i, 2000000);
            stats.onDecoderOutput(0, i, 3000000);
            stats.onOutputReleased(0, 4000000, i != 2, i != 3);
        }
        stats.expire(6000000000L);
        stats.onFrameRendered(1, 5000000);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().split("\n");
        assertEquals(3, rows.length);
        for (String row : rows) {
            String[] fields = row.split(",", -1);
            assertEquals(41 + SCHEDULER_COLUMNS, fields.length);
            for (int column : new int[] {5, 10, 11, 14, 15}) assertEquals("", fields[column]);
        }
        assertEquals(0, stats.summarize()[2][0], 0);
        assertEquals(0, stats.summarize()[3][0], 0);
    }

    @Test
    public void invalidRenderTimesLeavePresentDurationsBlank() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 3; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i, 2000000);
            if (i != 3) stats.onDecoderOutput(0, i, i == 1 ? 1500000 : 3000000);
            stats.onOutputReleased(0, 4000000, true, true);
            stats.onFrameRendered(i, i == 2 ? 2500000 : 5000000);
        }
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        for (String row : csv.toString().split("\n")) {
            String[] fields = row.split(",", -1);
            assertEquals("invalid_render_time", fields[7]);
            for (int column : new int[] {10, 11, 14, 15}) assertEquals("", fields[column]);
        }
        assertEquals(0, stats.summarize()[2][0], 0);
        assertEquals(0, stats.summarize()[3][0], 0);
    }

    @Test
    public void presentPercentilesUseOnlyObservedFramesAndRollOver() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 100; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i, 2000000);
            stats.onDecoderOutput(0, i, 3000000);
            stats.onOutputReleased(0, 3000000, true, true);
            stats.onFrameRendered(i, 3000000 + i * 1000000L);
            stats.onDecoderInput(i + 100, i + 100, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i + 100, 2000000);
            stats.discard(i + 100, "dropped");
        }
        assertArrayEquals(new double[] {100, 50.5, 95, 99}, stats.summarize()[2], 0.00001);
        assertArrayEquals(new double[] {100, 52.5, 97, 101}, stats.summarize()[3], 0.00001);
        for (int i = 0; i < FrameLatencyStats.WINDOW_SIZE; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0);
            stats.onDecoderQueued(i, 2000000);
            stats.onDecoderOutput(0, i, 3000000);
            stats.onOutputReleased(0, 3000000, true, true);
            stats.onFrameRendered(i, 3000000);
        }
        assertArrayEquals(new double[] {600, 0, 0, 0}, stats.summarize()[2], 0.00001);
        assertArrayEquals(new double[] {600, 2, 2, 2}, stats.summarize()[3], 0.00001);
    }

    @Test
    public void vrrCsvIntervalsUseDecoderOutputEvenWithoutOrWithReorderedPresentCallbacks() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats(true);
        for (int i = 1; i <= 3; i++) {
            long start = i * 20000000L;
            stats.onDecoderInput(i, i, start, start, start + 1000000, (char) 0);
            stats.onDecoderQueued(i, start + 1000000);
            stats.onDecoderOutput(0, i, start + 3000000);
            stats.onOutputReleased(0, start + 4000000, true, i != 3);
        }
        stats.onFrameRendered(2, 49000000);
        stats.onFrameRendered(1, 29000000);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().split("\n");
        assertEquals(3, rows.length);
        assertTrue(rows[0].contains("render_unavailable"));
        assertTrue(rows[0].contains(",1.000000,20.000000,20.000000,"));
        assertTrue(rows[1].contains(",1.000000,20.000000,20.000000,"));
        assertTrue(rows[2].contains(",1.000000,,,"));
    }

    @Test
    public void vrrCsvIntervalsIncludeOutputsDroppedBeforeReleaseAndResetStartsANewSeries() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats(true);
        for (int i = 1; i <= 3; i++) {
            long start = i * 10000000L;
            stats.onDecoderInput(i, i, start, start, start + 1000000, (char) 0);
            stats.onDecoderQueued(i, start + 1000000);
            stats.onDecoderOutput(0, i, start + 2000000);
            stats.onOutputReleased(0, start + 3000000, i != 2, false);
        }
        stats.discardPending("codec_reset");
        stats.onDecoderInput(4, 4, 40000000, 40000000, 41000000, (char) 0);
        stats.onDecoderQueued(4, 41000000);
        stats.onDecoderOutput(0, 4, 42000000);
        stats.onOutputReleased(0, 43000000, true, false);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().split("\n");
        assertTrue(rows[1].contains("dropped"));
        assertTrue(rows[1].contains(",1.000000,10.000000,,"));
        assertTrue(rows[2].contains(",1.000000,10.000000,20.000000,"));
        assertTrue(rows[3].contains(",1.000000,,,"));
    }

    @Test
    public void vrrInvalidReleaseTimesAndUnreleasedFramesDoNotInventIntervals() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats(true);
        stats.onDecoderInput(1, 1, 100, 100, 200, (char) 0);
        stats.onDecoderQueued(1, 200);
        stats.onDecoderOutput(0, 1, 300);
        stats.onOutputReleased(0, 250, true, false);
        stats.onDecoderInput(2, 2, 400, 400, 500, (char) 0);
        stats.onDecoderQueued(2, 500);
        stats.discardPending("codec_reset");
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        for (String row : csv.toString().split("\n")) {
            String[] fields = row.split(",", -1);
            for (int column : new int[] {16, 17, 18}) assertEquals("", fields[column]);
        }
    }

    @Test
    public void halfRatePresentationIsVisibleInFpsAndPresentDropCount() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats(true);
        long startNs = 1000000000L;
        stats.onFrameReceived(startNs);
        for (int i = 1; i <= 240; i++) {
            long receiveNs = startNs + i * 8333333L;
            stats.onFrameReceived(receiveNs);
            stats.onDecoderInput(i, i, receiveNs, receiveNs, receiveNs, (char) 0);
            stats.onDecoderQueued(i, receiveNs);
            stats.onDecoderOutput(0, i, receiveNs);
            stats.onOutputReleased(0, receiveNs, true, true);
        }
        // Deliver one batch backwards to ensure callback delivery time and ordering do not determine FPS.
        for (int i = 240; i >= 2; i -= 2) {
            stats.onFrameRendered(i, startNs + i * 8333333L);
            stats.onFrameRendered(i, startNs + i * 8333333L);
        }
        assertArrayEquals(new float[] {120, 120, 60}, stats.getFrameRates(startNs + 2000000000L), 0.001f);
        assertEquals(0, stats.getPresentDrops());
        stats.expire(startNs + 7000000000L);
        assertEquals(120, stats.getPresentDrops());
        stats.expire(startNs + 8000000000L);
        assertEquals(120, stats.getPresentDrops());
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        int unobserved = 0;
        for (String row : csv.toString().split("\n")) {
            String[] fields = row.split(",", -1);
            assertEquals(41 + SCHEDULER_COLUMNS, fields.length);
            assertTrue(Long.parseLong(fields[19]) > 0);
            assertEquals("120", fields[23]);
            if (fields[7].equals("render_unobserved")) unobserved++;
        }
        assertEquals(120, unobserved);
        assertArrayEquals(new float[] {0, 0, 0}, stats.getFrameRates(startNs + 8000000000L), 0);
    }

    @Test
    public void delayedCallbacksPreRenderDropsAndResetsDoNotCountAsPresentDrops() {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 4; i++) {
            stats.onFrameReceived(i * 1000000L);
            stats.onDecoderInput(i, i, i * 1000000L, i * 1000000L, i * 1000000L, (char) 0);
            stats.onDecoderQueued(i, i * 1000000L);
            stats.onDecoderOutput(0, i, i * 1000000L);
            stats.onOutputReleased(0, 1000000000L, i != 2, i != 3);
        }
        stats.expire(5999999999L);
        assertEquals(0, stats.getPresentDrops());
        stats.onFrameRendered(1, 2000000000L);
        stats.discardPending("codec_reset");
        stats.expire(9000000000L);
        assertEquals(0, stats.getPresentDrops());
    }

    @Test
    public void missingRenderCallbackSupportDoesNotClaimShownFrames() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onFrameReceived(1000000);
        stats.onDecoderInput(1, 1, 1000000, 1000000, 2000000, (char) 0);
        stats.onDecoderQueued(1, 2000000);
        stats.onDecoderOutput(0, 1, 3000000);
        stats.onOutputReleased(0, 4000000, true, false);
        assertEquals(-1, stats.getFrameRates(5000000)[2], 0);
        stats.expire(6000000000L);
        assertEquals(0, stats.getPresentDrops());
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        assertEquals("", csv.toString().trim().split(",", -1)[22]);
    }

    // The receiver's columns before the host timing record's nine were appended.
    private static final String HEADER_BEFORE_THE_HOST_RECORD =
            "frame_number,pts_us,receive_ns,decoder_input_ns,decoder_output_ns," +
            "render_ns,release_ns,status,receive_to_input_ms,input_to_output_ms,output_to_render_ms," +
            "receive_to_render_ms,csv_rows_lost,host_processing_ms," +
            "decode_to_present_ms,receive_to_present_ms," +
            "decode_to_release_ms,decode_interval_ms,release_interval_ms," +
            "fps_sample_ns,received_fps,released_fps,shown_fps,present_drops,enqueue_ns,queue_wait_ms," +
            "host_present_to_sent_ms,host_queue_ms,host_encode_ms,host_send_ms," +
            "callback_received_ns,callback_delay_ms";
    private static final int HOST_RECORD_FIRST_COLUMN = 32;
    private static final int SCHEDULER_COLUMNS = 7;

    // An extended report: {frame, host, queue, encode, send, presentedAt, capture, convert}.
    private static HostTimingReport extendedReport(long[] r) {
        byte[] data = new byte[4 + 20 + 16];
        data[0] = 1;
        data[1] = 1;
        data[2] = 16;
        for (int field = 0; field < 5; field++) {
            for (int b = 0; b < 4; b++) data[4 + 4 * field + b] = (byte) (r[field] >> (8 * b));
        }
        for (int b = 0; b < 8; b++) data[24 + b] = (byte) (r[5] >> (8 * b));
        for (int b = 0; b < 4; b++) data[32 + b] = (byte) (r[6] >> (8 * b));
        for (int b = 0; b < 4; b++) data[36 + b] = (byte) (r[7] >> (8 * b));
        return HostTimingReport.parse(data);
    }

    @Test
    public void theHostRecordColumnsFollowTheReceiversColumnsUnchanged() {
        String header = FrameLatencyStats.CSV_HEADER;
        assertTrue(header.startsWith(HEADER_BEFORE_THE_HOST_RECORD + ","));
        assertEquals(HEADER_BEFORE_THE_HOST_RECORD.split(",").length, HOST_RECORD_FIRST_COLUMN);
        assertTrue(header.contains(",rtp_host_us,host_present_us,host_acquired_us,host_submitted_us," +
                "host_encoded_us,host_first_sent_us,host_last_sent_us,clock_offset_us,clock_rtt_us,sched_vsync_ns"));
        assertEquals(HOST_RECORD_FIRST_COLUMN + 9 + SCHEDULER_COLUMNS, header.trim().split(",").length);
    }

    @Test
    public void hostRecordColumnsComeFromTheExtendedReportAndTheClockOffset() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        // rtp 9000 ticks = 100000 us. Present 1000000; queue 300 with capture 100 and convert 150;
        // total 900 with encode 200 and send 100.
        stats.onHostTiming(extendedReport(new long[] {42, 900, 300, 200, 100, 1000000, 100, 150}), 1000000000L);
        stats.onDecoderInput(42, 123, 1000000, 1000000, 2000000, (char) 0, 100000);
        stats.onDecoderQueued(123, 2000000);
        stats.onDecoderOutput(0, 123, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(123, 7000000);

        StringWriter csv = new StringWriter();
        long[] offsetCalls = new long[1];
        stats.writeCsv(csv, out -> {
            offsetCalls[0]++;
            out[0] = -3955;
            out[1] = 90;
            return true;
        });
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        // The receiver's own host timing columns are unchanged by the extension.
        assertEquals("0.900000", fields[26]);
        assertEquals("0.300000", fields[27]);
        String[] host = java.util.Arrays.copyOfRange(fields, HOST_RECORD_FIRST_COLUMN, HOST_RECORD_FIRST_COLUMN + 9);
        assertArrayEquals(new String[] {"100000", "1000000", "1000100", "1000300", "1000500", "1000800",
                "1000900", "-3955", "90"}, host);
        assertEquals(1, offsetCalls[0]);
    }

    @Test
    public void hostRecordColumnsStayEmptyForAnOldReportAndWithoutAClockExchange() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onHostTiming(report(new long[] {42, 900, 300, 200, 100}), 1000000000L);
        stats.onDecoderInput(42, 123, 1000000, 1000000, 2000000, (char) 0, 100000);
        stats.onDecoderQueued(123, 2000000);
        stats.onDecoderOutput(0, 123, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(123, 7000000);

        // An old (v1) report fills the old columns only; a host without clock sync gives no offset.
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv, out -> false);
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        assertEquals("0.900000", fields[26]);
        String[] host = java.util.Arrays.copyOfRange(fields, HOST_RECORD_FIRST_COLUMN, HOST_RECORD_FIRST_COLUMN + 9);
        assertArrayEquals(new String[] {"100000", "", "", "", "", "", "", "", ""}, host);

        // A frame without an RTP time (the six-argument call) leaves that column empty too, and
        // writeCsv(Writer) without a source behaves as no clock sync.
        stats.onDecoderInput(43, 124, 1000000, 1000000, 2000000, (char) 0);
        stats.onDecoderQueued(124, 2000000);
        stats.onDecoderOutput(1, 124, 5000000);
        stats.onOutputReleased(1, 6000000, true, true);
        stats.onFrameRendered(124, 7000000);
        csv = new StringWriter();
        stats.writeCsv(csv);
        fields = csv.toString().trim().split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        host = java.util.Arrays.copyOfRange(fields, HOST_RECORD_FIRST_COLUMN, HOST_RECORD_FIRST_COLUMN + 9);
        assertArrayEquals(new String[] {"", "", "", "", "", "", "", "", ""}, host);
    }

    @Test
    public void theSchedulerColumnsAreTheLastOnesAndNamedInOrder() {
        String header = FrameLatencyStats.CSV_HEADER.trim();
        assertTrue(header.endsWith(",clock_offset_us,clock_rtt_us,sched_vsync_ns,sched_hold_ms,sched_late," +
                "queue_depth,residency_excess_ms,in_flight,drop_reason"));
        assertEquals(HOST_RECORD_FIRST_COLUMN + 9 + SCHEDULER_COLUMNS, header.split(",").length);
    }

    @Test
    public void theSchedulerColumnsAreEmptyForAFrameThatWasNeverScheduled() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1000000, 1000000, 2000000, (char) 0, 1);
        stats.onDecoderQueued(1, 2000000);
        stats.onDecoderOutput(0, 1, 5000000);
        stats.onOutputReleased(0, 6000000, true, true);
        stats.onFrameRendered(1, 7000000);
        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] fields = csv.toString().trim().split(",", -1);
        assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
        assertArrayEquals(new String[] {"", "", "", "", "", "", ""},
                java.util.Arrays.copyOfRange(fields, fields.length - SCHEDULER_COLUMNS, fields.length));
    }

    @Test
    public void theSchedulerColumnsCarryThePlanTheQueueAndTheDropReason() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 0; i < 2; i++) {
            stats.onDecoderInput(i + 1, i + 1, 1000000, 1000000, 2000000, (char) 0, i + 1);
            stats.onDecoderQueued(i + 1, 2000000);
            stats.onDecoderOutput(i, i + 1, 5000000);
        }
        assertEquals("input to output of the owned buffer", 3000000, stats.getInputToOutputNs(0));
        assertEquals(0, stats.getInputToOutputNs(7));
        stats.onQueueState(0, 3, 6500000);
        stats.onDropReason(0, "late");
        stats.onOutputReleased(0, 6000000, false, true);
        stats.onScheduled(1, 123456789L, 2500000L, true, 2);
        stats.onQueueState(1, 0, 0);
        stats.onOutputReleased(1, 6100000, true, true);
        stats.onFrameRendered(2, 7000000);
        // Calls for a buffer that is not owned are ignored.
        stats.onScheduled(9, 1, 1, true, 1);
        stats.onQueueState(9, 1, 1);
        stats.onDropReason(9, "x");

        StringWriter csv = new StringWriter();
        stats.writeCsv(csv);
        String[] rows = csv.toString().trim().split("\n");
        assertEquals(2, rows.length);
        java.util.Map<Long, String[]> byPts = new java.util.HashMap<>();
        for (String row : rows) {
            String[] fields = row.split(",", -1);
            assertEquals(FrameLatencyStats.CSV_HEADER.trim().split(",").length, fields.length);
            byPts.put(Long.parseLong(fields[1]), java.util.Arrays.copyOfRange(fields,
                    fields.length - SCHEDULER_COLUMNS, fields.length));
        }
        assertArrayEquals(new String[] {"", "", "", "3", "6.500000", "", "late"}, byPts.get(1L));
        assertArrayEquals(new String[] {"123456789", "2.500000", "1", "0", "0.000000", "2", ""}, byPts.get(2L));
    }

    @Test
    public void theClockOffsetIsReadOnceAFlushAndNotUnderTheStatsLock() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        for (int i = 1; i <= 3; i++) {
            stats.onDecoderInput(i, i, 1000000, 1000000, 2000000, (char) 0, i);
            stats.onDecoderQueued(i, 2000000);
            stats.onDecoderOutput(0, i, 5000000);
            stats.onOutputReleased(0, 6000000, true, true);
            stats.onFrameRendered(i, 7000000);
        }
        int[] reads = new int[1];
        boolean[] heldTheLock = new boolean[1];
        stats.writeCsv(new StringWriter(), out -> {
            reads[0]++;
            heldTheLock[0] = Thread.holdsLock(stats);
            return false;
        });
        assertEquals(1, reads[0]);
        assertFalse(heldTheLock[0]);
    }

}
