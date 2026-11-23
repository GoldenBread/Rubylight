package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nativeapi.NativeApiFixtures;

import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.junit.Test;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.math.BigInteger;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.cert.Certificate;
import java.security.cert.X509Certificate;
import java.util.Date;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLServerSocket;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * Another certificate than the pinned one at a PC's address (another PC took its lease, or a second
 * streaming app answers on the same ports): serverinfo says so, so the poll keeps the pairing
 * instead of taking that answer's PairStatus 0 as the PC forgetting this device.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ServerInfoCertificateTest {
    private static final String SERVERINFO = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">"
            + "<hostname>Other</hostname><uniqueid>other</uniqueid><HttpsPort>47984</HttpsPort>"
            + "<PairStatus>0</PairStatus><currentgame>0</currentgame><state>SUNSHINE_SERVER_FREE</state></root>";

    @Test
    public void anotherCertificateIsReportedAndPlainHttpIsNot() throws Exception {
        ExecutorService workers = Executors.newFixedThreadPool(4);
        InetAddress loopback = InetAddress.getLoopbackAddress();
        try (SSLServerSocket tls = otherHostTls(loopback); ServerSocket http = new ServerSocket(0, 4, loopback)) {
            workers.submit(() -> {
                // The handshake fails on the client's pin; nothing else is said.
                while (!tls.isClosed()) {
                    try (Socket socket = tls.accept()) {
                        socket.getInputStream().read();
                    } catch (Exception ignored) {
                    }
                }
                return null;
            });
            workers.submit(() -> {
                while (!http.isClosed()) {
                    try (Socket socket = http.accept()) {
                        BufferedReader reader = new BufferedReader(
                                new InputStreamReader(socket.getInputStream(), StandardCharsets.US_ASCII));
                        String line;
                        while ((line = reader.readLine()) != null && !line.isEmpty()) { }
                        byte[] body = SERVERINFO.getBytes(StandardCharsets.UTF_8);
                        socket.getOutputStream().write(("HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\n"
                                + "Content-Length: " + body.length + "\r\nConnection: close\r\n\r\n")
                                .getBytes(StandardCharsets.US_ASCII));
                        socket.getOutputStream().write(body);
                        socket.getOutputStream().flush();
                    } catch (Exception ignored) {
                    }
                }
                return null;
            });
            ComputerDetails.AddressTuple address =
                    new ComputerDetails.AddressTuple(loopback.getHostAddress(), http.getLocalPort());

            // Pinned to this PC's certificate, another one answers over HTTPS.
            NvHTTP pinned = new NvHTTP(address, tls.getLocalPort(), "1234567890abcdef",
                    NativeApiFixtures.certificate(), NativeApiFixtures.crypto());
            String reply = pinned.getServerInfo(true);
            assertTrue(reply.contains("<uniqueid>other</uniqueid>"));
            assertTrue("another certificate is reported", pinned.serverInfoFromAnotherCertificate());

            // Nothing pinned: plain HTTP, as for a PC not paired yet.
            NvHTTP plain = new NvHTTP(address, tls.getLocalPort(), "1234567890abcdef", null, NativeApiFixtures.crypto());
            plain.getServerInfo(true);
            assertFalse(plain.serverInfoFromAnotherCertificate());
        } finally {
            workers.shutdownNow();
        }
    }

    /** A TLS server with its own certificate, not the one the client pins. */
    private static SSLServerSocket otherHostTls(InetAddress at) throws Exception {
        KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
        generator.initialize(2048);
        KeyPair keys = generator.generateKeyPair();
        X500Name name = new X500Name("CN=Another host");
        X509Certificate certificate = new JcaX509CertificateConverter().getCertificate(
                new JcaX509v3CertificateBuilder(name, BigInteger.TEN, new Date(0), new Date(4102444800000L), name,
                        keys.getPublic()).build(new JcaContentSignerBuilder("SHA256withRSA").build(keys.getPrivate())));
        char[] password = "test".toCharArray();
        KeyStore store = KeyStore.getInstance("PKCS12");
        store.load(null, null);
        store.setKeyEntry("host", keys.getPrivate(), password, new Certificate[] {certificate});
        KeyManagerFactory managers = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
        managers.init(store, password);
        SSLContext context = SSLContext.getInstance("TLS");
        context.init(managers.getKeyManagers(), null, null);
        return (SSLServerSocket) context.getServerSocketFactory().createServerSocket(0, 4, at);
    }
}
