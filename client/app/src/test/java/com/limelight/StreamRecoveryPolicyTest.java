package com.limelight;

import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.junit.Test;
import static org.junit.Assert.*;

public class StreamRecoveryPolicyTest {
    private static long nextDelay(int[] state, long now, long connectedAt) {
        long[] res = NightjarPolicyJni.recoveryNextDelay(state[0], state[1], now, connectedAt);
        state[0] = (int) res[1];
        return res[0];
    }

    @Test public void increasingDelaysAndBudgetSurviveActivityReplacement() {
        int attempts = 0;
        long startedAt = 0;
        long now = 10000;
        for (long delay : new long[] {1000, 2000, 4000, 8000, 12000}) {
            long[] res = NightjarPolicyJni.recoveryNextDelay(attempts, startedAt, now, -1);
            assertEquals(delay, res[0]);
            now += delay;
            attempts = (int) res[1];
            startedAt = res[2];
        }
        long[] end = NightjarPolicyJni.recoveryNextDelay(attempts, startedAt, now, -1);
        assertEquals(-1, end[0]);
        assertEquals(10000, end[2]);
    }

    @Test public void slowStartupAndFlappingConnectionsCannotExtendTheWindow() {
        assertEquals(-1, NightjarPolicyJni.recoveryNextDelay(3, 1000, 59000, 58000)[0]);
        assertEquals(-1, NightjarPolicyJni.recoveryNextDelay(5, 1000, 25000, 20000)[0]);
    }

    @Test public void stableConnectionReceivesANewBudget() {
        long[] res = NightjarPolicyJni.recoveryNextDelay(5, 1000, 70000, 40000);
        assertEquals(1000, res[0]);
        assertEquals(1, res[1]);
        assertEquals(70000, res[2]);
    }

    @Test public void aStableSessionResetsAtStopWhileTimeLockedDoesNotCountAsStability() {
        long[] stable = NightjarPolicyJni.recoveryResetAfterStableConnection(5, 1000, 40000, 10000);
        long[] stableNext = NightjarPolicyJni.recoveryNextDelay((int) stable[0], stable[1], 200000, -1);
        assertEquals(1000, stableNext[0]);
        assertEquals(200000, stableNext[2]);

        long[] flapping = NightjarPolicyJni.recoveryResetAfterStableConnection(4, 1000, 25000, 20000);
        long[] flappingNext = NightjarPolicyJni.recoveryNextDelay((int) flapping[0], flapping[1], 200000, -1);
        assertEquals(-1, flappingNext[0]);
        assertEquals(4, flappingNext[1]);
        assertEquals(1000, flappingNext[2]);
    }

    @Test public void aUserTripSpendsNothingAndPausesARunningWindow() {
        long[] fresh = NightjarPolicyJni.recoveryAfterUserTrip(0, 0, 20000, 50000);
        assertEquals(0, fresh[0]);
        assertEquals(0, fresh[1]);
        assertEquals(1000, NightjarPolicyJni.recoveryNextDelay((int) fresh[0], fresh[1], 60000, -1)[0]);

        long[] paused = NightjarPolicyJni.recoveryAfterUserTrip(3, 1000, 20000, 50000);
        assertEquals(3, paused[0]);
        assertEquals(31000, paused[1]);
        assertEquals(8000, NightjarPolicyJni.recoveryNextDelay((int) paused[0], paused[1], 70000, -1)[0]);
        long[] exhausted = NightjarPolicyJni.recoveryAfterUserTrip(4, 1000, 20000, 50000);
        assertEquals(-1, NightjarPolicyJni.recoveryNextDelay((int) exhausted[0], exhausted[1], 80000, -1)[0]);
        assertEquals(1000, NightjarPolicyJni.recoveryAfterUserTrip(2, 1000, 20000, 10000)[1]);
    }

    @Test public void aFailureLoopAfterAUserTripIsStillRefusedAfterItsMinute() {
        long[] state = NightjarPolicyJni.recoveryAfterUserTrip(0, 0, 10000, 30000);
        int attempts = (int) state[0];
        long startedAt = state[1];
        long now = 40000;
        int spent = 0;
        while (true) {
            long[] res = NightjarPolicyJni.recoveryNextDelay(attempts, startedAt, now, -1);
            if (res[0] < 0) break;
            spent++;
            now += res[0] + 10000;
            attempts = (int) res[1];
            startedAt = res[2];
        }
        assertEquals(4, spent);
        assertTrue(now - 40000 < NightjarPolicyJni.recoveryWindowMs());
    }

    @Test public void gracefulPermanentAndUnknownErrorsDoNotRetry() {
        for (int code : new int[] {0, -103, -104, -105, 401, 403, 409, 470, 525, -999, 123456}) {
            assertFalse("code " + code, NightjarPolicyJni.recoveryTransientTermination(code));
        }
        for (int code : new int[] {-100, -101, -102, -1, 104, 110, 10054, 10060}) {
            assertTrue("code " + code, NightjarPolicyJni.recoveryTransientTermination(code));
        }
    }

    @Test public void noReconnectBeforeTheFirstVideo() {
        for (int code : new int[] {-100, -101, -102, -1, 10054}) {
            assertFalse("code " + code, NightjarPolicyJni.recoveryRecoversTermination(code, false));
            assertTrue("code " + code, NightjarPolicyJni.recoveryRecoversTermination(code, true));
        }
        for (int code : new int[] {0, -103, 403}) {
            assertFalse("code " + code, NightjarPolicyJni.recoveryRecoversTermination(code, true));
        }
    }

    @Test public void startupRejectsDecoderAndHostErrorsEvenWithPortFlags() {
        assertTrue(NightjarPolicyJni.recoveryTransientStartup("Control stream", 4, -1));
        assertTrue(NightjarPolicyJni.recoveryTransientStartup("Game", 3, 0));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("Video stream", 256, -1));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("Game", 3, 403));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("Game", 0, 0));
    }

    @Test public void nativeStartupTransportErrorsRetryWithoutTreatingHostStatusAsTransport() {
        String[] stages = {"Control stream establishment", "RTSP handshake"};
        int[] portFlags = {0x0200, 0x0c04};
        for (int i = 0; i < stages.length; i++) {
            for (int error : new int[] {32, 101, 104, 110, 111, 113}) {
                assertTrue(stages[i] + " errno " + error,
                        NightjarPolicyJni.recoveryTransientStartup(stages[i], portFlags[i], error));
            }
            for (int error : new int[] {12, 22, 401, 403, 408, 409, 470, 500, 503, 525}) {
                assertFalse(stages[i] + " error " + error,
                        NightjarPolicyJni.recoveryTransientStartup(stages[i], portFlags[i], error));
            }
        }
    }

    @Test public void startupRetriesTheStagesBothAppsRetry() {
        for (String stage : new String[] {"Name resolution", "Control stream establishment", "RTSP handshake"}) {
            assertTrue(stage, NightjarPolicyJni.recoveryTransientStartup(stage, 0, 110));
            assertTrue(stage, NightjarPolicyJni.recoveryTransientStartup(stage, 0, -1));
            assertFalse(stage, NightjarPolicyJni.recoveryTransientStartup(stage, 0, 403));
        }
        assertTrue(NightjarPolicyJni.recoveryTransientStartup("Name Resolution", 0, 113));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("audio stream establishment", 0, -1));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("audio stream initialization", 0, 110));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("video stream establishment", 0, 110));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("platform initialization", 0, -1));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("A game called name resolution", 0, 0));
    }

    @Test public void javaTransportFailuresDoNotInterpretTheGameTitleAsADecoderStage() {
        assertTrue(NightjarPolicyJni.recoveryTransientStartup("Video Game", 0x0003, 0));
        assertTrue(NightjarPolicyJni.recoveryTransientStartup("VIDEO", 0x0003, 0));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("Video stream", 0x0100, -1));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("Video Game", 0, 0));
        assertFalse(NightjarPolicyJni.recoveryTransientStartup("Video Game", 0x0003, 403));
    }

    @Test public void invalidPersistedStateCannotOverflowOrRunAfterAClockReset() {
        assertEquals(-1, NightjarPolicyJni.recoveryNextDelay(Integer.MAX_VALUE, 1000, 2000, -1)[0]);
        assertEquals(-1, NightjarPolicyJni.recoveryNextDelay(1, 90000, 1000, -1)[0]);
    }
}
