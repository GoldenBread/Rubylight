package com.limelight;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * The calls of the launch traces in plain values, as nightjar-policy's launch module takes them.
 * Roles and controls are lower-case NvApp names. Input-only takes the host's remote session tiles
 * and pairing as booleans: each call runs on every fixture or pair state that stands for the value
 * and fails unless they all agree.
 */
final class LaunchTrace {
    private LaunchTrace() {}

    static boolean resumesOnPcTap(boolean quickResume, boolean gameRunning) {
        return LaunchPolicy.resumesOnPcTap(quickResume, gameRunning);
    }

    static boolean offersQuitOnLeave(boolean canLaunch, String control, String role) {
        return LaunchPolicy.offersQuitOnLeave(canLaunch,
                NvApp.Control.valueOf(control.toUpperCase(Locale.ROOT)), role(role));
    }

    static boolean quitsOnLeave(boolean quitOnLeaveChosen, String role, boolean connected, boolean secondScreen) {
        return NightjarPolicyJni.exitQuitsOnLeave(quitOnLeaveChosen, role(role).ordinal(), connected, secondScreen);
    }

    static boolean terminationNeedsAction(int errorCode, boolean portTestBlocked,
                                          boolean networkGraceGaveUp, boolean recoveryExhausted) {
        return NightjarPolicyJni.exitTerminationNeedsAction(errorCode, portTestBlocked, networkGraceGaveUp,
                recoveryExhausted);
    }

    static String mode(boolean anyHostEnabled, boolean remoteSessionTiles, boolean hostListsInputOnly) {
        List<HostCapabilities> hosts = new ArrayList<>();
        String[] fixtures = remoteSessionTiles
                ? new String[] {"rubylight-serverinfo.xml", "vibepollo-serverinfo.xml", "butterpollo-serverinfo.xml"}
                : new String[] {"sunshine-serverinfo.xml", "gamestream-serverinfo.xml", "apollo-serverinfo.xml",
                        "apollo-input-only-serverinfo.xml"};
        for (String fixture : fixtures) hosts.add(TestHosts.fixture(fixture));
        if (!remoteSessionTiles) hosts.add(HostCapabilities.UNKNOWN);
        Map<Object, String> modes = new LinkedHashMap<>();
        for (HostCapabilities caps : hosts) {
            if (caps.has(HostCapabilities.Feature.REMOTE_SESSION_TILES) != remoteSessionTiles) {
                throw new AssertionError(caps + " does not stand for remote session tiles " + remoteSessionTiles);
            }
            int mode = NightjarPolicyJni.inputOnlyMode(
                    anyHostEnabled, caps.has(HostCapabilities.Feature.REMOTE_SESSION_TILES), hostListsInputOnly);
            modes.put(caps, mode == NightjarPolicyJni.INPUT_ONLY_MODE_NATIVE ? "native"
                    : mode == NightjarPolicyJni.INPUT_ONLY_MODE_SMALL_STREAM ? "small_stream" : "off");
        }
        return agreed(modes);
    }

    static String smallStreamBlocker(boolean paired, long permission) {
        List<PairingManager.PairState> states = new ArrayList<>();
        for (PairingManager.PairState state : PairingManager.PairState.values()) {
            if ((state == PairingManager.PairState.PAIRED) == paired) states.add(state);
        }
        // A PC this device never asked has no pair state.
        if (!paired) states.add(null);
        Map<Object, String> blockers = new LinkedHashMap<>();
        for (PairingManager.PairState state : states) {
            ComputerDetails pc = new ComputerDetails();
            pc.pairState = state;
            pc.permission = permission;
            blockers.put(state, blocker(NightjarPolicyJni.inputOnlySmallStreamBlocker(
                    pc.pairState == PairingManager.PairState.PAIRED, pc.permission)));
        }
        return agreed(blockers);
    }

    static boolean keepsRunningGame(boolean hostRunningGame, boolean thisGameRunning) {
        return NightjarPolicyJni.inputOnlyKeepsRunningGame(hostRunningGame, thisGameRunning);
    }

    static int estimatedKbps() {
        return NightjarPolicyJni.inputOnlyEstimatedKbps();
    }

    static int estimatedMegabytesPerHour() {
        return NightjarPolicyJni.inputOnlyEstimatedMegabytesPerHour();
    }

    private static NvApp.Role role(String role) {
        return NvApp.Role.valueOf(role.toUpperCase(Locale.ROOT));
    }

    private static String blocker(int code) {
        if (code == NightjarPolicyJni.INPUT_ONLY_BLOCKER_NONE) return null;
        if (code == NightjarPolicyJni.INPUT_ONLY_BLOCKER_PAIR) return "pair";
        if (code == NightjarPolicyJni.INPUT_ONLY_BLOCKER_VIEW_PERMISSION) return "view_permission";
        if (code == NightjarPolicyJni.INPUT_ONLY_BLOCKER_INPUT_PERMISSION) return "input_permission";
        throw new AssertionError("unknown blocker code " + code);
    }

    /** The one result every input gave; null results count too. */
    private static String agreed(Map<Object, String> results) {
        if (new HashSet<>(results.values()).size() != 1) throw new AssertionError("inputs disagree: " + results);
        return results.values().iterator().next();
    }
}
