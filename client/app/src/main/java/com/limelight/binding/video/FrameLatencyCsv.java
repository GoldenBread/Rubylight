package com.limelight.binding.video;

import com.limelight.nvstream.av.video.HostTimingReport;

/**
 * Pure static CSV row formatting extracted from {@link FrameLatencyStats}.
 *
 * <p>Formats each completed frame row into a reused {@link StringBuilder} without intermediate
 * strings or locale-sensitive formatting. {@code FrameLatencyCsvRowTest} holds every character
 * identical to the original output.
 */
final class FrameLatencyCsv {
    private FrameLatencyCsv() {}

    static void appendRow(StringBuilder row, FrameLatencyStats.Frame frame, boolean vrr, long lost,
                          long dropped, long sampleNs, String frameRates, String clockColumns) {
        boolean rendered = "rendered".equals(frame.status);
        row.append(frame.number).append(',').append(frame.ptsUs).append(',');
        timestamp(row, frame.receiveNs).append(',');
        timestamp(row, frame.inputNs).append(',');
        timestamp(row, frame.outputNs).append(',');
        timestamp(row, frame.renderNs).append(',');
        timestamp(row, frame.releaseNs).append(',');
        row.append(frame.status).append(',');
        duration(row, frame.receiveNs, frame.inputNs).append(',');
        duration(row, frame.inputNs, frame.outputNs).append(',');
        if (rendered) duration(row, frame.outputNs, frame.renderNs);
        row.append(',');
        if (rendered) duration(row, frame.receiveNs, frame.renderNs);
        row.append(',').append(lost).append(',');
        if (frame.hostProcessingNs != 0) milliseconds(row, frame.hostProcessingNs);
        row.append(',');
        // decode_to_present_ms and receive_to_present_ms repeat the two columns before csv_rows_lost.
        if (rendered) duration(row, frame.outputNs, frame.renderNs);
        row.append(',');
        if (rendered) duration(row, frame.receiveNs, frame.renderNs);
        row.append(',');
        if (vrr) duration(row, frame.outputNs, frame.releaseNs);
        row.append(',');
        duration(row, frame.previousOutputNs, frame.outputNs).append(',');
        duration(row, frame.previousReleaseNs, frame.releaseNs).append(',');
        row.append(sampleNs).append(',').append(frameRates).append(',').append(dropped).append(',');
        timestamp(row, frame.enqueueNs).append(',');
        duration(row, frame.enqueueNs, frame.inputNs);
        hostTimingColumns(row, frame.hostTiming);
        row.append(',');
        timestamp(row, frame.callbackReceivedNs).append(',');
        duration(row, frame.renderNs, frame.callbackReceivedNs).append(',');
        if (frame.hostRtpUs >= 0) row.append(frame.hostRtpUs);
        hostStampColumns(row, frame.hostTiming);
        row.append(',').append(clockColumns);
        schedulerColumns(row, frame);
        row.append('\n');
    }

    private static void schedulerColumns(StringBuilder row, FrameLatencyStats.Frame frame) {
        row.append(',');
        timestamp(row, frame.schedVsyncNs).append(',');
        if (frame.schedHoldNs >= 0) milliseconds(row, frame.schedHoldNs);
        row.append(',');
        if (frame.schedHoldNs >= 0) row.append(frame.schedLate ? '1' : '0');
        row.append(',');
        if (frame.queueDepth >= 0) row.append(frame.queueDepth);
        row.append(',');
        if (frame.residencyExcessNs >= 0) milliseconds(row, frame.residencyExcessNs);
        row.append(',');
        if (frame.inFlight >= 0) row.append(frame.inFlight);
        row.append(',');
        if (frame.dropReason != null) row.append(frame.dropReason);
    }

    private static void hostTimingColumns(StringBuilder row, HostTimingReport.Record record) {
        if (record == null) {
            row.append(",,,,");
            return;
        }
        milliseconds(row.append(','), record.hostUs * 1000L);
        milliseconds(row.append(','), record.queueUs * 1000L);
        milliseconds(row.append(','), record.encodeUs * 1000L);
        milliseconds(row.append(','), record.sendUs * 1000L);
    }

    // Host present, acquired, submitted, encoded, first and last packet sent, on the host's video
    // clock (microseconds), each with its leading comma; empty without an extended report.
    private static void hostStampColumns(StringBuilder row, HostTimingReport.Record record) {
        if (record == null || !record.hasExtra) {
            row.append(",,,,,,");
            return;
        }
        long present = record.presentedAtUs;
        long submitted = present + record.queueUs;
        long lastSent = present + record.hostUs;
        unsigned(row.append(','), present);
        unsigned(row.append(','), present + record.captureUs);
        unsigned(row.append(','), submitted);
        unsigned(row.append(','), submitted + record.encodeUs);
        unsigned(row.append(','), lastSent - record.sendUs);
        unsigned(row.append(','), lastSent);
    }

    /** The host's clock is a u64 carried in a long. */
    private static void unsigned(StringBuilder row, long value) {
        if (value >= 0) {
            row.append(value);
        } else {
            row.append(Long.toUnsignedString(value));
        }
    }

    private static StringBuilder timestamp(StringBuilder row, long timeNs) {
        if (timeNs != 0) row.append(timeNs);
        return row;
    }

    private static StringBuilder duration(StringBuilder row, long startNs, long endNs) {
        if (startNs != 0 && endNs != 0 && endNs >= startNs) milliseconds(row, endNs - startNs);
        return row;
    }

    // Milliseconds with all six decimals, from whole numbers so no locale can touch them: the nanoseconds below a
    // millisecond are written behind a leading 1, which is then taken out again.
    private static void milliseconds(StringBuilder row, long durationNs) {
        row.append(durationNs / 1000000).append('.');
        int lead = row.length();
        row.append(1000000 + durationNs % 1000000).deleteCharAt(lead);
    }
}
