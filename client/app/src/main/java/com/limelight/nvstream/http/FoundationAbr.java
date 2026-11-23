/**
 * Thin JNI delegate for {@code nightjar_core::http::foundation::HostAbr}.
 */
package com.limelight.nvstream.http;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.IOException;

public final class FoundationAbr implements AutoCloseable {
    public static final long FEEDBACK_INTERVAL_MS = 3000;

    private final long handle;
    private final HandleReaper.Token reaper;

    public FoundationAbr(int initialKbps, int streamKbps, long nowMs) {
        this.handle = NightjarPolicyJni.foundationAbrCreate(initialKbps, streamKbps, nowMs);
        this.reaper = HandleReaper.register(this, handle, NightjarPolicyJni::foundationAbrDestroy);
    }

    public int currentKbps() { return NightjarPolicyJni.foundationAbrCurrentKbps(handle); }

    public void add(double lossPercent, double fps) { NightjarPolicyJni.foundationAbrAddRate(handle, lossPercent, fps); }

    public JSONObject due(long nowMs, double rttMs) {
        String raw = NightjarPolicyJni.foundationAbrDue(handle, nowMs, rttMs);
        if (raw == null) return null;
        try {
            JSONObject s = new JSONObject(raw);
            return new JSONObject().put("packetLoss", s.getDouble("packet_loss"))
                    .put("rttMs", s.getDouble("rtt_ms")).put("decodeFps", s.getDouble("decode_fps"))
                    .put("droppedFrames", s.getInt("dropped_frames")).put("currentBitrate", s.getInt("current_bitrate"));
        } catch (JSONException e) {
            throw new IllegalStateException(e);
        }
    }

    public int answered(JSONObject action) {
        boolean has = action != null;
        int kbps = has ? action.optInt("newBitrate", 0) : 0;
        boolean applied = has && action.optBoolean("bitrateApplied", false);
        return NightjarPolicyJni.foundationAbrAnswered(handle, has, kbps, applied);
    }

    public static JSONObject enableRequest(int maxKbps) {
        try {
            return new JSONObject().put("enabled", true).put("mode", "balanced")
                    .put("minBitrate", 0).put("maxBitrate", maxKbps);
        } catch (JSONException e) { throw new IllegalStateException(e); }
    }

    public static JSONObject disableRequest() {
        try { return new JSONObject().put("enabled", false); }
        catch (JSONException e) { throw new IllegalStateException(e); }
    }

    public static boolean supported(String capabilities) throws IOException {
        try { return new JSONObject(capabilities).getBoolean("supported"); }
        catch (JSONException e) { throw new IOException("Invalid ABR capabilities", e); }
    }

    public static int initialBitrate(String configured) throws IOException {
        try {
            JSONObject obj = new JSONObject(configured);
            if (!obj.optBoolean("success", false)) {
                throw new IOException(obj.optString("error", "ABR configuration rejected"));
            }
            return Math.max(0, obj.optInt("initialBitrate", 0));
        } catch (JSONException e) { throw new IOException("Invalid ABR configuration", e); }
    }

    @Override public void close() { reaper.close(); }
    public void destroy() { close(); }
}
