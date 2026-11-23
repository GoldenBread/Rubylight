package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nvstream.jni.NightjarHttpJni;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.xmlpull.v1.XmlPullParserException;

import java.net.URI;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvHTTPNetworkingTest {
    private NvHTTP http(String address) throws Exception {
        return new NvHTTP(new ComputerDetails.AddressTuple(address, 47989), 47984, "client", null,
                new LimelightCryptoProvider() {
                    public X509Certificate getClientCertificate() { return null; }
                    public PrivateKey getClientPrivateKey() { return null; }
                    public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
                    public String encodeBase64String(byte[] data) { return ""; }
                });
    }

    @Test
    public void scopedIpv6CanBuildHttpAndHttpsUrls() throws Exception {
        NvHTTP http = http("fe80::1234%1");
        URI boxArt = URI.create(NightjarHttpJni.getBoxArtUrl(http.httpHandle(), 42));
        assertEquals("https", boxArt.getScheme());
        assertEquals("[fe80::1234]", boxArt.getHost());
        assertEquals(47984, boxArt.getPort());
    }

    @Test
    public void hostIdsCannotEscapeTheCacheDirectory() throws Exception {
        NvHTTP http = http("192.0.2.1");
        for (String uuid : new String[] {"..", "../escape", "..\\escape", "C:\\escape"}) {
            assertThrows(XmlPullParserException.class, () -> http.getComputerDetails(
                    "<root status_code=\"200\"><uniqueid>" + uuid +
                            "</uniqueid><PairStatus>0</PairStatus><state>FREE</state></root>"));
        }
    }

    @Test
    public void portUpdatesDoNotMutateOtherComputerSnapshotsOrManualAddresses() {
        ComputerDetails original = new ComputerDetails();
        original.remoteAddress = new ComputerDetails.AddressTuple("192.0.2.1", 47989);
        original.manualAddress = original.remoteAddress;
        ComputerDetails copy = new ComputerDetails(original);
        ComputerDetails update = new ComputerDetails();
        update.externalPort = 48000;
        copy.update(update);
        assertEquals(48000, copy.remoteAddress.port);
        assertEquals(47989, original.remoteAddress.port);
        assertEquals(47989, copy.manualAddress.port);
    }

    @Test
    public void addressesRejectOutOfRangePorts() {
        assertThrows(IllegalArgumentException.class, () -> new ComputerDetails.AddressTuple("pc", 65536));
        assertEquals(65535, new ComputerDetails.AddressTuple("pc", 65535).port);
    }
}
