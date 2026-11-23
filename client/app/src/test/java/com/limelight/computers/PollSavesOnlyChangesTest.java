package com.limelight.computers;

import android.app.Application;
import android.content.Context;
import android.database.sqlite.SQLiteDatabase;

import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.KeyStore;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.function.Consumer;

import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509TrustManager;

import static org.junit.Assert.*;

/**
 * A poll that finds a PC as it is saved writes nothing to the database, and one that finds
 * anything the database keeps of a PC changed writes it: the name, each address, the MAC address
 * and the certificate, also when the certificate goes.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class PollSavesOnlyChangesTest {
    private static final String UUID = "polled-host";
    private static final ComputerDetails.AddressTuple REMOTE = new ComputerDetails.AddressTuple("203.0.113.9", 47989);
    private static final ComputerDetails.AddressTuple LOCAL = new ComputerDetails.AddressTuple("192.168.1.9", 47989);

    @After
    public void tearDown() {
        ComputerDatabaseManager.resetForTests();
    }

    /** A PC that answers every poll; what it says of itself is the test's to change. */
    public static class AnsweringManager extends ComputerManagerLifecycleTest.Manager {
        Consumer<ComputerDetails> answer = polled -> {};

        @Override boolean pollComputer(ComputerDetails details) {
            // As the service's own: what the PC answered is merged into the PC being polled.
            ComputerDetails polled = new ComputerDetails();
            polled.uuid = details.uuid;
            polled.name = "PC";
            polled.state = ComputerDetails.State.ONLINE;
            polled.activeAddress = LOCAL;
            polled.localAddress = LOCAL;
            polled.macAddress = "AA:BB:CC:DD:EE:01";
            polled.pairState = PairingManager.PairState.PAIRED;
            answer.accept(polled);
            details.update(polled);
            return true;
        }
    }

    /** The database of the service, counting the rows written and letting a test in after a read. */
    private static final class CountingDatabase extends ComputerDatabaseManager {
        final List<String> written = new ArrayList<>();
        Runnable afterNextRead;

        CountingDatabase(Context context) {
            super(context);
        }

        @Override public boolean updateComputer(ComputerDetails details) {
            // Null only while the constructor moves old databases over, which no test here has.
            if (written != null) {
                written.add(details.uuid);
            }
            return super.updateComputer(details);
        }

        @Override public ComputerDetails getComputerByUUID(String uuid) {
            ComputerDetails read = super.getComputerByUUID(uuid);
            Runnable then = afterNextRead;
            afterNextRead = null;
            if (then != null) {
                then.run();
            }
            return read;
        }
    }

    /** The service with one saved PC, paired or not. */
    private static final class Polls implements AutoCloseable {
        final Context context = RuntimeEnvironment.getApplication();
        final ServiceController<AnsweringManager> controller;
        final AnsweringManager service;
        final CountingDatabase database;
        final ComputerManagerService.TrackedHost tuple;

        Polls(X509Certificate certificate) {
            ComputerDetails stored = new ComputerDetails();
            stored.uuid = UUID;
            stored.name = "PC";
            stored.localAddress = LOCAL;
            stored.remoteAddress = REMOTE;
            stored.macAddress = "AA:BB:CC:DD:EE:01";
            stored.serverCert = certificate;
            ComputerDatabaseManager saved = new ComputerDatabaseManager(context);
            saved.updateComputer(stored);
            saved.close();

            controller = Robolectric.buildService(AnsweringManager.class).create();
            service = controller.get();
            ComputerDatabaseManager own = ReflectionHelpers.getField(service, "dbManager");
            own.close();
            database = new CountingDatabase(service);
            ReflectionHelpers.setField(service, "dbManager", database);
            List<ComputerManagerService.TrackedHost> tuples = ReflectionHelpers.getField(service, "pollingTuples");
            tuple = tuples.get(0);
            assertEquals(UUID, tuple.computer.uuid);
        }

        void poll() {
            ReflectionHelpers.callInstanceMethod(service, "runPoll",
                    ReflectionHelpers.ClassParameter.from(ComputerDetails.class, tuple.computer),
                    ReflectionHelpers.ClassParameter.from(ComputerManagerService.TrackedHost.class, tuple),
                    ReflectionHelpers.ClassParameter.from(int.class, 0));
        }

        ComputerDetails saved() {
            return database.getComputerByUUID(UUID);
        }

        @Override public void close() {
            controller.destroy();
        }
    }

    /** A certificate that is not the fixture's: the first one this JVM trusts. */
    private static X509Certificate otherCertificate() throws Exception {
        TrustManagerFactory factory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
        factory.init((KeyStore) null);
        for (TrustManager manager : factory.getTrustManagers()) {
            if (manager instanceof X509TrustManager) {
                return ((X509TrustManager) manager).getAcceptedIssuers()[0];
            }
        }
        throw new AssertionError("No trusted certificate");
    }

    @Test
    public void aPollThatFindsThePcAsItIsSavedWritesNothing() {
        try (Polls polls = new Polls(NativeApiFixtures.certificate())) {
            Object[] before = ComputerDatabaseManager.columns(polls.saved());
            for (int i = 0; i < 5; i++) {
                polls.poll();
            }
            assertEquals(Collections.emptyList(), polls.database.written);
            assertTrue(Arrays.deepEquals(before, ComputerDatabaseManager.columns(polls.saved())));
            assertEquals(ComputerDetails.State.ONLINE, polls.tuple.computer.state);
        }
    }

    // Each thing the database keeps of a PC, changed one after the other. The poll that finds the
    // change writes the PC once, and the polls after it find the PC as saved again.
    @Test
    public void aPollThatFindsAnythingSavedChangedWritesIt() throws Exception {
        X509Certificate pairedAgain = otherCertificate();
        assertNotEquals(NativeApiFixtures.certificate(), pairedAgain);
        try (Polls polls = new Polls(NativeApiFixtures.certificate())) {
            int writes = 0;

            polls.service.answer = polled -> polled.name = "DEN-PC";
            polls.poll();
            assertEquals("name", ++writes, polls.database.written.size());
            assertEquals("DEN-PC", polls.saved().name);

            polls.service.answer = polls.service.answer.andThen(polled ->
                    polled.localAddress = new ComputerDetails.AddressTuple("192.168.1.10", 47989));
            polls.poll();
            assertEquals("local address", ++writes, polls.database.written.size());
            assertEquals(new ComputerDetails.AddressTuple("192.168.1.10", 47989), polls.saved().localAddress);

            // The host's external port goes into the remote address that was saved.
            polls.service.answer = polls.service.answer.andThen(polled -> polled.externalPort = 48989);
            polls.poll();
            assertEquals("remote port", ++writes, polls.database.written.size());
            assertEquals(new ComputerDetails.AddressTuple("203.0.113.9", 48989), polls.saved().remoteAddress);

            polls.service.answer = polls.service.answer.andThen(polled ->
                    polled.ipv6Address = new ComputerDetails.AddressTuple("2001:db8::9", 47989));
            polls.poll();
            assertEquals("IPv6 address", ++writes, polls.database.written.size());
            assertEquals(new ComputerDetails.AddressTuple("2001:db8::9", 47989), polls.saved().ipv6Address);

            polls.service.answer = polls.service.answer.andThen(polled -> polled.macAddress = "AA:BB:CC:DD:EE:02");
            polls.poll();
            assertEquals("MAC address", ++writes, polls.database.written.size());
            assertEquals("AA:BB:CC:DD:EE:02", polls.saved().macAddress);

            // An address the user typed and a certificate from a pairing are set on the PC that is
            // polled, and the next poll is what saves them.
            polls.tuple.computer.manualAddress = new ComputerDetails.AddressTuple("den.example", 47989);
            polls.poll();
            assertEquals("manual address", ++writes, polls.database.written.size());
            assertEquals(new ComputerDetails.AddressTuple("den.example", 47989), polls.saved().manualAddress);

            polls.tuple.computer.serverCert = pairedAgain;
            polls.poll();
            assertEquals("certificate", ++writes, polls.database.written.size());
            assertEquals(pairedAgain, polls.saved().serverCert);

            // Every change is saved and the PC answers the same from here on.
            for (int i = 0; i < 3; i++) {
                polls.poll();
            }
            assertEquals(writes, polls.database.written.size());
            ComputerDetails saved = polls.saved();
            assertEquals("DEN-PC", saved.name);
            assertEquals(new ComputerDetails.AddressTuple("192.168.1.10", 47989), saved.localAddress);
            assertEquals(new ComputerDetails.AddressTuple("203.0.113.9", 48989), saved.remoteAddress);
            assertEquals(new ComputerDetails.AddressTuple("den.example", 47989), saved.manualAddress);
            assertEquals(new ComputerDetails.AddressTuple("2001:db8::9", 47989), saved.ipv6Address);
            assertEquals("AA:BB:CC:DD:EE:02", saved.macAddress);
            assertEquals(pairedAgain, saved.serverCert);
        }
    }

    @Test
    public void aPairingMadeSinceTheLastPollIsSavedByTheNextOne() {
        try (Polls polls = new Polls(null)) {
            polls.poll();
            assertEquals(Collections.emptyList(), polls.database.written);
            assertNull(polls.saved().serverCert);

            polls.tuple.computer.serverCert = NativeApiFixtures.certificate();
            polls.poll();
            assertEquals(1, polls.database.written.size());
            assertEquals(NativeApiFixtures.certificate(), polls.saved().serverCert);
        }
    }

    @Test
    public void aPcThatSaysNotPairedHasItsCertificateForgottenOnceAndIsThenLeftAlone() {
        try (Polls polls = new Polls(NativeApiFixtures.certificate())) {
            polls.service.answer = polled -> polled.pairState = PairingManager.PairState.NOT_PAIRED;
            polls.poll();
            assertEquals(1, polls.database.written.size());
            assertNull(polls.saved().serverCert);
            assertNull(polls.tuple.computer.serverCert);

            polls.poll();
            polls.poll();
            assertEquals(1, polls.database.written.size());
            assertEquals("the rest of the PC stays", REMOTE, polls.saved().remoteAddress);
        }
    }

    // The poll reads the PC, then the user unpairs it, then the poll decides what to save. It learned
    // nothing new, so it leaves the row as the unpairing wrote it and no longer puts back what it read.
    @Test
    public void aPairingForgottenWhileAPollRunsStaysForgotten() {
        try (Polls polls = new Polls(NativeApiFixtures.certificate())) {
            polls.database.afterNextRead = () -> polls.service.forgetCertificate(UUID);
            polls.poll();
            assertNull(polls.saved().serverCert);
            assertEquals("Only the unpairing wrote", 1, polls.database.written.size());
        }
    }

    @Test
    public void aPcSeenForTheFirstTimeIsSaved() {
        try (Polls polls = new Polls(null)) {
            ComputerDetails found = new ComputerDetails();
            found.uuid = "new-host";
            found.remoteAddress = REMOTE;
            ReflectionHelpers.callInstanceMethod(polls.service, "runPoll",
                    ReflectionHelpers.ClassParameter.from(ComputerDetails.class, found),
                    ReflectionHelpers.ClassParameter.from(ComputerManagerService.TrackedHost.class, null),
                    ReflectionHelpers.ClassParameter.from(int.class, 0));
            assertEquals(Collections.singletonList("new-host"), polls.database.written);
            ComputerDetails saved = polls.database.getComputerByUUID("new-host");
            assertEquals("PC", saved.name);
            assertEquals(LOCAL, saved.localAddress);
        }
    }

    // What decides whether a poll writes is ComputerDatabaseManager.columns(). Every persistent field
    // saved into hosts.json has to be compared there too, or a poll that changes only that field saves nothing.
    @Test
    public void everyColumnAndAddressOfTheTableIsCompared() throws Exception {
        try (Polls polls = new Polls(NativeApiFixtures.certificate())) {
            polls.tuple.computer.manualAddress = new ComputerDetails.AddressTuple("den.example", 47989);
            polls.service.answer = polled -> polled.ipv6Address = new ComputerDetails.AddressTuple("2001:db8::9", 47989);
            polls.poll();

            File hostsFile = new File(polls.context.getFilesDir(), "hosts.json");
            assertTrue(hostsFile.isFile());
            String json = new String(Files.readAllBytes(hostsFile.toPath()), StandardCharsets.UTF_8);
            JSONArray entries = new JSONArray(json);
            assertEquals(1, entries.length());
            JSONObject saved = entries.getJSONObject(0);
            List<String> fields = new ArrayList<>();
            for (java.util.Iterator<String> keys = saved.keys(); keys.hasNext(); ) {
                String k = keys.next();
                if (!saved.isNull(k)) {
                    fields.add(k);
                }
            }
            Collections.sort(fields);
            String hint = "ComputerDatabaseManager.columns() has to compare what is new here";
            assertTrue(hint + ": " + fields, fields.containsAll(Arrays.asList(
                    "ipv6_endpoint", "local_endpoint", "mac", "manual_endpoint",
                    "name", "remote_endpoint", "server_certificate", "unique_id")));
            // The UUID, the name, the four addresses, the MAC address and the certificate.
            assertEquals(hint, 8, ComputerDatabaseManager.columns(polls.saved()).length);
        }
    }

    @Test
    public void legacySqliteMigratesOnceAndDeletedHostDoesNotResurrect() throws Exception {
        Context context = RuntimeEnvironment.getApplication();
        File dbFile = context.getDatabasePath("computers4.db");
        dbFile.getParentFile().mkdirs();
        try (SQLiteDatabase db = SQLiteDatabase.openOrCreateDatabase(dbFile, null)) {
            db.execSQL("CREATE TABLE Computers (UUID TEXT PRIMARY KEY, ComputerName TEXT NOT NULL, "
                    + "Addresses TEXT NOT NULL, MacAddress TEXT, ServerCert BLOB)");
            db.execSQL("INSERT INTO Computers (UUID, ComputerName, Addresses, MacAddress, ServerCert) VALUES (?, ?, ?, ?, ?)",
                    new Object[] {
                            "legacy-pc",
                            "Legacy PC",
                            "{\"local\":{\"address\":\"192.168.1.50\",\"port\":47989}}",
                            "11:22:33:44:55:66",
                            NativeApiFixtures.certificate().getEncoded()
                    });
        }

        ComputerDatabaseManager first = new ComputerDatabaseManager(context);
        ComputerDetails migrated = first.getComputerByUUID("legacy-pc");
        assertNotNull(migrated);
        assertEquals("Legacy PC", migrated.name);
        assertEquals(new ComputerDetails.AddressTuple("192.168.1.50", 47989), migrated.localAddress);
        assertEquals("11:22:33:44:55:66", migrated.macAddress);
        assertEquals(NativeApiFixtures.certificate(), migrated.serverCert);
        assertTrue(new File(context.getFilesDir(), "hosts.json").isFile());

        first.deleteComputer(migrated);
        assertNull(first.getComputerByUUID("legacy-pc"));
        first.close();
        ComputerDatabaseManager.resetForTests();

        assertTrue("legacy SQLite remains intact until future cleanup", dbFile.exists());
        ComputerDatabaseManager second = new ComputerDatabaseManager(context);
        assertNull("deleted host must never resurrect from legacy computers4.db", second.getComputerByUUID("legacy-pc"));
        second.close();
    }

    @Test
    public void updateComputerReturnsTrueOnlyWhenChangedAndDoesNotFabricateLocalAddress() {
        Context context = RuntimeEnvironment.getApplication();
        ComputerDatabaseManager db = new ComputerDatabaseManager(context);

        ComputerDetails manualOnly = new ComputerDetails();
        manualOnly.uuid = "manual-only";
        manualOnly.name = "ManualPC";
        manualOnly.manualAddress = new ComputerDetails.AddressTuple("my-pc.example", 47989);
        assertTrue(db.updateComputer(manualOnly));
        assertFalse("identical upsert returns false", db.updateComputer(manualOnly));
        ComputerDetails manualRead = db.getComputerByUUID("manual-only");
        assertNotNull(manualRead);
        assertNull("manual-only host must not fabricate localAddress", manualRead.localAddress);
        assertEquals(manualOnly.manualAddress, manualRead.manualAddress);

        ComputerDetails remoteOnly = new ComputerDetails();
        remoteOnly.uuid = "remote-only";
        remoteOnly.name = "RemotePC";
        remoteOnly.remoteAddress = new ComputerDetails.AddressTuple("203.0.113.55", 48989);
        assertTrue(db.updateComputer(remoteOnly));
        assertFalse(db.updateComputer(remoteOnly));
        ComputerDetails remoteRead = db.getComputerByUUID("remote-only");
        assertNotNull(remoteRead);
        assertNull("remote-only host must not fabricate localAddress", remoteRead.localAddress);
        assertEquals(remoteOnly.remoteAddress, remoteRead.remoteAddress);

        ComputerDetails ipv6Only = new ComputerDetails();
        ipv6Only.uuid = "ipv6-only";
        ipv6Only.name = "V6PC";
        ipv6Only.ipv6Address = new ComputerDetails.AddressTuple("2001:db8::1", 47989);
        assertTrue(db.updateComputer(ipv6Only));
        ComputerDetails ipv6Read = db.getComputerByUUID("ipv6-only");
        assertNotNull(ipv6Read);
        assertNull("ipv6-only host must not fabricate localAddress", ipv6Read.localAddress);
        assertEquals(ipv6Only.ipv6Address, ipv6Read.ipv6Address);

        // Clearing localAddress on an existing host with remoteAddress leaves localAddress null.
        ComputerDetails cleared = new ComputerDetails();
        cleared.uuid = "local-cleared";
        cleared.name = "ClearedPC";
        cleared.localAddress = new ComputerDetails.AddressTuple("192.168.1.10", 47989);
        cleared.remoteAddress = new ComputerDetails.AddressTuple("203.0.113.99", 47989);
        assertTrue(db.updateComputer(cleared));
        cleared.localAddress = null;
        assertTrue(db.updateComputer(cleared));
        assertFalse(db.updateComputer(cleared));
        ComputerDetails clearedRead = db.getComputerByUUID("local-cleared");
        assertNotNull(clearedRead);
        assertNull("cleared localAddress must stay null", clearedRead.localAddress);
        assertEquals(cleared.remoteAddress, clearedRead.remoteAddress);

        db.close();
    }

    @Test
    public void legacyComputersDbSurvivesWhenHostsJsonIsUnreadableDirectory() {
        Context context = RuntimeEnvironment.getApplication();
        File v1Db = context.getDatabasePath("computers.db");
        v1Db.getParentFile().mkdirs();
        try (SQLiteDatabase db = SQLiteDatabase.openOrCreateDatabase(v1Db, null)) {
            db.execSQL("CREATE TABLE Computers (ComputerName TEXT, UUID TEXT, LocalAddress TEXT, RemoteAddress TEXT, MacAddress TEXT)");
            db.execSQL("INSERT INTO Computers (ComputerName, UUID, LocalAddress, RemoteAddress, MacAddress) VALUES (?, ?, ?, ?, ?)",
                    new Object[] {"LegacyV1", "v1-uuid", "ADDRESS_PREFIX__192.168.1.40", null, "00:11:22:33:44:55"});
        }

        File hostsDir = new File(context.getFilesDir(), "hosts.json");
        assertTrue(hostsDir.mkdirs());

        ComputerDatabaseManager mgr = new ComputerDatabaseManager(context);
        mgr.close();

        assertTrue("computers.db must survive when hosts.json cannot be written", v1Db.exists());
        assertTrue(hostsDir.isDirectory());
        assertFalse(new File(context.getFilesDir(), ".hosts_migrated").exists());
    }
}
