package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import android.app.Application;
import android.content.Context;

import org.robolectric.RuntimeEnvironment;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.NightjarJni;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/**
 * {@link InternetPlay} backed directly by {@code libnightjar_core} via {@link NightjarJni#internetCall}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class InternetPlayTest {
    private static final String REPLY = "{\"version\":1,\"enabled\":true,\"portMapping\":true,"
            + "\"externalIpv4\":\"203.0.113.5\",\"mappedPorts\":true,\"ipv6\":[\"2001:db8::5\"],"
            + "\"ports\":{\"http\":47989,\"https\":47984,\"rtsp\":48010,\"video\":47998,"
            + "\"audio\":48000,\"control\":47999,\"mic\":48002}}";
    private static final String REPLY_DISABLED = "{\"version\":1,\"enabled\":false,\"portMapping\":false,"
            + "\"externalIpv4\":null,\"mappedPorts\":false,\"ipv6\":[],"
            + "\"ports\":{\"http\":47989,\"https\":47984,\"rtsp\":48010,\"video\":47998,"
            + "\"audio\":48000,\"control\":47999,\"mic\":48002}}";

    @Test
    public void theInternetAddressesComeAfterThePcsOwnInOrder() throws Exception {
        InternetPlay.Addresses addresses = InternetPlay.addresses(REPLY);
        assertEquals(new ComputerDetails.AddressTuple("2001:db8::5", 47989), addresses.ipv6.get(0));
        assertEquals(new ComputerDetails.AddressTuple("203.0.113.5", 47989), addresses.mapped);
        // A PC that never gave a reply has no internet addresses, and nothing is asked.
        NightjarJni.forceUnavailableForTest(new AssertionError("asked"));
        try {
            assertTrue(InternetPlay.addresses(null).ipv6.isEmpty());
        } finally {
            NightjarJni.forceUnavailableForTest(null);
        }
    }

    @Test
    public void connectGivesTheTunnelsLoopbackEndOrTheWordsForEveryPath() throws Exception {
        InternetPlay.Tried tried = new InternetPlay.Tried();
        tried.lan = true;
        tried.mapped = true;
        assertTrue(InternetPlay.isTunnelAddress("127.78.10.20"));
        try {
            InternetPlay.connect("uuid", REPLY_DISABLED, tried);
            fail("no tunnel came up");
        } catch (InternetPlay.Unreachable e) {
            assertTrue(e.getMessage(), e.getMessage().startsWith("The PC could not be reached."));
            assertTrue(e.getMessage(), e.getMessage().contains("local network"));
        }
    }

    @Test
    public void thePathInUseIsNamedAndTheLocalNetworkNeedsNoWords() throws Exception {
        Context context = RuntimeEnvironment.getApplication();
        ComputerDetails details = new ComputerDetails();
        details.localAddress = new ComputerDetails.AddressTuple("192.168.1.10", 47989);
        details.remoteAddress = new ComputerDetails.AddressTuple("203.0.113.5", 47989);
        details.activeAddress = details.remoteAddress;
        // Without a reply naming it, the remote address is still a forwarded port.
        assertEquals("mapped", InternetPlay.path(details, null));
        details.activeAddress = details.localAddress;
        assertEquals("lan", InternetPlay.path(details, REPLY));
        details.activeAddress = new ComputerDetails.AddressTuple("127.78.10.20", 47989);
        assertEquals("punched", InternetPlay.path(details, REPLY));
        assertEquals("Internet (forwarded port)", InternetPlay.pathLabel(context, "mapped"));
        assertEquals("Internet (direct path)", InternetPlay.pathLabel(context, "punched"));
        assertNull(InternetPlay.pathLabel(context, "lan"));
        assertFalse(InternetPlay.isTunnelAddress("127.0.0.1"));
    }

    @Test
    public void theKeptReplyIsPerPcAndGoesWithIt() {
        Context context = RuntimeEnvironment.getApplication();
        InternetPlay.save(context, "a", REPLY);
        assertEquals(REPLY, InternetPlay.reply(context, "a"));
        assertNull(InternetPlay.reply(context, "b"));
        InternetPlay.forget(context, "a");
        assertNull(InternetPlay.reply(context, "a"));
        assertTrue(InternetPlay.due(null, 0));
        assertFalse(InternetPlay.due(0L, InternetPlay.READ_INTERVAL_MS - 1));
        assertTrue(InternetPlay.due(0L, InternetPlay.READ_INTERVAL_MS));
    }
}
