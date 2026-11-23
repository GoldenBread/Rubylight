package com.limelight.nvstream.wol;

import com.limelight.LimeLog;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nativeapi.NativeWake;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarDiscoveryJni;

import java.util.concurrent.Executor;
import java.util.concurrent.RejectedExecutionException;
import java.util.function.LongSupplier;

/**
 * Keeps the wake targets of the PCs the computer manager polls, delegating reservation and
 * 30-second cooldown state to Rust {@code WakeTargetsPollState} via {@link NightjarDiscoveryJni}.
 */
public final class WakeTargetsPoll implements AutoCloseable {
    static final long INTERVAL_MS = 30_000;

    public interface Clients {
        /** A client for the PC's native API, or null when it has no address or pinned certificate yet. */
        NativeApiClient open(ComputerDetails computer);
    }

    private final long handle = NightjarDiscoveryJni.wakePollCreate();
    private final HandleReaper.Token handleToken =
            HandleReaper.register(this, handle, NightjarDiscoveryJni::wakePollDestroy);
    private final WakeTargets targets;
    private final Clients clients;
    private final Executor requests;
    private final LongSupplier clock;
    private final Object storeLock = new Object();

    /** {@code requests} runs the HTTPS requests, so a poll never waits on them. */
    public WakeTargetsPoll(WakeTargets targets, Clients clients, Executor requests, LongSupplier clock) {
        this.targets = targets;
        this.clients = clients;
        this.requests = requests;
        this.clock = clock;
    }

    @Override
    public void close() {
        handleToken.close();
    }

    /** After each poll of the PC, on the polling thread; an ask that is due goes to the request executor. */
    public void polled(ComputerDetails computer) {
        String uuid = computer.uuid;
        if (uuid == null) {
            return;
        }
        long token = NightjarDiscoveryJni.wakePollReserve(
                handle,
                uuid,
                computer.state == ComputerDetails.State.ONLINE,
                computer.caps.has(HostCapabilities.Feature.NATIVE_API),
                clock.getAsLong());
        if (token == 0) {
            return;
        }
        ComputerDetails asked = new ComputerDetails(computer);
        try {
            requests.execute(() -> ask(uuid, asked, token));
        } catch (RejectedExecutionException e) {
            NightjarDiscoveryJni.wakePollDrop(handle, uuid, token);
        }
    }

    /** A removed PC is asked nothing more, and an ask still in flight for it keeps nothing. */
    public void forget(String uuid) {
        if (uuid != null) {
            NightjarDiscoveryJni.wakePollForget(handle, uuid);
        }
    }

    /** The service is going away: an ask still queued or in flight stores nothing. */
    public void shutdown() {
        NightjarDiscoveryJni.wakePollShutdown(handle);
    }

    boolean hasPendingAsks() {
        return NightjarDiscoveryJni.wakePollPendingCount(handle) > 0;
    }

    private void ask(String uuid, ComputerDetails computer, long token) {
        if (!NightjarDiscoveryJni.wakePollIsActive(handle, uuid, token)) {
            return;
        }
        NativeApiClient client = clients.open(computer);
        if (client == null) {
            NightjarDiscoveryJni.wakePollDrop(handle, uuid, token);
            return;
        }
        try {
            NativeCapabilities capabilities = client.capabilities();
            NativeWake wake = capabilities.has(NativeCapabilities.WAKE) ? client.wake() : null;
            synchronized (storeLock) {
                if (!NightjarDiscoveryJni.wakePollIsActive(handle, uuid, token)) {
                    return;
                }
                if (wake == null) {
                    targets.forget(uuid);
                } else {
                    targets.store(uuid, wake);
                }
            }
            if (capabilities.has(NativePcPower.FEATURE_UPDATE) && capabilities.hostControl) {
                readUpdate(uuid, client);
            }
        } catch (NativeApiException e) {
            LimeLog.info("Wake targets of " + uuid + " not read: " + e.getMessage());
        }
    }

    private static void readUpdate(String uuid, NativeApiClient client) {
        try {
            client.hostUpdate();
        } catch (NativeApiException e) {
            LimeLog.info("Host update of " + uuid + " not read: " + e.getMessage());
        }
    }
}
