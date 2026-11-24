package com.limelight.nvstream;

import android.app.Application;

import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.lang.reflect.Proxy;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;

/**
 * The bitrate a stream starts at once the host chose its format: PyroWave's own when it streams
 * PyroWave, whether picked or reached from Auto (the decision keys on the negotiated format).
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {NvConnectionRecoveryTest.FakeHost.class, NvConnectionHapticsFlagTest.CapturingBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NvConnectionPyroWaveStartTest {
    private static final int HEVC = MoonBridge.VIDEO_FORMAT_H265;
    private static final int PYROWAVE = MoonBridge.VIDEO_FORMAT_PYROWAVE;

    private static int start(int negotiated, int width, int height, int fps, int bitrate, boolean automatic, int limit) {
        return start(negotiated, width, height, fps, bitrate, automatic, limit, 0);
    }

    private static int start(int negotiated, int width, int height, int fps, int bitrate, boolean automatic, int limit,
                             int linkMbps) {
        ConnectionContext context = new ConnectionContext();
        context.streamConfig = new StreamConfiguration.Builder()
                .setResolution(width, height).setRefreshRate(fps).setBitrate(bitrate)
                .setAutomaticBitrate(automatic).setBitrateLimit(limit).build();
        context.negotiatedVideoFormats = negotiated;
        context.negotiatedWidth = width;
        context.negotiatedHeight = height;
        return NvConnection.pyroWaveStartKbps(context, linkMbps);
    }

    @Test
    public void anAutomaticPyroWaveStreamStartsAtPyroWavesDefault() {
        // The automatic bitrate arrives as the H.264/HEVC default for the mode. With the link
        // unknown (Wi-Fi), the default stays within 900 Mbps unless the floor is higher.
        assertEquals(398132, start(PYROWAVE, 1920, 1080, 60, 20000, true, 0));
        assertEquals(900000, start(PYROWAVE, 2560, 1440, 120, 70000, true, 0));
        assertEquals(900000, start(PYROWAVE, 3840, 2160, 60, 80000, true, 0));
        assertEquals(900000, start(MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10, 3840, 2160, 60, 80000, true, 0));
        assertEquals(1492992, start(PYROWAVE, 3840, 2160, 120, 80000, true, 0));
    }

    // A default over gigabit broke a first PyroWave stream on ordinary Ethernet and Wi-Fi.
    @Test
    public void theDefaultStaysWithinTheWiredLink() {
        assertEquals(900000, start(PYROWAVE, 3840, 2160, 60, 80000, true, 0, 1000));
        assertEquals(1592525, start(PYROWAVE, 3840, 2160, 60, 80000, true, 0, 2500));
        assertEquals(1415578, start(PYROWAVE, 2560, 1440, 120, 70000, true, 0, 2500));
        assertEquals(186624, start(PYROWAVE, 1920, 1080, 60, 20000, true, 0, 100));
        // 10G gets the full clean target at 4K60, 1G about 900 Mbps at 1440p120, unknown 900.
        assertEquals(1592525, start(PYROWAVE, 3840, 2160, 60, 80000, true, 0, 10000));
        assertEquals(900000, start(PYROWAVE, 2560, 1440, 120, 70000, true, 0, 1000));
        assertEquals(900000, start(PYROWAVE, 2560, 1440, 120, 70000, true, 0, 0));
        // A chosen bitrate is the user's, whatever the link.
        assertEquals(1500000, start(PYROWAVE, 3840, 2160, 60, 1500000, false, 0, 1000));
    }

    @Test
    public void aStreamThatFellBackToHevcKeepsItsBitrate() {
        assertEquals(20000, start(HEVC, 1920, 1080, 60, 20000, true, 0));
        assertEquals(20000, start(HEVC, 1920, 1080, 60, 20000, false, 0));
    }

    // A chosen bitrate is kept, also below PyroWave's minimum: the bitrate setting warns instead.
    @Test
    public void aChosenBitrateIsKept() {
        assertEquals(20000, start(PYROWAVE, 1920, 1080, 60, 20000, false, 0));
        assertEquals(500000, start(PYROWAVE, 1920, 1080, 60, 500000, false, 0));
        assertEquals(100000, start(PYROWAVE, 2560, 1440, 120, 100000, false, 0));
        assertEquals(900000, start(PYROWAVE, 2560, 1440, 120, 900000, false, 0));
        assertEquals(1000000, start(PYROWAVE, 3840, 2160, 60, 1000000, false, 0));
    }

    @Test
    public void theMobileDataLimitStaysACeiling() {
        assertEquals(50000, start(PYROWAVE, 1920, 1080, 60, 20000, true, 50000));
        assertEquals(20000, start(PYROWAVE, 1920, 1080, 60, 20000, false, 100000));
        // A chosen rate above the limit was the user's own call before; it stays.
        assertEquals(300000, start(PYROWAVE, 1920, 1080, 60, 300000, false, 100000));
    }

    @Test
    public void pyroWaveStartBitrateSurvivesFullStartAppAndLaunch() throws Exception {
        for (int[] caseData : new int[][] {{0, 398132}, {50000, 50000}}) {
            NvConnectionRecoveryTest.FakeHost.initial = TestHosts.serverInfo("vibepollo-serverinfo.xml")
                    .replace("</root>", "<BandwidthProbeVersion>1</BandwidthProbeVersion></root>");
            NvConnectionRecoveryTest.FakeHost.resumeFailure = null;
            NvConnectionRecoveryTest.FakeHost.resumeResult = true;
            NvConnectionRecoveryTest.FakeHost.requests.clear();
            NvConnectionHapticsFlagTest.CapturingBridge.bitrate = -1;

            NvApp app = new NvApp("Game", 73, false);
            app.setAppUuid("game-a");
            NvConnection connection = new NvConnection(RuntimeEnvironment.getApplication(),
                    new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device",
                    new StreamConfiguration.Builder().setApp(app).setResolution(1920, 1080).setRefreshRate(60)
                            .setBitrate(20000).setAutomaticBitrate(true).setProbeFirstBitrate(true)
                            .setBitrateLimit(caseData[0]).setSupportedVideoFormats(PYROWAVE).build(),
                    crypto(), null);
            NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                    NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                    (proxy, method, args) -> null);
            connection.start(null, new VideoDecoderRenderer() {
                @Override public int setup(int format, int width, int height, int rate) { return -1; }
                @Override public void start() {}
                @Override public void stop() {}
                @Override public void cleanup() {}
                @Override public int getCapabilities() { return 0; }
                @Override public void setHdrMode(boolean enabled, byte[] metadata) {}
                @Override public int submitDecodeUnit(byte[] data, int length, int type, int frame, int frameType,
                        char latency, long received, long queued, long receivedNs, long presentationUs) { return 0; }
            }, listener);
            Thread worker = ConnectionStartup.await(connection);
            assertFalse("Connection startup did not finish", worker.isAlive());
            assertEquals(caseData[1], NvConnectionHapticsFlagTest.CapturingBridge.bitrate);
            assertEquals(caseData[1], connection.getStartBitrateKbps());
        }
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
