package com.limelight;

import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;

import com.limelight.nativeapi.NativeGuests;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamGuestsInviteTest {
    @Test
    public void theSharedMessageSaysWhatToDoAndEndsWithTheLink() {
        Context context = RuntimeEnvironment.getApplication();
        String link = "rubylight://invite#v=1&k=" + "01".repeat(32);
        String message = StreamGuests.inviteMessage(context, link);
        assertTrue(message, message.startsWith("Join my game with Nightjar"));
        assertTrue(message, message.endsWith("\n" + link));
    }

    @Test
    public void anInviteNeverPrintsItsLink() {
        NativeGuests.Invite made = new NativeGuests.Invite("rubylight://invite#v=1&k=secret", 1_800_000_000L);
        assertTrue(made.toString(), !made.toString().contains("secret"));
    }
}
