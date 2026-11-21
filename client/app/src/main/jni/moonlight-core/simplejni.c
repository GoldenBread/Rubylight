// The controller tables Java asks about: what kind of pad a vendor/product id is. The rest of
// libmoonlight-core is AAudio output (aaudio_output.c) and the DualSense USB audio
// (dualsense_usb_audio.c); the streaming engine is nightjar-core (Rust).
#include <jni.h>

#include "minisdl.h"
#include "controller_type.h"
#include "controller_list.h"

// Equal to MoonBridge.LI_CTYPE_*.
#define LI_CTYPE_UNKNOWN  0x00
#define LI_CTYPE_XBOX     0x01
#define LI_CTYPE_PS       0x02
#define LI_CTYPE_NINTENDO 0x03
#define LI_CTYPE_STEAM    0x04

JNIEXPORT jbyte JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_guessControllerType(JNIEnv *env, jclass clazz, jint vendorId, jint productId) {
    unsigned int unDeviceID = MAKE_CONTROLLER_ID(vendorId, productId);
    for (int i = 0; i < sizeof(arrControllers) / sizeof(arrControllers[0]); i++) {
        if (unDeviceID == arrControllers[i].m_unDeviceID) {
            switch (arrControllers[i].m_eControllerType) {
                case k_eControllerType_XBox360Controller:
                case k_eControllerType_XBoxOneController:
                case k_eControllerType_XBoxEliteController:
                    return LI_CTYPE_XBOX;

                case k_eControllerType_PS3Controller:
                case k_eControllerType_PS4Controller:
                case k_eControllerType_PS5Controller:
                case k_eControllerType_PS5EdgeController:
                    return LI_CTYPE_PS;

                case k_eControllerType_WiiController:
                case k_eControllerType_SwitchProController:
                case k_eControllerType_SwitchJoyConLeft:
                case k_eControllerType_SwitchJoyConRight:
                case k_eControllerType_SwitchJoyConPair:
                case k_eControllerType_SwitchInputOnlyController:
                case k_eControllerType_Switch2ProController:
                case k_eControllerType_Switch2InputOnlyController:
                    return LI_CTYPE_NINTENDO;

                case k_eControllerType_SteamController:
                case k_eControllerType_SteamControllerV2:
                case k_eControllerType_SteamControllerNeptune:
                case k_eControllerType_SteamControllerTriton:
                case k_eControllerType_HoriSteamController:
                case k_eControllerType_UnknownSteamController:
                    return LI_CTYPE_STEAM;

                default:
                    return LI_CTYPE_UNKNOWN;
            }
        }
    }
    return LI_CTYPE_UNKNOWN;
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_guessControllerHasPaddles(JNIEnv *env, jclass clazz, jint vendorId, jint productId) {
    // Xbox Elite and DualSense Edge controllers have paddles
    return SDL_IsJoystickXboxOneElite(vendorId, productId) || SDL_IsJoystickDualSenseEdge(vendorId, productId);
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_guessControllerHasShareButton(JNIEnv *env, jclass clazz, jint vendorId, jint productId) {
    // Xbox Series X controllers have a share button
    return SDL_IsJoystickXboxSeriesX(vendorId, productId);
}
