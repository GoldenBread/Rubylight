package com.limelight.utils;

import android.app.Application;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nvstream.http.HostHttpResponseException;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.xmlpull.v1.XmlPullParserException;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.net.UnknownHostException;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;

/** A failed Quit says what happened and what to try, in the app's own words, never the error's. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class QuitFailureMessageTest {
    private final Application app = RuntimeEnvironment.getApplication();

    private String said(Exception error) {
        return ServerHelper.quitFailure(app, "Celeste", error);
    }

    @Test
    public void everyFailureIsASentenceOfTheAppAndNeverTheErrorsWording() {
        String check = app.getString(R.string.applist_quit_fail_check, "Celeste");
        assertEquals("the PC answered but kept the game", check, said(null));
        assertEquals(check, said(new NativeApiException(NativeApiException.Kind.STATUS, "HTTP 500 <xml>")));
        assertEquals(check, said(new HostHttpResponseException(503, "Service Unavailable")));
        assertEquals(check, said(new IOException("Connection reset by peer")));
        assertEquals(check, said(new XmlPullParserException("unterminated entity ref")));
        assertEquals("an error without a message is no empty toast", check, said(new IOException()));
        assertFalse(check.isEmpty());
    }

    @Test
    public void aSessionAnotherDeviceStartedSaysWhoCanQuitIt() {
        assertEquals(app.getString(R.string.applist_quit_other_device),
                said(new HostHttpResponseException(599, "Not the session owner")));
    }

    @Test
    public void anUnknownAddressAndAMissingServiceKeepTheirOwnSentences() {
        assertEquals(app.getString(R.string.error_unknown_host), said(new UnknownHostException("pc.local")));
        assertEquals(app.getString(R.string.error_404), said(new FileNotFoundException("/cancel")));
    }
}
