package com.limelight.binding.video;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

/**
 * Paces decoder recoveries that keep failing before the decoder puts out a frame
 * ({@code nightjar_policy::video_queue::DecoderRecoveryBackoff}).
 */
final class DecoderRecoveryBackoff implements AutoCloseable {
    /** The wait before the 1st, 2nd... recovery in a row without a frame; one more gives up. */
    static final long[] DELAYS_MS = {0, 250, 500, 1000, 2000};

    private final long handle;
    private final HandleReaper.Token reaper;

    DecoderRecoveryBackoff() {
        this.handle = NightjarPolicyJni.queueSlabCreate(NightjarPolicyJni.QUEUE_SLAB_RECOVERY_BACKOFF);
        this.reaper = registerReaper(handle);
    }

    DecoderRecoveryBackoff(long[] delaysMs) {
        this.handle = NightjarPolicyJni.recoveryBackoffCreate(delaysMs);
        this.reaper = registerReaper(handle);
    }

    private HandleReaper.Token registerReaper(long h) {
        return HandleReaper.register(
                this,
                h,
                id -> NightjarPolicyJni.queueSlabDestroy(NightjarPolicyJni.QUEUE_SLAB_RECOVERY_BACKOFF, id));
    }

    @Override
    public void close() {
        reaper.close();
    }

    /**
     * Before a recovery, with the frames the decoder has put out so far: how long to wait first,
     * or -1 when that many recoveries in a row brought no frame and the decoder is given up.
     */
    long beforeRecovery(int framesOut) {
        return NightjarPolicyJni.recoveryBackoffOp(0, handle, framesOut);
    }

    /** The recoveries in a row that brought no frame, the one just asked about included. */
    int recoveriesWithoutFrame() {
        return (int) NightjarPolicyJni.recoveryBackoffOp(1, handle, 0);
    }
}
