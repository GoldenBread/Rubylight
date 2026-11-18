package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.cert.X509Certificate;
import java.util.Date;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class InvitesTest {
    private static X509Certificate certificate() throws Exception {
        KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
        generator.initialize(2048);
        KeyPair keys = generator.generateKeyPair();
        X500Name name = new X500Name("CN=Invited PC");
        return new JcaX509CertificateConverter().getCertificate(new JcaX509v3CertificateBuilder(
                name, BigInteger.ONE, new Date(0), new Date(4102444800000L), name, keys.getPublic())
                .build(new JcaContentSignerBuilder("SHA256withRSA").build(keys.getPrivate())));
    }

    private static JSONObject redeemed(String address, X509Certificate certificate) throws Exception {
        return new JSONObject().put("uniqueId", "0123ABCD-ef").put("name", "Wohnzimmer").put("address", address)
                .put("port", 47989).put("httpsPort", 47984).put("path", "punched")
                .put("certificate", Invites.pem("CERTIFICATE", certificate.getEncoded())).put("reply", "{}");
    }

    @Test
    public void theEnginesWordsAreTheOnesTranslatedHere() throws Exception {
        String core = new String(Files.readAllBytes(Paths.get("../protocol/client/src/invite.rs")), StandardCharsets.UTF_8);
        for (String words : new String[] {Invites.NOT_AN_INVITE, Invites.EXPIRED, Invites.REFUSED, Invites.INTERNET_OFF}) {
            assertTrue(words, core.contains("\"" + words + "\""));
        }
    }

    @Test
    public void knownFailuresAreShownInTheAppsLanguageAndOthersAsTheyCame() {
        Context context = RuntimeEnvironment.getApplication();
        assertEquals(context.getString(R.string.invite_refused), Invites.errorText(context, Invites.REFUSED));
        assertEquals(context.getString(R.string.invite_internet_off), Invites.errorText(context, Invites.INTERNET_OFF));
        assertEquals(context.getString(R.string.invite_expired), Invites.errorText(context, Invites.EXPIRED));
        assertEquals(context.getString(R.string.invite_not_a_link), Invites.errorText(context, Invites.NOT_AN_INVITE));
        assertEquals("No path reached the PC", Invites.errorText(context, "No path reached the PC"));
    }

    @Test
    public void thePairedPcIsKeptWithItsCertificateButNoTunnelAddress() throws Exception {
        X509Certificate certificate = certificate();
        ComputerDetails punched = Invites.details(redeemed("127.78.0.2", certificate));
        assertEquals("0123ABCD-ef", punched.uuid);
        assertEquals("Wohnzimmer", punched.name);
        assertEquals(47984, punched.httpsPort);
        assertEquals(certificate, punched.serverCert);
        assertNull("a punched path's end lasts only as long as its tunnel", punched.manualAddress);
        assertEquals(new ComputerDetails.AddressTuple("127.78.0.2", 47989), punched.activeAddress);

        ComputerDetails direct = Invites.details(redeemed("2001:db8::2", certificate));
        assertEquals(new ComputerDetails.AddressTuple("2001:db8::2", 47989), direct.manualAddress);
    }

    @Test
    public void pemLinesAreSixtyFourWide() {
        String pem = Invites.pem("PRIVATE KEY", new byte[100]);
        String[] lines = pem.split("\n");
        assertEquals("-----BEGIN PRIVATE KEY-----", lines[0]);
        assertEquals(64, lines[1].length());
        assertEquals("-----END PRIVATE KEY-----", lines[lines.length - 1]);
    }

    @Test
    public void anInviteWithoutANameShowsThePcAndPermissionsUseTheGuestsWords() {
        Context context = RuntimeEnvironment.getApplication();
        assertEquals(context.getString(R.string.invite_this_pc), Invites.shownName(context, null));
        assertEquals(context.getString(R.string.guests_view), Invites.permissionLabel(context, "view"));
        assertEquals(context.getString(R.string.guests_keyboard_mouse),
                Invites.permissionLabel(context, "controller-keyboard-mouse"));
    }
}
