/**
 * Thin JNI delegate for {@code nightjar_policy::quality::AdaptiveQualityController}.
 */
package com.limelight.nvstream;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Objects;

public final class AdaptiveQualityController implements AutoCloseable {
    public enum Mode {
        OFF, MOTION, DETAIL, BATTERY;
        public static Mode parse(String value) {
            if ("motion".equals(value)) return MOTION;
            if ("detail".equals(value)) return DETAIL;
            if ("battery".equals(value)) return BATTERY;
            return OFF;
        }
        public String preference() { return name().toLowerCase(Locale.ROOT); }
    }

    public static final class Target {
        public final int width, height, fps;
        private Target(int width, int height, int fps) { this.width = width; this.height = height; this.fps = fps; }
        public int[] request() { return new int[] {width, height, fps * 1000}; }
        public boolean matches(int w, int h, int f) { return width == w && height == h && fps == f; }
        static Target fromArray(int[] arr) { return arr == null ? null : new Target(arr[0], arr[1], arr[2]); }
        @Override public boolean equals(Object o) { return o instanceof Target && ((Target) o).matches(width, height, fps); }
        @Override public int hashCode() { return Objects.hash(width, height, fps); }
    }

    public static final long DOWN_INTERVAL_MS = 15000;
    public static final long RECOVER_MS = 30000;
    private final long handle;
    private final HandleReaper.Token reaper;

    public AdaptiveQualityController(Mode mode, int width, int height, int fps, int minShortEdge,
                                     int minFps, int alignWidth, int alignHeight, long now) {
        this.handle = NightjarPolicyJni.qualityCreate(mode.ordinal(), width, height, fps,
                minShortEdge, minFps, alignWidth, alignHeight, now);
        this.reaper = HandleReaper.register(this, handle, NightjarPolicyJni::qualityDestroy);
    }

    public void resumeAt(int width, int height, int fps) { NightjarPolicyJni.qualityResumeAt(handle, width, height, fps); }
    public Target ceiling() { return Target.fromArray(NightjarPolicyJni.qualityCeiling(handle)); }
    public Target current() { return Target.fromArray(NightjarPolicyJni.qualityCurrent(handle)); }
    public List<Target> levels() {
        int[] raw = NightjarPolicyJni.qualityLevels(handle);
        List<Target> out = new ArrayList<>(raw == null ? 0 : raw.length / 3);
        if (raw != null) for (int i = 0; i + 2 < raw.length; i += 3) out.add(new Target(raw[i], raw[i + 1], raw[i + 2]));
        return out;
    }
    public Target resizeCeiling(int w, int h, int fps, long now) {
        return Target.fromArray(NightjarPolicyJni.qualityResizeCeiling(handle, w, h, fps, now));
    }

    public Target sample(long now, boolean recentVideo, boolean poorConnection, float loss,
                         float decodeMs, float jitterMs, float queueMs, int thermalLevel,
                         boolean bitrateHasRoom) {
        return sample(now, recentVideo, poorConnection, loss, decodeMs, jitterMs, queueMs,
                thermalLevel, bitrateHasRoom, null, false);
    }

    public Target sample(long now, boolean recentVideo, boolean poorConnection, float loss,
                         float decodeMs, float jitterMs, float queueMs, int thermalLevel,
                         boolean bitrateHasRoom, DecodeTiming timing) {
        return sample(now, recentVideo, poorConnection, loss, decodeMs, jitterMs, queueMs,
                thermalLevel, bitrateHasRoom, timing, true);
    }

    private Target sample(long now, boolean recentVideo, boolean poorConnection, float loss,
                          float decodeMs, float jitterMs, float queueMs, int thermalLevel,
                          boolean bitrateHasRoom, DecodeTiming t, boolean observeTail) {
        int[] out = NightjarPolicyJni.qualitySample(handle, now, recentVideo, poorConnection, loss,
                decodeMs, jitterMs, queueMs, thermalLevel, bitrateHasRoom, observeTail, t != null,
                t == null ? 0 : t.generation, t == null ? 0 : t.windowStartNs,
                t == null ? 0 : t.windowEndNs, t == null ? 0 : t.firstSampleNs,
                t == null ? 0 : t.lastSampleNs, t == null ? 0 : t.fps,
                t == null ? 0 : t.samples, t == null ? 0 : t.lateFrames);
        return Target.fromArray(out);
    }

    public void applied(Target target, long now) {
        NightjarPolicyJni.qualityApplied(handle, target.width, target.height, target.fps, now);
    }
    public void suspend() { NightjarPolicyJni.qualitySuspend(handle); }
    @Override public void close() { reaper.close(); }
    public void destroy() { close(); }
}
