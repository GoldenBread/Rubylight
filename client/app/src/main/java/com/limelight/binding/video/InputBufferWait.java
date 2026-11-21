package com.limelight.binding.video;

/**
 * How the renderer waits for a free decoder input buffer. A frame's submit waits for one in
 * {@link #WAIT_STEP_US} steps (rechecking recovery and shutdown between them) until
 * {@link #HANG_MS}, when the decoder counts as hung. Right after a frame is queued the renderer
 * only takes a buffer that is already free (a prefetch): with direct submit that runs on the
 * thread that receives the next frame's packets, and waiting there, with no frame waiting for the
 * buffer, held those packets in the socket. The next frame's submit does the waiting instead, so
 * the input stall count and the hang detection see only waits a frame really had.
 *
 * <p>A frame's wait itself must not hold that thread either: it waits at most
 * {@link #RECEIVE_WAIT_MS} there, then {@link SubmitHandoff} lets a worker thread do the wait. The
 * hang limit counts from the first try that found no buffer, wherever the later ones ran.
 */
final class InputBufferWait {
    static final long WAIT_STEP_US = 1000;
    static final long HANG_MS = 5000;
    /**
     * The most the thread that receives the video packets waits for a buffer before the frame goes
     * to {@link SubmitHandoff}'s worker: a few ms is as long as the socket may go undrained.
     */
    static final long RECEIVE_WAIT_MS = 3;
    static final int RECEIVE_TRIES = 3;

    private InputBufferWait() {}

    /** The dequeueInputBuffer timeout of one try. */
    static long timeoutUs(boolean prefetch) {
        return prefetch ? 0 : WAIT_STEP_US;
    }

    /**
     * Whether to ask the decoder for a buffer again after {@code attempts} tries that found none,
     * {@code waitedMs} after the first: a prefetch once, a frame's fetch until the hang limit.
     */
    static boolean askAgain(boolean prefetch, int attempts, long waitedMs) {
        if (prefetch) {
            return attempts == 0;
        }
        return attempts == 0 || waitedMs < HANG_MS;
    }

    /** Whether the receive thread asks again after {@code attempts} tries that found no buffer. */
    static boolean askAgainOnReceive(int attempts, long waitedMs) {
        return attempts == 0 || (attempts < RECEIVE_TRIES && waitedMs < RECEIVE_WAIT_MS);
    }

    /**
     * Whether ending without a buffer fails the fetch (stall count, hang detection, a key frame
     * request): only for a frame's fetch. A prefetch leaves the buffer to the next frame's.
     */
    static boolean missingBufferFails(boolean prefetch) {
        return !prefetch;
    }
}
