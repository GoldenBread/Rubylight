/**
 * Non-final HTTP/TLS host client facade delegating networking, pairing, XML/JSON parsing, launch,
 * probes, and clipboard operations to {@code nightjar_core::http::HostClient} via {@link NightjarHttpJni}.
 */
package com.limelight.nvstream.http;

import android.os.Build;

import com.limelight.BuildConfig;
import com.limelight.nvstream.ConnectionContext;
import com.limelight.nvstream.PyroWavePace;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarHttpJni;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.utils.ClipboardSync;

import org.xmlpull.v1.XmlPullParserException;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.Reader;
import java.security.PrivateKey;
import java.security.cert.CertificateEncodingException;
import java.security.cert.X509Certificate;
import java.util.LinkedList;
import java.util.List;
import java.util.ListIterator;
import java.util.Locale;
import java.util.function.IntConsumer;

public class NvHTTP implements AutoCloseable {
    public static final int DEFAULT_HTTP_PORT = 47989;
    public static final int SHORT_CONNECTION_TIMEOUT = 3000;
    public static final int LONG_CONNECTION_TIMEOUT = 5000;
    public static final int READ_TIMEOUT = 7000;
    public static final int PYROWAVE_BANDWIDTH_PROBE_BYTES = 32 * 1024 * 1024;
    public static final String RATE_CHANGE_LAUNCH_PARAMETER = "rlRateChange";
    public static final String SECOND_SCREEN_PARAMETER = "rlSecondScreen";
    public static final int LINK_PROBE_BYTES = 4 * 1024 * 1024;
    public static final int LAUNCH_READ_TIMEOUT_SECONDS = 120;
    public static final String USER_AGENT = "Nightjar/" + BuildConfig.VERSION_NAME;
    static final int MAX_XML_RESPONSE_BYTES = 256 * 1024;
    static final int MAX_APP_LIST_BYTES = 16 * 1024 * 1024;

    static {
        NightjarHttpJni.setAppVersion(BuildConfig.VERSION_NAME);
    }

    private final long handle;
    private final HandleReaper.Token reaper;
    private final int httpPort;
    private X509Certificate serverCert;
    private volatile boolean serverInfoFromAnotherCertificate;
    private HostCapabilities hostCaps = HostCapabilities.UNKNOWN;
    private final LimelightCryptoProvider cryptoProvider;

    public static final class LaunchTimeoutException extends IOException {
        public final int seconds;

        public LaunchTimeoutException(int seconds) {
            super("The host did not answer the launch within " + seconds + " seconds");
            this.seconds = seconds;
        }
    }

    public NvHTTP(ComputerDetails.AddressTuple address, int httpsPort, String uniqueId,
                  X509Certificate serverCert, LimelightCryptoProvider cryptoProvider) throws IOException {
        if (address == null) throw new IOException("Missing host address");
        this.httpPort = address.port;
        this.serverCert = serverCert;
        this.cryptoProvider = cryptoProvider;
        byte[] clientCertDer = encodeCert(cryptoProvider == null ? null : cryptoProvider.getClientCertificate());
        byte[] clientKeyPkcs8 = encodeKey(cryptoProvider == null ? null : cryptoProvider.getClientPrivateKey());
        byte[] serverCertDer = encodeCert(serverCert);
        String deviceName = Build.MODEL != null && !Build.MODEL.isEmpty() ? Build.MODEL : "Nightjar";
        this.handle = NightjarHttpJni.clientCreate(address.address, address.port, httpsPort,
                uniqueId == null ? "" : uniqueId, clientCertDer, clientKeyPkcs8, serverCertDer, deviceName);
        if (this.handle <= 0) {
            throw new IOException("Invalid host address or identity: " + address.address);
        }
        this.reaper = HandleReaper.register(this, handle, NightjarHttpJni::clientDestroy);
    }

    private static byte[] encodeCert(X509Certificate cert) throws IOException {
        if (cert == null) return new byte[0];
        try { return cert.getEncoded(); }
        catch (CertificateEncodingException e) { throw new IOException("Invalid certificate", e); }
    }

    private static byte[] encodeKey(PrivateKey key) {
        return key == null ? new byte[0] : key.getEncoded();
    }

    long httpHandle() { return handle; }

    void setServerCert(X509Certificate serverCert) {
        this.serverCert = serverCert;
        try {
            NightjarHttpJni.clientSetServerCertDer(handle, encodeCert(serverCert));
        } catch (IOException e) {
            throw new IllegalStateException(e);
        }
    }

    public boolean serverInfoFromAnotherCertificate() { return serverInfoFromAnotherCertificate; }

    public void setHttpsPort(int httpsPort) {
        NightjarHttpJni.clientSetHttpsPort(handle, httpsPort);
    }

    public int getHttpsPort() {
        return NightjarHttpJni.clientGetHttpsPort(handle);
    }

    public static void setSecondScreen(boolean second) {
        NightjarHttpJni.setSecondScreen(second);
    }

    public void cancelPendingRequests() {
        NightjarHttpJni.clientCancelPendingRequests(handle);
    }

    @Override
    public void close() {
        reaper.close();
    }

    public String getServerInfo(boolean likelyOnline) throws IOException, XmlPullParserException {
        serverInfoFromAnotherCertificate = false;
        Json.Obj ok = unwrapOkObj(NightjarHttpJni.getServerInfo(handle, likelyOnline), true);
        serverInfoFromAnotherCertificate = "cert_mismatch".equals(ok.optString("path"));
        try {
            return ok.getObject("info").getString("raw");
        } catch (Json.SyntaxException e) {
            throw new XmlPullParserException(e.getMessage());
        }
    }

    public ComputerDetails getComputerDetails(boolean likelyOnline) throws IOException, XmlPullParserException {
        serverInfoFromAnotherCertificate = false;
        Json.Obj ok = unwrapOkObj(NightjarHttpJni.getServerInfo(handle, likelyOnline), true);
        serverInfoFromAnotherCertificate = "cert_mismatch".equals(ok.optString("path"));
        try {
            return detailsFromSnapshot(ok.getObject("info"));
        } catch (Json.SyntaxException e) {
            throw new XmlPullParserException(e.getMessage());
        }
    }

    public ComputerDetails getComputerDetails(String serverInfo) throws IOException, XmlPullParserException {
        Json.Obj snap = unwrapOkObj(NightjarHttpJni.parseServerInfoXml(serverInfo, httpPort, true), true);
        return detailsFromSnapshot(snap);
    }

    public ComputerDetails probePairedComputer() throws IOException, XmlPullParserException {
        Json.Obj snap = unwrapOkObj(NightjarHttpJni.probePairedComputer(handle), true);
        return detailsFromSnapshot(snap);
    }

    static void readClientCapabilities(ComputerDetails details, String serverInfo)
            throws IOException, XmlPullParserException {
        String xml = serverInfo == null ? "" : serverInfo;
        try {
            Json.Obj snap = unwrapOkObj(NightjarHttpJni.parseServerInfoXml(xml, DEFAULT_HTTP_PORT, false), true);
            applyCapabilities(details, snap);
        } catch (Json.SyntaxException e) {
            throw new XmlPullParserException(e.getMessage());
        }
    }

    private ComputerDetails detailsFromSnapshot(Json.Obj snap) throws XmlPullParserException {
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
            hostCaps = details.caps;
            details.state = ComputerDetails.State.ONLINE;
            return details;
        } catch (Json.SyntaxException e) {
            throw new XmlPullParserException(e.getMessage());
        }
    }

    private static void applyCapabilities(ComputerDetails details, Json.Obj snap) {
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

    private Json.Obj parseSnapshot(String serverInfo) throws XmlPullParserException, IOException {
        return unwrapOkObj(NightjarHttpJni.parseServerInfoXml(serverInfo, httpPort, false), true);
    }

    public String getServerVersion(String serverInfo) throws XmlPullParserException, IOException {
        return optNullString(parseSnapshot(serverInfo), "app_version");
    }

    public PairingManager.PairState getPairState() throws IOException, XmlPullParserException {
        return getComputerDetails(true).pairState;
    }

    public PairingManager.PairState getPairState(String serverInfo) throws IOException, XmlPullParserException {
        return parseSnapshot(serverInfo).optBoolean("paired")
                ? PairingManager.PairState.PAIRED : PairingManager.PairState.NOT_PAIRED;
    }

    public long getMaxLumaPixelsH264(String serverInfo) throws XmlPullParserException, IOException {
        return parseSnapshot(serverInfo).optLong("max_luma_pixels_h264", 0);
    }

    public long getMaxLumaPixelsHEVC(String serverInfo) throws XmlPullParserException, IOException {
        return parseSnapshot(serverInfo).optLong("max_luma_pixels_hevc", 0);
    }

    public long getServerCodecModeSupport(String serverInfo) throws XmlPullParserException, IOException {
        return parseSnapshot(serverInfo).optLong("server_codec_mode_support", 0);
    }

    public String getGpuType(String serverInfo) throws XmlPullParserException, IOException {
        return optNullString(parseSnapshot(serverInfo), "gpu_type");
    }

    public boolean supports4K(String serverInfo) throws XmlPullParserException, IOException {
        return parseSnapshot(serverInfo).optBoolean("supports_4k", false);
    }

    public int getCurrentGame(String serverInfo) throws IOException, XmlPullParserException {
        return parseSnapshot(serverInfo).optInt("current_game", 0);
    }

    public int getHttpsPort(String serverInfo) {
        try { return parseSnapshot(serverInfo).optInt("https_port", 47984); }
        catch (Exception e) { return 47984; }
    }

    public int getExternalPort(String serverInfo) {
        try { return parseSnapshot(serverInfo).optInt("external_port", httpPort); }
        catch (Exception e) { return httpPort; }
    }

    public int[] getServerAppVersionQuad(String serverInfo) throws XmlPullParserException, IOException {
        String ver = getServerVersion(serverInfo);
        if (ver == null) throw new XmlPullParserException("Missing server version field");
        String[] parts = ver.split("\\.");
        if (parts.length != 4) throw new XmlPullParserException("Malformed server version field: " + ver);
        int[] quad = new int[4];
        try {
            for (int i = 0; i < 4; i++) {
                int n = Integer.parseInt(parts[i]);
                if (n < (i == 3 ? -1 : 0)) {
                    throw new XmlPullParserException("Malformed server version field: " + ver);
                }
                quad[i] = n;
            }
        } catch (NumberFormatException e) {
            throw new XmlPullParserException("Malformed server version field: " + ver);
        }
        return quad;
    }

    public int getServerMajorVersion(String serverInfo) throws XmlPullParserException, IOException {
        return getServerAppVersionQuad(serverInfo)[0];
    }

    public HardwareMetrics getHardwareMetrics() throws IOException {
        try {
            Json.Obj ok = unwrapOkObj(NightjarHttpJni.getHardwareMetrics(handle, hostCaps.toJson()), false);
            return HardwareMetrics.fromJsonObj(ok);
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException("Invalid hardware metrics response", e);
        }
    }

    public NetworkPathInfo getNetworkPath() throws IOException {
        try {
            Json.Obj reply = unwrapOk(NightjarHttpJni.getNetworkPath(handle, hostCaps.toJson()), false);
            return new NetworkPathInfo(NetworkPathInfo.Kind.valueOf(reply.getString("ok")));
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException("Invalid route response", e);
        }
    }

    public String getInternetPlay() throws IOException {
        try {
            return unwrapOk(NightjarHttpJni.getInternetPlay(handle, hostCaps.toJson()), false).getString("ok");
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException("Invalid internet play response", e);
        }
    }

    public boolean supportsFoundationAbr() throws IOException {
        try {
            return unwrapOk(NightjarHttpJni.supportsFoundationAbr(handle), false).optBoolean("ok", false);
        } catch (XmlPullParserException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public int configureFoundationAbr(org.json.JSONObject request) throws IOException {
        try {
            Json.Obj reply = unwrapOkObj(NightjarHttpJni.configureFoundationAbr(handle, request.toString()), false);
            if (!reply.optBoolean("success", false)) {
                throw new IOException(reply.optString("error", "ABR configuration rejected"));
            }
            return Math.max(0, reply.optInt("initial_bitrate", 0));
        } catch (XmlPullParserException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public org.json.JSONObject sendFoundationAbrFeedback(org.json.JSONObject feedback) throws IOException {
        try {
            Json.Obj req = Json.obj()
                    .put("packet_loss", feedback.optDouble("packetLoss", 0))
                    .put("rtt_ms", feedback.optDouble("rttMs", 0))
                    .put("decode_fps", feedback.optDouble("decodeFps", 0))
                    .put("dropped_frames", feedback.optInt("droppedFrames", 0))
                    .put("current_bitrate", feedback.optInt("currentBitrate", 0));
            Json.Obj raw = unwrapOkObj(NightjarHttpJni.sendFoundationAbrFeedback(handle, req.toString()), false);
            org.json.JSONObject out = new org.json.JSONObject();
            if (!raw.isNull("new_bitrate")) out.put("newBitrate", raw.getInt("new_bitrate"));
            out.put("bitrateApplied", raw.optBoolean("bitrate_applied", false));
            if (!raw.isNull("bitrate_apply_error")) out.put("bitrateApplyError", raw.getString("bitrate_apply_error"));
            out.put("reason", raw.optString("reason", ""));
            return out;
        } catch (XmlPullParserException | org.json.JSONException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public double probePyroWaveBandwidth(ComputerDetails details, IntConsumer progress) throws IOException {
        if (serverCert == null || details == null || !details.supportsPyroWaveBandwidthProbe()) {
            throw new IOException("This paired host does not advertise the bandwidth test");
        }
        return probePyroWaveBandwidthBytes(details.pyroWaveBandwidthProbeBytes, progress);
    }

    double probePyroWaveBandwidthBytes(int expectedBytes, IntConsumer progress) throws IOException {
        try {
            NightjarHttpJni.PercentProgress cb = progress == null ? null : progress::accept;
            return unwrapOk(NightjarHttpJni.probeBandwidth(handle, expectedBytes, cb), false)
                    .getDouble("ok");
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public boolean supportsFoundationBandwidthTest() throws IOException {
        try {
            return unwrapOk(NightjarHttpJni.supportsFoundationBandwidthTest(handle), false).optBoolean("ok", false);
        } catch (XmlPullParserException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public double probeFoundationBandwidth(IntConsumer progress) throws IOException {
        try {
            NightjarHttpJni.PercentProgress cb = progress == null ? null : progress::accept;
            return unwrapOkObj(NightjarHttpJni.probeFoundationBandwidth(handle, cb), false).getDouble("bandwidth_mbps");
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public static int probeStartKbps(int configuredKbps, double probeMbps) {
        long share = Math.round(Math.max(0.0, probeMbps) * 1000.0 * 0.8);
        return (int) Math.max(Math.min(2000, configuredKbps), Math.min(configuredKbps, share));
    }

    public double probeLinkMbps() throws IOException {
        try {
            return unwrapOk(NightjarHttpJni.probeLinkMbps(handle), false).getDouble("ok");
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public PyroWavePace.Result probePyroWaveUdp(int kbps, int packetSize, int fps, int paceKbps) throws IOException {
        try {
            return PyroWavePace.Result.fromJsonObj(unwrapOkObj(
                    NightjarHttpJni.probePyroWaveUdp(handle, kbps, packetSize, fps, paceKbps), false));
        } catch (XmlPullParserException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public double probeServerLatency() throws IOException, XmlPullParserException {
        try {
            return unwrapOk(NightjarHttpJni.probeServerLatency(handle, SHORT_CONNECTION_TIMEOUT), true)
                    .getDouble("ok");
        } catch (Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public PairingManager getPairingManager() {
        return new PairingManager(this, cryptoProvider);
    }

    static String stripLegacyOrderPrefix(String title) {
        if (title == null) return "";
        int i = 0;
        while (i < title.length() && (title.charAt(i) == '\u200B' || title.charAt(i) == '\u200C')) i++;
        return title.substring(i);
    }

    public static LinkedList<NvApp> getAppListByReader(Reader r) throws XmlPullParserException, IOException {
        return getAppListByReader(r, HostCapabilities.UNKNOWN);
    }

    public static LinkedList<NvApp> getAppListByReader(Reader r, HostCapabilities caps)
            throws XmlPullParserException, IOException {
        StringBuilder sb = new StringBuilder();
        char[] buf = new char[4096];
        int n;
        while ((n = r.read(buf)) != -1) sb.append(buf, 0, n);
        try {
            Json.Arr arr = unwrapOk(NightjarHttpJni.parseAppListXml(sb.toString(), caps.toJson()), true)
                    .getArray("ok");
            LinkedList<NvApp> list = new LinkedList<>();
            boolean monitorCatalogue = false;
            for (int i = 0; i < arr.length(); i++) {
                Json.Obj o = arr.getObject(i);
                NvApp app = new NvApp(o.getString("name"), o.getInt("id"), o.getBoolean("hdr_supported"));
                app.setHostCapabilities(caps);
                app.setAppUuid(o.optString("uuid", ""));
                if (!o.isNull("host_index")) app.setHostIndex(String.valueOf(o.getInt("host_index")));
                app.setArtVersion(o.optString("art_version", ""));
                app.setMonitorResume(o.optBoolean("monitor_resume", false));
                monitorCatalogue |= app.getControl() == NvApp.Control.DISCONNECT_MONITOR;
                list.addLast(app);
            }
            for (NvApp app : list) app.setMonitorResume(monitorCatalogue);
            return list;
        } catch (Json.SyntaxException e) {
            throw new XmlPullParserException(e.getMessage());
        }
    }

    public String getAppListRaw() throws IOException {
        try {
            return unwrapOk(NightjarHttpJni.getAppListRaw(handle), false).getString("ok");
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public LinkedList<NvApp> getAppList() throws HostHttpResponseException, IOException, XmlPullParserException {
        return getAppListByReader(new java.io.StringReader(getAppListRaw()), hostCaps);
    }

    public NvApp getAppById(int appId) throws IOException, XmlPullParserException {
        for (NvApp app : getAppList()) if (app.getAppId() == appId) return app;
        return null;
    }

    public NvApp getAppByUuid(String appUuid) throws IOException, XmlPullParserException {
        if (appUuid == null || appUuid.isEmpty()) return null;
        for (NvApp app : getAppList()) if (appUuid.equals(app.getAppUuid())) return app;
        return null;
    }

    public NvApp getAppByName(String appName) throws IOException, XmlPullParserException {
        for (NvApp app : getAppList()) if (app.getAppName().equalsIgnoreCase(appName)) return app;
        return null;
    }

    public void unpair() throws IOException {
        try { unwrapOk(NightjarHttpJni.unpair(handle), false); }
        catch (XmlPullParserException e) { throw new IOException(e.getMessage(), e); }
    }

    public InputStream getBoxArt(NvApp app) throws IOException {
        byte[] bytes = NightjarHttpJni.getBoxArt(handle, app.getAppId());
        if (bytes == null) throw new FileNotFoundException("Box art not found");
        return new ByteArrayInputStream(bytes);
    }

    public static HostDisplayChoice launchDisplayChoice(ConnectionContext context, NvApp.Role role) {
        if (!context.hostCaps.has(HostCapabilities.Feature.HOST_DISPLAY_MODES)) {
            return HostDisplayChoice.HOST_DEFAULT;
        }
        HostDisplayChoice c = context.streamConfig.getHostDisplay() != null
                ? context.streamConfig.getHostDisplay() : context.hostDisplay;
        return c == null ? HostDisplayChoice.HOST_DEFAULT : c.forRole(role);
    }

    public static boolean wantsVirtualDisplay(ConnectionContext context, NvApp.Role role) {
        return role != NvApp.Role.INPUT_ONLY
                && launchDisplayChoice(context, role).requestsVirtualDisplay(context.streamConfig.getVirtualDisplay());
    }

    public static int getStreamFrameRateMilliHz(ConnectionContext context) {
        StreamConfiguration sc = context.streamConfig;
        int streamFps = Math.max(0, sc.getRefreshRate());
        int launchFps = Math.max(0, sc.getLaunchRefreshRate());
        if (context.hostCaps.has(HostCapabilities.Feature.FRACTIONAL_REFRESH)) {
            int explicit = sc.getLaunchRefreshRateX100();
            int frac = (explicit >= 100 && explicit <= 100_000) ? explicit : 0;
            if (frac == 0) {
                int display = sc.getClientRefreshRateX100();
                if (display >= 100 && display <= 100_000 && (display + 50) / 100 == launchFps) {
                    frac = display;
                }
            }
            if (frac > 0 && (frac + 50) / 100 == streamFps) {
                return frac * 10;
            }
        }
        return streamFps * 1000;
    }

    public static boolean asksClipboardSync(ConnectionContext context) {
        NvApp app = context.streamConfig.getApp();
        return app != null && app.getRole() == NvApp.Role.STREAM && context.streamConfig.getClipboardSync()
                && getLaunchQuery(context, app.getAppId(), false).contains("&rlClipboardSync=1");
    }

    static String getLaunchQuery(ConnectionContext context, int appId, boolean enableHdr) {
        try {
            String req = buildLaunchRequestJson(context, appId, enableHdr);
            return unwrapOk(NightjarHttpJni.buildLaunchQuery(
                    req, context.hostCaps.toJson(), context.isNvidiaServerSoftware), false).getString("ok");
        } catch (IOException | XmlPullParserException | Json.SyntaxException e) {
            throw new IllegalStateException(e);
        }
    }

    public boolean launchApp(ConnectionContext context, String verb, int appId, boolean enableHdr)
            throws IOException, XmlPullParserException {
        return launchApp(context, verb, appId, enableHdr, LAUNCH_READ_TIMEOUT_SECONDS * 1000L);
    }

    boolean launchApp(ConnectionContext context, String verb, int appId, boolean enableHdr, long readTimeoutMs)
            throws IOException, XmlPullParserException {
        String req = buildLaunchRequestJson(context, appId, enableHdr);
        Json.Obj ok = unwrapOkObj(NightjarHttpJni.launchApp(
                handle, verb, req, context.hostCaps.toJson(), context.isNvidiaServerSoftware,
                (int) Math.max(1, Math.min(Integer.MAX_VALUE, readTimeoutMs))), true);
        if (ok.optBoolean("confirming", false)) {
            throw new HostHttpResponseException(ok.optInt("status", 0), ok.optString("body", ""));
        }
        if (!ok.optBoolean("ok", false)) return false;
        context.rtspSessionUrl = optNullString(ok, "session_url");
        return true;
    }

    public boolean disconnectRole(NvApp.Role role) throws IOException, XmlPullParserException {
        return unwrapOk(NightjarHttpJni.disconnectRole(handle, role.ordinal()), true, true).optBoolean("ok", false);
    }

    public boolean quitApp() throws IOException, XmlPullParserException {
        return unwrapOk(NightjarHttpJni.quitApp(handle), true).optBoolean("ok", false);
    }

    public String getClipboard() throws IOException {
        try { return unwrapOk(NightjarHttpJni.getClipboard(handle), false).getString("ok"); }
        catch (XmlPullParserException | Json.SyntaxException e) { throw new IOException(e.getMessage(), e); }
    }

    public int setBitrate(int kbps) throws IOException, XmlPullParserException {
        try { return unwrapOk(NightjarHttpJni.setBitrate(handle, kbps), false, true).getInt("ok"); }
        catch (Json.SyntaxException e) { throw new IOException(e.getMessage(), e); }
    }

    public void sendClipboard(String text) throws IOException {
        try { unwrapOk(NightjarHttpJni.sendClipboard(handle, text), false); }
        catch (XmlPullParserException e) { throw new IOException(e.getMessage(), e); }
    }

    public boolean getClipboardImage(OutputStream out) throws IOException {
        IOException[] sinkErr = new IOException[1];
        NightjarHttpJni.ByteSink sink = (buf, off, len) -> {
            try { out.write(buf, off, len); }
            catch (IOException e) { sinkErr[0] = e; throw e; }
        };
        try {
            Json.Obj reply = unwrapClipboard(NightjarHttpJni.getClipboardImage(handle, sink), sinkErr[0]);
            return !reply.isNull("ok");
        } catch (Json.SyntaxException e) { throw new IOException(e); }
    }

    public void sendClipboardImage(File png) throws IOException {
        try { unwrapClipboard(NightjarHttpJni.sendClipboardImageFile(handle, png.getAbsolutePath()), null); }
        catch (Json.SyntaxException e) { throw new IOException(e); }
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
            if ("launch_timeout".equals(kind)) throw new LaunchTimeoutException(err.optInt("seconds", 120));
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

    private static Json.Obj unwrapClipboard(String raw, IOException sinkErr) throws IOException {
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

    private static String buildLaunchRequestJson(ConnectionContext ctx, int appId, boolean enableHdr) {
        StreamConfiguration sc = ctx.streamConfig;
        NvApp app = sc.getApp();
        NvApp.Role role = app != null ? app.getRole() : NvApp.Role.STREAM;
        MoonBridge.AudioConfiguration audio = sc.getAudioConfiguration();
        String audioKind = audio == null ? "STEREO"
                : audio.channelCount == 12 ? "SURROUND_714"
                : audio.channelCount == 8 ? "SURROUND_71"
                : audio.channelCount == 6 ? "SURROUND_51" : "STEREO";
        Json.Obj j = Json.obj()
                .put("app_id", appId)
                .put("app_uuid", app != null ? app.getAppUuid() : "")
                .put("role", role.name())
                .put("small_stream_input_only", app != null && app.isSmallStreamInputOnly())
                .put("mic_only", app != null && app.isMicOnly())
                .put("width", ctx.negotiatedWidth).put("height", ctx.negotiatedHeight)
                .put("fps", sc.getLaunchRefreshRate())
                .put("launch_refresh_rate_x100", sc.getLaunchRefreshRateX100())
                .put("client_refresh_rate_x100", sc.getClientRefreshRateX100())
                .put("virtual_display", sc.getVirtualDisplay())
                .put("virtual_display_scale", sc.getVirtualDisplayScale())
                .put("virtual_display_dpi", sc.getVirtualDisplayDpi())
                .put("host_display", launchDisplayChoice(ctx, role).name())
                .put("vrr", sc.getVrr()).put("sops", sc.getSops())
                .put("play_local_audio", sc.getPlayLocalAudio())
                .put("hdr", enableHdr).put("ten_bit_sdr", !enableHdr && ctx.negotiated10Bit)
                .put("audio_surround", audioKind)
                .put("ri_key_hex", bytesToHex(ctx.riKey.getEncoded())).put("ri_key_id", ctx.riKeyId)
                .put("prepared_controllers", ctx.preparedControllers == null ? "" : ctx.preparedControllers)
                .put("gamepad_mask", sc.getAttachedGamepadMask())
                .put("persist_gamepads", sc.getPersistGamepadsAfterDisconnect())
                .put("rate_change", ctx.rateChange).put("confirm_replace", ctx.confirmReplace)
                .put("clipboard_sync", ctx.streamConfig.getClipboardSync());
        if (ctx.fecChoice != null) j.put("fec", ctx.fecChoice.wireValue);
        if (sc.getDisplayLuminance() != null) {
            j.put("display_luminance", Json.obj()
                    .put("max_nits", sc.getDisplayLuminance().maxNits)
                    .put("min_nits", sc.getDisplayLuminance().minNits)
                    .put("max_full_frame_nits", sc.getDisplayLuminance().maxFullFrameNits)
                    .put("estimated_from_ratio", sc.getDisplayLuminance().estimatedFromRatio));
        }
        if (sc.getRtxHdr() != null) j.put("rtx_hdr", sc.getRtxHdr().booleanValue());
        if (sc.getRtxVideo() != null) j.put("rtx_video", sc.getRtxVideo().booleanValue());
        return j.toString();
    }

    private static ComputerDetails.AddressTuple makeTuple(String address, int port) {
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

    private static String optNullString(Json.Obj obj, String key) {
        return !obj.has(key) || obj.isNull(key) ? null : obj.optString(key, null);
    }

    private static final char[] HEX = "0123456789ABCDEF".toCharArray();
    private static String bytesToHex(byte[] bytes) {
        char[] out = new char[bytes.length * 2];
        for (int i = 0; i < bytes.length; i++) {
            int v = bytes[i] & 0xFF;
            out[i * 2] = HEX[v >>> 4];
            out[i * 2 + 1] = HEX[v & 0x0F];
        }
        return new String(out);
    }
}
