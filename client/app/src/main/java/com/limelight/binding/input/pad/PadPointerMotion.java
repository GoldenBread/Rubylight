package com.limelight.binding.input.pad;

import com.limelight.binding.input.MouseDeltaAccumulator;
import com.limelight.binding.input.touch.PointerAcceleration;

/**
 * A finger on the on-screen controller's touchpad, as mouse movement or as a right stick. The speed used for
 * acceleration is smoothed over the last three samples by their event times, so one jittery report neither spikes
 * nor stalls the pointer; slow movements stay precise and a fast flick crosses more of the screen. Mouse movement
 * keeps the leftover fraction of a pixel for the next report ({@link MouseDeltaAccumulator}), and the camera's
 * deflection is deadzone-free and returns to the center within about 50 ms after the finger stops.
 *
 * <p>Plain Java, no Android types, so the mapping is tested without a device.
 */
public final class PadPointerMotion {
    /** The last three moves smooth the speed, as the task's pointer model asks. */
    public static final int SMOOTHING_SAMPLES = 3;
    /** The finger speed, in dp per millisecond, that fully deflects the camera stick. */
    static final float CAMERA_FULL_SPEED = .30f;
    /** The camera's deflection falls by e every this many milliseconds without movement. */
    static final float CAMERA_DECAY_TAU_MS = 15f;
    /** Below this share of full deflection the camera is centered and the decay stops. */
    static final float CAMERA_STOP = .02f;

    private final float[] sampleDx = new float[SMOOTHING_SAMPLES], sampleDy = new float[SMOOTHING_SAMPLES];
    private final long[] sampleTimeNs = new long[SMOOTHING_SAMPLES];
    private int samples, next;
    // A fresh pair starts each touch, so no fraction of the last gesture's pixel is carried into the next.
    private MouseDeltaAccumulator deltaX = new MouseDeltaAccumulator(), deltaY = new MouseDeltaAccumulator();
    private final PointerAcceleration acceleration;
    private float speed, directionX, directionY;
    private float cameraX, cameraY;

    public PadPointerMotion() {
        this(PointerAcceleration.forCurve(PointerAcceleration.MEDIUM));
    }

    public PadPointerMotion(PointerAcceleration acceleration) {
        this.acceleration = acceleration;
    }

    /**
     * Adds a finger move of ({@code dx}, {@code dy}) pixels at {@code eventTimeNs} and writes the pointer's pixel
     * movement into {@code out[0]} and {@code out[1]}. The fraction of a pixel that does not fit is carried.
     *
     * @param density the display's density, so speed is measured in dp as the acceleration curve expects
     */
    public void move(float dx, float dy, long eventTimeNs, float density, int[] out) {
        addSample(dx, dy, eventTimeNs);
        float spanMs = (sampleTimeNs[(next - 1 + SMOOTHING_SAMPLES) % SMOOTHING_SAMPLES] - oldestTimeNs()) / 1_000_000f;
        float sumX = 0, sumY = 0;
        for (int i = 0; i < samples; i++) {
            sumX += sampleDx[i];
            sumY += sampleDy[i];
        }
        speed = PointerAcceleration.speed(sumX, sumY, Math.round(spanMs), density);
        float length = (float) Math.hypot(sumX, sumY);
        directionX = length > 0 ? sumX / length : 0;
        directionY = length > 0 ? sumY / length : 0;
        float gain = acceleration.gain(speed);
        out[0] = deltaX.scale(dx, gain);
        out[1] = deltaY.scale(dy, gain);
        updateCamera();
    }

    /** The smoothed finger speed in dp per millisecond after the last {@link #move}. */
    public float speed() {
        return speed;
    }

    /** The smoothed movement direction, a unit vector; (0, 0) when nothing moved. */
    public float directionX() {
        return directionX;
    }

    public float directionY() {
        return directionY;
    }

    /** The camera stick's deflection from the last movement, x right and y up positive. */
    public float cameraX() {
        return cameraX;
    }

    public float cameraY() {
        return cameraY;
    }

    public float cameraMagnitude() {
        return (float) Math.hypot(cameraX, cameraY);
    }

    /**
     * Ages the camera's deflection by {@code elapsedNs} without a movement: it decays to the center within about
     * 50 ms after the finger stops, and stops entirely once it is imperceptible.
     */
    public void tick(long elapsedNs) {
        if (elapsedNs <= 0 || (cameraX == 0 && cameraY == 0)) return;
        float factor = (float) Math.exp(-(elapsedNs / 1_000_000f) / CAMERA_DECAY_TAU_MS);
        cameraX *= factor;
        cameraY *= factor;
        if (cameraMagnitude() < CAMERA_STOP) {
            cameraX = 0;
            cameraY = 0;
        }
    }

    /** Centers the camera at once, for a cancelled touch or a second finger landing. */
    public void stopCamera() {
        cameraX = 0;
        cameraY = 0;
    }

    /** Forgets every sample and the camera, as a new touch does. */
    public void reset() {        samples = 0;
        next = 0;
        speed = 0;
        directionX = 0;
        directionY = 0;
        cameraX = 0;
        cameraY = 0;
        deltaX = new MouseDeltaAccumulator();
        deltaY = new MouseDeltaAccumulator();
    }

    private void addSample(float dx, float dy, long timeNs) {
        sampleDx[next] = dx;
        sampleDy[next] = dy;
        sampleTimeNs[next] = timeNs;
        next = (next + 1) % SMOOTHING_SAMPLES;
        if (samples < SMOOTHING_SAMPLES) samples++;
    }

    /** The time of the oldest sample still inside the window. */
    private long oldestTimeNs() {
        int oldest = (next - samples + SMOOTHING_SAMPLES) % SMOOTHING_SAMPLES;
        return sampleTimeNs[oldest];
    }

    private void updateCamera() {
        // No deadzone: any movement at all deflects the camera, and a faster one approaches full deflection.
        float magnitude = speed > 0 ? 1 - (float) Math.exp(-speed / CAMERA_FULL_SPEED) : 0;
        cameraX = directionX * magnitude;
        // Screen down is a negative stick y, so a finger moving down looks down.
        cameraY = -directionY * magnitude;
    }
}
