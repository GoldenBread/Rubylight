package com.limelight.files;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Intent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nvstream.jni.FileTransferBridge;
import com.limelight.ui.UiNavigationTest;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowToast;

/**
 * Leaving the Files page keeps unfinished transfers going while the app runs, as on desktop: the screens that lead
 * back show how many, the PC's page shows them again, and another PC's page waits for them.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class RunningTransfersTest {
    private static final HostEndpoint HOST = new HostEndpoint("pc", "10.0.0.2", 47989, 47984, "pem", "client",
            new byte[0], new byte[0]);

    private final Application app = RuntimeEnvironment.getApplication();

    /** One upload, waiting until {@link #state} says otherwise. */
    private static final class Engine implements FileTransferEngine {
        String state = "queued";
        int cancelAlls;
        boolean destroyed;

        @Override public long create() { return 7; }
        @Override public void destroy(long handle) { destroyed = true; }
        @Override public long queueUpload(long handle, String localName, long size) { return 1; }
        @Override public long queueDownload(long handle, String name, String localName, long size) { return -1; }
        @Override public boolean cancel(long handle, long id) { return false; }
        @Override public void cancelAll(long handle) { cancelAlls++; }
        @Override public boolean retry(long handle, long id) { return false; }
        @Override public void clearFinished(long handle) {}
        @Override public String snapshot(long handle) {
            return "[{\"id\":1,\"direction\":\"upload\",\"name\":\"save.bin\",\"local_name\":\"save.bin\",\"size\":10,"
                    + "\"state\":\"" + state + "\""
                    + ("failed".equals(state) ? ",\"problem\":\"network\",\"can_retry\":true" : "") + "}]";
        }
        @Override public String startNext(long handle) { return "[]"; }
        @Override public String list(HostEndpoint host) { return "{}"; }
        @Override public String run(long handle, long id, HostEndpoint host, int fd,
                                    FileTransferBridge.Progress progress) { return "{}"; }
    }

    @After
    public void endWhatIsHeld() {
        FileTransferActivity.TransferWork held = RunningTransfers.held();
        if (held != null) held.end();
    }

    private ActivityController<FileTransferActivity> page(String pc) {
        return Robolectric.buildActivity(FileTransferActivity.class, FileTransferActivity.forPc(app, pc)).setup();
    }

    @Test
    public void leavingThePageKeepsUnfinishedTransfersAndAnotherPcWaits() {
        ActivityController<FileTransferActivity> screen = page("tower");
        FileTransferActivity.TransferWork work = screen.get().work;
        Engine engine = new Engine();
        work.engine = engine;
        work.controller = new FileTransferController(engine, HOST, work.workers, work.handler::post, work);
        work.controller.send("save.bin", 10, () -> -1);

        screen.get().finish();
        screen.pause().stop().destroy();
        assertSame("the upload goes on", work, RunningTransfers.held());
        assertEquals(0, engine.cancelAlls);
        assertEquals(1, RunningTransfers.count());
        assertEquals("tower", RunningTransfers.pcUuid());

        // Home shows them, and a press opens the PC's Files page, where each can be cancelled.
        ActivityController<UiNavigationTest.Screen> home = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        Runnable hide = RunningTransfers.showOn(home.get());
        ViewGroup content = home.get().findViewById(android.R.id.content);
        Button button = (Button) content.getChildAt(content.getChildCount() - 1);
        assertEquals(View.VISIBLE, button.getVisibility());
        assertEquals("File transfers running: 1", button.getText().toString());
        button.performClick();
        Intent opened = Shadows.shadowOf(home.get()).getNextStartedActivity();
        assertEquals("tower", opened.getStringExtra(FileTransferActivity.EXTRA_PC_UUID));
        hide.run();
        home.destroy();

        // Another PC's page says why it cannot open yet.
        ActivityController<FileTransferActivity> other = page("laptop");
        assertTrue(other.get().isFinishing());
        assertEquals(app.getString(R.string.files_busy_other_pc), ShadowToast.getTextOfLatestToast());
        other.pause().stop().destroy();
        assertSame(work, RunningTransfers.held());

        // The PC's own page shows them again; once they are done, leaving ends the work.
        ActivityController<FileTransferActivity> again = page("tower");
        assertSame(work, again.get().work);
        assertNull(RunningTransfers.held());
        engine.state = "done";
        work.controller.clearFinished();
        again.get().finish();
        again.pause().stop().destroy();
        assertNull(RunningTransfers.held());
        assertEquals(1, engine.cancelAlls);
        assertTrue(engine.destroyed);
        assertTrue(work.workers.isShutdown());
    }

    @Test
    public void aTransferThatFailsAfterThePageClosedStaysUntilThePageShowsIt() {
        ActivityController<FileTransferActivity> screen = page("tower");
        FileTransferActivity.TransferWork work = screen.get().work;
        Engine engine = new Engine();
        work.engine = engine;
        work.controller = new FileTransferController(engine, HOST, work.workers, work.handler::post, work);
        work.controller.send("save.bin", 10, () -> -1);
        screen.get().finish();
        screen.pause().stop().destroy();
        assertSame(work, RunningTransfers.held());

        // A network blip fails it with no page open: nothing runs any more, but the failure is kept.
        engine.state = "failed";
        work.controller.cancel(1); // any change of the list reports it, as a worker's end does
        Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
        assertSame("the failure is not dropped unseen", work, RunningTransfers.held());
        assertEquals(0, engine.cancelAlls);
        assertEquals(0, RunningTransfers.count());
        assertEquals(1, RunningTransfers.failed());

        // The button says so, and opens the PC's page.
        ActivityController<UiNavigationTest.Screen> home = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        Runnable hide = RunningTransfers.showOn(home.get());
        ViewGroup content = home.get().findViewById(android.R.id.content);
        Button button = (Button) content.getChildAt(content.getChildCount() - 1);
        assertEquals(View.VISIBLE, button.getVisibility());
        assertEquals("File transfers failed: 1", button.getText().toString());
        button.performClick();
        assertEquals("tower", Shadows.shadowOf(home.get()).getNextStartedActivity()
                .getStringExtra(FileTransferActivity.EXTRA_PC_UUID));
        hide.run();
        home.destroy();

        // The page shows the failed row with Try again; that counts as seen, so leaving it drops the row.
        ActivityController<FileTransferActivity> again = page("tower");
        assertSame(work, again.get().work);
        View page = again.get().findViewById(android.R.id.content);
        assertTrue(page.findViewWithTag("retry:1") != null);
        assertEquals(0, work.unseenFailures());
        again.get().finish();
        again.pause().stop().destroy();
        assertNull(RunningTransfers.held());
        assertEquals(1, engine.cancelAlls);
        assertTrue(work.workers.isShutdown());
    }
}
