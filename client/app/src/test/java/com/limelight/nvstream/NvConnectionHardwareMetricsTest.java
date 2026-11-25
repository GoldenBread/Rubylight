package com.limelight.nvstream;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HardwareMetrics;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;

import org.junit.Test;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Consumer;

import static org.junit.Assert.*;

public class NvConnectionHardwareMetricsTest {
    @Test
    public void unsupportedHostNeverStartsPolling() {
        NvConnection connection = connection();
        assertFalse(connection.startHardwareMetricsPolling(metrics -> fail("Unsupported host was polled")));
        connection.stopHardwareMetricsPolling();
        connection.stop();
    }

    @Test
    public void hidingCancelsOnlyTelemetryAndLateResponsesAreDiscarded() throws Exception {
        NvConnection connection = supportedConnection();
        CountDownLatch entered = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        CountDownLatch exited = new CountDownLatch(1);
        AtomicReference<Thread> worker = new AtomicReference<>();
        AtomicInteger callbacks = new AtomicInteger();
        Http telemetry = new Http() {
            @Override public HardwareMetrics getHardwareMetrics() {
                worker.set(Thread.currentThread());
                entered.countDown();
                try {
                    release.await(2, TimeUnit.SECONDS);
                    return sample(1000);
                } catch (InterruptedException e) {
                    return sample(1000);
                } finally {
                    exited.countDown();
                }
            }
            @Override public void cancelPendingRequests() {
                super.cancelPendingRequests();
                release.countDown();
            }
        };
        Http commands = new Http();
        field(connection, "hardwareMetricsHttp", telemetry);
        field(connection, "http", commands);
        try {
            assertTrue(connection.startHardwareMetricsPolling(metrics -> callbacks.incrementAndGet()));
            assertTrue(entered.await(2, TimeUnit.SECONDS));
            assertTrue(connection.startHardwareMetricsPolling(metrics -> fail("Second poller was started")));
            assertNotSame(Thread.currentThread(), worker.get());
            connection.stopHardwareMetricsPolling();
            assertTrue(exited.await(2, TimeUnit.SECONDS));
            worker.get().join(2000);
            assertFalse(worker.get().isAlive());
            assertEquals(1, telemetry.cancellations);
            assertEquals(0, commands.cancellations);
            assertEquals(0, callbacks.get());
        } finally {
            connection.stop();
        }
    }

    @Test
    public void deniedUnsupportedAndInactiveSessionsRemainDisabledAfterOverlayToggle() throws Exception {
        for (IOException failure : new IOException[] {new HostHttpResponseException(403, "Denied"),
                new HostHttpResponseException(409, "No session"), new FileNotFoundException(),
                new HardwareMetrics.UnsupportedVersionException()}) {
            NvConnection connection = supportedConnection();
            Http telemetry = new Http();
            telemetry.failure = failure;
            field(connection, "hardwareMetricsHttp", telemetry);
            CountDownLatch delivered = new CountDownLatch(1);
            try {
                assertTrue(connection.startHardwareMetricsPolling(metrics -> {
                    assertNull(metrics);
                    delivered.countDown();
                }));
                assertTrue(delivered.await(2, TimeUnit.SECONDS));
                connection.stopHardwareMetricsPolling();
                assertFalse(connection.startHardwareMetricsPolling(metrics -> fail("Terminal failure was retried")));
            } finally {
                connection.stop();
            }
        }
    }

    @Test
    public void cadenceHonorsHostIntervalAndTemporaryFailuresBackOff() throws Exception {
        for (int interval : new int[] {500, 10000, -1}) {
            NvConnection connection = supportedConnection();
            ScheduledThreadPoolExecutor executor = new ScheduledThreadPoolExecutor(1);
            Http telemetry = new Http();
            if (interval == -1) telemetry.failure = new HostHttpResponseException(503, "Waiting for sample");
            else telemetry.sample = sample(interval);
            field(connection, "hardwareMetricsHttp", telemetry);
            field(connection, "hardwareMetricsPoller", executor);
            AtomicReference<HardwareMetrics> received = new AtomicReference<>();
            Method poll = NvConnection.class.getDeclaredMethod("pollHardwareMetrics",
                    java.util.concurrent.ScheduledExecutorService.class, Consumer.class);
            poll.setAccessible(true);
            try {
                poll.invoke(connection, executor, (Consumer<HardwareMetrics>) received::set);
                assertSame(interval == -1 ? null : telemetry.sample, received.get());
                assertEquals(1, executor.getQueue().size());
                long delay = ((java.util.concurrent.Delayed) executor.getQueue().peek()).getDelay(TimeUnit.MILLISECONDS);
                long expected = interval == 500 ? 2000 : 10000;
                assertTrue("Expected delay " + expected + ", got " + delay, delay > expected - 1000 && delay <= expected);
            } finally {
                connection.stop();
            }
        }
    }

    @Test
    public void pollReturnsEarlyWithoutSendingHttpWhenNotDueOrUnavailable() throws Exception {
        NvConnection connection = supportedConnection();
        ScheduledThreadPoolExecutor executor = new ScheduledThreadPoolExecutor(1);
        AtomicInteger httpCalls = new AtomicInteger();
        Http telemetry = new Http() {
            @Override public HardwareMetrics getHardwareMetrics() throws IOException {
                httpCalls.incrementAndGet();
                return super.getHardwareMetrics();
            }
        };
        telemetry.sample = sample(10_000);
        field(connection, "hardwareMetricsHttp", telemetry);
        field(connection, "hardwareMetricsPoller", executor);
        AtomicInteger deliveries = new AtomicInteger();
        Method poll = NvConnection.class.getDeclaredMethod("pollHardwareMetrics",
                java.util.concurrent.ScheduledExecutorService.class, Consumer.class);
        poll.setAccessible(true);
        try {
            // First poll succeeds and schedules 10s later (nowMs < next_ms).
            poll.invoke(connection, executor, (Consumer<HardwareMetrics>) m -> deliveries.incrementAndGet());
            assertEquals(1, httpCalls.get());
            assertEquals(1, deliveries.get());

            // Immediate early re-invocation returns -1 from metricsPollerDue -> must NOT poll HTTP.
            poll.invoke(connection, executor, (Consumer<HardwareMetrics>) m -> deliveries.incrementAndGet());
            assertEquals(1, httpCalls.get());
            assertEquals(1, deliveries.get());

            // When unavailable is latched, polling returns early and stops the poller without HTTP.
            Field handleField = NvConnection.class.getDeclaredField("hardwareMetricsPollerHandle");
            handleField.setAccessible(true);
            long handle = (Long) handleField.get(connection);
            int gen = com.limelight.nvstream.jni.NightjarSessionJni.metricsPollerDue(
                    handle, Long.MAX_VALUE / 4, true);
            com.limelight.nvstream.jni.NightjarSessionJni.metricsPollerFinishStep(
                    handle, gen, Long.MAX_VALUE / 4, false, 0L, 0, 403, false);
            poll.invoke(connection, executor, (Consumer<HardwareMetrics>) m -> deliveries.incrementAndGet());
            assertEquals(1, httpCalls.get());
            assertTrue(executor.isShutdown());
        } finally {
            connection.stop();
        }
    }

    @Test
    public void newlyCreatedTelemetryHttpInheritsHostCapabilitiesAndFetchesMetrics() throws Exception {
        String payload = "{\"version\":1,\"sampled_at_unix_ms\":1791712800000,\"interval_ms\":1000,\"cpu\":{},\"gpus\":[]}";
        try (com.limelight.nvstream.http.TestHttpServer server = new com.limelight.nvstream.http.TestHttpServer(req -> {
            assertEquals("https", req.scheme);
            assertEquals("/api/hardware-metrics", req.path);
            return com.limelight.nvstream.http.TestHttpServer.Response.json(200, payload);
        })) {
            NvConnection connection = new NvConnection(null,
                    new ComputerDetails.AddressTuple("127.0.0.1", server.httpPort()),
                    server.httpsPort(), "device", new StreamConfiguration.Builder().build(),
                    com.limelight.nativeapi.NativeApiFixtures.crypto(), server.serverCert());
            ComputerDetails details = connection.getHostDetails();
            details.caps = com.limelight.nvstream.http.TestHosts.rubylight();
            details.pairState = PairingManager.PairState.PAIRED;
            details.httpsPort = server.httpsPort();

            ScheduledThreadPoolExecutor executor = new ScheduledThreadPoolExecutor(1);
            field(connection, "hardwareMetricsPoller", executor);
            AtomicReference<HardwareMetrics> received = new AtomicReference<>();
            Method poll = NvConnection.class.getDeclaredMethod("pollHardwareMetrics",
                    java.util.concurrent.ScheduledExecutorService.class, Consumer.class);
            poll.setAccessible(true);
            try {
                // Warm up loopback TLS provider before the 2 s hardware-metrics call timeout.
                NvHTTP unseeded = server.newClient("device", true, com.limelight.nativeapi.NativeApiFixtures.crypto());
                assertThrows("Unseeded NvHTTP must reject locally before hitting network",
                        IOException.class, unseeded::getHardwareMetrics);
                assertEquals(0, server.requests().size());
                unseeded.setHostCapabilities(details.caps);
                assertEquals(1000, unseeded.getHardwareMetrics().intervalMs);
                assertEquals(1, server.requests().size());

                // hardwareMetricsHttp is null: pollHardwareMetrics() must construct NvHTTP and seed hostCaps.
                poll.invoke(connection, executor, (Consumer<HardwareMetrics>) received::set);
                Field httpField = NvConnection.class.getDeclaredField("hardwareMetricsHttp");
                httpField.setAccessible(true);
                NvHTTP created = (NvHTTP) httpField.get(connection);
                assertNotNull(created);
                assertEquals(details.caps, created.getHostCapabilities());
                if (received.get() == null) {
                    // Surface the exact transport exception instead of swallowing it in pollHardwareMetrics().
                    created.getHardwareMetrics();
                }
                assertNotNull("Expected live hardware metrics sample (hostCaps seeded on NvHTTP)", received.get());
                assertEquals(1000, received.get().intervalMs);
                assertEquals(2, server.requests().size());
            } finally {
                connection.stop();
            }
        }
    }

    private static HardwareMetrics sample(int interval) {
        return new HardwareMetrics(1, interval, Map.of(), List.of());
    }

    private static NvConnection connection() {
        return new NvConnection(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989),
                47984, "test", new StreamConfiguration.Builder().build(), null, null);
    }

    private static NvConnection supportedConnection() {
        NvConnection connection = connection();
        ComputerDetails details = connection.getHostDetails();
        details.caps = com.limelight.nvstream.http.TestHosts.rubylight();
        details.pairState = PairingManager.PairState.PAIRED;
        details.httpsPort = 47984;
        return connection;
    }

    private static void field(NvConnection connection, String name, Object value) throws Exception {
        Field field = NvConnection.class.getDeclaredField(name);
        field.setAccessible(true);
        field.set(connection, value);
    }

    private static class Http extends NvHTTP {
        IOException failure;
        HardwareMetrics sample = sample(1000);
        int cancellations;

        Http() throws IOException {
            super(new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test", null,
                    new LimelightCryptoProvider() {
                        public java.security.cert.X509Certificate getClientCertificate() { return null; }
                        public java.security.PrivateKey getClientPrivateKey() { return null; }
                        public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
                        public String encodeBase64String(byte[] data) { return ""; }
                    });
        }

        @Override public HardwareMetrics getHardwareMetrics() throws IOException {
            if (failure != null) throw failure;
            return sample;
        }

        @Override public void cancelPendingRequests() { cancellations++; }
    }
}
