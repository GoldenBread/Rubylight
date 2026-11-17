package com.limelight.binding.input.touch;

import com.limelight.binding.input.MouseDeltaAccumulator;

import org.junit.Test;

import static org.junit.Assert.*;

public class PointerAccelerationTest {
    private static final String[] CURVES = {PointerAcceleration.GENTLE, PointerAcceleration.MEDIUM, PointerAcceleration.STRONG};
    /** 125 Hz touch sampling, the rate most phone screens report while a finger moves. */
    private static final long FRAME_NS = 8_000_000L;

    /** Feeds one move of distanceDp pixels every frame and returns the gain of the last move. */
    private static float feed(PointerAcceleration acceleration, float distanceDp, long startNs, int moves) {
        float gain = 0;
        for (int i = 0; i < moves; i++) {
            gain = acceleration.gain(distanceDp, 0, startNs + i * FRAME_NS, 1);
        }
        return gain;
    }

    @Test
    public void gainNeverDropsAsTheFingerSpeedsUp() {
        for (String curve : CURVES) {
            PointerAcceleration acceleration = PointerAcceleration.forCurve(curve);
            float previous = acceleration.gainForSpeed(0);
            for (float speed = 0; speed <= 3000; speed += 1) {
                float gain = acceleration.gainForSpeed(speed);
                assertTrue(curve + " at " + speed, gain >= previous - 1e-6f);
                previous = gain;
            }
        }
    }

    @Test
    public void slowMovesArePreciseAndFastMovesTravelFarther() {
        for (String curve : CURVES) {
            PointerAcceleration acceleration = PointerAcceleration.forCurve(curve);
            assertTrue(curve, acceleration.gainForSpeed(0) < 1);
            assertTrue(curve, acceleration.gainForSpeed(10_000) > 1);
            assertEquals(curve, acceleration.gainForSpeed(PointerAcceleration.SLOW_SPEED), acceleration.gainForSpeed(0), 0);
            assertEquals(curve, acceleration.gainForSpeed(PointerAcceleration.FAST_SPEED), acceleration.gainForSpeed(100_000), 0);
        }
    }

    @Test
    public void strongerCurvesReachFurther() {
        float gentle = PointerAcceleration.forCurve(PointerAcceleration.GENTLE).gainForSpeed(5000);
        float medium = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM).gainForSpeed(5000);
        float strong = PointerAcceleration.forCurve(PointerAcceleration.STRONG).gainForSpeed(5000);
        assertTrue(gentle < medium && medium < strong);
    }

    @Test
    public void rampHasNoJumps() {
        PointerAcceleration acceleration = PointerAcceleration.forCurve(PointerAcceleration.STRONG);
        float previous = acceleration.gainForSpeed(0);
        for (float speed = 0; speed <= 2000; speed += 0.5f) {
            float gain = acceleration.gainForSpeed(speed);
            assertTrue(Math.abs(gain - previous) < 0.01f);
            previous = gain;
        }
    }

    @Test
    public void unknownCurveIsMediumAndBadSpeedsAreSlow() {
        assertEquals(PointerAcceleration.forCurve(PointerAcceleration.MEDIUM).gainForSpeed(1000),
                PointerAcceleration.forCurve(null).gainForSpeed(1000), 0);
        PointerAcceleration acceleration = PointerAcceleration.forCurve("unknown");
        assertEquals(acceleration.gainForSpeed(0), acceleration.gainForSpeed(Float.NaN), 0);
        assertEquals(acceleration.gainForSpeed(0), acceleration.gainForSpeed(-5), 0);
        assertEquals(acceleration.gainForSpeed(PointerAcceleration.FAST_SPEED),
                acceleration.gainForSpeed(Float.POSITIVE_INFINITY), 0);
    }

    @Test
    public void speedIsAveragedOverTheLastFewMovesByTime() {
        PointerAcceleration acceleration = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        // 4 dp every 8 ms is 500 dp/s; the first move only starts the clock.
        assertEquals(acceleration.gainForSpeed(500), feed(acceleration, 4, 0, 3), 1e-4f);

        // A dense screen reports twice the pixels for the same dp: the same gain.
        PointerAcceleration dense = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        float gain = 0;
        for (int i = 0; i < 3; i++) {
            gain = dense.gain(8, 0, i * FRAME_NS, 2);
        }
        assertEquals(acceleration.gainForSpeed(500), gain, 1e-4f);
    }

    @Test
    public void speedComesFromTimestampsNotFromTheNumberOfSamples() {
        // 750 dp/s on a 60 Hz screen (12 dp every 16 ms) and on a 250 Hz screen (3 dp every 4 ms).
        PointerAcceleration slowSamples = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        PointerAcceleration fastSamples = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        float slowGain = 0, fastGain = 0;
        for (int i = 0; i < 4; i++) {
            slowGain = slowSamples.gain(12, 0, i * 16_000_000L, 1);
            fastGain = fastSamples.gain(3, 0, i * 4_000_000L, 1);
        }
        assertEquals(slowSamples.gainForSpeed(750), slowGain, 1e-4f);
        assertEquals(slowSamples.gainForSpeed(750), fastGain, 1e-4f);
    }

    @Test
    public void oneJitterySampleDoesNotJumpTheGain() {
        PointerAcceleration burst = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        // 3 dp, 3 dp, 3 dp.
        float even = feed(burst, 3, 0, 4);
        PointerAcceleration jolted = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        float jolt = 0;
        for (int i = 0; i < 4; i++) {
            // 9 dp in one sample, then two still samples: the same distance over the same time.
            jolt = jolted.gain(i == 1 ? 9 : 0, 0, i * FRAME_NS, 1);
        }
        assertEquals(even, jolt, 1e-4f);
        // The average decides, not the jolted sample: 9 dp in 8 ms would be a much faster gain.
        assertTrue(jolt < jolted.gainForSpeed(9 * 1e9f / FRAME_NS));
    }

    @Test
    public void samplesOlderThanTheWindowAreNotUsed() {
        PointerAcceleration acceleration = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        // A fast swipe, then the finger rests for longer than the window.
        assertEquals(acceleration.gainForSpeed(PointerAcceleration.FAST_SPEED), feed(acceleration, 40, 0, 4), 1e-4f);
        long rest = 4 * FRAME_NS + PointerAcceleration.WINDOW_NS + FRAME_NS;
        assertEquals(acceleration.gainForSpeed(0), acceleration.gain(40, 0, rest, 1), 0);
        // The next move is measured on its own: 4 dp in 8 ms.
        assertEquals(acceleration.gainForSpeed(500), acceleration.gain(4, 0, rest + FRAME_NS, 1), 1e-4f);
    }

    @Test
    public void sixtyHertzSamplesKeepAllThreeMovesInTheWindow() {
        PointerAcceleration acceleration = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        // A 60 Hz panel reports whole milliseconds that alternate 16 and 17: three 17 ms moves must
        // all stay in the window, so the average is 12 dp in 51 ms, not 10 dp in 34 ms.
        acceleration.gain(1, 0, 0, 1);
        float gain = 0;
        float[] distances = {2, 4, 6};
        for (int i = 0; i < distances.length; i++) {
            gain = acceleration.gain(distances[i], 0, (i + 1) * 17_000_000L, 1);
        }
        assertEquals(acceleration.gainForSpeed(12 * 1e9f / 51_000_000L), gain, 1e-4f);
    }

    @Test
    public void intervalsBelowFourMillisecondsCountAsFour() {
        PointerAcceleration acceleration = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        acceleration.gain(0, 0, 0, 1);
        float gain = 0;
        // Whole-millisecond rounding makes 1 ms gaps unreliable; each counts as 4 ms (250 dp/s).
        for (long gap : new long[] {1, 2, 3}) {
            gain = acceleration.gain(1, 0, gap * 1_000_000L, 1);
        }
        assertEquals(acceleration.gainForSpeed(250), gain, 1e-4f);
    }

    @Test
    public void zeroAndHugeIntervalsAreSafe() {
        PointerAcceleration acceleration = PointerAcceleration.forCurve(PointerAcceleration.STRONG);
        acceleration.gain(2, 0, 0, 1);
        // Moves with one timestamp add up instead of dividing by zero.
        float sameTime = acceleration.gain(30, 0, 0, 1);
        assertEquals(acceleration.gainForSpeed(0), sameTime, 0);
        assertTrue(Float.isFinite(sameTime));

        // A second without a move starts the speed over rather than counting as a slow swipe.
        float afterRest = acceleration.gain(30, 0, 1_000_000_000L, 1);
        assertEquals(acceleration.gainForSpeed(0), afterRest, 0);

        // A timestamp that goes backwards is not a negative interval.
        float backwards = acceleration.gain(30, 0, 1_000_000_000L - FRAME_NS, 1);
        assertTrue(Float.isFinite(backwards));
        assertTrue(backwards >= acceleration.gainForSpeed(0) && backwards <= acceleration.gainForSpeed(100_000));

        // A NaN delta and a missing density cannot poison the gain.
        float bad = acceleration.gain(Float.NaN, 0, 1_000_000_000L + FRAME_NS, 0);
        assertTrue(Float.isFinite(bad));
        assertTrue(bad >= acceleration.gainForSpeed(0) && bad <= acceleration.gainForSpeed(100_000));
    }

    @Test
    public void aNewGestureStartsAtTheSlowEnd() {
        PointerAcceleration acceleration = PointerAcceleration.forCurve(PointerAcceleration.STRONG);
        assertEquals(acceleration.gainForSpeed(PointerAcceleration.FAST_SPEED), feed(acceleration, 40, 0, 4), 1e-4f);
        acceleration.reset();
        assertEquals(acceleration.gainForSpeed(0), acceleration.gain(40, 0, 10 * FRAME_NS, 1), 0);
        assertEquals("A reset gesture has no speed behind it", 0, acceleration.speed(), 0);
        assertEquals(acceleration.gainForSpeed(500), acceleration.gain(4, 0, 11 * FRAME_NS, 1), 1e-4f);
    }

    /** The camera stick maps the same smoothed speed the pointer's gain uses, so it is exposed in dp per second. */
    @Test
    public void theSpeedBehindTheGainIsDistanceOverTimeInDpPerSecond() {
        PointerAcceleration acceleration = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        assertEquals(0, acceleration.speed(), 0);
        acceleration.gain(4, 0, 0, 1);
        assertEquals("The first move only starts the clock", 0, acceleration.speed(), 0);
        acceleration.gain(4, 0, FRAME_NS, 1);
        assertEquals(4 * 1e9f / FRAME_NS, acceleration.speed(), 1e-3f);
        acceleration.gain(4, 0, 2 * FRAME_NS, 1);
        assertEquals("Two moves of 4 dp over two 8 ms intervals", 8 * 1e9f / (2 * FRAME_NS),
                acceleration.speed(), 1e-3f);
        float gain = acceleration.gain(0, 0, 3 * FRAME_NS, 1);
        assertEquals("The gain is for that speed", acceleration.gainForSpeed(acceleration.speed()), gain, 1e-4f);
        acceleration.reset();
        assertEquals(0, acceleration.speed(), 0);
    }

    @Test
    public void slowSubPixelMovesKeepTheirRemainder() {
        PointerAcceleration acceleration = PointerAcceleration.forCurve(PointerAcceleration.MEDIUM);
        MouseDeltaAccumulator accumulator = new MouseDeltaAccumulator();
        int total = 0;
        for (int i = 0; i < 100; i++) {
            float gain = acceleration.gain(0.3f, 0, i * FRAME_NS, 1);
            total += accumulator.scale(0.3f, gain);
        }
        // 100 moves of 0.3 px at the slow gain of 0.6: 18 px, carried rather than lost.
        assertEquals(18, total);
    }

    @Test
    public void withoutAccelerationTheSameMovesGoThroughUnscaled() {
        MouseDeltaAccumulator accumulator = new MouseDeltaAccumulator();
        int total = 0;
        for (int i = 0; i < 100; i++) {
            total += accumulator.scale(0.3f, 1);
        }
        assertEquals(30, total);
        assertEquals(new MouseDeltaAccumulator().scale(15, 1.3f), 20);
    }
}
