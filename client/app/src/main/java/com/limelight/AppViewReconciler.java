package com.limelight;

import android.content.Context;

import com.limelight.computers.HostCapabilityCache;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.ui.ShellBar;
import com.limelight.utils.CacheHelper;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.io.StringReader;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;

/**
 * Cached app-list loading, server-info running-state sync, and UUID/ID-aware app catalogue
 * reconciliation with hidden-app ID remapping extracted from {@link AppView}.
 */
final class AppViewReconciler {
    private AppViewReconciler() {}

    static void populateAppGridWithCache(AppView view) {
        // The grid says it is loading until the saved list (or the host's) paints; a cache miss
        // must not wait for the reader to say so.
        view.loadAppsBlocking();
        Context app = view.getApplicationContext();
        String uuid = view.uuidString;
        Thread reader = new Thread(() -> {
            String raw = null;
            List<NvApp> parsed = null;
            try {
                // The cache can hold a few hundred games: read and parse it off the UI thread.
                raw = CacheHelper.readInputStreamToString(CacheHelper.openCacheFileForInput(app.getCacheDir(), "applist", uuid));
                parsed = NvHTTP.getAppListByReader(new StringReader(raw), HostCapabilityCache.load(app, uuid));
            } catch (IOException | XmlPullParserException e) {
                if (raw != null) {
                    LimeLog.warning("Saved applist corrupted");
                    e.printStackTrace();
                }
                LimeLog.info("Loading applist from the network");
            }
            String rawList = raw;
            List<NvApp> apps = parsed;
            if (apps == null) return;
            view.runOnUiThread(() -> {
                if (view.isFinishing() || view.isDestroyed() || view.appGridAdapter == null) return;
                // A poll may have brought the host's own list while the cache was read: it is newer.
                if (view.lastRawApplist != null) return;
                view.lastRawApplist = rawList;
                view.updateUiWithAppList(apps);
                LimeLog.info("Loaded applist from cache");
            });
        }, "applist-cache");
        reader.setDaemon(true);
        reader.start();
    }

    static void updateUiWithServerinfo(AppView view, final ComputerDetails details) {
        view.runOnUiThread(() -> {
            if (view.isFinishing() || view.isDestroyed()) {
                return;
            }
            if (view.appGridAdapter == null) return;
            boolean updated = false;

            // Update the full catalogue so a filtered or hidden running app never keeps a stale badge.
            for (AppView.AppObject app : view.appGridAdapter.getAllApps()) {
                boolean running = app.app.matchesRunningApp(details.runningGameId, details.runningGameUuid);
                if (app.isRunning != running) {
                    app.isRunning = running;
                    updated = true;
                }
            }

            if (updated) {
                view.appGridAdapter.notifyDataSetChanged();
                view.updateLibraryState();
            }
            view.viewChrome.showPcState(details.state != ComputerDetails.State.OFFLINE);
        });
    }

    static void updateUiWithAppList(AppView view, final List<NvApp> appList) {
        view.runOnUiThread(() -> {
            if (view.isFinishing() || view.isDestroyed()) {
                return;
            }
            view.lastAppListEmpty = appList.isEmpty();
            // The first saved list of a PC without the native API makes Home available.
            ShellBar.refresh(view);
            view.showAppListStatus(!view.computer.hasPermission(ComputerDetails.PERMISSION_LIST) ?
                    R.string.applist_permission_required : appList.isEmpty() ? R.string.library_no_apps : 0);
            boolean updated = false;
            List<AppView.AppObject> removedApps = view.appGridAdapter.getAllApps();
            Map<NvApp, AppView.AppObject> matches = matchApps(appList, removedApps);
            HashSet<Integer> remappedIds = new HashSet<>();
            HashSet<Integer> remappedHiddenIds = new HashSet<>();

            // First handle app updates and additions
            for (NvApp app : appList) {
                AppView.AppObject existingApp = matches.get(app);
                if (existingApp != null) {
                    if (existingApp.app.getAppId() != app.getAppId()) {
                        remappedIds.add(existingApp.app.getAppId());
                        if (existingApp.isHidden) remappedHiddenIds.add(app.getAppId());
                    }
                    // Found the app; update its properties
                    if (existingApp.app.getAppId() != app.getAppId() ||
                            !existingApp.app.getAppName().equals(app.getAppName()) ||
                            !existingApp.app.getAppUuid().equals(app.getAppUuid()) ||
                            existingApp.app.getHostIndex() != app.getHostIndex() ||
                            !existingApp.app.getArtVersion().equals(app.getArtVersion()) ||
                            existingApp.app.getRole() != app.getRole() ||
                            existingApp.app.isHdrSupported() != app.isHdrSupported()) {
                        existingApp.app = app;
                        updated = true;
                    }
                } else {
                    // This app must be new
                    view.appGridAdapter.addApp(new AppView.AppObject(app));

                    // We could have a leftover shortcut from last time this PC was paired
                    // or if this app was removed then added again. Enable those shortcuts
                    // again if present.
                    view.shortcutHelper.enableAppShortcut(view.computer, app);

                    updated = true;
                }
            }

            // Next handle app removals
            for (AppView.AppObject existingApp : removedApps) {
                for (NvApp app : appList) {
                    if (existingApp.app.getAppId() == app.getAppId() &&
                            !existingApp.app.getAppUuid().isEmpty() && !app.getAppUuid().isEmpty() &&
                            !existingApp.app.getAppUuid().equals(app.getAppUuid())) {
                        // A different known game owns this number now; it must not inherit hiding.
                        remappedIds.add(existingApp.app.getAppId());
                        break;
                    }
                }
                view.shortcutHelper.disableAppShortcut(view.computer, existingApp.app, "App removed from PC");
                view.appGridAdapter.removeApp(existingApp);
                updated = true;
            }

            if (!remappedIds.isEmpty()) {
                // Apply all ID moves together so hosts swapping IDs cannot lose a hidden choice.
                view.hiddenAppIds.removeAll(remappedIds);
                view.hiddenAppIds.addAll(remappedHiddenIds);
                view.updateHiddenApps(true);
            } else if (updated) {
                view.appGridAdapter.notifyDataSetChanged();
            }
            view.updateLibraryState();
        });
    }

    private static Map<NvApp, AppView.AppObject> matchApps(List<NvApp> apps, List<AppView.AppObject> unmatched) {
        Map<NvApp, AppView.AppObject> matches = new HashMap<>();
        // Resolve UUIDs before legacy numbers, using the original catalogue throughout. Otherwise
        // an ID move can make a legacy item consume a known game's identity later in the same poll.
        for (int pass = 0; pass < 2; pass++) {
            for (NvApp app : apps) {
                if (matches.containsKey(app)) continue;
                for (int i = 0; i < unmatched.size(); i++) {
                    NvApp previous = unmatched.get(i).app;
                    boolean bothHaveUuid = !previous.getAppUuid().isEmpty() && !app.getAppUuid().isEmpty();
                    boolean match = pass == 0 ? bothHaveUuid && previous.getAppUuid().equals(app.getAppUuid()) :
                            !bothHaveUuid && previous.getAppId() == app.getAppId();
                    if (match) {
                        matches.put(app, unmatched.remove(i));
                        break;
                    }
                }
            }
        }
        return matches;
    }
}
