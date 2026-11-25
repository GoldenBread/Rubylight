package com.limelight.binding.video;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.Random;

import static com.limelight.binding.video.DecoderQueuePolicyTest.FRAME_NS;
import static com.limelight.binding.video.DecoderQueuePolicyTest.MS;
import static com.limelight.binding.video.DecoderQueuePolicyTest.QueuedDecoder;
import static com.limelight.binding.video.DecoderQueuePolicyTest.firstDebtDrop;
import static com.limelight.binding.video.DecoderQueuePolicyTest.shieldCapture;
import static org.junit.Assert.*;

/** Display-queue-watching decoder debt policy, recent level, display gate, in-flight frames, and decoder depth. */
public class DecoderQueuePolicyDisplayTest {
    private static DecoderQueuePolicy.Debt watchingDebt() {
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        debt.watchDisplay(true);
        return debt;
    }

    /** As [firstDebtDrop], watching the display. The captures only hold the stamp after the queue call. */
    private static long firstWatchingDrop(long[][] capture) {
        DecoderQueuePolicy.Debt debt = watchingDebt();
        for (long[] output : capture) {
            debt.onOutput(output[1], output[1], output[0], FRAME_NS, (int) output[2]);
            if (debt.shouldDrop()) return output[0] - capture[0][0];
        }
        return 0;
    }

    /**
     * Replays a capture and acts on the policy: a held output dropped with nothing behind it gives
     * the outputs a frame of room, so they follow their recorded inputs again. This loop takes the
     * frame of room off itself, so a replay shows when the policy drops, and how long the wait got by
     * then. It cannot show that a drop at that point frees a buffer: only the debt drops recorded at
     * half a frame (8.7 to 0.9 ms) speak for that. Returns the drop times, and the highest median of
     * a second before the first one.
     */
    private static List<Long> replayDrops(DecoderQueuePolicy.Debt debt, long[][] capture, long[] peakBeforeNs) {
        List<Long> drops = new ArrayList<>();
        List<Long> second = new ArrayList<>();
        long startNs = capture[0][0];
        long secondNs = startNs;
        long roomNs = 0;
        for (long[] output : capture) {
            long inputNs = output[0] - output[1];
            long i2oNs = roomNs > 0 ? Math.max(Math.min(output[1], 900 * 1000L), output[1] - roomNs) : output[1];
            debt.onOutput(i2oNs, i2oNs, inputNs + i2oNs, FRAME_NS, (int) output[2]);
            second.add(i2oNs);
            if (output[0] - secondNs >= 1_000 * MS) {
                java.util.Collections.sort(second);
                if (roomNs == 0) peakBeforeNs[0] = Math.max(peakBeforeNs[0], second.get(second.size() / 2));
                second.clear();
                secondNs = output[0];
            }
            if (debt.shouldDrop()) {
                debt.onDropped();
                drops.add(output[0] - startNs);
                roomNs += FRAME_NS;
            }
        }
        return drops;
    }

    @Test public void debtWatchesTheDisplayOnlyOnTheShieldsDecoders() {
        assertTrue(DecoderQueuePolicy.watchesDisplayQueue("OMX.Nvidia.h265.decode"));
        assertTrue(DecoderQueuePolicy.watchesDisplayQueue("omx.nvidia.h264.decode"));
        assertTrue(DecoderQueuePolicy.watchesDisplayQueue("c2.nvidia.hevc.decoder"));
        assertFalse(DecoderQueuePolicy.watchesDisplayQueue("c2.qti.hevc.decoder.low_latency"));
        assertFalse(DecoderQueuePolicy.watchesDisplayQueue("c2.mtk.hevc.decoder"));
        assertFalse(DecoderQueuePolicy.watchesDisplayQueue("OMX.MTK.VIDEO.DECODER.HEVC"));
        assertFalse(DecoderQueuePolicy.watchesDisplayQueue("c2.android.hevc.decoder"));
    }

    @Test public void debtElsewhereGivesItsOldAnswersOnTheTracesThatChangedOnAShield() throws Exception {
        // What the policy answered before the display rules, on the 4.6.1 captures: half a frame only.
        assertEquals(0, firstDebtDrop(shieldCapture("461-sm3-rise"), 0, Long.MAX_VALUE));
        assertEquals(0, firstDebtDrop(shieldCapture("461-sm4-rise"), 0, Long.MAX_VALUE));
        assertEquals(0, firstDebtDrop(shieldCapture("461-sm4-start"), 0, Long.MAX_VALUE));
        assertEquals(0, firstDebtDrop(shieldCapture("461-sm2-tail"), 0, Long.MAX_VALUE));
        assertEquals(0, firstDebtDrop(shieldCapture("461-sm1-edge"), 0, Long.MAX_VALUE));

        // And the start-up burst still raises the floor, whatever its level.
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        long[][] start = shieldCapture("461-sm1-start");
        int drops = 0;
        String event = null;
        for (int i = 0; i < start.length; i++) {
            debt.onOutput(start[i][1], start[i][0], FRAME_NS);
            if (debt.shouldDrop()) {
                if (start[i][2] == 0 || i % 3 == 0) {
                    debt.onDropped();
                    drops++;
                } else {
                    debt.onNoNewer();
                }
            }
            String taken = debt.takeEvent();
            if (taken != null) event = taken;
        }
        assertEquals(1, drops);
        assertEquals("Decoder debt paused, a drop did not shorten the queue: 1 stale output dropped, "
                + "excess 51.0 -> 57.1 ms over a 11.1 ms floor, floor raised to 68.2 ms", event);
    }

    @Test public void debtElsewhereLeavesAHeldOutputAloneBelowHalfAFrame() {
        // The same held stream the watching tests drop within seconds.
        QueuedDecoder d = new QueuedDecoder();
        d.dips = false;
        d.start();
        d.run(15_000 * MS);
        d.bufferNs = 4 * MS;
        d.run(120_000 * MS);
        assertEquals(0, d.drops);
    }

    @Test public void debtWatchingSubFrameDriftCostsOneDropForEachFrameItAdds() {
        // The 0.7.13 and 4.6.1 captures: the display runs 45 ppm slower than the host, so the wait
        // for a display buffer grows by 0.045 ms a second with nothing behind it. It is seen within
        // a minute instead of the three half a frame takes. A dropped output then gives this model's
        // queue a frame of room, 16.67 / 0.045 s of that drift.
        QueuedDecoder d = new QueuedDecoder(true);
        d.dips = false;
        d.start();
        long startNs = d.nowNs;
        d.drift(45_000, 60_000 * MS);
        assertEquals(1, d.drops);
        assertEquals(1, d.heldDrops);
        assertTrue("dropped at " + (d.firstDropNs - startNs) / 1e9 + " s", d.firstDropNs - startNs <= 60_000 * MS);
        d.drift(45_000, 240_000 * MS);
        assertEquals("the frame of room lasts", 1, d.drops);
        assertTrue(d.events.get(0), d.events.get(0).startsWith("Decoder debt recovered: 1 stale output dropped"));

        // Half an hour costs what the drift adds, 30 * 60 * 45 us of a 16.7 ms frame, and no more.
        d.drift(45_000, 1_500_000 * MS);
        assertEquals(5, d.drops);
        assertEquals(5, d.events.size());
        assertTrue("peak " + d.peakI2oNs / 1e6 + " ms", d.peakI2oNs < 4 * MS);
    }

    @Test public void debtWatchingSeesAPlateauWithNothingBehindWithinSeconds() {
        // The 4.4.0 capture, as debtShieldPlateauWithNothingBehindIsDroppedWithinSeconds.
        QueuedDecoder d = new QueuedDecoder(true);
        d.start();
        d.run(60_000 * MS);
        assertEquals(0, d.drops);
        long riseNs = d.nowNs;
        d.drift(400_000, 25_000 * MS);
        d.run(20_000 * MS);
        assertEquals(1, d.drops);
        assertEquals(1, d.heldDrops);
        assertTrue("buffer " + d.bufferNs / 1e6 + " ms", d.bufferNs < 4 * MS);
        // Half a frame would be reached 21 s into the rise; ten seconds of held outputs come first.
        assertTrue("dropped " + (d.firstDropNs - riseNs) / 1e9 + " s into the rise",
                d.firstDropNs - riseNs <= 15_000 * MS);
        d.run(60_000 * MS);
        assertEquals("healthy again", 1, d.drops);

        // A drop that frees nothing with half a frame on the clock: one drop, and the gate stays off.
        QueuedDecoder own = new QueuedDecoder(true);
        own.dropsHelp = false;
        own.start();
        own.run(15_000 * MS);
        own.bufferNs = 11 * MS;
        own.run(60_000 * MS);
        assertEquals(1, own.drops);
        assertTrue(own.events.get(0), own.events.get(0).startsWith("Decoder debt paused, a drop did not shorten the queue"));
        own.bufferNs = 0;
        own.run(15_000 * MS);
        own.bufferNs = 13 * MS;
        own.run(30_000 * MS);
        assertEquals("under half a frame over the raised floor", 1, own.drops);
    }

    @Test public void debtWatchingReplaysTheRisingStreamsOfNightjar461() throws Exception {
        // sm3 (from 100 s): the wait starts at about 115 s, input-to-output passes 2 ms at 125 s, and
        // half a frame was reached at 259 s at 8.7 ms, before the recorded debt drop.
        long[] peakNs = new long[1];
        DecoderQueuePolicy.Debt sm3 = watchingDebt();
        List<Long> drops = replayDrops(sm3, shieldCapture("461-sm3-rise"), peakNs);
        assertEquals("one drop", 1, drops.size());
        assertTrue("dropped at " + (100 + drops.get(0) / 1e9) + " s", drops.get(0) >= 25_000 * MS && drops.get(0) <= 42_000 * MS);
        assertTrue("before the drop " + peakNs[0] / 1e6 + " ms", peakNs[0] < 5 * MS);
        String event = sm3.takeEvent();
        assertTrue(event, event.startsWith("Decoder debt recovered: 1 stale output dropped"));

        // sm4 (from 190 s, the minute it measures against): the wait starts at about 225 s. Half a frame came at 369 s.
        peakNs[0] = 0;
        drops = replayDrops(watchingDebt(), shieldCapture("461-sm4-rise"), peakNs);
        assertEquals(1, drops.size());
        assertTrue("dropped at " + (190 + drops.get(0) / 1e9) + " s", drops.get(0) >= 45_000 * MS && drops.get(0) <= 70_000 * MS);
        assertTrue("before the drop " + peakNs[0] / 1e6 + " ms", peakNs[0] < 5 * MS);
    }

    @Test public void debtWatchingNeedsAFreeLevelToMeasureAWaitAgainst() throws Exception {
        // sm4's start: 11 ms from the second second on, 8.1 ms over the smallest time against a half
        // frame of 8.3, for 14 s until a stall of the host's cleared it. That was the level from the
        // start, and nothing in the recent past says it can be lower.
        assertEquals(0, firstWatchingDrop(shieldCapture("461-sm4-start")));
    }

    @Test public void debtWatchingLeavesTheSteadyPartsOfNightjar461Alone() throws Exception {
        // sm2's end: free outputs at 1.2 ms, until the wait starts 6.5 s before the run ends.
        assertEquals(0, firstWatchingDrop(shieldCapture("461-sm2-tail")));
        // sm1's end: 62 ms to the screen with outputs half following their input, 1.6-2.4 ms.
        assertEquals(0, firstWatchingDrop(shieldCapture("461-sm1-edge")));
        // The earlier second capture: a healthy 1-2 ms for two minutes, with 4-6 ms of jitter at times.
        assertEquals(0, firstWatchingDrop(shieldCapture("sm2")));
    }

    @Test public void debtWatchingKeepsTheFloorOfAStartUpBurst() throws Exception {
        // sm1's start: frames behind by up to 450 ms, and a drop in the middle of it did not shorten
        // anything. On 4.6.1 the floor rose to 81 ms: residency_excess_ms read 0 for the 4.5 minutes
        // that followed, however high input-to-output went.
        DecoderQueuePolicy.Debt debt = watchingDebt();
        long[][] start = shieldCapture("461-sm1-start");
        boolean dropped = false;
        for (long[] output : start) {
            debt.onOutput(output[1], output[1], output[0], FRAME_NS, (int) output[2]);
            if (debt.shouldDrop() && !dropped) {
                debt.onDropped();
                dropped = true;
            }
            String event = debt.takeEvent();
            if (event != null) assertFalse(event, event.contains("floor raised"));
        }
        assertTrue(dropped);
        long nowNs = start[start.length - 1][0] + FRAME_NS;
        assertTrue("a later 30 ms is seen as excess", debt.onOutput(30 * MS, 30 * MS, nowNs, FRAME_NS, 0) > 20 * MS);
    }

    /** Five seconds light and five deep for an hour, a frame behind each deep output; no drop frees anything. */
    private static int[] deepInMotion(boolean watching, long frameNs, long deepNs) {
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        debt.watchDisplay(watching);
        Random random = new Random(5);
        long sceneFrames = 5_000_000_000L / frameNs;
        long now = 1_000_000_000L;
        int drops = 0;
        int raised = 0;
        for (long i = 0; i < 3_600_000_000_000L / frameNs; i++) {
            boolean deep = (i / sceneFrames) % 2 == 1;
            long i2o = (deep ? deepNs : 2 * MS) + random.nextInt(500_000);
            now += frameNs;
            if (watching) debt.onOutput(i2o, i2o, now + i2o, frameNs, deep ? 2 : 0);
            else debt.onOutput(i2o, now + i2o, frameNs);
            String event = debt.takeEvent();
            if (event != null && event.contains("floor raised")) raised++;
            if (debt.shouldDrop()) {
                debt.onDropped();
                drops++;
            }
        }
        return new int[] {drops, raised};
    }

    @Test public void debtLearnsADeepDecodersLevelEvenPastThreeFrames() {
        // A decoder that pipelines 60 ms in motion: one pause could be a stall, the second at the same
        // level is the decoder's. Not a drop per scene change.
        assertArrayEquals(new int[] {2, 1}, deepInMotion(true, FRAME_NS, 60 * MS));
        assertArrayEquals(new int[] {2, 1}, deepInMotion(true, 8_333_333L, 30 * MS));
        assertArrayEquals("under three frames the first time", new int[] {1, 1}, deepInMotion(true, FRAME_NS, 40 * MS));
        assertArrayEquals(new int[] {1, 1}, deepInMotion(false, FRAME_NS, 60 * MS));
        assertArrayEquals(new int[] {1, 1}, deepInMotion(false, 8_333_333L, 30 * MS));
    }

    @Test public void debtWatchingFreeOutputsAboveAWaitLevelNeverDrop() {
        // A heavier scene every minute: 3 ms over the light one, the outputs still following their input.
        QueuedDecoder d = new QueuedDecoder(true);
        d.start();
        for (int i = 0; i < 10; i++) {
            d.phaseNs = 0;
            d.run(15_000 * MS);
            d.phaseNs = 3 * MS;
            d.run(45_000 * MS);
        }
        assertEquals(0, d.drops);
        assertTrue(d.events.isEmpty());
    }

    @Test public void debtWatchingOutputsOnADecoderClockCostOneDropAStream() {
        // A decoder that hands out on its own clock looks held, but a drop frees nothing. Half a
        // frame is never reached, so the check on the first drop is what ends it.
        QueuedDecoder d = new QueuedDecoder(true);
        d.dropsHelp = false;
        d.dips = false;
        d.start();
        d.run(15_000 * MS);
        d.bufferNs = 3 * MS;
        d.run(30_000 * MS);
        assertEquals(1, d.drops);
        assertEquals(2, d.events.size());
        assertTrue(d.events.get(1), d.events.get(1).startsWith("Decoder debt: outputs still follow the display"));
        d.bufferNs = 0;
        d.run(15_000 * MS);
        d.bufferNs = 3 * MS;
        d.run(30_000 * MS);
        assertEquals("the check stays failed", 1, d.drops);

        // A flush is a new stream state, and a real wait after it is still caught.
        d.debt.reset();
        d.dropsHelp = true;
        d.bufferNs = 0;
        d.run(15_000 * MS);
        d.bufferNs = 3 * MS;
        d.run(15_000 * MS);
        assertEquals(2, d.drops);
    }

    @Test public void debtWatchingFramesBehindRuleTheDisplayOut() {
        // Input-to-output that does not follow the input with a frame in the decoder behind each
        // output is a decoder finishing one frame when the next one comes in.
        QueuedDecoder d = new QueuedDecoder(true);
        d.dips = false;
        d.start();
        d.run(15_000 * MS);
        d.behind = 1;
        d.bufferNs = 4 * MS;
        d.run(60_000 * MS);
        assertEquals(0, d.drops);
        assertEquals(0, d.waits);
    }

    @Test public void debtWatchingHeldOutputWaitsForOneWithNothingBehindIt() {
        // The same stream twice: the first says when it drops, the second has frames behind by then.
        QueuedDecoder first = heldWithNothingBehind();
        long heldNs = first.nowNs;
        first.run(20_000 * MS);
        assertEquals(1, first.heldDrops);

        QueuedDecoder d = heldWithNothingBehind();
        d.run(first.firstDropNs - heldNs - 300 * MS);
        assertEquals("not before the window", 0, d.drops);
        d.behind = 1;
        d.newerEvery = 1;
        d.run(600 * MS);
        assertEquals("a frame behind this one: no wait, no drop", 0, d.drops);
        assertEquals(0, d.waits);
        d.behind = 0;
        d.run(4_000 * MS);
        assertEquals(1, d.heldDrops);
    }

    private static QueuedDecoder heldWithNothingBehind() {
        QueuedDecoder d = new QueuedDecoder(true);
        d.dips = false;
        d.start();
        d.behind = 0;
        d.run(15_000 * MS);
        d.bufferNs = 3 * MS;
        return d;
    }

    /** Outputs at a level: on the display's grid when held, else following an input with half a ms of jitter. */
    private static final class WatchedStream {
        final DecoderQueuePolicy.Debt debt = watchingDebt();
        final Random random = new Random(23);
        final List<Long> drops = new ArrayList<>();
        final List<String> events = new ArrayList<>();
        long gridNs = 1_000_000_000L;

        /** Until durationNs has passed or the policy drops; returns whether it dropped. */
        boolean run(long durationNs, long levelNs, boolean held) {
            long endNs = gridNs + durationNs;
            int before = drops.size();
            while (gridNs < endNs && drops.size() == before) {
                gridNs += FRAME_NS;
                long jitterNs = random.nextInt(1_000_000) - 500_000;
                long i2oNs = held ? levelNs - jitterNs : levelNs + random.nextInt(100_000);
                debt.onOutput(i2oNs, i2oNs, held ? gridNs : gridNs + jitterNs + i2oNs, FRAME_NS, 0);
                String event = debt.takeEvent();
                if (event != null) events.add(event);
                if (debt.shouldDrop()) {
                    debt.onDropped();
                    drops.add(gridNs);
                }
            }
            return drops.size() > before;
        }
    }

    @Test public void debtWatchingBacksOffWhenADropChangesNothing() {
        // Outputs that read held for a while, then free again at the same level: the drop did nothing.
        WatchedStream s = new WatchedStream();
        for (int i = 0; i < 60; i++) {
            s.run(12_000 * MS, MS, false);
            if (s.run(30_000 * MS, 4 * MS, true)) s.run(8_000 * MS, 4 * MS, false);
        }
        assertEquals("first, then ten minutes on, then twenty", 3, s.drops.size());
        assertTrue(s.drops.get(1) - s.drops.get(0) >= DecoderQueuePolicy.Debt.GATE_BACKOFF_NS);
        assertTrue(s.drops.get(2) - s.drops.get(1) >= 2 * DecoderQueuePolicy.Debt.GATE_BACKOFF_NS);
        assertEquals(3, s.events.stream().filter(e -> e.contains("the next one waits")).count());
    }

    @Test public void debtWatchingStopsAtItsLimitAndLeavesHalfAFrame() {
        // Every drop ends the wait, so nothing backs off, but a flush is what renews the limit.
        WatchedStream s = new WatchedStream();
        for (int i = 0; i < 2 * DecoderQueuePolicy.Debt.MAX_GATE_DROPS; i++) {
            s.run(12_000 * MS, MS, false);
            if (s.run(30_000 * MS, 4 * MS, true)) s.run(8_000 * MS, MS, false);
        }
        assertEquals(DecoderQueuePolicy.Debt.MAX_GATE_DROPS, s.drops.size());
        assertEquals(1, s.events.stream().filter(e -> e.contains("24 drops for the display")).count());
        assertTrue("half a frame still counts", s.run(5_000 * MS, 12 * MS, true));
        int atLimit = s.drops.size();
        s.debt.reset();
        s.run(12_000 * MS, MS, false);
        assertTrue(s.run(30_000 * MS, 4 * MS, true));
        assertEquals(atLimit + 1, s.drops.size());
    }

    @Test public void debtWatchingMeasuresTheWaitAgainstTheLastMinute() {
        // One light scene at the start of a stream, then the level it stays at. Nothing is waiting
        // any more than it did a minute ago, whatever the outputs read.
        WatchedStream s = new WatchedStream();
        s.run(5_000 * MS, MS, false);
        s.run(120_000 * MS, 4 * MS, false);
        s.run(60_000 * MS, 4 * MS, true);
        assertTrue(s.drops.isEmpty());
    }

    @Test public void debtWatchesTheTimeFromBeforeTheQueueCall() {
        // A free decoder on a steady link, whose queue call returns up to a millisecond late. That is
        // in input-to-output, and no output follows it: read from there, the outputs look held.
        DecoderQueuePolicy.Debt debt = watchingDebt();
        Random random = new Random(29);
        long gridNs = 1_000_000_000L;
        for (int i = 0; i < 60 * 600; i++) {
            gridNs += FRAME_NS;
            long submitNs = gridNs + (long) (random.nextGaussian() * 150_000);
            long levelNs = i % 3600 < 1200 ? 3 * MS : 5 * MS;
            long outputNs = submitNs + levelNs + (long) (random.nextGaussian() * 50_000);
            long queuedNs = submitNs + (long) (Math.abs(random.nextGaussian()) * 600_000);
            debt.onOutput(outputNs - queuedNs, outputNs - submitNs, outputNs, FRAME_NS, 0);
            assertFalse("output " + i, debt.shouldDrop());
        }
    }

    @Test public void recentLevelForgetsALightSceneAfterAMinute() {
        DecoderQueuePolicy.RecentLevel level = new DecoderQueuePolicy.RecentLevel();
        long now = 1_000_000_000L;
        level.add(MS, now);
        assertEquals("nothing before a second has ended", Long.MAX_VALUE, level.lowNs());
        for (int i = 0; i < 60 * 10; i++) {
            now += FRAME_NS;
            level.add(i < 60 * 5 ? MS : 4 * MS, now);
        }
        assertEquals(MS, level.lowNs());
        assertEquals(4 * MS, level.lastNs());
        for (int i = 0; i < 60 * 60; i++) {
            now += FRAME_NS;
            level.add(4 * MS, now);
        }
        assertEquals(4 * MS, level.lowNs());
        level.reset();
        assertEquals(Long.MAX_VALUE, level.lowNs());
    }

    @Test public void displayGateWantsASlopeItsInputsCanShow() {
        // A free decoder whose decode noise is five times its input spread reads low slopes now and
        // then: one standard error of 0.25 over ten seconds at 60 fps.
        DecoderQueuePolicy.DisplayGate gate = new DecoderQueuePolicy.DisplayGate();
        Random random = new Random(19);
        long gridNs = 1_000_000_000L;
        for (int i = 0; i < 3600 * 60; i++) {
            gridNs += FRAME_NS;
            long decodeNs = (long) (5 * MS + random.nextGaussian() * MS);
            long submitNs = gridNs + (long) (random.nextGaussian() * 200_000);
            gate.onOutput(decodeNs, submitNs + decodeNs, FRAME_NS, true, false);
            assertFalse("output " + i, gate.held());
        }
    }

    @Test public void displayGateSaysNothingAboutInputsSteadierThanItsResolution() {
        // Outputs on a grid, inputs within 50 us of one: nothing for an output to have followed.
        DecoderQueuePolicy.DisplayGate gate = new DecoderQueuePolicy.DisplayGate();
        Random random = new Random(17);
        long now = 1_000_000_000L;
        for (int i = 0; i < 60 * 60; i++) {
            now += FRAME_NS;
            gate.onOutput(4 * MS + random.nextInt(50_000), now, FRAME_NS, true, false);
            assertFalse("output " + i, gate.held());
        }
    }

    @Test public void displayGateCountsFramesBehindOverEveryOutput() {
        // Every fourth output comes after a burst with a frame behind it: only its steps are large.
        DecoderQueuePolicy.DisplayGate gate = new DecoderQueuePolicy.DisplayGate();
        Random random = new Random(13);
        long now = 1_000_000_000L;
        for (int i = 0; i < 60 * 30; i++) {
            now += FRAME_NS;
            boolean burst = i % 4 == 0;
            gate.onOutput(burst ? 25 * MS : 3 * MS + random.nextInt(1_000_000), now, FRAME_NS, true, burst);
            assertFalse("output " + i, gate.held());
        }
    }

    @Test public void displayGateReadsOutputsThatFollowTheirInputAsFree() {
        DecoderQueuePolicy.DisplayGate gate = new DecoderQueuePolicy.DisplayGate();
        Random random = new Random(11);
        long gridNs = 1_000_000_000L;
        for (int i = 0; i < 60 * 15; i++) {
            gridNs += FRAME_NS;
            long i2oNs = 3 * MS + random.nextInt(500_000);
            gate.onOutput(i2oNs, gridNs + random.nextInt(1_000_000) + i2oNs, FRAME_NS, true, false);
            assertFalse("output " + i, gate.held());
        }
        gate.reset();
        for (int i = 0; i < 60 * 15; i++) {
            gridNs += FRAME_NS;
            gate.onOutput(3 * MS + random.nextInt(1_000_000), gridNs, FRAME_NS, true, false);
        }
        assertTrue(gate.held());
        gate.clear();
        assertFalse(gate.held());
    }

    @Test public void inFlightCountsFramesUntilTheirVsyncPasses() {
        DecoderQueuePolicy.InFlight f = new DecoderQueuePolicy.InFlight();
        f.onReleased(1000, 100 * MS);
        assertEquals(1, f.count(90 * MS));
        assertFalse(f.full(90 * MS));
        f.onReleased(2000, 117 * MS);
        assertTrue("two released, none shown", f.full(90 * MS));
        assertEquals("the first one's vsync passed", 1, f.count(105 * MS));
        assertEquals(0, f.count(120 * MS));
    }

    @Test public void inFlightRenderedFramesAndOlderOnesNoLongerCount() {
        DecoderQueuePolicy.InFlight f = new DecoderQueuePolicy.InFlight();
        f.onReleased(1000, 500 * MS);
        f.onReleased(2000, 517 * MS);
        assertTrue(f.full(400 * MS));
        f.onRendered(1000);
        assertEquals(1, f.count(400 * MS));
        // A later frame being shown means the compositor dropped the earlier one.
        f.onRendered(2000);
        assertEquals(0, f.count(400 * MS));
        f.onReleased(3000, 534 * MS);
        assertEquals(1, f.count(400 * MS));
    }

    @Test public void inFlightKeepsTheNewestFramesWhenMoreAreReleasedThanItHasSlots() {
        DecoderQueuePolicy.InFlight f = new DecoderQueuePolicy.InFlight();
        for (int i = 1; i <= 20; i++) f.onReleased(i * 1000L, 1_000 * MS + i * MS);
        assertEquals(8, f.count(0));
        assertTrue(f.full(0));
        f.reset();
        assertEquals(0, f.count(0));
        f.onReleased(1, 10);
        assertEquals("a reset also forgets what was rendered", 1, f.count(0));
    }

    @Test public void decoderDepthCountsFromTheLastFlushAndIsNeverNegative() {
        assertEquals(3, DecoderQueuePolicy.depth(10, 7, 0));
        // A flush with three frames inside: they are gone, so the count starts again from zero.
        assertEquals(0, DecoderQueuePolicy.depth(10, 7, 3));
        assertEquals(2, DecoderQueuePolicy.depth(12, 7, 3));
        assertEquals("outputs returned for frames from before the flush", 0, DecoderQueuePolicy.depth(10, 9, 3));
    }
}
