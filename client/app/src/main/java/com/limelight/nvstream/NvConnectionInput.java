/**
 * Batched and direct input forwarding helpers for {@link NvConnection}, preserving
 * {@link MouseButtonOwnership} arbitration and {@link InputBatcher} dispatch.
 */
package com.limelight.nvstream;

import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.nvstream.jni.MoonBridge;

import java.nio.charset.StandardCharsets;

final class NvConnectionInput {
    private NvConnectionInput() {
    }

    static void sendMouseMove(NvConnection c, short dx, short dy) {
        if (!c.canSendInput()) return;
        if (c.inputPoller != null) c.inputBatcher.mouse(0, dx, dy, (short) 0, (short) 0);
        else MoonBridge.sendMouseMove(dx, dy);
    }

    static void sendMousePosition(NvConnection c, short x, short y, short rw, short rh) {
        if (!c.canSendInput()) return;
        if (c.inputPoller != null) c.inputBatcher.mouse(1, x, y, rw, rh);
        else MoonBridge.sendMousePosition(x, y, rw, rh);
    }

    static void sendMouseMoveAsMousePosition(
            NvConnection c, short dx, short dy, short rw, short rh) {
        if (!c.canSendInput()) return;
        if (c.inputPoller != null) c.inputBatcher.mouse(2, dx, dy, rw, rh);
        else MoonBridge.sendMouseMoveAsMousePosition(dx, dy, rw, rh);
    }

    static void sendOwnedMouseButton(NvConnection c, byte button, boolean down, int source) {
        if (!down || c.canSendInput()) {
            c.inputBatcher.boundary(
                    () -> {
                        synchronized (c.mouseButtonOwnership) {
                            if (down && !c.canSendInput()) return;
                            int transition = c.mouseButtonOwnership.update(button, down, source);
                            if (c.canSendInput() && transition != 0) {
                                MoonBridge.sendMouseButton(
                                        transition > 0
                                                ? MouseButtonPacket.PRESS_EVENT
                                                : MouseButtonPacket.RELEASE_EVENT,
                                        button);
                            }
                        }
                    });
        }
    }

    static void sendControllerInput(
            NvConnection c,
            short controllerNumber,
            short activeGamepadMask,
            int buttonFlags,
            byte leftTrigger,
            byte rightTrigger,
            short leftStickX,
            short leftStickY,
            short rightStickX,
            short rightStickY) {
        if (!c.canSendControllerInput()) return;
        if (c.inputPoller != null) {
            c.inputBatcher.controller(
                    controllerNumber,
                    activeGamepadMask,
                    buttonFlags,
                    leftTrigger,
                    rightTrigger,
                    () -> {
                        if (c.canSendControllerInput()) {
                            MoonBridge.sendMultiControllerInput(
                                    controllerNumber,
                                    activeGamepadMask,
                                    buttonFlags,
                                    leftTrigger,
                                    rightTrigger,
                                    leftStickX,
                                    leftStickY,
                                    rightStickX,
                                    rightStickY);
                        }
                    });
        } else {
            MoonBridge.sendMultiControllerInput(
                    controllerNumber,
                    activeGamepadMask,
                    buttonFlags,
                    leftTrigger,
                    rightTrigger,
                    leftStickX,
                    leftStickY,
                    rightStickX,
                    rightStickY);
        }
    }

    static void sendKeyboardInput(
            NvConnection c, short keyMap, byte keyDirection, byte modifier, byte flags) {
        if (c.canSendInput()) {
            c.inputBatcher.boundary(
                    () -> {
                        if (c.canSendInput()) {
                            MoonBridge.sendKeyboardInput(keyMap, keyDirection, modifier, flags);
                        }
                    });
        }
    }

    static void sendMouseHighResScroll(NvConnection c, short scrollAmount) {
        if (c.canSendInput()) {
            c.inputBatcher.boundary(
                    () -> {
                        if (c.canSendInput()) MoonBridge.sendMouseHighResScroll(scrollAmount);
                    });
        }
    }

    static void sendMouseHighResHScroll(NvConnection c, short scrollAmount) {
        if (c.canSendInput()) {
            c.inputBatcher.boundary(
                    () -> {
                        if (c.canSendInput()) MoonBridge.sendMouseHighResHScroll(scrollAmount);
                    });
        }
    }

    static void sendUtf8Text(NvConnection c, String text) {
        if (c.canSendInput()) {
            c.inputBatcher.boundary(
                    () -> {
                        if (c.canSendInput()) {
                            MoonBridge.sendUtf8Text(text.getBytes(StandardCharsets.UTF_8));
                        }
                    });
        }
    }
}
