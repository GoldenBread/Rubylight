package com.limelight.console;

import android.content.Context;
import android.text.Editable;
import android.text.TextWatcher;
import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;

import androidx.activity.ComponentActivity;
import androidx.activity.OnBackPressedCallback;

import com.limelight.R;
import com.limelight.ui.GlassSurface;
import com.limelight.ui.Motion;

import java.util.function.BooleanSupplier;
import java.util.function.Consumer;

/**
 * In-page search bar controller for {@link AllGamesActivity}: swaps the top row for the search field, tracks the
 * query text, manages the soft keyboard and Back handling, and moves focus down to the first matching poster.
 */
final class AllGamesSearchBar {
    private static final int SEARCH_FADE_MS = 150;

    private final ComponentActivity activity;
    private final View topRow;
    private final View tools;
    private final Runnable onOpened;
    private final Consumer<Boolean> bindGrid;
    private final BooleanSupplier focusFirstMatch;

    private String query = "";
    private GlassSurface searchBar;
    private EditText searchField;
    private View searchClear;
    private View searchButton;
    private final OnBackPressedCallback closeSearchOnBack = new OnBackPressedCallback(false) {
        @Override public void handleOnBackPressed() {
            close();
        }
    };

    AllGamesSearchBar(ComponentActivity activity, View topRow, View tools, Runnable onOpened,
                      Consumer<Boolean> bindGrid, BooleanSupplier focusFirstMatch) {
        this.activity = activity;
        this.topRow = topRow;
        this.tools = tools;
        this.onOpened = onOpened;
        this.bindGrid = bindGrid;
        this.focusFirstMatch = focusFirstMatch;
    }

    /** Wires the search views and restores any query saved across recreation. */
    void install(String restored) {
        searchBar = activity.findViewById(R.id.home_search_bar);
        searchField = activity.findViewById(R.id.home_search_field);
        searchClear = activity.findViewById(R.id.home_search_clear);
        searchButton = activity.findViewById(R.id.all_games_search);
        searchButton.setOnClickListener(v -> open());
        activity.getOnBackPressedDispatcher().addCallback(activity, closeSearchOnBack);
        searchField.setOnFocusChangeListener((v, focused) -> searchBar.setFocusShown(focused && !v.isInTouchMode()));
        searchField.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int start, int count, int after) { }
            @Override public void onTextChanged(CharSequence s, int start, int before, int count) { }
            @Override public void afterTextChanged(Editable text) {
                if (!isSearching()) return;
                query = text.toString();
                searchClear.setVisibility(query.isEmpty() ? View.GONE : View.VISIBLE);
                bindGrid.accept(true);
            }
        });
        searchField.setOnEditorActionListener((v, action, event) -> {
            if (action != EditorInfo.IME_ACTION_SEARCH) return false;
            keyboard(false);
            focusFirstMatch.getAsBoolean();
            return true;
        });
        searchField.setOnKeyListener((v, keyCode, event) -> {
            if (keyCode != KeyEvent.KEYCODE_DPAD_DOWN || event.getAction() != KeyEvent.ACTION_DOWN) return false;
            keyboard(false);
            return focusFirstMatch.getAsBoolean();
        });
        searchClear.setOnClickListener(v -> {
            searchField.setText("");
            searchField.requestFocus();
        });
        if (restored != null) {
            open();
            searchField.setText(restored);
            searchField.setSelection(restored.length());
        }
    }

    String query() {
        return query;
    }

    boolean isSearching() {
        return searchBar != null && searchBar.getVisibility() == View.VISIBLE;
    }

    void open() {
        if (isSearching()) return;
        onOpened.run();
        query = "";
        searchField.setText("");
        searchClear.setVisibility(View.GONE);
        topRow.setVisibility(View.INVISIBLE);
        tools.setVisibility(View.INVISIBLE);
        searchBar.setVisibility(View.VISIBLE);
        closeSearchOnBack.setEnabled(true);
        comeIn(searchBar);
        searchField.requestFocus();
        keyboard(true);
    }

    /** All the filter's games again, and focus back on Search. */
    void close() {
        if (!isSearching()) return;
        keyboard(false);
        closeSearchOnBack.setEnabled(false);
        searchBar.setVisibility(View.GONE);
        topRow.setVisibility(View.VISIBLE);
        tools.setVisibility(View.VISIBLE);
        query = "";
        bindGrid.accept(true);
        if (!searchButton.isInTouchMode()) searchButton.requestFocus();
    }

    private void keyboard(boolean show) {
        InputMethodManager manager = (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
        if (manager == null) return;
        if (show) manager.showSoftInput(searchField, InputMethodManager.SHOW_IMPLICIT);
        else manager.hideSoftInputFromWindow(searchField.getWindowToken(), 0);
    }

    private void comeIn(View view) {
        view.animate().cancel();
        if (Motion.reduced(activity)) {
            view.setAlpha(1f);
            return;
        }
        view.setAlpha(0f);
        view.animate().alpha(1f).setDuration(SEARCH_FADE_MS).start();
    }
}
