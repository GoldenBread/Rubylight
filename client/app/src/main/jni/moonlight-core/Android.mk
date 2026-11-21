# Android.mk for libmoonlight-core: the small native library Java loads with MoonBridge,
# AAudio output, DualSense USB audio and the controller tables. The streaming engine is
# nightjar-core (Rust, libnightjar_core.so).
MY_LOCAL_PATH := $(call my-dir)

include $(call all-subdir-makefiles)

LOCAL_PATH := $(MY_LOCAL_PATH)

include $(CLEAR_VARS)
LOCAL_MODULE    := moonlight-core

LOCAL_SRC_FILES := aaudio_output.c \
                   dualsense_usb_audio.c \
                   minisdl.c \
                   simplejni.c

LOCAL_C_INCLUDES := $(LOCAL_PATH)

ifeq ($(NDK_DEBUG),1)
LOCAL_CFLAGS += -DLC_DEBUG
endif

LOCAL_LDLIBS := -llog -ldl

LOCAL_LDFLAGS += -Wl,--exclude-libs,ALL

LOCAL_BRANCH_PROTECTION := standard

include $(BUILD_SHARED_LIBRARY)
