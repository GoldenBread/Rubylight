package com.limelight.computers;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.concurrent.Executor;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Update the PC's host via {@link NightjarPolicyJni} {@code UpdateTracker}: POST /update/install, then poll GET /update
 * every {@link #POLL_MS} up to {@link #WAIT_MS} active wait ({@link #MAX_WAIT_MS} overall including waits).
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
            String target;
            try {
                api = client.open();
                target = api.installHostUpdate().offered();
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
            if (target == null) {
                // No 15-minute wait for a version that isn't coming.
                main.execute(ui::nothingToInstall);
                return;
            }
            main.execute(() -> ui.started(target));
            follow(api, target);
        });
    }

    private void follow(NativeApiClient api, String target) {
        int polls = (int) (WAIT_MS / POLL_MS);
        int most = (int) (MAX_WAIT_MS / POLL_MS);
        long tracker = NightjarPolicyJni.updateTrackerCreate(target, polls, most);
        try {
            while (!cancelled.get()) {
                try {
                    sleeper.sleep(POLL_MS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return;
                }
                if (cancelled.get()) {
                    return;
                }
                String runningVersion = null;
                String updateJson = null;
                try {
                    NativePcPower.Update update = api.hostUpdate(target);
                    if (update.arrived) {
                        runningVersion = target;
                    } else {
                        updateJson = encodeUpdate(update, target);
                    }
                } catch (NativeApiException e) {
                    // Down while installing: pass updateJson = null so UpdateTracker marks gone = true.
                }
                String stepRaw = NightjarPolicyJni.updateTrackerStep(tracker, runningVersion, updateJson);
                JSONObject step = new JSONObject(stepRaw);
                String action = step.optString("action", "continue");
                switch (action) {
                    case "continue":
                        break;
                    case "waiting": {
                        String reason = step.optString("reason", "");
                        main.execute(() -> ui.waiting(reason));
                        break;
                    }
                    case "back": {
                        String version = step.optString("version", target);
                        main.execute(() -> ui.back(version));
                        return;
                    }
                    case "update_failed": {
                        String err = step.isNull("error") ? null : step.optString("error", null);
                        main.execute(() -> ui.updateFailed(err));
                        return;
                    }
                    case "came_back_on": {
                        String current = step.optString("current_version", "");
                        String err = step.isNull("error") ? null : step.optString("error", null);
                        main.execute(() -> ui.cameBackOn(current, err));
                        return;
                    }
                    case "timed_out": {
                        String version = step.optString("target", target);
                        main.execute(() -> ui.timedOut(version));
                        return;
                    }
                    default:
                        break;
                }
            }
        } catch (JSONException e) {
            if (!cancelled.get()) {
                main.execute(() -> ui.timedOut(target));
            }
        } finally {
            NightjarPolicyJni.updateTrackerDestroy(tracker);
        }
    }

    private static String encodeUpdate(NativePcPower.Update update, String target) throws JSONException {
        JSONObject obj = new JSONObject();
        obj.put("current_version", update.currentVersion == null ? "" : update.currentVersion);
        obj.put("latest_version", target);
        if (update.failed) {
            obj.put("phase", "failed");
            obj.put("install_now", false);
            obj.put("error", update.failure);
        } else if (update.waitingFor != null) {
            obj.put("phase", "waiting");
            obj.put("install_now", true);
            obj.put("waiting_for", update.waitingFor);
        } else if (update.gaveUp) {
            obj.put("phase", "idle");
            obj.put("install_now", false);
            obj.put("error", update.error);
        } else {
            obj.put("phase", "installing");
            obj.put("install_now", true);
        }
        return obj.toString();
    }

    /** Stops the wait: nothing more is asked or reported. */
    void cancel() {
        cancelled.set(true);
    }
}
