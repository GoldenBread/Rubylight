package com.limelight.nvstream;

import android.annotation.TargetApi;
import android.content.Context;
import android.net.ConnectivityManager;
import android.net.LinkAddress;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Build;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * While a stream runs, follows the default network. It tells the engine about each
 * default-network change and whether the new network is metered: with client roaming
 * (Experimental) on both sides the engine can move the stream (docs/protocol/client-roaming.md),
 * and never moves onto a network not reported here; otherwise it only answers whether the
 * connection carries on. It tells the listener when the stream's network is lost and when one
 * returns, so a stream whose connection cannot carry on reconnects at once; a network back
 * before its address toward the PC is asked about again as its addresses arrive.
 * The callbacks run on ConnectivityManager's thread, never the stream's.
 */
@TargetApi(Build.VERSION_CODES.N)
final class RoamingNetworkWatch {
    private final ConnectivityManager connMgr;
    private final RoamingNetworkTracker tracker;
    private final ConnectivityManager.NetworkCallback callback;
    // The tracker's settle check, on the main thread.
    private final android.os.Handler settleHandler = new android.os.Handler(android.os.Looper.getMainLooper());
    private final Runnable settleCheck = new Runnable() {
        @Override
        public void run() {
            tracker.settle();
        }
    };

    private RoamingNetworkWatch(ConnectivityManager connMgr, Network streamNetwork, String pcAddress,
                                NvConnectionListener listener) {
        this.connMgr = connMgr;
        tracker = new RoamingNetworkTracker(streamNetwork, new RoamingNetworkTracker.Reporter() {
            @Override
            public int networkChanged(boolean metered) {
                // Without roaming on both sides the engine only tells whether the connection
                // carries on; the C engine cannot tell, and the tracker answers for it.
                int change = MoonBridge.networkChanged(metered);
                LimeLog.info("Roaming: the default network changed (" + (metered ? "metered" : "not metered") +
                        "), " + (change == MoonBridge.NETWORK_ROAMED ? "roaming" :
                        change == MoonBridge.NETWORK_CANNOT_CONTINUE ? "not roaming, the connection cannot carry on" :
                        "not roaming, the connection carries on"));
                return change;
            }

            @Override
            public boolean answersForTheRoute() {
                return MoonBridge.isRustStream();
            }

            @Override
            public void settleAfter(long delayMs) {
                settleHandler.removeCallbacks(settleCheck);
                settleHandler.postDelayed(settleCheck, delayMs);
            }

            @Override
            public void streamNetworkLost() {
                LimeLog.warning("The stream's network was lost");
                listener.streamNetworkLost();
            }

            @Override
            public void streamNetworkReturned(int change, boolean metered) {
                listener.streamNetworkReturned(change, metered);
            }
        }, () -> System.nanoTime() / 1000000, addressFamily(pcAddress));
        callback = new ConnectivityManager.NetworkCallback() {
            @Override
            public void onAvailable(Network network) {
                // From Android 8 onCapabilitiesChanged always follows; before that, ask.
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                    NetworkCapabilities caps = connMgr.getNetworkCapabilities(network);
                    if (caps != null) {
                        tracker.capabilities(network, caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED),
                                caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN));
                    }
                }
            }

            @Override
            public void onCapabilitiesChanged(Network network, NetworkCapabilities caps) {
                tracker.capabilities(network, caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED),
                        caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN));
            }

            @Override
            public void onLinkPropertiesChanged(Network network, LinkProperties properties) {
                // Only the addresses: DNS and other updates do not move the route.
                java.util.Set<java.net.InetAddress> addresses = new java.util.HashSet<>();
                for (LinkAddress address : properties.getLinkAddresses()) {
                    addresses.add(address.getAddress());
                }
                tracker.addresses(network, addresses);
            }

            @Override
            public void onLost(Network network) {
                tracker.lost(network);
            }
        };
    }

    /**
     * The family of an address literal (Inet4Address or Inet6Address), or null for a host name or
     * none: names are not looked up here.
     */
    static Class<?> addressFamily(String address) {
        if (address == null || address.isEmpty()) return null;
        if (address.startsWith("[") && address.endsWith("]")) address = address.substring(1, address.length() - 1);
        // An IPv4-mapped literal (::ffff:192.168.1.10, or ::ffff:c0a8:10a) is an Inet4Address to Java.
        if (address.regionMatches(true, 0, "::ffff:", 0, 7)) {
            String rest = address.substring(7);
            if (rest.indexOf('.') > 0 || (rest.indexOf(':') > 0 && rest.indexOf(':') == rest.lastIndexOf(':'))) {
                return java.net.Inet4Address.class;
            }
        }
        if (address.indexOf(':') >= 0) return java.net.Inet6Address.class;
        for (int i = 0; i < address.length(); i++) {
            char c = address.charAt(i);
            if (c != '.' && (c < '0' || c > '9')) return null;
        }
        return java.net.Inet4Address.class;
    }

    /** The default network now, to pass to start() as the one the stream starts on; may be null. */
    static Network defaultNetwork(Context context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) {
            return null;
        }
        ConnectivityManager connMgr = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        return connMgr == null ? null : connMgr.getActiveNetwork();
    }

    /**
     * Starts reporting network changes, or returns null (with a warning) where it cannot: then
     * the stream does not roam onto another network.
     */
    static RoamingNetworkWatch start(Context context, Network streamNetwork, String pcAddress, NvConnectionListener listener) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) {
            LimeLog.warning("Roaming: needs Android 7 to follow network changes; not roaming");
            return null;
        }
        ConnectivityManager connMgr = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connMgr == null) {
            LimeLog.warning("Roaming: no ConnectivityManager; not roaming");
            return null;
        }
        RoamingNetworkWatch watch = new RoamingNetworkWatch(connMgr, streamNetwork, pcAddress, listener);
        try {
            connMgr.registerDefaultNetworkCallback(watch.callback);
        } catch (RuntimeException e) {
            LimeLog.warning("Roaming: could not follow network changes, not roaming: " + e);
            return null;
        }
        return watch;
    }

    /** The stream ends: no report reaches the engine after this returns. */
    void stop() {
        tracker.stop();
        settleHandler.removeCallbacks(settleCheck);
        try {
            connMgr.unregisterNetworkCallback(callback);
        } catch (RuntimeException e) {
            LimeLog.warning("Roaming: could not stop following network changes: " + e);
        }
    }
}
