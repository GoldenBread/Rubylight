package com.limelight.nvstream.http;

import com.limelight.nvstream.jni.Json;

import org.xmlpull.v1.XmlPullParserException;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.util.Locale;

/**
 * Package-private JSON envelope and server-info snapshot helpers extracted from {@link NvHTTP}.
 */
final class NvHttpJson {
    private NvHttpJson() {}

    static ComputerDetails detailsFromSnapshot(Json.Obj snap, int httpPort) throws XmlPullParserException {
        try {
            ComputerDetails details = new ComputerDetails();
            details.name = snap.optString("hostname", "UNKNOWN");
            details.uuid = snap.getString("unique_id");
            details.httpsPort = snap.getInt("https_port");
            details.macAddress = optNullString(snap, "mac_address");
            details.localAddress = makeTuple(optNullString(snap, "local_ip"), httpPort);
            details.externalPort = snap.getInt("external_port");
            details.remoteAddress = makeTuple(optNullString(snap, "external_ip"), details.externalPort);
            details.pairState = snap.getBoolean("paired")
                    ? PairingManager.PairState.PAIRED : PairingManager.PairState.NOT_PAIRED;
            details.runningGameId = snap.getInt("current_game");
            details.sessionIdle = snap.getBoolean("session_idle");
            details.runningGameUuid = optNullString(snap, "current_game_uuid");
            applyCapabilities(details, snap);
            details.nvidiaServer = snap.getBoolean("nvidia_server");
            details.state = ComputerDetails.State.ONLINE;
            return details;
        } catch (Json.SyntaxException e) {
            throw new XmlPullParserException(e.getMessage());
        }
    }

    static void applyServerInfoSnapshot(ComputerDetails target, Json.Obj snap, int httpPort) {
        if (target == null || snap == null) return;
        String name = snap.optString("hostname", "UNKNOWN");
        if (!name.isEmpty() && !"UNKNOWN".equals(name)) {
            target.name = name;
        }
        String uid = optNullString(snap, "unique_id");
        if (uid != null && !uid.isEmpty()) target.uuid = uid;
        int https = snap.optInt("https_port", 0);
        if (https != 0) target.httpsPort = https;
        String mac = optNullString(snap, "mac_address");
        if (mac != null) target.macAddress = mac;
        String localIp = optNullString(snap, "local_ip");
        if (localIp != null && !localIp.startsWith("127.")) {
            ComputerDetails.AddressTuple local = makeTuple(localIp, httpPort);
            if (local != null) target.localAddress = local;
        }
        target.externalPort = snap.optInt("external_port", httpPort);
        ComputerDetails.AddressTuple remote = makeTuple(optNullString(snap, "external_ip"), target.externalPort);
        if (remote != null) target.remoteAddress = remote;
        target.pairState = snap.optBoolean("paired", false)
                ? PairingManager.PairState.PAIRED : PairingManager.PairState.NOT_PAIRED;
        target.runningGameId = snap.optInt("current_game", 0);
        target.sessionIdle = snap.optBoolean("session_idle", false);
        target.runningGameUuid = optNullString(snap, "current_game_uuid");
        applyCapabilities(target, snap);
        target.nvidiaServer = snap.optBoolean("nvidia_server", false);
        target.state = ComputerDetails.State.ONLINE;
    }

    static void applyCapabilities(ComputerDetails details, Json.Obj snap) {
        details.caps = HostCapabilities.fromSnapshotJson(snap.getObject("capabilities"));
        details.permission = snap.isNull("permission") ? -1 : snap.getLong("permission");
        details.encoderBitrateSupported = snap.getBoolean("encoder_bitrate_supported");
        details.encoderBitrateKbps = snap.getInt("encoder_bitrate_kbps");
        details.initialEncoderBitrateKbps = snap.getInt("initial_encoder_bitrate_kbps");
        Json.Arr cmds = snap.getArray("server_commands");
        details.serverCommands.clear();
        for (int i = 0; i < cmds.length(); i++) {
            details.serverCommands.add(cmds.getString(i));
        }
        details.frameLimiterEnabled = snap.getBoolean("frame_limiter_enabled");
        details.virtualDisplayFrameLimiterEnabled = snap.getBoolean("virtual_display_frame_limiter_enabled");
        details.frameLimiterFpsLimitMilliHz = snap.getLong("frame_limiter_fps_limit_millihz");
        details.pyroWaveHostLinkMbps = snap.getLong("pyrowave_host_link_mbps");
        details.pyroWaveBandwidthProbeBytes = snap.getInt("pyrowave_bandwidth_probe_bytes");
        details.pyroWaveBurstProbe = snap.getBoolean("pyrowave_burst_probe");
        details.pyroWaveCriticalFecPercentage = snap.getInt("pyrowave_critical_fec_percentage");
        details.pyroWaveMinParityShards = snap.getInt("pyrowave_min_parity_shards");
        details.packed444Modes = snap.isNull("packed444_modes") ? -1 : snap.getInt("packed444_modes");
        details.hostLastFailure = snap.isNull("last_failure") ? "" : snap.getString("last_failure");
    }

    static Json.Obj unwrapOkObj(String raw, boolean xmlErrors) throws IOException, XmlPullParserException {
        try { return unwrapOk(raw, xmlErrors).getObject("ok"); }
        catch (Json.SyntaxException e) { throw new IOException("Malformed JNI JSON reply", e); }
    }

    static Json.Obj unwrapOk(String raw, boolean xmlErrors) throws IOException, XmlPullParserException {
        return unwrapOk(raw, xmlErrors, false);
    }

    static Json.Obj unwrapOk(String raw, boolean xmlErrors, boolean illegalArgument)
            throws IOException, XmlPullParserException {
        try {
            Json.Obj obj = Json.parseObject(raw);
            if (!obj.has("error")) return obj;
            Json.Obj err = obj.getObject("error");
            String kind = err.optString("kind");
            String msg = err.optString("message", "Request failed");
            if ("launch_timeout".equals(kind)) throw new NvHTTP.LaunchTimeoutException(err.optInt("seconds", 120));
            if ("not_found".equals(kind)) throw new FileNotFoundException(msg);
            if ("unsupported_version".equals(kind)) throw new HardwareMetrics.UnsupportedVersionException();
            if (illegalArgument && "invalid_argument".equals(kind)) throw new IllegalArgumentException(msg);
            if ("malformed".equals(kind) && xmlErrors && !isWireLimitError(msg)) {
                throw new XmlPullParserException(msg);
            }
            if ("status".equals(kind)) {
                int code = err.has("error_code") ? err.getInt("error_code") : err.optInt("http_status", 500);
                throw new HostHttpResponseException(code, err.optString("error_message", msg));
            }
            if (err.has("http_status")) throw new HostHttpResponseException(err.getInt("http_status"), msg);
            throw new IOException(msg);
        } catch (Json.SyntaxException e) { throw new IOException("Malformed JNI JSON reply", e); }
    }

    static Json.Obj unwrapClipboard(String raw, IOException sinkErr) throws IOException {
        Json.Obj obj = Json.parseObject(raw);
        if (!obj.has("error")) return obj;
        if (sinkErr != null) throw sinkErr;
        Json.Obj err = obj.getObject("error");
        if ("clipboard_rejected".equals(err.optString("kind"))) {
            throw new ClipboardImageException(
                    ClipboardImageException.Reason.valueOf(err.getString("reason")), err.optString("message", ""));
        }
        try { unwrapOk(raw, false); } catch (XmlPullParserException e) { throw new IOException(e.getMessage(), e); }
        return obj;
    }

    static ComputerDetails.AddressTuple makeTuple(String address, int port) {
        if (address == null || address.isEmpty()) return null;
        try { return new ComputerDetails.AddressTuple(address, port); }
        catch (IllegalArgumentException e) { return null; }
    }

    private static boolean isWireLimitError(String msg) {
        if (msg == null) return false;
        String lower = msg.toLowerCase(Locale.ROOT);
        return lower.contains("exceed") || lower.contains("too large") || lower.contains("limit")
                || lower.contains("truncated") || lower.contains("connection closed")
                || lower.contains("content-length") || lower.contains("chunk");
    }

    static String optNullString(Json.Obj obj, String key) {
        return !obj.has(key) || obj.isNull(key) ? null : obj.optString(key, null);
    }
}
