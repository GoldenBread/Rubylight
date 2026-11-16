package com.limelight.console;

import androidx.annotation.VisibleForTesting;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;
import java.util.function.LongSupplier;

/**
 * Artwork and game facts, the same lookup desktop Nightjar runs: nightjar-policy's game_info (protocol/policy), reached
 * through libnightjar_core (protocol/android/src/game_info.rs). The library only plans the requests; this class carries
 * them over a {@link Transport} and keeps nothing. Every call is one JSON request in, one answer out.
 */
public final class GameInfoModel {
    private GameInfoModel() { }

    /** One JSON request in, one JSON answer out: {@code {"ok": ...}} or {@code {"error": code, "detail": ...}}. */
    public interface Calls {
        String call(String request);
    }

    /** What came back from one HTTPS request: status 0 when no answer came. */
    public static final class Reply {
        public final int status;
        public final String body;
        /** The Retry-After header in seconds, or null. */
        public final Long retryAfter;

        public Reply(int status, String body, Long retryAfter) {
            this.status = status;
            this.body = body;
            this.retryAfter = retryAfter;
        }
    }

    /** Sends one planned request ({@code service, method, url, headers, body}); never throws: no answer is status 0. */
    public interface Transport {
        Reply send(JSONObject request);
    }

    public interface Sleeper {
        void sleep(long millis) throws InterruptedException;
    }

    /** The three keys; a service is on when its key (IGDB: both) is set. */
    public static final class Keys {
        public final String steamgriddb;
        public final String igdbClientId;
        public final String igdbClientSecret;

        public Keys(String steamgriddb, String igdbClientId, String igdbClientSecret) {
            this.steamgriddb = trim(steamgriddb);
            this.igdbClientId = trim(igdbClientId);
            this.igdbClientSecret = trim(igdbClientSecret);
        }

        private static String trim(String s) {
            return s == null ? "" : s.trim();
        }

        public boolean art() {
            return !steamgriddb.isEmpty();
        }

        public boolean info() {
            return !igdbClientId.isEmpty() && !igdbClientSecret.isEmpty();
        }

        public boolean any() {
            return art() || info();
        }

        /** The keys of the services that are both set and wanted; the others blank, so a record that skipped them stays fresh. */
        public JSONObject json(boolean wantArt, boolean wantInfo) {
            try {
                return new JSONObject()
                        .put("steamgriddb", wantArt ? steamgriddb : "")
                        .put("igdb_client_id", wantInfo ? igdbClientId : "")
                        .put("igdb_client_secret", wantInfo ? igdbClientSecret : "");
            } catch (JSONException e) {
                throw new IllegalStateException(e);
            }
        }

        String[] secrets() {
            return new String[] {steamgriddb, igdbClientId, igdbClientSecret};
        }
    }

    private static Calls calls;

    private static synchronized Calls calls() {
        if (calls == null) {
            System.loadLibrary("nightjar_core");
            calls = GameInfoModel::call;
        }
        return calls;
    }

    /** JVM tests cannot load the library. */
    @VisibleForTesting
    public static synchronized void use(Calls replacement) {
        calls = replacement;
    }

    /** The library's calls: the native one, or a test's. */
    static Calls library() {
        return calls();
    }

    private static native String call(String request);

    /** The {@code ok} object of one call; throws when the library cannot answer. */
    static JSONObject ask(Calls calls, JSONObject request) throws JSONException {
        JSONObject answer = new JSONObject(calls.call(request.toString()));
        JSONObject ok = answer.optJSONObject("ok");
        if (ok == null) throw new JSONException("game info call failed: " + answer.optString("error"));
        return ok;
    }

    /** A game's cleaned name and key. */
    public static final class Cleaned {
        public final String clean;
        public final String key;
        /** The Steam app id, or null. */
        public final Long steam;

        Cleaned(String clean, String key, Long steam) {
            this.clean = clean;
            this.key = key;
            this.steam = steam;
        }
    }

    static Cleaned clean(Calls calls, String name, String gameId) throws JSONException {
        JSONObject ok = ask(calls, new JSONObject().put("op", "clean").put("name", name)
                .put("game_id", gameId == null ? JSONObject.NULL : gameId));
        return new Cleaned(ok.getString("clean"), ok.getString("key"), ok.isNull("steam") ? null : ok.getLong("steam"));
    }

    static boolean fresh(Calls calls, JSONObject record, long nowSecs, Keys keys, boolean wantArt, boolean wantInfo) throws JSONException {
        return ask(calls, new JSONObject().put("op", "fresh").put("record", record).put("now_secs", nowSecs)
                .put("keys", keys.json(wantArt, wantInfo))).getBoolean("fresh");
    }

    /** The new record with what the older one held that the new lookup did not ask for or could not repeat. */
    static JSONObject merge(Calls calls, JSONObject record, JSONObject older) throws JSONException {
        return ask(calls, new JSONObject().put("op", "merge").put("record", record)
                .put("older", older == null ? JSONObject.NULL : older)).getJSONObject("record");
    }

    /** The record's file name and, for a URL, the image's (null without a URL). */
    static String[] files(Calls calls, String key, String url) throws JSONException {
        JSONObject ok = ask(calls, new JSONObject().put("op", "files").put("key", key).put("url", url == null ? JSONObject.NULL : url));
        return new String[] {ok.getString("record"), ok.isNull("image") ? null : ok.getString("image")};
    }

    /** What one lookup ended with. */
    public static final class Finished {
        public final JSONObject record;
        public final JSONObject session;
        public final List<String> log;

        Finished(JSONObject record, JSONObject session, List<String> log) {
            this.record = record;
            this.session = session;
            this.log = log;
        }
    }

    /** No lookup takes this many steps; a library that never finishes cannot hold the worker. */
    private static final int MAX_STEPS = 400;

    /**
     * Runs one lookup to its end: {@code start}, then each planned request through {@code transport}, each wait through
     * {@code sleeper}, until {@code done}. Returns null when the library cannot answer, the thread is interrupted or the
     * steps run out.
     */
    static Finished lookup(Calls calls, Transport transport, Sleeper sleeper, LongSupplier wallSecs, LongSupplier nowMs,
                           JSONObject game, Keys keys, boolean cover, boolean facts, JSONObject session) {
        try {
            JSONObject step = ask(calls, new JSONObject().put("op", "start").put("game", game).put("keys", keys.json(true, true))
                    .put("wants", new JSONObject().put("cover", cover).put("hero", false).put("logo", false).put("facts", facts))
                    .put("session", session == null ? JSONObject.NULL : session)
                    .put("wall_secs", wallSecs.getAsLong()).put("now_ms", nowMs.getAsLong()));
            for (int i = 0; i < MAX_STEPS; i++) {
                JSONObject job = step.getJSONObject("job");
                JSONObject what = step.getJSONObject("step");
                Object reply = JSONObject.NULL;
                if (what.has("done")) {
                    JSONObject done = what.getJSONObject("done");
                    List<String> log = new ArrayList<>();
                    JSONArray lines = done.optJSONArray("log");
                    for (int j = 0; lines != null && j < lines.length(); j++) log.add(lines.getString(j));
                    return new Finished(done.getJSONObject("record"), done.getJSONObject("session"), log);
                } else if (what.has("wait")) {
                    sleeper.sleep(what.getLong("wait"));
                } else if (what.has("send")) {
                    Reply answer = transport.send(what.getJSONObject("send"));
                    reply = new JSONObject().put("status", answer.status).put("body", answer.body == null ? "" : answer.body)
                            .put("retry_after", answer.retryAfter == null ? JSONObject.NULL : answer.retryAfter);
                } else {
                    return null;
                }
                step = ask(calls, new JSONObject().put("op", "next").put("job", job).put("reply", reply).put("now_ms", nowMs.getAsLong()));
            }
            return null;
        } catch (JSONException | RuntimeException e) {
            return null;
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            return null;
        }
    }

    /** One game's time to beat in hours as text and its length; each null when missing. */
    public static final class TimeToBeat {
        public final String length;
        public final String hastily;
        public final String normally;
        public final String completely;

        TimeToBeat(String length, String hastily, String normally, String completely) {
            this.length = length;
            this.hastily = hastily;
            this.normally = normally;
            this.completely = completely;
        }
    }

    static TimeToBeat timeToBeat(Calls calls, JSONObject facts) throws JSONException {
        JSONObject ok = ask(calls, new JSONObject().put("op", "facts").put("facts", facts));
        return new TimeToBeat(ok.isNull("length") ? null : ok.getString("length"),
                ok.isNull("hastily") ? null : ok.getString("hastily"),
                ok.isNull("normally") ? null : ok.getString("normally"),
                ok.isNull("completely") ? null : ok.getString("completely"));
    }

    /** One value of a filter chip's list and how many games have it. */
    public static final class Count {
        public final String value;
        public final int count;

        Count(String value, int count) {
            this.value = value;
            this.count = count;
        }
    }

    /** The genres and lengths present in a library, and the places of the games a choice shows. */
    public static final class Library {
        public final List<Count> genres;
        public final List<Count> lengths;
        public final int[] shown;

        Library(List<Count> genres, List<Count> lengths, int[] shown) {
            this.genres = genres;
            this.lengths = lengths;
            this.shown = shown;
        }

        /** True when at least one game has facts: the filter chips show. */
        public boolean any() {
            return !genres.isEmpty() || !lengths.isEmpty();
        }
    }

    /** {@code facts} holds one entry per game, null for a game without facts. */
    static Library library(Calls calls, List<JSONObject> facts, String genre, String length) throws JSONException {
        JSONArray games = new JSONArray();
        for (JSONObject f : facts) games.put(f == null ? JSONObject.NULL : f);
        JSONObject ok = ask(calls, new JSONObject().put("op", "library").put("games", games)
                .put("genre", genre == null ? JSONObject.NULL : genre).put("length", length == null ? JSONObject.NULL : length));
        JSONArray shown = ok.getJSONArray("shown");
        int[] at = new int[shown.length()];
        for (int i = 0; i < at.length; i++) at[i] = shown.getInt(i);
        return new Library(counts(ok.getJSONArray("genres")), counts(ok.getJSONArray("lengths")), at);
    }

    private static List<Count> counts(JSONArray array) throws JSONException {
        List<Count> out = new ArrayList<>(array.length());
        for (int i = 0; i < array.length(); i++) {
            JSONObject c = array.getJSONObject(i);
            out.add(new Count(c.getString("value"), c.getInt("count")));
        }
        return out;
    }
}
