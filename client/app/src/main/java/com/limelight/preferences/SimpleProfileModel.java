package com.limelight.preferences;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;

import org.json.JSONException;
import org.json.JSONObject;

/**
 * The simple settings layer's mapping, decided by nightjar-policy's {@code simple_profile} (protocol/policy),
 * reached through libnightjar_core (protocol/android/src/simple_profile.rs), so Android and desktop Nightjar
 * map the same choices the same way. A model that cannot answer reads as Custom: it changes nothing.
 */
public final class SimpleProfileModel {
    private SimpleProfileModel() { }

    /** One JSON request in, one JSON answer out: {@code {"ok": …}} or {@code {"error": code, "detail": …}}. */
    public interface Calls {
        String call(String request);
    }

    private static Calls calls;
    /** The engine could not be loaded: every row reads as Custom without logging on each bind. */
    private static boolean unavailable;

    private static synchronized Calls calls() {
        if (calls == null) {
            try {
                System.loadLibrary("nightjar_core");
                calls = SimpleProfileModel::call;
            } catch (LinkageError e) {
                // A JVM test, or a build without the engine: every row reads as Custom and changes
                // nothing, rather than retrying the load on each bind.
                LimeLog.warning("Simple settings: the engine is not available: " + e);
                unavailable = true;
                calls = request -> "{\"error\":\"unavailable\"}";
            }
        }
        return calls;
    }

    /** JVM tests cannot load the library. */
    @VisibleForTesting
    public static synchronized void use(Calls replacement) {
        calls = replacement;
        unavailable = false;
    }

    private static native String call(String request);

    /** What this device and PC measured: the mapping's whole input. */
    public static final class Capabilities {
        public final int width, height, refreshHz;
        public final boolean displayHdr, displayVrr;
        public final boolean h264, hevc, av1, decoderHdr;
        /** The benchmark's sustained rate at the screen's size, or null when it has not run. */
        public final Integer maxFpsAtNative;
        /** What the paired PC offers; null is not known yet, which never rules anything out. */
        public final Boolean hostHdr, hostHevc, hostAv1;

        public Capabilities(int width, int height, int refreshHz, boolean displayHdr, boolean displayVrr,
                            boolean h264, boolean hevc, boolean av1, boolean decoderHdr, Integer maxFpsAtNative,
                            Boolean hostHdr, Boolean hostHevc, Boolean hostAv1) {
            this.width = width;
            this.height = height;
            this.refreshHz = refreshHz;
            this.displayHdr = displayHdr;
            this.displayVrr = displayVrr;
            this.h264 = h264;
            this.hevc = hevc;
            this.av1 = av1;
            this.decoderHdr = decoderHdr;
            this.maxFpsAtNative = maxFpsAtNative;
            this.hostHdr = hostHdr;
            this.hostHevc = hostHevc;
            this.hostAv1 = hostAv1;
        }

        JSONObject json() throws JSONException {
            return new JSONObject()
                    .put("display", new JSONObject().put("width", width).put("height", height)
                            .put("refresh_hz", refreshHz).put("hdr", displayHdr).put("vrr", displayVrr))
                    .put("decoder", new JSONObject().put("h264", h264).put("hevc", hevc).put("av1", av1)
                            .put("hdr", decoderHdr).put("max_fps_at_native",
                                    maxFpsAtNative == null ? JSONObject.NULL : maxFpsAtNative))
                    .put("host", new JSONObject().put("hdr", hostHdr == null ? JSONObject.NULL : hostHdr)
                            .put("hevc", hostHevc == null ? JSONObject.NULL : hostHevc)
                            .put("av1", hostAv1 == null ? JSONObject.NULL : hostAv1));
        }
    }

    /** What the stream's current advanced settings say. */
    public static final class Current {
        public final boolean nativeResolution;
        public final Integer fps;
        public final String pacing;
        public final boolean vrr;
        public final boolean latencySwitches;
        public final boolean dropLateFrames;
        public final boolean hostFrameTimes;
        public final boolean realVrr;
        public final boolean reduceRefresh;
        public final boolean contentRate;
        /** The stored bitrate as thousandths of the default, or null for the automatic one. */
        public final Integer bitrateMilli;
        public final String codec;
        public final boolean hdr;
        public final boolean adaptiveBitrate;
        public final String adaptiveQuality;
        /** The choices the player last stored, or null when there are none. */
        public final String storedSmoothness, storedPicture, storedHdr;

        public Current(boolean nativeResolution, Integer fps, String pacing, boolean vrr, boolean latencySwitches,
                       boolean dropLateFrames, boolean hostFrameTimes, boolean realVrr, boolean reduceRefresh,
                       boolean contentRate, Integer bitrateMilli, String codec, boolean hdr, boolean adaptiveBitrate,
                       String adaptiveQuality, String storedSmoothness, String storedPicture, String storedHdr) {
            this.nativeResolution = nativeResolution;
            this.fps = fps;
            this.pacing = pacing;
            this.vrr = vrr;
            this.latencySwitches = latencySwitches;
            this.dropLateFrames = dropLateFrames;
            this.hostFrameTimes = hostFrameTimes;
            this.realVrr = realVrr;
            this.reduceRefresh = reduceRefresh;
            this.contentRate = contentRate;
            this.bitrateMilli = bitrateMilli;
            this.codec = codec;
            this.hdr = hdr;
            this.adaptiveBitrate = adaptiveBitrate;
            this.adaptiveQuality = adaptiveQuality;
            this.storedSmoothness = storedSmoothness;
            this.storedPicture = storedPicture;
            this.storedHdr = storedHdr;
        }

        JSONObject json() throws JSONException {
            return new JSONObject().put("native_resolution", nativeResolution)
                    .put("fps", fps == null ? JSONObject.NULL : fps)
                    .put("pacing", pacing == null ? JSONObject.NULL : pacing)
                    .put("vrr", vrr).put("latency_switches", latencySwitches)
                    .put("drop_late_frames", dropLateFrames).put("host_frame_times", hostFrameTimes)
                    .put("real_vrr", realVrr).put("reduce_refresh", reduceRefresh).put("content_rate", contentRate)
                    .put("bitrate_milli", bitrateMilli == null ? JSONObject.NULL : bitrateMilli)
                    .put("codec", codec).put("hdr", hdr).put("adaptive_bitrate", adaptiveBitrate)
                    .put("adaptive_quality", adaptiveQuality == null ? JSONObject.NULL : adaptiveQuality)
                    .put("stored_smoothness", storedSmoothness == null ? JSONObject.NULL : storedSmoothness)
                    .put("stored_picture", storedPicture == null ? JSONObject.NULL : storedPicture)
                    .put("stored_hdr", storedHdr == null ? JSONObject.NULL : storedHdr);
        }
    }

    /** The concrete settings a set of choices means: the values the apps write into their own keys. */
    public static final class Values {
        public final boolean nativeResolution;
        public final Integer fps;
        /** latency, balanced or smoothest. */
        public final String pacing;
        public final boolean vrr;
        public final boolean latencySwitches;
        public final boolean dropLateFrames;
        public final boolean hostFrameTimes;
        public final Boolean realVrr;
        public final boolean reduceRefresh;
        public final boolean contentRate;
        /** The default bitrate's thousandths; 1000 is the automatic one. */
        public final int bitrateMilli;
        /** auto, h264, hevc or av1. */
        public final String codec;
        public final boolean hdr;
        public final boolean adaptiveBitrate;
        /** null when off, else the save-power mode. */
        public final String adaptiveQuality;

        private Values(JSONObject ok) throws JSONException {
            nativeResolution = ok.getBoolean("native_resolution");
            fps = ok.isNull("fps") ? null : ok.getInt("fps");
            pacing = ok.getString("pacing");
            vrr = ok.getBoolean("vrr");
            latencySwitches = ok.getBoolean("latency_switches");
            dropLateFrames = ok.getBoolean("drop_late_frames");
            hostFrameTimes = ok.getBoolean("host_frame_times");
            realVrr = ok.isNull("real_vrr") ? null : ok.getBoolean("real_vrr");
            reduceRefresh = ok.getBoolean("reduce_refresh");
            contentRate = ok.getBoolean("content_rate");
            JSONObject bitrate = ok.getJSONObject("bitrate");
            bitrateMilli = "factor".equals(bitrate.getString("mode")) ? bitrate.getInt("milli") : 1000;
            codec = ok.getString("codec");
            hdr = ok.getBoolean("hdr");
            adaptiveBitrate = ok.getBoolean("adaptive_bitrate");
            adaptiveQuality = ok.getString("adaptive_quality");
        }
    }

    /** The answer's {@code ok} object, null (and a log line) when the model refused or could not be reached. */
    private static JSONObject ask(JSONObject request) {
        if (unavailable) return null;
        try {
            String answer = calls().call(request.toString());
            JSONObject json = new JSONObject(answer);
            JSONObject ok = json.optJSONObject("ok");
            if (ok == null) LimeLog.warning("Simple settings: model answered " + answer);
            return ok;
        } catch (JSONException | LinkageError | RuntimeException e) {
            LimeLog.warning("Simple settings: model unavailable: " + e);
            return null;
        }
    }

    /** The values the choices mean, or null when the model could not answer. */
    static Values values(String smoothness, String picture, String hdr, Capabilities capabilities) {
        try {
            JSONObject request = capabilities.json().put("op", "values")
                    .put("smoothness", smoothness).put("picture", picture).put("hdr", hdr);
            JSONObject ok = ask(request);
            return ok == null ? null : new Values(ok);
        } catch (JSONException e) {
            LimeLog.warning("Simple settings: bad answer: " + e);
            return null;
        }
    }

    /** The recommended choices, {smoothness, picture, hdr}, or null when the model could not answer. */
    static String[] recommended(Capabilities capabilities) {
        try {
            JSONObject ok = ask(capabilities.json().put("op", "recommended"));
            return ok == null ? null : new String[] {ok.getString("smoothness"), ok.getString("picture"),
                    ok.getString("hdr")};
        } catch (JSONException e) {
            LimeLog.warning("Simple settings: bad answer: " + e);
            return null;
        }
    }

    /** The choices these settings are, {smoothness, picture, hdr}, or null for Custom. */
    static String[] chosen(Capabilities capabilities, Current current) {
        try {
            JSONObject ok = ask(capabilities.json().put("op", "chosen").put("current", current.json()));
            if (ok == null || ok.isNull("choices")) return null;
            JSONObject choices = ok.getJSONObject("choices");
            return new String[] {choices.getString("smoothness"), choices.getString("picture"), choices.getString("hdr")};
        } catch (JSONException e) {
            LimeLog.warning("Simple settings: bad answer: " + e);
            return null;
        }
    }
}
