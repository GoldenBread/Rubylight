package com.limelight.console;

import androidx.annotation.VisibleForTesting;

import com.limelight.nvstream.jni.NightjarJni;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.List;
import java.util.Locale;

/**
 * The order Home and All games list a library in, the same one desktop Nightjar sorts with: nightjar-policy's
 * library_order (protocol/policy), reached through libnightjar_core (protocol/android/src/library_order.rs).
 */
public final class LibraryOrder {
    private LibraryOrder() { }

    /** What the order reads of one game: its title, and when and how long its PCs say it was played (null: none says). */
    public static final class Facts {
        final String title;
        final Long lastPlayed;
        final Long playtimeMinutes;

        public Facts(String title, Long lastPlayed, Long playtimeMinutes) {
            this.title = title;
            this.lastPlayed = lastPlayed;
            this.playtimeMinutes = playtimeMinutes;
        }
    }

    @VisibleForTesting
    static int[] parseOrder(String answer, int count) {
        try {
            JSONObject root = new JSONObject(answer);
            JSONObject ok = root.optJSONObject("ok");
            if (ok == null) throw new JSONException(root.toString());
            JSONArray places = ok.getJSONArray("order");
            if (places.length() != count) throw new JSONException("an order of " + places.length() + " for " + count);
            int[] at = new int[places.length()];
            for (int i = 0; i < at.length; i++) at[i] = places.getInt(i);
            return at;
        } catch (JSONException | RuntimeException e) {
            com.limelight.LimeLog.warning("The library could not be sorted: " + e);
            return null;
        }
    }

    /**
     * The places of these games in the chosen order. Null when the library cannot answer: the caller keeps the order it
     * has, by title.
     */
    static int[] of(List<Facts> games, HomeView.Sort sort, HomeView.Order order) {
        try {
            JSONArray list = new JSONArray();
            for (Facts game : games) {
                JSONObject facts = new JSONObject().put("title", game.title == null ? "" : game.title);
                if (game.lastPlayed != null) facts.put("last_played", game.lastPlayed.longValue());
                if (game.playtimeMinutes != null) facts.put("playtime_minutes", game.playtimeMinutes.longValue());
                list.put(facts);
            }
            JSONObject request = new JSONObject().put("key", sort.name().toLowerCase(Locale.ROOT))
                    .put("direction", order.name().toLowerCase(Locale.ROOT)).put("games", list);
            return parseOrder(NightjarJni.libraryOrderCall(request.toString()), games.size());
        } catch (JSONException | LinkageError | RuntimeException e) {
            com.limelight.LimeLog.warning("The library could not be sorted: " + e);
            return null;
        }
    }
}
