package com.limelight.console;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;

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

/**
 * The wire contract with libnightjar_core: {@link CollectionGroups} sends its requests through
 * {@link NightjarJni#collectionsCall} and reads the answers matching protocol/android/src/collections_contract.json.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class CollectionGroupsTest {
    private static JSONArray cases() throws Exception {
        for (String path : new String[] {"../protocol/android/src/collections_contract.json", "protocol/android/src/collections_contract.json"}) {
            File file = new File(path);
            if (file.isFile()) return new JSONArray(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        }
        throw new AssertionError("collections_contract.json not found from " + new File(".").getAbsolutePath());
    }

    private static List<List<String>> gamesOf(JSONObject request) throws Exception {
        JSONArray games = request.getJSONArray("games");
        List<List<String>> out = new ArrayList<>();
        for (int i = 0; i < games.length(); i++) {
            List<String> names = new ArrayList<>();
            for (int j = 0; j < games.getJSONArray(i).length(); j++) names.add(games.getJSONArray(i).getString(j));
            out.add(names);
        }
        return out;
    }

    @Test
    public void sendsTheContractsRequestAndReadsItsAnswer() throws Exception {
        JSONObject several = cases().getJSONObject(0);
        List<CollectionGroups.Group> groups = CollectionGroups.of(gamesOf(several.getJSONObject("request")));
        assertEquals(3, groups.size());
        assertEquals("Couch", groups.get(0).name);
        assertArrayEquals(new int[] {0, 2}, groups.get(0).games);
        assertEquals("VR", groups.get(1).name);
        assertNull(groups.get(2).name);
        assertArrayEquals(new int[] {1}, groups.get(2).games);
    }

    @Test
    public void noCollectionsAsksNothing() throws Exception {
        List<List<String>> games = gamesOf(cases().getJSONObject(1).getJSONObject("request"));
        assertFalse(CollectionGroups.needsGrouping(games));
        NightjarJni.forceUnavailableForTest(new AssertionError("asked"));
        try {
            assertTrue(CollectionGroups.of(games).isEmpty());
        } finally {
            NightjarJni.forceUnavailableForTest(null);
        }
    }

    @Test
    public void aRefusalOrAMissingLibraryIsNoGroups() {
        List<List<String>> games = Collections.singletonList(Arrays.asList("VR"));
        assertTrue(CollectionGroups.parseGroups("{\"error\":\"invalid\",\"detail\":\"x\"}").isEmpty());
        NightjarJni.forceUnavailableForTest(new UnsatisfiedLinkError("no library"));
        try {
            assertTrue(CollectionGroups.of(games).isEmpty());
        } finally {
            NightjarJni.forceUnavailableForTest(null);
        }
    }
}
