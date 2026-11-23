/**
 * Thin JNI delegate for {@code nightjar_policy::pyrowave::BitrateController}.
 */
package com.limelight.nvstream;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

public final class PyroWaveBitrateController implements AutoCloseable {
    public static final int MAX_RUNTIME_KBPS = StreamConfiguration.MAX_BITRATE_KBPS;

    private final long handle;
    private final HandleReaper.Token reaper;
    private int thermalLevel;

    public PyroWaveBitrateController(int ceilingKbps, int currentKbps, int fps, long nowMs) {
        this.handle = NightjarPolicyJni.pyroAbrCreate(ceilingKbps, currentKbps, fps, nowMs);
        this.reaper = HandleReaper.register(this, handle, NightjarPolicyJni::pyroAbrDestroy);
    }

    public int sample(long nowMs, boolean recentVideo, boolean poorConnection, float lossPercent, float queueMs, float decodeMs) {
        return sample(nowMs, recentVideo, poorConnection, lossPercent, queueMs, decodeMs, 0);
    }

    public int sample(long nowMs, boolean recentVideo, boolean poorConnection, float lossPercent, float queueMs, float decodeMs, float jitterMs) {
        return NightjarPolicyJni.pyroAbrSample(handle, nowMs, recentVideo, poorConnection,
                lossPercent, queueMs, decodeMs, jitterMs, thermalLevel);
    }

    public void setThermalLevel(int level) { this.thermalLevel = level; }

    public void applied(int requestedKbps, int appliedKbps, long nowMs) {
        NightjarPolicyJni.pyroAbrApplied(handle, requestedKbps, appliedKbps, nowMs);
    }

    public void suspend() { NightjarPolicyJni.pyroAbrSuspend(handle); }
    @Override public void close() { reaper.close(); }
    public void destroy() { close(); }
}
