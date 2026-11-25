package com.limelight.console;

import android.app.Activity;
import android.os.Handler;
import android.os.Looper;
import android.view.LayoutInflater;
import android.view.View;
import android.widget.LinearLayout;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.utils.UiHelper;

import org.json.JSONObject;

import java.lang.ref.WeakReference;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.function.Consumer;
import java.util.function.IntConsumer;
import java.util.function.Supplier;

/**
 * IGDB Genre and Length fact loading, prefetching, filtering and chip UI for {@link AllGamesActivity}.
 * Chips appear only while at least one game has cached facts and GameInfo facts are enabled.
 */
final class AllGamesFactFilter {
    private static final String TAG_GENRE = "game_info:genre";
    private static final String TAG_LENGTH = "game_info:length";

    private final Activity activity;
    private final LinearLayout filterRow;
    private final Supplier<HomeModel> model;
    private final Supplier<String> filter;
    private final Consumer<Boolean> bindGrid;

    private final Map<String, JSONObject> facts = new HashMap<>();
    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private GameInfoModel.Library factsLibrary;
    private String genreChoice;
    private String lengthChoice;
    private String factsLoadedFor;
    private boolean factsReloadPending;
    private boolean destroyed;

    AllGamesFactFilter(Activity activity, LinearLayout filterRow, Supplier<HomeModel> model,
                       Supplier<String> filter, Consumer<Boolean> bindGrid) {
        this.activity = activity;
        this.filterRow = filterRow;
        this.model = model;
        this.filter = filter;
        this.bindGrid = bindGrid;
    }

    /** Stops pending callbacks and lets queued background prefetch tasks drop their reference to this filter. */
    void destroy() {
        destroyed = true;
        factsReloadPending = false;
        mainHandler.removeCallbacksAndMessages(null);
    }

    /** Reads the facts kept for the library's games, then asks for the missing ones in the background. */
    void loadFacts() {
        if (destroyed || !GameInfo.factsWanted(activity)) return;
        List<HomeModel.GameCard> cards = AllGamesActivity.cardsOf(model.get(), HomeModel.FILTER_ALL);
        // The keys are part of it: corrected keys ask again for the games a refused key found nothing for.
        StringBuilder signature = new StringBuilder(GameInfo.keys(activity).json(true, true).toString()).append('|');
        for (HomeModel.GameCard card : cards) signature.append(card.key).append(';');
        if (signature.toString().equals(factsLoadedFor)) return;
        factsLoadedFor = signature.toString();
        List<String> keys = new ArrayList<>();
        List<GameInfo.Game> games = new ArrayList<>();
        for (HomeModel.GameCard card : cards) {
            keys.add(card.key);
            games.add(new GameInfo.Game(card.title, card.source == null ? null : card.source.gameId,
                    card.source == null ? null : card.source.storeSource, card.source == null ? null : card.source.storeId));
        }
        GameInfo info = GameInfo.get(activity);
        WeakReference<AllGamesFactFilter> selfRef = new WeakReference<>(this);
        Thread thread = new Thread(() -> {
            Map<String, JSONObject> found = new HashMap<>();
            List<GameInfo.Game> missing = new ArrayList<>();
            for (int i = 0; i < games.size(); i++) {
                AllGamesFactFilter current = selfRef.get();
                if (current == null || current.destroyed) return;
                JSONObject kept = info.cachedFacts(games.get(i));
                if (kept != null) found.put(keys.get(i), kept);
                else missing.add(games.get(i));
            }
            AllGamesFactFilter current = selfRef.get();
            if (current == null || current.destroyed) return;
            current.mainHandler.post(() -> {
                AllGamesFactFilter target = selfRef.get();
                if (target == null || target.destroyed || target.activity.isFinishing()
                        || target.activity.isDestroyed() || found.isEmpty()) return;
                target.facts.putAll(found);
                target.bindGrid.accept(false);
            });
            info.prefetchFacts(missing, () -> {
                AllGamesFactFilter target = selfRef.get();
                if (target != null) target.factsArrived();
            });
        }, "game-info-library");
        thread.setDaemon(true);
        thread.start();
    }

    /** A lookup found facts: read them again once things settle, not after each game. */
    private void factsArrived() {
        mainHandler.post(() -> {
            if (destroyed || factsReloadPending || activity.isFinishing() || activity.isDestroyed()) return;
            factsReloadPending = true;
            mainHandler.postDelayed(() -> {
                factsReloadPending = false;
                if (destroyed || activity.isFinishing() || activity.isDestroyed()) return;
                factsLoadedFor = null;
                loadFacts();
            }, 2000);
        });
    }

    /** The cards the Genre and Length choices leave; every card when no game has facts. Draws the chips. */
    List<HomeModel.GameCard> withFacts(List<HomeModel.GameCard> cards) {
        factsLibrary = null;
        if (facts.isEmpty() || !GameInfo.factsWanted(activity)) {
            bindFactChips();
            return cards;
        }
        List<HomeModel.GameCard> sourceCards = AllGamesActivity.cardsOf(model.get(), filter.get());
        GameInfoFilter.Result<HomeModel.GameCard> result = GameInfoFilter.apply(sourceCards,
                card -> facts.get(card.key), genreChoice, lengthChoice);
        if (result == null) {
            bindFactChips();
            return cards;
        }
        // A choice the library no longer has (the last game of a genre went) falls back to Any.
        boolean stale = false;
        if (genreChoice != null && !hasValue(result.library.genres, genreChoice)) {
            genreChoice = null;
            stale = true;
        }
        if (lengthChoice != null && !hasValue(result.library.lengths, lengthChoice)) {
            lengthChoice = null;
            stale = true;
        }
        if (stale) {
            result = GameInfoFilter.apply(sourceCards, card -> facts.get(card.key), genreChoice, lengthChoice);
            if (result == null) {
                bindFactChips();
                return cards;
            }
        }
        factsLibrary = result.library;
        bindFactChips();
        if (genreChoice == null && lengthChoice == null) return cards;
        Set<String> keep = new HashSet<>();
        for (HomeModel.GameCard card : result.shown) keep.add(card.key);
        List<HomeModel.GameCard> out = new ArrayList<>();
        for (HomeModel.GameCard card : cards) {
            if (keep.contains(card.key)) out.add(card);
        }
        return out;
    }

    private static boolean hasValue(List<GameInfoModel.Count> counts, String value) {
        for (GameInfoModel.Count count : counts) {
            if (count.value.equals(value)) return true;
        }
        return false;
    }

    /** The Genre and Length chips after the source chips, present while some game has facts. */
    void bindFactChips() {
        View genre = filterRow.findViewWithTag(TAG_GENRE);
        View length = filterRow.findViewWithTag(TAG_LENGTH);
        boolean show = factsLibrary != null && factsLibrary.any();
        if (!show) {
            if (genre != null) filterRow.removeView(genre);
            if (length != null) filterRow.removeView(length);
            return;
        }
        if (genre == null) genre = addFactChip(TAG_GENRE, this::chooseGenre);
        if (length == null) length = addFactChip(TAG_LENGTH, this::chooseLength);
        ((MaterialButton) genre).setText(genreChoice == null ? activity.getString(R.string.game_info_genre) : genreChoice);
        genre.setSelected(genreChoice != null);
        ((MaterialButton) length).setText(lengthChoice == null ? activity.getString(R.string.game_info_length)
                : activity.getString(GameInfoFilter.lengthLabel(lengthChoice)));
        length.setSelected(lengthChoice != null);
    }

    private View addFactChip(String tag, Runnable open) {
        MaterialButton chip = (MaterialButton) LayoutInflater.from(activity)
                .inflate(R.layout.view_filter_chip, filterRow, false);
        chip.setTag(tag);
        chip.setOnClickListener(v -> open.run());
        filterRow.addView(chip);
        return chip;
    }

    private void chooseGenre() {
        if (factsLibrary == null) return;
        List<GameInfoModel.Count> counts = factsLibrary.genres;
        CharSequence[] names = new CharSequence[counts.size() + 1];
        names[0] = activity.getString(R.string.game_info_any_genre);
        int checked = 0;
        for (int i = 0; i < counts.size(); i++) {
            names[i + 1] = counts.get(i).value + "  " + counts.get(i).count;
            if (counts.get(i).value.equals(genreChoice)) checked = i + 1;
        }
        showFactChoices(R.string.game_info_genre, names, checked,
                which -> genreChoice = which == 0 ? null : counts.get(which - 1).value);
    }

    private void chooseLength() {
        if (factsLibrary == null) return;
        List<GameInfoModel.Count> counts = factsLibrary.lengths;
        CharSequence[] names = new CharSequence[counts.size() + 1];
        names[0] = activity.getString(R.string.game_info_any_length);
        int checked = 0;
        for (int i = 0; i < counts.size(); i++) {
            names[i + 1] = activity.getString(GameInfoFilter.lengthLabel(counts.get(i).value)) + "  " + counts.get(i).count;
            if (counts.get(i).value.equals(lengthChoice)) checked = i + 1;
        }
        showFactChoices(R.string.game_info_length, names, checked,
                which -> lengthChoice = which == 0 ? null : counts.get(which - 1).value);
    }

    /** A list dialog a controller or the D-pad reaches; the choice combines with the source filter. */
    private void showFactChoices(int title, CharSequence[] names, int checked, IntConsumer pick) {
        AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle(title)
                .setSingleChoiceItems(names, checked, (d, which) -> {
                    pick.accept(which);
                    d.dismiss();
                    bindGrid.accept(true);
                }).setNegativeButton(android.R.string.cancel, null).create();
        UiHelper.showDialog(activity, dialog);
    }
}
