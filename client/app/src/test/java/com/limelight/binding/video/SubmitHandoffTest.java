package com.limelight.binding.video;

import static org.junit.Assert.assertEquals;
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

    private final List<Integer> submitted = new CopyOnWriteArrayList<>();
    private final CountDownLatch release = new CountDownLatch(1);
    private final CountDownLatch started = new CountDownLatch(1);

    private SubmitHandoff<Integer> holdingWorker(int bound, int failingUnit) {
        handoff = new SubmitHandoff<>(bound, "test-submit", unit -> {
            started.countDown();
            try {
                release.await(10, TimeUnit.SECONDS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
            submitted.add(unit);
            return unit == failingUnit ? MoonBridge.DR_NEED_IDR : MoonBridge.DR_OK;
        });
        return handoff;
    }

    private int submit(int unit, boolean ready, List<Integer> inPlace) {
        return handoff.submit(() -> ready, () -> {
            inPlace.add(unit);
            return MoonBridge.DR_OK;
        }, () -> unit, 1);
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
    public void aQueuedUnitThatFailsAsksForAKeyFrameWithTheNextOne() throws Exception {
        holdingWorker(4, 1);
        List<Integer> inPlace = new CopyOnWriteArrayList<>();
        assertEquals(MoonBridge.DR_OK, submit(1, false, inPlace));
        release.countDown();
        awaitSize(submitted, 1);
        long deadline = System.nanoTime() + 10_000_000_000L;
        while (handoff.drops() == 0 && System.nanoTime() < deadline) Thread.sleep(1);
        assertEquals(1, handoff.drops());
        assertEquals(MoonBridge.DR_NEED_IDR, submit(2, true, inPlace));
        // Asked once: the unit after it is as any other.
        assertEquals(MoonBridge.DR_OK, submit(3, true, inPlace));
        assertEquals(List.of(2, 3), inPlace);
    }
}
