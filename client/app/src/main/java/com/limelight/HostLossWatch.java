package com.limelight;

import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.RouteInfo;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarJni;

import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.function.BooleanSupplier;
import java.util.function.Function;

/**
 * After this session's host said goodbye: delegates probing to {@code nightjar_core::host_loss::watch_goodbye}
 * via {@link NightjarJni}, supplying only Android {@link ConnectivityManager} on-link gateway lookups and
 * DNS-free IP literal parsing.
 */
final class HostLossWatch {
    static final int GOODBYE_WAIT = 0;
    static final int GOODBYE_GONE = 1;
    static final int GOODBYE_UNKNOWN = 2;

    interface WatchRunner {
        int run(long watchHandle, String host, int port, NightjarJni.HostLossCallback callback);
    }

    private final Function<InetAddress, String> gatewayToward;
    private final BooleanSupplier saidGoodbye;
    private final WatchRunner runner;
    private final InetAddress host;
    private final String literal;
    private final int port;
    private final long watchHandle;
    private final HandleReaper.Token reaper;
    private volatile boolean stopped;

    HostLossWatch(ConnectivityManager connectivity, InetAddress host, int port) {
        this(address -> gatewayOn(connectivity, address), MoonBridge::hostSaidGoodbye,
                NightjarJni::hostLossWatchGoodbye, host, port);
    }

    HostLossWatch(Function<InetAddress, String> gatewayToward, BooleanSupplier saidGoodbye,
                  WatchRunner runner, InetAddress host, int port) {
        this.gatewayToward = gatewayToward;
        this.saidGoodbye = saidGoodbye;
        this.runner = runner;
        this.host = host;
        this.literal = literal(host);
        this.port = port;
        this.watchHandle = NightjarJni.isAvailable() ? NightjarJni.hostLossWatchCreate() : 0;
        this.reaper = watchHandle > 0 ? HandleReaper.register(this, watchHandle, NightjarJni::hostLossWatchDestroy) : null;
    }

    static InetAddress parse(String address) {
        if (address == null || address.isEmpty()) return null;
        boolean ipv6 = address.indexOf(':') >= 0;
        for (int i = 0; i < address.length(); i++) {
            char c = address.charAt(i);
            boolean allowed = Character.isDigit(c) || c == '.' || (ipv6 && (Character.digit(c, 16) >= 0 || c == ':' || c == '%'));
            if (!allowed) return null;
        }
        try {
            return InetAddress.getByName(address);
        } catch (UnknownHostException e) {
            return null;
        }
    }

    void stop() {
        stopped = true;
        if (watchHandle > 0) NightjarJni.hostLossWatchStop(watchHandle);
        if (reaper != null) reaper.close();
    }

    int afterGoodbye() {
        if (stopped || !saidGoodbye.getAsBoolean()) return GOODBYE_UNKNOWN;
        int verdict = runner.run(watchHandle, literal, port, new NightjarJni.HostLossCallback() {
            @Override public String gatewayToward() { return gatewayToward.apply(host); }
            @Override public boolean isStopped() { return stopped; }
            @Override public void log(String line) { LimeLog.info(line); }
        });
        if (verdict == GOODBYE_GONE) LimeLog.info("The PC is gone: it said goodbye and closed its server");
        return verdict;
    }

    private static String literal(InetAddress host) {
        String text = host.getHostAddress();
        int zone = text.indexOf('%');
        return zone < 0 ? text : text.substring(0, zone);
    }

    static String gatewayToward(Iterable<RouteInfo> routes, InetAddress host) {
        boolean onLink = false;
        InetAddress gateway = null;
        for (RouteInfo route : routes) {
            if (route.isDefaultRoute()) {
                if (hasGateway(route) && route.getGateway() instanceof Inet4Address) gateway = route.getGateway();
            } else if (!hasGateway(route) && route.matches(host)) {
                onLink = true;
            }
        }
        return onLink && gateway != null ? gateway.getHostAddress() : null;
    }

    private static boolean hasGateway(RouteInfo route) {
        InetAddress gateway = route.getGateway();
        return gateway != null && !gateway.isAnyLocalAddress();
    }

    private static String gatewayOn(ConnectivityManager connectivity, InetAddress host) {
        Network network = connectivity.getActiveNetwork();
        if (network == null) return null;
        NetworkCapabilities capabilities = connectivity.getNetworkCapabilities(network);
        LinkProperties link = connectivity.getLinkProperties(network);
        if (capabilities == null || link == null || capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) return null;
        return gatewayToward(link.getRoutes(), host);
    }
}
