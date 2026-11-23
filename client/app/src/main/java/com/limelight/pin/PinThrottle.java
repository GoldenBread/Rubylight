/**
 * Backoff after wrong PINs backed by {@code nightjar_policy::pin::throttle} via
 * {@link NightjarPinJni}.
 */
package com.limelight.pin;

import com.limelight.nvstream.jni.NightjarPinJni;

final class PinThrottle {
    static final int FREE_FAILURES = 4;
    static final long BASE_MS = 30_000;
    static final long MAX_MS = 30 * 60_000L;

    private PinThrottle() {
    }

    /** What is stored: wrong tries in a row, when the lock ends and on which boot that time counts. */
    static final class State {
        final int fails;
        final long lockUntil;
        final long lockBoot;

        State(int fails, long lockUntil, long lockBoot) {
            this.fails = fails;
            this.lockUntil = lockUntil;
            this.lockBoot = lockBoot;
        }

        static final State CLEAN = new State(0, 0, 0);
    }

    /** The wait that follows the {@code fails}-th wrong try in a row. */
    static long delayAfter(int fails) {
        return NightjarPinJni.throttleDelayAfter(fails);
    }

    /** The state seen from boot {@code boot}: a lock written on another boot is renewed at its current step. */
    static State onBoot(State s, long now, long boot) {
        long[] res = NightjarPinJni.throttleOnBoot(s.fails, s.lockUntil, s.lockBoot, now, boot);
        if (res == null || res.length < 4 || res[3] == 0) {
            return s;
        }
        return new State((int) res[0], res[1], res[2]);
    }

    /** How long input stays refused; never more than the longest step, whatever was stored. */
    static long remainingMs(State s, long now) {
        return NightjarPinJni.throttleRemainingMs(s.lockUntil, now);
    }

    /** The state after one more wrong (or not yet verified) try. */
    static State failed(State s, long now, long boot) {
        long[] res = NightjarPinJni.throttleFailed(s.fails, now, boot);
        return new State((int) res[0], res[1], res[2]);
    }
}
