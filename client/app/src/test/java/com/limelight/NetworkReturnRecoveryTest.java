package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.junit.Test;

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
