package com.limelight.ui;

import android.content.Context;
import android.os.SystemClock;
import android.widget.Toast;

import com.limelight.computers.HostNotices;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * What a stream tells the person, one message at a time. Something to act on, or that failed,
 * replaces whatever shows. A lasting "not supported" fact goes to the overlay's status line, and
 * shows as a message once per PC, and only while nothing else shows; otherwise a later stream says it.
 */
public final class StreamMessages {
    // How long Android keeps a toast up.
    static final long LONG_MS = 3500;
    static final long SHORT_MS = 2000;

    private final Context context;
    private final Set<String> notes = new LinkedHashSet<>();
    private final Map<String, String> statuses = new LinkedHashMap<>();
    // The messages handed to Android, in the order it shows them; the last is the newest.
    private final List<Message> shown = new ArrayList<>();

    private static final class Message {
        final Toast toast;
        final CharSequence text;
        // When Android shows it (after the messages before it) and when it is gone.
        final long from, until;
        // A follow(): it must be seen, so a message that replaces it before it shows hands it on.
        final boolean follows;

        Message(Toast toast, CharSequence text, long from, long until, boolean follows) {
            this.toast = toast;
            this.text = text;
            this.from = from;
            this.until = until;
            this.follows = follows;
        }
    }

    public StreamMessages(Context context) {
        this.context = context;
    }

    /** Something to act on, or a failure: shown now in place of what shows, unless it already shows. */
    public void alert(CharSequence text, int duration) {
        if (text == null || text.length() == 0) return;
        if (showing() && text.toString().contentEquals(newest().text)) return;
        show(text, duration);
    }

    public void alert(int text, int duration) {
        alert(context.getString(text), duration);
    }

    /**
     * A fact that stays true for this PC. It joins the status line, and shows once per PC when nothing
     * else shows; while something does, the PC is not marked, so a later stream says it instead.
     */
    public void notice(String pcUuid, HostNotices.Notice notice, CharSequence text) {
        note(text);
        if (!showing() && HostNotices.claim(context, pcUuid, notice)) {
            show(text, Toast.LENGTH_LONG);
        }
    }

    /** A fact the connection already counted once for this PC: it shows only when nothing else does. */
    public void countedNotice(CharSequence text) {
        note(text);
        if (!showing()) show(text, Toast.LENGTH_LONG);
    }

    /**
     * Something the person must see that should not hide what shows, such as why their input does
     * nothing: it joins the status line and shows now, or right after the current message (Android
     * shows an app's messages in turn). A message that replaces it before it shows is followed by it.
     */
    public void follow(CharSequence text) {
        if (text == null || text.length() == 0) return;
        note(text);
        if (!showing()) {
            show(text, Toast.LENGTH_LONG);
            return;
        }
        if (text.toString().contentEquals(newest().text)) return;
        hand(text, Toast.LENGTH_LONG, newest().until, true);
    }

    /**
     * The stream screen is going: messages still waiting for their turn are dropped, so none shows
     * over the screen after it. One showing keeps its time, as a stream's last word may be in it.
     */
    public void dropWaiting() {
        long now = SystemClock.uptimeMillis();
        for (int i = shown.size() - 1; i >= 0; i--) {
            if (shown.get(i).from > now) shown.remove(i).toast.cancel();
        }
    }

    /** A fact for the overlay's status line alone. */
    public void note(CharSequence text) {
        if (text != null && text.length() != 0) notes.add(text.toString());
    }

    /**
     * A fact for the overlay's status line that can stop being true, such as a screen that did not
     * follow the frame rate vote. One line per key; null or empty text removes it. Returns whether
     * the line changed.
     */
    public boolean status(String key, CharSequence text) {
        if (text == null || text.length() == 0) return statuses.remove(key) != null;
        return !text.toString().equals(statuses.put(key, text.toString()));
    }

    public List<String> notes() {
        List<String> all = new ArrayList<>(notes);
        all.addAll(statuses.values());
        return all;
    }

    public boolean showing() {
        return !shown.isEmpty() && SystemClock.uptimeMillis() < newest().until;
    }

    private Message newest() {
        return shown.get(shown.size() - 1);
    }

    /** Shows `text` now in place of every message handed over; a follow() that had not shown yet comes after it. */
    private void show(CharSequence text, int duration) {
        long now = SystemClock.uptimeMillis();
        List<CharSequence> waiting = new ArrayList<>();
        for (Message message : shown) {
            if (message.follows && message.from > now) waiting.add(message.text);
            message.toast.cancel();
        }
        shown.clear();
        hand(text, duration, now, false);
        for (CharSequence follow : waiting) {
            if (!follow.toString().contentEquals(newest().text)) hand(follow, Toast.LENGTH_LONG, newest().until, true);
        }
    }

    /** Hands a message to Android, which shows it at `from`, after those handed before it. */
    private void hand(CharSequence text, int duration, long from, boolean follows) {
        Toast toast = Toast.makeText(context, text, duration);
        shown.add(new Message(toast, text, from, from + (duration == Toast.LENGTH_LONG ? LONG_MS : SHORT_MS), follows));
        toast.show();
    }
}
