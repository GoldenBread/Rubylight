package com.limelight.computers;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nvstream.jni.NightjarHostsJni;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.concurrent.Executor;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Update the PC's host via {@link NightjarHostsJni#hostFlowUpdateRun}: POST /update/install,
 * then poll GET /update up to {@link #WAIT_MS} active wait ({@link #MAX_WAIT_MS} overall).
 */
final class HostUpdateFlow {
    static final long WAIT_MS = NightjarPolicyJni.hostUpdateWaitMs();
    static final long MAX_WAIT_MS = NightjarPolicyJni.hostUpdateMaxWaitMs();
    static final long POLL_MS = NightjarPolicyJni.hostUpdatePollMs();

    interface Sleeper {
        void sleep(long ms) throws InterruptedException;
    }

    interface Ui {
        /** The host took the request and installs {@code version}. */
        void started(String version);

        /** The install waits for another device's stream: the host's words, once per reason. */
        void waiting(String reason);

        /** The PC answers with {@code version}. */
        void back(String version);

        /** The PC did not come back with {@code version} within {@link #WAIT_MS}, or {@link #MAX_WAIT_MS} in all. */
        void timedOut(String version);

        /** The host's 202 offers no version to install: nothing to wait for. */
        void nothingToInstall();

        /** The PC's updater failed: its words, or null when it gave none. */
        void updateFailed(String error);

        /** The PC came back on {@code version}, not the new one: setup gave up, with the updater's error or null. */
        void cameBackOn(String version, String error);

        /** The host refused: its words as sent. */
        void refused(String message);

        /** The host could not be asked or its answer was unreadable. */
        void failed(NativeApiException error);
    }

    private final SleepFlow.Client client;
    private final Ui ui;
    private final Executor worker;
    private final Executor main;
    private final Sleeper sleeper;
    private final AtomicBoolean cancelled = new AtomicBoolean();

    HostUpdateFlow(SleepFlow.Client client, Ui ui, Executor worker, Executor main, Sleeper sleeper) {
        this.client = client;
        this.ui = ui;
        this.worker = worker;
        this.main = main;
        this.sleeper = sleeper;
    }

    void start() {
        worker.execute(() -> {
            NativeApiClient api;
            try {
                api = client.open();
            } catch (NativeApiException e) {
                main.execute(() -> {
                    if (PcPowerFlow.isRefusal(e)) {
                        ui.refused(e.getMessage());
                    } else {
                        ui.failed(e);
                    }
                });
                return;
            }
            NightjarHostsJni.hostFlowUpdateRun(
                    api::requestRaw,
                    ms -> {
                        if (cancelled.get()) {
                            return false;
                        }
                        try {
                            sleeper.sleep(ms);
                            return !cancelled.get();
                        } catch (InterruptedException e) {
                            Thread.currentThread().interrupt();
                            return false;
                        }
                    },
                    stepJson -> {
                        if (cancelled.get()) {
                            return;
                        }
                        try {
                            JSONObject step = new JSONObject(stepJson);
                            main.execute(() -> deliverStep(step));
                        } catch (JSONException ignored) {
                        }
                    });
        });
    }

    private void deliverStep(JSONObject step) {
        if (cancelled.get()) {
            return;
        }
        switch (step.optString("outcome")) {
            case "started":
                ui.started(step.optString("version", ""));
                break;
            case "waiting":
                ui.waiting(step.optString("reason", ""));
                break;
            case "back":
                ui.back(step.optString("version", ""));
                break;
            case "timed_out":
                ui.timedOut(step.optString("version", ""));
                break;
            case "nothing_to_install":
                ui.nothingToInstall();
                break;
            case "update_failed":
                ui.updateFailed(step.isNull("error") ? null : step.optString("error", null));
                break;
            case "came_back_on":
                ui.cameBackOn(
                        step.optString("current_version", ""),
                        step.isNull("error") ? null : step.optString("error", null));
                break;
            case "refused":
                ui.refused(step.optString("message", ""));
                break;
            default:
                ui.failed(SleepFlow.exceptionFromJson(step.optJSONObject("error")));
                break;
        }
    }

    /** Stops the wait: nothing more is asked or reported. */
    void cancel() {
        cancelled.set(true);
    }
}
