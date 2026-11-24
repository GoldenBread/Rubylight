package com.limelight.nvstream.jni;

/**
 * Pure-JDK JNI declarations for {@code nightjar_policy::controller}
 * ({@code client/protocol/android/src/policy_input/}). Loads {@code nightjar_core} via
 * {@link NightjarJni#ensureLibraryLoaded()}.
 */
public final class NightjarInputJni {
    static {
        NightjarJni.ensureLibraryLoaded();
    }

    private NightjarInputJni() {
    }

    // --- 1. Deadzone & Normalization ---

    public static native void applyStickDeadzone(
            float x, float y, float deadzoneRadius, float[] out);

    public static native float applyTriggerDeadzone(float value, float deadzone);

    public static native byte maxByMagnitudeByte(byte a, byte b);

    public static native short maxByMagnitudeShort(short a, short b);

    public static native void mouseStickCurve(short stickX, short stickY, float[] out);

    public static native void normalizeAxisSet(
            float lsX, float lsY, float rsX, float rsY,
            float lt, float rt, float hatX, float hatY,
            boolean hasLeftStick, boolean hasRightStick, boolean hasTriggers, boolean hasHat,
            float leftDeadzone, float rightDeadzone, float triggerDeadzone,
            boolean triggersIdleNegative, boolean leftTriggerUsed, boolean rightTriggerUsed,
            boolean hatXUsed, boolean hatYUsed, int currentInputMap,
            int[] out);

    public static native void normalizeUsbAxes(
            float lsX, float lsY, float rsX, float rsY, float lt, float rt,
            float leftDeadzone, float rightDeadzone, float triggerDeadzone,
            int[] out);

    // --- 2. Remapping & Emulated Combos ---

    public static final int QUIRK_IGNORE_BACK = 1 << 0;
    public static final int QUIRK_HAS_SHARE = 1 << 1;
    public static final int QUIRK_DS4_STANDALONE_TOUCHPAD = 1 << 2;
    public static final int QUIRK_LINUX_STD_FACE = 1 << 3;
    public static final int QUIRK_NON_STD_DS4 = 1 << 4;
    public static final int QUIRK_SERVAL = 1 << 5;
    public static final int QUIRK_NON_STD_XBOX_BT = 1 << 6;
    public static final int QUIRK_NO_HAT = 1 << 7;
    public static final int QUIRK_BACK_IS_START = 1 << 8;
    public static final int QUIRK_MODE_IS_SELECT = 1 << 9;
    public static final int QUIRK_SEARCH_IS_MODE = 1 << 10;

    public static native int[] androidButtonKeycodes();

    public static native int remapKeyEvent(
            boolean backAsGuide, int keyCode, int scanCode, int flags, boolean hasNoModifiers,
            int vendorId, int productId, int sdkInt, int quirkFlags, int[] outStateUpdate);

    public static native int flipFaceButtonKeyCode(int keyCode);

    public static native int normalizeMappedTarget(int target);

    public static native boolean isHandledGamepadKey(int keyCode, int scanCode, boolean hasPaddles);

    public static native void buttonDownStep(
            int inputMap, byte leftTrigger, byte rightTrigger,
            boolean hasPaddles, boolean hatXUsed, boolean hatYUsed,
            boolean ltUsed, boolean rtUsed, int keyCode, int scanCode, int[] out);

    public static native void buttonUpStep(
            int inputMap, byte leftTrigger, byte rightTrigger,
            boolean hasPaddles, boolean hatXUsed, boolean hatYUsed,
            boolean ltUsed, boolean rtUsed, int keyCode, int scanCode, int[] out);

    public static native int heldButtons(
            int inputMap, int emulatingFlags,
            boolean hasTargetTouchpad, boolean hasSelect, boolean hasLb);

    public static native void applyEmulatedCombosOnDown(
            int inputMap, int emulatingFlags, boolean hasSelect, boolean hasMode,
            boolean needsClickpad, long eventTimeMs, long lastLbUpMs, long lastRbUpMs,
            int[] out);

    public static native void applyEmulatedCombosOnUp(
            int inputMap, int emulatingFlags,
            boolean hasTargetTouchpad, boolean hasSelect, boolean hasLb,
            int[] out);

    public static native void applyUsbClickpad(
            int buttons, int clickpadSwallowed, byte deviceType, byte connectAs,
            boolean hasDeviceMotion, boolean needsClickpad, int[] out);

    // --- 3. Arrival, Capabilities & Steam Controls ---

    public static native boolean countsAsExternalController(
            boolean external, boolean gamepad, boolean remote,
            boolean keyboard, boolean handheldControls);

    public static native boolean onscreenOwnsPlayerOne(
            boolean onscreenOwnPlayer, boolean multiController,
            boolean onscreenController, boolean onlyL3R3);

    public static native boolean looksLikeHandheldControls(
            String name, int vendorId, boolean onlyGamepad, boolean attachedUsb);

    public static native boolean isBackAsGuide(boolean backAsGuide, int keyCode, int flags);

    public static native boolean isPadBackAsGuide(
            boolean backAsGuide, int keyCode, int flags, boolean backIsStart);

    public static native boolean isHandheldSystemBack(
            int keyCode, int flags, int source, boolean virtualDevice, boolean external,
            boolean alphabeticKeyboard, boolean gamepad, boolean backIsSpareButton);

    public static native boolean backIsSpareButton(
            boolean internalGamepad, boolean internalSelect, boolean anyGamepad);

    public static native int isExternalOverride(String buildModel, String devName);

    public static native boolean shouldIgnoreBack(
            String devName, boolean hasJoystickAxes, boolean hasGamepadButtons,
            boolean external, boolean internalGamepad, boolean internalSelect);

    public static native byte arrivalType(
            byte detectedType, byte connectAs, int targetButtonFlags);

    public static native byte inputDeviceArrivalType(
            byte detectedType, boolean hasMotion, byte connectAs, int targetButtonFlags);

    public static native boolean dropsMotionAndTouchpad(byte connectAs);

    public static native short arrivalCapabilities(short capabilities, byte connectAs);

    public static native boolean takesDeviceMotion(
            boolean fallbackToDevice, short relativeSlot, short padCapabilities, byte connectAs);

    public static native boolean motionOverridesType(
            byte detectedType, boolean hasMotion, byte connectAs);

    public static native boolean needsClickpadEmulation(int targetButtonFlags);

    public static native byte onscreenPadType(boolean touchpadButton);

    public static native int onscreenPadButtons(boolean touchpadButton, boolean steamControls);

    public static native short onscreenPadCapabilities(boolean steamControls);

    public static native boolean carriesSteamControls(
            boolean steamControls, boolean onscreenOwnPlayer, short controllerNumber);

    public static native short withSteamControlsCapabilities(
            boolean steamControls, boolean onscreenOwnPlayer, short controllerNumber,
            short capabilities, byte connectAs);

    public static native int withSteamControlsButtons(
            boolean steamControls, boolean onscreenOwnPlayer, short controllerNumber, int buttons);

    public static native void classifyInputDevice(
            int vendorId, int productId, String devName, boolean isKeyboardMouseOnly,
            boolean hasLTriggerRTrigger, boolean hasBrakeGas, boolean hasBrakeThrottle,
            boolean hasGasRange, boolean hasRxRy, boolean hasZRz,
            boolean hasButtonC, boolean hasHat, boolean hasStartOrMenu,
            boolean initialHasSelect, boolean initialHasMode, boolean ignoreBack,
            float baseTriggerDeadzone, int[] outInts, float[] outTriggerDeadzone);

    public static native int computeBaseSupportedButtons(
            boolean[] hasKeys, boolean hasPaddles, boolean hasShare,
            int targetButtonFlags, boolean hasHatX, boolean hasHatY, boolean hasClickpad);

    public static native void computeInputDeviceArrival(
            byte detectedType, int baseSupportedButtons, short baseCapabilities,
            boolean hasMotion, byte connectAs, int targetButtonFlags,
            boolean carriesTouchpadButton, boolean steamControls,
            boolean onscreenOwnPlayer, short controllerNumber, int[] out);

    public static native String formatLaunchController(
            short slot, byte kind, short capabilities, int buttons);

    // --- 4. Slot Reservation & Context Fusion ---

    public static native long slotStateCreate(
            short initialControllers, boolean onscreenOwnPlayerSession,
            boolean touchpadButtonSession, boolean steamControlsSession);

    public static native void slotStateDestroy(long handle);

    public static native short activeControllerMask(
            long handle, boolean sendControllers, boolean multiController,
            boolean onscreenController, boolean layoutGamepadUsed, boolean hasBuiltInPlayerOne);

    public static native int assignInputDeviceSlot(
            long handle, boolean external, boolean hasJoystickAxes,
            boolean hasGamepadButtons, boolean hasAdjacentAssociatedJoystick,
            boolean multiController, int builtInPadSlot);

    public static native int resolveAssociatedFallbackSlot(long handle);

    public static native int assignUsbDeviceSlot(long handle, boolean multiController);

    public static native int releaseReservedSlot(
            long handle, short slot, boolean wasReserved, boolean assigned,
            boolean sendControllers, boolean multiController, boolean onscreenController,
            boolean layoutGamepadUsed, boolean hasBuiltInPlayerOne, boolean pendingUsbArrival);

    public static native void markSlotHasInput(long handle, short slot);

    public static native boolean slotHasInput(long handle, short slot);

    public static native boolean announcesOnscreenPad(
            long handle, boolean sendControllers, boolean onscreenController,
            boolean layoutGamepadUsed, boolean hasSlotZeroDevice);

    public static native int launchPassForInputDevice(
            boolean external, boolean hasJoystickAxes);

    public static native void fuseControllerInputs(
            int[] packedContexts, int count, boolean steamControlsSession,
            short controllerNumber, float steamPadTouchX, boolean mouseEmulationActive,
            int[] out);
}
