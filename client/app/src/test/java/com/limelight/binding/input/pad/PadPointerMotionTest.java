package com.limelight.binding.input.pad;

import com.limelight.binding.input.touch.PointerAcceleration;

import org.junit.Test;

import static org.junit.Assert.*;

/** The pointer touchpad's smoothing, acceleration, carried fraction and right-stick mapping. */
public class PadPointerMotionTest {
    private static final float DENSITY = 2;

    @Test public void slowMovesStayPreciseAndAFastFlickCrossesMore() {
        int[] slow = new int[2], fast = new int[2];
        PadPointerMotion slowTracker = new PadPointerMotion();
        PadPointerMotion fastTracker = new PadPointerMotion();
        // Three samples over 20 ms: 12 px slowly, 120 px quickly. Both end with the same code path.
        for (int i = 0; i < 3; i++) slowTracker.move(4, 0, i * 10_000_000L, DENSITY, slow);
        for (int i = 0; i < 3; i++) fastTracker.move(40, 0, i * 10_000_000L, DENSITY, fast);
        assertTrue("slow keeps some movement", slow[0] > 0);
        assertTrue("a flick goes much further: " + slow[0] + " vs " + fast[0], fast[0] > slow[0] * 8);
        assertTrue("acceleration is allowed to pass the finger's own 40 px", fast[0] > 40);
    }

    @Test public void theSpeedUsesOnlyTheLastThreeSamples() {
        PadPointerMotion tracker = new PadPointerMotion();
        int[] out = new int[2];
        // A huge first sample, then three small ones: the huge one is out of the window.
        tracker.move(1000, 0, 0, DENSITY, out);
        tracker.move(1, 0, 10_000_000L, DENSITY, out);
        tracker.move(1, 0, 20_000_000L, DENSITY, out);
        tracker.move(1, 0, 30_000_000L, DENSITY, out);
        float fresh = tracker.speed();
        PadPointerMotion onlySmall = new PadPointerMotion();
        onlySmall.move(1, 0, 0, DENSITY, out);
        onlySmall.move(1, 0, 10_000_000L, DENSITY, out);
        onlySmall.move(1, 0, 20_000_000L, DENSITY, out);
        assertEquals(onlySmall.speed(), fresh, 1e-5f);
    }

    @Test public void theLeftoverFractionOfAPixelIsCarried() {
        PadPointerMotion tracker = new PadPointerMotion(PointerAcceleration.forCurve(PointerAcceleration.GENTLE));
        int[] out = new int[2];
        int total = 0;
        // Slow, so the gentle curve's 0.8 gain applies: five 1 px moves are four whole pixels, not three or five.
        for (int i = 0; i < 5; i++) {
            tracker.move(1, 0, i * 10_000_000L, DENSITY, out);
            total += out[0];
        }
        assertEquals(4, total);
    }

    @Test public void theCameraMappingHasNoDeadzoneAndReachesFullAtASpeed() {
        PadPointerMotion tracker = new PadPointerMotion();
        int[] out = new int[2];
        tracker.move(.01f, 0, 0, DENSITY, out);
        tracker.move(.01f, 0, 10_000_000L, DENSITY, out);
        tracker.move(.01f, 0, 20_000_000L, DENSITY, out);
        assertTrue("a tiny movement still deflects", tracker.cameraMagnitude() > 0);
        assertTrue(tracker.cameraMagnitude() < .05f);

        tracker.reset();
        tracker.move(100, 0, 0, DENSITY, out);
        tracker.move(100, 0, 10_000_000L, DENSITY, out);
        tracker.move(100, 0, 20_000_000L, DENSITY, out);
        assertTrue("a fast flick is nearly full", tracker.cameraMagnitude() > .9f);
        assertTrue(tracker.cameraX() > 0);
        assertEquals(0, tracker.cameraY(), 1e-5f);

        tracker.reset();
        tracker.move(0, 100, 0, DENSITY, out);
        tracker.move(0, 100, 10_000_000L, DENSITY, out);
        tracker.move(0, 100, 20_000_000L, DENSITY, out);
        assertTrue("a finger moving down looks down", tracker.cameraY() < -.9f);
    }

    @Test public void theCameraReturnsToTheCenterWithinFiftyMilliseconds() {
        PadPointerMotion tracker = new PadPointerMotion();
        int[] out = new int[2];
        for (int i = 0; i < 3; i++) tracker.move(100, 100, i * 10_000_000L, DENSITY, out);
        assertTrue(tracker.cameraMagnitude() > .9f);
        tracker.tick(20_000_000L);
        assertTrue("still partly held after 20 ms", tracker.cameraMagnitude() > .1f);
        tracker.tick(30_000_000L);
        assertTrue("gone within 50 ms", tracker.cameraMagnitude() < .05f);
        tracker.tick(100_000_000L);
        assertEquals(0, tracker.cameraMagnitude(), 1e-6f);
        assertEquals(0, tracker.cameraX(), 0);
        assertEquals(0, tracker.cameraY(), 0);
    }

    @Test public void repeatedTimesAndOutOfOrderSamplesDoNotBreakTheSpeed() {
        PadPointerMotion tracker = new PadPointerMotion();
        int[] out = new int[2];
        tracker.move(5, 0, 10_000_000L, DENSITY, out);
        tracker.move(5, 0, 10_000_000L, DENSITY, out);
        tracker.move(5, 0, 5_000_000L, DENSITY, out);
        assertTrue(Float.isFinite(tracker.speed()));
        assertFalse(Float.isNaN(tracker.cameraX()) || Float.isNaN(tracker.cameraY()));
    }

    @Test public void stopCameraCentersItAtOnce() {
        PadPointerMotion tracker = new PadPointerMotion();
        int[] out = new int[2];
        for (int i = 0; i < 3; i++) tracker.move(100, 0, i * 10_000_000L, DENSITY, out);
        assertTrue(tracker.cameraMagnitude() > 0);
        tracker.stopCamera();
        assertEquals(0, tracker.cameraMagnitude(), 0);
    }
}
