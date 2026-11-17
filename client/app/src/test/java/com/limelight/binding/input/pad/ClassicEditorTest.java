package com.limelight.binding.input.pad;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/** What the classic layout editor leaves on screen: never with the switch off, and not after a gamepad hid the controls. */
public class ClassicEditorTest {
    private static final class Fake implements ClassicEditor.Controls {
        boolean switchOn = true, belong = true;
        final List<String> calls = new ArrayList<>();

        @Override public boolean switchOn() { return switchOn; }
        @Override public boolean belongOnScreen() { return belong; }
        @Override public void show() { calls.add("show"); }
        @Override public void hide() { calls.add("hide"); }
    }

    @Test public void withTheSwitchOffTheControlsNeverAppear() {
        Fake controls = new Fake();
        controls.switchOn = false;
        controls.belong = false;
        ClassicEditor.open(controls);
        ClassicEditor.choose(controls, false);
        ClassicEditor.choose(controls, true);
        ClassicEditor.cancel(controls);
        assertFalse("Never shown", controls.calls.contains("show"));
        assertTrue("Left hidden", controls.calls.contains("hide"));
    }

    @Test public void aGamepadHidesTheControlsAgainOnceTheEditEnds() {
        Fake controls = new Fake();
        // The switch is on, but a connected pad keeps the controls off the screen.
        controls.belong = false;
        ClassicEditor.open(controls);
        assertEquals(List.of("show"), controls.calls);
        // Active ends the edit: the controls go away again, instead of staying on top of the game.
        ClassicEditor.choose(controls, true);
        assertEquals(List.of("show", "show", "hide"), controls.calls);
        controls.calls.clear();
        ClassicEditor.cancel(controls);
        assertEquals("Cancel also leaves nothing on screen", List.of("hide"), controls.calls);
    }

    @Test public void controlsThatBelongStayThroughTheEdit() {
        Fake controls = new Fake();
        ClassicEditor.open(controls);
        ClassicEditor.choose(controls, false);
        assertEquals(List.of("show", "show"), controls.calls);
        ClassicEditor.cancel(controls);
        assertEquals("Cancel leaves them as they were", List.of("show", "show"), controls.calls);
        ClassicEditor.choose(controls, true);
        assertEquals(List.of("show", "show", "show"), controls.calls);
    }
}
