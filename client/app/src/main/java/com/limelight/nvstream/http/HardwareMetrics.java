/**
 * Host CPU and GPU telemetry ({@code GET /api/hardware-metrics}), parsed in Rust via {@link NightjarHttpJni}.
 */
package com.limelight.nvstream.http;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHttpJni;

import java.io.IOException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public final class HardwareMetrics {
    static final int MAX_RESPONSE_BYTES = 256 * 1024;

    public final long sampledAtUnixMs;
    public final int intervalMs;
    public final Map<String, Double> cpu;
    public final List<Gpu> gpus;

    public HardwareMetrics(long sampledAtUnixMs, int intervalMs, Map<String, Double> cpu, List<Gpu> gpus) {
        this.sampledAtUnixMs = sampledAtUnixMs;
        this.intervalMs = intervalMs;
        this.cpu = Collections.unmodifiableMap(new LinkedHashMap<>(cpu));
        this.gpus = Collections.unmodifiableList(new ArrayList<>(gpus));
    }

    public static final class Gpu {
        public final int id;
        public final String name;
        public final Map<String, Double> metrics;

        public Gpu(int id, String name, Map<String, Double> metrics) {
            this.id = id;
            this.name = name;
            this.metrics = Collections.unmodifiableMap(new LinkedHashMap<>(metrics));
        }
    }

    public static final class UnsupportedVersionException extends IOException {
        public UnsupportedVersionException() {
            super("Unsupported hardware metrics version");
        }
    }

    public static HardwareMetrics parse(String json) throws IOException {
        try {
            Json.Obj reply = Json.parseObject(NightjarHttpJni.parseHardwareMetrics(json));
            if (reply.has("error")) {
                Json.Obj err = reply.getObject("error");
                if ("unsupported_version".equals(err.optString("kind"))) {
                    throw new UnsupportedVersionException();
                }
                throw new IOException(err.optString("message", "Invalid hardware metrics response"));
            }
            return fromJsonObj(reply.getObject("ok"));
        } catch (Json.SyntaxException e) {
            throw new IOException("Invalid hardware metrics response", e);
        }
    }

    static HardwareMetrics fromJsonObj(Json.Obj root) {
        long sampledAt = root.getLong("sampled_at_unix_ms");
        int interval = root.getInt("interval_ms");
        Map<String, Double> cpu = toMap(root.getObject("cpu"));
        Json.Arr entries = root.getArray("gpus");
        List<Gpu> gpus = new ArrayList<>(entries.length());
        for (int i = 0; i < entries.length(); i++) {
            Json.Obj entry = entries.getObject(i);
            gpus.add(new Gpu(entry.getInt("id"), entry.getString("name"), toMap(entry.getObject("metrics"))));
        }
        return new HardwareMetrics(sampledAt, interval, cpu, gpus);
    }

    private static Map<String, Double> toMap(Json.Obj obj) {
        Map<String, Double> out = new LinkedHashMap<>();
        Iterator<String> keys = obj.keys();
        while (keys.hasNext()) {
            String key = keys.next();
            out.put(key, obj.getDouble(key));
        }
        return out;
    }
}
