package com.limelight.console;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarDiscoveryJni;

/**
 * The plan of waking a PC to play on it, backed by Rust {@code WakeLaunch} via
 * {@link NightjarDiscoveryJni}.
 */
final class WakeLaunch implements AutoCloseable {
    /** What to do on this tick. */
    enum Step {
        /** Nothing yet: the packet went out lately. */
        WAIT,
        /** Send a wake packet now. */
        SEND,
        /** The PC answers; carry on with the launch. */
        ONLINE,
        /** The PC did not wake in time. */
        TIMED_OUT
    }

    /** A resume from sleep takes well under this on the hardware we know of; longer counts as a failed wake. */
    static final long TIMEOUT_MS = 90_000;
    static final long RESEND_MS = 8_000;

    private final long handle;
    private final HandleReaper.Token handleToken;

    WakeLaunch(long startMs) {
        this(startMs, TIMEOUT_MS, RESEND_MS);
    }

    WakeLaunch(long startMs, long timeoutMs, long resendMs) {
        this.handle = NightjarDiscoveryJni.wakeLaunchCreate(startMs, timeoutMs, resendMs);
        this.handleToken = HandleReaper.register(this, handle, NightjarDiscoveryJni::wakeLaunchDestroy);
    }

    @Override
    public void close() {
        handleToken.close();
    }

    /** An answering PC wins over everything, including the time that ran out in the same tick. */
    Step next(boolean online, long nowMs) {
        switch (NightjarDiscoveryJni.wakeLaunchNext(handle, online, nowMs)) {
            case 0: return Step.WAIT;
            case 1: return Step.SEND;
            case 2: return Step.ONLINE;
            default: return Step.TIMED_OUT;
        }
    }
}
