package com.limelight.policy;

import com.limelight.nvstream.jni.NightjarPolicyJni;

/**
 * Test-only JNI trace adapter for {@code nightjar-policy::continue_here} traces in
 * {@link PolicyTraceTest}.
 */
public final class ContinueHereTrace {
    private ContinueHereTrace() {}

    public static boolean recovers(boolean transientFailure, boolean moved) {
        return NightjarPolicyJni.continueHereRecovers(transientFailure, moved);
    }

    public static boolean readsMoveAfterEnd(boolean offered, boolean leftOnPurpose, boolean recoveryFollows) {
        return NightjarPolicyJni.continueHereReadsMoveAfterEnd(offered, leftOnPurpose, recoveryFollows);
    }

    public static boolean asksOnSelect(boolean offered, boolean plainStart, boolean sameGame) {
        return NightjarPolicyJni.continueHereAsksOnSelect(offered, plainStart, sameGame);
    }

    public static boolean cameBack(boolean moved, boolean owns) {
        return NightjarPolicyJni.continueHereCameBack(moved, owns);
    }

    public static boolean joinsAfterRead(boolean too, boolean sameGame, boolean pressFailed) {
        return NightjarPolicyJni.continueHereJoinsAfterRead(too, sameGame, pressFailed);
    }

    public static String resumeRefusal(boolean join, int code, String message) {
        switch (NightjarPolicyJni.continueHereResumeRefusal(join, code, message)) {
            case NightjarPolicyJni.RESUME_REFUSAL_SESSION_CHANGED:
                return "session_changed";
            case NightjarPolicyJni.RESUME_REFUSAL_BROWSER_STREAM:
                return "browser_stream";
            default:
                return null;
        }
    }
}
