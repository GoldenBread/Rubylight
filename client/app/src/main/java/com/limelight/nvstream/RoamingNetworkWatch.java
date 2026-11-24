package com.limelight.nvstream;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.LinkAddress;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Build;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * While a stream runs, follows the default network via {@link ConnectivityManager.NetworkCallback}
 * and delegates tracking state transitions to {@code nightjar_policy::roaming::RoamingTracker} via
 * {@link NightjarPolicyJni}.
 */
final class RoamingNetworkWatch {
    private final ConnectivityManager connMgr;
    private final long trackerHandle;
    private final HandleReaper.Token reaper;
    private final ConnectivityManager.NetworkCallback callback;

    private RoamingNetworkWatch(ConnectivityManager connMgr, Network streamNetwork,
                                NvConnectionListener listener) {
        this.connMgr = connMgr;
        this.trackerHandle = NightjarPolicyJni.roamingCreate(
                streamNetwork != null, streamNetwork != null ? streamNetwork.getNetworkHandle() : 0L);
        this.reaper = HandleReaper.register(this, trackerHandle, NightjarPolicyJni::roamingDestroy);
        callback = new ConnectivityManager.NetworkCallback() {
            @Override
            public void onAvailable(Network network) {
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                    NetworkCapabilities caps = connMgr.getNetworkCapabilities(network);
                    if (caps != null) {
                        handleCapabilities(network, caps);
                    }
                }
            }

            @Override
            public void onCapabilitiesChanged(Network network, NetworkCapabilities caps) {
                handleCapabilities(network, caps);
            }

            @Override
            public void onLinkPropertiesChanged(Network network, LinkProperties properties) {
                handleAddresses(network, properties);
            }

            @Override
            public void onLost(Network network) {
                boolean lost;
                synchronized (RoamingNetworkWatch.this) {
                    lost = NightjarPolicyJni.roamingOnLost(trackerHandle, network.getNetworkHandle());
                }
                if (lost) {
                    LimeLog.warning("The stream's network was lost");
                    listener.streamNetworkLost();
                }
            }

            private void handleCapabilities(Network network, NetworkCapabilities caps) {
                long networkId = network.getNetworkHandle();
                boolean notMetered = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED);
                int step;
                boolean metered;
                boolean returned;
                int change;
                boolean emitReturn;
                synchronized (RoamingNetworkWatch.this) {
                    step = NightjarPolicyJni.roamingOnCapabilities(trackerHandle, networkId, notMetered);
                    if (step == NightjarPolicyJni.ROAMING_CAP_IGNORE) {
                        return;
                    }
                    metered = step == NightjarPolicyJni.ROAMING_CAP_ASK_METERED
                            || step == NightjarPolicyJni.ROAMING_CAP_RETURNED_ASK_METERED;
                    returned = step == NightjarPolicyJni.ROAMING_CAP_RETURNED_ASK_NOT_METERED
                            || step == NightjarPolicyJni.ROAMING_CAP_RETURNED_ASK_METERED;
                    change = askEngine(metered);
                    emitReturn = NightjarPolicyJni.roamingRecordChange(
                            trackerHandle, networkId, metered, returned, change, System.nanoTime() / 1_000_000L);
                }
                if (emitReturn) {
                    listener.streamNetworkReturned(change, metered);
                }
            }

            private void handleAddresses(Network network, LinkProperties properties) {
                long networkId = network.getNetworkHandle();
                String addressesKey = canonicalAddresses(properties);
                boolean nat64 = Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && properties.getNat64Prefix() != null;
                long nowMs = System.nanoTime() / 1_000_000L;
                int change;
                boolean metered;
                boolean emitReturn;
                synchronized (RoamingNetworkWatch.this) {
                    int step = NightjarPolicyJni.roamingOnAddresses(trackerHandle, networkId, addressesKey, nat64, nowMs);
                    if (step == NightjarPolicyJni.ROAMING_ADDR_IGNORE) {
                        return;
                    }
                    metered = step == NightjarPolicyJni.ROAMING_ADDR_ASK_METERED;
                    change = askEngine(metered);
                    emitReturn = NightjarPolicyJni.roamingRecordAddressChange(trackerHandle, change);
                }
                if (emitReturn) {
                    listener.streamNetworkReturned(change, metered);
                }
            }
        };
    }

    private static int askEngine(boolean metered) {
        int change = MoonBridge.networkChanged(metered);
        LimeLog.info("Roaming: the default network changed (" + (metered ? "metered" : "not metered") +
                "), " + (change == MoonBridge.NETWORK_ROAMED ? "roaming" :
                change == MoonBridge.NETWORK_CANNOT_CONTINUE ? "not roaming, the connection cannot carry on" :
                "not roaming, the connection carries on"));
        return change;
    }

    private static String canonicalAddresses(LinkProperties properties) {
        List<String> list = new ArrayList<>();
        for (LinkAddress address : properties.getLinkAddresses()) {
            if (address.getAddress() != null) {
                String text = address.getAddress().getHostAddress();
                if (text != null && !list.contains(text)) {
                    list.add(text);
                }
            }
        }
        Collections.sort(list);
        return String.join(" ", list);
    }

    static Network defaultNetwork(Context context) {
        ConnectivityManager connMgr = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        return connMgr == null ? null : connMgr.getActiveNetwork();
    }

    static RoamingNetworkWatch start(Context context, Network streamNetwork, NvConnectionListener listener) {
        ConnectivityManager connMgr = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connMgr == null) {
            LimeLog.warning("Roaming: no ConnectivityManager; not roaming");
            return null;
        }
        RoamingNetworkWatch watch = new RoamingNetworkWatch(connMgr, streamNetwork, listener);
        try {
            connMgr.registerDefaultNetworkCallback(watch.callback);
        } catch (RuntimeException e) {
            LimeLog.warning("Roaming: could not follow network changes, not roaming: " + e);
            watch.reaper.close();
            return null;
        }
        return watch;
    }

    synchronized void stop() {
        NightjarPolicyJni.roamingStop(trackerHandle);
        try {
            connMgr.unregisterNetworkCallback(callback);
        } catch (RuntimeException e) {
            LimeLog.warning("Roaming: could not stop following network changes: " + e);
        } finally {
            reaper.close();
        }
    }
}
