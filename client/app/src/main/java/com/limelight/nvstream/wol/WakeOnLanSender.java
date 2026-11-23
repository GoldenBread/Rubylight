/**
 * Thin Android Wake-on-LAN adapter delegating packet creation, port expansion, planning, and
 * UDP socket bursts to {@link NightjarDiscoveryJni} while keeping Android {@link NetworkInterface}
 * broadcast/IPv6-scope enumeration in Java.
 */
package com.limelight.nvstream.wol;

import android.content.Context;

import com.limelight.LimeLog;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarDiscoveryJni;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.InterfaceAddress;
import java.net.NetworkInterface;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Enumeration;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

public class WakeOnLanSender {
    private static final byte[] ALL_NODES = {(byte) 0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1};

    /** What one wake sends: each payload (one per MAC) to every destination, and to the best-effort ones. */
    static final class Plan {
        final List<byte[]> payloads;
        final Set<InetSocketAddress> destinations;
        /** IPv6 all-nodes on each link; sends there may all fail without failing the wake. */
        final Set<InetSocketAddress> bestEffort;

        Plan(List<byte[]> payloads, Set<InetSocketAddress> destinations, Set<InetSocketAddress> bestEffort) {
            this.payloads = payloads;
            this.destinations = destinations;
            this.bestEffort = bestEffort;
        }
    }

    static Set<InetSocketAddress> destinations(InetAddress address, int httpPort) {
        int[] ports = NightjarDiscoveryJni.wolPorts(httpPort);
        Set<InetSocketAddress> destinations = new LinkedHashSet<>();
        for (int port : ports) {
            destinations.add(new InetSocketAddress(address, port));
        }
        return destinations;
    }

    static Plan plan(ComputerDetails computer, WakeTargets.Targets extra, Set<InetAddress> localBroadcasts,
                     Set<InetAddress> allNodes) throws IOException {
        Json.Arr macs = Json.arr();
        if (computer.macAddress != null) macs.add(computer.macAddress);
        for (String mac : extra.macs) macs.add(mac);

        Json.Arr endpoints = Json.arr();
        Json.Arr resolvedEndpoints = Json.arr();
        for (ComputerDetails.AddressTuple address : new ComputerDetails.AddressTuple[] {
                computer.localAddress, computer.remoteAddress, computer.manualAddress, computer.ipv6Address,
        }) {
            if (address == null) continue;
            endpoints.add(Json.obj().put("addr", address.address).put("port", address.port));
            try {
                for (InetAddress resolved : InetAddress.getAllByName(address.address)) {
                    resolvedEndpoints.add(Json.obj().put("addr", resolved.getHostAddress()).put("port", address.port));
                }
            } catch (IOException e) {
                LimeLog.info("Wake-on-LAN skips " + address + ": " + e.getMessage());
            }
        }

        Json.Arr wakeBroadcasts = Json.arr();
        for (String b : extra.broadcasts) wakeBroadcasts.add(b);
        Json.Arr localBcasts = Json.arr();
        for (InetAddress b : localBroadcasts) localBcasts.add(b.getHostAddress());

        Json.Arr ipv6Scopes = Json.arr();
        Map<Integer, InetAddress> scopeNodes = new HashMap<>();
        for (InetAddress link : allNodes) {
            int scope = link instanceof Inet6Address ? ((Inet6Address) link).getScopeId() : 0;
            ipv6Scopes.add(scope);
            scopeNodes.put(scope, link);
        }

        Json.Obj req = Json.obj()
                .put("macs", macs)
                .put("endpoints", endpoints)
                .put("resolved_endpoints", resolvedEndpoints)
                .put("wake_broadcasts", wakeBroadcasts)
                .put("local_broadcasts", localBcasts)
                .put("ipv6_scopes", ipv6Scopes)
                .put("fallback_port", 47989)
                .put("use_pre_resolved", true);
        Json.Obj reply = Json.parseObject(NightjarDiscoveryJni.wolPlan(req.toString()));
        if (reply.has("error")) {
            throw new IOException(reply.getString("error"));
        }
        Json.Obj ok = reply.getObject("ok");
        Json.Arr payloadsHex = ok.getArray("payloads_hex");
        List<byte[]> payloads = new ArrayList<>(payloadsHex.length());
        for (int i = 0; i < payloadsHex.length(); i++) {
            payloads.add(decodeHex(payloadsHex.getString(i)));
        }
        Set<InetSocketAddress> destinations = new LinkedHashSet<>();
        Json.Arr destArr = ok.getArray("destinations");
        for (int i = 0; i < destArr.length(); i++) {
            Json.Obj d = destArr.getObject(i);
            InetAddress addr = InetAddress.getByName(d.getString("ip"));
            int scope = d.isNull("scope") ? 0 : d.optInt("scope", 0);
            if (scope > 0 && addr instanceof Inet6Address) {
                addr = Inet6Address.getByAddress(null, addr.getAddress(), scope);
            }
            destinations.add(new InetSocketAddress(addr, d.getInt("port")));
        }
        Set<InetSocketAddress> bestEffort = new LinkedHashSet<>();
        Json.Arr bestArr = ok.getArray("best_effort");
        for (int i = 0; i < bestArr.length(); i++) {
            Json.Obj d = bestArr.getObject(i);
            int scope = d.isNull("scope") ? 0 : d.optInt("scope", 0);
            InetAddress node = scopeNodes.get(scope);
            if (node == null) {
                node = scope > 0 ? Inet6Address.getByAddress(null, ALL_NODES, scope) : InetAddress.getByName(d.getString("ip"));
            }
            bestEffort.add(new InetSocketAddress(node, d.getInt("port")));
        }
        return new Plan(payloads, destinations, bestEffort);
    }

    static void send(Plan plan, DatagramSocket sock) throws IOException {
        IOException lastException = null;
        boolean sentWolPacket = false;
        int bestEffortFailures = 0;
        for (int attempt = 0; attempt < 3; attempt++) {
            for (byte[] payload : plan.payloads) {
                for (InetSocketAddress destination : plan.destinations) {
                    try {
                        sock.send(new DatagramPacket(payload, payload.length, destination));
                        sentWolPacket = true;
                    } catch (IOException e) {
                        lastException = e;
                    }
                }
                for (InetSocketAddress destination : plan.bestEffort) {
                    try {
                        sock.send(new DatagramPacket(payload, payload.length, destination));
                    } catch (IOException e) {
                        bestEffortFailures++;
                    }
                }
            }
        }
        if (bestEffortFailures > 0) {
            LimeLog.info("Wake-on-LAN: " + bestEffortFailures + " IPv6 all-nodes sends failed");
        }
        if (!sentWolPacket) {
            throw lastException != null ? lastException : new IOException("No Wake-on-LAN destinations");
        }
    }

    /**
     * Sends the magic packets for the PC (blocks). Throws when no packet could be sent at all, or
     * when the PC has no usable MAC address.
     */
    public static void sendWolPacket(Context context, ComputerDetails computer) throws IOException {
        WakeTargets.Targets extra = new WakeTargets(context).load(computer.uuid);
        Json.Arr macs = Json.arr();
        if (computer.macAddress != null) macs.add(computer.macAddress);
        for (String mac : extra.macs) macs.add(mac);

        Json.Arr endpoints = Json.arr();
        for (ComputerDetails.AddressTuple address : new ComputerDetails.AddressTuple[] {
                computer.localAddress, computer.remoteAddress, computer.manualAddress, computer.ipv6Address,
        }) {
            if (address != null) {
                endpoints.add(Json.obj().put("addr", address.address).put("port", address.port));
            }
        }

        Json.Arr wakeBroadcasts = Json.arr();
        for (String b : extra.broadcasts) wakeBroadcasts.add(b);

        Json.Arr localBroadcasts = Json.arr();
        Json.Arr ipv6Scopes = Json.arr();
        try {
            Enumeration<NetworkInterface> interfaces = NetworkInterface.getNetworkInterfaces();
            for (NetworkInterface nic : interfaces == null ? Collections.<NetworkInterface>emptyList() : Collections.list(interfaces)) {
                if (!nic.isUp() || nic.isLoopback()) continue;
                boolean ipv6 = false;
                for (InterfaceAddress address : nic.getInterfaceAddresses()) {
                    if (address.getBroadcast() instanceof Inet4Address) {
                        localBroadcasts.add(address.getBroadcast().getHostAddress());
                    }
                    ipv6 |= address.getAddress() instanceof Inet6Address;
                }
                if (ipv6 && nic.supportsMulticast() && nic.getIndex() > 0) {
                    ipv6Scopes.add(nic.getIndex());
                }
            }
        } catch (IOException | SecurityException e) {
            LimeLog.warning("Wake-on-LAN cannot list the network interfaces: " + e);
        }

        Json.Obj req = Json.obj()
                .put("macs", macs)
                .put("endpoints", endpoints)
                .put("wake_broadcasts", wakeBroadcasts)
                .put("local_broadcasts", localBroadcasts)
                .put("ipv6_scopes", ipv6Scopes)
                .put("fallback_port", 47989)
                .put("use_pre_resolved", false);
        String err = NightjarDiscoveryJni.wolSend(req.toString());
        if (err != null && !err.isEmpty()) {
            throw new IOException(err);
        }
    }

    static byte[] createWolPayload(String macAddress) throws IOException {
        byte[] payload = NightjarDiscoveryJni.wolMagicPacket(macAddress);
        if (payload == null) {
            throw new IOException("Invalid Wake-on-LAN MAC address");
        }
        return payload;
    }

    private static byte[] decodeHex(String hex) {
        byte[] out = new byte[hex.length() / 2];
        for (int i = 0; i < out.length; i++) {
            out[i] = (byte) Integer.parseInt(hex.substring(i * 2, i * 2 + 2), 16);
        }
        return out;
    }
}
