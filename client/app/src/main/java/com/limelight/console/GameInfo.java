package com.limelight.console;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;
import java.util.function.Supplier;

import okhttp3.MediaType;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.Response;

/**
 * SteamGridDB covers and IGDB facts for the library. One background thread runs every lookup, so the services' pace and
 * IGDB's token are shared; records and cover images are files under the cache folder, the keys sit in their own
 * preferences file, and the session (token, pace) stays in memory. With no key set nothing here sends a request.
 */
public final class GameInfo {
    public static final String KEYS_FILE = "game_info_keys";
    public static final String KEY_STEAMGRIDDB = "steamgriddb";
    public static final String KEY_IGDB_ID = "igdb_client_id";
    public static final String KEY_IGDB_SECRET = "igdb_client_secret";

    private static GameInfo instance;

    /** The keys live in their own file, out of presets, profiles, exports and backups. */
    public static SharedPreferences keyPrefs(Context context) {
        return context.getApplicationContext().getSharedPreferences(KEYS_FILE, Context.MODE_PRIVATE);
    }

    public static GameInfoModel.Keys keys(Context context) {
        SharedPreferences prefs = keyPrefs(context);
        return new GameInfoModel.Keys(prefs.getString(KEY_STEAMGRIDDB, ""), prefs.getString(KEY_IGDB_ID, ""),
                prefs.getString(KEY_IGDB_SECRET, ""));
    }

    /** True when a SteamGridDB key is set: covers the host has none of may come from there. */
    public static boolean coverWanted(Context context) {
        return keys(context).art();
    }

    /** True when both IGDB keys are set: facts are looked up. */
    public static boolean factsWanted(Context context) {
        return keys(context).info();
    }

    public static synchronized GameInfo get(Context context) {
        if (instance == null) {
            Context app = context.getApplicationContext();
            instance = new GameInfo(new File(app.getCacheDir(), "game-info"), null, null, () -> keys(app), true);
        }
        return instance;
    }

    /** What one game's lookup holds now: its record and, when a cover was wanted and found, the image file. */
    public static final class Entry {
        public final JSONObject record;
        public final File image;

        Entry(JSONObject record, File image) {
            this.record = record;
            this.image = image;
        }

        public JSONObject facts() {
            return record == null ? null : record.optJSONObject("facts");
        }
    }

    private final File dir;
    private final GameInfoModel.Calls fixedCalls;
    private final GameInfoModel.Transport transport;
    private GameInfoModel.Sleeper sleeper;
    private final Supplier<GameInfoModel.Keys> keys;
    private final ExecutorService executor;
    private final OkHttpClient client;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final Map<String, JSONObject> records = new ConcurrentHashMap<>();
    private final Set<String> imagesFailed = ConcurrentHashMap.newKeySet();
    private final Set<String> factsAsked = ConcurrentHashMap.newKeySet();
    /** In memory only: the token and the pace of the services. */
    private JSONObject session;

    @VisibleForTesting
    GameInfo(File dir, GameInfoModel.Calls calls, GameInfoModel.Transport transport, Supplier<GameInfoModel.Keys> keys, boolean background) {
        this.dir = dir;
        this.fixedCalls = calls;
        this.keys = keys;
        this.client = new OkHttpClient.Builder().connectTimeout(10, TimeUnit.SECONDS).readTimeout(20, TimeUnit.SECONDS)
                .callTimeout(40, TimeUnit.SECONDS).build();
        this.transport = transport != null ? transport : this::send;
        this.sleeper = Thread::sleep;
        this.executor = background ? Executors.newSingleThreadExecutor(r -> {
            Thread t = new Thread(r, "game-info");
            t.setDaemon(true);
            return t;
        }) : null;
    }

    @VisibleForTesting
    void setSleeper(GameInfoModel.Sleeper replacement) {
        sleeper = replacement;
    }

    private GameInfoModel.Calls calls() {
        return fixedCalls != null ? fixedCalls : GameInfoModel.library();
    }

    private static long wallSecs() {
        return System.currentTimeMillis() / 1000;
    }

    // Requests: every one runs on the single worker thread.

    /**
     * Looks the game's cover and facts up and calls {@code done} on the worker thread with the cover image, or null
     * when there is none. Does nothing without a SteamGridDB key.
     */
    public void cover(String name, String gameId, Consumer<File> done) {
        if (executor == null || !keys.get().art()) return;
        executor.execute(() -> {
            Entry entry = resolve(name, gameId, true, true);
            done.accept(entry == null ? null : entry.image);
        });
    }

    /** Looks the game's facts up (no cover) and calls {@code done} on the main thread with them, or null. Nothing without IGDB keys. */
    public void facts(String name, String gameId, Consumer<JSONObject> done) {
        if (executor == null || !keys.get().info()) return;
        executor.execute(() -> {
            Entry entry = resolve(name, gameId, false, true);
            JSONObject facts = entry == null ? null : entry.facts();
            main.post(() -> done.accept(facts));
        });
    }

    /**
     * Looks the facts of these games up in the background ({@code name, gameId} pairs), asking {@code changed} on the
     * main thread after each batch. A game asked once is not asked again until the process restarts.
     */
    public void prefetchFacts(List<String[]> games, Runnable changed) {
        if (executor == null || !keys.get().info()) return;
        int[] pending = {0};
        for (String[] game : games) {
            String id = game[0] + "\n" + game[1];
            if (!factsAsked.add(id)) continue;
            pending[0]++;
            executor.execute(() -> {
                Entry entry = resolve(game[0], game[1], false, true);
                if (entry != null && entry.facts() != null) main.post(changed);
            });
        }
    }

    /** The facts a record on disk holds, without any request; call off the main thread. Null when there are none. */
    public JSONObject cachedFacts(String name, String gameId) {
        try {
            GameInfoModel.Cleaned c = GameInfoModel.clean(calls(), name, gameId);
            JSONObject record = read(c.key, new File(dir, GameInfoModel.files(calls(), c.key, null)[0]));
            return record == null ? null : record.optJSONObject("facts");
        } catch (JSONException | RuntimeException | LinkageError e) {
            return null;
        }
    }

    /** The cover image a record on disk points at, without any request; call off the main thread. Null when none is kept. */
    public File cachedCover(String name, String gameId) {
        try {
            GameInfoModel.Cleaned c = GameInfoModel.clean(calls(), name, gameId);
            JSONObject record = read(c.key, new File(dir, GameInfoModel.files(calls(), c.key, null)[0]));
            return record == null ? null : imageFile(c.key, record);
        } catch (JSONException | RuntimeException | LinkageError e) {
            return null;
        }
    }

    // The lookup, on the worker thread (or a test's).

    /**
     * The game's record, from disk while it holds and from a new lookup when it does not, and its cover image when one
     * is wanted. Null when no service wanted here has a key. Failures keep what was known before.
     */
    @VisibleForTesting
    synchronized Entry resolve(String name, String gameId, boolean wantCover, boolean wantFacts) {
        GameInfoModel.Keys k = keys.get();
        boolean art = wantCover && k.art();
        boolean info = wantFacts && k.info();
        if (!art && !info) return null;
        try {
            GameInfoModel.Calls calls = calls();
            GameInfoModel.Cleaned c = GameInfoModel.clean(calls, name, gameId);
            File file = new File(dir, GameInfoModel.files(calls, c.key, null)[0]);
            JSONObject record = read(c.key, file);
            if (record == null || !GameInfoModel.fresh(calls, record, wallSecs(), k, art, info)) {
                JSONObject game = new JSONObject().put("name", c.clean).put("steam", c.steam == null ? JSONObject.NULL : c.steam);
                GameInfoModel.Finished finished = GameInfoModel.lookup(calls, transport, sleeper, GameInfo::wallSecs,
                        SystemClock::elapsedRealtime, game, k, art, info, session);
                if (finished != null) {
                    session = finished.session;
                    for (String line : finished.log) LimeLog.warning("Game info: " + line);
                    record = GameInfoModel.merge(calls, finished.record, record);
                    write(file, record);
                    records.put(c.key, record);
                }
            }
            if (record == null) return null;
            return new Entry(record, art ? imageFile(c.key, record) : null);
        } catch (JSONException | RuntimeException | LinkageError e) {
            LimeLog.warning("Game info could not look up a game: " + e.getClass().getSimpleName());
            return null;
        }
    }

    /** The cover's image file, fetched when it is not kept yet; null when the record has none or it cannot be had. */
    private File imageFile(String key, JSONObject record) throws JSONException {
        String url = record.isNull("cover") ? null : record.optString("cover", null);
        if (url == null || url.isEmpty()) return null;
        File image = new File(new File(dir, "img"), GameInfoModel.files(calls(), key, url)[1]);
        if (image.isFile() && image.length() > 0) return image;
        if (imagesFailed.contains(url)) return null;
        if (!download(url, image)) {
            imagesFailed.add(url);
            return null;
        }
        return image;
    }

    private boolean download(String url, File target) {
        File temporary = null;
        try {
            File folder = target.getParentFile();
            if (folder != null && !folder.isDirectory() && !folder.mkdirs()) return false;
            try (Response response = client.newCall(new Request.Builder().url(url).get().build()).execute()) {
                if (!response.isSuccessful() || response.body() == null) return false;
                temporary = File.createTempFile("cover-", ".tmp", folder);
                try (InputStream in = response.body().byteStream(); OutputStream out = new FileOutputStream(temporary)) {
                    byte[] buffer = new byte[16 * 1024];
                    int n;
                    while ((n = in.read(buffer)) > 0) out.write(buffer, 0, n);
                }
            }
            if (temporary.length() == 0) return false;
            Files.move(temporary.toPath(), target.toPath(), java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            temporary = null;
            return true;
        } catch (IOException | RuntimeException e) {
            return false;
        } finally {
            if (temporary != null) temporary.delete();
        }
    }

    private JSONObject read(String key, File file) {
        JSONObject cached = records.get(key);
        if (cached != null) return cached;
        try {
            if (!file.isFile()) return null;
            JSONObject record = new JSONObject(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
            records.put(key, record);
            return record;
        } catch (IOException | JSONException e) {
            return null;
        }
    }

    private static void write(File file, JSONObject record) {
        File temporary = null;
        try {
            File folder = file.getParentFile();
            if (folder != null && !folder.isDirectory() && !folder.mkdirs()) return;
            temporary = File.createTempFile("record-", ".tmp", folder);
            Files.write(temporary.toPath(), record.toString().getBytes(StandardCharsets.UTF_8));
            Files.move(temporary.toPath(), file.toPath(), java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            temporary = null;
        } catch (IOException | RuntimeException e) {
            LimeLog.warning("Game info could not keep a record");
        } finally {
            if (temporary != null) temporary.delete();
        }
    }

    private static String joined(JSONObject facts, String field) {
        org.json.JSONArray array = facts.optJSONArray(field);
        StringBuilder out = new StringBuilder();
        for (int i = 0; array != null && i < array.length(); i++) {
            String value = array.optString(i, "");
            if (value.isEmpty()) continue;
            if (out.length() > 0) out.append(" · ");
            out.append(value);
        }
        return out.toString();
    }

    /**
     * The lines a game's facts show: Genres, Themes and Time to beat ("Story 20 h · Story + extras 24 h · Everything
     * 90 h"), each only when there is something to say. Empty when there is nothing.
     */
    public static List<String> describe(Context context, JSONObject facts) {
        return describe(context, GameInfoModel.library(), facts);
    }

    static List<String> describe(Context context, GameInfoModel.Calls calls, JSONObject facts) {
        List<String> lines = new java.util.ArrayList<>();
        if (facts == null) return lines;
        String genres = joined(facts, "genres");
        if (!genres.isEmpty()) lines.add(context.getString(com.limelight.R.string.game_info_genres) + ": " + genres);
        String themes = joined(facts, "themes");
        if (!themes.isEmpty()) lines.add(context.getString(com.limelight.R.string.game_info_themes) + ": " + themes);
        try {
            GameInfoModel.TimeToBeat t = GameInfoModel.timeToBeat(calls, facts);
            StringBuilder time = new StringBuilder();
            String[] hours = {t.hastily, t.normally, t.completely};
            int[] labels = {com.limelight.R.string.game_info_story, com.limelight.R.string.game_info_extras,
                    com.limelight.R.string.game_info_everything};
            for (int i = 0; i < hours.length; i++) {
                if (hours[i] == null) continue;
                if (time.length() > 0) time.append(" · ");
                time.append(context.getString(labels[i])).append(' ').append(context.getString(com.limelight.R.string.game_info_hours, hours[i]));
            }
            if (time.length() > 0) lines.add(context.getString(com.limelight.R.string.game_info_time_to_beat) + ": " + time);
        } catch (JSONException | RuntimeException | LinkageError e) {
            // No time to beat when the library cannot say.
        }
        return lines;
    }

    /** One planned request over HTTPS; never logs the URL, the headers or the body, which can hold keys. */
    @VisibleForTesting
    GameInfoModel.Reply send(JSONObject planned) {
        try {
            String method = planned.getString("method");
            Request.Builder request = new Request.Builder().url(planned.getString("url"));
            org.json.JSONArray headers = planned.optJSONArray("headers");
            for (int i = 0; headers != null && i < headers.length(); i++) {
                org.json.JSONArray pair = headers.getJSONArray(i);
                request.header(pair.getString(0), pair.getString(1));
            }
            String body = planned.isNull("body") ? null : planned.getString("body");
            if ("GET".equalsIgnoreCase(method)) {
                request.get();
            } else {
                request.method(method, RequestBody.create(body == null ? "" : body, MediaType.parse("text/plain; charset=utf-8")));
            }
            try (Response response = client.newCall(request.build()).execute()) {
                String text = response.body() == null ? "" : response.body().string();
                Long retry = null;
                String header = response.header("Retry-After");
                if (header != null) {
                    try {
                        retry = Long.parseLong(header.trim());
                    } catch (NumberFormatException ignored) {
                        // An HTTP date: the library waits its own default.
                    }
                }
                return new GameInfoModel.Reply(response.code(), text, retry);
            }
        } catch (IOException | JSONException | RuntimeException e) {
            return new GameInfoModel.Reply(0, "", null);
        }
    }
}
