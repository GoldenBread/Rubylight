package com.limelight.computers;

import android.text.BidiFormatter;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePairing;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.json.JSONArray;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.Executor;
import java.util.function.LongSupplier;
import java.util.function.Predicate;

/**
 * The devices waiting to pair with the PCs this device may approve for, backed by
 * {@link NightjarPolicyJni} {@code PairingWatchTracker} and BiDi control sanitization.
 */
public final class PairingWatch {
    public interface Reader {
        List<NativePairing.Request> read(ComputerDetails host) throws NativeApiException;
    }

    public interface Listener {
        void onWaiting(List<Waiting> waiting);
    }

    /** A device waiting on one PC. */
    public static final class Waiting {
        public final ComputerDetails host;
        public final NativePairing.Request request;

        public Waiting(ComputerDetails host, NativePairing.Request request) {
            this.host = host;
            this.request = request;
        }

        /** The name, safe beside other text after Rust BiDi-control stripping. */
        public String shownName() {
            return BidiFormatter.getInstance().unicodeWrap(NightjarPolicyJni.stripBidiControls(request.name));
        }
    }

    private final Reader reader;
    private final Predicate<ComputerDetails> asks;
    private final Executor worker;
    private final Executor main;
    private final LongSupplier clock;
    private final Listener listener;
    private final long tracker;

    public PairingWatch(Reader reader, Predicate<ComputerDetails> asks, Executor worker, Executor main,
                        Listener listener) {
        this(reader, asks, worker, main, android.os.SystemClock::elapsedRealtime, listener);
    }

    @VisibleForTesting
    PairingWatch(Reader reader, Predicate<ComputerDetails> asks, Executor worker, Executor main, LongSupplier clock,
                 Listener listener) {
        this.reader = reader;
        this.asks = asks;
        this.worker = worker;
        this.main = main;
        this.clock = clock;
        this.listener = listener;
        this.tracker = NightjarPolicyJni.pairingWatchCreate();
    }

    /** The screen's hosts were polled. A PC that doesn't answer drops out, logged. */
    public void poll(List<ComputerDetails> hosts) {
        final List<ComputerDetails> asked = new ArrayList<>();
        for (ComputerDetails host : hosts) {
            if (asks.test(host)) {
                asked.add(host);
            }
        }
        long command = NightjarPolicyJni.pairingWatchBeginPoll(tracker, clock.getAsLong(), !asked.isEmpty());
        if (command == 0) {
            return;
        }
        if (command < 0) {
            listener.onWaiting(Collections.emptyList());
            return;
        }
        final long generation = command;
        worker.execute(() -> {
            final List<Waiting> waiting = new ArrayList<>();
            JSONArray ids = new JSONArray();
            for (ComputerDetails host : asked) {
                try {
                    for (NativePairing.Request request : reader.read(host)) {
                        waiting.add(new Waiting(host, request));
                        ids.put(request.id);
                    }
                } catch (NativeApiException e) {
                    LimeLog.warning("Devices waiting to pair on " + host.uuid + ": " + e.getMessage());
                }
            }
            main.execute(() -> {
                if (NightjarPolicyJni.pairingWatchFinishPoll(tracker, generation, ids.toString()) == 2) {
                    listener.onWaiting(waiting);
                }
            });
        });
    }

    /** The next {@link #poll} reads at once: the list just changed. */
    public void refreshSoon() {
        NightjarPolicyJni.pairingWatchRefreshSoon(tracker);
    }

    /** The screen left: an answer still on its way is dropped, and the next poll reads at once. */
    public void pause() {
        NightjarPolicyJni.pairingWatchPause(tracker);
    }
}
