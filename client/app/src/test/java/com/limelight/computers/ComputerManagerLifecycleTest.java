package com.limelight.computers;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;

import com.limelight.AppView;
import com.limelight.discovery.DiscoveryService;
import com.limelight.nativeapi.HiddenStore;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.nvstream.http.TestHttpServer;
import com.limelight.nvstream.jni.NightjarHostsJni;
import com.limelight.nvstream.mdns.MdnsComputer;
import com.limelight.nvstream.mdns.MdnsDiscoveryAgent;
import com.limelight.nvstream.mdns.MdnsDiscoveryListener;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.utils.CacheHelper;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.math.BigInteger;
import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.Date;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;

import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLServerSocket;
import javax.net.ssl.SSLSocket;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class ComputerManagerLifecycleTest {
    public static class Manager extends ComputerManagerService {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    @After
    public void cleanup() {
        HostMetaStores.resetForTests();
        ComputerDatabaseManager.resetForTests();
    }

    private static void seedHost(ComputerDetails computer) {
        ComputerDatabaseManager db = new ComputerDatabaseManager(RuntimeEnvironment.getApplication());
        try {
            db.updateComputer(computer);
        } finally {
            db.close();
        }
    }

    private static final class FakeServerInfoServer implements AutoCloseable {
        final ServerSocket listener;
        final int port;
        final Thread thread;
        volatile String body;

        FakeServerInfoServer(String xml) throws IOException {
            this.body = xml;
            this.listener = new ServerSocket(0, 8, InetAddress.getByName("127.0.0.1"));
            this.port = listener.getLocalPort();
            this.thread = new Thread(() -> {
                while (!listener.isClosed()) {
                    try (Socket sock = listener.accept()) {
                        InputStream in = sock.getInputStream();
                        byte[] buf = new byte[2048];
                        in.read(buf);
                        String xmlBody = body;
                        String reply = "HTTP/1.1 200 OK\r\nContent-Length: " + xmlBody.length()
                                + "\r\nConnection: close\r\n\r\n" + xmlBody;
                        OutputStream out = sock.getOutputStream();
                        out.write(reply.getBytes(StandardCharsets.US_ASCII));
                        out.flush();
                    } catch (IOException ignored) {
                    }
                }
            }, "fake-serverinfo");
            this.thread.start();
        }

        @Override public void close() throws Exception {
            listener.close();
            thread.join(2000);
        }
    }

    @Test
    public void deletingComputerRemovesItsCachedLibraryArtworkAndHiddenAppsOnly() throws Exception {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "removed-host";
        try {
            for (String host : new String[] {computer.uuid, "another-host"}) {
                Files.writeString(CacheHelper.openPath(true, service.getCacheDir(), "applist", host).toPath(), "library");
                Files.writeString(CacheHelper.openPath(true, service.getCacheDir(), "boxart", host, "42.png").toPath(), "cover");
                service.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0).edit()
                        .putStringSet(host, Collections.singleton("42"))
                        .putStringSet(HiddenStore.Preferences.idsKey(host), Collections.singleton("42:install")).commit();
            }

            service.removeComputer(computer);

            assertFalse(CacheHelper.openPath(false, service.getCacheDir(), "applist", computer.uuid).exists());
            assertFalse(CacheHelper.openPath(false, service.getCacheDir(), "boxart", computer.uuid).exists());
            assertTrue(new HiddenStore.Preferences(service).of(computer.uuid).isEmpty());
            assertTrue(new HiddenStore.Preferences(service).ids(computer.uuid).isEmpty());
            assertEquals(Collections.singletonMap("install", 42),
                    new HiddenStore.Preferences(service).ids("another-host"));
            assertTrue(CacheHelper.openPath(false, service.getCacheDir(), "applist", "another-host").isFile());
            assertTrue(CacheHelper.openPath(false, service.getCacheDir(), "boxart", "another-host", "42.png").isFile());
            assertEquals(Collections.singleton(42),
                    new HiddenStore.Preferences(service).of("another-host"));
        } finally {
            com.limelight.nativeapi.ConsoleRepository.resetForTests();
            controller.destroy();
        }
    }

    @Test
    public void deletingComputerRemovesOnlyItsSavedStreamSettings() {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "removed-host";
        String[] files = {"HostStreamProfiles", "HostDisplayChoices", "AdaptiveBitrate", "PyroWaveBitrate"};
        try {
            for (String file : files) {
                service.getSharedPreferences(file, 0).edit()
                        .putString(computer.uuid, "saved").putString("another-host", "retained").commit();
            }
            HostCapabilityCache.save(service, computer.uuid, TestHosts.rubylight());
            HostCapabilityCache.save(service, "another-host", TestHosts.rubylight());
            assertTrue(HostNotices.claim(service, computer.uuid, HostNotices.Notice.PYROWAVE_FALLBACK));
            assertTrue(HostNotices.claim(service, "another-host", HostNotices.Notice.PYROWAVE_FALLBACK));
            HostNames.set(service, computer.uuid, "Removed PC");
            HostNames.set(service, "another-host", "Retained PC");

            service.removeComputer(computer);
            for (String file : files) {
                assertFalse(service.getSharedPreferences(file, 0).contains(computer.uuid));
                assertEquals("retained", service.getSharedPreferences(file, 0).getString("another-host", null));
            }
            assertSame(HostCapabilities.UNKNOWN, HostCapabilityCache.load(service, computer.uuid));
            assertEquals(TestHosts.rubylight(), HostCapabilityCache.load(service, "another-host"));
            assertTrue(HostNotices.claim(service, computer.uuid, HostNotices.Notice.PYROWAVE_FALLBACK));
            assertFalse(HostNotices.claim(service, "another-host", HostNotices.Notice.PYROWAVE_FALLBACK));
            assertNull(HostNames.get(service, computer.uuid));
            assertEquals("Retained PC", HostNames.get(service, "another-host"));
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void aNameGivenToAPcSurvivesPollsThatRenameTheHost() {
        ComputerDetails stored = new ComputerDetails();
        stored.uuid = "named-host";
        stored.name = "DESKTOP-1";
        seedHost(stored);
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        Manager service = controller.get();
        ComputerManagerService.ComputerManagerBinder binder =
                (ComputerManagerService.ComputerManagerBinder) service.onBind(null);
        try {
            assertNull(binder.getComputer("named-host").alias);

            service.renameComputer("named-host", "Den PC");
            ComputerDetails tuple = binder.getComputer("named-host");
            assertEquals("Den PC", tuple.alias);
            assertEquals("Den PC", tuple.displayName());
            assertEquals("Den PC", HostNames.get(service, "named-host"));

            service.renameComputer("named-host", null);
            assertEquals("DESKTOP-1", binder.getComputer("named-host").displayName());
            assertNull(HostNames.get(service, "named-host"));
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void aRediscoveredPcIsFirstShownWithItsStoredName() throws Exception {
        try (FakeServerInfoServer srv = new FakeServerInfoServer(
                "<root status_code=\"200\"><hostname>DESKTOP-9</hostname><uniqueid>named-host</uniqueid>"
                        + "<state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus></root>")) {
            ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
            Manager service = controller.get();
            ComputerManagerService.ComputerManagerBinder binder =
                    (ComputerManagerService.ComputerManagerBinder) service.onBind(null);
            try {
                HostNames.set(service, "named-host", "Den PC");
                List<String> shown = new CopyOnWriteArrayList<>();
                CountDownLatch notified = new CountDownLatch(1);
                binder.startPolling(details -> {
                    if ("named-host".equals(details.uuid)) {
                        shown.add(details.displayName());
                        notified.countDown();
                    }
                });

                ComputerDetails found = new ComputerDetails();
                found.localAddress = new ComputerDetails.AddressTuple("127.0.0.1", srv.port);
                assertTrue(service.addComputerBlocking(found));
                assertTrue(notified.await(5, TimeUnit.SECONDS));
                assertEquals(Collections.singletonList("Den PC"), shown);
                assertEquals("DESKTOP-9", found.name);
            } finally {
                controller.destroy();
            }
        }
    }

    @Test
    public void cachedCapabilitiesSurviveRestartBeforeTheFirstPoll() {
        Context context = RuntimeEnvironment.getApplication();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "cached-caps-host";
        computer.name = "Cached";
        seedHost(computer);
        HostCapabilityCache.save(context, computer.uuid, TestHosts.fixture("vibepollo-serverinfo.xml"));
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        Manager service = controller.get();
        ComputerManagerService.ComputerManagerBinder binder =
                (ComputerManagerService.ComputerManagerBinder) service.onBind(null);
        try {
            ComputerDetails restored = binder.getComputer(computer.uuid);
            assertNotNull(restored);
            assertEquals(HostCapabilities.Family.VIBEPOLLO, restored.caps.family());
            assertTrue(restored.caps.has(HostCapabilities.Feature.VIRTUAL_DISPLAY));
        } finally {
            controller.destroy();
        }
    }

    public static class NativeManager extends Manager {
        final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();

        @Override NativeApiClient nativeClient(ComputerDetails computer) {
            HostEndpoint endpoint = HostEndpoint.of(computer, "unique", NativeApiFixtures.crypto());
            return endpoint == null ? null : new NativeApiClient(transport, endpoint);
        }
    }

    @Test
    public void aPollOfAPcThatListsWakeKeepsItsTargetsForThePcListAndShortcuts() throws Exception {
        java.util.concurrent.atomic.AtomicInteger httpsPort = new java.util.concurrent.atomic.AtomicInteger();
        try (TestHttpServer srv = new TestHttpServer(req -> {
            int paired = "https".equals(req.scheme) ? 1 : 0;
            return TestHttpServer.Response.xml(200,
                    "<root status_code=\"200\"><hostname>Tower</hostname><uniqueid>tower</uniqueid>"
                            + "<state>MJ_SERVER_FREE</state><PairStatus>" + paired + "</PairStatus>"
                            + "<HttpsPort>" + httpsPort.get() + "</HttpsPort>"
                            + "<RustHostVersion>3.0.0</RustHostVersion><NativeApiVersion>1</NativeApiVersion></root>");
        })) {
            httpsPort.set(srv.httpsPort());
            ComputerDetails initial = new ComputerDetails();
            initial.localAddress = new ComputerDetails.AddressTuple("127.0.0.1", srv.httpPort());

            ServiceController<NativeManager> controller = Robolectric.buildService(NativeManager.class).create();
            NativeManager service = controller.get();
            ComputerManagerService.ComputerManagerBinder binder =
                    (ComputerManagerService.ComputerManagerBinder) service.onBind(null);
            try {
                service.transport.reply("tower", "capabilities", NativeApiFixtures.ok("{\"api_version\":1,"
                        + "\"features\":[\"wake\"],\"permissions\":{\"list\":true,\"view\":true,\"launch\":true,"
                        + "\"host_control\":false}}"));
                service.transport.reply("tower", "wake", NativeApiFixtures.ok("{\"revision\":1,\"targets\":["
                        + "{\"mac\":\"02:e0:4c:68:00:01\",\"broadcasts\":[\"10.0.0.255\"]}]}"));
                assertTrue(service.addComputerBlocking(initial));
                binder.setServerCertificate("tower", srv.serverCert().getEncoded());
                CountDownLatch online = new CountDownLatch(1);
                binder.startPolling(details -> {
                    if ("tower".equals(details.uuid) && details.state == ComputerDetails.State.ONLINE
                            && details.serverCert != null
                            && details.caps.has(HostCapabilities.Feature.NATIVE_API)) {
                        online.countDown();
                    }
                });
                assertTrue(online.await(5, TimeUnit.SECONDS));
                awaitWakeRequests(service);

                WakeTargets.Targets targets = new WakeTargets(service).load("tower");
                assertEquals(Collections.singletonList("02:e0:4c:68:00:01"), targets.macs);
                assertEquals(Collections.singletonList("10.0.0.255"), targets.broadcasts);
            } finally {
                controller.destroy();
            }
        }
    }

    private static void awaitWakeRequests(ComputerManagerService service) throws Exception {
        ExecutorService requests = ReflectionHelpers.getField(service, "wakeRequests");
        requests.submit(() -> { }).get(10, TimeUnit.SECONDS);
    }

    @Test
    public void discoveryUsesIpv6TooAndPreservesTheDiscoveredV4AndV6Addresses() throws Exception {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        Manager service = controller.get();
        List<ComputerDetails> added = new ArrayList<>();
        try {
            DiscoveryService.DiscoveryBinder discovery = new DiscoveryService().new DiscoveryBinder() {
                @Override public void setListener(MdnsDiscoveryListener listener) {
                    ComputerDetails details = new ComputerDetails();
                    try {
                        Inet4Address v4 = (Inet4Address) InetAddress.getByAddress(new byte[] {(byte) 192, (byte) 168, 4, 21});
                        Inet6Address v6 = (Inet6Address) InetAddress.getByAddress(
                                new byte[] {0x20, 1, 0x0d, (byte) 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x21});
                        MdnsComputer discovered = new MdnsComputer("Desk", v4, v6, 47989);
                        if (discovered.getLocalAddress() != null) {
                            details.localAddress = new ComputerDetails.AddressTuple(
                                    discovered.getLocalAddress().getHostAddress(), discovered.getPort());
                        }
                        if (discovered.getIpv6Address() != null) {
                            details.ipv6Address = new ComputerDetails.AddressTuple(
                                    discovered.getIpv6Address().getHostAddress(), discovered.getPort());
                        }
                        added.add(details);
                    } catch (Exception e) {
                        throw new AssertionError(e);
                    }
                }
            };
            discovery.setListener(null);
            assertEquals(1, added.size());
            assertEquals("192.168.4.21", added.get(0).localAddress.address);
            assertEquals("2001:db8:0:0:0:0:0:21", added.get(0).ipv6Address.address);
            assertEquals(47989, added.get(0).ipv6Address.port);
            assertFalse(Arrays.asList(MdnsDiscoveryAgent.class.getInterfaces()).isEmpty());
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void networkChangeRespectsAtMsGuardAndNeverBlocks() throws Exception {
        try (FakeServerInfoServer srv = new FakeServerInfoServer(
                "<root status_code=\"200\"><hostname>NetPC</hostname><uniqueid>network-host</uniqueid>"
                        + "<state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus></root>")) {
            ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
            Manager service = controller.get();
            ComputerManagerService.ComputerManagerBinder binder =
                    (ComputerManagerService.ComputerManagerBinder) service.onBind(null);
            try {
                long handle = binder.hostManagerHandle();
                long beforePollMs = NightjarHostsJni.hostManagerNowMs(handle);
                Thread.sleep(15);

                ComputerDetails computer = new ComputerDetails();
                computer.localAddress = new ComputerDetails.AddressTuple("127.0.0.1", srv.port);
                assertTrue(service.addComputerBlocking(computer));
                long afterPollMs = NightjarHostsJni.hostManagerNowMs(handle) + 100;

                // Network transition timestamped before the successful poll leaves the host ONLINE.
                service.resetStateForNetworkChange(true, beforePollMs);
                assertEquals(ComputerDetails.State.ONLINE, binder.getComputer("network-host").state);

                // Newer transition after the poll resets state to UNKNOWN (or OFFLINE when !available).
                service.resetStateForNetworkChange(false, afterPollMs);
                assertEquals(ComputerDetails.State.OFFLINE, binder.getComputer("network-host").state);
            } finally {
                controller.destroy();
            }
        }
    }

    @Test
    public void unpairingForgetsTheCertificateInMemoryAndSaved() {
        ComputerDetails stored = new ComputerDetails();
        stored.uuid = "unpaired-host";
        stored.name = "PC";
        stored.activeAddress = new ComputerDetails.AddressTuple("192.168.1.9", 47989);
        stored.remoteAddress = stored.activeAddress;
        stored.serverCert = NativeApiFixtures.certificate();
        seedHost(stored);

        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        Manager service = controller.get();
        ComputerManagerService.ComputerManagerBinder binder =
                (ComputerManagerService.ComputerManagerBinder) service.onBind(null);
        try {
            service.forgetCertificate("unpaired-host");
            assertNull(binder.getComputer("unpaired-host").serverCert);
            ComputerDatabaseManager db = new ComputerDatabaseManager(service);
            try {
                ComputerDetails saved = db.getComputerByUUID("unpaired-host");
                assertNull(saved.serverCert);
                assertEquals("PC", saved.name);
            } finally {
                db.close();
            }
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void invalidateStateForComputerResetsToUnknown() throws Exception {
        try (FakeServerInfoServer srv = new FakeServerInfoServer(
                "<root status_code=\"200\"><hostname>Busy</hostname><uniqueid>busy-host</uniqueid>"
                        + "<state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus></root>")) {
            ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
            Manager service = controller.get();
            ComputerManagerService.ComputerManagerBinder binder =
                    (ComputerManagerService.ComputerManagerBinder) service.onBind(null);
            try {
                ComputerDetails comp = new ComputerDetails();
                comp.localAddress = new ComputerDetails.AddressTuple("127.0.0.1", srv.port);
                assertTrue(service.addComputerBlocking(comp));
                assertEquals(ComputerDetails.State.ONLINE, binder.getComputer("busy-host").state);
                binder.invalidateStateForComputer("busy-host");
                assertEquals(ComputerDetails.State.UNKNOWN, binder.getComputer("busy-host").state);
            } finally {
                controller.destroy();
            }
        }
    }

    @Test
    public void pollComputerSetsAnotherCertificateWhenMismatchAndNoFallbackSucceeds() throws Exception {
        KeyPairGenerator gen = KeyPairGenerator.getInstance("RSA");
        gen.initialize(2048);
        KeyPair kp = gen.generateKeyPair();
        X500Name dn = new X500Name("CN=Other Host");
        X509Certificate serverCert = new JcaX509CertificateConverter().getCertificate(
                new JcaX509v3CertificateBuilder(dn, BigInteger.valueOf(99), new Date(0),
                        new Date(4102444800000L), dn, kp.getPublic())
                        .build(new JcaContentSignerBuilder("SHA256withRSA").build(kp.getPrivate())));
        KeyStore ks = KeyStore.getInstance(KeyStore.getDefaultType());
        ks.load(null, null);
        ks.setKeyEntry("k", (PrivateKey) kp.getPrivate(), new char[0], new java.security.cert.Certificate[] {serverCert});
        KeyManagerFactory kmf = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
        kmf.init(ks, new char[0]);
        SSLContext ssl = SSLContext.getInstance("TLS");
        ssl.init(kmf.getKeyManagers(), null, null);

        try (SSLServerSocket tlsSock = (SSLServerSocket) ssl.getServerSocketFactory()
                .createServerSocket(0, 4, InetAddress.getByName("127.0.0.1"))) {
            int httpsPort = tlsSock.getLocalPort();
            Thread tlsSrv = new Thread(() -> {
                try (SSLSocket s = (SSLSocket) tlsSock.accept()) {
                    s.startHandshake();
                } catch (IOException ignored) {
                }
            }, "mismatch-tls");
            tlsSrv.start();

            try (FakeServerInfoServer httpDisc = new FakeServerInfoServer(
                    "<root status_code=\"200\"><hostname>Other</hostname><uniqueid>pc-mismatch</uniqueid>"
                            + "<state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>" + httpsPort
                            + "</HttpsPort></root>")) {
                ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
                Manager service = controller.get();
                try {
                    ComputerDetails details = new ComputerDetails();
                    details.uuid = "pc-mismatch";
                    details.localAddress = new ComputerDetails.AddressTuple("127.0.0.1", httpDisc.port);
                    details.httpsPort = httpsPort;
                    details.serverCert = NativeApiFixtures.certificate();

                    assertFalse(service.pollComputer(details));
                    assertTrue(details.anotherCertificate);
                    assertNotNull("mismatch must preserve pinned certificate", details.serverCert);
                } finally {
                    controller.destroy();
                }
            }
            tlsSrv.join(2000);
        }
    }

    @Test
    public void pollComputerPopulatesLocalAddressSessionIdleAndNvidiaServer() throws Exception {
        String xml = "<root status_code=\"200\"><hostname>NvidiaPC</hostname><uniqueid>nv-host</uniqueid>"
                + "<state>MJOLNIR_STATE_SERVER_FREE</state><PairStatus>0</PairStatus><LocalIP>192.168.1.50</LocalIP>"
                + "<GfeVersion>3.23.0.74</GfeVersion><ServerCodecModeSupport>8457985</ServerCodecModeSupport></root>";
        try (FakeServerInfoServer srv = new FakeServerInfoServer(xml)) {
            ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
            Manager service = controller.get();
            try {
                ComputerDetails details = new ComputerDetails();
                details.uuid = "nv-host";
                details.localAddress = new ComputerDetails.AddressTuple("127.0.0.1", srv.port);

                assertTrue(service.pollComputer(details));
                assertEquals(new ComputerDetails.AddressTuple("192.168.1.50", srv.port), details.localAddress);
                assertTrue(details.sessionIdle);
                assertTrue(details.nvidiaServer);
                assertEquals(ComputerDetails.State.ONLINE, details.state);
            } finally {
                controller.destroy();
            }
        }
    }

    @Test
    public void pollComputerTriesInternetPlayMappedWhileStateIsUnknown() throws Exception {
        String xml = "<root status_code=\"200\"><hostname>WanPC</hostname><uniqueid>wan-pc</uniqueid>"
                + "<state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus></root>";
        try (FakeServerInfoServer srv = new FakeServerInfoServer(xml)) {
            ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
            Manager service = controller.get();
            try {
                ComputerDetails details = new ComputerDetails();
                details.uuid = "wan-pc";
                details.state = ComputerDetails.State.UNKNOWN;
                details.localAddress = new ComputerDetails.AddressTuple("127.0.0.2", 1);
                String reply = "{\"version\":1,\"enabled\":false,\"externalIpv4\":\"127.0.0.1\","
                        + "\"mappedPorts\":true,\"ports\":{\"http\":" + srv.port
                        + ",\"https\":47984,\"rtsp\":48010,\"video\":47998,\"audio\":48000,\"control\":47999,\"mic\":48002}}";
                InternetPlay.save(service, "wan-pc", reply);

                assertTrue(service.pollComputer(details));
                assertEquals(new ComputerDetails.AddressTuple("127.0.0.1", srv.port), details.activeAddress);
                assertEquals("mapped", details.internetPath);
            } finally {
                InternetPlay.forget(service, "wan-pc");
                controller.destroy();
            }
        }
    }

    @Test
    public void playOverTheInternetIsTakenInAtStartAndTurningItOffEndsTheTunnelsAtOnce() throws Exception {
        String enabledReply = "{\"version\":1,\"enabled\":true,\"portMapping\":false,\"externalIpv4\":null,"
                + "\"mappedPorts\":false,\"ipv6\":[],\"ports\":{\"http\":47989,\"https\":47984,\"rtsp\":48010,"
                + "\"video\":47998,\"audio\":48000,\"control\":47999,\"mic\":48002}}";
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        try {
            ExecutorService executor = ReflectionHelpers.getField(controller.get(), "internetPlayCalls");
            Future<?> startDone = executor.submit(() -> {});
            startDone.get(5, TimeUnit.SECONDS);
            String before = InternetPlay.unreachable(enabledReply, new InternetPlay.Tried());
            assertFalse("service start enables internet play: " + before,
                    before.contains("off in Nightjar's settings"));

            android.preference.PreferenceManager.getDefaultSharedPreferences(controller.get()).edit()
                    .putBoolean(com.limelight.preferences.PreferenceConfiguration.INTERNET_PLAY_PREF_STRING, false).commit();
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            Future<?> offDone = executor.submit(() -> {});
            offDone.get(5, TimeUnit.SECONDS);
            String after = InternetPlay.unreachable(enabledReply, new InternetPlay.Tried());
            assertTrue("switching the setting off reaches the tunnels: " + after,
                    after.contains("off in Nightjar's settings"));
        } finally {
            InternetPlay.setEnabled(true);
            controller.destroy();
        }
    }

    @Test
    public void pollComputerAfterShutdownDoesNotCrash() {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails details = new ComputerDetails();
        details.uuid = "internet-host";
        details.state = ComputerDetails.State.ONLINE;
        details.localAddress = new ComputerDetails.AddressTuple("127.0.0.1", 47989);
        controller.destroy();

        assertFalse(service.pollComputer(details));
    }
}
