/**
 * Launch/resume/quit/410-confirm orchestration, SharpColor request/pick wrappers, and
 * first-bitrate link probing for {@link NvConnection}, backed by {@link NightjarSessionJni}.
 */
package com.limelight.nvstream;

import android.content.Context;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.video.SharpColor;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.nvstream.jni.NightjarSessionJni;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.atomic.AtomicReference;

final class NvConnectionLaunch {
    private NvConnectionLaunch() {}

    private static String jsonQuote(String value) {
        if (value == null) return "null";
        StringBuilder sb = new StringBuilder(value.length() + 2);
        sb.append('"');
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            switch (c) {
                case '"': sb.append("\\\""); break;
                case '\\': sb.append("\\\\"); break;
                case '\b': sb.append("\\b"); break;
                case '\f': sb.append("\\f"); break;
                case '\n': sb.append("\\n"); break;
                case '\r': sb.append("\\r"); break;
                case '\t': sb.append("\\t"); break;
                default:
                    if (c < 0x20) sb.append(String.format("\\u%04x", (int) c));
                    else sb.append(c);
            }
        }
        return sb.append('"').toString();
    }

    private static Map<String, Object> parseFlatJson(String json) {
        Map<String, Object> map = new HashMap<>();
        if (json == null) return map;
        int n = json.length();
        int i = 0;
        while (i < n && json.charAt(i) != '{') i++;
        if (i >= n) return map;
        i++;
        while (i < n) {
            while (i < n && (Character.isWhitespace(json.charAt(i)) || json.charAt(i) == ',')) i++;
            if (i >= n || json.charAt(i) == '}') break;
            if (json.charAt(i) != '"') break;
            int[] pos = new int[] { i };
            String key = readJsonString(json, pos);
            i = pos[0];
            while (i < n && Character.isWhitespace(json.charAt(i))) i++;
            if (i >= n || json.charAt(i) != ':') break;
            i++;
            while (i < n && Character.isWhitespace(json.charAt(i))) i++;
            if (i >= n) break;
            char ch = json.charAt(i);
            if (ch == '"') {
                pos[0] = i;
                map.put(key, readJsonString(json, pos));
                i = pos[0];
            } else if (ch == 't' && json.startsWith("true", i)) {
                map.put(key, Boolean.TRUE);
                i += 4;
            } else if (ch == 'f' && json.startsWith("false", i)) {
                map.put(key, Boolean.FALSE);
                i += 5;
            } else if (ch == 'n' && json.startsWith("null", i)) {
                i += 4;
            } else {
                int start = i;
                while (i < n && ",}] \t\r\n".indexOf(json.charAt(i)) < 0) i++;
                try {
                    map.put(key, Integer.parseInt(json.substring(start, i)));
                } catch (NumberFormatException ignored) {}
            }
        }
        return map;
    }

    private static String readJsonString(String s, int[] pos) {
        int i = pos[0] + 1;
        int n = s.length();
        StringBuilder sb = new StringBuilder();
        while (i < n) {
            char c = s.charAt(i++);
            if (c == '"') {
                pos[0] = i;
                return sb.toString();
            }
            if (c == '\\' && i < n) {
                char esc = s.charAt(i++);
                switch (esc) {
                    case '"': sb.append('"'); break;
                    case '\\': sb.append('\\'); break;
                    case '/': sb.append('/'); break;
                    case 'b': sb.append('\b'); break;
                    case 'f': sb.append('\f'); break;
                    case 'n': sb.append('\n'); break;
                    case 'r': sb.append('\r'); break;
                    case 't': sb.append('\t'); break;
                    case 'u':
                        if (i + 4 <= n) {
                            sb.append((char) Integer.parseInt(s.substring(i, i + 4), 16));
                            i += 4;
                        }
                        break;
                    default: sb.append(esc);
                }
            } else {
                sb.append(c);
            }
        }
        pos[0] = i;
        return sb.toString();
    }

    private static String optString(Map<String, Object> m, String key, String fallback) {
        Object v = m.get(key);
        return v instanceof String ? (String) v : fallback;
    }

    private static boolean optBoolean(Map<String, Object> m, String key, boolean fallback) {
        Object v = m.get(key);
        return v instanceof Boolean ? (Boolean) v : fallback;
    }

    private static int optInt(Map<String, Object> m, String key, int fallback) {
        Object v = m.get(key);
        return v instanceof Number ? ((Number) v).intValue() : fallback;
    }

    static SharpColor.Request sharpColorRequest(int offered, boolean sdr10, boolean pyroWave,
            boolean packedEngine, int hostScm, int hostPacked, boolean packedRefusedBefore) {
        String json = NightjarSessionJni.sharpColorRequestJson(
                offered, sdr10, pyroWave, packedEngine, hostScm, hostPacked, packedRefusedBefore);
        Map<String, Object> obj = parseFlatJson(json);
        SharpColor.Request req = new SharpColor.Request();
        req.auto = optBoolean(obj, "auto", true);
        req.hdr = optBoolean(obj, "hdr", false);
        req.sdr10 = optBoolean(obj, "sdr10", false);
        req.pyroWave = optBoolean(obj, "pyrowave", false);
        req.packedEngine = optBoolean(obj, "packed_engine", false);
        req.base = optInt(obj, "base", 0);
        req.decodable = optInt(obj, "decodable", 0);
        req.decoderReason = optString(obj, "decoder_reason", "");
        req.hostScm = optInt(obj, "host_scm", 0);
        req.hostPacked = optInt(obj, "host_packed", -1);
        req.packedRefusedBefore = optBoolean(obj, "packed_refused_before", false);
        return req;
    }

    static int formatsAfter(SharpColor.Pick pick, int offered, boolean hdr) {
        String kind;
        int arg;
        if (pick.kind == SharpColor.Pick.Kind.NATIVE) {
            kind = "native";
            arg = pick.formats;
        } else if (pick.kind == SharpColor.Pick.Kind.PACKED) {
            kind = "packed";
            arg = pick.modes;
        } else {
            kind = "without";
            arg = 0;
        }
        return NightjarSessionJni.formatsAfterSharpColor(kind, arg, offered, hdr);
    }

    static String launchFailureMessage(Context context, HostHttpResponseException e) {
        if (e instanceof NvConnection.LocalLaunchFailure) {
            return context.getString(((NvConnection.LocalLaunchFailure) e).messageId);
        }
        String reason = e.getErrorMessage() == null ? "" : e.getErrorMessage().trim();
        int kind = NightjarSessionJni.classifyLaunchFailure(false, e.getErrorCode(), reason);
        if (kind == 1) return context.getString(R.string.startup_permission_denied);
        if (kind == 2) return context.getString(R.string.startup_permission_denied) + "\n\n" + reason;
        if (kind == 3) return context.getString(R.string.startup_host_message, reason, e.getErrorCode());
        return context.getString(R.string.startup_host_error, e.getErrorCode());
    }

    static boolean probesFirstBitrate(ConnectionContext context) {
        NvApp app = context.streamConfig.getApp();
        return context.streamConfig.probesFirstBitrate()
                && !context.streamConfig.isResumeOnly()
                && app.getControl() == NvApp.Control.NONE
                && app.getRole() != NvApp.Role.INPUT_ONLY
                && !app.isSmallStreamInputOnly()
                && (context.negotiatedVideoFormats & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0
                && context.hostCaps.has(HostCapabilities.Feature.BANDWIDTH_PROBE);
    }

    static int firstBitrateKbps(
            ConnectionContext context, boolean paired, NvConnection.LinkProbe probe) {
        NvApp app = context.streamConfig.getApp();
        NightjarSessionJni.LinkProbe jniProbe = new NightjarSessionJni.LinkProbe() {
            @Override public String probeLinkMbps() {
                try {
                    return "{\"ok\":" + probe.mbps() + "}";
                } catch (Throwable t) {
                    return errorEnvelope(t);
                }
            }
            @Override public String probePace(int kbps, int size, int fps, int pace) {
                return "{\"error\":{\"kind\":\"cancelled\",\"message\":\"\"}}";
            }
        };
        boolean probeFirst = context.streamConfig.probesFirstBitrate();
        return NightjarSessionJni.firstBitrateKbps(
                context.streamConfig.getBitrate(), probeFirst,
                probeFirst, context.streamConfig.isResumeOnly(),
                app.getControl().ordinal(), NvConnectionActions.roleOrdinal(app.getRole()),
                app.isSmallStreamInputOnly(), context.negotiatedVideoFormats, paired,
                context.hostCaps.has(HostCapabilities.Feature.BANDWIDTH_PROBE), jniProbe);
    }

    private static String targetJson(NvApp app) {
        return "{"
                + "\"app_id\":" + app.getAppId() + ","
                + "\"app_uuid\":" + jsonQuote(app.getAppUuid() == null ? "" : app.getAppUuid()) + ","
                + "\"app_name\":" + jsonQuote(app.getAppName() == null ? "" : app.getAppName()) + ","
                + "\"control\":" + app.getControl().ordinal() + ","
                + "\"role\":" + NvConnectionActions.roleOrdinal(app.getRole()) + ","
                + "\"monitor_resume\":"
                        + (app.getControl() == NvApp.Control.RESUME && app.getRole() == NvApp.Role.REMOTE_MONITOR)
                        + ","
                + "\"small_stream_input_only\":" + app.isSmallStreamInputOnly() + ","
                + "\"mic_only\":" + app.isMicOnly()
                + "}";
    }

    private static String optsJson(ConnectionContext ctx) {
        return "{"
                + "\"join_running\":" + ctx.streamConfig.isJoinRunning() + ","
                + "\"resume_only\":" + ctx.streamConfig.isResumeOnly() + ","
                + "\"allow_idle_session_end\":" + ctx.streamConfig.isIdleSessionEndAllowed() + ","
                + "\"never_replace\":" + ctx.streamConfig.isNeverReplace() + ","
                + "\"hdr\":" + ctx.negotiatedHdr
                + "}";
    }

    private static String serverInfoJson(
            NvHTTP h, ComputerDetails details, String serverInfo, boolean join)
            throws IOException, XmlPullParserException {
        int currentGame = details != null && details.runningGameId != 0
                ? details.runningGameId : h.getCurrentGame(serverInfo);
        String uuid = details != null && details.runningGameUuid != null ? details.runningGameUuid : "";
        boolean paired = details == null || details.pairState == null
                || details.pairState == PairingManager.PairState.PAIRED;
        boolean sessionIdle = details != null && details.sessionIdle;
        boolean confirmsReplace = details != null && details.caps != null
                && details.caps.has(HostCapabilities.Feature.HOST_CONFIRMS_REPLACEMENT);
        return "{"
                + "\"paired\":" + paired + ","
                + "\"current_game\":" + (join && currentGame == 0 ? 1 : currentGame) + ","
                + "\"current_game_uuid\":" + jsonQuote(uuid) + ","
                + "\"session_idle\":" + sessionIdle + ","
                + "\"host_confirms_replacement\":" + confirmsReplace
                + "}";
    }

    static String errorEnvelope(Throwable t) {
        if (t instanceof HostHttpResponseException) {
            HostHttpResponseException he = (HostHttpResponseException) t;
            String msg = he.getErrorMessage() == null ? "" : he.getErrorMessage();
            return "{\"error\":{\"kind\":\"status\",\"code\":" + he.getErrorCode()
                    + ",\"message\":" + jsonQuote(msg) + "}}";
        }
        String msg = t.getMessage() == null ? "" : t.getMessage();
        return "{\"error\":{\"kind\":\"io\",\"message\":" + jsonQuote(msg) + "}}";
    }

    private static int localFailureMessageId(String localKind) {
        switch (localKind) {
            case "RecoveryOnly":
            case "SessionChanged": return R.string.stream_recovery_session_changed;
            case "BrowserStream": return R.string.continue_here_browser_stream;
            case "ConfirmRejected": return R.string.startup_confirmation_rejected;
            case "ConfirmTimedOut": return R.string.startup_confirmation_timed_out;
            default: return R.string.startup_launch_cancelled;
        }
    }

    private static NightjarSessionJni.HostCallback buildHostCallback(
            NvConnection conn, NvHTTP h, ConnectionContext context, AtomicReference<Throwable> rawError) {
        NvApp app = context.streamConfig.getApp();
        return new NightjarSessionJni.HostCallback() {
            @Override public String serverInfo(boolean fast) {
                try {
                    rawError.set(null);
                    String xml = h.getServerInfo(fast);
                    ComputerDetails d = h.getComputerDetails(xml);
                    return "{\"ok\":" + serverInfoJson(
                            h, d, xml, context.streamConfig.isJoinRunning()) + "}";
                } catch (Throwable t) {
                    rawError.set(t);
                    return errorEnvelope(t);
                }
            }

            @Override public boolean hasEndedSession() {
                return conn != null && NvConnectionActions.hasEndedSession(conn, h);
            }

            @Override public String launch(String verb, int appId, boolean hdr, boolean confirmReplace) {
                context.confirmReplace = confirmReplace;
                rawError.set(null);
                try {
                    if (conn != null && "launch".equals(verb) && !confirmReplace) {
                        context.startBitrateKbps = firstBitrateKbps(context, true, h::probeLinkMbps);
                        if (conn.stopRequested) return "{\"error\":{\"kind\":\"cancelled\",\"message\":\"stopped\"}}";
                    }
                    boolean started = h.launchApp(context, verb, appId, hdr);
                    StringBuilder ok = new StringBuilder("{\"started\":").append(started);
                    if (context.rtspSessionUrl != null) {
                        ok.append(",\"rtsp_session_url\":").append(jsonQuote(context.rtspSessionUrl));
                    }
                    ok.append('}');
                    return "{\"ok\":" + ok + "}";
                } catch (Throwable t) {
                    rawError.set(t);
                    return errorEnvelope(t);
                }
            }

            @Override public String quitApp() {
                rawError.set(null);
                try {
                    return "{\"ok\":" + h.quitApp() + "}";
                } catch (Throwable t) {
                    rawError.set(t);
                    return errorEnvelope(t);
                }
            }

            @Override public int confirmReplace(boolean terminateOnly) {
                LaunchConfirmation confirmation = new LaunchConfirmation(
                        app.getControl() == NvApp.Control.TERMINATE || terminateOnly,
                        LaunchConfirmation.nowMs());
                try {
                    context.connListener.launchConfirmationRequired(confirmation);
                    LaunchConfirmation.State state = confirmation.await();
                    if (state == LaunchConfirmation.State.CONFIRMED) return 0;
                    if (state == LaunchConfirmation.State.TIMED_OUT) return 2;
                    return 3;
                } catch (InterruptedException interrupted) {
                    confirmation.cancel();
                    Thread.currentThread().interrupt();
                    return 4;
                } finally {
                    context.connListener.launchConfirmationFinished();
                }
            }

            @Override public boolean isStopped() { return conn != null && conn.stopRequested; }
        };
    }

    private static boolean handleLaunchReply(NvConnection conn, NvHTTP h, ConnectionContext context,
            String replyJson, AtomicReference<Throwable> rawError)
            throws IOException, XmlPullParserException {
        Map<String, Object> out = parseFlatJson(replyJson);
        if (out.isEmpty()) throw new IOException("Invalid launch reply");
        String kind = optString(out, "kind", "");
        Context appContext = conn != null ? conn.appContext : null;
        switch (kind) {
            case "started":
                if ("resume".equals(optString(out, "verb", ""))) LimeLog.info("Resumed existing game session");
                return true;
            case "action_completed":
                context.launchActionCompleted = true;
                context.connListener.launchActionCompleted(optString(out, "message", ""));
                return false;
            case "stopped_automatic_start":
                context.connListener.automaticStartStopped();
                context.launchActionCompleted = true;
                context.connListener.launchActionCompleted(context.gameRunningMessage);
                return false;
            case "session_ended":
                throw new NvConnection.SessionEndedException();
            case "declined":
                String declinedVerb = optString(out, "verb", "");
                if ("resume".equals(declinedVerb) && context.streamConfig.isResumeOnly()
                        && context.streamConfig.isIdleSessionEndAllowed()
                        && conn != null && NvConnectionActions.hasEndedSession(conn, h)) {
                    throw new NvConnection.SessionEndedException();
                }
                if (conn != null && !context.launchActionCompleted && appContext != null) {
                    context.connListener.displayMessage(appContext.getString(
                            "resume".equals(declinedVerb)
                                    ? R.string.startup_resume_failed : R.string.startup_launch_failed));
                }
                return false;
            case "quit_failed":
                if (appContext != null) context.connListener.displayMessage(appContext.getString(R.string.startup_quit_failed));
                return false;
            case "session_owner_denied":
                if (appContext != null) context.connListener.displayMessage(appContext.getString(R.string.startup_session_owner));
                return false;
            case "app_minimized":
                if (appContext != null) context.connListener.displayMessage(appContext.getString(R.string.startup_app_minimized));
                return false;
            case "input_only_other_game":
                if (appContext != null) context.connListener.displayMessage(appContext.getString(R.string.input_only_other_game));
                return false;
            case "local_failure": {
                String localKind = optString(out, "local_kind", "");
                int code = optInt(out, "code", 499);
                Throwable raw = rawError.get();
                if (("SessionChanged".equals(localKind) || "BrowserStream".equals(localKind))
                        && raw instanceof HostHttpResponseException) {
                    code = ((HostHttpResponseException) raw).getErrorCode();
                }
                throw new NvConnection.LocalLaunchFailure(
                        code,
                        localFailureMessageId(localKind),
                        optString(out, "message", ""));
            }
            default:
                if ("host_status".equals(kind)) {
                    int code = optInt(out, "code", 500);
                    String msg = optString(out, "message", "");
                    int refusal = NightjarPolicyJni.continueHereResumeRefusal(
                            context.streamConfig.isJoinRunning(), code, msg);
                    if (refusal == NightjarPolicyJni.RESUME_REFUSAL_SESSION_CHANGED) {
                        throw new NvConnection.LocalLaunchFailure(
                                code, R.string.stream_recovery_session_changed, msg);
                    }
                    if (refusal == NightjarPolicyJni.RESUME_REFUSAL_BROWSER_STREAM) {
                        throw new NvConnection.LocalLaunchFailure(
                                code, R.string.continue_here_browser_stream, msg);
                    }
                }
                Throwable raw = rawError.get();
                if (raw instanceof HostHttpResponseException) throw (HostHttpResponseException) raw;
                if (raw instanceof IOException) throw (IOException) raw;
                if (raw instanceof XmlPullParserException) throw (XmlPullParserException) raw;
                if ("host_status".equals(kind)) {
                    throw new HostHttpResponseException(optInt(out, "code", 500), optString(out, "message", ""));
                }
                throw new IOException(optString(out, "message", "Launch failed"));
        }
    }

    static boolean launchApp(NvHTTP http, ConnectionContext context, String verb)
            throws IOException, XmlPullParserException {
        AtomicReference<Throwable> rawError = new AtomicReference<>();
        boolean originalConfirm = context.confirmReplace;
        try {
            String target = targetJson(context.streamConfig.getApp());
            String opts = optsJson(context);
            String reply = NightjarSessionJni.launchAppConfirming(
                    target, opts, verb, buildHostCallback(null, http, context, rawError));
            return handleLaunchReply(null, http, context, reply, rawError);
        } finally {
            context.confirmReplace = originalConfirm;
        }
    }

    static boolean quitAndLaunch(NvConnection conn, NvHTTP h, ConnectionContext context)
            throws IOException, XmlPullParserException {
        AtomicReference<Throwable> rawError = new AtomicReference<>();
        boolean originalConfirm = context.confirmReplace;
        try {
            String target = targetJson(context.streamConfig.getApp());
            String opts = optsJson(context);
            String reply = NightjarSessionJni.quitAndLaunch(
                    target, opts, buildHostCallback(conn, h, context, rawError));
            return handleLaunchReply(conn, h, context, reply, rawError);
        } finally {
            context.confirmReplace = originalConfirm;
        }
    }

    static boolean launchOrResume(NvConnection conn, NvHTTP h, ConnectionContext context,
            ComputerDetails details, NvApp app, String serverInfo)
            throws XmlPullParserException, IOException {
        context.gameRunningMessage = conn.appContext.getString(R.string.open_play_game_running, details.name);
        if (!context.streamConfig.isJoinRunning()
                && !NvConnection.launchesBesideRunningGame(app)
                && details.runningGameId != 0
                && !app.matchesRunningApp(details.runningGameId, details.runningGameUuid)
                && !app.isSmallStreamInputOnly()
                && !details.caps.has(HostCapabilities.Feature.HOST_CONFIRMS_REPLACEMENT)
                && !(context.streamConfig.isNeverReplace() && !context.streamConfig.isResumeOnly())) {
            return conn.quitAndLaunch(h, context);
        }
        AtomicReference<Throwable> rawError = new AtomicReference<>();
        boolean originalConfirm = context.confirmReplace;
        try {
            String si = serverInfoJson(h, details, serverInfo, context.streamConfig.isJoinRunning());
            String target = targetJson(app);
            String opts = optsJson(context);
            String reply = NightjarSessionJni.launchOrResume(
                    si, target, opts, buildHostCallback(conn, h, context, rawError));
            return handleLaunchReply(conn, h, context, reply, rawError);
        } finally {
            context.confirmReplace = originalConfirm;
        }
    }
}
