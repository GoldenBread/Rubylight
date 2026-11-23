package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.*;

/**
 * Presented FPS and present-interval jitter accuracy across compositor reporting lag and
 * Vulkan scanout reports (issue #119).
 */
public class FrameLatencyPresentedFpsTest {
    @Test
    public void presentedFpsCompensatesCompositorReportLagWithoutMaskingDropsOrStalls() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long startNs = 1_000_000_000L;
        stats.onFrameReceived(startNs);
        // 3 seconds at exact 120 fps (8,333,333.333 ns per frame) with 35 ms compositor report lag:
        // frames released up to t = 3.000 s, but scanout confirmations have only arrived up to t = 2.965 s.
        long queryNs = startNs + 3_000_000_000L;
        for (int i = 1; i <= 360; i++) {
            long t = startNs + Math.round(i * 1_000_000_000.0 / 120.0);
            stats.onFrameReceived(t);
            stats.onDecoderInput(i, i, t, t, t, (char) 0);
            stats.onDecoderQueued(i, t);
            stats.onDecoderOutput(0, i, t + 1_000_000L);
            stats.onOutputReleased(0, t + 2_000_000L, true, true);
            long scanoutNs = t + 20_000_000L;
            if (scanoutNs <= queryNs - 35_000_000L) {
                stats.onFrameShown(i, t + 3_000_000L, scanoutNs, queryNs);
            }
        }
        float[] rates = stats.getFrameRates(queryNs);
        assertEquals(120f, rates[0], 0.5f);
        assertEquals(120f, rates[1], 0.5f);
        // Without lag compensation, 35 ms missing from the 2 s window would report ~117.5 FPS.
        assertEquals(120f, rates[2], 0.5f);

        // Once the stream actually stalls well past the lag cap, Presented FPS drops to zero.
        assertEquals(0f, stats.getFrameRates(queryNs + 3_000_000_000L)[2], 0.001f);
    }

    @Test
    public void presentedFpsStillReflectsRealDisplayDropsUnderReportLag() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long startNs = 1_000_000_000L;
        stats.onFrameReceived(startNs);
        long queryNs = startNs + 3_000_000_000L;
        for (int i = 1; i <= 360; i++) {
            long t = startNs + Math.round(i * 1_000_000_000.0 / 120.0);
            stats.onFrameReceived(t);
            stats.onDecoderInput(i, i, t, t, t, (char) 0);
            stats.onDecoderQueued(i, t);
            stats.onDecoderOutput(0, i, t + 1_000_000L);
            stats.onOutputReleased(0, t + 2_000_000L, true, true);
            long scanoutNs = t + 20_000_000L;
            if (scanoutNs <= queryNs - 35_000_000L) {
                // Real 2.5% drop rate (1 in 40 frames replaced by compositor).
                if (i % 40 == 0) stats.onFrameReplaced(i);
                else stats.onFrameShown(i, t + 3_000_000L, scanoutNs, queryNs);
            }
        }
        float[] rates = stats.getFrameRates(queryNs);
        assertEquals(120f, rates[1], 0.5f);
        assertEquals(117f, rates[2], 0.5f);
    }

    @Test
    public void vulkanPresentJitterUsesScanoutTimeInsteadOfQueueTime() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.setPresentJitterEnabled(true);
        long t0 = 1_000_000_000L;
        long vsync = 8_333_333L;
        // Vulkan queues presents evenly 1 vsync apart, but scanout slips by a refresh on frame 3.
        long[] scanouts = {t0 + 20_000_000L, t0 + 20_000_000L + vsync, t0 + 20_000_000L + 3 * vsync};
        for (int i = 0; i < scanouts.length; i++) {
            long base = t0 + i * vsync;
            stats.onDecoderInput(i + 1, i + 1, base, base, base, (char) 0);
            stats.onDecoderQueued(i + 1, base);
            stats.onDecoderOutput(0, i + 1, base + 1_000_000L);
            stats.onOutputReleased(0, base + 1_500_000L, true, true);
            stats.onFrameShown(i + 1, base + 2_000_000L, scanouts[i], scanouts[i] + 5_000_000L);
        }
        PresentJitter.Snapshot snapshot = stats.presentJitter(vsync);
        assertNotNull(snapshot);
        assertEquals(2, snapshot.count);
        assertEquals(1, snapshot.histogram[1]);
        assertEquals(1, snapshot.histogram[2]);
    }
}
