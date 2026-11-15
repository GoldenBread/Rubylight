package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.widget.TextView;
import android.view.inputmethod.EditorInfo;
import com.google.android.material.textfield.TextInputLayout;
import com.limelight.R;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import java.util.concurrent.LinkedBlockingQueue;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AddComputerInputTest {
    public static class Screen extends AddComputerManually {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) { return true; }
        @Override public void unbindService(ServiceConnection connection) { }
    }
    public static class UnavailableScreen extends Screen {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) { return false; }
    }

    @Test public void unavailableDiscoveryDoesNotLeaveSubmitDisabled() {
        try (var controller = Robolectric.buildActivity(UnavailableScreen.class).setup()) {
            UnavailableScreen activity = controller.get();
            ((TextView) activity.findViewById(R.id.hostTextView)).setText("pc.local");
            activity.findViewById(R.id.addPcButton).performClick();
            TextInputLayout input = activity.findViewById(R.id.host_input);
            assertEquals(activity.getString(R.string.add_pc_service_unavailable), input.getError());
            assertTrue(activity.findViewById(R.id.addPcButton).isEnabled());
        }
    }

    @Test public void invalidInputStaysInlineAndEditingClearsTheError() {
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen activity = controller.get();
            TextView field = activity.findViewById(R.id.hostTextView);
            TextInputLayout input = activity.findViewById(R.id.host_input);
            activity.findViewById(R.id.addPcButton).performClick();
            assertEquals(activity.getString(R.string.addpc_enter_ip), input.getError());
            field.setText("pc.local:65536");
            assertNull(input.getError());
            field.onEditorAction(EditorInfo.IME_ACTION_DONE);
            assertEquals(activity.getString(R.string.add_pc_address_error), input.getError());
            LinkedBlockingQueue<?> queue = ReflectionHelpers.getField(activity, "computersToAdd");
            assertTrue(queue.isEmpty());
        }
    }

    @Test public void repeatedImeAndButtonSubmissionsQueueOnlyOneRequest() {
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen activity = controller.get();
            TextView field = activity.findViewById(R.id.hostTextView);
            field.setText("  pc.local:47989  ");
            field.onEditorAction(EditorInfo.IME_ACTION_DONE);
            field.onEditorAction(EditorInfo.IME_ACTION_DONE);
            activity.findViewById(R.id.addPcButton).performClick();
            LinkedBlockingQueue<?> queue = ReflectionHelpers.getField(activity, "computersToAdd");
            assertEquals(1, queue.size());
            assertEquals("pc.local:47989", queue.peek());
            assertFalse(activity.findViewById(R.id.addPcButton).isEnabled());
        }
    }

    private static final String LINK = "rubylight://pair?v=1&host=192.168.4.2&port=47989&otp=4821&pass=hunter2&fp="
            + "00".repeat(32) + "&name=Wohnzimmer";

    private static Intent fromLink() {
        return new Intent(RuntimeEnvironment.getApplication(), Screen.class).putExtra(AddComputerManually.EXTRA_FROM_LINK, true);
    }

    private static LinkedBlockingQueue<?> queue(Screen activity) {
        return ReflectionHelpers.getField(activity, "computersToAdd");
    }

    @Test public void aConfirmedLinkGoesThroughTheExistingFlowOnce() {
        LinkGate.reset();
        LinkGate.confirm(LINK);
        try (var controller = Robolectric.buildActivity(Screen.class, fromLink()).setup()) {
            assertEquals(LINK, queue(controller.get()).peek());
            assertTrue("a link from outside waits for this request", LinkGate.held());
        }
        assertFalse("the screen leaving lets go", LinkGate.held());
        // Back from Recents or recreated: the mark is there, the link is not.
        try (var controller = Robolectric.buildActivity(Screen.class, fromLink()).setup()) {
            assertTrue(queue(controller.get()).isEmpty());
        }
    }

    @Test public void aSecondLinkReachesTheScreenAnEarlierOneLeftOpen() {
        LinkGate.reset();
        try (var controller = Robolectric.buildActivity(Screen.class, fromLink()).setup()) {
            assertTrue(queue(controller.get()).isEmpty());
            // Android brings the old task back for a new link: no onCreate, and no new intent.
            controller.pause().stop();
            LinkGate.confirm(LINK);
            controller.restart().resume();
            assertEquals(LINK, queue(controller.get()).peek());
            assertTrue("the link went on its way, not left to block later ones", LinkGate.held());
        }
        assertFalse(LinkGate.held());
    }

    @Test public void aScreenOpenedForALinkLeavesToThePcListWhateverBroughtItBack() {
        LinkGate.reset();
        try (var controller = Robolectric.buildActivity(Screen.class, fromLink()).setup()) {
            Screen activity = controller.get();
            org.robolectric.Shadows.shadowOf(activity).setIsTaskRoot(true);
            activity.finish();
            Intent next = org.robolectric.Shadows.shadowOf(activity).getNextStartedActivity();
            assertEquals(com.limelight.PcView.class.getName(), next.getComponent().getClassName());
        }
    }

    @Test public void aNormalStartLeavesAConfirmedLinkAlone() {
        LinkGate.reset();
        LinkGate.confirm(LINK);
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            assertTrue(queue(controller.get()).isEmpty());
        }
        LinkGate.reset();
    }

    @Test public void aLinkWaitsBehindATypedAddressAndIsNotLostWhenItSucceeds() {
        LinkGate.reset();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen activity = controller.get();
            ((TextView) activity.findViewById(R.id.hostTextView)).setText("pc.local");
            activity.findViewById(R.id.addPcButton).performClick();
            assertTrue(LinkGate.held());
            LinkGate.put(LINK);
            // Added: the screen closes, as it does after a typed address.
            activity.finish();
            controller.pause().stop().destroy();
            assertFalse(LinkGate.held());
            Intent asks = org.robolectric.Shadows.shadowOf(RuntimeEnvironment.getApplication()).getNextStartedActivity();
            assertEquals(PairLinkActivity.class.getName(), asks.getComponent().getClassName());
            assertEquals(LINK, LinkGate.take());
        }
    }

    /**
     * Back on "Connecting to the PC" stops the attempt and keeps the screen with its address. A late answer from the
     * attempt, even one that swallowed the interrupt, neither closes the screen nor reports the PC added.
     */
    @Test public void backWhileConnectingKeepsTheAddressAndALateAnswerIsDropped() throws Exception {
        LinkGate.reset();
        java.util.concurrent.CountDownLatch asked = new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.CountDownLatch answer = new java.util.concurrent.CountDownLatch(1);
        com.limelight.computers.ComputerManagerService manager = new com.limelight.computers.ComputerManagerService();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen activity = controller.get();
            ReflectionHelpers.setField(activity, "managerBinder", manager.new ComputerManagerBinder() {
                @Override public boolean addComputerBlocking(com.limelight.nvstream.http.ComputerDetails details) {
                    asked.countDown();
                    // An answer that comes anyway: the interrupt is swallowed.
                    while (true) {
                        try {
                            answer.await();
                            break;
                        } catch (InterruptedException ignored) {
                        }
                    }
                    details.state = com.limelight.nvstream.http.ComputerDetails.State.ONLINE;
                    return true;
                }
            });
            ReflectionHelpers.callInstanceMethod(activity, "startAddThread");
            Thread attempt = ReflectionHelpers.getField(activity, "addThread");
            TextView field = activity.findViewById(R.id.hostTextView);
            field.setText("pc.local");
            activity.findViewById(R.id.addPcButton).performClick();
            assertTrue(asked.await(5, java.util.concurrent.TimeUnit.SECONDS));
            org.robolectric.shadows.ShadowLooper.idleMainLooper();

            android.app.Dialog spinner = org.robolectric.shadows.ShadowDialog.getLatestDialog();
            assertTrue(spinner.isShowing());
            spinner.onBackPressed();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertFalse(spinner.isShowing());
            assertFalse("Add PC stays open", activity.isFinishing());
            assertEquals("pc.local", field.getText().toString());
            assertTrue(field.isEnabled());
            assertTrue(activity.findViewById(R.id.addPcButton).isEnabled());
            assertNotSame("a new thread takes the next address", attempt, ReflectionHelpers.getField(activity, "addThread"));

            answer.countDown();
            attempt.join(5000);
            assertFalse(attempt.isAlive());
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertFalse("the late answer does not close the screen", activity.isFinishing());
            assertNull("nor says the PC was added", org.robolectric.shadows.ShadowToast.getLatestToast());
            assertTrue(field.isEnabled());
        }
        LinkGate.reset();
    }

    @Test public void aFailedRequestLetsTheLinkThrough() {
        LinkGate.reset();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen activity = controller.get();
            ((TextView) activity.findViewById(R.id.hostTextView)).setText("pc.local");
            activity.findViewById(R.id.addPcButton).performClick();
            LinkGate.put(LINK);
            ReflectionHelpers.callInstanceMethod(activity, "setSubmitting",
                    ReflectionHelpers.ClassParameter.from(boolean.class, false));
            Intent asks = org.robolectric.Shadows.shadowOf(RuntimeEnvironment.getApplication()).getNextStartedActivity();
            assertEquals(PairLinkActivity.class.getName(), asks.getComponent().getClassName());
        }
        LinkGate.reset();
    }
}
