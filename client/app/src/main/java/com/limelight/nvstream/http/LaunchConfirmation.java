/**
 * In-flight host launch/terminate confirmation state and HTTP 410 classification via {@link NightjarHttpJni}.
 */
package com.limelight.nvstream.http;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHttpJni;

public final class LaunchConfirmation {
    public enum State { WAITING, CONFIRMED, CANCELLED, TIMED_OUT }

    static final long TIMEOUT_MS = 60_000;
    public static final String REPLACE_PARAMETER = "rlConfirmReplace";
    static final String REPLACE = "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.";

    private final boolean terminate;
    private final long deadlineMs;
    private State state = State.WAITING;

    public LaunchConfirmation(boolean terminate, long nowMs) {
        this.terminate = terminate;
        this.deadlineMs = nowMs + TIMEOUT_MS;
    }

    private static Json.Obj classify(HostHttpResponseException error) {
        if (error == null) return null;
        try {
            return Json.parseObject(NightjarHttpJni.launchConfirmationClassify(
                    error.getErrorCode(), error.getErrorMessage())).optObject("ok");
        } catch (Json.SyntaxException e) {
            return null;
        }
    }

    public static boolean isRequired(NvApp app, HostHttpResponseException error) {
        Json.Obj info = classify(error);
        if (info == null) return false;
        if (app == null) return false;
        if (app.getControl() == NvApp.Control.TERMINATE) {
            return info.optBoolean("terminate_required", false);
        }
        return app.getControl() == NvApp.Control.NONE && info.optBoolean("replace_required", false);
    }

    public static boolean isCompleted(NvApp app, HostHttpResponseException error) {
        Json.Obj info = classify(error);
        if (app == null || info == null) return false;
        NvApp.Control control = app.getControl();
        if (control == NvApp.Control.DISCONNECT_MONITOR) {
            return info.optBoolean("remote_monitor_completed", false);
        }
        if (control == NvApp.Control.DISCONNECT_INPUT) {
            return info.optBoolean("input_only_completed", false);
        }
        if (control == NvApp.Control.TERMINATE) {
            return info.optBoolean("terminate_completed", false);
        }
        return false;
    }

    public static long nowMs() {
        return System.nanoTime() / 1_000_000L;
    }

    public boolean isTerminate() {
        return terminate;
    }

    public synchronized State getState(long nowMs) {
        if (state == State.WAITING && nowMs >= deadlineMs) {
            state = State.TIMED_OUT;
            notifyAll();
        }
        return state;
    }

    public synchronized void confirm(long nowMs) {
        if (getState(nowMs) != State.WAITING) return;
        state = State.CONFIRMED;
        notifyAll();
    }

    public synchronized void cancel() {
        if (state != State.WAITING) return;
        state = State.CANCELLED;
        notifyAll();
    }

    public synchronized State await() throws InterruptedException {
        while (getState(nowMs()) == State.WAITING) {
            long remaining = deadlineMs - nowMs();
            if (remaining > 0) wait(remaining);
        }
        return state;
    }
}
