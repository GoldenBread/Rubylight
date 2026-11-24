package com.limelight.computers;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nvstream.jni.NightjarHostsJni;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.concurrent.Executor;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.LongSupplier;

/**
 * Restart PC or Shut down PC, delegating offer, staleness refresh, confirmation spend, and late
 * refusal polling to {@link NightjarHostsJni#hostFlowPcPowerOffer} / {@link NightjarHostsJni#hostFlowPcPowerSpend}.
 */
final class PcPowerFlow {
    static final long REFUSAL_POLL_MS = NightjarPolicyJni.hostPcPowerRefusalPollMs();
    static final int REFUSAL_READS = NightjarPolicyJni.hostPcPowerRefusalReads();

    interface Ui {
        /**
         * The host offered a token: ask the person and run {@code go} on yes; it acts only the first time.
         * {@code wakeFromOff} is false when the PC may not wake again once it is off.
         */
        void confirm(boolean wakeFromOff, Runnable go);

        /** The host took the request (202). */
        void accepted();

        /** The host can't do it now, or not for this device: its reason as sent, "" when it gave none. */
        void unavailable(String reason);

        /** The host refused the request: its words as sent. */
        void refused(String message);

        /** The host could not be asked or its answer was unreadable. */
        void failed(NativeApiException error);

        /**
         * The PC took the request but did not go ahead after all: the host's words. {@code unconfirmed}: no
         * refusal, Windows did not confirm it in time and the PC may still go down.
         */
        void refusedLater(String refusal, boolean unconfirmed);
    }

    private final NativePcPower.Action action;
    private final SleepFlow.Client client;
    private final Ui ui;
    private final Executor worker;
    private final Executor main;
    private final LongSupplier clockMs;
    private final LongSupplier wallClockMs;
    private final HostUpdateFlow.Sleeper sleeper;

    PcPowerFlow(NativePcPower.Action action, SleepFlow.Client client, Ui ui, Executor worker, Executor main,
                LongSupplier clockMs, HostUpdateFlow.Sleeper sleeper) {
        this(action, client, ui, worker, main, clockMs, System::currentTimeMillis, sleeper);
    }

    PcPowerFlow(NativePcPower.Action action, SleepFlow.Client client, Ui ui, Executor worker, Executor main,
                LongSupplier clockMs, LongSupplier wallClockMs, HostUpdateFlow.Sleeper sleeper) {
        this.action = action;
        this.client = client;
        this.ui = ui;
        this.worker = worker;
        this.main = main;
        this.clockMs = clockMs;
        this.wallClockMs = wallClockMs;
        this.sleeper = sleeper;
    }

    void start() {
        final boolean isRestart = action == NativePcPower.Action.RESTART;
        worker.execute(() -> {
            try {
                NativeApiClient api = client.open();
                long monoAsked = clockMs.getAsLong();
                long wallAsked = wallClockMs.getAsLong();
                JSONObject res = new JSONObject(NightjarHostsJni.hostFlowPcPowerOffer(api::requestRaw, isRestart));
                main.execute(() -> deliverOffer(api, isRestart, res, monoAsked, wallAsked));
            } catch (NativeApiException e) {
                main.execute(() -> fail(e));
            } catch (JSONException e) {
                main.execute(() -> ui.failed(new NativeApiException(NativeApiException.Kind.MALFORMED, e.getMessage())));
            }
        });
    }

    private void deliverOffer(NativeApiClient api, boolean isRestart, JSONObject res, long monoAsked, long wallAsked) {
        if ("token".equals(res.optString("outcome"))) {
            final String token = res.optString("token", "");
            final boolean wakeFromOff = res.optBoolean("wake_from_off", false);
            AtomicBoolean confirmed = new AtomicBoolean();
            ui.confirm(wakeFromOff, () -> {
                if (confirmed.compareAndSet(false, true)) {
                    worker.execute(() -> spend(api, isRestart, token, monoAsked, wallAsked));
                }
            });
        } else {
            deliverStep(res);
        }
    }

    private void spend(NativeApiClient api, boolean isRestart, String token, long monoAsked, long wallAsked) {
        NightjarHostsJni.hostFlowPcPowerSpend(
                api::requestRaw,
                isRestart,
                token,
                monoAsked,
                wallAsked,
                clockMs.getAsLong(),
                wallClockMs.getAsLong(),
                ms -> {
                    try {
                        sleeper.sleep(ms);
                        return true;
                    } catch (InterruptedException e) {
                        Thread.currentThread().interrupt();
                        return false;
                    }
                },
                stepJson -> {
                    try {
                        JSONObject step = new JSONObject(stepJson);
                        main.execute(() -> deliverStep(step));
                    } catch (JSONException ignored) {
                    }
                });
    }

    private void deliverStep(JSONObject step) {
        switch (step.optString("outcome")) {
            case "accepted":
                ui.accepted();
                break;
            case "unavailable":
                ui.unavailable(step.optString("reason", ""));
                break;
            case "refused":
                ui.refused(step.optString("message", ""));
                break;
            case "refused_later":
                ui.refusedLater(step.optString("refusal", ""), step.optBoolean("unconfirmed", false));
                break;
            default:
                ui.failed(SleepFlow.exceptionFromJson(step.optJSONObject("error")));
                break;
        }
    }

    private void fail(NativeApiException e) {
        if (isRefusal(e)) {
            ui.refused(e.getMessage());
        } else {
            ui.failed(e);
        }
    }

    /** The host answered with an API error that carries its own words: shown as they are. */
    static boolean isRefusal(NativeApiException e) {
        return e.getKind() == NativeApiException.Kind.API && e.getMessage() != null && !e.getMessage().isEmpty();
    }
}
