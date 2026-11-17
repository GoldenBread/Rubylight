package com.limelight.binding.input.touch;

/**
 * Trackpad pointer acceleration: slow finger movement is scaled down for precision and fast
 * movement is scaled up, with a smooth ramp in between. The speed behind the gain is averaged over
 * the last few moves by their event timestamps, so one jittery sample does not jump the gain. A
 * gesture starts at the slow end. Speeds are in dp per second.
 */
public final class PointerAcceleration {
    public static final String GENTLE = "gentle";
    public static final String MEDIUM = "medium";
    public static final String STRONG = "strong";
    /**
     * No acceleration: every move passes through as it is, as the screen touchpad does with Trackpad acceleration
     * off. The speed is still measured, for a mapping that needs it (the camera stick).
     */
    public static final String LINEAR = "linear";

    static final float SLOW_SPEED = 50f;
    static final float FAST_SPEED = 1500f;
    /**
     * Moves averaged into the speed: enough to smooth a touch screen's jitter, short enough to
     * follow a real change of pace.
     */
    static final int WINDOW_MOVES = 3;
    /**
     * A move after this long without one starts the speed over: what came before says nothing. The
     * window is wide enough for three 17 ms moves, which is what a 60 Hz panel reports in whole
     * milliseconds, so the average keeps all three there.
     */
    static final long WINDOW_NS = 60_000_000L;
    // Event times are whole milliseconds; shorter gaps would turn rounding into huge speeds.
    static final long MIN_INTERVAL_NS = 4_000_000L;

    private final float slowGain;
    private final float fastGain;

    // The last WINDOW_MOVES moves as (distance in dp, time since the previous move in ns).
    private final float[] distances = new float[WINDOW_MOVES];
    private final long[] intervals = new long[WINDOW_MOVES];
    private int samples;
    private float windowDistance;
    private long windowTime;
    private long lastEventTime = -1;
    private float pendingDistance;
    private float gain;
    private float speed;

    private PointerAcceleration(float slowGain, float fastGain) {
        this.slowGain = slowGain;
        this.fastGain = fastGain;
        this.gain = slowGain;
    }

    public static PointerAcceleration forCurve(String curve) {
        if (LINEAR.equals(curve)) return new PointerAcceleration(1f, 1f);
        if (GENTLE.equals(curve)) return new PointerAcceleration(0.8f, 1.6f);
        if (STRONG.equals(curve)) return new PointerAcceleration(0.5f, 3.0f);
        return new PointerAcceleration(0.6f, 2.2f);
    }

    /** The gain of the smooth ramp at a finger speed in dp per second. */
    public float gainForSpeed(float speed) {
        if (!(speed > SLOW_SPEED)) return slowGain;
        if (speed >= FAST_SPEED) return fastGain;
        // Smootherstep, in double so the flat ends of the ramp cannot round the gain back down:
        // the gain also levels off where the ramp begins and ends, so no pace makes it jump.
        double t = (speed - SLOW_SPEED) / (double) (FAST_SPEED - SLOW_SPEED);
        double ramp = t * t * t * (t * (t * 6 - 15) + 10);
        float gain = (float) (slowGain + (fastGain - slowGain) * ramp);
        return Math.max(slowGain, Math.min(fastGain, gain));
    }

    /** Forgets the moves of the gesture that just ended, so the next one starts at the slow end. */
    public void reset() {
        samples = 0;
        windowDistance = 0;
        windowTime = 0;
        lastEventTime = -1;
        pendingDistance = 0;
        gain = slowGain;
        speed = 0;
    }

    /**
     * The smoothed finger speed in dp per second behind the last {@link #gain}: what a mapping that is not
     * a pointer gain, such as the on-screen controls' camera stick, needs. Zero before the first move and
     * after {@link #reset}.
     */
    public float speed() {
        return speed;
    }

    /**
     * Records a move of (dx, dy) pixels at eventTimeNs, measured against the screen's density, and
     * returns the gain for that move.
     */
    public float gain(float dx, float dy, long eventTimeNs, float density) {
        if (!(density > 0)) density = 1;
        float distance = (float) Math.sqrt(dx * dx + dy * dy) / density;
        if (!(distance >= 0) || distance > Float.MAX_VALUE) {
            // A NaN or infinite delta must not poison the gain for the rest of the gesture.
            distance = 0;
        }
        if (lastEventTime < 0) {
            // The first move has no interval behind it: start at the slow end.
            lastEventTime = eventTimeNs;
            gain = slowGain;
            speed = 0;
            return gain;
        }
        long interval = eventTimeNs - lastEventTime;
        if (interval <= 0) {
            // Moves with one timestamp add up into the next measurable interval.
            pendingDistance += distance;
            return gain;
        }
        lastEventTime = eventTimeNs;
        if (interval > WINDOW_NS) {
            // The finger had stopped or the timestamps jumped: the old moves say nothing about
            // this one, so the speed starts over.
            samples = 0;
            windowDistance = 0;
            windowTime = 0;
            pendingDistance = 0;
            gain = slowGain;
            speed = 0;
            return gain;
        }
        add(distance + pendingDistance, interval);
        pendingDistance = 0;
        speed = windowTime > 0 ? windowDistance * 1e9f / windowTime : 0f;
        gain = gainForSpeed(speed);
        return gain;
    }

    private void add(float distance, long interval) {
        // A same-millisecond sample still means some interval; shorter gaps divide by rounding.
        if (interval < MIN_INTERVAL_NS) interval = MIN_INTERVAL_NS;
        if (samples == WINDOW_MOVES) {
            windowDistance -= distances[0];
            windowTime -= intervals[0];
            System.arraycopy(distances, 1, distances, 0, WINDOW_MOVES - 1);
            System.arraycopy(intervals, 1, intervals, 0, WINDOW_MOVES - 1);
            samples--;
        }
        distances[samples] = distance;
        intervals[samples] = interval;
        windowDistance += distance;
        windowTime += interval;
        samples++;
        // Older moves than the window would make the speed lag behind a change of pace.
        while (samples > 1 && windowTime > WINDOW_NS) {
            windowDistance -= distances[0];
            windowTime -= intervals[0];
            System.arraycopy(distances, 1, distances, 0, WINDOW_MOVES - 1);
            System.arraycopy(intervals, 1, intervals, 0, WINDOW_MOVES - 1);
            samples--;
        }
    }
}
