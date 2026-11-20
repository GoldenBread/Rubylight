package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

/** The mode a stream switches the screen to, which the Settings screen's fractional V-Sync row names. */
public class DisplayModeChoiceTest {
    private static DisplayModeChoice.Mode mode(int width, int height, float hz) {
        return new DisplayModeChoice.Mode(width, height, hz);
    }

    private static DisplayModeChoice.Mode[] at(int width, int height, float... rates) {
        DisplayModeChoice.Mode[] modes = new DisplayModeChoice.Mode[rates.length];
        for (int i = 0; i < rates.length; i++) modes[i] = mode(width, height, rates[i]);
        return modes;
    }

    /** The refresh of the chosen mode, for a 1080p screen streaming 1080p (not native) at {@code fps}. */
    private static float refresh(float nowHz, DisplayModeChoice.Mode[] modes, int fps, boolean reduce) {
        DisplayModeChoice.Mode now = mode(1920, 1080, nowHz);
        int chosen = DisplayModeChoice.choose(now, modes, 1920, 1080, fps, false, false, reduce);
        return chosen < 0 ? now.refreshHz : modes[chosen].refreshHz;
    }

    @Test
    public void theHighestWholeMultipleWinsEvenBelowTheRefreshNow() {
        assertEquals("144 Hz is no multiple of 60", 120f, refresh(144f, at(1920, 1080, 144f, 120f, 60f), 60, false), 0f);
        assertEquals("60 fps on a 90 Hz screen", 60f, refresh(90f, at(1920, 1080, 90f, 60f), 60, false), 0f);
        assertEquals("raised to the highest multiple", 120f, refresh(60f, at(1920, 1080, 60f, 120f), 60, false), 0f);
        assertEquals("no multiple: the refresh is never lowered", 144f, refresh(144f, at(1920, 1080, 144f, 100f), 60, false), 0f);
        assertEquals("within 3 Hz above a multiple", 119.88f, refresh(144f, at(1920, 1080, 144f, 119.88f), 60, false), 0f);
    }

    @Test
    public void reduceRefreshRateTakesTheLowestMultiple() {
        assertEquals(60f, refresh(120f, at(1920, 1080, 120f, 60f), 60, true), 0f);
        assertEquals("the frame rate's own mode stays once found", 60f, refresh(60f, at(1920, 1080, 60f, 120f), 60, true), 0f);
        assertEquals("60 Hz listed first", 60f, refresh(120f, at(1920, 1080, 60f, 120f), 60, true), 0f);
    }

    /** Lower refresh rate off, whatever the frame pacing (GameVideoOutput.mayReduceRefreshRate). */
    @Test
    public void withoutLowerRefreshRateAWholeMultipleKeepsTheTopRate() {
        assertEquals("60 fps on a 60/120 Hz screen at 120", 120f, refresh(120f, at(1920, 1080, 60f, 120f), 60, false), 0f);
        assertEquals("30 fps on it", 120f, refresh(120f, at(1920, 1080, 60f, 120f), 30, false), 0f);
        assertEquals("the top rate would judder: 60 fps on a 60/90 Hz screen", 60f,
                refresh(90f, at(1920, 1080, 60f, 90f), 60, false), 0f);
    }

    @Test
    public void theResolutionStaysForAStreamUpTo60Fps() {
        DisplayModeChoice.Mode now = mode(2560, 1440, 144f);
        DisplayModeChoice.Mode[] modes = {now, mode(1920, 1080, 120f)};
        assertEquals("1080p's 120 Hz would change the size", 0,
                DisplayModeChoice.choose(now, modes, 1920, 1080, 60, false, false, false));
        assertEquals("above 60 fps the size may drop to one that holds the stream", 1,
                DisplayModeChoice.choose(now, modes, 1920, 1080, 120, false, false, false));
    }

    @Test
    public void vrrTakesTheTopRateThatKeepsTheStream() {
        DisplayModeChoice.Mode now = mode(1920, 1080, 60f);
        DisplayModeChoice.Mode[] modes = at(1920, 1080, 60f, 144f, 120f);
        assertEquals(1, DisplayModeChoice.choose(now, modes, 1920, 1080, 60, true, false, false));
    }
}
