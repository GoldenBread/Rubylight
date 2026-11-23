package com.limelight.nvstream.http;

import android.app.Application;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.util.concurrent.atomic.AtomicReference;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvHTTPPaceProbeTest {
    private static final String VIBESHINE = "<root status_code=\"200\"><PairStatus>1</PairStatus>" +
            "<ServerCodecModeSupport>8388608</ServerCodecModeSupport>" +
            "<PyroWaveHostLinkMbps>2500</PyroWaveHostLinkMbps><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes>" +
            "<PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion><PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion>" +
            "<PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion><PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion>" +
            "<PyroWaveCriticalFecPercentage>25</PyroWaveCriticalFecPercentage><PyroWaveMinParityShards>3</PyroWaveMinParityShards></root>";

    @Test
    public void burstProbeHostIsReadWithItsFecBudget() throws Exception {
        ComputerDetails details = new ComputerDetails();
        NvHTTP.readClientCapabilities(details, VIBESHINE);
        assertTrue(details.pyroWaveBurstProbe);
        assertEquals(25, details.pyroWaveCriticalFecPercentage);
        assertEquals(3, details.pyroWaveMinParityShards);
        ComputerDetails copy = new ComputerDetails(details);
        assertTrue(copy.pyroWaveBurstProbe);
        assertEquals(25, copy.pyroWaveCriticalFecPercentage);
        assertEquals(3, copy.pyroWaveMinParityShards);
    }

    @Test
    public void hostWithoutBurstProbesOrHandshakeIsNotCalibrated() throws Exception {
        ComputerDetails details = new ComputerDetails();
        NvHTTP.readClientCapabilities(details, VIBESHINE.replace(
                "<PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion>", ""));
        assertFalse(details.pyroWaveBurstProbe);
        NvHTTP.readClientCapabilities(details, VIBESHINE.replace(
                "<PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion>", ""));
        assertFalse(details.pyroWaveBurstProbe);
        details.pairState = PairingManager.PairState.PAIRED;
        assertFalse(details.supportsPyroWavePaceCalibration());
    }

    @Test
    public void onlyAPlainUnprivilegedPortIsAccepted() throws Exception {
        AtomicReference<String> port = new AtomicReference<>();
        try (TestHttpServer server = new TestHttpServer(req -> {
            TestHttpServer.Response res = TestHttpServer.Response.xml(200, "<root status_code=\"200\"/>");
            if (port.get() != null) res.header("X-PyroWave-Udp-Port", port.get());
            return res;
        })) {
            NvHTTP http = server.newClient();
            for (String bad : new String[] {null, "", "80", "1023", "70000", " 5000", "+5000"}) {
                port.set(bad);
                assertThrows("port=" + bad, IOException.class,
                        () -> http.probePyroWaveUdp(5000, 1392, 60, 50000));
            }
        }
    }
}
