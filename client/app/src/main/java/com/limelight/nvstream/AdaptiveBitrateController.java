/**
 * Thin JNI delegate for {@code nightjar_policy::bitrate::AdaptiveBitrateController}.
 */
package com.limelight.nvstream;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

public final class AdaptiveBitrateController implements AutoCloseable {
    public static final int DEFAULT_FLOOR_KBPS = 2000;

    public enum Codec {
        H264(100), HEVC(70), AV1(65);
        final int bitsPerKiloPixel;
        Codec(int bitsPerKiloPixel) { this.bitsPerKiloPixel = bitsPerKiloPixel; }
    }

    private final long handle;
    private final HandleReaper.Token reaper;

    public static int builtInFloorKbps(int width, int height, int fps, Codec codec) {
        return NightjarPolicyJni.abrBuiltInFloorKbps(width, height, fps, codec == null ? -1 : codec.ordinal());
    }

    public AdaptiveBitrateController(int ceilingKbps, int currentKbps, int fps, long nowMs) {
        this(ceilingKbps, currentKbps, fps, nowMs, 0, 0, null);
    }

    public AdaptiveBitrateController(int ceilingKbps, int currentKbps, int fps, long nowMs,
                                     int width, int height, Codec codec) {
        this.handle = NightjarPolicyJni.abrCreate(ceilingKbps, currentKbps, fps, nowMs,
                width, height, codec == null ? -1 : codec.ordinal());
        this.reaper = HandleReaper.register(this, handle, NightjarPolicyJni::abrDestroy);
    }

    public String lastReason() {
        return NightjarPolicyJni.abrLastReason(handle);
    }

    public int ceilingKbps() {
        return NightjarPolicyJni.abrCeilingKbps(handle);
    }

    public int floorKbps() {
        return NightjarPolicyJni.abrFloorKbps(handle);
    }

    public int sample(long nowMs, boolean recentVideo, boolean poorConnection, float lossPercent, float decodeMs) {
        return sample(nowMs, recentVideo, poorConnection, lossPercent, decodeMs, 0);
    }

    public int sample(long nowMs, boolean recentVideo, boolean poorConnection, float lossPercent, float decodeMs, float jitterMs) {
        return sample(nowMs, recentVideo, poorConnection, lossPercent, decodeMs, jitterMs, -1);
    }

    public int sample(long nowMs, boolean recentVideo, boolean poorConnection, float lossPercent, float decodeMs,
                      float jitterMs, float queueMs) {
        return NightjarPolicyJni.abrSample(handle, nowMs, recentVideo, poorConnection,
                lossPercent, decodeMs, jitterMs, queueMs);
    }

    public void setThermalLevel(int level) { NightjarPolicyJni.abrSetThermalLevel(handle, level); }
    public void setFrameRate(int fps) { NightjarPolicyJni.abrSetFrameRate(handle, fps); }
    public void setStreamSize(int width, int height) { NightjarPolicyJni.abrSetStreamSize(handle, width, height); }
    public boolean hasRoomBeforeQualityReduction() { return NightjarPolicyJni.abrHasRoomBeforeQualityReduction(handle); }
    public void applied(int requestedKbps, int appliedKbps, long nowMs) {
        NightjarPolicyJni.abrApplied(handle, requestedKbps, appliedKbps, nowMs);
    }
    public void suspend() { NightjarPolicyJni.abrSuspend(handle); }
    @Override public void close() { reaper.close(); }
    public void destroy() { close(); }
}
