package com.limelight.console;

import androidx.annotation.VisibleForTesting;

import com.limelight.nvstream.jni.NightjarJni;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Home's groups by collection, the same ones desktop Nightjar draws: nightjar-policy's collections
 * (protocol/policy), reached through libnightjar_core (protocol/android/src/collections.rs).
 */
public final class CollectionGroups {
    private CollectionGroups() { }

    /** One group: its collection's name, null for the games in none, and the places of its games in the list asked about. */
    public static final class Group {
        public final String name;
        public final int[] games;

        Group(String name, int[] games) {
            this.name = name;
            this.games = games;
        }
    }

    @VisibleForTesting
    static boolean needsGrouping(List<List<String>> games) {
        for (List<String> names : games) {
            if (!names.isEmpty()) return true;
        }
        return false;
    }

    @VisibleForTesting
    static List<Group> parseGroups(String answer) {
        try {
            JSONObject root = new JSONObject(answer);
            JSONObject ok = root.optJSONObject("ok");
            if (ok == null) throw new JSONException(root.toString());
            JSONArray groups = ok.getJSONArray("groups");
            List<Group> out = new ArrayList<>(groups.length());
            for (int i = 0; i < groups.length(); i++) {
                JSONObject group = groups.getJSONObject(i);
                JSONArray places = group.getJSONArray("games");
                int[] at = new int[places.length()];
                for (int j = 0; j < at.length; j++) at[j] = places.getInt(j);
                out.add(new Group(group.isNull("name") ? null : group.getString("name"), at));
            }
            return out;
        } catch (JSONException | RuntimeException e) {
            com.limelight.LimeLog.warning("Collections could not be grouped: " + e);
            return Collections.emptyList();
        }
    }

    /**
     * The groups of these games' collections by place: a group per collection by name, then the games in none. Empty
     * when no game is in one, without asking, and when the library cannot answer: Home then shows its rows by store.
     */
    static List<Group> of(List<List<String>> games) {
        if (!needsGrouping(games)) return Collections.emptyList();
        try {
            JSONArray request = new JSONArray();
            for (List<String> names : games) {
                request.put(new JSONArray(names));
            }
            return parseGroups(NightjarJni.collectionGroupsCall(new JSONObject().put("games", request).toString()));
        } catch (JSONException | LinkageError | RuntimeException e) {
            com.limelight.LimeLog.warning("Collections could not be grouped: " + e);
            return Collections.emptyList();
        }
    }
}
