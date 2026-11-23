package com.limelight.update;

import android.content.Context;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHttpJni;

import java.io.File;
import java.io.IOException;

/** Downloads a release APK into the cache and checks it against the release's SHA256SUMS file via nightjar-core. */
public final class UpdateDownloader {
    public interface Progress {
        void onProgress(long done, long total);
    }

    private static final String DIRECTORY = "updates";

    private long cancelHandle;
    private boolean cancelled;

    public void cancel() {
        long handle;
        synchronized (this) {
            cancelled = true;
            handle = cancelHandle;
            if (handle != 0) {
                NightjarHttpJni.transferCancelCancel(handle);
            }
        }
    }

    static File directory(Context context) {
        return new File(context.getCacheDir(), DIRECTORY);
    }

    /** Removes downloaded APKs left by earlier updates. */
    public static void cleanUp(Context context) {
        File[] files = directory(context).listFiles();
        for (int i = 0; files != null && i < files.length; i++) {
            files[i].delete();
        }
    }

    /**
     * Returns the verified APK. The checksum comes first, so a release without one costs no download.
     * Blocks on the network; call from a background thread.
     */
    public File download(Context context, ReleaseInfo release, boolean root, Progress progress) throws IOException {
        if (release == null) {
            throw new UpdateException(UpdateException.Reason.NO_APK);
        }
        long handle = NightjarHttpJni.transferCancelCreate();
        synchronized (this) {
            cancelHandle = handle;
            if (cancelled && handle != 0) {
                NightjarHttpJni.transferCancelCancel(handle);
            }
        }
        try {
            NightjarHttpJni.FolderProgress jniProgress =
                    progress == null ? null : (done, total) -> progress.onProgress(done, total);
            String raw = NightjarHttpJni.updateDownloadApk(
                    release.toJson(),
                    root,
                    directory(context).getAbsolutePath(),
                    jniProgress,
                    handle);
            if (raw == null) {
                throw new UpdateException(UpdateException.Reason.DOWNLOAD_FAILED);
            }
            Json.Obj reply;
            try {
                reply = Json.parseObject(raw);
            } catch (Json.SyntaxException e) {
                throw new UpdateException(UpdateException.Reason.DOWNLOAD_FAILED, e);
            }
            if (reply.has("ok")) {
                return new File(reply.getString("ok"));
            }
            throw mapError(reply.optObject("error"));
        } finally {
            synchronized (this) {
                cancelHandle = 0;
            }
            if (handle != 0) {
                NightjarHttpJni.transferCancelDestroy(handle);
            }
        }
    }

    private IOException mapError(Json.Obj err) {
        String kind = err == null ? "" : err.optString("kind", "");
        String msg = err == null ? "Download failed" : err.optString("message", "Download failed");
        switch (kind) {
            case "no_apk":
                return new UpdateException(UpdateException.Reason.NO_APK);
            case "no_checksums":
                return new UpdateException(UpdateException.Reason.NO_CHECKSUMS);
            case "checksum_entry_missing":
                return new UpdateException(UpdateException.Reason.CHECKSUM_ENTRY_MISSING);
            case "checksum_mismatch":
                return new UpdateException(UpdateException.Reason.CHECKSUM_MISMATCH);
            case "cancelled":
                return new UpdateException(UpdateException.Reason.CANCELLED);
            case "io":
                return new IOException(msg);
            default:
                return new UpdateException(cancelled
                        ? UpdateException.Reason.CANCELLED
                        : UpdateException.Reason.DOWNLOAD_FAILED);
        }
    }
}
