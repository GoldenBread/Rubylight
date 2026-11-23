package com.limelight.console;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

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
import java.util.List;

/**
 * The wire contract with libnightjar_core: {@link LibraryOrder} sends its requests through
 * {@link NightjarJni#libraryOrderCall} and reads the answers matching protocol/android/src/library_order_contract.json.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class LibraryOrderTest {
    private static JSONArray cases() throws Exception {
        for (String path : new String[] {"../protocol/android/src/library_order_contract.json", "protocol/android/src/library_order_contract.json"}) {
            File file = new File(path);
            if (file.isFile()) return new JSONArray(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        }
        throw new AssertionError("library_order_contract.json not found from " + new File(".").getAbsolutePath());
    }

    private static List<LibraryOrder.Facts> gamesOf(JSONObject request) throws Exception {
        JSONArray games = request.getJSONArray("games");
        List<LibraryOrder.Facts> out = new ArrayList<>();
        for (int i = 0; i < games.length(); i++) {
            JSONObject game = games.getJSONObject(i);
            out.add(new LibraryOrder.Facts(game.getString("title"),
                    game.has("last_played") ? game.getLong("last_played") : null,
                    game.has("playtime_minutes") ? game.getLong("playtime_minutes") : null));
        }
        return out;
    }

    private static int[] orderOf(JSONObject response) throws Exception {
        JSONArray places = response.getJSONObject("ok").getJSONArray("order");
        int[] at = new int[places.length()];
        for (int i = 0; i < at.length; i++) at[i] = places.getInt(i);
        return at;
    }

    @Test
    public void sendsEachContractRequestAndReadsItsAnswer() throws Exception {
        List<Object[]> sorts = Arrays.asList(
                new Object[] {HomeView.Sort.LAST_PLAYED, HomeView.Order.DESCENDING},
                new Object[] {HomeView.Sort.PLAYTIME, HomeView.Order.ASCENDING},
                new Object[] {HomeView.Sort.TITLE, HomeView.Order.ASCENDING});
        for (int i = 0; i < sorts.size(); i++) {
            JSONObject contract = cases().getJSONObject(i);
            int[] order = LibraryOrder.of(gamesOf(contract.getJSONObject("request")), (HomeView.Sort) sorts.get(i)[0],
                    (HomeView.Order) sorts.get(i)[1]);
            assertArrayEquals(contract.getString("name"), orderOf(contract.getJSONObject("response")), order);
        }
    }

    @Test
    public void aRefusalOrAWrongLengthKeepsTheOrderItHas() throws Exception {
        List<LibraryOrder.Facts> two = gamesOf(cases().getJSONObject(2).getJSONObject("request"));
        assertNull(LibraryOrder.parseOrder("{\"error\":\"invalid\",\"detail\":\"no\"}", two.size()));
        assertNull(LibraryOrder.parseOrder("{\"ok\":{\"order\":[0]}}", two.size()));
        NightjarJni.forceUnavailableForTest(new UnsatisfiedLinkError("no library"));
        try {
            assertNull(LibraryOrder.of(two, HomeView.Sort.TITLE, HomeView.Order.DESCENDING));
        } finally {
            NightjarJni.forceUnavailableForTest(null);
        }
    }
}
