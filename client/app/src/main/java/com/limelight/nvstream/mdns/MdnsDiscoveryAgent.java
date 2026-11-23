/**
 * Thin Android mDNS discovery base delegating generation tracking, candidate/answered
 * deduplication, and IPv6 SLAAC/filter ranking to {@link NightjarDiscoveryJni}.
 */
package com.limelight.nvstream.mdns;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarDiscoveryJni;

import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ConcurrentHashMap;

public abstract class MdnsDiscoveryAgent implements AutoCloseable {
    protected MdnsDiscoveryListener listener;

    private final long handle = NightjarDiscoveryJni.mdnsCreate();
    private final HandleReaper.Token handleToken =
            HandleReaper.register(this, handle, NightjarDiscoveryJni::mdnsDestroy);
    private final ConcurrentHashMap<String, InetAddress> addrCache = new ConcurrentHashMap<>();

    public MdnsDiscoveryAgent(MdnsDiscoveryListener listener) {
        this.listener = listener;
    }

    @Override
    public void close() {
        handleToken.close();
    }

    public abstract void startDiscovery(final int discoveryIntervalMs);

    public abstract void stopDiscovery();

    protected final long resetDiscovery() {
        return NightjarDiscoveryJni.mdnsReset(handle);
    }

    protected final long getDiscoveryGeneration() {
        return NightjarDiscoveryJni.mdnsGeneration(handle);
    }

    protected void reportNewComputer(String name, int port, Inet4Address[] v4Addrs, Inet6Address[] v6Addrs) {
        reportNewComputer(name, port, v4Addrs, v6Addrs, getDiscoveryGeneration());
    }

    protected void reportNewComputer(String name, int port, Inet4Address[] v4Addrs, Inet6Address[] v6Addrs,
                                     long generation) {
        LimeLog.info("mDNS: " + name + " has " + v4Addrs.length + " IPv4 addresses");
        LimeLog.info("mDNS: " + name + " has " + v6Addrs.length + " IPv6 addresses");

        for (Inet4Address v4 : v4Addrs) {
            if (v4 != null) addrCache.put(v4.getHostAddress(), v4);
        }
        for (Inet6Address v6 : v6Addrs) {
            if (v6 != null) addrCache.put(v6.getHostAddress(), v6);
        }

        Inet6Address v6GlobalAddr = getBestIpv6Address(v6Addrs);

        for (Inet4Address v4Addr : v4Addrs) {
            if (!addComputer(new MdnsComputer(name, v4Addr, v6GlobalAddr, port), generation)) {
                return;
            }
        }
        if (v4Addrs.length == 0) {
            Inet6Address v6LocalAddr = getLocalAddress(v6Addrs);

            if (v6LocalAddr != null || v6GlobalAddr != null) {
                addComputer(new MdnsComputer(name, v6LocalAddr, v6GlobalAddr, port), generation);
            }
        }
    }

    private boolean addComputer(MdnsComputer computer, long generation) {
        String localKey = keyOf(computer.getLocalAddress());
        String v6Key = keyOf(computer.getIpv6Address());
        int decision = NightjarDiscoveryJni.mdnsBeginCandidate(
                handle, computer.getName(), computer.getPort(), localKey, v6Key, generation);
        if (decision == 0) {
            return false;
        }
        // The probe can block on the network, so it runs without any Rust or Java lock held.
        if (decision == 2 && listener.notifyComputerAdded(computer)) {
            NightjarDiscoveryJni.mdnsMarkAnswered(
                    handle, computer.getName(), computer.getPort(), localKey, v6Key);
        }
        return true;
    }

    // A deleted PC must be able to return the next time discovery finds it.
    public void forgetAnsweredComputers() {
        NightjarDiscoveryJni.mdnsForgetAnswered(handle);
    }

    public List<MdnsComputer> getComputerSet() {
        Json.Arr arr = Json.parseArray(NightjarDiscoveryJni.mdnsComputers(handle));
        List<MdnsComputer> out = new ArrayList<>(arr.length());
        for (int i = 0; i < arr.length(); i++) {
            Json.Obj item = arr.getObject(i);
            String name = item.getString("name");
            int port = item.getInt("port");
            InetAddress local = resolveCached(item.isNull("local") ? null : item.optString("local", null));
            InetAddress v6Raw = resolveCached(item.isNull("v6") ? null : item.optString("v6", null));
            Inet6Address v6 = v6Raw instanceof Inet6Address ? (Inet6Address) v6Raw : null;
            out.add(new MdnsComputer(name, local, v6, port));
        }
        return out;
    }

    private static String keyOf(InetAddress addr) {
        return addr == null ? null : addr.getHostAddress();
    }

    private InetAddress resolveCached(String hostAddr) {
        if (hostAddr == null || hostAddr.isEmpty()) {
            return null;
        }
        InetAddress cached = addrCache.get(hostAddr);
        if (cached != null) {
            return cached;
        }
        try {
            return InetAddress.getByName(hostAddr);
        } catch (UnknownHostException e) {
            return null;
        }
    }

    private static String encodeAddrs(Inet6Address[] addresses) {
        Json.Arr arr = Json.arr();
        for (Inet6Address addr : addresses) {
            arr.add(addr == null ? "" : addr.getHostAddress());
        }
        return arr.toString();
    }

    protected static Inet6Address getLocalAddress(Inet6Address[] addresses) {
        if (addresses == null || addresses.length == 0) {
            return null;
        }
        int[] selected = NightjarDiscoveryJni.mdnsSelectIpv6(encodeAddrs(addresses));
        if (selected == null || selected.length < 2) {
            return null;
        }
        int idx = selected[1];
        return idx >= 0 && idx < addresses.length ? addresses[idx] : null;
    }

    protected static Inet6Address getLinkLocalAddress(Inet6Address[] addresses) {
        if (addresses == null || addresses.length == 0) {
            return null;
        }
        int idx = NightjarDiscoveryJni.mdnsLinkLocalIpv6(encodeAddrs(addresses));
        return idx >= 0 && idx < addresses.length ? addresses[idx] : null;
    }

    protected static Inet6Address getBestIpv6Address(Inet6Address[] addresses) {
        if (addresses == null || addresses.length == 0) {
            return null;
        }
        int[] selected = NightjarDiscoveryJni.mdnsSelectIpv6(encodeAddrs(addresses));
        if (selected == null || selected.length < 1) {
            return null;
        }
        int idx = selected[0];
        return idx >= 0 && idx < addresses.length ? addresses[idx] : null;
    }
}
