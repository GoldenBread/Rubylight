/**
 * Native transfer cancellation token backed by {@code nightjar_core::http::TransferCancel}.
 */
package com.limelight.nvstream.http;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarHttpJni;

public final class TransferCancel implements AutoCloseable {
    private final long handle;
    private final HandleReaper.Token reaper;

    public TransferCancel() {
        this.handle = NightjarHttpJni.transferCancelCreate();
        this.reaper = HandleReaper.register(this, handle, NightjarHttpJni::transferCancelDestroy);
    }

    public void cancel() {
        NightjarHttpJni.transferCancelCancel(handle);
    }

    public boolean isCancelled() {
        return NightjarHttpJni.transferCancelIsCancelled(handle);
    }

    long handle() {
        return handle;
    }

    @Override
    public void close() {
        reaper.close();
    }
}
