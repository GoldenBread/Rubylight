package com.limelight.computers;

import java.io.IOException;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.UnknownHostException;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.LinkedList;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.locks.Lock;
import java.util.concurrent.locks.ReentrantLock;

import com.limelight.LimeLog;
import com.limelight.binding.PlatformBinding;
import com.limelight.discovery.DiscoveryService;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.InviteLink;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.jni.NightjarHostsJni;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.nvstream.mdns.MdnsComputer;
import com.limelight.nvstream.mdns.MdnsDiscoveryListener;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.nvstream.wol.WakeTargetsPoll;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.FrontendSync;
import com.limelight.utils.RedactedLog;
import com.limelight.utils.ServerHelper;

import android.app.Service;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.net.ConnectivityManager;
import android.net.Network;
import android.os.Binder;
import android.os.IBinder;
import android.os.SystemClock;
import android.preference.PreferenceManager;

import androidx.annotation.VisibleForTesting;

/**
 * Android foreground/bound service hosting tracked PCs, mDNS binding, network callbacks,
 * VPN-bound STUN lookup, and Android cleanup cascades while delegating pooling, probe plans,
 * parallel direct polling, and app-list polling to {@link NightjarHostsJni} and {@link NightjarPolicyJni}.
 */
public class ComputerManagerService extends Service {
    private static final int MDNS_QUERY_PERIOD_MS = 1000;
    private static final long POLL_DATA_TTL_MS = NightjarPolicyJni.hostPollDataTtlMs();
    private static final long SERVERINFO_POLLING_PERIOD_MS = 1500;
    private static final int OFFLINE_POLL_TRIES = NightjarPolicyJni.hostOfflinePollTries();
    private static final int INITIAL_POLL_TRIES = NightjarPolicyJni.hostInitialPollTries();

    private final ComputerManagerBinder binder = new ComputerManagerBinder();

    private ComputerDatabaseManager dbManager;
    private final AtomicInteger dbRefCount = new AtomicInteger(0);

    private IdentityManager idManager;
    private long hostManagerHandle;
    private WakeTargetsPoll wakeTargets;
    private ExecutorService wakeRequests;
    private ExecutorService internetPlayCalls;
    private ExecutorService networkChanges;
    private final SharedPreferences.OnSharedPreferenceChangeListener internetPlaySetting =
            (preferences, key) -> {
                if (PreferenceConfiguration.INTERNET_PLAY_PREF_STRING.equals(key)) {
                    applyInternetPlay(preferences);
                }
            };
    private final LinkedList<TrackedHost> pollingTuples = new LinkedList<>();
    private final Object removalLock = new Object();
    private volatile ComputerManagerListener listener;
    private final AtomicInteger activePolls = new AtomicInteger(0);
    private final HashMap<String, Long> internetReads = new HashMap<>();
    private volatile boolean pollingActive;
    private volatile boolean destroyed;
    private boolean discoveryServiceBound;
    private final Lock defaultNetworkLock = new ReentrantLock();

    private ConnectivityManager.NetworkCallback networkCallback;
    private volatile DiscoveryService.DiscoveryBinder discoveryBinder;
    private final ServiceConnection discoveryServiceConnection = new ServiceConnection() {
        public void onServiceConnected(ComponentName className, IBinder binder) {
            synchronized (discoveryServiceConnection) {
                if (destroyed) {
                    return;
                }
                DiscoveryService.DiscoveryBinder privateBinder = (DiscoveryService.DiscoveryBinder) binder;
                privateBinder.setListener(createDiscoveryListener());
                discoveryBinder = privateBinder;
                if (pollingActive) {
                    privateBinder.startDiscovery(MDNS_QUERY_PERIOD_MS);
                }
                discoveryServiceConnection.notifyAll();
            }
        }

        public void onServiceDisconnected(ComponentName className) {
            discoveryBinder = null;
        }
    };

    static final class TrackedHost {
        Thread thread;
        final ComputerDetails computer;
        final Object networkLock = new Object();
        long lastSuccessfulPollMs;
        volatile boolean removed;

        TrackedHost(ComputerDetails computer, Thread thread) {
            this.computer = computer;
            this.thread = thread;
        }
    }

    static boolean punchesNow(ComputerDetails details) {
        return NightjarPolicyJni.pollPunchesNow(
                details.localAddress != null,
                details.state == ComputerDetails.State.OFFLINE,
                details.activeAddress != null && NightjarPolicyJni.isTunnelAddress(details.activeAddress.address));
    }

    interface HostResolver {
        InetAddress resolve(String host) throws UnknownHostException;
    }

    static boolean isSiteLocalHost(String host) {
        return NightjarHostsJni.hostManagerIsSiteLocalHost(host);
    }

    static boolean isSiteLocalHost(String host, HostResolver resolver) {
        if (resolver == null) {
            return isSiteLocalHost(host);
        }
        try {
            return resolver.resolve(host).isSiteLocalAddress();
        } catch (UnknownHostException e) {
            return false;
        }
    }

    @VisibleForTesting
    boolean pollComputer(ComputerDetails details) throws InterruptedException {
        RedactedLog.addComputerSecrets(details);
        details.anotherCertificate = false;
        boolean ok = HostServiceHelpers.pollComputerDirect(hostManagerHandle, details);
        if (!ok && details.uuid != null && punchesNow(details)) {
            ok = HostServiceHelpers.pollOverTheInternet(this, hostManagerHandle, details);
        }
        if (ok) {
            details.unreachable = null;
            RedactedLog.addComputerSecrets(details);
            details.internetPath = InternetPlay.path(details, InternetPlay.reply(this, details.uuid));
            return true;
        }
        details.internetPath = null;
        return false;
    }

    private boolean runPoll(ComputerDetails details, TrackedHost tuple, int offlineCount) throws InterruptedException {
        if (!getLocalDatabaseReference()) {
            return false;
        }
        try {
            final boolean newPc = tuple == null;
            final int triesBeforeOffline = details.state == ComputerDetails.State.UNKNOWN
                    ? INITIAL_POLL_TRIES : OFFLINE_POLL_TRIES;

            activePolls.incrementAndGet();
            final X509Certificate pinned = details.serverCert;
            try {
                if (!pollComputer(details)) {
                    if (!newPc && offlineCount < triesBeforeOffline) {
                        return false;
                    }
                    details.state = ComputerDetails.State.OFFLINE;
                }
            } finally {
                activePolls.decrementAndGet();
            }

            ComputerDetails existingComputer = null;
            Object[] savedColumns = null;
            if (details.state == ComputerDetails.State.ONLINE) {
                existingComputer = dbManager.getComputerByUUID(details.uuid);
                if (existingComputer != null) {
                    savedColumns = ComputerDatabaseManager.columns(existingComputer);
                }
                if (!newPc && existingComputer == null) {
                    return false;
                }
                if (existingComputer == null && details.remoteAddress == null && details.activeAddress != null
                        && isSiteLocalHost(details.activeAddress.address)) {
                    HostServiceHelpers.populateExternalAddress(this, details);
                }
            }

            synchronized (removalLock) {
                if (!newPc && tuple.removed) {
                    return false;
                }
                if (details.state == ComputerDetails.State.ONLINE) {
                    if (NightjarPolicyJni.pollShouldClearCertOnUnpair(
                            details.pairState == PairingManager.PairState.PAIRED,
                            pinned == null,
                            Objects.equals(details.serverCert, pinned))) {
                        details.serverCert = null;
                        if (existingComputer != null && Objects.equals(existingComputer.serverCert, pinned)) {
                            existingComputer.serverCert = null;
                        }
                        if (details.serverCert == null) {
                            InternetPlay.forget(this, details.uuid);
                        }
                    }
                    HostCapabilityCache.save(this, details.uuid, details.caps);
                    if (existingComputer != null) {
                        existingComputer.update(details);
                        if (!Arrays.deepEquals(savedColumns, ComputerDatabaseManager.columns(existingComputer))) {
                            dbManager.updateComputer(existingComputer);
                        }
                    } else {
                        dbManager.updateComputer(details);
                    }
                }

                ComputerManagerListener currentListener = listener;
                if ((!newPc || details.state == ComputerDetails.State.ONLINE) && currentListener != null) {
                    if (newPc) {
                        details.alias = HostNames.get(this, details.uuid);
                        details.pinnedAddress = PinnedAddresses.get(this, details.uuid);
                        if (details.pinnedAddress != null) details.activeAddress = details.pinnedAddress;
                    }
                    currentListener.notifyComputerUpdated(details);
                }

                wakeTargets.polled(details);
                readInternetPlay(details);
            }
            return true;
        } finally {
            releaseLocalDatabaseReference();
        }
    }

    private Thread createPollingThread(final TrackedHost tuple) {
        Thread t = new Thread() {
            @Override
            public void run() {
                int offlineCount = 0;
                while (!isInterrupted() && pollingActive && tuple.thread == this) {
                    try {
                        synchronized (tuple.networkLock) {
                            if (!runPoll(tuple.computer, tuple, offlineCount)) {
                                offlineCount++;
                            } else {
                                tuple.lastSuccessfulPollMs = SystemClock.elapsedRealtime();
                                offlineCount = 0;
                            }
                        }
                        Thread.sleep(SERVERINFO_POLLING_PERIOD_MS);
                    } catch (InterruptedException e) {
                        break;
                    }
                }
            }
        };
        t.setName("Polling thread for " + tuple.computer.name);
        return t;
    }

    public class ComputerManagerBinder extends Binder {
        public boolean isPersistent() { return true; }

        public void startPolling(ComputerManagerListener listener) {
            if (destroyed) return;
            pollingActive = true;
            ComputerManagerService.this.listener = listener;
            DiscoveryService.DiscoveryBinder discovery = discoveryBinder;
            if (discovery != null) discovery.startDiscovery(MDNS_QUERY_PERIOD_MS);

            synchronized (pollingTuples) {
                for (TrackedHost tuple : pollingTuples) {
                    if (SystemClock.elapsedRealtime() - tuple.lastSuccessfulPollMs > POLL_DATA_TTL_MS) {
                        tuple.computer.state = ComputerDetails.State.UNKNOWN;
                    }
                    listener.notifyComputerUpdated(tuple.computer);
                    if (tuple.thread == null) {
                        tuple.thread = createPollingThread(tuple);
                        tuple.thread.start();
                    }
                }
            }
        }

        public boolean waitForReady() {
            synchronized (discoveryServiceConnection) {
                try {
                    while (discoveryBinder == null && discoveryServiceBound && !destroyed) {
                        discoveryServiceConnection.wait(1000);
                    }
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return false;
                }
                return discoveryBinder != null && !destroyed;
            }
        }

        public void waitForPollingStopped() {
            while (activePolls.get() != 0) {
                try {
                    Thread.sleep(250);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return;
                }
            }
        }

        public boolean addComputerBlocking(ComputerDetails fakeDetails) throws InterruptedException {
            return ComputerManagerService.this.addComputerBlocking(fakeDetails);
        }

        public LinkAdd addComputerForLink(ComputerDetails fakeDetails, PairingLink link) throws InterruptedException {
            return ComputerManagerService.this.addComputerForLink(fakeDetails, link);
        }

        public void removeComputer(ComputerDetails computer) {
            ComputerManagerService.this.removeComputer(computer);
        }

        public boolean addInvitedComputer(ComputerDetails details, String reply) {
            return ComputerManagerService.this.addInvitedComputer(details, reply);
        }

        public boolean inviteClaimsKnownPc(InviteLink invite) {
            return ComputerManagerService.this.inviteClaimsKnownPc(invite);
        }

        public void renameComputer(String uuid, String alias) {
            ComputerManagerService.this.renameComputer(uuid, alias);
        }

        public void pinAddress(String uuid, ComputerDetails.AddressTuple address) {
            ComputerManagerService.this.pinAddress(uuid, address);
        }

        public void forgetCertificate(String uuid) {
            ComputerManagerService.this.forgetCertificate(uuid);
        }

        public void stopPolling() {
            ComputerManagerService.this.onUnbind(null);
        }

        public ApplistPoller createAppListPoller(ComputerDetails computer) {
            return new ApplistPoller(computer);
        }

        public String getUniqueId() {
            return idManager.getUniqueId();
        }

        public ComputerDetails getComputer(String uuid) {
            synchronized (pollingTuples) {
                for (TrackedHost tuple : pollingTuples) {
                    if (uuid.equalsIgnoreCase(tuple.computer.uuid)) {
                        return tuple.computer;
                    }
                }
            }
            return null;
        }

        public void invalidateStateForComputer(String uuid) {
            List<TrackedHost> matching = new ArrayList<>();
            synchronized (pollingTuples) {
                for (TrackedHost tuple : pollingTuples) {
                    if (uuid.equals(tuple.computer.uuid)) {
                        matching.add(tuple);
                    }
                }
            }
            for (TrackedHost tuple : matching) {
                synchronized (tuple.networkLock) {
                    tuple.computer.state = ComputerDetails.State.UNKNOWN;
                }
            }
        }
    }

    @Override
    public boolean onUnbind(Intent intent) {
        DiscoveryService.DiscoveryBinder discovery = discoveryBinder;
        if (discovery != null) {
            discovery.stopDiscovery();
        }
        pollingActive = false;
        synchronized (pollingTuples) {
            for (TrackedHost tuple : pollingTuples) {
                if (tuple.thread != null) {
                    tuple.thread.interrupt();
                    tuple.thread = null;
                }
            }
        }
        NightjarHostsJni.hostManagerClearPool(hostManagerHandle);
        listener = null;
        return false;
    }

    private MdnsDiscoveryListener createDiscoveryListener() {
        return new MdnsDiscoveryListener() {
            @Override
            public boolean notifyComputerAdded(MdnsComputer computer) {
                ComputerDetails details = HostServiceHelpers.detailsFromMdns(computer, ComputerManagerService.this);
                try {
                    return addComputerBlocking(details);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return false;
                }
            }

            @Override
            public void notifyDiscoveryFailure(Exception e) {
                LimeLog.severe("mDNS discovery failed: " + e);
            }
        };
    }

    private void addTuple(ComputerDetails details) {
        details.alias = HostNames.get(this, details.uuid);
        details.pinnedAddress = PinnedAddresses.get(this, details.uuid);
        if (details.pinnedAddress != null) details.activeAddress = details.pinnedAddress;
        synchronized (pollingTuples) {
            for (TrackedHost tuple : pollingTuples) {
                if (tuple.computer.uuid.equals(details.uuid)) {
                    tuple.computer.update(details);
                    if (!Objects.equals(tuple.computer.alias, details.alias)) {
                        tuple.computer.alias = details.alias;
                    }
                    if (pollingActive && tuple.thread == null) {
                        tuple.thread = createPollingThread(tuple);
                        tuple.thread.start();
                    }
                    return;
                }
            }
            TrackedHost tuple = new TrackedHost(details, null);
            if (pollingActive) {
                tuple.thread = createPollingThread(tuple);
            }
            pollingTuples.add(tuple);
            if (tuple.thread != null) {
                tuple.thread.start();
            }
        }
    }

    public boolean addComputerBlocking(ComputerDetails fakeDetails) throws InterruptedException {
        return finishAdding(fakeDetails, pollComputer(fakeDetails));
    }

    public boolean addInvitedComputer(ComputerDetails details, String reply) {
        if (pinsAnotherCertificate(details.uuid, pinned -> Objects.equals(pinned, details.serverCert))) {
            return false;
        }
        if (getLocalDatabaseReference()) {
            try {
                synchronized (removalLock) {
                    if (!HostServiceHelpers.saveInvitedComputerInDb(this, dbManager, details, reply)) {
                        return false;
                    }
                }
            } finally {
                releaseLocalDatabaseReference();
            }
        }
        addTuple(details);
        return true;
    }

    public boolean inviteClaimsKnownPc(InviteLink invite) {
        return pinsAnotherCertificate(invite.uniqueId, invite::matches);
    }

    private boolean pinsAnotherCertificate(String uuid, java.util.function.Predicate<X509Certificate> matches) {
        synchronized (pollingTuples) {
            for (TrackedHost tuple : pollingTuples) {
                if (uuid.equals(tuple.computer.uuid)) {
                    return tuple.computer.serverCert != null && !matches.test(tuple.computer.serverCert);
                }
            }
        }
        return false;
    }

    public enum LinkAdd { ADDED, UNREACHABLE, CERTIFICATE_MISMATCH }

    public LinkAdd addComputerForLink(ComputerDetails fakeDetails, PairingLink link) throws InterruptedException {
        boolean polled = pollComputer(fakeDetails);
        if (polled && pinsAnotherCertificate(fakeDetails.uuid, link::matches)) {
            return LinkAdd.CERTIFICATE_MISMATCH;
        }
        return finishAdding(fakeDetails, polled) ? LinkAdd.ADDED : LinkAdd.UNREACHABLE;
    }

    private boolean finishAdding(ComputerDetails fakeDetails, boolean polled) throws InterruptedException {
        if (Thread.currentThread().isInterrupted()) throw new InterruptedException();
        if (polled) {
            synchronized (pollingTuples) {
                for (TrackedHost tuple : pollingTuples) {
                    if (tuple.computer.uuid.equals(fakeDetails.uuid)) {
                        fakeDetails.serverCert = tuple.computer.serverCert;
                        break;
                    }
                }
            }
            runPoll(fakeDetails, null, 0);
        }
        if (fakeDetails.state == ComputerDetails.State.ONLINE) {
            addTuple(fakeDetails);
            return true;
        }
        return false;
    }

    public void pinAddress(String uuid, ComputerDetails.AddressTuple address) {
        PinnedAddresses.set(this, uuid, address);
        synchronized (pollingTuples) {
            for (TrackedHost tuple : pollingTuples) {
                if (uuid.equals(tuple.computer.uuid)) {
                    tuple.computer.pinnedAddress = address;
                    if (address != null) tuple.computer.activeAddress = address;
                }
            }
        }
    }

    public void forgetCertificate(String uuid) {
        synchronized (pollingTuples) {
            for (TrackedHost tuple : pollingTuples) {
                if (uuid.equals(tuple.computer.uuid)) tuple.computer.serverCert = null;
            }
        }
        if (!getLocalDatabaseReference()) return;
        try {
            synchronized (removalLock) {
                HostServiceHelpers.forgetCertInDb(dbManager, uuid);
            }
        } finally {
            releaseLocalDatabaseReference();
        }
    }

    public void renameComputer(String uuid, String alias) {
        HostNames.set(this, uuid, alias);
        synchronized (pollingTuples) {
            for (TrackedHost tuple : pollingTuples) {
                if (uuid.equals(tuple.computer.uuid)) tuple.computer.alias = alias;
            }
        }
    }

    public void removeComputer(ComputerDetails computer) {
        if (!getLocalDatabaseReference()) return;
        try {
            synchronized (pollingTuples) {
                for (TrackedHost tuple : pollingTuples) {
                    if (tuple.computer.uuid.equals(computer.uuid)) {
                        tuple.removed = true;
                        if (tuple.thread != null) {
                            tuple.thread.interrupt();
                            tuple.thread = null;
                        }
                        pollingTuples.remove(tuple);
                        break;
                    }
                }
            }
            synchronized (removalLock) {
                HostServiceHelpers.cleanupRemovedComputer(this, dbManager, wakeTargets, computer);
            }
            NightjarHostsJni.hostManagerRemoveComputer(hostManagerHandle, computer.uuid);
            HostServiceHelpers.cleanupRemovedComputerPrefs(this, computer.uuid);

            DiscoveryService.DiscoveryBinder discovery = discoveryBinder;
            if (discovery != null) discovery.forgetAnsweredComputers();
        } finally {
            releaseLocalDatabaseReference();
        }
    }

    private boolean getLocalDatabaseReference() {
        int count;
        do {
            count = dbRefCount.get();
            if (count == 0) return false;
        } while (!dbRefCount.compareAndSet(count, count + 1));
        return true;
    }

    private void releaseLocalDatabaseReference() {
        if (dbRefCount.decrementAndGet() == 0) dbManager.close();
    }

    private void applyInternetPlay(SharedPreferences preferences) {
        final boolean on = preferences.getBoolean(PreferenceConfiguration.INTERNET_PLAY_PREF_STRING, true);
        try {
            internetPlayCalls.execute(() -> InternetPlay.setEnabled(on));
        } catch (RejectedExecutionException ignored) {
        }
    }

    @Override
    public IBinder onBind(Intent intent) { return binder; }

    NativeApiClient nativeClient(ComputerDetails computer) {
        HostEndpoint endpoint = HostEndpoint.of(computer, idManager.getUniqueId(),
                PlatformBinding.getCryptoProvider(this));
        return endpoint == null ? null : new NativeApiClient(new JniNativeApiTransport(), endpoint);
    }

    @Override
    public void onCreate() {
        super.onCreate();
        discoveryServiceBound = bindService(new Intent(this, DiscoveryService.class),
                discoveryServiceConnection, Service.BIND_AUTO_CREATE);
        idManager = new IdentityManager(this);
        hostManagerHandle = NightjarHostsJni.hostManagerOpen(
                getFilesDir().getAbsolutePath(), idManager.getUniqueId(), null, null);
        wakeRequests = Executors.newSingleThreadExecutor(task -> new Thread(task, "Wake targets"));
        wakeTargets = new WakeTargetsPoll(new WakeTargets(this), this::nativeClient, wakeRequests,
                SystemClock::elapsedRealtime);
        internetPlayCalls = Executors.newSingleThreadExecutor(task -> new Thread(task, "Internet play setting"));
        networkChanges = Executors.newSingleThreadExecutor(task -> new Thread(task, "Network changes"));
        SharedPreferences preferences = PreferenceManager.getDefaultSharedPreferences(this);
        preferences.registerOnSharedPreferenceChangeListener(internetPlaySetting);
        applyInternetPlay(preferences);

        dbManager = new ComputerDatabaseManager(this);
        dbRefCount.set(1);
        if (!getLocalDatabaseReference()) return;
        for (ComputerDetails computer : dbManager.getAllComputers()) {
            computer.caps = HostCapabilityCache.load(this, computer.uuid);
            addTuple(computer);
        }
        releaseLocalDatabaseReference();

        networkCallback = new ConnectivityManager.NetworkCallback() {
            @Override public void onAvailable(Network network) {
                applyNetworkChange(true, SystemClock.elapsedRealtime());
            }

            @Override public void onLost(Network network) {
                applyNetworkChange(false, SystemClock.elapsedRealtime());
            }
        };
        ConnectivityManager connMgr = (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        connMgr.registerDefaultNetworkCallback(networkCallback);
    }

    void resetStateForNetworkChange(boolean available, long atMs) {
        NightjarHostsJni.hostManagerResetNetworkState(hostManagerHandle);
        List<TrackedHost> tuples;
        synchronized (pollingTuples) {
            tuples = new ArrayList<>(pollingTuples);
        }
        List<TrackedHost> updated = new ArrayList<>();
        for (TrackedHost tuple : tuples) {
            synchronized (tuple.networkLock) {
                if (destroyed || tuple.removed) continue;
                if (tuple.lastSuccessfulPollMs >= atMs) continue;
                tuple.computer.state = available ? ComputerDetails.State.UNKNOWN : ComputerDetails.State.OFFLINE;
            }
            updated.add(tuple);
        }
        ComputerManagerListener currentListener = listener;
        if (currentListener == null || destroyed) return;
        for (TrackedHost tuple : updated) {
            currentListener.notifyComputerUpdated(tuple.computer);
        }
    }

    void readInternetPlay(ComputerDetails details) {
        if (!destroyed) {
            HostServiceHelpers.readInternetPlay(this, idManager, internetReads, wakeRequests, details);
        }
    }

    private void applyNetworkChange(boolean available, long atMs) {
        try {
            networkChanges.execute(() -> resetStateForNetworkChange(available, atMs));
        } catch (RejectedExecutionException ignored) {
        }
    }

    @Override
    public void onDestroy() {
        synchronized (discoveryServiceConnection) {
            destroyed = true;
            discoveryServiceConnection.notifyAll();
        }
        onUnbind(null);
        ConnectivityManager connMgr = (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        connMgr.unregisterNetworkCallback(networkCallback);
        if (discoveryServiceBound) {
            unbindService(discoveryServiceConnection);
            discoveryServiceBound = false;
        }
        discoveryBinder = null;
        wakeRequests.shutdownNow();
        wakeTargets.shutdown();
        networkChanges.shutdownNow();
        PreferenceManager.getDefaultSharedPreferences(this).unregisterOnSharedPreferenceChangeListener(internetPlaySetting);
        internetPlayCalls.shutdown();
        NightjarHostsJni.hostManagerDestroy(hostManagerHandle);
        hostManagerHandle = 0;
        defaultNetworkLock.hashCode();
        releaseLocalDatabaseReference();
    }

    public class ApplistPoller {
        private Thread thread;
        private final ComputerDetails computer;
        private long pollerHandle;

        public ApplistPoller(ComputerDetails computer) {
            this.computer = computer;
        }

        public void pollNow() {
            long h = pollerHandle;
            if (h != 0) NightjarHostsJni.applistPollerPollNow(h);
        }

        private boolean isCurrentWorker() {
            return thread == Thread.currentThread() && !Thread.currentThread().isInterrupted();
        }

        private synchronized boolean saveAppList(String appList) {
            if (!isCurrentWorker()) return false;
            HostServiceHelpers.saveAppListFile(ComputerManagerService.this, computer.uuid, appList);
            computer.rawAppList = appList;
            return true;
        }

        public synchronized void start() {
            if (thread != null) return;
            ComputerDetails.AddressTuple addr = HostServiceHelpers.currentAddressOrNull(computer);
            byte[] cert = HostServiceHelpers.certDer(computer.serverCert);
            if (addr == null || cert == null) return;
            final long h = NightjarHostsJni.applistPollerStart(
                    hostManagerHandle, addr.address, addr.port, computer.httpsPort, cert);
            pollerHandle = h;
            thread = new Thread(() -> {
                while (isCurrentWorker()) {
                    String rawXml = NightjarHostsJni.applistPollerNextUpdate(h);
                    if (rawXml == null || !isCurrentWorker()) break;
                    if (!saveAppList(rawXml)) break;
                    ComputerManagerListener currentListener = listener;
                    if (currentListener != null && isCurrentWorker()) {
                        currentListener.notifyAppListUpdated(computer, true);
                    }
                    FrontendSync.appListChanged(ComputerManagerService.this, computer);
                }
            }, "App list polling thread for " + computer.name);
            thread.start();
        }

        public synchronized void stop() {
            long h = pollerHandle;
            pollerHandle = 0;
            if (h != 0) NightjarHostsJni.applistPollerStop(h);
            if (thread != null) {
                Thread stopping = thread;
                thread = null;
                stopping.interrupt();
            }
        }
    }
}
