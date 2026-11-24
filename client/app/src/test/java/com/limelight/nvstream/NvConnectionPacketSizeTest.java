/**
 * Regression tests for packet size preflight resolution ({@code STREAM_CFG_AUTO} over
 * tunnel/VPN/cellular -> 1024 vs explicit {@code STREAM_CFG_REMOTE}/local -> 1392) and
 * XML-special-character UUID safety across {@link NightjarSessionJni}.
 */
package com.limelight.nvstream;

import android.app.Application;
import android.content.Context;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;

import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarSessionJni;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowConnectivityManager;
import org.robolectric.shadows.ShadowNetworkCapabilities;

import java.lang.reflect.Proxy;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {NvConnectionRecoveryTest.FakeHost.class, NvConnectionHapticsFlagTest.CapturingBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NvConnectionPacketSizeTest {
    private static void setActiveNetwork(int transport, boolean notVpn) {
        ConnectivityManager connMgr = (ConnectivityManager) RuntimeEnvironment.getApplication()
                .getSystemService(Context.CONNECTIVITY_SERVICE);
        ShadowConnectivityManager shadow = Shadows.shadowOf(connMgr);
        Network active = connMgr.getActiveNetwork();
        assertNotNull(active);
        NetworkCapabilities caps = ShadowNetworkCapabilities.newInstance();
        Shadows.shadowOf(caps).addTransportType(transport);
        if (notVpn) {
            Shadows.shadowOf(caps).addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN);
        }
        shadow.setNetworkCapabilities(active, caps);
    }

    private static void startStream(String hostAddress, int remoteCfg, int maxPacketSize) throws Exception {
        NvConnectionRecoveryTest.FakeHost.initial = "<root status_code=\"200\"><appversion>7.1.431.0</appversion>"
                + "<uniqueid>test-host</uniqueid><HttpsPort>47984</HttpsPort><state>SUNSHINE_SERVER_FREE</state>"
                + "<currentgame>0</currentgame><currentgameuuid></currentgameuuid><PairStatus>1</PairStatus></root>";
        NvConnectionRecoveryTest.FakeHost.resumeFailure = null;
        NvConnectionRecoveryTest.FakeHost.resumeResult = true;
        NvConnectionRecoveryTest.FakeHost.requests.clear();
        NvConnectionHapticsFlagTest.CapturingBridge.packetSize = -1;
        NvConnectionHapticsFlagTest.CapturingBridge.streamingRemotely = -1;

        NvApp app = new NvApp("Game", 73, false);
        app.setAppUuid("game-a");
        NvConnection connection = new NvConnection(RuntimeEnvironment.getApplication(),
                new ComputerDetails.AddressTuple(hostAddress, 47989), 47984, "device",
                new StreamConfiguration.Builder().setApp(app).setResolution(1280, 720).setRefreshRate(60)
                        .setRemoteConfiguration(remoteCfg).setMaxPacketSize(maxPacketSize)
                        .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264).build(),
                crypto(), null);
        NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> null);
        connection.start(null, renderer(), listener);
        Thread worker = ConnectionStartup.await(connection);
        assertFalse("Connection startup did not finish", worker.isAlive());
    }

    @Test
    public void autoOverTunnelVpnAndCellularUses1024WhileExplicitRemoteAndLocalKeep1392() throws Exception {
        // Non-VPN Wi-Fi + Auto on non-tunnel route keeps Android's default 1392.
        setActiveNetwork(NetworkCapabilities.TRANSPORT_WIFI, true);
        startStream("192.0.2.1", StreamConfiguration.STREAM_CFG_AUTO, 1392);
        assertEquals(1392, NvConnectionHapticsFlagTest.CapturingBridge.packetSize);

        // Auto + punched tunnel (127.78.x.y) over Wi-Fi with default 1392 -> 1024.
        startStream("127.78.0.1", StreamConfiguration.STREAM_CFG_AUTO, 1392);
        assertEquals(StreamConfiguration.STREAM_CFG_REMOTE, NvConnectionHapticsFlagTest.CapturingBridge.streamingRemotely);
        assertEquals(1024, NvConnectionHapticsFlagTest.CapturingBridge.packetSize);

        // Auto + VPN transport with default 1392 -> 1024.
        setActiveNetwork(NetworkCapabilities.TRANSPORT_VPN, false);
        startStream("192.0.2.1", StreamConfiguration.STREAM_CFG_AUTO, 1392);
        assertEquals(StreamConfiguration.STREAM_CFG_REMOTE, NvConnectionHapticsFlagTest.CapturingBridge.streamingRemotely);
        assertEquals(1024, NvConnectionHapticsFlagTest.CapturingBridge.packetSize);

        // Auto + Cellular transport with default 1392 -> 1024.
        setActiveNetwork(NetworkCapabilities.TRANSPORT_CELLULAR, true);
        startStream("192.0.2.1", StreamConfiguration.STREAM_CFG_AUTO, 1392);
        assertEquals(StreamConfiguration.STREAM_CFG_REMOTE, NvConnectionHapticsFlagTest.CapturingBridge.streamingRemotely);
        assertEquals(1024, NvConnectionHapticsFlagTest.CapturingBridge.packetSize);

        // Explicit STREAM_CFG_REMOTE keeps streamConfig.getMaxPacketSize() (1392).
        startStream("127.78.0.1", StreamConfiguration.STREAM_CFG_REMOTE, 1392);
        assertEquals(StreamConfiguration.STREAM_CFG_REMOTE, NvConnectionHapticsFlagTest.CapturingBridge.streamingRemotely);
        assertEquals(1392, NvConnectionHapticsFlagTest.CapturingBridge.packetSize);

        // Explicit STREAM_CFG_LOCAL keeps streamConfig.getMaxPacketSize() (1392).
        startStream("192.0.2.1", StreamConfiguration.STREAM_CFG_LOCAL, 1392);
        assertEquals(StreamConfiguration.STREAM_CFG_LOCAL, NvConnectionHapticsFlagTest.CapturingBridge.streamingRemotely);
        assertEquals(1392, NvConnectionHapticsFlagTest.CapturingBridge.packetSize);

        // Auto + tunnel with explicit custom packet size preserves the explicit size.
        setActiveNetwork(NetworkCapabilities.TRANSPORT_WIFI, true);
        startStream("127.78.0.1", StreamConfiguration.STREAM_CFG_AUTO, 1200);
        assertEquals(1200, NvConnectionHapticsFlagTest.CapturingBridge.packetSize);
    }

    @Test
    public void jniResolvePacketSizePreflightDistinguishesAutoVpnFromExplicitRemote() {
        int[] autoVpn = NightjarSessionJni.resolvePacketSizePreflight(
                0, false, true, 8192, true, 1392, false, false, null);
        assertArrayEquals(new int[] {1024, 0}, autoVpn);

        int[] explicitRemote = NightjarSessionJni.resolvePacketSizePreflight(
                2, false, true, 8192, true, 1392, false, false, null);
        assertArrayEquals(new int[] {1392, 0}, explicitRemote);
    }

    @Test
    public void xmlSpecialCharactersInCurrentGameUuidDoNotAbortJniLaunchOrServerInfoCheck() {
        NightjarSessionJni.HostCallback noop = new NightjarSessionJni.HostCallback() {
            @Override public String serverInfo(boolean fast) { return "{\"error\":{\"kind\":\"cancelled\",\"message\":\"\"}}"; }
            @Override public boolean hasEndedSession() { return false; }
            @Override public String launch(String verb, int appId, boolean hdr, boolean confirmReplace) {
                return "{\"ok\":{\"started\":false}}";
            }
            @Override public String quitApp() { return "{\"ok\":false}"; }
            @Override public int confirmReplace(boolean terminateOnly) { return 3; }
            @Override public boolean isStopped() { return false; }
        };
        for (String uuid : new String[] {"bad<uuid", "a&b"}) {
            String reply = NightjarSessionJni.launchOrResume(
                    "{\"paired\":true,\"current_game\":1,\"current_game_uuid\":\"" + uuid + "\",\"session_idle\":false}",
                    "{\"app_id\":73,\"app_uuid\":\"game-a\",\"app_name\":\"Game\",\"control\":0,\"role\":0}",
                    "{}",
                    noop);
            assertNotNull(reply);
            assertFalse(NightjarSessionJni.serverinfoConfirmsEndedSession(
                    false, 0, false, true, true, true, 0, uuid));
        }
    }

    private static VideoDecoderRenderer renderer() {
        return new VideoDecoderRenderer() {
            @Override public int setup(int format, int width, int height, int rate) { return -1; }
            @Override public void start() {}
            @Override public void stop() {}
            @Override public void cleanup() {}
            @Override public int getCapabilities() { return 0; }
            @Override public void setHdrMode(boolean enabled, byte[] metadata) {}
            @Override public int submitDecodeUnit(byte[] data, int length, int type, int frame, int frameType,
                    char latency, long received, long queued, long receivedNs, long presentationUs) { return 0; }
        };
    }

    private static LimelightCryptoProvider crypto() {
        return new LimelightCryptoProvider() {
            @Override public java.security.cert.X509Certificate getClientCertificate() { return null; }
            @Override public java.security.PrivateKey getClientPrivateKey() { return null; }
            @Override public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            @Override public String encodeBase64String(byte[] data) { return ""; }
        };
    }
}
