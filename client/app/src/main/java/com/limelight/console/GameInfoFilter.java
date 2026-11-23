package com.limelight.console;

import androidx.annotation.VisibleForTesting;

import com.limelight.R;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;
import java.util.function.Function;

/** All games' Genre and Length filters: the choice, and what it leaves of the games the source filter shows. */
final class GameInfoFilter {
    private GameInfoFilter() { }

    /** The games a choice shows, in their order, and the library's genres and lengths over all the games asked about. */
    static final class Result<T> {
        final List<T> shown;
        final GameInfoModel.Library library;

        Result(List<T> shown, GameInfoModel.Library library) {
            this.shown = shown;
            this.library = library;
        }
    }

    @VisibleForTesting
    static GameInfoModel.Library parseLibrary(String answer) {
        try {
            return GameInfoModel.readLibrary(GameInfoModel.parseAnswer(answer));
        } catch (JSONException | RuntimeException e) {
            return null;
        }
    }

    /**
     * {@code games} are the ones the source filter shows; {@code factsOf} gives a game's facts or null. The result
     * keeps the games {@code genre} and {@code length} (either may be null) let through, so the choices combine with
     * the source filter; null when the library cannot answer, and then the games stay as they are.
     */
    static <T> Result<T> apply(List<T> games, Function<T, JSONObject> factsOf, String genre, String length) {
        try {
            List<JSONObject> facts = new ArrayList<>(games.size());
            for (T game : games) facts.add(factsOf.apply(game));
            GameInfoModel.Library library = GameInfoModel.library(facts, genre, length);
            List<T> shown = new ArrayList<>(library.shown.length);
            for (int at : library.shown) shown.add(games.get(at));
            return new Result<>(shown, library);
        } catch (JSONException | RuntimeException | LinkageError e) {
            return null;
        }
    }

    /** The name a length key goes by in the chip's list: Under 5 hours, 5 to 15 hours, 15 to 40 hours, Over 40 hours. */
    static int lengthLabel(String length) {
        switch (length) {
            case "short":
                return R.string.game_info_length_short;
            case "medium":
                return R.string.game_info_length_medium;
            case "long":
                return R.string.game_info_length_long;
            default:
                return R.string.game_info_length_very_long;
        }
    }
}
