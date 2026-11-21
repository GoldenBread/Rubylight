package com.limelight.binding.video;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.limelight.nvstream.jni.MoonBridge;

import org.junit.After;
import org.junit.Test;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** The queue semantics of the hand-off, with a worker the test holds. */
public class SubmitHandoffTest {
    private SubmitHandoff<Integer> handoff;

    @After
    public void tearDown() {
        if (handoff != null) handoff.close(2000);
    }

    private volatile String initThread;
    private volatile int throwingUnit = -1;
    private final List<Integer> submitted = new CopyOnWriteArrayList<>();
    private final CountDownLatch release = new CountDownLatch(1);
    private final CountDownLatch started = new CountDownLatch(1);

    private SubmitHandoff<Integer> holdingWorker(int bound, int failingUnit) {
        handoff = new SubmitHandoff<>(bound, "test-submit", () -> initThread = Thread.currentThread().getName(), unit -> {
            started.countDown();
            try {
                release.await(10, TimeUnit.SECONDS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
            submitted.add(unit);
            if (unit == throwingUnit) throw new IllegalStateException("the codec threw");
            return unit == failingUnit ? MoonBridge.DR_NEED_IDR : MoonBridge.DR_OK;
        });
        return handoff;
    }

    private int submit(int unit, boolean ready, List<Integer> inPlace) {
        return submit(unit, ready, inPlace, false);
    }

    private int submit(int unit, boolean ready, List<Integer> inPlace, boolean key) {
        return submitOf(unit, unit, ready, inPlace, key);
    }

    /** A unit of {@code frame}, as a key frame's parameter sets and picture are several. */
    private int submitOf(int unit, int frame, boolean ready, List<Integer> inPlace, boolean key) {
        return handoff.submit(() -> ready, () -> {
            inPlace.add(unit);
            return MoonBridge.DR_OK;
        }, () -> unit, 1, key, frame);
    }

    private static void awaitSize(List<Integer> list, int size) throws InterruptedException {
        long deadline = System.nanoTime() + 10_000_000_000L;
        while (list.size() < size && System.nanoTime() < deadline) Thread.sleep(1);
        assertEquals(size, list.size());
    }

    @Test
    public void aDecoderWithABufferTakesTheUnitInPlaceOnTheCallingThread() {
        holdingWorker(4, -1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, true, inPlace));
        assertEquals(List.of(1), inPlace);
        assertTrue(submitted.isEmpty());
        assertEquals(0, handoff.drops());
    }

    @Test
    public void aStalledDecoderLeavesTheCallAtOnceAndTheWorkerSubmitsInOrder() throws Exception {
        holdingWorker(4, -1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        assertTrue(started.await(10, TimeUnit.SECONDS));
        // The worker has unit 1 in hand and the decoder would now take one: it still goes behind it.
        assertEquals(MoonBridge.DR_OK, submit(2, true, inPlace));
        assertEquals(MoonBridge.DR_OK, submit(3, false, inPlace));
        assertTrue("nothing ran on the calling thread", inPlace.isEmpty());
        release.countDown();
        awaitSize(submitted, 3);
        assertEquals(List.of(1, 2, 3), submitted);
        assertEquals(0, handoff.drops());
    }

    @Test
    public void aFullQueueDropsWhatWaitsAndAsksForAKeyFrame() throws Exception {
        holdingWorker(2, -1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        assertTrue(started.await(10, TimeUnit.SECONDS));
        assertEquals(MoonBridge.DR_OK, submit(2, false, inPlace));
        assertEquals(MoonBridge.DR_OK, submit(3, false, inPlace));
        // Over the bound: the two waiting and this one are dropped, as the decode unit queue does.
        assertEquals(MoonBridge.DR_NEED_IDR, submit(4, false, inPlace));
        assertEquals(3, handoff.drops());
        release.countDown();
        awaitSize(submitted, 1);
        assertEquals(List.of(1), submitted);
    }

    @Test
    public void aFullQueueAlsoDropsTheUnitsAfterItUpToAKeyFrame() throws Exception {
        holdingWorker(1, -1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        assertTrue(started.await(10, TimeUnit.SECONDS));
        assertEquals(MoonBridge.DR_OK, submit(2, false, inPlace));
        assertEquals(MoonBridge.DR_NEED_IDR, submit(3, false, inPlace));
        release.countDown();
        // The engine was told at once; what follows is dropped, not given to the decoder.
        assertEquals(MoonBridge.DR_OK, submit(4, true, inPlace));
        assertTrue(inPlace.isEmpty());
        assertEquals(3, handoff.drops());
        assertTrue(handoff.takeKeyGap());
        assertFalse("taken once", handoff.takeKeyGap());
        // The key frame's first unit is taken again, whatever it is told.
        assertEquals(MoonBridge.DR_OK, submit(5, true, inPlace, true));
        assertEquals(MoonBridge.DR_OK, submit(6, true, inPlace));
        assertEquals(List.of(5, 6), inPlace);
    }

    private void awaitFailure() throws Exception {
        long deadline = System.nanoTime() + 10_000_000_000L;
        while (!handoff.takeKeyGapPeek() && System.nanoTime() < deadline) Thread.sleep(1);
    }

    @Test
    public void aQueuedUnitThatFailsDropsTheRestOfTheQueueAndAsksForAKeyFrameOnce() throws Exception {
        holdingWorker(4, 1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        assertTrue(started.await(10, TimeUnit.SECONDS));
        assertEquals(MoonBridge.DR_OK, submit(2, false, inPlace));
        assertEquals(MoonBridge.DR_OK, submit(3, false, inPlace));
        release.countDown();
        awaitFailure();
        // Units 2 and 3 never reach the decoder: they would reference the frame that failed.
        assertEquals(2, handoff.drops());
        assertEquals(List.of(1), submitted);
        // The next one is dropped with the news, though the decoder would take it.
        assertEquals(MoonBridge.DR_NEED_IDR, submit(4, true, inPlace));
        assertEquals(MoonBridge.DR_OK, submit(5, true, inPlace));
        assertTrue(inPlace.isEmpty());
        assertEquals(4, handoff.drops());
        // A key frame in progress is not told to abandon itself, and ends the wait.
        assertEquals(MoonBridge.DR_OK, submit(6, true, inPlace, true));
        assertEquals(MoonBridge.DR_OK, submit(7, true, inPlace));
        assertEquals(List.of(6, 7), inPlace);
    }

    @Test
    public void aKeyFrameArrivingBeforeTheNewsEndsTheWaitWithoutNeedIdr() throws Exception {
        holdingWorker(4, 1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        release.countDown();
        awaitFailure();
        assertEquals(MoonBridge.DR_OK, submit(2, true, inPlace, true));
        assertEquals(MoonBridge.DR_OK, submit(3, true, inPlace));
        assertEquals(List.of(2, 3), inPlace);
    }

    @Test
    public void theRestOfAKeyFrameThatLostAUnitIsDroppedAndTheNextKeyFrameIsTaken() throws Exception {
        holdingWorker(1, -1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        assertTrue(started.await(10, TimeUnit.SECONDS));
        assertEquals(MoonBridge.DR_OK, submit(2, false, inPlace));
        // The key frame 10's first unit overflows: dropped, and the engine is told.
        assertEquals(MoonBridge.DR_NEED_IDR, submitOf(30, 10, false, inPlace, true));
        release.countDown();
        // Its other units, if they come anyway, would be a picture without parameter sets.
        assertEquals(MoonBridge.DR_OK, submitOf(31, 10, true, inPlace, true));
        assertEquals(MoonBridge.DR_OK, submitOf(32, 10, true, inPlace, true));
        assertTrue(inPlace.isEmpty());
        // A different key frame ends the wait (taken in place once the worker is idle).
        long deadline = System.nanoTime() + 10_000_000_000L;
        while (handoff.busy() && System.nanoTime() < deadline) Thread.sleep(1);
        assertEquals(MoonBridge.DR_OK, submitOf(40, 11, true, inPlace, true));
        assertEquals(MoonBridge.DR_OK, submitOf(41, 11, true, inPlace, true));
        assertEquals(List.of(40, 41), inPlace);
    }

    @Test
    public void aKeyFrameThatWaitedInTheQueueWhenTheWorkerFailedIsLostWhole() throws Exception {
        holdingWorker(4, 1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        assertTrue(started.await(10, TimeUnit.SECONDS));
        assertEquals(MoonBridge.DR_OK, submitOf(30, 10, false, inPlace, true));
        release.countDown();
        awaitFailure();
        // Frame 10's parameter sets were dropped from the queue; its picture must not reach the decoder.
        assertEquals(MoonBridge.DR_NEED_IDR, submitOf(31, 10, true, inPlace, true));
        assertEquals(MoonBridge.DR_OK, submitOf(50, 12, true, inPlace, true));
        assertEquals(List.of(50), inPlace);
    }

    @Test
    public void aWorkerThatThrowsKeepsRunningAndCountsTheUnitAsFailed() throws Exception {
        throwingUnit = 1;
        holdingWorker(4, -1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        release.countDown();
        awaitFailure();
        assertEquals(MoonBridge.DR_NEED_IDR, submit(2, true, inPlace));
        // The stream goes on from a key frame, through the same worker.
        assertEquals(MoonBridge.DR_OK, submit(3, false, inPlace, true));
        awaitSize(submitted, 2);
        assertEquals(List.of(1, 3), submitted);
    }

    @Test
    public void aUnitThatCouldNotBeCopiedIsDroppedAndAsksForAKeyFrame() {
        holdingWorker(4, -1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_NEED_IDR, handoff.submit(() -> false, () -> MoonBridge.DR_OK, () -> null, 1, false, 7));
        assertEquals(1, handoff.drops());
        assertEquals(MoonBridge.DR_OK, submit(8, true, inPlace));
        assertTrue(inPlace.isEmpty());
        assertTrue(handoff.takeKeyGap());
    }

    @Test
    public void onlyTheWorkerThreadKnowsItself() throws Exception {
        holdingWorker(4, -1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertFalse(handoff.onWorkerThread());
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        assertTrue(started.await(10, TimeUnit.SECONDS));
        assertFalse(handoff.onWorkerThread());
        release.countDown();
    }

    @Test
    public void theWorkerRunsTheThreadInitAndCloseWaitsForIt() throws Exception {
        holdingWorker(4, -1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        assertTrue(started.await(10, TimeUnit.SECONDS));
        assertEquals("test-submit", initThread);
        // Busy with its unit: not ended within the wait.
        assertFalse(handoff.close(50));
        release.countDown();
        assertTrue(handoff.close(10_000));
    }
}
