package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.InviteLink;
import com.limelight.nvstream.http.PairingLink;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.junit.Test;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.math.BigInteger;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.MessageDigest;
import java.security.cert.X509Certificate;
import java.util.Date;
import java.util.HexFormat;

/** Whatever answers at a link's address may claim any PC's identity: a web page can pick that address. */
@org.junit.runner.RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class LinkToKnownPcTest {
    private static final ComputerDetails.AddressTuple KNOWN_ADDRESS = new ComputerDetails.AddressTuple("192.168.4.2", 47989);
    private static final ComputerDetails.AddressTuple LINK_ADDRESS = new ComputerDetails.AddressTuple("203.0.113.9", 47989);

    /** Answers every poll in the name of the known PC. */
    public static class Manager extends ComputerManagerService {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }

        @Override boolean pollComputer(ComputerDetails details) {
            details.uuid = "known-pc";
            details.name = "Claimed";
            details.state = ComputerDetails.State.ONLINE;
            details.activeAddress = details.manualAddress;
            return true;
        }
    }

    private static X509Certificate certificate(long serial) throws Exception {
        KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
        generator.initialize(2048);
        KeyPair keys = generator.generateKeyPair();
        X500Name name = new X500Name("CN=Known PC");
        return new JcaX509CertificateConverter().getCertificate(new JcaX509v3CertificateBuilder(
                name, BigInteger.valueOf(serial), new Date(0), new Date(4102444800000L), name, keys.getPublic())
                .build(new JcaContentSignerBuilder("SHA256withRSA").build(keys.getPrivate())));
    }

    private static PairingLink linkNaming(X509Certificate certificate) throws Exception {
        String fp = HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(certificate.getEncoded()));
        PairingLink link = PairingLink.parse("rubylight://pair?v=1&host=203.0.113.9&port=47989&otp=4821&fp=" + fp);
        assertNotNull(link);
        return link;
    }

    private static ComputerDetails knownPc(X509Certificate pinned) {
        ComputerDetails known = new ComputerDetails();
        known.uuid = "known-pc";
        known.name = "Desk";
        known.state = ComputerDetails.State.ONLINE;
        known.manualAddress = KNOWN_ADDRESS;
        known.activeAddress = KNOWN_ADDRESS;
        known.serverCert = pinned;
        return known;
    }

    private static ComputerDetails linkDetails() {
        ComputerDetails details = new ComputerDetails();
        details.manualAddress = LINK_ADDRESS;
        return details;
    }

    @Test
    public void aServerClaimingAPinnedPcIsRefusedBeforeItsAddressChanges() throws Exception {
        var controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails known = knownPc(certificate(1));
        ReflectionHelpers.callInstanceMethod(service, "addTuple",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, known));

        assertEquals(ComputerManagerService.LinkAdd.CERTIFICATE_MISMATCH,
                service.addComputerForLink(linkDetails(), linkNaming(certificate(2))));

        assertEquals(KNOWN_ADDRESS, known.manualAddress);
        assertEquals(KNOWN_ADDRESS, known.activeAddress);
        assertEquals("Desk", known.name);
        controller.destroy();
    }

    private static InviteLink inviteNaming(String id, X509Certificate certificate) throws Exception {
        String fp = HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(certificate.getEncoded()));
        InviteLink invite = InviteLink.parse("rubylight://invite#v=1&k=" + "ab".repeat(32) + "&fp=" + fp + "&id=" + id
                + "&perm=controller&exp=4000000000&ports=47989,47984,48010,47998,48000,47999,48002");
        assertNotNull(invite);
        return invite;
    }

    private static ComputerDetails invited(X509Certificate certificate) {
        ComputerDetails details = new ComputerDetails();
        details.uuid = "known-pc";
        details.name = "Claimed";
        details.state = ComputerDetails.State.UNKNOWN;
        details.manualAddress = LINK_ADDRESS;
        details.activeAddress = LINK_ADDRESS;
        details.httpsPort = 47984;
        details.serverCert = certificate;
        return details;
    }

    // Anyone can write an invite with the ID of a PC paired here and the certificate of the PC it really leads to.
    @Test
    public void anInviteNeverReplacesTheCertificateOfAPcPairedWithAnotherOne() throws Exception {
        var controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        X509Certificate pinned = certificate(1);
        X509Certificate other = certificate(2);
        ComputerDetails known = knownPc(pinned);
        ReflectionHelpers.callInstanceMethod(service, "addTuple",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, known));
        InternetPlay.save(service, "known-pc", "its own");

        // Before anything is sent.
        assertTrue(service.inviteClaimsKnownPc(inviteNaming("known-pc", other)));
        assertFalse(service.inviteClaimsKnownPc(inviteNaming("known-pc", pinned)));
        assertFalse(service.inviteClaimsKnownPc(inviteNaming("another-pc", other)));

        // And once more where the PC is kept.
        assertFalse(service.addInvitedComputer(invited(other), "the invite's"));
        assertEquals(pinned, known.serverCert);
        assertEquals(KNOWN_ADDRESS, known.manualAddress);
        assertEquals("Desk", known.name);
        assertEquals("its own", InternetPlay.reply(service, "known-pc"));

        // The same certificate is the PC itself: the invite's details are kept.
        assertTrue(service.addInvitedComputer(invited(pinned), "the invite's"));
        assertEquals("the invite's", InternetPlay.reply(service, "known-pc"));
        controller.destroy();
    }

    // The PC is not polled yet (the service starts its tuples after binding) but is saved with its certificate.
    @Test
    public void anInviteNeverReplacesASavedPcsCertificateEither() throws Exception {
        var controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        X509Certificate pinned = certificate(1);
        ComputerDatabaseManager database = new ComputerDatabaseManager(service);
        try {
            database.updateComputer(knownPc(pinned));
            assertFalse(service.addInvitedComputer(invited(certificate(2)), "the invite's"));
            assertEquals(pinned, database.getComputerByUUID("known-pc").serverCert);
            assertEquals(KNOWN_ADDRESS, database.getComputerByUUID("known-pc").manualAddress);
        } finally {
            database.close();
        }
        controller.destroy();
    }

    @Test
    public void theRightCertificateStillGoesThrough() throws Exception {
        var controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        X509Certificate pinned = certificate(1);
        ReflectionHelpers.callInstanceMethod(service, "addTuple",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, knownPc(pinned)));
        assertEquals(ComputerManagerService.LinkAdd.ADDED, service.addComputerForLink(linkDetails(), linkNaming(pinned)));
        controller.destroy();
    }

    @Test
    public void aPcWithoutACertificateStillGoesThrough() throws Exception {
        var controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ReflectionHelpers.callInstanceMethod(service, "addTuple",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, knownPc(null)));
        assertEquals(ComputerManagerService.LinkAdd.ADDED, service.addComputerForLink(linkDetails(), linkNaming(certificate(2))));
        controller.destroy();
    }
}
