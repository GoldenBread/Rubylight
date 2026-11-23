package com.limelight.nvstream.http;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.nativeapi.NativeApiFixtures;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

/** Shared folders over real TLS loopback and nightjar-core folders.rs JNI. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class SharedFoldersTest {
    private final List<TestHttpServer> servers = new ArrayList<>();
    private final List<TestHttpServer.RecordedRequest> requests = new ArrayList<>();
    private final List<byte[]> uploaded = new ArrayList<>();

    @After
    public void closeServers() throws IOException {
        for (TestHttpServer s : servers) {
            s.close();
        }
        servers.clear();
    }

    private SharedFolders folders(TestHttpServer.Handler host) throws Exception {
        TestHttpServer server = new TestHttpServer(req -> {
            requests.add(req);
            if (req.body.length > 0) {
                uploaded.add(req.body);
            }
            return host.handle(req);
        });
        servers.add(server);
        NvHTTP http = server.newClient("1234567890abcdef", true, NativeApiFixtures.crypto());
        return new SharedFolders(http);
    }

    private static TestHttpServer.Handler answer(int code, String json) {
        return req -> TestHttpServer.Response.json(code, json);
    }

    private static String refusal(String code) {
        return "{\"error\":{\"code\":\"" + code + "\",\"message\":\"refused\"}}";
    }

    @Test
    public void theFolderListAsksTheFoldersRouteAndReadsTheGrants() throws Exception {
        List<SharedFolders.Folder> list = folders(answer(200, "{\"folders\":[{\"id\":\"games\",\"name\":\"Games\","
                + "\"access\":\"write\"},{\"id\":\"music\",\"name\":\"Music\",\"access\":\"read\"},"
                + "{\"id\":\"new\",\"name\":\"New\",\"access\":\"append\"}]}")).folders();
        assertEquals(3, list.size());
        assertEquals("games", list.get(0).id);
        assertEquals("Games", list.get(0).name);
        assertTrue(list.get(0).writable);
        assertFalse(list.get(1).writable);
        assertFalse(list.get(2).writable);
        TestHttpServer.RecordedRequest request = requests.get(0);
        assertEquals("GET", request.method);
        assertEquals("/api/native/v1/folders", request.path);
        assertEquals("1234567890abcdef", request.queryParam("uniqueid"));
        assertNull(request.queryParam("path"));
    }

    @Test
    public void aListingIsReadStrictly() throws Exception {
        SharedFolders.Listing listing = folders(answer(200, "{\"path\":\"Saves\",\"entries\":["
                + "{\"name\":\"Slot 1\",\"kind\":\"dir\",\"size\":0,\"modified_unix_ms\":1700000000000},"
                + "{\"name\":\"a&b.sav\",\"kind\":\"file\",\"size\":5000000000,\"modified_unix_ms\":0},"
                + "{\"name\":\"link\",\"kind\":\"other\",\"size\":0,\"modified_unix_ms\":0}]}")).entries("games", "Saves");
        assertEquals("Saves", listing.path);
        assertEquals(SharedFolders.Kind.DIR, listing.entries.get(0).kind);
        assertEquals(1700000000000L, listing.entries.get(0).modifiedUnixMs);
        assertEquals(SharedFolders.Kind.FILE, listing.entries.get(1).kind);
        assertEquals(5000000000L, listing.entries.get(1).size);
        assertEquals(SharedFolders.Kind.UNKNOWN, listing.entries.get(2).kind);
        assertEquals("/api/native/v1/folders/games/entries", requests.get(0).path);
        assertEquals("Saves", requests.get(0).queryParam("path"));
        for (String bad : new String[] {"{\"entries\":[]}", "{\"path\":\"\",\"entries\":[{\"name\":\"a\",\"kind\":\"file\","
                + "\"size\":\"5\",\"modified_unix_ms\":0}]}", "{\"path\":\"\",\"entries\":[{\"name\":\"a\",\"kind\":\"file\","
                + "\"size\":-1,\"modified_unix_ms\":0}]}", "{\"path\":\"\",\"entries\":[1]}", "[]"}) {
            assertThrows(bad, IOException.class, () -> folders(answer(200, bad)).entries("games", ""));
        }
        assertThrows(IOException.class, () -> folders(answer(200,
                "{\"folders\":[{\"id\":1,\"name\":\"a\",\"access\":\"read\"}]}")).folders());
    }

    @Test
    public void anAnswerOverItsLimitIsRefused() throws Exception {
        String big = "{\"folders\":[],\"pad\":\"" + "a".repeat(SharedFolders.MAX_JSON_BYTES) + "\"}";
        assertThrows(IOException.class, () -> folders(answer(200, big)).folders());
    }

    @Test
    public void eachDocumentedRefusalIsKnownByStatusAndCode() throws Exception {
        for (SharedFolders.Refusal expected : SharedFolders.Refusal.values()) {
            SharedFolders.RefusedException refused = assertThrows(SharedFolders.RefusedException.class,
                    () -> folders(answer(expected.status, refusal(expected.code))).folders());
            assertEquals(expected, refused.refusal);
            assertEquals(expected.status, refused.status);
        }
        for (TestHttpServer.Handler other : new TestHttpServer.Handler[] {
                answer(409, refusal("forbidden")), answer(403, refusal("new_reason")),
                req -> TestHttpServer.Response.bytes(500, "text/html", "<html>".getBytes()),
                answer(204, "")}) {
            IOException failure = assertThrows(IOException.class, () -> folders(other).folders());
            assertTrue(failure.toString(), failure instanceof HostHttpResponseException);
        }
    }

    @Test
    public void aDownloadWritesTheWholeFileAndReportsProgress() throws Exception {
        ByteArrayOutputStream sink = new ByteArrayOutputStream();
        List<long[]> progress = new ArrayList<>();
        long length = folders(req -> TestHttpServer.Response.bytes(
                200, "application/octet-stream", "hello".getBytes(StandardCharsets.UTF_8)))
                .download("games", "Saves/a b.sav", sink,
                        (done, total) -> progress.add(new long[] {done, total}), new TransferCancel());
        assertEquals(5, length);
        assertEquals("hello", sink.toString("UTF-8"));
        assertEquals(5, progress.get(progress.size() - 1)[0]);
        assertEquals(5, progress.get(progress.size() - 1)[1]);
        assertEquals("/api/native/v1/folders/games/content", requests.get(0).path);
        assertTrue(requests.get(0).rawQuery,
                requests.get(0).rawQuery.startsWith("path=Saves%2Fa%20b.sav"));
    }

    @Test
    public void aHostThatSendsMoreThanItListedIsStoppedAtTheListedSize() throws Exception {
        ByteArrayOutputStream sink = new ByteArrayOutputStream();
        IOException failure = assertThrows(IOException.class, () -> folders(req -> TestHttpServer.Response.bytes(
                200, "application/octet-stream", "hello, and more".getBytes(StandardCharsets.UTF_8)))
                .download("games", "a.sav", 5, sink, (done, total) -> { }, new TransferCancel()));
        assertFalse(failure instanceof SharedFolders.LocalFileException);
        assertTrue(failure.getMessage(), failure.getMessage().contains("more than the 5 bytes it listed"));
        assertEquals("nothing past the listed size is written", 0, sink.size());
        assertEquals(5, folders(req -> TestHttpServer.Response.bytes(
                200, "application/octet-stream", "hello".getBytes(StandardCharsets.UTF_8)))
                .download("games", "a.sav", 5, new ByteArrayOutputStream(),
                        (done, total) -> { }, new TransferCancel()));
    }

    @Test
    public void aShortDownloadFails() throws Exception {
        IOException failure = assertThrows(IOException.class, () -> folders(req -> TestHttpServer.Response.bytes(
                200, "application/octet-stream", "hello".getBytes(StandardCharsets.UTF_8))
                .declaredContentLength(10))
                .download("games", "a.sav", new ByteArrayOutputStream(), (done, total) -> { }, new TransferCancel()));
        assertFalse(failure instanceof SharedFolders.LocalFileException);
        assertTrue(failure.getMessage(), failure.getMessage().contains("5 of 10"));
    }

    @Test
    public void aRefusedDownloadWritesNothing() throws Exception {
        ByteArrayOutputStream sink = new ByteArrayOutputStream();
        assertEquals(SharedFolders.Refusal.TRANSFER_BUSY, assertThrows(SharedFolders.RefusedException.class,
                () -> folders(answer(409, refusal("transfer_busy"))).download("games", "a.sav", sink,
                        (done, total) -> { }, new TransferCancel())).refusal);
        assertEquals(0, sink.size());
    }

    @Test
    public void aCancelledTransferSaysSo() throws Exception {
        TransferCancel cancel = new TransferCancel();
        cancel.cancel();
        assertThrows(SharedFolders.CancelledException.class, () -> folders(answer(200, "")).download("games", "a.sav",
                new ByteArrayOutputStream(), (done, total) -> { }, cancel));
        assertThrows(SharedFolders.CancelledException.class, () -> folders(answer(201, "")).upload("games", "a.sav",
                new ByteArrayInputStream(new byte[1]), 1, (done, total) -> { }, cancel));
    }

    @Test
    public void anUploadPutsItsLengthAndReturnsTheStoredName() throws Exception {
        byte[] file = "hello".getBytes(StandardCharsets.UTF_8);
        String stored = folders(answer(201, "{\"name\":\"a (1).txt\",\"size\":5}")).upload("games", "Docs/a.txt",
                new ByteArrayInputStream(file), file.length, (done, total) -> { }, new TransferCancel());
        assertEquals("a (1).txt", stored);
        TestHttpServer.RecordedRequest request = requests.get(0);
        assertEquals("PUT", request.method);
        assertEquals("/api/native/v1/folders/games/content", request.path);
        assertEquals("Docs/a.txt", request.queryParam("path"));
        assertEquals("5", request.header("content-length"));
        assertEquals("application/octet-stream", request.header("content-type"));
        assertEquals("hello", new String(uploaded.get(0), StandardCharsets.UTF_8));
    }

    @Test
    public void anUploadTheHostStoredShortOrAnsweredWithoutCreatingFails() throws Exception {
        byte[] file = new byte[5];
        assertThrows(IOException.class, () -> folders(answer(201, "{\"name\":\"a.txt\",\"size\":4}")).upload("games",
                "a.txt", new ByteArrayInputStream(file), 5, (done, total) -> { }, new TransferCancel()));
        assertThrows(HostHttpResponseException.class, () -> folders(answer(200, "{\"name\":\"a.txt\",\"size\":5}"))
                .upload("games", "a.txt", new ByteArrayInputStream(file), 5, (done, total) -> { }, new TransferCancel()));
    }

    @Test
    public void aFileShorterThanItsLengthFailsOnThisDevice() throws Exception {
        assertThrows(SharedFolders.LocalFileException.class, () -> folders(answer(201, "{\"name\":\"a.txt\",\"size\":10}"))
                .upload("games", "a.txt", new ByteArrayInputStream(new byte[5]), 10, (done, total) -> { },
                        new TransferCancel()));
    }

    @Test
    public void aFileThatGrewSinceItsLengthWasReadFailsBeforeItsLastBytesGo() throws Exception {
        for (int length : new int[] {5, 0, 64 * 1024, 64 * 1024 + 3}) {
            long[] sent = {0};
            SharedFolders.LocalFileException changed = assertThrows(SharedFolders.LocalFileException.class,
                    () -> folders(answer(201, "{\"name\":\"a.txt\",\"size\":" + length + "}")).upload("games", "a.txt",
                            new ByteArrayInputStream(new byte[length + 1]), length, (done, total) -> sent[0] = done,
                            new TransferCancel()));
            assertTrue(changed.getMessage(), changed.getMessage().contains("changed during the upload"));
            assertTrue(length + ": sent " + sent[0], length == 0 || sent[0] < length);
        }
    }

    @Test
    public void namesThatCannotTravelAreRefusedBeforeAnyRequest() throws Exception {
        SharedFolders folders = folders(answer(201, "{}"));
        for (String name : new String[] {"a.part", "Docs/b.PART", "", "Docs/"}) {
            assertThrows(name, IllegalArgumentException.class, () -> folders.upload("games", name,
                    new ByteArrayInputStream(new byte[0]), 0, (done, total) -> { }, new TransferCancel()));
        }
        for (String id : new String[] {"", "a/b", "a b", "ü", "a".repeat(65)}) {
            assertThrows(id, IllegalArgumentException.class, () -> folders.entries(id, ""));
        }
        for (String path : new String[] {"/a", "a//b", "a/", String.join("/", java.util.Collections.nCopies(65, "a"))}) {
            assertThrows(path, IllegalArgumentException.class, () -> folders.entries("games", path));
        }
        assertThrows(IllegalArgumentException.class, () -> folders.download("games", "", new ByteArrayOutputStream(),
                (done, total) -> { }, new TransferCancel()));
        assertTrue(requests.isEmpty());
        SharedFolders.checkId("Games_2-x");
        SharedFolders.checkPath(String.join("/", java.util.Collections.nCopies(64, "a")));
    }

    @Test
    public void pathsJoinAndSplitByName() throws Exception {
        assertEquals("a", SharedFolders.childPath("", "a"));
        assertEquals("a/b", SharedFolders.childPath("a", "b"));
        assertEquals("", SharedFolders.parentPath("a"));
        assertEquals("a/b", SharedFolders.parentPath("a/b/c"));
        assertEquals("c.txt", SharedFolders.fileName("a/b/c.txt"));
        assertTrue(SharedFolders.hasPartSuffix("x.Part"));
        assertFalse(SharedFolders.hasPartSuffix("part"));
        assertFalse(SharedFolders.hasPartSuffix("x.parts"));
        folders(answer(200, "{\"path\":\"a b/c&d=\",\"entries\":[]}")).entries("games", "a b/c&d=");
        assertTrue(requests.get(0).rawQuery,
                requests.get(0).rawQuery.startsWith("path=a%20b%2Fc%26d%3D"));
    }
}
