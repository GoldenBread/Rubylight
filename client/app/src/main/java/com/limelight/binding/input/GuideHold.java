package com.limelight.binding.input;

import com.limelight.nvstream.jni.NightjarInputJni;

/**
 * Hold Guide to open the stream menu, for one controller. While the hold time runs Guide stays off
 * the host; let go before it and the press goes out as a short Guide tap, held past it and the stream
 * menu opens and Guide never reaches the host. Another button pressed meanwhile is a chord (Steam's
 * Guide+RB): Guide goes down at once, before that button, and is an ordinary press from then on.
 * Other buttons are never held back. The caller runs the timer and calls {@link #expire()} when it
 * ends, on the main thread, while a USB driver's pad reports on its own thread.
 */
final class GuideHold {
    enum Release {
        /** Not a press this gesture holds back: Guide's release goes to the host as usual. */
        PASS,
        /** Let go before the hold time: send Guide as a short tap now. */
        TAP,
        /** The stream menu opened for this hold, or the input was let go: the release goes nowhere. */
        SWALLOW,
    }

    private static final int OP_DOWN = 0;
    private static final int OP_EXPIRE = 1;
    private static final int OP_CHORD = 2;
    private static final int OP_UP = 3;
    private static final int OP_CANCEL_PENDING = 4;
    private static final int OP_RESET = 5;

    // Bit 0: pending, Bit 1: swallowing (mirrors nightjar_policy::controller::state_machine::GuideHoldState).
    private int state;

    private int step(int op, long holdMs, boolean repeat) {
        int packed = NightjarInputJni.guideHoldStep(state, op, holdMs, repeat);
        state = packed & 0x3;
        return packed >> 8;
    }

    synchronized int stateBits() {
        return state;
    }

    synchronized void setStateBits(int bits) {
        state = bits & 0x3;
    }

    /**
     * Guide went down. Returns whether the press is held back (the hold time starts for a new
     * press); false when holding is off, and Guide goes to the host at once.
     */
    synchronized boolean down(long holdMs, boolean repeat) {
        return step(OP_DOWN, holdMs, repeat) != 0;
    }

    /** The hold time ended; returns whether the stream menu opens now. */
    synchronized boolean expire() {
        return step(OP_EXPIRE, 0, false) != 0;
    }

    /**
     * Another button went down: returns whether a waiting Guide must go to the host now, before
     * that button. Its release is then an ordinary one ({@link Release#PASS}).
     */
    synchronized boolean chord() {
        return step(OP_CHORD, 0, false) != 0;
    }

    synchronized Release up() {
        int res = step(OP_UP, 0, false);
        if (res == 1) return Release.TAP;
        if (res == 2) return Release.SWALLOW;
        return Release.PASS;
    }

    /** Whether Guide is held back now: waiting for the hold time or until it is let go. */
    synchronized boolean active() {
        return state != 0;
    }

    /**
     * The input was let go before the hold time: no menu and no tap, and Guide, still held, stays
     * off the host until let go. Returns whether a hold was waiting.
     */
    synchronized boolean cancelPending() {
        return step(OP_CANCEL_PENDING, 0, false) != 0;
    }

    synchronized void reset() {
        step(OP_RESET, 0, false);
    }
}
