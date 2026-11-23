package com.limelight.console;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;

import com.limelight.nvstream.jni.NightjarJni;

import org.json.JSONArray;
import org.json.JSONObject;
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
 * The wire contract with libnightjar_core (protocol/android/src/game_info_contract.json, backed directly by
 * {@link NightjarJni#gameInfoCall}), the driver loop with an offline scripted transport, and the Genre and Length
 * choices. No test touches the network.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameInfoModelTest {
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

    @Test
    public void cleanSendsTheContractsRequestAndReadsItsAnswer() throws Exception {
        for (int nth = 0; nth < 3; nth++) {
            JSONObject c = caseOf("clean", nth);
            JSONObject request = c.getJSONObject("request");
            GameInfoModel.Cleaned cleaned = GameInfoModel.clean(request.getString("name"),
                    request.isNull("game_id") ? null : request.getString("game_id"),
                    request.has("store") ? request.getJSONObject("store").getString("source") : null,
                    request.has("store") ? request.getJSONObject("store").getString("id") : null);
            JSONObject ok = c.getJSONObject("response").getJSONObject("ok");
            assertEquals(ok.getString("clean"), cleaned.clean);
            assertEquals(ok.getString("key"), cleaned.key);
            assertEquals(ok.isNull("steam") ? null : (Long) ok.getLong("steam"), cleaned.steam);
        }
    }

    @Test
    public void freshSendsOnlyTheKeysOfTheServicesWanted() throws Exception {
        JSONObject c = caseOf("fresh", 0);
        JSONObject request = c.getJSONObject("request");
        JSONObject rec = request.getJSONObject("record");
        // Build a matching record with real key hashes for steamgriddb="k", igdb="old"/"sec" so art matches "k".
        long artKeys = lookupKeysHash(new GameInfoModel.Keys("k", "", ""), true, false).getLong("art_keys");
        rec.put("art_keys", artKeys);
        rec.put("fetched_at", request.getLong("now_secs"));
        GameInfoModel.Keys keys = new GameInfoModel.Keys("k", "i", "s");
        assertTrue(GameInfoModel.fresh(rec, request.getLong("now_secs"), keys, true, false));
        // When IGDB facts are wanted too, mismatched info_keys makes the same record stale.
        assertFalse(GameInfoModel.fresh(rec, request.getLong("now_secs"), keys, true, true));

        JSONObject added = caseOf("fresh", 1);
        assertFalse(GameInfoModel.fresh(added.getJSONObject("request").getJSONObject("record"), 2000, keys, true, true));
    }

    @Test
    public void libraryFactsFilesAndMergeSendTheContractsRequests() throws Exception {
        JSONObject library = caseOf("library", 0);
        JSONObject request = library.getJSONObject("request");
        List<JSONObject> games = new ArrayList<>();
        for (int i = 0; i < request.getJSONArray("games").length(); i++) {
            games.add(request.getJSONArray("games").isNull(i) ? null : request.getJSONArray("games").getJSONObject(i));
        }
        GameInfoModel.Library answer = GameInfoModel.library(games, request.getString("genre"), null);
        assertEquals(2, answer.genres.size());
        assertEquals("Puzzle", answer.genres.get(0).value);
        assertArrayEquals(new int[] {0}, answer.shown);
        assertTrue(answer.any());

        JSONObject facts = caseOf("facts", 0);
        GameInfoModel.TimeToBeat time = GameInfoModel.timeToBeat(facts.getJSONObject("request").getJSONObject("facts"));
        assertEquals("medium", time.length);
        assertEquals("1.5", time.hastily);
        assertNull(time.completely);

        JSONObject files = caseOf("files", 0);
        JSONObject expectedFiles = files.getJSONObject("response").getJSONObject("ok");
        String[] names = GameInfoModel.files("steam:1", "https://cdn2.steamgriddb.com/grid/a.png");
        assertEquals(expectedFiles.getString("record"), names[0]);
        assertEquals(expectedFiles.getString("image"), names[1]);

        JSONObject merge = caseOf("merge", 0);
        JSONObject request2 = merge.getJSONObject("request");
        JSONObject merged = GameInfoModel.merge(request2.getJSONObject("record"),
                request2.isNull("older") ? null : request2.getJSONObject("older"));
        assertNotNull(merged);
        assertTrue(merged.toString(), same(merge.getJSONObject("response").getJSONObject("ok").getJSONObject("record"), merged));
    }

    private static boolean same(Object a, Object b) throws Exception {
        if (a instanceof JSONObject && b instanceof JSONObject) {
            JSONObject x = (JSONObject) a, y = (JSONObject) b;
            if (x.length() != y.length()) return false;
            for (java.util.Iterator<String> it = x.keys(); it.hasNext(); ) {
                String k = it.next();
                if (!y.has(k) || !same(x.get(k), y.get(k))) return false;
            }
            return true;
        }
        if (a instanceof JSONArray && b instanceof JSONArray) {
            JSONArray x = (JSONArray) a, y = (JSONArray) b;
            if (x.length() != y.length()) return false;
            for (int i = 0; i < x.length(); i++) if (!same(x.get(i), y.get(i))) return false;
            return true;
        }
        if (JSONObject.NULL.equals(a) || JSONObject.NULL.equals(b)) return JSONObject.NULL.equals(a) && JSONObject.NULL.equals(b);
        return a.equals(b);
    }

    private static void assertArrayEquals(int[] expected, int[] actual) {
        assertEquals(Arrays.toString(expected), Arrays.toString(actual));
    }

    private static JSONObject lookupKeysHash(GameInfoModel.Keys keys, boolean art, boolean info) throws Exception {
        long[] nowMs = {10_000L};
        long nowSecs = System.currentTimeMillis() / 1000L;
        GameInfoModel.Finished finished = GameInfoModel.lookup(
                req -> {
                    String service = req.optString("service");
                    if ("twitch".equals(service)) {
                        return new GameInfoModel.Reply(200,
                                "{\"access_token\":\"tok\",\"expires_in\":3600,\"token_type\":\"bearer\"}", null);
                    }
                    if ("steam_grid_db".equals(service)) {
                        String url = req.optString("url");
                        if (url.contains("/search/autocomplete/")) {
                            return new GameInfoModel.Reply(200,
                                    "{\"success\":true,\"data\":[{\"id\":42,\"name\":\"Doom\"}]}", null);
                        }
                        if (url.contains("/grids/game/")) {
                            return new GameInfoModel.Reply(200,
                                    "{\"success\":true,\"data\":[{\"id\":1,\"score\":5,\"url\":\"https://example.invalid/c.png\"}]}",
                                    null);
                        }
                        return new GameInfoModel.Reply(200, "{\"success\":true,\"data\":[]}", null);
                    }
                    if ("igdb".equals(service)) {
                        if (req.optString("url").contains("game_time_to_beats")) {
                            return new GameInfoModel.Reply(200, "[{\"id\":1,\"game_id\":7,\"normally\":36000}]", null);
                        }
                        return new GameInfoModel.Reply(200,
                                "[{\"id\":7,\"name\":\"Doom\",\"genres\":[{\"id\":1,\"name\":\"Shooter\"}]}]", null);
                    }
                    return new GameInfoModel.Reply(200, "[]", null);
                },
                millis -> nowMs[0] += millis,
                () -> nowSecs,
                () -> nowMs[0],
                new JSONObject().put("name", "Doom").put("steam", JSONObject.NULL),
                keys, art, info, null);
        assertNotNull(finished);
        return finished.record;
    }

    private static GameInfoModel.Reply igdbReply(JSONObject request, List<JSONObject> sent, boolean force429OnFirstGames) {
        sent.add(request);
        String service = request.optString("service");
        if ("twitch".equals(service)) {
            return new GameInfoModel.Reply(200,
                    "{\"access_token\":\"tok-abcdef\",\"expires_in\":5587808,\"token_type\":\"bearer\"}", null);
        }
        if ("igdb".equals(service)) {
            String url = request.optString("url");
            if (url.contains("game_time_to_beats")) {
                return new GameInfoModel.Reply(200, "[{\"id\":1,\"game_id\":7,\"normally\":36000}]", null);
            }
            if (force429OnFirstGames) {
                return new GameInfoModel.Reply(429, "", 2L);
            }
            return new GameInfoModel.Reply(200,
                    "[{\"id\":7,\"name\":\"Doom\",\"genres\":[{\"id\":1,\"name\":\"Shooter\"}]}]", null);
        }
        return new GameInfoModel.Reply(404, "", null);
    }

    private static File temp() throws Exception {
        File dir = Files.createTempDirectory("game-info-test").toFile();
        dir.deleteOnExit();
        return dir;
    }

    private static final GameInfoModel.Keys IGDB = new GameInfoModel.Keys("", "id", "secret");
    private static final GameInfo.Game DOOM = new GameInfo.Game("Doom", null, null, null);

    private static void advanceClock(long millis) {
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofMillis(millis));
    }

    @Test
    public void aFullLookupEndsInARecordOnDisk() throws Exception {
        List<JSONObject> sent = new ArrayList<>();
        File dir = temp();
        GameInfo info = new GameInfo(dir, request -> igdbReply(request, sent, false), () -> IGDB, false);
        info.setSleeper(GameInfoModelTest::advanceClock);

        GameInfo.Entry entry = info.resolve(DOOM, false, true);

        assertNotNull(entry);
        assertFalse(sent.isEmpty());
        File record = new File(dir, GameInfoModel.files("name:doom", null)[0]);
        assertTrue(record.isFile());
        JSONObject kept = new JSONObject(new String(Files.readAllBytes(record.toPath()), StandardCharsets.UTF_8));
        assertEquals("Shooter", kept.getJSONObject("facts").getJSONArray("genres").getString(0));

        // A fresh record is read from disk: no second request.
        GameInfo again = new GameInfo(dir, request -> {
            throw new AssertionError("a fresh record needs no request");
        }, () -> IGDB, false);
        again.setSleeper(GameInfoModelTest::advanceClock);
        assertNotNull(again.resolve(DOOM, false, true).facts());
    }

    @Test
    public void aTooManyRequestsReplyCarriesItsRetryAfterAndTheLookupWaitsThenSucceeds() throws Exception {
        boolean[] rateLimitedOnce = {false};
        List<Long> slept = new ArrayList<>();
        List<JSONObject> sent = new ArrayList<>();
        File dir = temp();
        GameInfo info = new GameInfo(dir, request -> {
            if ("igdb".equals(request.optString("service"))
                    && !request.optString("url").contains("game_time_to_beats")
                    && !rateLimitedOnce[0]) {
                rateLimitedOnce[0] = true;
                return igdbReply(request, sent, true);
            }
            return igdbReply(request, sent, false);
        }, () -> IGDB, false);
        info.setSleeper(ms -> {
            slept.add(ms);
            advanceClock(ms);
        });

        GameInfo.Entry entry = info.resolve(DOOM, false, true);

        assertNotNull(entry);
        assertTrue(slept.toString(), slept.contains(2000L));
        assertTrue(new File(dir, GameInfoModel.files("name:doom", null)[0]).isFile());
    }

    @Test
    public void noAnswerIsStatusZeroForTheLibrary() throws Exception {
        GameInfo info = new GameInfo(temp(), request -> new GameInfoModel.Reply(0, "", null), () -> IGDB, false);
        info.setSleeper(GameInfoModelTest::advanceClock);
        GameInfo.Entry entry = info.resolve(DOOM, false, true);
        assertNotNull(entry);
        assertNull(entry.facts());
    }

    @Test
    public void anIoErrorOfTheRealTransportIsStatusZero() throws Exception {
        GameInfo info = new GameInfo(temp(), null, () -> IGDB, false);
        // Nothing listens on port 1 of this machine: the connection is refused at once.
        GameInfoModel.Reply reply = info.send(new JSONObject().put("service", "igdb").put("method", "POST")
                .put("url", "http://127.0.0.1:1/x").put("headers", new JSONArray()).put("body", "x"));
        assertEquals(0, reply.status);
    }

    @Test
    public void withoutKeysNothingIsAskedAndNothingIsSent() throws Exception {
        GameInfo info = new GameInfo(temp(), request -> {
            throw new AssertionError("no key, no request");
        }, () -> new GameInfoModel.Keys("", "", ""), true);
        assertNull(info.resolve(DOOM, true, true));
        boolean[] called = {false};
        info.cover(DOOM, file -> called[0] = true);
        info.facts(DOOM, facts -> called[0] = true);
        info.prefetchFacts(Collections.singletonList(DOOM), () -> called[0] = true);
        Thread.sleep(100);
        assertFalse(called[0]);

        Context context = org.robolectric.RuntimeEnvironment.getApplication();
        assertFalse(GameInfo.coverWanted(context));
        assertFalse(GameInfo.factsWanted(context));
        // Only the SteamGridDB key: cover lookups, no facts.
        assertNull(new GameInfo(temp(), request -> {
            throw new AssertionError("IGDB is off");
        }, () -> new GameInfoModel.Keys("k", "", ""), false).resolve(DOOM, false, true));
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

        GameInfoFilter.Result<String> any = GameInfoFilter.apply(shownBySource, facts::get, null, null);
        // Counts cover the source filter's games only: Puzzle (game C) is not offered.
        assertEquals(1, any.library.genres.size());
        assertEquals("Shooter", any.library.genres.get(0).value);
        assertEquals(2, any.library.genres.get(0).count);

        assertEquals(Arrays.asList("A", "B"), GameInfoFilter.apply(shownBySource, facts::get, "Shooter", null).shown);
        assertEquals(Collections.singletonList("B"), GameInfoFilter.apply(shownBySource, facts::get, "Shooter", "medium").shown);
        assertEquals(Collections.emptyList(), GameInfoFilter.apply(shownBySource, facts::get, "Puzzle", null).shown);
        assertEquals(Collections.singletonList("A"), GameInfoFilter.apply(shownBySource, facts::get, null, "short").shown);
    }

    @Test
    public void noFactsMeansNoChips() throws Exception {
        GameInfoFilter.Result<String> none = GameInfoFilter.apply(Arrays.asList("A", "B"), k -> null, null, null);
        assertFalse(none.library.any());
        // The library cannot answer: the games stay as the source filter left them.
        assertNull(GameInfoFilter.parseLibrary("{\"error\":\"invalid\"}"));
        NightjarJni.forceUnavailableForTest(new UnsatisfiedLinkError("no library"));
        try {
            assertNull(GameInfoFilter.apply(Arrays.asList("A"), k -> null, null, null));
        } finally {
            NightjarJni.forceUnavailableForTest(null);
        }
    }

    @Test
    public void describeLinesNameGenresThemesAndTimeToBeat() throws Exception {
        Context context = org.robolectric.RuntimeEnvironment.getApplication();
        JSONObject facts = new JSONObject().put("genres", new JSONArray(Arrays.asList("Shooter", "Adventure")))
                .put("themes", new JSONArray(Collections.singletonList("Horror")))
                .put("hastily", 72000).put("normally", 86400);
        List<String> lines = GameInfo.describe(context, facts);
        assertEquals(Arrays.asList("Genres: Shooter · Adventure", "Themes: Horror",
                "Time to beat: Story 20 h · Story + extras 24 h"), lines);
        assertTrue(GameInfo.describe(context, null).isEmpty());
    }

    private static boolean waitFor(java.util.function.BooleanSupplier condition) throws Exception {
        for (int i = 0; i < 100 && !condition.getAsBoolean(); i++) Thread.sleep(50);
        return condition.getAsBoolean();
    }

    @Test
    public void aSteamStoreEntryIsPassedToTheCleanOp() throws Exception {
        JSONObject c = caseOf("clean", 2);
        JSONObject request = c.getJSONObject("request");
        GameInfoModel.Cleaned cleaned = GameInfoModel.clean(request.getString("name"),
                request.getString("game_id"), "steam", "1145360");
        JSONObject ok = c.getJSONObject("response").getJSONObject("ok");
        assertEquals(ok.getString("clean"), cleaned.clean);
        assertEquals(ok.getString("key"), cleaned.key);
        assertEquals(Long.valueOf(ok.getLong("steam")), cleaned.steam);
        // Without a store entry the steam id is null unless the game_id names it.
        GameInfoModel.Cleaned plain = GameInfoModel.clean("Mafia: Definitive Edition", null, null, null);
        assertEquals("Mafia", plain.clean);
        assertNull(plain.steam);
    }

    @Test
    public void aJobWithLargeNumbersGoesThroughStartAndNextUnchanged() throws Exception {
        // Fingerprints are 53-bit whole numbers computed by Rust; they travel through start/next/record unchanged.
        long nowSecs = System.currentTimeMillis() / 1000L;
        for (int i = 0; i < 10; i++) {
            GameInfoModel.Keys keys = new GameInfoModel.Keys("sgdb-" + i, "id-" + i, "secret-" + i);
            JSONObject record = lookupKeysHash(keys, true, true);
            long artKeys = record.getLong("art_keys");
            long infoKeys = record.getLong("info_keys");
            assertTrue(artKeys > 0 && artKeys <= (1L << 53) - 1);
            assertTrue(infoKeys > 0 && infoKeys <= (1L << 53) - 1);
            assertTrue(GameInfoModel.fresh(record, nowSecs, keys, true, true));
            record.put("art", "failed");
            assertTrue(GameInfoModel.fresh(record, nowSecs, keys, true, true));
            assertFalse(GameInfoModel.fresh(record, nowSecs, new GameInfoModel.Keys("other-" + i, "id-" + i, "secret-" + i), true, true));
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
        AtomicInteger sends = new AtomicInteger();
        GameInfoModel.Keys[] now = {IGDB};
        GameInfo info = new GameInfo(temp(), request -> {
            if ("twitch".equals(request.optString("service"))) {
                return new GameInfoModel.Reply(200,
                        "{\"access_token\":\"tok\",\"expires_in\":3600,\"token_type\":\"bearer\"}", null);
            }
            sends.incrementAndGet();
            return new GameInfoModel.Reply(500, "", null);
        }, () -> now[0], true);
        info.setSleeper(GameInfoModelTest::advanceClock);

        info.prefetchFacts(Collections.singletonList(DOOM), () -> { });
        assertTrue(waitFor(() -> sends.get() >= 4));
        int afterFirst = sends.get();
        // The same keys: the game is not asked again.
        info.prefetchFacts(Collections.singletonList(DOOM), () -> { });
        Thread.sleep(200);
        assertEquals(afterFirst, sends.get());
        // New keys: asked again.
        now[0] = new GameInfoModel.Keys("", "id2", "secret2");
        info.prefetchFacts(Collections.singletonList(DOOM), () -> { });
        assertTrue(waitFor(() -> sends.get() > afterFirst));
    }

    @Test
    public void aCoverOnDiskComesWithoutAnyRequestAndViewsShareOneLookup() throws Exception {
        File dir = temp();
        String url = "https://example.invalid/c.png";
        String[] names = GameInfoModel.files("name:doom", url);
        GameInfoModel.Keys keys = new GameInfoModel.Keys("k", "", "");
        JSONObject freshRecord = lookupKeysHash(keys, true, false);
        assertEquals(freshRecord.toString(), url, freshRecord.optString("cover"));
        Files.write(new File(dir, names[0]).toPath(), freshRecord.toString().getBytes(StandardCharsets.UTF_8));
        GameInfo info = new GameInfo(dir, request -> {
            throw new AssertionError("a kept cover needs no request");
        }, () -> keys, true);

        // Not downloaded yet: the cache read finds no file and fetches nothing.
        assertNull(info.cachedCover(DOOM));
        assertEquals(0, info.downloads());

        File image = new File(new File(dir, "img"), names[1]);
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
        File dir = temp();
        String url = "http://127.0.0.1:1/c.png";
        String[] names = GameInfoModel.files("name:doom", url);
        GameInfoModel.Keys keys = new GameInfoModel.Keys("k", "", "");
        JSONObject freshRecord = lookupKeysHash(keys, true, false).put("cover", url);
        Files.write(new File(dir, names[0]).toPath(), freshRecord.toString().getBytes(StandardCharsets.UTF_8));
        GameInfo info = new GameInfo(dir, request -> new GameInfoModel.Reply(0, "", null),
                () -> keys, false);
        assertNull(info.resolve(DOOM, true, false).image);
        assertEquals(1, info.downloads());
        assertNull(info.resolve(DOOM, true, false).image);
        assertEquals(1, info.downloads());
    }
}
