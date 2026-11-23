/**
 * Thin JNI delegate for {@code nightjar_core::http::pace} and {@code PaceSearch}.
 */
package com.limelight.nvstream;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import java.util.Locale;
import java.util.function.BooleanSupplier;

public final class PyroWavePace {
    public static final int MIN_PROBE_KBPS = 5000;
    public static final int MAX_PROBE_KBPS = 3000000;
    public static final int MAX_PACE_KBPS = 9999999;
    public static final int DURATION_MS = 2000;
    public static final int WIRE_OVERHEAD = 134;
    public static final int PROBE_EXTRA_BYTES = 48;
    public static final int JUMBO_PROBE_KBPS = 100000;
    public static final int JUMBO_PROBE_FPS = 60;
    public static final double JUMBO_LOSS_MARGIN_PERCENT = 0.5;

    private PyroWavePace() {}

    public static final class Result {
        public long expected, sent, received, frames, damagedFrames, excessiveLossFrames, missingTailFrames;
        public double senderMs, lossPercent = 100, worstWindowLossPercent = 100, worstFrameLossPercent = 100;

        public double missingTailFramePercent() { return frames == 0 ? 100.0 : 100.0 * missingTailFrames / frames; }
        public double excessiveLossFramePercent() { return frames == 0 ? 100.0 : 100.0 * excessiveLossFrames / frames; }

        public static Result fromJson(org.json.JSONObject j) {
            Result r = new Result();
            r.expected = j.optLong("expected");
            r.sent = j.optLong("sent");
            r.received = j.optLong("received");
            r.senderMs = j.optDouble("sender_ms");
            r.lossPercent = j.optDouble("loss", 100);
            r.worstWindowLossPercent = j.optDouble("worst_window_loss", 100);
            r.frames = j.optLong("frames");
            r.damagedFrames = j.optLong("damaged_frames");
            r.excessiveLossFrames = j.optLong("excessive_loss_frames");
            r.missingTailFrames = j.optLong("missing_tail_frames");
            r.worstFrameLossPercent = j.optDouble("worst_frame_loss", 100);
            return r;
        }

        public static Result fromJsonObj(Json.Obj j) {
            Result r = new Result();
            r.expected = j.optLong("expected");
            r.sent = j.optLong("sent");
            r.received = j.optLong("received");
            r.senderMs = j.optDouble("sender_ms");
            r.lossPercent = j.optDouble("loss", 100);
            r.worstWindowLossPercent = j.optDouble("worst_window_loss", 100);
            r.frames = j.optLong("frames");
            r.damagedFrames = j.optLong("damaged_frames");
            r.excessiveLossFrames = j.optLong("excessive_loss_frames");
            r.missingTailFrames = j.optLong("missing_tail_frames");
            r.worstFrameLossPercent = j.optDouble("worst_frame_loss", 100);
            return r;
        }

        String toJson() {
            return Json.obj().put("expected", expected).put("sent", sent).put("received", received)
                    .put("sender_ms", senderMs).put("loss", lossPercent)
                    .put("worst_window_loss", worstWindowLossPercent).put("frames", frames)
                    .put("damaged_frames", damagedFrames).put("excessive_loss_frames", excessiveLossFrames)
                    .put("missing_tail_frames", missingTailFrames).put("worst_frame_loss", worstFrameLossPercent)
                    .toString();
        }

        @Override public String toString() {
            return String.format(Locale.US, "loss %.2f%% (window %.2f%%), frames %d/%d damaged, tail %.2f%%, worst frame %.2f%%, received %d/%d (sent %d) in %.0f ms",
                    lossPercent, worstWindowLossPercent, damagedFrames, frames, missingTailFramePercent(),
                    worstFrameLossPercent, received, expected, sent, senderMs);
        }
    }

    public interface Probe { Result probe(int paceKbps) throws Exception; }

    public static final class Outcome {
        public final int paceKbps;
        public final boolean qualified;
        public final Result probe;
        public Outcome(int paceKbps, boolean qualified, Result probe) { this.paceKbps = paceKbps; this.qualified = qualified; this.probe = probe; }
        static final Outcome NONE = new Outcome(0, false, null);
    }

    public static long expectedPackets(int kbps, int packetSize) { return NightjarPolicyJni.paceExpectedPackets(kbps, packetSize); }
    public static long burstGroupPackets(int paceKbps, int packetSize, boolean ipv6) { return NightjarPolicyJni.paceBurstGroupPackets(paceKbps, packetSize, ipv6); }
    public static void summarize(Result result, boolean[] arrived, int fps, long groupPackets) {
        Result copy = Result.fromJsonObj(Json.parseObject(NightjarPolicyJni.paceSummarize(
                result.expected, result.sent, result.senderMs, arrived, fps, groupPackets)));
        result.received = copy.received; result.lossPercent = copy.lossPercent;
        result.worstWindowLossPercent = copy.worstWindowLossPercent; result.frames = copy.frames;
        result.damagedFrames = copy.damagedFrames; result.excessiveLossFrames = copy.excessiveLossFrames;
        result.missingTailFrames = copy.missingTailFrames; result.worstFrameLossPercent = copy.worstFrameLossPercent;
    }
    public static boolean measured(Result r) { return r != null && NightjarPolicyJni.paceMeasured(r.toJson()); }
    public static boolean qualified(Result r) { return r != null && NightjarPolicyJni.paceQualified(r.toJson()); }
    public static int jumboProbeSize(int configured) { return NightjarPolicyJni.paceJumboProbeSize(configured); }
    public static boolean jumboPasses(Result jumbo, Result control) {
        return jumbo != null && NightjarPolicyJni.paceJumboPasses(jumbo.toJson(), control == null ? null : control.toJson());
    }
    public static int wireKbps(int imageKbps, int fps, int packetSize, int fec, int minParity) {
        return NightjarPolicyJni.paceWireKbps(imageKbps, fps, packetSize, fec, minParity);
    }
    public static int probeKbps(int imageKbps, int fps, int packetSize, int fec, int minParity) {
        return NightjarPolicyJni.paceProbeKbps(imageKbps, fps, packetSize, fec, minParity);
    }
    public static int floorKbps(int probeKbps) { return NightjarPolicyJni.paceFloorKbps(probeKbps); }
    public static Outcome search(int linkKbps, int floorKbps, Probe probe, BooleanSupplier cancelled) throws Exception {
        long handle = NightjarPolicyJni.paceSearchCreate(linkKbps, floorKbps);
        try {
            int pace;
            while ((pace = NightjarPolicyJni.paceSearchNextPace(handle)) >= 0) {
                if (cancelled != null && cancelled.getAsBoolean()) return Outcome.NONE;
                Result r = probe.probe(pace);
                NightjarPolicyJni.paceSearchRecord(handle, pace, r == null ? "{}" : r.toJson());
            }
            String json = NightjarPolicyJni.paceSearchOutcome(handle);
            if (json == null) return Outcome.NONE;
            Json.Obj obj = Json.parseObject(json);
            return new Outcome(obj.getInt("pace_kbps"), obj.getBoolean("qualified"), Result.fromJsonObj(obj));
        } finally { NightjarPolicyJni.paceSearchDestroy(handle); }
    }
}
