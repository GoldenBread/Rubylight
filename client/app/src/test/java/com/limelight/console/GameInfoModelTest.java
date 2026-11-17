package com.limelight.console;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;


import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * The wire contract with libnightjar_core (protocol/android/src/game_info_contract.json, which the Rust side's test
 * checks too), the driver loop with a fake transport, and the Genre and Length choices. No test touches the network.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameInfoModelTest {
    @After
    public void forget() {
        GameInfoModel.use(null);
    }

    private static JSONArray cases() throws Exception {
        for (String path : new String[] {"../protocol/android/src/game_info_contract.json", "protocol/android/src/game_info_contract.json"}) {
            File file = new File(path);
            if (file.isFile()) return new JSONArray(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        }
        throw new AssertionError("game_info_contract.json not found from " + new File(".").getAbsolutePath());
    }

    private static JSONObject caseOf(String op, int nth) throws Exception {
        JSONArray all = cases();
        int seen = 0;
        for (int i = 0; i < all.length(); i++) {
            JSONObject c = all.getJSONObject(i);
            if (op.equals(c.getJSONObject("request").getString("op")) && seen++ == nth) return c;
        }
        throw new AssertionError("no " + op + " case " + nth);
    }

    /** Answers with the contract case's response and keeps the request it was sent. */
    private static final class Fixed implements GameInfoModel.Calls {
        final JSONObject response;
        JSONObject sent;

        Fixed(JSONObject contractCase) throws Exception {
            response = contractCase.getJSONObject("response");
        }

        @Override
        public String call(String request) {
            try {
                sent = new JSONObject(request);
            } catch (Exception e) {
                throw new AssertionError(e);
            }
            return response.toString();
        }
    }

    private static boolean same(Object a, Object b) throws Exception {
        if (a instanceof JSONObject && b instanceof JSONObject) {
            JSONObject x = (JSONObject) a;
            JSONObject y = (JSONObject) b;
            if (x.length() != y.length()) return false;
            for (java.util.Iterator<String> it = x.keys(); it.hasNext(); ) {
                String k = it.next();
                if (!y.has(k) || !same(x.get(k), y.get(k))) return false;
            }
            return true;
        }
        if (a instanceof JSONArray && b instanceof JSONArray) {
            JSONArray x = (JSONArray) a;
            JSONArray y = (JSONArray) b;
            if (x.length() != y.length()) return false;
            for (int i = 0; i < x.length(); i++) if (!same(x.get(i), y.get(i))) return false;
            return true;
        }
        if (JSONObject.NULL.equals(a) || JSONObject.NULL.equals(b)) return JSONObject.NULL.equals(a) && JSONObject.NULL.equals(b);
        return a.equals(b);
    }

    private static void assertSent(JSONObject expected, JSONObject sent) throws Exception {
        assertTrue("sent " + sent + " but the contract says " + expected, same(expected, sent));
    }

    @Test
    public void cleanSendsTheContractsRequestAndReadsItsAnswer() throws Exception {
        for (int nth = 0; nth < 3; nth++) {
            JSONObject c = caseOf("clean", nth);
            Fixed calls = new Fixed(c);
            JSONObject request = c.getJSONObject("request");
            GameInfoModel.Cleaned cleaned = GameInfoModel.clean(calls, request.getString("name"),
                    request.isNull("game_id") ? null : request.getString("game_id"),
                    request.has("store") ? request.getJSONObject("store").getString("source") : null,
                    request.has("store") ? request.getJSONObject("store").getString("id") : null);
            assertSent(request, calls.sent);
            JSONObject ok = c.getJSONObject("response").getJSONObject("ok");
            assertEquals(ok.getString("clean"), cleaned.clean);
            assertEquals(ok.getString("key"), cleaned.key);
            assertEquals(ok.isNull("steam") ? null : (Long) ok.getLong("steam"), cleaned.steam);
        }
    }

    @Test
    public void freshSendsOnlyTheKeysOfTheServicesWanted() throws Exception {
        JSONObject c = caseOf("fresh", 0);
        Fixed calls = new Fixed(c);
        JSONObject request = c.getJSONObject("request");
        GameInfoModel.Keys keys = new GameInfoModel.Keys("k", "i", "s");
        boolean fresh = GameInfoModel.fresh(calls, request.getJSONObject("record"), request.getLong("now_secs"), keys, true, false);
        assertTrue(fresh);
        assertEquals("k", calls.sent.getJSONObject("keys").getString("steamgriddb"));
        // IGDB is not wanted: its keys go blank, so a record that skipped it holds.
        assertEquals("", calls.sent.getJSONObject("keys").getString("igdb_client_id"));
        assertEquals("", calls.sent.getJSONObject("keys").getString("igdb_client_secret"));
        assertEquals(request.getJSONObject("record").toString().length(), calls.sent.getJSONObject("record").toString().length());

        JSONObject added = caseOf("fresh", 1);
        Fixed stale = new Fixed(added);
        assertFalse(GameInfoModel.fresh(stale, added.getJSONObject("request").getJSONObject("record"), 2000, keys, true, true));
        assertSent(added.getJSONObject("request"), stale.sent);
    }

    @Test
    public void libraryFactsFilesAndMergeSendTheContractsRequests() throws Exception {
        JSONObject library = caseOf("library", 0);
        Fixed calls = new Fixed(library);
        JSONObject request = library.getJSONObject("request");
        List<JSONObject> games = new ArrayList<>();
        for (int i = 0; i < request.getJSONArray("games").length(); i++) {
            games.add(request.getJSONArray("games").isNull(i) ? null : request.getJSONArray("games").getJSONObject(i));
        }
        GameInfoModel.Library answer = GameInfoModel.library(calls, games, request.getString("genre"), null);
        assertSent(request, calls.sent);
        assertEquals(2, answer.genres.size());
        assertEquals("Puzzle", answer.genres.get(0).value);
        assertArrayEquals(new int[] {0}, answer.shown);
        assertTrue(answer.any());

        JSONObject facts = caseOf("facts", 0);
        Fixed factCalls = new Fixed(facts);
        GameInfoModel.TimeToBeat time = GameInfoModel.timeToBeat(factCalls, facts.getJSONObject("request").getJSONObject("facts"));
        assertSent(facts.getJSONObject("request"), factCalls.sent);
        assertEquals("medium", time.length);
        assertEquals("1.5", time.hastily);
        assertNull(time.completely);

        JSONObject files = caseOf("files", 0);
        Fixed fileCalls = new Fixed(files);
        String[] names = GameInfoModel.files(fileCalls, "steam:1", "https://cdn2.steamgriddb.com/grid/a.png");
        assertSent(files.getJSONObject("request"), fileCalls.sent);
        assertEquals("20134e0ee372376e.json", names[0]);
        assertEquals("148fdc164de6e06e.png", names[1]);

        JSONObject merge = caseOf("merge", 0);
        Fixed mergeCalls = new Fixed(merge);
        JSONObject request2 = merge.getJSONObject("request");
        GameInfoModel.merge(mergeCalls, request2.getJSONObject("record"), request2.isNull("older") ? null : request2.getJSONObject("older"));
        assertSent(request2, mergeCalls.sent);
    }

    private static void assertArrayEquals(int[] expected, int[] actual) {
        assertEquals(Arrays.toString(expected), Arrays.toString(actual));
    }

    /**
     * A stand-in for the library: the lookup it plans is a script of steps, and each reply the app gives is kept.
     * Everything else answers the way the real one does for these tests.
     */
    private static final class FakeLibrary implements GameInfoModel.Calls {
        final List<JSONObject> script = new ArrayList<>();
        final List<JSONObject> replies = new ArrayList<>();
        boolean freshAnswer;

        @Override
        public String call(String request) {
            try {
                JSONObject r = new JSONObject(request);
                JSONObject ok = new JSONObject();
                switch (r.getString("op")) {
                    case "clean":
                        ok.put("clean", r.getString("name")).put("key", "name:" + r.getString("name").toLowerCase()).put("steam", JSONObject.NULL);
                        break;
                    case "files":
                        ok.put("record", "record.json").put("image", r.isNull("url") ? JSONObject.NULL : "cover.png");
                        break;
                    case "fresh":
                        ok.put("fresh", freshAnswer);
                        break;
                    case "start":
                        ok.put("job", new JSONObject().put("at", 0)).put("step", script.get(0));
                        break;
                    case "next":
                        replies.add(r.isNull("reply") ? null : r.getJSONObject("reply"));
                        int at = r.getJSONObject("job").getInt("at") + 1;
                        ok.put("job", new JSONObject().put("at", at)).put("step", script.get(at));
                        break;
                    case "merge":
                        ok.put("record", r.getJSONObject("record"));
                        break;
                    default:
                        return new JSONObject().put("error", "invalid").toString();
                }
                return new JSONObject().put("ok", ok).toString();
            } catch (Exception e) {
                throw new AssertionError(e);
            }
        }
    }

    private static JSONObject send(String service) throws Exception {
        return new JSONObject().put("send", new JSONObject().put("service", service).put("method", "POST")
                .put("url", "https://example.invalid/x").put("headers", new JSONArray()).put("body", "fields name;"));
    }

    private static JSONObject done() throws Exception {
        JSONObject record = new JSONObject().put("key", "name:doom").put("fetched_at", 1000).put("art", "skipped").put("info", "found")
                .put("facts", new JSONObject().put("genres", new JSONArray(Collections.singletonList("Shooter"))).put("normally", 36000));
        return new JSONObject().put("done", new JSONObject().put("record", record).put("session", new JSONObject().put("token", "t"))
                .put("log", new JSONArray(Collections.singletonList("fine"))));
    }

    private static File temp() throws Exception {
        File dir = Files.createTempDirectory("game-info-test").toFile();
        dir.deleteOnExit();
        return dir;
    }

    private static final GameInfoModel.Keys IGDB = new GameInfoModel.Keys("", "id", "secret");
    private static final GameInfo.Game DOOM = new GameInfo.Game("Doom", null, null, null);

    @Test
    public void aFullLookupEndsInARecordOnDisk() throws Exception {
        FakeLibrary library = new FakeLibrary();
        library.script.add(send("igdb"));
        library.script.add(done());
        List<JSONObject> sent = new ArrayList<>();
        File dir = temp();
        GameInfo info = new GameInfo(dir, library, request -> {
            sent.add(request);
            return new GameInfoModel.Reply(200, "[]", null);
        }, () -> IGDB, false);

        GameInfo.Entry entry = info.resolve(DOOM, false, true);

        assertNotNull(entry);
        assertEquals(1, sent.size());
        File record = new File(dir, "record.json");
        assertTrue(record.isFile());
        JSONObject kept = new JSONObject(new String(Files.readAllBytes(record.toPath()), StandardCharsets.UTF_8));
        assertEquals("Shooter", kept.getJSONObject("facts").getJSONArray("genres").getString(0));
        assertEquals(200, library.replies.get(0).getInt("status"));

        // A fresh record is read from disk: no second request.
        library.freshAnswer = true;
        GameInfo again = new GameInfo(dir, library, request -> {
            throw new AssertionError("a fresh record needs no request");
        }, () -> IGDB, false);
        assertNotNull(again.resolve(DOOM, false, true).facts());
    }

    @Test
    public void aTooManyRequestsReplyCarriesItsRetryAfterAndTheLookupWaitsThenSucceeds() throws Exception {
        FakeLibrary library = new FakeLibrary();
        library.script.add(send("igdb"));
        library.script.add(new JSONObject().put("wait", 2000));
        library.script.add(send("igdb"));
        library.script.add(done());
        int[] calls = {0};
        List<Long> slept = new ArrayList<>();
        File dir = temp();
        GameInfo info = new GameInfo(dir, library, request -> new GameInfoModel.Reply(calls[0]++ == 0 ? 429 : 200, "[]", calls[0] == 1 ? 2L : null),
                () -> IGDB, false);
        info.setSleeper(slept::add);

        GameInfo.Entry entry = info.resolve(DOOM, false, true);

        assertNotNull(entry);
        assertEquals(Collections.singletonList(2000L), slept);
        assertEquals(429, library.replies.get(0).getInt("status"));
        assertEquals(2, library.replies.get(0).getInt("retry_after"));
        assertTrue(library.replies.get(1) == null);
        assertEquals(200, library.replies.get(2).getInt("status"));
        assertTrue(new File(dir, "record.json").isFile());
    }

    @Test
    public void noAnswerIsStatusZeroForTheLibrary() throws Exception {
        FakeLibrary library = new FakeLibrary();
        library.script.add(send("igdb"));
        library.script.add(done());
        GameInfo info = new GameInfo(temp(), library, request -> new GameInfoModel.Reply(0, "", null), () -> IGDB, false);
        assertNotNull(info.resolve(DOOM, false, true));
        assertEquals(0, library.replies.get(0).getInt("status"));
        assertTrue(library.replies.get(0).isNull("retry_after"));
    }

    @Test
    public void anIoErrorOfTheRealTransportIsStatusZero() throws Exception {
        GameInfo info = new GameInfo(temp(), new FakeLibrary(), null, () -> IGDB, false);
        // Nothing listens on port 1 of this machine: the connection is refused at once.
        GameInfoModel.Reply reply = info.send(new JSONObject().put("service", "igdb").put("method", "POST")
                .put("url", "http://127.0.0.1:1/x").put("headers", new JSONArray()).put("body", "x"));
        assertEquals(0, reply.status);
    }

    @Test
    public void withoutKeysNothingIsAskedAndNothingIsSent() throws Exception {
        FakeLibrary library = new FakeLibrary();
        GameInfo info = new GameInfo(temp(), library, request -> {
            throw new AssertionError("no key, no request");
        }, () -> new GameInfoModel.Keys("", "", ""), true);
        assertNull(info.resolve(DOOM, true, true));
        boolean[] called = {false};
        info.cover(DOOM, file -> called[0] = true);
        info.facts(DOOM, facts -> called[0] = true);
        info.prefetchFacts(Collections.singletonList(DOOM), () -> called[0] = true);
        Thread.sleep(100);
        assertFalse(called[0]);
        assertTrue(library.replies.isEmpty());

        Context context = org.robolectric.RuntimeEnvironment.getApplication();
        assertFalse(GameInfo.coverWanted(context));
        assertFalse(GameInfo.factsWanted(context));
        // Only the SteamGridDB key: cover lookups, no facts.
        assertNull(new GameInfo(temp(), library, request -> {
            throw new AssertionError("IGDB is off");
        }, () -> new GameInfoModel.Keys("k", "", ""), false).resolve(DOOM, false, true));
    }

    /** Answers the library op the way the real one does for two genres and lengths, by the games it is given. */
    private static final class FakeFilter implements GameInfoModel.Calls {
        @Override
        public String call(String request) {
            try {
                JSONObject r = new JSONObject(request);
                JSONArray games = r.getJSONArray("games");
                String genre = r.isNull("genre") ? null : r.getString("genre");
                String length = r.isNull("length") ? null : r.getString("length");
                JSONArray shown = new JSONArray();
                java.util.TreeMap<String, Integer> genres = new java.util.TreeMap<>();
                java.util.TreeMap<String, Integer> lengths = new java.util.TreeMap<>();
                for (int i = 0; i < games.length(); i++) {
                    if (games.isNull(i)) continue;
                    JSONObject g = games.getJSONObject(i);
                    String l = g.getLong("normally") < 18000 ? "short" : "medium";
                    boolean hasGenre = genre == null;
                    for (int j = 0; j < g.getJSONArray("genres").length(); j++) {
                        genres.merge(g.getJSONArray("genres").getString(j), 1, Integer::sum);
                        hasGenre |= g.getJSONArray("genres").getString(j).equals(genre);
                    }
                    lengths.merge(l, 1, Integer::sum);
                    if (hasGenre && (length == null || length.equals(l))) shown.put(i);
                }
                return new JSONObject().put("ok", new JSONObject().put("genres", counts(genres)).put("lengths", counts(lengths))
                        .put("shown", shown)).toString();
            } catch (Exception e) {
                throw new AssertionError(e);
            }
        }

        private static JSONArray counts(java.util.Map<String, Integer> map) throws Exception {
            JSONArray out = new JSONArray();
            for (java.util.Map.Entry<String, Integer> e : map.entrySet()) out.put(new JSONObject().put("value", e.getKey()).put("count", e.getValue()));
            return out;
        }
    }

    private static JSONObject facts(String genre, long seconds) throws Exception {
        return new JSONObject().put("genres", new JSONArray(Collections.singletonList(genre))).put("normally", seconds);
    }

    @Test
    public void genreLengthAndSourceChoicesCombine() throws Exception {
        // The source filter shows A, B and D; C is under another source. D has no facts.
        List<String> shownBySource = Arrays.asList("A", "B", "D");
        java.util.Map<String, JSONObject> facts = new java.util.HashMap<>();
        facts.put("A", facts("Shooter", 7200));
        facts.put("B", facts("Shooter", 36000));
        facts.put("C", facts("Puzzle", 7200));

        GameInfoFilter.Result<String> any = GameInfoFilter.apply(new FakeFilter(), shownBySource, facts::get, null, null);
        // Counts cover the source filter's games only: Puzzle (game C) is not offered.
        assertEquals(1, any.library.genres.size());
        assertEquals("Shooter", any.library.genres.get(0).value);
        assertEquals(2, any.library.genres.get(0).count);

        assertEquals(Arrays.asList("A", "B"), GameInfoFilter.apply(new FakeFilter(), shownBySource, facts::get, "Shooter", null).shown);
        assertEquals(Collections.singletonList("B"), GameInfoFilter.apply(new FakeFilter(), shownBySource, facts::get, "Shooter", "medium").shown);
        assertEquals(Collections.emptyList(), GameInfoFilter.apply(new FakeFilter(), shownBySource, facts::get, "Puzzle", null).shown);
        assertEquals(Collections.singletonList("A"), GameInfoFilter.apply(new FakeFilter(), shownBySource, facts::get, null, "short").shown);
    }

    @Test
    public void noFactsMeansNoChips() throws Exception {
        GameInfoFilter.Result<String> none = GameInfoFilter.apply(new FakeFilter(), Arrays.asList("A", "B"), k -> null, null, null);
        assertFalse(none.library.any());
        // The library cannot answer: the games stay as the source filter left them.
        assertNull(GameInfoFilter.apply(request -> "{\"error\":\"invalid\"}", Arrays.asList("A"), k -> null, null, null));
    }

    @Test
    public void describeLinesNameGenresThemesAndTimeToBeat() throws Exception {
        Context context = org.robolectric.RuntimeEnvironment.getApplication();
        GameInfoModel.Calls hours = request -> {
            try {
                return new JSONObject().put("ok", new JSONObject().put("length", "long").put("hastily", "20").put("normally", "24")
                        .put("completely", JSONObject.NULL)).toString();
            } catch (Exception e) {
                throw new AssertionError(e);
            }
        };
        JSONObject facts = new JSONObject().put("genres", new JSONArray(Arrays.asList("Shooter", "Adventure")))
                .put("themes", new JSONArray(Collections.singletonList("Horror")));
        List<String> lines = GameInfo.describe(context, hours, facts);
        assertEquals(Arrays.asList("Genres: Shooter · Adventure", "Themes: Horror",
                "Time to beat: Story 20 h · Story + extras 24 h"), lines);
        assertTrue(GameInfo.describe(context, hours, null).isEmpty());
    }

    private static boolean waitFor(java.util.function.BooleanSupplier condition) throws Exception {
        for (int i = 0; i < 100 && !condition.getAsBoolean(); i++) Thread.sleep(50);
        return condition.getAsBoolean();
    }

    @Test
    public void aSteamStoreEntryIsPassedToTheCleanOp() throws Exception {
        JSONObject c = caseOf("clean", 2);
        Fixed calls = new Fixed(c);
        JSONObject request = c.getJSONObject("request");
        GameInfoModel.clean(calls, request.getString("name"), request.getString("game_id"), "steam", "1145360");
        assertSent(request, calls.sent);
        // Without a store entry the key is left out.
        Fixed plain = new Fixed(caseOf("clean", 1));
        GameInfoModel.clean(plain, "Mafia: Definitive Edition", null, null, null);
        assertFalse(plain.sent.has("store"));
    }

    @Test
    public void aJobWithLargeNumbersGoesThroughStartAndNextUnchanged() throws Exception {
        // Fingerprints are whole numbers up to 2^53; none may pass through a double on the way (org.json would).
        for (int i = 0; i < 50; i++) {
            long fingerprint = (1L << 53) - 1 - i * 7919L;
            long second = (1L << 52) + i * 104729L;
            JSONObject job = new JSONObject().put("art_keys", fingerprint).put("info_keys", second).put("at", 0);
            String[] sentJob = new String[1];
            GameInfoModel.Calls calls = request -> {
                try {
                    JSONObject r = new JSONObject(request);
                    JSONObject ok = new JSONObject();
                    if (r.getString("op").equals("start")) {
                        ok.put("job", job).put("step", new JSONObject().put("wait", 1));
                    } else {
                        sentJob[0] = r.getJSONObject("job").toString();
                        ok.put("job", r.getJSONObject("job")).put("step", done());
                    }
                    return new JSONObject().put("ok", ok).toString();
                } catch (Exception e) {
                    throw new AssertionError(e);
                }
            };
            GameInfoModel.Finished finished = GameInfoModel.lookup(calls, request -> new GameInfoModel.Reply(200, "", null),
                    millis -> { }, () -> 1000L, () -> 5L, new JSONObject().put("name", "Doom").put("steam", JSONObject.NULL),
                    IGDB, false, true, null);
            assertNotNull(finished);
            JSONObject back = new JSONObject(sentJob[0]);
            assertEquals(fingerprint, back.getLong("art_keys"));
            assertEquals(second, back.getLong("info_keys"));
            assertTrue(sentJob[0].contains(Long.toString(fingerprint)));
        }
    }

    @Test
    public void onlyTheHostsNotFoundCountsAsNoCover() throws Exception {
        assertTrue(com.limelight.grid.assets.NetworkAssetLoader.isNotFound(new com.limelight.nvstream.http.HostHttpResponseException(404, "x")));
        assertFalse(com.limelight.grid.assets.NetworkAssetLoader.isNotFound(new com.limelight.nvstream.http.HostHttpResponseException(500, "x")));
        assertTrue(com.limelight.grid.assets.NetworkAssetLoader.isNotFound(new java.io.FileNotFoundException("404")));
        assertFalse(com.limelight.grid.assets.NetworkAssetLoader.isNotFound(new java.io.IOException("timeout")));
    }

    @Test
    public void changedKeysAskTheFactsAgain() throws Exception {
        FakeLibrary library = new FakeLibrary();
        for (int i = 0; i < 4; i++) {
            library.script.add(send("igdb"));
            library.script.add(done());
        }
        // One script position per lookup: start returns script[0], next script[1]; so run each lookup from a fresh script.
        AtomicInteger sends = new AtomicInteger();
        GameInfoModel.Keys[] now = {IGDB};
        GameInfo info = new GameInfo(temp(), library, request -> {
            sends.incrementAndGet();
            return new GameInfoModel.Reply(500, "", null);
        }, () -> now[0], true);

        info.prefetchFacts(Collections.singletonList(DOOM), () -> { });
        assertTrue(waitFor(() -> sends.get() == 1));
        // The same keys: the game is not asked again.
        info.prefetchFacts(Collections.singletonList(DOOM), () -> { });
        Thread.sleep(200);
        assertEquals(1, sends.get());
        // New keys: asked again.
        now[0] = new GameInfoModel.Keys("", "id2", "secret2");
        info.prefetchFacts(Collections.singletonList(DOOM), () -> { });
        assertTrue(waitFor(() -> sends.get() == 2));
    }

    @Test
    public void aCoverOnDiskComesWithoutAnyRequestAndViewsShareOneLookup() throws Exception {
        FakeLibrary library = new FakeLibrary();
        library.freshAnswer = true;
        File dir = temp();
        // A record that points at a cover, and the cover's image beside it.
        Files.write(new File(dir, "record.json").toPath(), new JSONObject().put("key", "name:doom").put("fetched_at", 1000)
                .put("cover", "https://example.invalid/c.png").toString().getBytes(StandardCharsets.UTF_8));
        GameInfo info = new GameInfo(dir, library, request -> {
            throw new AssertionError("a kept cover needs no request");
        }, () -> new GameInfoModel.Keys("k", "", ""), true);

        // Not downloaded yet: the cache read finds no file and fetches nothing.
        assertNull(info.cachedCover(DOOM));
        assertEquals(0, info.downloads());

        File image = new File(new File(dir, "img"), "cover.png");
        assertTrue(image.getParentFile().mkdirs());
        Files.write(image.toPath(), new byte[] {1, 2, 3});
        assertNotNull(info.cachedCover(DOOM));

        AtomicInteger delivered = new AtomicInteger();
        for (int i = 0; i < 5; i++) info.cover(DOOM, file -> {
            if (file != null) delivered.incrementAndGet();
        });
        assertTrue(waitFor(() -> delivered.get() == 5));
        assertEquals(0, info.downloads());
    }

    @Test
    public void aMissingCoverIsFetchedOnceAndAFailureIsNotRepeated() throws Exception {
        FakeLibrary library = new FakeLibrary();
        library.freshAnswer = true;
        File dir = temp();
        Files.write(new File(dir, "record.json").toPath(), new JSONObject().put("key", "name:doom").put("fetched_at", 1000)
                .put("cover", "http://127.0.0.1:1/c.png").toString().getBytes(StandardCharsets.UTF_8));
        GameInfo info = new GameInfo(dir, library, request -> new GameInfoModel.Reply(0, "", null),
                () -> new GameInfoModel.Keys("k", "", ""), false);
        assertNull(info.resolve(DOOM, true, false).image);
        assertEquals(1, info.downloads());
        assertNull(info.resolve(DOOM, true, false).image);
        assertEquals(1, info.downloads());
    }
}
