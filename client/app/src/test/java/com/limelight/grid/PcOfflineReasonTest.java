package com.limelight.grid;

import android.app.Application;
import android.content.Context;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertEquals;

/**
 * An away PC says why: another certificate at its address (another PC or program answers in its place) tells the
 * user what to do, as desktop does, ahead of the internet paths' words; otherwise the caller's plain text.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class PcOfflineReasonTest {
    @Test
    public void anotherCertificateIsTheReasonAnAwayPcShows() {
        Context context = RuntimeEnvironment.getApplication();
        ComputerDetails details = new ComputerDetails();
        details.state = ComputerDetails.State.OFFLINE;
        assertEquals("Offline", PcGridAdapter.offlineReason(context, details, "Offline"));

        details.unreachable = "No path reached the PC.";
        assertEquals("No path reached the PC.", PcGridAdapter.offlineReason(context, details, "Offline"));

        details.anotherCertificate = true;
        String another = context.getString(R.string.pc_another_answers);
        assertEquals(another, PcGridAdapter.offlineReason(context, details, "Offline"));
        assertEquals("Another PC or program answers at this PC’s address. If this PC was set up again, remove it and add it again.", another);

        // A poll that reaches the PC again clears it, and a copy carries it.
        ComputerDetails copy = new ComputerDetails(details);
        assertEquals(another, PcGridAdapter.offlineReason(context, copy, "Offline"));
        ComputerDetails answered = new ComputerDetails();
        copy.update(answered);
        assertEquals("Offline", PcGridAdapter.offlineReason(context, copy, "Offline"));
    }
}
