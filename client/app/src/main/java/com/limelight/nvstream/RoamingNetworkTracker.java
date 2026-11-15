package com.limelight.nvstream;

import com.limelight.nvstream.jni.MoonBridge;

/**
 * Client roaming (Experimental): which default-network callbacks are a network change the
 * Rust engine must hear about. Plain Java, so the decisions are tested on the JVM;
 * {@link RoamingNetworkWatch} feeds it from ConnectivityManager.
 *
 * <p>The network the stream started on is no change. A new default network is one, and so is
 * the same network coming back after it was lost. Capability updates of the network already
 * reported are not: each report starts a roaming attempt in the engine.
 *
 * <p>Losing the stream's network, and a network coming back after that, are reported too: a
 * return the stream's connection cannot carry on over lets the stream reconnect at once
 * instead of waiting for its connection to time out. A network is often back before it has an
 * address toward the PC (IPv6 first, DHCP later), and then the connection carries on for now;
 * so for {@link #RECHECK_MS} each change of its addresses asks again, and an answer that is no
 * longer "carries on" is reported as the return.
 *
 * <p>An engine that cannot tell whether its connection carries on (the C engine, whose sockets
 * stay on the address they started from) gets the answer from here: a return whose addresses
 * share none with the lost network's cannot carry on, so the stream reconnects at once instead
 * of freezing until its connection times out. So does a switch to another default network while
 * the old one is still up (mobile data already connected when Wi-Fi goes), which Android reports
 * without a loss: it is compared with the old network's addresses, and reported as a return
 * once it cannot carry on.
 */
final class RoamingNetworkTracker {
    interface Reporter {
        /**
         * MoonBridge.networkChanged: metered is the new network's !NET_CAPABILITY_NOT_METERED.
         * Returns what it means for the stream (MoonBridge.NETWORK_*).
         */
        int networkChanged(boolean metered);

        /** The stream's network went away; the stream cannot reach the PC until one returns. */
        default void streamNetworkLost() {}

        /**
         * A default network after the stream's was lost: change is networkChanged's answer,
         * metered the new network's.
         */
        default void streamNetworkReturned(int change, boolean metered) {}

        /**
         * Whether networkChanged's answer tells if the connection carries on. False for an engine
         * that always answers that it does (the C engine): then a return onto other addresses
         * is reported as NETWORK_CANNOT_CONTINUE.
         */
        default boolean answersForTheRoute() { return true; }
    }

    /** Elapsed milliseconds on a monotonic clock. */
    interface Clock {
        long millis();
    }

    /**
     * How long a return the connection carries on over is asked about again: the connection's
     * own timeout (NetworkReturnRecovery.PEER_TIMEOUT_MS), which decides after that.
     */
    static final long RECHECK_MS = 10000;

    private final Reporter reporter;
    private final Clock clock;
    // Inet4Address or Inet6Address: the family of the PC's address, so only addresses of the family
    // the stream reaches the PC with decide whether it carries on; null when unknown (a host name).
    private final Class<?> pcFamily;
    private Object current;
    private boolean lost;
    private boolean stopped;
    // The network that came back with the connection carrying on, while it is asked again.
    private Object recheck;
    private boolean recheckMetered;
    private long recheckUntil;
    private Object recheckAddresses;
    // The newest addresses reported for each network, and those of the network the stream's
    // connection left (lost, or switched away from).
    private final java.util.Map<Object, Object> addressesOf = new java.util.HashMap<>();
    private Object lostAddresses;

    /** streamNetwork: the default network when the stream started, or null if unknown. */
    RoamingNetworkTracker(Object streamNetwork, Reporter reporter) {
        this(streamNetwork, reporter, () -> System.nanoTime() / 1000000);
    }

    RoamingNetworkTracker(Object streamNetwork, Reporter reporter, Clock clock) {
        this(streamNetwork, reporter, clock, null);
    }

    /** pcFamily: Inet4Address.class or Inet6Address.class for the PC's address, or null if unknown. */
    RoamingNetworkTracker(Object streamNetwork, Reporter reporter, Clock clock, Class<?> pcFamily) {
        this.reporter = reporter;
        this.clock = clock;
        this.pcFamily = pcFamily;
        current = streamNetwork;
    }

    /** The default network's capabilities, from onCapabilitiesChanged (or onAvailable). */
    synchronized void capabilities(Object network, boolean notMetered) {
        if (stopped) {
            return;
        }
        if (current == null) {
            // Nothing known about the stream's network: the first one seen is where it runs.
            current = network;
            return;
        }
        if (network.equals(current) && !lost) {
            if (network.equals(recheck)) {
                recheckMetered = !notMetered;
            }
            return;
        }
        boolean returned = lost;
        Object previous = current;
        current = network;
        lost = false;
        recheck = null;
        // Under the lock, so stop() returning means no report is still on its way.
        int change = reporter.networkChanged(!notMetered);
        // Without a loss only an engine that cannot tell is asked about here: one that tells
        // roams or answers on its own.
        boolean switched = !returned && !reporter.answersForTheRoute();
        if (switched) {
            lostAddresses = addressesOf.get(previous);
        }
        if (returned || switched) {
            change = routeAnswer(change, addressesOf.get(network));
            if (change == MoonBridge.NETWORK_CARRIES_ON) {
                // The same address back, or no route yet: ask again when its addresses change.
                recheck = network;
                recheckMetered = !notMetered;
                recheckUntil = clock.millis() + RECHECK_MS;
                recheckAddresses = null;
            }
            // A switch that carries on for now is no news; one that cannot is reported as a return.
            if (returned || change != MoonBridge.NETWORK_CARRIES_ON) {
                reporter.streamNetworkReturned(change, !notMetered);
            }
        }
    }

    /**
     * A network's addresses, from onLinkPropertiesChanged (any object that is equal for the
     * same addresses). For the network that came back with the connection carrying on, a change
     * within {@link #RECHECK_MS} asks the engine again: an address toward the PC that arrived
     * after the return (DHCP) may strand the connection, and then it is reported as the return.
     */
    synchronized void addresses(Object network, Object addresses) {
        if (stopped) {
            return;
        }
        addressesOf.put(network, addresses);
        if (lost || recheck == null || !network.equals(recheck)) {
            return;
        }
        if (clock.millis() > recheckUntil) {
            recheck = null;
            return;
        }
        if (addresses.equals(recheckAddresses)) {
            return;
        }
        recheckAddresses = addresses;
        int change = routeAnswer(reporter.networkChanged(recheckMetered), addresses);
        if (change != MoonBridge.NETWORK_CARRIES_ON) {
            recheck = null;
            reporter.streamNetworkReturned(change, recheckMetered);
        }
    }

    /** The default network went away; the same network coming back is then a change. */
    synchronized void lost(Object network) {
        if (stopped) {
            return;
        }
        Object addresses = addressesOf.remove(network);
        if (lost || !network.equals(current)) {
            return;
        }
        lost = true;
        recheck = null;
        lostAddresses = addresses;
        reporter.streamNetworkLost();
    }

    /**
     * The engine's answer for a returned network with these addresses. An engine that cannot
     * tell carries on while the network has no routable address yet (link-local only), while
     * an address of the lost network is back, or when the lost network's addresses are unknown;
     * routable addresses without any of the lost ones strand its connection. Only addresses of
     * the PC's family count when it is known: a new temporary IPv6 address arriving before the
     * same IPv4 lease says nothing about an IPv4 stream.
     */
    private int routeAnswer(int change, Object addresses) {
        if (change != MoonBridge.NETWORK_CARRIES_ON || reporter.answersForTheRoute() ||
                addresses == null || lostAddresses == null) {
            return change;
        }
        if (!(addresses instanceof java.util.Collection) || !(lostAddresses instanceof java.util.Collection)) {
            return addresses.equals(lostAddresses) ? change : MoonBridge.NETWORK_CANNOT_CONTINUE;
        }
        boolean routable = false;
        for (Object address : (java.util.Collection<?>) addresses) {
            if (!routable(address) || (pcFamily != null && !pcFamily.isInstance(address))) continue;
            if (((java.util.Collection<?>) lostAddresses).contains(address)) return change;
            routable = true;
        }
        return routable ? MoonBridge.NETWORK_CANNOT_CONTINUE : change;
    }

    /** Whether an address can carry a route toward the PC: not link-local, loopback or unspecified. */
    private static boolean routable(Object address) {
        if (!(address instanceof java.net.InetAddress)) return true;
        java.net.InetAddress inet = (java.net.InetAddress) address;
        return !inet.isLinkLocalAddress() && !inet.isLoopbackAddress() && !inet.isAnyLocalAddress();
    }

    /** The stream ends: nothing is reported after this returns. */
    synchronized void stop() {
        stopped = true;
    }
}
