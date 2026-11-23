package com.limelight.nvstream.http;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** The confirmation reads what libnightjar_core's invite_uri.rs writes, and refuses what it would refuse. */
public class InviteLinkTest {
    /** As invite_uri.rs writes one: everything after the #, the secret first. */
    static final String LINK = "rubylight://invite#v=1&k=" + "01".repeat(32) + "&id=0123ABCD-ef&fp=" + "ab".repeat(32)
            + "&name=Wohnzimmer-PC%20%C3%A4&exp=1800000000&perm=controller"
            + "&ports=47989,47984,48010,47998,48000,47999,0&s=" + "07".repeat(16)
            + "&e=203.0.113.5:40000,%5B2001:db8::5%5D:40001&nat=cone&v6=2001:db8::2&ip4=203.0.113.5&mapped=1";

    @Test
    public void aLinkGivesItsNameWhatTheFriendMayDoAndItsEnd() {
        InviteLink invite = InviteLink.parse(LINK);
        assertNotNull(invite);
        assertEquals("Wohnzimmer-PC ä", invite.shownName());
        assertEquals(InviteLink.CONTROLLER, invite.permission);
        assertEquals(1_800_000_000L, invite.expires);
        assertEquals("the link is handed on as it came", LINK, invite.text);
        assertFalse(invite.expired(1_799_999_999L));
        assertTrue(invite.expired(1_800_000_000L));
    }

    @Test
    public void theWindowsShellFormAndSurroundingSpaceAreRead() {
        String shell = LINK.replace("rubylight://invite#", "rubylight://invite/#");
        assertNotNull(InviteLink.parse(shell));
        assertNotNull(InviteLink.parse("  " + LINK + "\n"));
        assertTrue(InviteLink.looksLikeLink(shell));
        assertFalse(InviteLink.looksLikeLink("rubylight://pair?v=1"));
        assertFalse(InviteLink.looksLikeLink(null));
    }

    @Test
    public void aLinkWithoutWhatOpeningItNeedsIsRefused() {
        String[] bad = {
                LINK.replace("v=1", "v=2"),
                LINK.replace("k=" + "01".repeat(32), "k=" + "01".repeat(31)),
                LINK.replace("&id=0123ABCD-ef", ""),
                LINK.replace("&id=0123ABCD-ef", "&id=bad%20id"),
                LINK.replace("fp=" + "ab".repeat(32), "fp=zz"),
                LINK.replace("perm=controller", "perm=admin"),
                LINK.replace("exp=1800000000", "exp=soon"),
                LINK.replace(",47999,0", ",47999"),
                LINK.replace("ports=47989,", "ports=0,"),
                LINK.replace("ports=47989,", "ports=70000,"),
                LINK + "&perm=view",
                LINK + "&broken",
                LINK + "&x=" + "a".repeat(5000),
                "rubylight://invite#",
                "rubylight://invite?v=1",
        };
        for (String link : bad) {
            assertNull(link.length() > 200 ? link.substring(0, 200) : link, InviteLink.parse(link));
        }
    }

    @Test
    public void everyPermissionTheHostGivesIsRead() {
        for (String permission : new String[] {InviteLink.VIEW, InviteLink.CONTROLLER, InviteLink.KEYBOARD_MOUSE}) {
            InviteLink invite = InviteLink.parse(LINK.replace("perm=controller", "perm=" + permission));
            assertNotNull(permission, invite);
            assertEquals(permission, invite.permission);
        }
    }

    @Test
    public void percentEncodedNulInNameRoundTripsThroughJni() {
        InviteLink invite = InviteLink.parse(LINK.replace("Wohnzimmer-PC%20%C3%A4", "Room%00PC"));
        assertNotNull(invite);
        assertEquals("Room\u0000PC", invite.name);
        assertEquals("RoomPC", invite.shownName());
    }
}
