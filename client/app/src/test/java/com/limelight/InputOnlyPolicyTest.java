package com.limelight;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.junit.Test;

import static org.junit.Assert.*;

public class InputOnlyPolicyTest {
    private static ComputerDetails pc(PairingManager.PairState pairState, long permission) {
        ComputerDetails pc = new ComputerDetails();
        pc.pairState = pairState;
        pc.permission = permission;
        return pc;
    }

    private static int mode(boolean anyHostEnabled, HostCapabilities caps, boolean hostListsInputOnly) {
        return NightjarPolicyJni.inputOnlyMode(
                anyHostEnabled, caps.has(HostCapabilities.Feature.REMOTE_SESSION_TILES), hostListsInputOnly);
    }

    private static int blocker(ComputerDetails pc) {
        return NightjarPolicyJni.inputOnlySmallStreamBlocker(
                pc.pairState == PairingManager.PairState.PAIRED, pc.permission);
    }

    @Test public void hostsWithTheirOwnInputOnlyModeKeepIt() {
        for (String host : new String[] {"rubylight-serverinfo.xml", "vibepollo-serverinfo.xml", "butterpollo-serverinfo.xml"}) {
            HostCapabilities caps = TestHosts.fixture(host);
            assertEquals(host, NightjarPolicyJni.INPUT_ONLY_MODE_NATIVE, mode(true, caps, false));
            assertEquals(host, NightjarPolicyJni.INPUT_ONLY_MODE_NATIVE, mode(false, caps, false));
        }
    }

    @Test public void aListedInputOnlyAppWinsOnAnyHost() {
        HostCapabilities apollo = TestHosts.fixture("apollo-input-only-serverinfo.xml");
        assertEquals(NightjarPolicyJni.INPUT_ONLY_MODE_NATIVE, mode(true, apollo, true));
        assertEquals(NightjarPolicyJni.INPUT_ONLY_MODE_NATIVE, mode(true, HostCapabilities.UNKNOWN, true));
    }

    @Test public void hostsWithoutTheModeGetTheSmallStreamOnlyWhenTheSettingIsOn() {
        for (String host : new String[] {"sunshine-serverinfo.xml", "gamestream-serverinfo.xml", "apollo-serverinfo.xml"}) {
            HostCapabilities caps = TestHosts.fixture(host);
            assertEquals(host, NightjarPolicyJni.INPUT_ONLY_MODE_SMALL_STREAM, mode(true, caps, false));
            assertEquals(host, NightjarPolicyJni.INPUT_ONLY_MODE_OFF, mode(false, caps, false));
        }
        assertEquals(NightjarPolicyJni.INPUT_ONLY_MODE_SMALL_STREAM, mode(true, HostCapabilities.UNKNOWN, false));
        assertEquals(NightjarPolicyJni.INPUT_ONLY_MODE_OFF, mode(false, HostCapabilities.UNKNOWN, false));
    }

    @Test public void anUnpairedPcMustPairFirst() {
        assertEquals(NightjarPolicyJni.INPUT_ONLY_BLOCKER_PAIR,
                blocker(pc(PairingManager.PairState.NOT_PAIRED, -1)));
        assertEquals(NightjarPolicyJni.INPUT_ONLY_BLOCKER_PAIR,
                blocker(pc(null, -1)));
    }

    @Test public void missingPermissionsSayWhichOne() {
        assertEquals(NightjarPolicyJni.INPUT_ONLY_BLOCKER_VIEW_PERMISSION, blocker(
                pc(PairingManager.PairState.PAIRED, ComputerDetails.PERMISSION_INPUT | ComputerDetails.PERMISSION_LIST)));
        assertEquals(NightjarPolicyJni.INPUT_ONLY_BLOCKER_INPUT_PERMISSION, blocker(
                pc(PairingManager.PairState.PAIRED, ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH)));
        assertEquals(NightjarPolicyJni.INPUT_ONLY_BLOCKER_NONE, blocker(
                pc(PairingManager.PairState.PAIRED, ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_CONTROLLER)));
    }

    @Test public void aPairedHostWithoutPermissionsCanStart() {
        assertEquals(NightjarPolicyJni.INPUT_ONLY_BLOCKER_NONE, blocker(pc(PairingManager.PairState.PAIRED, -1)));
    }

    @Test public void theSmallStreamNeverReplacesARunningGame() {
        assertTrue(NightjarPolicyJni.inputOnlyKeepsRunningGame(false, false));
        assertTrue(NightjarPolicyJni.inputOnlyKeepsRunningGame(true, true));
        assertFalse(NightjarPolicyJni.inputOnlyKeepsRunningGame(true, false));
    }

    @Test public void theEstimateCountsErrorCorrectionAndAudio() {
        assertEquals(696, NightjarPolicyJni.inputOnlyEstimatedKbps());
        assertEquals(313, NightjarPolicyJni.inputOnlyEstimatedMegabytesPerHour());
        assertTrue(NightjarPolicyJni.inputOnlyEstimatedKbps() < 1000);
    }
}
