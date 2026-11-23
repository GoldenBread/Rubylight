package com.limelight.computers;

import com.limelight.nvstream.jni.NightjarDiscoveryJni;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.SocketTimeoutException;
import java.net.UnknownHostException;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * IPv4 STUN binding discovery on a socket the caller binds, delegating RFC 5389 packet
 * encoding/decoding to Rust ({@code nightjar_core::stun}) via {@link NightjarDiscoveryJni}.
 */
final class StunClient {
    interface Resolver {
        InetAddress[] resolve(String host) throws UnknownHostException;
    }

    interface SocketBinder {
        void bind(DatagramSocket socket) throws IOException;
    }

    private StunClient() {
    }

    /**
     * The WAN IPv4 address the STUN server sees, or null without an answer before the timeout.
     * The request goes to up to three IPv4 addresses of the server, again every second.
     */
    static String query(String hostname, int port, long timeoutMs, Resolver resolver, SocketBinder binder)
            throws IOException {
        long deadline = System.nanoTime() + timeoutMs * 1_000_000L;
        List<InetAddress> servers = new ArrayList<>();
        for (InetAddress address : resolver.resolve(hostname)) {
            if (address instanceof Inet4Address && servers.size() < 3) servers.add(address);
        }
        if (servers.isEmpty()) return null;
        byte[] transaction = new byte[12];
        new SecureRandom().nextBytes(transaction);
        byte[] request = NightjarDiscoveryJni.stunBindingRequest(transaction);
        if (request == null) {
            throw new IOException("Unable to build STUN binding request");
        }
        try (DatagramSocket socket = new DatagramSocket()) {
            binder.bind(socket);
            long nextSend = System.nanoTime();
            byte[] buffer = new byte[1024];
            while (System.nanoTime() < deadline) {
                long now = System.nanoTime();
                if (now >= nextSend) {
                    for (InetAddress server : servers) {
                        socket.send(new DatagramPacket(request, request.length, server, port));
                    }
                    nextSend = now + 1_000_000_000L;
                }
                long left = (deadline - System.nanoTime()) / 1_000_000L;
                socket.setSoTimeout((int) Math.max(1, Math.min(200, left)));
                DatagramPacket response = new DatagramPacket(buffer, buffer.length);
                try {
                    socket.receive(response);
                } catch (SocketTimeoutException e) {
                    continue;
                }
                if (response.getPort() != port || !servers.contains(response.getAddress())) continue;
                String address = decode(Arrays.copyOf(buffer, response.getLength()), transaction);
                if (address != null) return address;
            }
        }
        return null;
    }

    /** The mapped IPv4 address of a binding success for this transaction, else null. */
    static String decode(byte[] data, byte[] transaction) {
        if (data == null || transaction == null) return null;
        return NightjarDiscoveryJni.stunMappedAddress(data, transaction);
    }
}
