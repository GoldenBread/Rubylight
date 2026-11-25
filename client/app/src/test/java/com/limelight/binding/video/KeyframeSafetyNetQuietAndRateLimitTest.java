package com.limelight.binding.video;

import com.limelight.binding.video.KeyframeSafetyNetTest.Sim;

import org.junit.Test;

import static com.limelight.binding.video.KeyframeSafetyNetTest.FPS_120;
import static com.limelight.binding.video.KeyframeSafetyNetTest.FPS_30;
import static com.limelight.binding.video.KeyframeSafetyNetTest.FPS_60;
import static com.limelight.binding.video.KeyframeSafetyNetTest.KBPS;
import static com.limelight.binding.video.KeyframeSafetyNetTest.SMALL;
import static com.limelight.binding.video.KeyframeSafetyNetTest.due;
import static com.limelight.binding.video.KeyframeSafetyNetTest.inMotion;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * Quiet-picture bitrate thresholds, periodic limit refreshes, keyframe rate limiting, gap
 * forgiveness, and {@link KeyframeSafetyNet.Counts} lifetime tests for {@link KeyframeSafetyNet}.
 */
public class KeyframeSafetyNetQuietAndRateLimitTest {
    @Test
    public void calmIsAQuarterOfAFramesShareOfTheBitrate() {
        assertEquals(32_812, KeyframeSafetyNet.quietFrameBytes(63_000, FPS_60));
        assertEquals(16_406, KeyframeSafetyNet.quietFrameBytes(63_000, FPS_120));
        assertEquals(62_500, KeyframeSafetyNet.quietFrameBytes(60_000, FPS_30));
        assertEquals(5_208, KeyframeSafetyNet.quietFrameBytes(10_000, FPS_60));
        assertEquals("the cap", 64 * 1024, KeyframeSafetyNet.quietFrameBytes(160_000, FPS_60));
        assertEquals("the cap", 64 * 1024, KeyframeSafetyNet.quietFrameBytes(300_000, FPS_60));
        assertEquals("the cap", 64 * 1024, KeyframeSafetyNet.quietFrameBytes(63_000, 24_000));
        assertEquals("no bitrate", 0, KeyframeSafetyNet.quietFrameBytes(0, FPS_60));
        assertEquals("no frame rate", 0, KeyframeSafetyNet.quietFrameBytes(63_000, 0));
    }

    private static void assertFullMotionIsNotCalm(int fpsMilliHz, long frameMs, int bitrateKbps, int frameBytes) {
        Sim sim = new Sim(fpsMilliHz, frameMs);
        sim.bitrateKbps = bitrateKbps;
        sim.frameBytes = frameBytes;
        sim.steady(29_000, 3);
        assertEquals("every " + frameBytes + " B frame is what the bitrate allows", 0, sim.counts.requests());
        sim.steady(2_000, 3);
        assertEquals(1, sim.counts.limitRepairs);
    }

    @Test
    public void fullMotionAtALowBitrateIsNotCalm() {
        assertFullMotionIsNotCalm(FPS_60, 16, 10_000, 20_833);
        assertFullMotionIsNotCalm(FPS_60, 16, 15_000, 31_250);
        assertFullMotionIsNotCalm(FPS_120, 8, 30_000, 31_250);
    }

    @Test
    public void aCalmPictureAtALowBitrateStillRefreshes() {
        Sim sim = new Sim();
        sim.bitrateKbps = 10_000;
        sim.frameBytes = 3 * 1024;
        sim.steady(9_500, 3);
        assertEquals(0, sim.counts.requests());
        sim.steady(1_000, 3);
        assertEquals(1, sim.counts.quietRepairs);
    }

    @Test
    public void aHighBitrateCallsLargerFramesCalm() {
        // Over 63 Mbps' 32 KB and under the cap. It used to prove 150 Mbps' whole share, about 78 KB; the
        // 64 KiB cap now ends that, since a share of a high bitrate called moderate motion calm.
        Sim sim = new Sim();
        sim.bitrateKbps = 150_000;
        sim.frameBytes = 60 * 1024;
        sim.steady(10_500, 3);
        assertEquals(1, sim.counts.quietRepairs);
    }

    @Test
    public void realMotionAtAHighBitrateIsNotCalm() {
        assertFullMotionIsNotCalm(FPS_60, 16, 160_000, 70 * 1024);
        assertFullMotionIsNotCalm(FPS_60, 16, 300_000, 130 * 1024);
    }

    @Test
    public void realMotionAt24FpsIsNotCalmAndACalmPictureIs() {
        assertFullMotionIsNotCalm(24_000, 42, 63_000, 70 * 1024);
        Sim sim = new Sim(24_000, 42);
        sim.frameBytes = 30 * 1024;
        sim.steady(10_500, 3);
        assertEquals(1, sim.counts.quietRepairs);
    }


    @Test
    public void aBitrateCutChangesWhatCalmIsAtOnce() {
        Sim sim = new Sim();
        sim.frameBytes = 25 * 1024;
        sim.bitrateKbps = 30_000;
        sim.steady(15_000, 3);
        assertEquals("over a quarter of 30 Mbps' frame", 0, sim.counts.requests());
        sim.bitrateKbps = KBPS;
        sim.steady(100, 3);
        assertEquals("under a quarter of 63 Mbps' frame", 1, sim.counts.quietRepairs);
    }

    @Test
    public void anUnknownBitrateIsNeverCalm() {
        Sim sim = new Sim();
        sim.bitrateKbps = 0;
        sim.frameBytes = SMALL;
        sim.steady(29_000, 3);
        assertEquals(0, sim.counts.requests());
        sim.steady(2_000, 3);
        assertEquals(1, sim.counts.limitRepairs);
    }

    @Test
    public void aCalmPictureRefreshesAfterTenSeconds() {
        Sim sim = new Sim();
        sim.frameBytes = SMALL;
        sim.steady(9_500, 3);
        assertEquals(0, sim.counts.requests());
        sim.steady(1_000, 3);
        assertEquals(1, sim.counts.quietRepairs);
        assertEquals(0, sim.counts.limitRepairs);
    }

    @Test
    public void largeFramesWaitForTheLimit() {
        Sim sim = new Sim();
        sim.steady(29_000, 3);
        assertEquals(0, sim.counts.requests());
        sim.steady(2_000, 3);
        assertEquals(1, sim.counts.limitRepairs);
        assertEquals(0, sim.counts.quietRepairs);
    }

    @Test
    public void motionThatCalmsDownRefreshesAboutASecondLater() {
        Sim sim = new Sim();
        sim.steady(15_000, 3);
        assertEquals(0, sim.counts.requests());
        sim.frameBytes = SMALL;
        sim.steady(200, 3);
        assertEquals("the average is still coming down", 0, sim.counts.requests());
        sim.steady(1_800, 3);
        assertEquals(1, sim.counts.quietRepairs);
    }

    @Test
    public void aKeyframeForAnyOtherReasonRestartsTheWaits() {
        KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        KeyframeSafetyNet net = inMotion(counts);
        net.onKeyframe(0);
        assertFalse(due(net, 29_000));
        net.onKeyframe(29_000);
        assertFalse(due(net, 58_999));
        assertTrue(due(net, 59_000));
        assertEquals(1, counts.limitRepairs);
    }

    @Test
    public void nothingBeforeTheFirstKeyframe() {
        KeyframeSafetyNet net = new KeyframeSafetyNet(new KeyframeSafetyNet.Counts());
        assertFalse(due(net, 60_000));
        assertFalse(due(net, 120_000));
    }

    @Test
    public void theRateLimitHoldsARepairUntilItsTime() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.net.onKeyframe(sim.nowMs);
        long keyframeAt = sim.nowMs;
        sim.steady(2_000, 20);
        assertEquals("the climb is seen", 1, sim.counts.climbs);
        assertEquals("but this soon after a keyframe it waits", 0, sim.counts.climbRepairs);
        sim.steady(4_000, 20);
        assertEquals(1, sim.counts.climbRepairs);
        assertTrue(sim.requestsAtMs.get(0) - keyframeAt >= KeyframeSafetyNet.MIN_GAP_MS);
    }

    @Test
    public void aKeyframeBeforeTheRepairsTurnAnswersIt() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.net.onKeyframe(sim.nowMs);
        sim.steady(2_000, 20);
        assertEquals(1, sim.counts.climbs);
        sim.net.onKeyframe(sim.nowMs);
        sim.steady(8_000, 20);
        assertEquals("the keyframe that came is the repair", 0, sim.counts.climbRepairs);
    }

    @Test
    public void aKeyframeDuringAClimbLeavesTheRepairAtItsEnd() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.steady(3_000, 20);
        assertEquals(1, sim.counts.climbRepairs);
        sim.net.onKeyframe(sim.nowMs);
        sim.steady(6_000, 20);
        sim.steady(8_000, 3);
        assertEquals(1, sim.counts.climbEndRepairs);
    }

    @Test
    public void anUnansweredRequestWaitsAgainAndDoesNotAskOnEveryFrame() {
        KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        KeyframeSafetyNet net = inMotion(counts);
        net.onKeyframe(0);
        assertTrue(due(net, 30_000));
        for (long at = 30_016; at < 60_000; at += 16) {
            assertFalse(due(net, at));
        }
        assertTrue(due(net, 60_000));
        assertEquals(2, counts.limitRepairs);
    }

    @Test
    public void oneGapPerRequest() {
        KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        KeyframeSafetyNet net = new KeyframeSafetyNet(counts);
        assertFalse("nothing asked yet", net.takeAnsweredGap(4));
        net.onKeyframe(0);
        assertTrue(due(net, 30_000));
        assertTrue(net.takeAnsweredGap(4));
        assertFalse(net.takeAnsweredGap(4));
        assertEquals(4, counts.gapFrames);
    }

    @Test
    public void aKeyframeRightBehindTheRequestLeavesNoGapToForgive() {
        KeyframeSafetyNet net = new KeyframeSafetyNet(new KeyframeSafetyNet.Counts());
        net.onKeyframe(0);
        assertTrue(due(net, 30_000));
        net.onKeyframe(30_050);
        assertFalse(net.takeAnsweredGap(3));
    }

    @Test
    public void countsOutliveTheNet() {
        KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        KeyframeSafetyNet first = new KeyframeSafetyNet(counts);
        first.onKeyframe(0);
        assertTrue(due(first, 30_000));
        KeyframeSafetyNet restarted = new KeyframeSafetyNet(counts);
        restarted.onKeyframe(40_000);
        assertTrue(due(restarted, 70_000));
        assertEquals(2, counts.requests());
        assertEquals("climbs 0, settled 0, repairs climb 0 end 0 quiet 0 limit 2, gap frames 0, held 0 frames 0 ms", counts.describe());
    }

}
