package com.limelight.binding.input.pad;

import com.limelight.binding.input.MouseDeltaAccumulator;

/**
 * The pointer touchpad's movement in the host's pixels. The screen touchpad's gestures scale by the stream's size
 * over the view's, so a movement goes as far on the host whatever the phone's size and the stream's resolution, and
 * by Trackpad speed. The fraction of a pixel is kept for the next movement, and a move stays inside the range the
 * mouse packet carries.
 *
 * <p>Plain data and math, so the scaling is tested without a device.
 */
public final class PointerMove {
    static final int MIN = Short.MIN_VALUE, MAX = Short.MAX_VALUE;
    private final MouseDeltaAccumulator deltaX = new MouseDeltaAccumulator(), deltaY = new MouseDeltaAccumulator();
    private float scaleX = 1, scaleY = 1;

    /**
     * Sets how far one touchpad pixel moves the host pointer: the stream's size (the host's) over the view's, times
     * Trackpad speed in percent. An unsized view moves nothing rather than jumping once it is laid out.
     */
    public void setViewport(int streamWidth, int streamHeight, int viewWidth, int viewHeight, int speedPercent) {
        float speed = Math.max(0, speedPercent) / 100f;
        scaleX = viewWidth > 0 ? streamWidth / (float) viewWidth * speed : 0;
        scaleY = viewHeight > 0 ? streamHeight / (float) viewHeight * speed : 0;
    }

    /**
     * Adds a movement of ({@code dx}, {@code dy}) touchpad pixels and writes the host-pixel movement into
     * {@code out[0]} and {@code out[1]}.
     *
     * @return false when it rounds to no movement at all
     */
    public boolean move(int dx, int dy, int[] out) {
        out[0] = clamp(deltaX.scale(dx, scaleX));
        out[1] = clamp(deltaY.scale(dy, scaleY));
        return out[0] != 0 || out[1] != 0;
    }

    private static int clamp(int delta) {
        return Math.max(MIN, Math.min(MAX, delta));
    }
}
