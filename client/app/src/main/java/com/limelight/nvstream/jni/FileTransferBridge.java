package com.limelight.nvstream.jni;

/**
 * File transfer (Experimental) from the Rust client (protocol/android, files.rs): one transfer list per
 * handle, on nightjar-core's shared queue, limits and refusal problems. {@link #list} and {@link #run} block
 * on HTTPS and stay off the UI thread; the rest only touch the list. Rows and errors are JSON.
 */
public final class FileTransferBridge {
    private FileTransferBridge() {}

    /** Reports how far a running transfer got: at 0, at most every 256 KiB and 100 ms, and at the end. */
    public interface Progress {
        void onProgress(long id, long done, long total);
    }

    public static boolean isAvailable() {
        return NightjarJni.isAvailable();
    }

    public static long create() {
        NightjarJni.ensureLoaded();
        return NightjarJni.fileTransferCreate();
    }

    /** Cancels what has not finished and forgets the list; workers still running finish on their own. */
    public static void destroy(long handle) {
        NightjarJni.ensureLoaded();
        NightjarJni.fileTransferDestroy(handle);
    }

    /** The new transfer's id, or -1 for a list that is gone. */
    public static long queueUpload(long handle, String localName, long size) {
        NightjarJni.ensureLoaded();
        return NightjarJni.fileTransferQueueUpload(handle, localName, size);
    }

    public static long queueDownload(long handle, String name, String localName, long size) {
        NightjarJni.ensureLoaded();
        return NightjarJni.fileTransferQueueDownload(handle, name, localName, size);
    }

    public static boolean cancel(long handle, long id) {
        NightjarJni.ensureLoaded();
        return NightjarJni.fileTransferCancel(handle, id);
    }

    public static void cancelAll(long handle) {
        NightjarJni.ensureLoaded();
        NightjarJni.fileTransferCancelAll(handle);
    }

    public static boolean retry(long handle, long id) {
        NightjarJni.ensureLoaded();
        return NightjarJni.fileTransferRetry(handle, id);
    }

    public static void clearFinished(long handle) {
        NightjarJni.ensureLoaded();
        NightjarJni.fileTransferClearFinished(handle);
    }

    /** Every transfer, oldest first: a JSON array of rows. */
    public static String snapshot(long handle) {
        NightjarJni.ensureLoaded();
        return NightjarJni.fileTransferSnapshot(handle);
    }

    /** Starts what the host's limits allow: a JSON array of {id, direction, name, local_name, size}. */
    public static String startNext(long handle) {
        NightjarJni.ensureLoaded();
        return NightjarJni.fileTransferStartNext(handle);
    }

    /** The PC's files: {"ok": {folder, free_bytes, max_file_bytes, files}} or {"error": {problem, message, ...}}. */
    public static String list(String address, int httpPort, int httpsPort, String serverCertificatePem,
                              String uniqueId, byte[] clientCertificateDer, byte[] clientPrivateKeyPkcs8) {
        NightjarJni.ensureLoaded();
        return NightjarJni.fileTransferList(address, httpPort, httpsPort, serverCertificatePem,
                uniqueId, clientCertificateDer, clientPrivateKeyPkcs8);
    }

    /**
     * Runs a started transfer on {@code fd}, which the call takes over and closes (detached, -1 when the file
     * would not open): {"ok": row} or {"error": {problem, message}} when the list or the start is gone.
     */
    public static String run(long handle, long id, String address, int httpPort, int httpsPort,
                             String serverCertificatePem, String uniqueId, byte[] clientCertificateDer,
                             byte[] clientPrivateKeyPkcs8, int fd, Progress progress) {
        NightjarJni.ensureLoaded();
        return NightjarJni.fileTransferRun(handle, id, address, httpPort, httpsPort,
                serverCertificatePem, uniqueId, clientCertificateDer, clientPrivateKeyPkcs8, fd, progress);
    }
}
