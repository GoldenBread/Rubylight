package com.limelight;

import android.app.Application;
import android.net.IpPrefix;
import android.net.RouteInfo;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarJni;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.net.InetAddress;
import java.net.ServerSocket;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostLossWatchTest {
    @Test public void aClosedServerEndsAfterOneRound() throws Exception {
        List<String> logs = new ArrayList<>();
        HostLossWatch watch = new HostLossWatch(
                addr -> "192.168.4.1",
                () -> true,
                (handle, host, port, cb) -> {
                    assertEquals("192.168.4.69", host);
                    assertEquals("192.168.4.1", cb.gatewayToward());
                    assertFalse(cb.isStopped());
                    cb.log("Host check after the goodbye, 0 ms in: SilentLinkUp: Gone");
                    return HostLossWatch.GOODBYE_GONE;
                },
                InetAddress.getByName("192.168.4.69"), 47984);
        assertEquals(HostLossWatch.GOODBYE_GONE, watch.afterGoodbye());
    }

    @Test public void aHostStillExitingGetsMoreRoundsWithTheTimeWaited() throws Exception {
        AtomicInteger rounds = new AtomicInteger();
        HostLossWatch watch = new HostLossWatch(
                addr -> "192.168.4.1",
                () -> true,
                (handle, host, port, cb) -> {
                    rounds.incrementAndGet();
                    assertEquals("192.168.4.1", cb.gatewayToward());
                    return HostLossWatch.GOODBYE_UNKNOWN;
                },
                InetAddress.getByName("192.168.4.69"), 47984);
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch.afterGoodbye());
        assertEquals(1, rounds.get());
    }

    @Test public void aSilenceIsNeverChecked() throws Exception {
        AtomicInteger calls = new AtomicInteger();
        HostLossWatch watch = new HostLossWatch(
                addr -> "192.168.4.1",
                () -> false,
                (handle, host, port, cb) -> {
                    calls.incrementAndGet();
                    return HostLossWatch.GOODBYE_GONE;
                },
                InetAddress.getByName("192.168.4.69"), 47984);
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch.afterGoodbye());
        assertEquals(0, calls.get());
    }

    @Test public void aPcOffThisLinkIsTheOldBehaviour() throws Exception {
        HostLossWatch watch = new HostLossWatch(
                addr -> null,
                () -> true,
                NightjarJni::hostLossWatchGoodbye,
                InetAddress.getByName("127.0.0.1"), 47984);
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch.afterGoodbye());
    }

    @Test public void aLinkThatGoesAwayMidWayIsTheOldBehaviour() throws Exception {
        try (ServerSocket listener = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            AtomicInteger checks = new AtomicInteger();
            HostLossWatch watch = new HostLossWatch(
                    addr -> checks.getAndIncrement() == 0 ? "127.0.0.2" : null,
                    () -> true,
                    NightjarJni::hostLossWatchGoodbye,
                    InetAddress.getByName("127.0.0.1"), listener.getLocalPort());
            assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch.afterGoodbye());
            assertEquals(2, checks.get());
        }
    }

    @Test public void aStopEndsTheWaitWithTheOldBehaviour() throws Exception {
        try (ServerSocket listener = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            final HostLossWatch[] ref = new HostLossWatch[1];
            AtomicInteger checks = new AtomicInteger();
            HostLossWatch watch = new HostLossWatch(
                    addr -> {
                        if (checks.incrementAndGet() == 1 && ref[0] != null) {
                            ref[0].stop();
                        }
                        return "127.0.0.2";
                    },
                    () -> true,
                    NightjarJni::hostLossWatchGoodbye,
                    InetAddress.getByName("127.0.0.1"), listener.getLocalPort());
            ref[0] = watch;
            assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch.afterGoodbye());
            assertEquals(1, checks.get());
        }

        AtomicInteger calls = new AtomicInteger();
        HostLossWatch stopped = new HostLossWatch(
                addr -> "127.0.0.2",
                () -> true,
                (handle, host, port, cb) -> {
                    calls.incrementAndGet();
                    return HostLossWatch.GOODBYE_GONE;
                },
                InetAddress.getByName("127.0.0.1"), 47984);
        stopped.stop();
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, stopped.afterGoodbye());
        assertEquals(0, calls.get());
    }

    @Test public void theStreamsAddressIsUsedAsIsAndNeverLookedUp() throws Exception {
        List<String> seen = new ArrayList<>();
        HostLossWatch watch = new HostLossWatch(
                addr -> "192.168.4.1",
                () -> true,
                (handle, host, port, cb) -> {
                    seen.add(host);
                    return HostLossWatch.GOODBYE_GONE;
                },
                InetAddress.getByName("192.168.4.69"), 47984);
        watch.afterGoodbye();
        assertEquals(Arrays.asList("192.168.4.69"), seen);
        assertEquals(InetAddress.getByName("192.168.4.69"), HostLossWatch.parse("192.168.4.69"));
        assertEquals(InetAddress.getByName("fe80::1"), HostLossWatch.parse("fe80::1"));
        assertNull("a name would need DNS", HostLossWatch.parse("my-pc.local"));
        assertNull(HostLossWatch.parse("deadbeef"));
        assertNull(HostLossWatch.parse(""));
        assertNull(HostLossWatch.parse(null));
    }

    private static RouteInfo route(String prefix, int length, String gateway) throws Exception {
        IpPrefix ipPrefix = ReflectionHelpers.callConstructor(IpPrefix.class,
                ClassParameter.from(InetAddress.class, InetAddress.getByName(prefix)), ClassParameter.from(int.class, length));
        return ReflectionHelpers.callConstructor(RouteInfo.class, ClassParameter.from(IpPrefix.class, ipPrefix),
                ClassParameter.from(InetAddress.class, gateway != null ? InetAddress.getByName(gateway) : InetAddress.getByName("0.0.0.0")),
                ClassParameter.from(String.class, "wlan0"));
    }

    @Test public void theGatewayCountsOnlyForAPcOnThisLink() throws Exception {
        List<RouteInfo> wifi = Arrays.asList(route("192.168.4.0", 24, null), route("0.0.0.0", 0, "192.168.4.1"));
        assertEquals("192.168.4.1", HostLossWatch.gatewayToward(wifi, InetAddress.getByName("192.168.4.69")));
        assertNull("behind a router", HostLossWatch.gatewayToward(wifi, InetAddress.getByName("10.0.0.5")));
        assertNull("a link without a gateway proves nothing",
                HostLossWatch.gatewayToward(Arrays.asList(route("192.168.4.0", 24, null)), InetAddress.getByName("192.168.4.69")));
        assertNull("only a route through a router", HostLossWatch.gatewayToward(
                Arrays.asList(route("192.168.4.0", 24, "192.168.1.1"), route("0.0.0.0", 0, "192.168.1.1")), InetAddress.getByName("192.168.4.69")));
    }

    @Test public void aGonePcEndsWithoutRecoveryOrAnotherLook() {
        int gone = MoonBridge.CLIENT_ERROR_HOST_GONE;
        assertFalse(NightjarPolicyJni.recoveryRecoversTermination(gone, true));
        assertFalse(NightjarPolicyJni.recoveryTransportTermination(gone));
        assertFalse(NightjarPolicyJni.exitHostEndedStream(gone));
        assertFalse("back to the list with a message, nothing to act on",
                NightjarPolicyJni.exitTerminationNeedsAction(gone, false, false, false));
    }
}
