package com.limelight.nvstream;

import static org.junit.Assert.assertEquals;

import java.util.ArrayList;
import java.util.List;
import java.util.function.LongSupplier;

import org.junit.Test;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;

/** Client roaming: verifies {@code nightjar_policy::roaming::RoamingTracker} across JNI. */
public class RoamingNetworkTrackerTest {
    private static final long WIFI = 1L;
    private static final long OTHER_WIFI = 2L;
    private static final long CELLULAR = 3L;

    interface Reporter {
        int networkChanged(boolean metered);
        default void streamNetworkLost() {}
        default void streamNetworkReturned(int change, boolean metered) {}
    }

    private static final class JniTracker implements AutoCloseable {
        private final long handle;
        private final Reporter reporter;
        private final LongSupplier clock;

        JniTracker(Long streamNetwork, Reporter reporter) {
            this(streamNetwork, reporter, () -> System.nanoTime() / 1_000_000L);
        }

        JniTracker(Long streamNetwork, Reporter reporter, LongSupplier clock) {
            this.handle = NightjarPolicyJni.roamingCreate(streamNetwork != null, streamNetwork != null ? streamNetwork : 0L);
            this.reporter = reporter;
            this.clock = clock;
        }

        void capabilities(long network, boolean notMetered) {
            int step = NightjarPolicyJni.roamingOnCapabilities(handle, network, notMetered);
            if (step == NightjarPolicyJni.ROAMING_CAP_IGNORE) return;
            boolean metered = step == NightjarPolicyJni.ROAMING_CAP_ASK_METERED
                    || step == NightjarPolicyJni.ROAMING_CAP_RETURNED_ASK_METERED;
            boolean returned = step == NightjarPolicyJni.ROAMING_CAP_RETURNED_ASK_NOT_METERED
                    || step == NightjarPolicyJni.ROAMING_CAP_RETURNED_ASK_METERED;
            int change = reporter.networkChanged(metered);
            if (NightjarPolicyJni.roamingRecordChange(handle, network, metered, returned, change, clock.getAsLong())) {
                reporter.streamNetworkReturned(change, metered);
            }
        }

        void addresses(long network, String addresses) {
            addresses(network, addresses, false);
        }

        void addresses(long network, String addresses, boolean nat64) {
            int step = NightjarPolicyJni.roamingOnAddresses(handle, network, addresses, nat64, clock.getAsLong());
            if (step == NightjarPolicyJni.ROAMING_ADDR_IGNORE) return;
            boolean metered = step == NightjarPolicyJni.ROAMING_ADDR_ASK_METERED;
            int change = reporter.networkChanged(metered);
            if (NightjarPolicyJni.roamingRecordAddressChange(handle, change)) {
                reporter.streamNetworkReturned(change, metered);
            }
        }

        void lost(long network) {
            if (NightjarPolicyJni.roamingOnLost(handle, network)) {
                reporter.streamNetworkLost();
            }
        }

        void stop() {
            NightjarPolicyJni.roamingStop(handle);
        }

        @Override public void close() {
            NightjarPolicyJni.roamingDestroy(handle);
        }
    }

    private static class Events implements Reporter {
        final List<String> seen = new ArrayList<>();
        int answer = MoonBridge.NETWORK_CARRIES_ON;

        @Override public int networkChanged(boolean metered) {
            seen.add(metered ? "changed metered" : "changed");
            return answer;
        }
        @Override public void streamNetworkLost() { seen.add("lost"); }
        @Override public void streamNetworkReturned(int change, boolean metered) {
            seen.add("returned " + change + (metered ? " metered" : ""));
        }
    }

    @Test public void theStreamsOwnNetworkAndItsCapabilityUpdatesAreNoChange() {
        List<Boolean> reports = new ArrayList<>();
        try (JniTracker tracker = new JniTracker(WIFI, m -> { reports.add(m); return MoonBridge.NETWORK_CARRIES_ON; })) {
            tracker.capabilities(WIFI, true);
            tracker.capabilities(WIFI, true);
            tracker.capabilities(WIFI, false);
            assertEquals(new ArrayList<Boolean>(), reports);
        }
    }

    @Test public void aNewDefaultNetworkIsReportedOnceWithWhetherItIsMetered() {
        List<Boolean> reports = new ArrayList<>();
        try (JniTracker tracker = new JniTracker(WIFI, m -> { reports.add(m); return MoonBridge.NETWORK_CARRIES_ON; })) {
            tracker.capabilities(CELLULAR, false);
            tracker.capabilities(CELLULAR, false);
            tracker.capabilities(OTHER_WIFI, true);
            assertEquals(java.util.Arrays.asList(true, false), reports);
        }
    }

    @Test public void theSameNetworkComingBackAfterItWasLostIsAChange() {
        List<Boolean> reports = new ArrayList<>();
        try (JniTracker tracker = new JniTracker(WIFI, m -> { reports.add(m); return MoonBridge.NETWORK_CARRIES_ON; })) {
            tracker.lost(WIFI);
            tracker.capabilities(WIFI, true);
            tracker.capabilities(WIFI, true);
            assertEquals(java.util.Arrays.asList(false), reports);
            tracker.lost(CELLULAR);
            tracker.capabilities(WIFI, true);
            assertEquals(java.util.Arrays.asList(false), reports);
        }
    }

    @Test public void withoutTheStartNetworkTheFirstOneSeenIsTheBaseline() {
        List<Boolean> reports = new ArrayList<>();
        try (JniTracker tracker = new JniTracker(null, m -> { reports.add(m); return MoonBridge.NETWORK_CARRIES_ON; })) {
            tracker.capabilities(WIFI, true);
            assertEquals(new ArrayList<Boolean>(), reports);
            tracker.capabilities(CELLULAR, false);
            assertEquals(java.util.Arrays.asList(true), reports);
        }
    }

    @Test public void aLostNetworkComingBackIsAReturnWithTheEnginesAnswer() {
        Events events = new Events();
        try (JniTracker tracker = new JniTracker(WIFI, events)) {
            tracker.lost(WIFI);
            tracker.lost(WIFI);
            tracker.capabilities(OTHER_WIFI, true);
            tracker.capabilities(OTHER_WIFI, true);
            assertEquals(java.util.Arrays.asList("lost", "changed", "returned " + MoonBridge.NETWORK_CARRIES_ON), events.seen);

            events.seen.clear();
            events.answer = MoonBridge.NETWORK_CANNOT_CONTINUE;
            tracker.lost(OTHER_WIFI);
            tracker.capabilities(CELLULAR, false);
            assertEquals(java.util.Arrays.asList("lost", "changed metered",
                    "returned " + MoonBridge.NETWORK_CANNOT_CONTINUE + " metered"), events.seen);
        }
    }

    @Test public void aReturnWithoutARouteYetIsAskedAgainAsItsAddressesArrive() {
        Events events = new Events();
        long[] now = {1000};
        try (JniTracker tracker = new JniTracker(WIFI, events, () -> now[0])) {
            tracker.lost(WIFI);
            tracker.capabilities(OTHER_WIFI, true);
            tracker.addresses(OTHER_WIFI, "fe80::1");
            tracker.addresses(OTHER_WIFI, "fe80::1");
            tracker.addresses(CELLULAR, "10.0.0.5");
            now[0] += 7000;
            events.answer = MoonBridge.NETWORK_CANNOT_CONTINUE;
            tracker.addresses(OTHER_WIFI, "fe80::1 192.168.1.21");
            tracker.addresses(OTHER_WIFI, "fe80::1 192.168.1.21 2001:db8::21");
            assertEquals(java.util.Arrays.asList("lost", "changed", "returned " + MoonBridge.NETWORK_CARRIES_ON,
                    "changed", "changed", "returned " + MoonBridge.NETWORK_CANNOT_CONTINUE), events.seen);
        }
    }

    @Test public void aReturnIsAskedAgainOnlyUntilTheConnectionsOwnTimeout() {
        Events events = new Events();
        long[] now = {1000};
        try (JniTracker tracker = new JniTracker(WIFI, events, () -> now[0])) {
            tracker.lost(WIFI);
            tracker.capabilities(OTHER_WIFI, false);
            events.seen.clear();
            tracker.capabilities(OTHER_WIFI, true);
            tracker.capabilities(OTHER_WIFI, false);
            tracker.addresses(OTHER_WIFI, "fe80::1");
            assertEquals(java.util.Arrays.asList("changed metered"), events.seen);
            now[0] += NightjarPolicyJni.recoveryPeerTimeoutMs() + 1;
            events.answer = MoonBridge.NETWORK_CANNOT_CONTINUE;
            tracker.addresses(OTHER_WIFI, "fe80::1 192.168.1.21");
            assertEquals(java.util.Arrays.asList("changed metered"), events.seen);
        }
    }

    @Test public void onlyAReturnTheConnectionCarriedOnOverIsAskedAgain() {
        Events events = new Events();
        try (JniTracker tracker = new JniTracker(WIFI, events, () -> 0)) {
            tracker.addresses(WIFI, "192.168.1.20");
            events.answer = MoonBridge.NETWORK_ROAMED;
            tracker.lost(WIFI);
            tracker.capabilities(OTHER_WIFI, true);
            events.seen.clear();
            tracker.addresses(OTHER_WIFI, "192.168.1.21");
            assertEquals(new ArrayList<String>(), events.seen);
            events.answer = MoonBridge.NETWORK_CANNOT_CONTINUE;
            tracker.lost(OTHER_WIFI);
            tracker.capabilities(CELLULAR, true);
            events.seen.clear();
            tracker.addresses(CELLULAR, "10.0.0.5");
            assertEquals(new ArrayList<String>(), events.seen);
            events.answer = MoonBridge.NETWORK_CARRIES_ON;
            tracker.lost(CELLULAR);
            tracker.capabilities(WIFI, true);
            tracker.lost(WIFI);
            events.seen.clear();
            tracker.addresses(WIFI, "192.168.1.22");
            tracker.capabilities(WIFI, true);
            tracker.stop();
            tracker.addresses(WIFI, "192.168.1.23");
            assertEquals(java.util.Arrays.asList("changed", "returned " + MoonBridge.NETWORK_CARRIES_ON), events.seen);
        }
    }

    @Test public void aNat64PrefixFoundAfterTheAddressesAsksAgain() {
        Events events = new Events();
        try (JniTracker tracker = new JniTracker(WIFI, events, () -> 0)) {
            tracker.lost(WIFI);
            tracker.capabilities(OTHER_WIFI, true);
            events.seen.clear();
            tracker.addresses(OTHER_WIFI, "fe80::1", false);
            tracker.addresses(OTHER_WIFI, "fe80::1", false);
            tracker.addresses(OTHER_WIFI, "fe80::1", true);
            assertEquals(java.util.Arrays.asList("changed", "changed"), events.seen);
        }
    }

    @Test public void anEngineThatTellsKeepsItsOwnAnswer() {
        Events events = new Events();
        try (JniTracker tracker = new JniTracker(WIFI, events, () -> 0)) {
            tracker.addresses(WIFI, "192.168.1.20");
            tracker.lost(WIFI);
            tracker.capabilities(CELLULAR, false);
            tracker.addresses(CELLULAR, "10.0.0.5");
            assertEquals(java.util.Arrays.asList("lost", "changed metered",
                    "returned " + MoonBridge.NETWORK_CARRIES_ON + " metered", "changed metered"), events.seen);
        }
    }

    @Test public void aNewNetworkWhileTheStreamsOwnStaysIsNoReturn() {
        Events events = new Events();
        try (JniTracker tracker = new JniTracker(WIFI, events)) {
            tracker.capabilities(OTHER_WIFI, true);
            tracker.lost(WIFI);
            assertEquals(java.util.Arrays.asList("changed"), events.seen);
        }
    }

    @Test public void nothingIsReportedAfterTheStreamEnds() {
        List<Boolean> reports = new ArrayList<>();
        try (JniTracker tracker = new JniTracker(WIFI, m -> { reports.add(m); return MoonBridge.NETWORK_CARRIES_ON; })) {
            tracker.stop();
            tracker.capabilities(CELLULAR, false);
            tracker.lost(WIFI);
            tracker.capabilities(WIFI, true);
            assertEquals(new ArrayList<Boolean>(), reports);
        }
        Events events = new Events();
        try (JniTracker ended = new JniTracker(WIFI, events)) {
            ended.stop();
            ended.lost(WIFI);
            ended.capabilities(OTHER_WIFI, true);
            assertEquals(new ArrayList<String>(), events.seen);
        }
    }
}
