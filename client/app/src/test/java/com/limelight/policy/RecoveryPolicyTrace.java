package com.limelight.policy;

import com.limelight.nvstream.jni.NightjarPolicyJni;

/**
 * Test-only JNI trace adapter for {@code nightjar-policy::recovery::StreamRecoveryPolicy}
 * traces in {@link PolicyTraceTest}.
 */
public final class RecoveryPolicyTrace {
    private int attempts;
    private long startedAt;

    public RecoveryPolicyTrace(int attempts, long startedAt) {
        this.attempts = Math.max(0, Math.min(NightjarPolicyJni.recoveryMaxAttempts(), attempts));
        this.startedAt = Math.max(0L, startedAt);
    }

    public long nextDelay(long now, long connectedAt) {
        long[] res = NightjarPolicyJni.recoveryNextDelay(attempts, startedAt, now, connectedAt);
        attempts = (int) res[1];
        startedAt = res[2];
        return res[0];
    }

    public void resetAfterStableConnection(long now, long connectedAt) {
        long[] res = NightjarPolicyJni.recoveryResetAfterStableConnection(attempts, startedAt, now, connectedAt);
        attempts = (int) res[0];
        startedAt = res[1];
    }

    public boolean allowsAttempt(long now, long connectedAt) {
        return NightjarPolicyJni.recoveryCanAutoRecover(
                true, false, true, true, true, false, false, true, false, true,
                attempts, startedAt, now, connectedAt);
    }

    public int attempts() {
        return attempts;
    }

    public long startedAt() {
        return startedAt;
    }

    public static boolean recoversTermination(int code, boolean videoStarted) {
        return NightjarPolicyJni.recoveryRecoversTermination(code, videoStarted);
    }

    public static boolean transientTermination(int code) {
        return NightjarPolicyJni.recoveryTransientTermination(code);
    }

    public static boolean transportTermination(int code) {
        return NightjarPolicyJni.recoveryTransportTermination(code);
    }

    public static boolean transientStartup(String stage, int portFlags, int error) {
        return NightjarPolicyJni.recoveryTransientStartup(stage, portFlags, error);
    }
}
