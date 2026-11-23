package com.limelight.computers;

import android.app.Application;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.jni.NightjarHostsJni;
import com.limelight.utils.CacheHelper;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.io.InputStream;
import java.io.OutputStream;
import java.math.BigInteger;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.cert.Certificate;
import java.security.cert.X509Certificate;
import java.util.Date;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLServerSocket;
import javax.net.ssl.SSLSocket;

import static org.junit.Assert.*;

/**
 * Verifies that stopping {@link ComputerManagerService.ApplistPoller} cancels in-flight Rust
 * requests and discards late replies before saving cache XML or notifying listeners.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class ApplistPollerLifecycleTest {
    @After
    public void resetManagers() {
        NightjarHostsJni.hostManagerResetForTests();
    }

    @Test
    public void stoppedRequestCannotReplaceTheSavedLibraryOrNotifyTheScreen() throws Exception {
        assertLateResponseDiscarded(false);
    }

    @Test
    public void previousRequestCannotPublishToTheResumedLibraryScreen() throws Exception {
        assertLateResponseDiscarded(true);
    }

    private void assertLateResponseDiscarded(boolean restart) throws Exception {
        com.limelight.nvstream.http.TestHttpServer.ensureIdentities();
        KeyPairGenerator gen = KeyPairGenerator.getInstance("RSA");
        gen.initialize(2048);
        KeyPair keys = gen.generateKeyPair();
        X500Name name = new X500Name("CN=ApplistFixture");
        X509Certificate cert = new JcaX509CertificateConverter().getCertificate(
                new JcaX509v3CertificateBuilder(
                        name, BigInteger.ONE, new Date(0), new Date(4102444800000L), name, keys.getPublic())
                        .build(new JcaContentSignerBuilder("SHA256withRSA").build(keys.getPrivate())));
        KeyStore ks = KeyStore.getInstance("PKCS12");
        ks.load(null, null);
        ks.setKeyEntry("host", keys.getPrivate(), new char[0], new Certificate[] {cert});
        KeyManagerFactory kmf = KeyManagerFactory.getInstance("SunX509", "SunJSSE");
        kmf.init(ks, new char[0]);
        SSLContext ssl = SSLContext.getInstance("TLS", "SunJSSE");
        ssl.init(kmf.getKeyManagers(), null, null);

        CountDownLatch firstEntered = new CountDownLatch(1);
        CountDownLatch firstRelease = new CountDownLatch(1);
        CountDownLatch firstDisconnected = new CountDownLatch(1);
        CountDownLatch secondEntered = new CountDownLatch(1);
        CountDownLatch secondRelease = new CountDownLatch(1);
        AtomicInteger reqCount = new AtomicInteger();

        try (ServerSocket httpPlain = new ServerSocket(0, 16, InetAddress.getLoopbackAddress());
             SSLServerSocket httpsSrv = (SSLServerSocket) ssl.getServerSocketFactory()
                     .createServerSocket(0, 16, InetAddress.getLoopbackAddress())) {
            int httpsPort = httpsSrv.getLocalPort();
            Thread httpsAccept = new Thread(() -> {
                while (!httpsSrv.isClosed()) {
                    try {
                        Socket s = httpsSrv.accept();
                        int n = reqCount.incrementAndGet();
                        new Thread(() -> handleHttps((SSLSocket) s, n,
                                n == 1 ? firstEntered : secondEntered,
                                n == 1 ? firstRelease : secondRelease,
                                n == 1 ? firstDisconnected : null), "test-https-" + n).start();
                    } catch (Exception ignored) {
                        break;
                    }
                }
            }, "test-https-accept");
            httpsAccept.start();

            ServiceController<ComputerManagerLifecycleTest.Manager> controller =
                    Robolectric.buildService(ComputerManagerLifecycleTest.Manager.class).create();
            ComputerManagerService service = controller.get();
            ComputerDetails computer = new ComputerDetails();
            computer.uuid = "delayed-library";
            computer.name = "PC";
            computer.state = ComputerDetails.State.ONLINE;
            computer.pairState = PairingManager.PairState.PAIRED;
            computer.activeAddress = new ComputerDetails.AddressTuple("127.0.0.1", httpPlain.getLocalPort());
            computer.httpsPort = httpsPort;
            computer.serverCert = cert;
            computer.rawAppList = appList("Saved app");

            java.nio.file.Path cache = CacheHelper.openPath(true, service.getCacheDir(), "applist", computer.uuid).toPath();
            Files.writeString(cache, computer.rawAppList);
            AtomicInteger updates = new AtomicInteger();
            CountDownLatch currentUpdated = new CountDownLatch(1);
            ReflectionHelpers.setField(service, "listener", new ComputerManagerListener() {
                @Override public void notifyComputerUpdated(ComputerDetails details) {}
                @Override public void notifyAppListUpdated(ComputerDetails details, boolean success) {
                    updates.incrementAndGet();
                    if (appList("Current app").equals(details.rawAppList)) currentUpdated.countDown();
                }
            });

            ComputerManagerService.ApplistPoller poller = service.new ApplistPoller(computer);
            Thread oldWorker = null, newWorker = null;
            try {
                poller.start();
                oldWorker = ReflectionHelpers.getField(poller, "thread");
                assertTrue("First applist request entered", firstEntered.await(5, TimeUnit.SECONDS));
                poller.stop();
                assertTrue("Stopping the poller closed its in-flight socket", firstDisconnected.await(3, TimeUnit.SECONDS));
                if (restart) {
                    poller.start();
                    newWorker = ReflectionHelpers.getField(poller, "thread");
                    assertTrue("Restarted request entered", secondEntered.await(5, TimeUnit.SECONDS));
                }
                firstRelease.countDown();
                oldWorker.join(3000);
                assertFalse("Stopped worker did not exit", oldWorker.isAlive());
                assertEquals("A stopped response replaced the current library", appList("Saved app"), computer.rawAppList);
                assertEquals(appList("Saved app"), Files.readString(cache));
                assertEquals("A stopped response notified the resumed screen", 0, updates.get());

                if (restart) {
                    secondRelease.countDown();
                    assertTrue(currentUpdated.await(5, TimeUnit.SECONDS));
                    assertEquals(1, updates.get());
                    assertEquals(appList("Current app"), computer.rawAppList);
                    assertEquals(appList("Current app"), Files.readString(cache));
                }
            } finally {
                firstRelease.countDown();
                secondRelease.countDown();
                poller.stop();
                if (oldWorker != null) oldWorker.join(2000);
                if (newWorker != null) newWorker.join(2000);
                controller.destroy();
            }
        }
    }

    private static void handleHttps(SSLSocket sock, int idx, CountDownLatch entered,
                                    CountDownLatch release, CountDownLatch disconnected) {
        try (SSLSocket s = sock) {
            InputStream in = s.getInputStream();
            byte[] buf = new byte[4096];
            int n = in.read(buf);
            if (n <= 0) return;
            entered.countDown();
            new Thread(() -> {
                try {
                    byte[] drain = new byte[256];
                    while (in.read(drain) >= 0) {}
                } catch (Exception ignored) {
                } finally {
                    if (disconnected != null) disconnected.countDown();
                }
            }, "test-https-eof").start();
            release.await(5, TimeUnit.SECONDS);
            // Detect peer disconnect if stop() cancelled the socket.
            if (s.getInputStream().available() == -1) {
                if (disconnected != null) disconnected.countDown();
                return;
            }
            String body = appList(idx == 1 ? "Stale app" : "Current app");
            byte[] payload = body.getBytes(StandardCharsets.UTF_8);
            String hdr = "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: "
                    + payload.length + "\r\nConnection: close\r\n\r\n";
            OutputStream out = s.getOutputStream();
            out.write(hdr.getBytes(StandardCharsets.US_ASCII));
            out.write(payload);
            out.flush();
            // Wait until client finishes or closes.
            in.read(buf);
        } catch (Exception ignored) {
        } finally {
            if (disconnected != null) disconnected.countDown();
        }
    }

    private static String appList(String title) {
        return "<root status_code=\"200\"><App><ID>73</ID><AppTitle>" + title + "</AppTitle></App></root>";
    }
}
