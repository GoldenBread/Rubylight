package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.limelight.nvstream.jni.NightjarJni;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

/** Recovering after the stream's network dropped: when to reconnect, and the wait for a route. */
public class NetworkReturnRecoveryTest {
    @Test public void onlyANetworkBackTheConnectionCannotCarryOnOverReconnectsAtOnce() {
        assertTrue(NightjarPolicyJni.recoveryReconnectsAtOnce(true, false, false, true, 0));
        assertFalse(NightjarPolicyJni.recoveryReconnectsAtOnce(false, false, false, true, 0));
        assertFalse(NightjarPolicyJni.recoveryReconnectsAtOnce(true, false, false, false, 0));
        assertFalse(NightjarPolicyJni.recoveryReconnectsAtOnce(true, true, false, true, 0));
        assertTrue(NightjarPolicyJni.recoveryReconnectsAtOnce(true, true, true, true, 0));
        assertTrue(NightjarPolicyJni.recoveryReconnectsAtOnce(true, false, false, true,
                NightjarPolicyJni.recoveryPeerTimeoutMs()));
        assertFalse(NightjarPolicyJni.recoveryReconnectsAtOnce(true, false, false, true, 20000));
    }

    @Test public void routeWaitReturnsReachableImmediatelyWhenProbeSucceeds() {
        NightjarJni.ensureLibraryLoaded();
        List<String> infos = new ArrayList<>();
        List<String> warns = new ArrayList<>();
        int[] probes = {0};
        int[] sleeps = {0};
        int outcome = NightjarJni.routeWait("127.0.0.1", 47989, 60000L, new NightjarJni.RouteWaitCallback() {
            @Override public long nowMs() { return 1000L; }
            @Override public String probe(String host, int port) {
                probes[0]++;
                return null;
            }
            @Override public boolean isCancelled() { return false; }
            @Override public boolean sleep(long ms) {
                sleeps[0]++;
                return true;
            }
            @Override public void logInfo(String line) { infos.add(line); }
            @Override public void logWarn(String line) { warns.add(line); }
        });
        assertEquals(0, outcome);
        assertEquals(1, probes[0]);
        assertEquals(0, sleeps[0]);
        assertTrue(infos.isEmpty());
        assertTrue(warns.isEmpty());
    }

    @Test public void routeWaitReplaysWifiBlipAndReturnsReachableAfterProbes() {
        NightjarJni.ensureLibraryLoaded();
        long networkBack = 12025L;
        long routeBack = 19777L;
        long deadline = networkBack + NightjarPolicyJni.recoveryWindowMs();
        long[] now = {networkBack};
        int[] probes = {0};
        List<String> infos = new ArrayList<>();
        List<String> warns = new ArrayList<>();
        int outcome = NightjarJni.routeWait("192.168.1.50", 47989, deadline, new NightjarJni.RouteWaitCallback() {
            @Override public long nowMs() { return now[0]; }
            @Override public String probe(String host, int port) {
                probes[0]++;
                return now[0] >= routeBack ? null : "Network is unreachable";
            }
            @Override public boolean isCancelled() { return false; }
            @Override public boolean sleep(long ms) {
                now[0] += ms;
                return true;
            }
            @Override public void logInfo(String line) { infos.add(line); }
            @Override public void logWarn(String line) { warns.add(line); }
        });
        assertEquals(0, outcome);
        assertTrue("started at " + now[0], now[0] >= routeBack && now[0] < routeBack + 500L);
        assertEquals(17, probes[0]);
        assertEquals(2, infos.size());
        assertTrue(warns.isEmpty());
    }

    @Test public void routeWaitReturnsExhaustedWhenDeadlineEnds() {
        NightjarJni.ensureLibraryLoaded();
        long[] now = {0L};
        List<String> warns = new ArrayList<>();
        int outcome = NightjarJni.routeWait("192.168.1.50", 47989, 10000L, new NightjarJni.RouteWaitCallback() {
            @Override public long nowMs() { return now[0]; }
            @Override public String probe(String host, int port) { return "No route to host"; }
            @Override public boolean isCancelled() { return false; }
            @Override public boolean sleep(long ms) {
                now[0] += ms;
                return true;
            }
            @Override public void logInfo(String line) {}
            @Override public void logWarn(String line) { warns.add(line); }
        });
        assertEquals(1, outcome);
        assertEquals(10000L, now[0]);
        assertEquals(1, warns.size());
    }

    @Test public void routeWaitReturnsCancelledOnCancelOrInterruptedSleep() {
        NightjarJni.ensureLibraryLoaded();
        int cancelledBeforeProbe = NightjarJni.routeWait("192.168.1.50", 47989, 60000L, new NightjarJni.RouteWaitCallback() {
            @Override public long nowMs() { return 0L; }
            @Override public String probe(String host, int port) { return null; }
            @Override public boolean isCancelled() { return true; }
            @Override public boolean sleep(long ms) { return true; }
            @Override public void logInfo(String line) {}
            @Override public void logWarn(String line) {}
        });
        assertEquals(2, cancelledBeforeProbe);

        int cancelledOnSleep = NightjarJni.routeWait("192.168.1.50", 47989, 60000L, new NightjarJni.RouteWaitCallback() {
            @Override public long nowMs() { return 0L; }
            @Override public String probe(String host, int port) { return "No route to host"; }
            @Override public boolean isCancelled() { return false; }
            @Override public boolean sleep(long ms) { return false; }
            @Override public void logInfo(String line) {}
            @Override public void logWarn(String line) {}
        });
        assertEquals(2, cancelledOnSleep);
    }

    @Test public void routeWaitDoesNotTreatProbeExceptionAsReachable() {
        NightjarJni.ensureLibraryLoaded();
        long[] now = {0L};
        int[] probes = {0};
        List<String> warns = new ArrayList<>();
        int outcome = NightjarJni.routeWait("192.168.1.50", 47989, 1200L, new NightjarJni.RouteWaitCallback() {
            @Override public long nowMs() { return now[0]; }
            @Override public String probe(String host, int port) {
                probes[0]++;
                throw new RuntimeException("boom");
            }
            @Override public boolean isCancelled() { return false; }
            @Override public boolean sleep(long ms) {
                now[0] += ms;
                return true;
            }
            @Override public void logInfo(String line) {}
            @Override public void logWarn(String line) { warns.add(line); }
        });
        assertEquals(1, outcome);
        assertTrue("expected multiple probes before exhaustion, got " + probes[0], probes[0] >= 2);
        assertEquals(1, warns.size());
        assertTrue(warns.get(0).contains("exception"));
    }

    @Test public void thePeerTimeoutIsTheEnginesConnectionTimeout() throws Exception {
        java.util.regex.Matcher engine = java.util.regex.Pattern.compile(
                "pub const CONTROL_PEER_TIMEOUT: Duration = Duration::from_secs\\((\\d+)\\);")
                .matcher(rust("client/src/control/driver.rs"));
        assertTrue("CONTROL_PEER_TIMEOUT in driver.rs", engine.find());
        int engineMs = Integer.parseInt(engine.group(1)) * 1000;
        java.util.regex.Matcher policy = java.util.regex.Pattern.compile(
                "pub const PEER_TIMEOUT_MS: u32 = ([\\d_]+);").matcher(rust("policy/src/recovery.rs"));
        assertTrue("PEER_TIMEOUT_MS in nightjar-policy", policy.find());
        assertEquals("nightjar-policy", engineMs, Integer.parseInt(policy.group(1).replace("_", "")));
        assertEquals(engineMs, NightjarPolicyJni.recoveryPeerTimeoutMs());
    }

    private static String rust(String path) throws java.io.IOException {
        for (String root : new String[] {"../protocol/", "protocol/"}) {
            java.io.File file = new java.io.File(root + path);
            if (file.isFile()) {
                return new String(java.nio.file.Files.readAllBytes(file.toPath()),
                        java.nio.charset.StandardCharsets.UTF_8);
            }
        }
        throw new AssertionError(path + " not found from " + new java.io.File(".").getAbsolutePath());
    }
}
