package com.limelight;

import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.junit.Test;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import static org.junit.Assert.*;

public class StreamExitPolicyTest {
    @Test public void leavingQuitsOnlyAGameThePlayerChoseToQuit() {
        assertTrue(NightjarPolicyJni.exitQuitsOnLeave(true, NvApp.Role.STREAM.ordinal(), true, false));
        assertFalse(NightjarPolicyJni.exitQuitsOnLeave(false, NvApp.Role.STREAM.ordinal(), true, false));
    }

    @Test public void aStreamThatIsNoLongerConnectedLeavesWithoutQuitting() {
        assertFalse(NightjarPolicyJni.exitQuitsOnLeave(true, NvApp.Role.STREAM.ordinal(), false, false));
    }

    @Test public void remoteMonitorAndInputSessionsAreNeverQuitOnLeave() {
        assertFalse(NightjarPolicyJni.exitQuitsOnLeave(true, NvApp.Role.REMOTE_MONITOR.ordinal(), true, false));
        assertFalse(NightjarPolicyJni.exitQuitsOnLeave(true, NvApp.Role.INPUT_ONLY.ordinal(), true, false));
    }

    @Test public void leavingTheSecondScreenNeverQuitsTheGameOfTheFirst() {
        assertFalse(NightjarPolicyJni.exitQuitsOnLeave(true, NvApp.Role.STREAM.ordinal(), true, true));
    }

    @Test public void theHostEndingTheStreamNeverNeedsADialog() {
        assertFalse(NightjarPolicyJni.exitTerminationNeedsAction(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, false, false, false));
        assertFalse(NightjarPolicyJni.exitTerminationNeedsAction(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, true, false, false));
    }

    @Test public void aLostConnectionOrAHostThatWentAwayReturnsToTheList() {
        for (int code : new int[] {-1, 32, 54, 104, 110, 111, 113, 10053, 10054, 10060}) {
            assertFalse("code " + code, NightjarPolicyJni.exitTerminationNeedsAction(code, false, false, false));
        }
    }

    @Test public void aReasonOnlyThePcKnowsReturnsToTheList() {
        for (int code : new int[] {503, 404, 0x80030020, 0xc0000005}) {
            assertFalse("code " + code, NightjarPolicyJni.exitTerminationNeedsAction(code, false, false, false));
        }
    }

    @Test public void errorsWithAdviceKeepTheDialog() {
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC, false, false, false));
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(MoonBridge.ML_ERROR_NO_VIDEO_FRAME, false, false, false));
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION, false, false, false));
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(MoonBridge.ML_ERROR_PROTECTED_CONTENT, false, false, false));
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(MoonBridge.ML_ERROR_FRAME_CONVERSION, false, false, false));
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(MoonBridge.CLIENT_ERROR_DECODER, false, false, false));
    }

    @Test public void aDecoderGivenUpWithVulkanOnNamesTheSettingInEveryLanguage() throws Exception {
        File[] dirs = new File("src/main/res").listFiles((dir, name) ->
                name.matches("values(-[a-z]{2}(-r[A-Z]{2})?)?") && new File(dir, name + "/strings.xml").exists());
        assertNotNull(dirs);
        assertEquals(15, dirs.length);
        for (File dir : dirs) {
            String strings = new String(Files.readAllBytes(new File(dir, "strings.xml").toPath()), StandardCharsets.UTF_8);
            String title = string(strings, "title_vulkan_render");
            String message = string(strings, "decoder_failed_vulkan");
            assertNotNull(dir.getName(), title);
            assertNotNull(dir.getName(), message);
            assertTrue(dir.getName() + ": " + message, message.contains(title));
            assertFalse(dir.getName(), string(strings, "decoder_failed").contains(title));
        }
    }

    private static String string(String xml, String name) {
        Matcher m = Pattern.compile("<string name=\"" + name + "\"[^>]*>([^<]*)</string>").matcher(xml);
        return m.find() ? m.group(1) : null;
    }

    @Test public void onlyTerminationsThePcSentAskItForTheReason() {
        for (int code : new int[] {0x80004005, 0x80030020, 0xc0000005,
                MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION, MoonBridge.ML_ERROR_PROTECTED_CONTENT,
                MoonBridge.ML_ERROR_FRAME_CONVERSION}) {
            assertTrue("code " + Integer.toHexString(code), NightjarPolicyJni.exitHostEndedStream(code));
        }
        for (int code : new int[] {MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, -1, 32, 110, 10054, 503,
                MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC, MoonBridge.ML_ERROR_NO_VIDEO_FRAME,
                MoonBridge.CLIENT_ERROR_VULKAN_RENDERER, MoonBridge.CLIENT_ERROR_DECODER}) {
            assertFalse("code " + code, NightjarPolicyJni.exitHostEndedStream(code));
        }
    }

    @Test public void aStopBeforeTheFirstPictureNoLongerBlamesProtectedVideosInAnyLanguage() throws Exception {
        File[] dirs = new File("src/main/res").listFiles((dir, name) ->
                name.matches("values(-[a-z]{2}(-r[A-Z]{2})?)?") && new File(dir, name + "/strings.xml").exists());
        assertNotNull(dirs);
        assertEquals(15, dirs.length);
        for (File dir : dirs) {
            String strings = new String(Files.readAllBytes(new File(dir, "strings.xml").toPath()), StandardCharsets.UTF_8);
            String early = string(strings, "early_stop_error");
            String reason = string(strings, "host_failure_reason");
            assertNotNull(dir.getName(), early);
            assertNotEquals(dir.getName(), string(strings, "early_termination_error"), early);
            assertNotNull(dir.getName(), reason);
            assertTrue(dir.getName() + ": " + reason, reason.contains("%1$s"));
        }
    }

    @Test public void aBlockedPortTestKeepsTheDialogForAnyError() {
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(-1, true, false, false));
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(110, true, false, false));
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(503, true, false, false));
    }

    @Test public void aGraceTimeThatMayHaveRunOutKeepsTheDialogForItsAdvice() {
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(-1, false, true, false));
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(110, false, true, false));
        assertFalse(NightjarPolicyJni.exitTerminationNeedsAction(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, false, true, true));
    }

    @Test public void usedUpAutomaticReconnectingKeepsTheDialogForItsReconnectButton() {
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(-1, false, false, true));
        assertTrue(NightjarPolicyJni.exitTerminationNeedsAction(110, false, false, true));
        assertFalse(NightjarPolicyJni.exitTerminationNeedsAction(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, false, false, true));
    }

    @Test public void onlyAStartRefusedInTheHandshakeAsksThePcWhy() {
        assertTrue(NightjarPolicyJni.exitRefusedInHandshake("RTSP handshake"));
        assertFalse(NightjarPolicyJni.exitRefusedInHandshake("video stream establishment"));
        assertFalse(NightjarPolicyJni.exitRefusedInHandshake(null));
    }
}
