package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;

import androidx.appcompat.app.AlertDialog;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.ClientHintBitrateController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.RustStreamMenuModel;
import com.limelight.ui.StreamMenuModel;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.shadows.ShadowToast;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static com.limelight.nvstream.RateChangeResult.*;
import static org.junit.Assert.*;

// The stream menu's 0x553C dialog, what Game says about the host's answers, and automatic
// bitrate on this device, with the bridge replaced by a recording sender.
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameRateChangeTest {
    // What went out: {serial, kbps, mHz}.
    private final List<int[]> sent = new ArrayList<>();
    private Game game;
    private PreferenceConfiguration prefs;

    @Before
    public void streamingGame() {
        game = Robolectric.buildActivity(Game.class).get();
        game.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "rate-change-pc"));
        game.setTheme(R.style.StreamTheme);
        game.setContentView(R.layout.activity_game);
        prefs = new PreferenceConfiguration();
        prefs.bitrate = 50_000;
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        NvApp app = new NvApp("Game", 73, false);
        ReflectionHelpers.setField(game, "app", app);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "currentBitrate", 50_000);
        ReflectionHelpers.setField(game, "streamFps", 60);
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "test", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
            @Override public void stop() {}
        };
        ReflectionHelpers.setField(connection, "hostDetails", TestHosts.details("rubylight-serverinfo.xml"));
        ReflectionHelpers.setField(game, "conn", connection);
        useRenderer();
        ShadowDialog.reset();
        ShadowToast.reset();
    }

    // The queries for the stream's bitrate (bitrate and rate 0), apart from what sets one; the order of both.
    private final List<int[]> queries = new ArrayList<>();
    private final List<String> order = new ArrayList<>();
    // Queries the sender cannot send yet, as a control stream that cannot carry them.
    private int unsentQueries;

    private StreamRateChanges recording() {
        return new StreamRateChanges((serial, kbps, mhz) -> {
            if (kbps == 0 && mhz == 0) {
                if (unsentQueries > 0) {
                    unsentQueries--;
                    return false;
                }
                queries.add(new int[] {serial, kbps, mhz});
                order.add("query");
                return true;
            }
            sent.add(new int[] {serial, kbps, mhz});
            order.add("change");
            return true;
        });
    }

    // A stream start on a PC that answers the query at once with the stream's own bitrate, as one running Rubylight
    // from after 6.0.1 does for the launch's.
    private void startStream(StreamRateChanges changes) {
        int asked = queries.size();
        startStreamAsking(changes);
        if (changes != null && queries.size() > asked) {
            answerQuery((int) ReflectionHelpers.getField(game, "streamStartBitrateKbps"));
        }
    }

    // A stream start whose query the test answers.
    private void startStreamAsking(StreamRateChanges changes) {
        ReflectionHelpers.callInstanceMethod(game.adaptation, "startStreamBitrate",
                ReflectionHelpers.ClassParameter.from(StreamRateChanges.class, changes));
    }

    // The PC's answer to the last query: the stream's bitrate as it counts it, nothing changed.
    private void answerQuery(int kbps) {
        assertFalse("a query went out", queries.isEmpty());
        game.rateChange(new RateChangeResult(queries.get(queries.size() - 1)[0], kbps, 60_000, STATUS_UNCHANGED, REASON_NONE,
                STATUS_REFUSED, REASON_INVALID));
        ShadowLooper.idleMainLooper();
    }

    private AlertDialog openDialog() {
        ReflectionHelpers.callInstanceMethod(game.adaptation, "showRateChangeDialog");
        AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
        assertNotNull(dialog);
        assertTrue(dialog.isShowing());
        return dialog;
    }

    // The dialog's fields in order: bitrate, then frame rate when it shows.
    private static List<EditText> fields(AlertDialog dialog) {
        List<EditText> found = new ArrayList<>();
        collect(dialog.getWindow().getDecorView(), found);
        return found;
    }

    private static void collect(View view, List<EditText> found) {
        if (view instanceof EditText) found.add((EditText) view);
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collect(group.getChildAt(i), found);
        }
    }

    private static void send(AlertDialog dialog) {
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
        ShadowLooper.idleMainLooper();
    }

    private void useRenderer() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(game, prefs, null, 0,
                false, false, "", null) {
            @Override public boolean hasRecentVideoFrames(long nowMs) { return true; }
        };
        ReflectionHelpers.setField(game, "decoderRenderer", renderer);
    }

    // Seconds of stream time, one automatic bitrate sample each.
    private static void runFor(int seconds) {
        for (int i = 0; i < seconds; i++) ShadowLooper.idleMainLooper(1, TimeUnit.SECONDS);
    }

    // The start delay, then clean samples until automatic bitrate asks for more.
    private void runUntilAutomaticAsks() {
        runFor((int) (ClientHintBitrateController.START_DELAY_MS / 1000) +
                ClientHintBitrateController.STABLE_SAMPLES + 2);
    }

    @Test
    public void theDialogSendsBitrateAndFrameRateInOneRequest() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        List<EditText> fields = fields(dialog);
        assertEquals(2, fields.size());
        assertEquals("50", fields.get(0).getText().toString());
        assertEquals("60", fields.get(1).getText().toString());
        fields.get(0).setText("30,5");
        fields.get(1).setText("90");
        send(dialog);
        assertEquals(1, sent.size());
        assertEquals(30_500, sent.get(0)[1]);
        assertEquals(90_000, sent.get(0)[2]);
        assertFalse(dialog.isShowing());
    }

    @Test
    public void aFieldLeftAsItIsKeepsItsValueAndNothingChangedSendsNothing() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        send(dialog);
        assertTrue("nothing to change", sent.isEmpty());
        assertTrue(dialog.isShowing());

        fields(dialog).get(1).setText("120");
        send(dialog);
        assertEquals(1, sent.size());
        assertEquals("the bitrate stays", 0, sent.get(0)[1]);
        assertEquals(120_000, sent.get(0)[2]);
    }

    @Test
    public void sendAfterTheStreamEndedClosesTheDialogAndSaysSo() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        fields(dialog).get(0).setText("30");
        ReflectionHelpers.setField(game, "connected", false);
        send(dialog);
        assertTrue(sent.isEmpty());
        assertFalse(dialog.isShowing());
        assertEquals(game.getString(R.string.stream_rate_change_stream_ended), ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void sendAfterTheTerminationArrivedClosesTheDialogAndSaysTheStreamEnded() {
        // The Retroid's R8: the host died with the dialog open. connectionTerminated marks the
        // stream terminated at once; connected stays true until stopConnection closes the dialog,
        // so this text never showed and "not sent" or "no answer" did instead.
        startStream(recording());
        AlertDialog dialog = openDialog();
        fields(dialog).get(0).setText("30");
        ReflectionHelpers.setField(game, "streamTerminated", true);
        assertTrue((boolean) ReflectionHelpers.getField(game, "connected"));
        send(dialog);
        assertTrue(sent.isEmpty());
        assertFalse(dialog.isShowing());
        assertEquals(game.getString(R.string.stream_rate_change_stream_ended), ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void sendAfterTheStreamRestartedClosesTheDialogInsteadOfAskingTheNewStream() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        fields(dialog).get(0).setText("30");
        // The stream restarted while the dialog was open: a new stream with its own rate changes.
        startStream(recording());
        send(dialog);
        assertTrue(sent.isEmpty());
        assertFalse(dialog.isShowing());
        assertEquals(game.getString(R.string.stream_rate_change_stream_ended), ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void theHostsAnswerShowsWhatBecameOfEachValueRefusalsWithTheirReason() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        List<EditText> fields = fields(dialog);
        fields.get(0).setText("30");
        fields.get(1).setText("120");
        send(dialog);
        int serial = sent.get(0)[0];

        game.rateChange(new RateChangeResult(serial, 30_000, 60_000, STATUS_APPLIED, REASON_NONE,
                STATUS_REFUSED, REASON_DISABLED));
        ShadowLooper.idleMainLooper();
        assertEquals("Bitrate: 30.0 Mbps\nFrame rate not changed, still 60 fps. The PC has these changes turned off.",
                ShadowToast.getTextOfLatestToast());
        assertEquals(30_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
        assertEquals("a refused rate keeps the stream's", 60, (int) ReflectionHelpers.getField(game, "streamFps"));

        // A result for no request of this stream (one from before a restart, say) says nothing.
        int toasts = ShadowToast.shownToastCount();
        game.rateChange(new RateChangeResult(serial, 10_000, 0, STATUS_APPLIED, REASON_NONE,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertEquals(toasts, ShadowToast.shownToastCount());
        assertEquals(30_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
    }

    @Test
    public void anAnswerThatComesAfterALaterRequestsKeepsThatOnesBitrate() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        List<EditText> fields = fields(dialog);
        fields.get(0).setText("30");
        fields.get(1).setText("120");
        send(dialog);
        dialog = openDialog();
        fields(dialog).get(0).setText("40");
        send(dialog);
        assertEquals(2, sent.size());

        // The host answers the bitrate-only request at once and the first once its switch ended,
        // here back at 60 fps, with the bitrate it set when that request came.
        game.rateChange(new RateChangeResult(sent.get(1)[0], 40_000, 60_000, STATUS_APPLIED, REASON_NONE,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertEquals(40_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
        game.rateChange(new RateChangeResult(sent.get(0)[0], 30_000, 60_000, STATUS_APPLIED, REASON_NONE,
                STATUS_REFUSED, REASON_ENCODER_FAILED));
        ShadowLooper.idleMainLooper();
        assertEquals("Bitrate: 30.0 Mbps\nFrame rate not changed, still 60 fps. " +
                        game.getString(R.string.stream_rate_change_reason_encoder_failed),
                ShadowToast.getTextOfLatestToast());
        assertEquals("the later request's bitrate stays", 40_000,
                (int) ReflectionHelpers.getField(game, "currentBitrate"));
    }

    @Test
    public void aChangeTheHostNeverAnswersIsGivenUpOnLoudly() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        fields(dialog).get(0).setText("30");
        send(dialog);
        runFor(4);
        assertNotEquals(game.getString(R.string.stream_rate_change_no_answer), ShadowToast.getTextOfLatestToast());
        runFor(1);
        assertEquals(game.getString(R.string.stream_rate_change_no_answer), ShadowToast.getTextOfLatestToast());

        // An answer after that is for no request any more.
        int toasts = ShadowToast.shownToastCount();
        game.rateChange(new RateChangeResult(sent.get(0)[0], 30_000, 60_000, STATUS_APPLIED, REASON_NONE,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertEquals(toasts, ShadowToast.shownToastCount());
        assertEquals(50_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
    }

    @Test
    public void automaticBitrateStopsWhenTheHostNeverAnswersItsRequest() {
        prefs.rateChangeBitrate = true;
        startStream(recording());
        runUntilAutomaticAsks();
        assertEquals(1, sent.size());
        runFor((int) (StreamRateChanges.ANSWER_MS / 1000));
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        assertEquals(game.getString(R.string.stream_rate_change_auto_no_answer), ShadowToast.getTextOfLatestToast());
        runFor(20);
        assertEquals("nothing more goes out", 1, sent.size());
    }

    @Test
    public void anAppliedBitrateFromTheOpenMenusSliderShowsNoToastOverTheMenu() {
        // A real stream on the Retroid: "Bitrate: 32.0 Mbps" lay over the menu's Disconnect while the slider showed 32 already.
        StreamMenuModel.use(new RustStreamMenuModel());
        try {
            startStream(recording());
            game.showStreamMenu();
            assertNotNull("the menu is open", ReflectionHelpers.getField(game.menuActions, "streamMenuSheet"));
            int toasts = ShadowToast.shownToastCount();
            assertEquals(32_000, game.adaptation.applyBitrateFromMenu(32_000));
            game.rateChange(new RateChangeResult(sent.get(0)[0], 32_000, 0, STATUS_APPLIED, REASON_NONE,
                    STATUS_UNCHANGED, REASON_NONE));
            ShadowLooper.idleMainLooper();
            assertEquals("the slider shows it", toasts, ShadowToast.shownToastCount());
            assertEquals(32_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));

            // A clamp is news the slider cannot show.
            game.adaptation.applyBitrateFromMenu(90_000);
            game.rateChange(new RateChangeResult(sent.get(1)[0], 80_000, 0, STATUS_CLAMPED, REASON_ENCODER_LIMIT,
                    STATUS_UNCHANGED, REASON_NONE));
            ShadowLooper.idleMainLooper();
            assertEquals(toasts + 1, ShadowToast.shownToastCount());
        } finally {
            StreamMenuModel.use(null);
        }
    }

    /** A host without the stream's rate changes takes the menu's bitrate over HTTP: the same rule for its toast. */
    @Test
    public void anAppliedLegacyBitrateFromTheOpenMenuShowsNoToastOverTheMenu() throws Exception {
        int[] answer = {32_000};
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "test", new StreamConfiguration.Builder().setApp(new NvApp("Game", 73, false)).build(), null, null) {
            @Override public void stop() {}
            @Override public int setBitrate(int kbps) { return answer[0]; }
        };
        ReflectionHelpers.setField(connection, "hostDetails", TestHosts.details("rubylight-serverinfo.xml"));
        ReflectionHelpers.setField(game, "conn", connection);
        StreamMenuModel.use(new RustStreamMenuModel());
        try {
            game.showStreamMenu();
            assertNotNull("the menu is open", ReflectionHelpers.getField(game.menuActions, "streamMenuSheet"));
            int toasts = ShadowToast.shownToastCount();
            assertEquals(32_000, game.adaptation.applyBitrateFromMenu(32_000));
            awaitHostAction();
            assertEquals("the slider shows it", toasts, ShadowToast.shownToastCount());
            assertEquals(32_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));

            // A rate the host lowered is news the slider cannot show.
            answer[0] = 20_000;
            game.adaptation.applyBitrateFromMenu(90_000);
            awaitHostAction();
            assertEquals(toasts + 1, ShadowToast.shownToastCount());
        } finally {
            StreamMenuModel.use(null);
        }
    }

    // The legacy bitrate goes out on its own thread; its answer comes back to the UI thread.
    private void awaitHostAction() throws InterruptedException {
        for (int i = 0; i < 500; i++) {
            ShadowLooper.idleMainLooper();
            if (!(boolean) ReflectionHelpers.getField(game, "hostActionInProgress")) return;
            Thread.sleep(10);
        }
        throw new AssertionError("the bitrate request never finished");
    }

    @Test
    public void neverKeepMyBitrateSendsTheStreamsBitrateOnceAfterTheStartAndOnlyOnAPcThatTakesRateChanges() {
        prefs.keepBitrate = true;
        StreamRateChanges changes = recording();
        startStream(changes);
        assertTrue("nothing at the very start", sent.isEmpty());
        runFor(1);
        assertEquals(1, sent.size());
        assertEquals(50_000, sent.get(0)[1]);
        assertEquals(0, sent.get(0)[2]);
        assertFalse("no controller of this device runs", changes.automaticOn());
        runFor(20);
        assertEquals("once", 1, sent.size());

        // A PC that took no rate changes: nothing is sent and the stream runs as The PC decides.
        sent.clear();
        startStream(null);
        runFor(5);
        assertTrue(sent.isEmpty());

        // Another choice sends nothing.
        prefs.keepBitrate = false;
        startStream(recording());
        runFor(1);
        assertTrue(sent.isEmpty());
    }

    @Test
    public void thePcLoweringTheBitrateIsToldOncePerStreamNotOverTheOverlayAndNeverWithNever() {
        assertTrue(GameAdaptation.tellLinkLowered(false, false, false));
        assertFalse("once per stream", GameAdaptation.tellLinkLowered(true, false, false));
        assertFalse("the overlay has its own line", GameAdaptation.tellLinkLowered(false, true, false));
        assertFalse("not with Never", GameAdaptation.tellLinkLowered(false, false, true));

        prefs.enablePerfOverlay = false;
        com.limelight.binding.video.LinkNoticeStatus status = com.limelight.binding.video.LinkNoticeStatus.parse(1, 50_000, 18_400);
        // A PC that took no rate changes: what happened, and no remedy that would not work.
        game.adaptation.noteLinkLowered(status);
        assertTrue(game.linkLoweredTold);
        String told = ShadowToast.getTextOfLatestToast();
        assertTrue(told, told.contains("18 Mbps") && !told.contains("Keep my bitrate") && !told.contains("Settings"));
        ShadowToast.reset();
        game.adaptation.noteLinkLowered(status);
        assertNull("not again", ShadowToast.getLatestToast());

        // A PC that took them: the stream menu's switch.
        startStream(recording());
        game.linkLoweredTold = false;
        game.adaptation.noteLinkLowered(status);
        told = ShadowToast.getTextOfLatestToast();
        assertTrue(told, told.contains("18 Mbps") && told.contains("stream menu") && told.contains("Keep my bitrate"));
        // Not while the PC keeps this stream's bitrate.
        ShadowToast.reset();
        game.linkLoweredTold = false;
        game.bitrateKept = true;
        game.adaptation.noteLinkLowered(status);
        assertNull(ShadowToast.getLatestToast());
    }

    // The bitrate dialog's Keep my bitrate.
    private static com.google.android.material.materialswitch.MaterialSwitch keepSwitch(AlertDialog dialog) {
        List<View> found = new ArrayList<>();
        collectSwitches(dialog.getWindow().getDecorView(), found);
        assertEquals(1, found.size());
        return (com.google.android.material.materialswitch.MaterialSwitch) found.get(0);
    }

    private static void collectSwitches(View view, List<View> found) {
        if (view instanceof com.google.android.material.materialswitch.MaterialSwitch) found.add(view);
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collectSwitches(group.getChildAt(i), found);
        }
    }

    private android.content.SharedPreferences stored() {
        return android.preference.PreferenceManager.getDefaultSharedPreferences(game);
    }

    @Test
    public void keepMyBitrateIsAbsentOnAPcThatTookNoRateChanges() {
        startStream(null);
        ReflectionHelpers.callInstanceMethod(game.adaptation, "showRateChangeDialog");
        assertNull("no bitrate dialog, so no switch", ShadowDialog.getLatestDialog());
        game.adaptation.keepBitrateFromMenu(true);
        assertTrue(sent.isEmpty());
        assertFalse(stored().contains(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING));
        assertFalse(game.adaptation.keepBitrateShown());
    }

    @Test
    public void turningKeepMyBitrateOnSendsOneChangeAtTheStreamsBitrateAndStoresNever() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        com.google.android.material.materialswitch.MaterialSwitch keep = keepSwitch(dialog);
        assertFalse(keep.isChecked());
        keep.setChecked(true);
        ShadowLooper.idleMainLooper();
        assertEquals(1, sent.size());
        assertEquals("the stream's bitrate", 50_000, sent.get(0)[1]);
        assertEquals("only the bitrate", 0, sent.get(0)[2]);
        assertTrue(dialog.isShowing());
        // Stored as choosing Never in Settings: its own key, the pair left as it was.
        assertTrue(stored().getBoolean(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING, false));
        assertFalse(stored().contains(com.limelight.preferences.AutoBitrateChoice.SETTINGS_KEY));
        assertTrue(prefs.keepBitrate);
        assertTrue(PreferenceConfiguration.readPreferences(game).keepBitrate);
        // Its answer shows nothing, and the switch stays on when the dialog opens again; nothing more goes out.
        int toasts = ShadowToast.shownToastCount();
        game.rateChange(new RateChangeResult(sent.get(0)[0], 50_000, 0, STATUS_APPLIED, REASON_NONE, STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertEquals(toasts, ShadowToast.shownToastCount());
        dialog.dismiss();
        assertTrue(keepSwitch(openDialog()).isChecked());
        assertEquals(1, sent.size());
    }

    @Test
    public void keepMyBitrateKeepsTheBitrateThePlayerChoseAndShowsOnOnceTheyChoseOne() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        fields(dialog).get(0).setText("30");
        send(dialog);
        game.rateChange(new RateChangeResult(sent.get(0)[0], 30_000, 60_000, STATUS_APPLIED, REASON_NONE, STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        // The chosen bitrate already makes the PC stand back: on, and nothing to send.
        com.google.android.material.materialswitch.MaterialSwitch keep = keepSwitch(openDialog());
        assertTrue(keep.isChecked());
        keep.setChecked(false);
        keep.setChecked(true);
        ShadowLooper.idleMainLooper();
        assertEquals(2, sent.size());
        assertEquals("the player's bitrate, not the stream's", 30_000, sent.get(1)[1]);
    }

    @Test
    public void aStreamStartedWithNeverShowsTheSwitchOnAndSendsNothingMore() {
        prefs.keepBitrate = true;
        startStream(recording());
        assertTrue("on before the takeover", keepSwitch(openDialog()).isChecked());
        runFor(1);
        assertEquals(1, sent.size());
        assertTrue(keepSwitch(openDialog()).isChecked());
        runFor(5);
        assertEquals("nothing more", 1, sent.size());
    }

    @Test
    public void turnedOffBeforeTheStartsTakeoverWentOutNothingIsSent() {
        prefs.keepBitrate = true;
        startStream(recording());
        // Within the first second, before the stream start's delayed takeover runs.
        keepSwitch(openDialog()).setChecked(false);
        ShadowLooper.idleMainLooper();
        runFor(5);
        assertEquals("the player turned it off first", 0, sent.size());
        assertFalse(game.adaptation.keepBitrateShown());
        assertFalse(prefs.keepBitrate);
    }

    @Test
    public void turningKeepMyBitrateOffStoresThePcDecidesAndSendsNothing() {
        stored().edit().putBoolean(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING, true)
                .putBoolean(com.limelight.preferences.AutoBitrateChoice.SETTINGS_KEY, true).commit();
        prefs.keepBitrate = true;
        startStream(recording());
        runFor(1);
        assertEquals(1, sent.size());
        com.google.android.material.materialswitch.MaterialSwitch keep = keepSwitch(openDialog());
        keep.setChecked(false);
        ShadowLooper.idleMainLooper();
        assertEquals("nothing goes out", 1, sent.size());
        assertFalse(stored().getBoolean(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING, true));
        assertFalse(stored().getBoolean(com.limelight.preferences.AutoBitrateChoice.SETTINGS_KEY, true));
        assertFalse(stored().getBoolean("checkbox_rate_change_bitrate", true));
        assertFalse(prefs.keepBitrate);
        assertEquals(game.getString(R.string.stream_keep_bitrate_off), ShadowToast.getTextOfLatestToast());
        runFor(5);
        assertEquals(1, sent.size());
    }

    @Test
    public void neverRunsNoBitrateControllerOfNightjarsEvenWhereAPcsStoredChoiceIsBusy() {
        com.limelight.preferences.AutoBitrateChoice.setForPc(game, com.limelight.preferences.AutoBitrateChoice.CODEC_STORE,
                "rate-change-pc", true);
        prefs.keepBitrate = true;
        startStream(recording());
        assertFalse(game.adaptation.isAdaptiveBitrateEnabled());
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        assertTrue(keepSwitch(openDialog()).isChecked());
        // The control case: without Never the PC's stored choice starts the busy-network controller.
        prefs.keepBitrate = false;
        startStream(recording());
        assertTrue(game.adaptation.isAdaptiveBitrateEnabled());
        // Automatic quality then steps only the resolution and frame rate.
        assertFalse(GameAdaptation.qualityTakesBitrate(true, true, false, false, 0));
        assertTrue(GameAdaptation.qualityTakesBitrate(false, true, false, false, 0));
    }

    @Test
    public void aTakeoverThePcRefusedOrNeverAnsweredIsNotKept() {
        prefs.keepBitrate = true;
        prefs.enablePerfOverlay = false;
        startStream(recording());
        runFor(1);
        assertTrue(game.adaptation.keepBitrateShown());
        game.rateChange(new RateChangeResult(sent.get(0)[0], 50_000, 0, STATUS_REFUSED, REASON_DISABLED, STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertFalse("refused: the switch shows off", game.adaptation.keepBitrateShown());
        game.adaptation.noteLinkLowered(com.limelight.binding.video.LinkNoticeStatus.parse(1, 50_000, 18_400));
        assertNotNull("and the notice may say the PC lowered it", ShadowToast.getLatestToast());

        // No answer in the usual time.
        sent.clear();
        startStream(recording());
        runFor(1);
        assertTrue(game.adaptation.keepBitrateShown());
        runFor((int) (StreamRateChanges.ANSWER_MS / 1000) + 1);
        assertFalse(game.adaptation.keepBitrateShown());
    }

    @Test
    public void aReconnectSendsTheTakeoverAgainOnTheNewSession() {
        // The switch turned on, then the stream reconnects: exactly one takeover on the new session.
        startStream(recording());
        keepSwitch(openDialog()).setChecked(true);
        ShadowLooper.idleMainLooper();
        assertEquals(1, sent.size());
        ShadowDialog.reset();
        sent.clear();
        startStream(recording());
        runFor(5);
        assertEquals(1, sent.size());
        assertEquals(50_000, sent.get(0)[1]);

        // A bitrate the player chose before the reconnect: sent again at that bitrate.
        prefs.keepBitrate = false;
        ReflectionHelpers.setField(game, "sessionManualBitrate", 30_000);
        ReflectionHelpers.setField(game, "currentBitrate", 30_000);
        sent.clear();
        startStream(recording());
        runFor(5);
        assertEquals(1, sent.size());
        assertEquals(30_000, sent.get(0)[1]);
    }

    @Test
    public void theSwitchSendsTheBitrateTheStreamStartedWithNotTheSetting() {
        // A link test before the launch started the stream below the setting.
        ReflectionHelpers.setField(game, "currentBitrate", 40_000);
        startStream(recording());
        keepSwitch(openDialog()).setChecked(true);
        ShadowLooper.idleMainLooper();
        assertEquals(40_000, sent.get(0)[1]);
    }

    @Test
    public void theSwitchStoresOnlyLowerBitratesKeysAsTheOverlaysSwitchDoes() {
        com.limelight.preferences.AutoBitrateChoice.setForPc(game, com.limelight.preferences.AutoBitrateChoice.CODEC_STORE,
                "another-pc", true);
        startStream(recording());
        keepSwitch(openDialog()).setChecked(true);
        ShadowLooper.idleMainLooper();
        assertTrue("another PC's own choice stays", game.getSharedPreferences(
                com.limelight.preferences.AutoBitrateChoice.CODEC_STORE, android.content.Context.MODE_PRIVATE).contains("another-pc"));
        assertTrue(stored().getBoolean(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING, false));
    }

    @Test
    public void onTheSecondScreenTheSwitchHoldsForItsStreamWithoutStoring() {
        game.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "rate-change-pc").putExtra(DualScreen.EXTRA_SECOND_SCREEN, true));
        startStream(recording());
        keepSwitch(openDialog()).setChecked(true);
        ShadowLooper.idleMainLooper();
        assertEquals(1, sent.size());
        assertTrue(prefs.keepBitrate);
        assertFalse(stored().contains(PreferenceConfiguration.KEEP_BITRATE_PREF_STRING));
    }

    @Test
    public void theNoticeStaysQuietWhileTheOverlayShows() {
        ReflectionHelpers.setField(game, "performanceOverlayScroll", game.findViewById(R.id.performanceOverlayScroll));
        game.performanceOverlayScroll.setVisibility(View.VISIBLE);
        game.adaptation.noteLinkLowered(com.limelight.binding.video.LinkNoticeStatus.parse(1, 50_000, 18_400));
        assertNull(ShadowToast.getLatestToast());
        assertFalse(game.linkLoweredTold);
        // The wiring from the PC's report: Game.linkNotice reaches it.
        game.performanceOverlayScroll.setVisibility(View.GONE);
        game.linkNotice(1, 50_000, 18_400);
        ShadowLooper.idleMainLooper();
        assertTrue(game.linkLoweredTold);
        assertNotNull(ShadowToast.getLatestToast());
    }

    @Test
    public void clampedValuesSayWhatLimitsThem() {
        RateChangeResult result = new RateChangeResult(1, 80_000, 90_000, STATUS_CLAMPED, REASON_ENCODER_LIMIT,
                STATUS_CLAMPED, REASON_HOST_LIMIT);
        assertEquals("Bitrate: 80.0 Mbps, lower than asked. That is the most the PC’s encoder manages at this resolution.\n" +
                        "Frame rate: 90 fps, lower than asked. That is the most the PC or this device allows.",
                Game.rateChangeText(game, result));
        assertEquals("Frame rate: 59.94 fps", Game.rateChangeText(game, new RateChangeResult(1, 30_000, 59_940,
                STATUS_UNCHANGED, REASON_NONE, STATUS_APPLIED, REASON_NONE)));
    }

    @Test
    public void withTheSettingOnAutomaticBitrateRunsOnThisDeviceAndSendsItsRequests() {
        prefs.rateChangeBitrate = true;
        startStream(recording());
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        assertNull("not the host's automatic bitrate as well", ReflectionHelpers.getField(game, "adaptiveBitrate"));
        runUntilAutomaticAsks();
        assertFalse("clean samples ask for more", sent.isEmpty());
        int[] request = sent.get(0);
        assertTrue(request[1] > 50_000);
        assertEquals("only the bitrate", 0, request[2]);

        // The host refuses it: automatic bitrate stops for this stream and says why.
        game.rateChange(new RateChangeResult(request[0], 50_000, 0, STATUS_REFUSED, REASON_CODEC,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        assertEquals(game.getString(R.string.stream_rate_change_auto_stopped,
                game.getString(R.string.stream_rate_change_reason_codec)), ShadowToast.getTextOfLatestToast());
        int before = sent.size();
        runFor(10);
        assertEquals(before, sent.size());
    }

    @Test
    public void withTheSettingOffNothingIsSentOnItsOwn() {
        startStream(recording());
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        runFor(15);
        assertTrue(sent.isEmpty());
    }

    @Test
    public void turningItOnFromTheMenuExplainsItAsTheSettingDoes() {
        prefs.rateChangeBitrate = true;
        startStream(recording());
        ReflectionHelpers.callInstanceMethod(game.adaptation, "toggleAdaptiveBitrateFromMenu");
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        ReflectionHelpers.callInstanceMethod(game.adaptation, "toggleAdaptiveBitrateFromMenu");
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        assertEquals(game.getString(R.string.summary_rate_change_bitrate), ShadowToast.getTextOfLatestToast());
        runUntilAutomaticAsks();
        assertFalse(sent.isEmpty());
    }

    // A Rubylight 6.0.1 PC counts a stream launched at the setting's 50 Mbps as its encoder's 37488 kbps.
    private static final int OLDER_PC_KBPS = 37_488;

    @Test
    public void onAnOlderPcKeepMyBitrateSendsWhatThePcAnsweredAndTheSliderShowsIt() {
        prefs.keepBitrate = true;
        startStreamAsking(recording());
        assertEquals("the query goes out at once", 1, queries.size());
        runFor(3);
        assertTrue("nothing sets a bitrate before the answer", sent.isEmpty());
        int toasts = ShadowToast.shownToastCount();
        answerQuery(OLDER_PC_KBPS);
        assertEquals(1, sent.size());
        assertEquals("not the setting's 50000, which would raise the encoder", OLDER_PC_KBPS, sent.get(0)[1]);
        assertEquals(0, sent.get(0)[2]);
        assertEquals(java.util.Arrays.asList("query", "change"), order);
        assertEquals("the answer says nothing", toasts, ShadowToast.shownToastCount());
        assertEquals(OLDER_PC_KBPS, (int) ReflectionHelpers.getField(game, "currentBitrate"));
        assertEquals("37.488", fields(openDialog()).get(0).getText().toString());
        // Its answer is the takeover's, and the switch stays on.
        game.rateChange(new RateChangeResult(sent.get(0)[0], OLDER_PC_KBPS, 60_000, STATUS_APPLIED, REASON_NONE,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertTrue(game.adaptation.keepBitrateShown());
        runFor(10);
        assertEquals("once", 1, sent.size());
    }

    @Test
    public void onAnOlderPcAutomaticBitrateStartsFromWhatThePcAnswered() {
        prefs.rateChangeBitrate = true;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReflectionHelpers.setField(game, "decoderRenderer", new MediaCodecDecoderRenderer(game, prefs, null, 0,
                false, false, "", null) {
            @Override public boolean hasRecentVideoFrames(long nowMs) { return true; }
            @Override public float getNetworkFrameLossPercent() { return 10; }
        });
        startStreamAsking(recording());
        assertNull("not from the setting's bitrate before the answer", game.rateChanges.automatic());
        assertTrue("it owns the bitrate meanwhile, so automatic quality starts no other controller",
                (boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        assertFalse(GameAdaptation.qualityTakesBitrate(false, true, false,
                (boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"), 0));
        runFor(3);
        assertTrue("nothing before the answer", sent.isEmpty());
        answerQuery(OLDER_PC_KBPS);
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        assertEquals(OLDER_PC_KBPS, game.rateChanges.automatic().currentKbps());
        for (int i = 0; i < 10 && sent.isEmpty(); i++) runFor(1);
        assertFalse("loss cuts the bitrate", sent.isEmpty());
        assertTrue("the first cut is below the stream's " + OLDER_PC_KBPS + ": " + sent.get(0)[1], sent.get(0)[1] < OLDER_PC_KBPS);
    }

    @Test
    public void onANewerPcTheAnswerIsTheStreamsOwnBitrateAndEverythingIsAsBefore() {
        prefs.keepBitrate = true;
        startStreamAsking(recording());
        answerQuery(50_000);
        assertTrue("a second into the stream, as before", sent.isEmpty());
        runFor(1);
        assertEquals(1, sent.size());
        assertEquals(50_000, sent.get(0)[1]);
        assertEquals(50_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
    }

    @Test
    public void withNoAnswerTheStreamGoesOnAsBeforeOnceAndALateAnswerChangesNothing() {
        prefs.keepBitrate = true;
        startStreamAsking(recording());
        runFor((int) (StreamRateChanges.ANSWER_MS / 1000) - 1);
        assertTrue("waits for the answer", sent.isEmpty());
        int toasts = ShadowToast.shownToastCount();
        runFor(1);
        assertEquals(1, sent.size());
        assertEquals("the stream's own, as before the query", 50_000, sent.get(0)[1]);
        assertEquals("quietly", toasts, ShadowToast.shownToastCount());
        answerQuery(OLDER_PC_KBPS);
        runFor(5);
        assertEquals("once", 1, sent.size());
        assertEquals(50_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
        assertEquals(toasts, ShadowToast.shownToastCount());

        // Automatic bitrate starts then too, from the stream's own.
        prefs.keepBitrate = false;
        prefs.rateChangeBitrate = true;
        startStreamAsking(recording());
        runFor((int) (StreamRateChanges.ANSWER_MS / 1000));
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        assertEquals(50_000, game.rateChanges.automatic().currentKbps());
    }

    @Test
    public void aQueryThatCannotBeSentYetGoesOnceMoreASecondLater() {
        prefs.keepBitrate = true;
        unsentQueries = 1;
        startStreamAsking(recording());
        assertTrue(queries.isEmpty());
        runFor(1);
        assertEquals("again a second later", 1, queries.size());
        assertTrue(sent.isEmpty());
        // Video ran meanwhile, so a cut by the PC may be in the answer: not kept, the setting's number goes out.
        answerQuery(OLDER_PC_KBPS);
        assertEquals(50_000, sent.get(0)[1]);
        assertEquals(50_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));

        // Twice unsent: the stream goes on from its own bitrate, as before the query.
        sent.clear();
        queries.clear();
        ReflectionHelpers.setField(game, "currentBitrate", 50_000);
        unsentQueries = 2;
        startStreamAsking(recording());
        runFor(1);
        assertTrue(queries.isEmpty());
        assertEquals(1, sent.size());
        assertEquals(50_000, sent.get(0)[1]);
    }

    @Test
    public void aReconnectAfterThePlayerChoseABitrateSendsThatBitrateWhateverThePcAnswers() {
        // The player's 30 Mbps came from this PC's own answers, so it is in the PC's unit already.
        ReflectionHelpers.setField(game, "sessionManualBitrate", 30_000);
        ReflectionHelpers.setField(game, "currentBitrate", 30_000);
        startStreamAsking(recording());
        answerQuery(22_488);
        runFor(1);
        assertEquals(1, sent.size());
        assertEquals(30_000, sent.get(0)[1]);
    }

    @Test
    public void aCutThePcMadeBeforeTheAnswerIsNotKept() {
        prefs.keepBitrate = true;
        startStreamAsking(recording());
        // The PC's link-rate controller lowered the bitrate before it handled the query: its answer is the cut.
        game.linkNotice(1, 50_000, 30_000);
        ShadowLooper.idleMainLooper();
        answerQuery(30_000);
        runFor(1);
        assertEquals(1, sent.size());
        assertEquals("the setting's number, as before the query", 50_000, sent.get(0)[1]);
        assertEquals(50_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));

        // Automatic bitrate starts from the setting's number too.
        prefs.keepBitrate = false;
        prefs.rateChangeBitrate = true;
        startStreamAsking(recording());
        game.linkNotice(2, 50_000, 5_000);
        ShadowLooper.idleMainLooper();
        answerQuery(5_000);
        assertEquals(50_000, game.rateChanges.automatic().currentKbps());

        // A notice from an earlier stream does not count against this one.
        prefs.rateChangeBitrate = false;
        prefs.keepBitrate = true;
        sent.clear();
        startStreamAsking(recording());
        answerQuery(OLDER_PC_KBPS);
        runFor(1);
        assertEquals(OLDER_PC_KBPS, sent.get(0)[1]);
    }

    @Test
    public void aBitratePickedBeforeTheAnswerGoesOutOnceAfterIt() {
        startStreamAsking(recording());
        assertEquals("the slider keeps the pick", 30_000, game.adaptation.applyBitrateFromMenu(30_000));
        assertTrue("held until the answer", sent.isEmpty());
        answerQuery(OLDER_PC_KBPS);
        assertEquals(1, sent.size());
        assertEquals(30_000, sent.get(0)[1]);
        runFor(10);
        assertEquals("once", 1, sent.size());

        // Without an answer it goes out after the timeout.
        sent.clear();
        startStreamAsking(recording());
        game.adaptation.applyBitrateFromMenu(25_000);
        runFor((int) (StreamRateChanges.ANSWER_MS / 1000));
        assertEquals(1, sent.size());
        assertEquals(25_000, sent.get(0)[1]);
    }

    @Test
    public void anOpenMenusSliderShowsWhatThePcAnswered() {
        StreamMenuModel.use(new RustStreamMenuModel());
        try {
            startStreamAsking(recording());
            game.showStreamMenu();
            Object sheet = ReflectionHelpers.getField(game.menuActions, "streamMenuSheet");
            com.limelight.ui.StepSlider slider = ReflectionHelpers.callInstanceMethod(sheet, "bitrateSlider");
            assertEquals(com.limelight.ui.BitrateSlider.progress(50_000), slider.getPosition());
            answerQuery(OLDER_PC_KBPS);
            assertEquals(com.limelight.ui.BitrateSlider.progress(OLDER_PC_KBPS), slider.getPosition());
        } finally {
            StreamMenuModel.use(null);
        }
    }

    @Test
    public void automaticBitrateTurnedOffBeforeTheAnswerStaysOff() {
        prefs.rateChangeBitrate = true;
        startStreamAsking(recording());
        ReflectionHelpers.callInstanceMethod(game.adaptation, "toggleAdaptiveBitrateFromMenu");
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        answerQuery(OLDER_PC_KBPS);
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(game.adaptation, "isRateChangeBitrateOn"));
        runFor(15);
        assertTrue(sent.isEmpty());
    }

    @Test
    public void keepMyBitrateTurnedOnBeforeTheAnswerSendsWhatThePcAnswered() {
        startStreamAsking(recording());
        keepSwitch(openDialog()).setChecked(true);
        ShadowLooper.idleMainLooper();
        assertTrue("nothing before the answer", sent.isEmpty());
        assertTrue(game.adaptation.keepBitrateShown());
        answerQuery(OLDER_PC_KBPS);
        runFor(1);
        assertEquals(1, sent.size());
        assertEquals(OLDER_PC_KBPS, sent.get(0)[1]);
    }

    @Test
    public void theDoctorOffersNoBitrateTrialWhileAutomaticBitrateOnThisDeviceRuns() {
        assertTrue(Game.healthTrialAllowed(false, false, false));
        assertFalse("automatic bitrate on this device owns the bitrate", Game.healthTrialAllowed(false, true, false));
        assertFalse(Game.healthTrialAllowed(true, false, false));
        assertFalse(Game.healthTrialAllowed(false, false, true));
    }
}
