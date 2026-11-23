package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nvstream.ConnectionContext;
import com.limelight.nvstream.PyroWavePace;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.Set;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

/** The UDP handshake and receiver over real TLS + nightjar-core JNI against a local stand-in for Vibeshine's /pyrowave-udp-probe. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvHTTPUdpProbeTest {
    private static final InetAddress LOOPBACK = InetAddress.getLoopbackAddress();
    private static final int KBPS = 5000, PACKET = 1392, FPS = 60, PACE = 50000;

    private static TestHttpServer server(Set<Integer> drop, boolean strangerTraffic) throws IOException {
        return new TestHttpServer(req -> {
            assertEquals("/pyrowave-udp-probe", req.path);
            assertEquals("1", req.queryParam("handshake"));
            assertEquals(String.valueOf(FPS), req.queryParam("fps"));
            assertEquals(String.valueOf(PACE), req.queryParam("pacekbps"));
            byte[] token = req.queryParam("token").getBytes(StandardCharsets.US_ASCII);
            assertEquals(32, token.length);
            InetSocketAddress client = new InetSocketAddress(LOOPBACK, Integer.parseInt(req.queryParam("port")));
            long expected = PyroWavePace.expectedPackets(
                    Integer.parseInt(req.queryParam("kbps")),
                    Integer.parseInt(req.queryParam("packetsize")));
            return TestHttpServer.Response.raw(out -> {
                long sent = 0;
                try (DatagramSocket socket = new DatagramSocket(new InetSocketAddress(LOOPBACK, 0));
                     DatagramSocket stranger = new DatagramSocket(new InetSocketAddress(LOOPBACK, 0))) {
                    socket.setSoTimeout(1500);
                    String head = "HTTP/1.1 200 OK\r\n" +
                            "Content-Type: application/xml\r\n" +
                            "X-PyroWave-Udp-Port: " + socket.getLocalPort() + "\r\n";
                    byte[] hello = new byte[64];
                    DatagramPacket in = new DatagramPacket(hello, hello.length);
                    String xml = "<root status_code=\"200\"><expected>" + expected + "</expected><sent>" + expected +
                            "</sent><elapsedMs>2000.4</elapsedMs><sendRetries>0</sendRetries><lastSendError>0</lastSendError></root>";
                    byte[] body = xml.getBytes(StandardCharsets.UTF_8);
                    out.write((head + "Content-Length: " + body.length + "\r\nConnection: close\r\n\r\n")
                            .getBytes(StandardCharsets.US_ASCII));
                    out.flush();

                    socket.receive(in);
                    assertEquals(32, in.getLength());
                    byte[] packet = new byte[PACKET + PyroWavePace.PROBE_EXTRA_BYTES];
                    System.arraycopy(token, 0, packet, 0, 32);
                    for (int seq = 0; seq < expected; seq++) {
                        packet[32] = (byte) (seq >>> 24);
                        packet[33] = (byte) (seq >>> 16);
                        packet[34] = (byte) (seq >>> 8);
                        packet[35] = (byte) seq;
                        sent++;
                        if (strangerTraffic) {
                            stranger.send(new DatagramPacket(packet, packet.length, client));
                        }
                        if (!drop.contains(seq)) {
                            socket.send(new DatagramPacket(packet, packet.length, client));
                        }
                        if (seq % 64 == 0) Thread.sleep(1);
                    }
                    assertEquals(expected, sent);
                    out.write(body);
                    out.flush();
                }
            });
        });
    }

    @Test
    public void completeProbeArrivesAndQualifies() throws Exception {
        try (TestHttpServer s = server(Set.of(), false)) {
            PyroWavePace.Result result = s.newClient().probePyroWaveUdp(KBPS, PACKET, FPS, PACE);
            assertEquals(PyroWavePace.expectedPackets(KBPS, PACKET), result.expected);
            assertEquals(result.expected, result.received);
            assertEquals(120, result.frames);
            assertTrue(PyroWavePace.qualified(result));
        }
    }

    @Test
    public void lostFrameTailsAreCounted() throws Exception {
        long expected = PyroWavePace.expectedPackets(KBPS, PACKET);
        java.util.HashSet<Integer> drop = new java.util.HashSet<>();
        for (int frame = 0; frame < 10; frame++) {
            drop.add((int) (expected * (frame + 1) / 120 - 1));
        }
        try (TestHttpServer s = server(drop, false)) {
            PyroWavePace.Result result = s.newClient().probePyroWaveUdp(KBPS, PACKET, FPS, PACE);
            assertEquals(expected - 10, result.received);
            assertEquals(10, result.missingTailFrames);
            assertFalse(PyroWavePace.qualified(result));
        }
    }

    @Test
    public void packetsFromAnotherPortAreIgnored() throws Exception {
        try (TestHttpServer s = server(Set.of(0, 1, 2, 3, 4, 5, 6, 7, 8, 9), true)) {
            PyroWavePace.Result result = s.newClient().probePyroWaveUdp(KBPS, PACKET, FPS, PACE);
            assertEquals(result.expected - 10, result.received);
        }
    }

    @Test
    public void hostWithoutAnAnnouncedPortFails() throws Exception {
        try (TestHttpServer s = new TestHttpServer(req -> TestHttpServer.Response.xml(200, "<root status_code=\"200\"/>"))) {
            assertThrows(IOException.class, () -> s.newClient().probePyroWaveUdp(KBPS, PACKET, FPS, PACE));
        }
    }

    @Test
    public void invalidUdpPortHeadersAreRejectedWhileBoundaryPortsConnect() throws Exception {
        AtomicInteger portHeaderMode = new AtomicInteger();
        String[] badPorts = {"", "80", "1023", "70000", " 5000", "+5000"};
        try (TestHttpServer s = new TestHttpServer(req -> {
            int idx = portHeaderMode.get();
            return TestHttpServer.Response.xml(200, "<root status_code=\"200\"/>")
                    .header("X-PyroWave-Udp-Port", badPorts[idx]);
        })) {
            NvHTTP http = s.newClient();
            for (int i = 0; i < badPorts.length; i++) {
                portHeaderMode.set(i);
                assertThrows("port " + badPorts[i], IOException.class,
                        () -> http.probePyroWaveUdp(KBPS, PACKET, FPS, PACE));
            }
        }
    }

    @Test
    public void mismatchedPacketCountIsRejected() throws Exception {
        try (TestHttpServer s = new TestHttpServer(req -> {
            byte[] token = req.queryParam("token").getBytes(StandardCharsets.US_ASCII);
            InetSocketAddress client = new InetSocketAddress(LOOPBACK, Integer.parseInt(req.queryParam("port")));
            long expected = PyroWavePace.expectedPackets(
                    Integer.parseInt(req.queryParam("kbps")),
                    Integer.parseInt(req.queryParam("packetsize")));
            return TestHttpServer.Response.raw(out -> {
                try (DatagramSocket socket = new DatagramSocket(new InetSocketAddress(LOOPBACK, 0))) {
                    socket.setSoTimeout(1500);
                    String xml = "<root status_code=\"200\"><expected>1</expected><sent>1</sent><elapsedMs>2000</elapsedMs></root>";
                    byte[] body = xml.getBytes(StandardCharsets.UTF_8);
                    out.write(("HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\n" +
                            "X-PyroWave-Udp-Port: " + socket.getLocalPort() + "\r\nContent-Length: " +
                            body.length + "\r\nConnection: close\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
                    out.flush();
                    socket.receive(new DatagramPacket(new byte[64], 64));
                    byte[] packet = new byte[PACKET + PyroWavePace.PROBE_EXTRA_BYTES];
                    System.arraycopy(token, 0, packet, 0, 32);
                    for (int seq = 0; seq < expected; seq++) {
                        packet[32] = (byte) (seq >>> 24);
                        packet[33] = (byte) (seq >>> 16);
                        packet[34] = (byte) (seq >>> 8);
                        packet[35] = (byte) seq;
                        socket.send(new DatagramPacket(packet, packet.length, client));
                        if (seq % 64 == 0) Thread.sleep(1);
                    }
                    out.write(body);
                    out.flush();
                }
            });
        })) {
            assertThrows(IOException.class, () -> s.newClient().probePyroWaveUdp(KBPS, PACKET, FPS, PACE));
        }
    }
}
