#pragma once

#include <algorithm>
#include <cstdint>
#include <vector>

// Which Vulkan path a device takes, decided from its runtime probe alone. Vulkan 1.3 keeps the
// core features, two queues and the timeline semaphore; below that the features come from
// extensions and decode completion is a fence on one shared queue.
constexpr uint32_t PYRO_VULKAN_1_1 = (1u << 22) | (1u << 12);
constexpr uint32_t PYRO_VULKAN_1_2 = (1u << 22) | (2u << 12);
constexpr uint32_t PYRO_VULKAN_1_3 = (1u << 22) | (3u << 12);

// PyroWave reads its payload through texel buffers when 8-bit storage is missing and a buffer
// view can hold this many elements (pyrowave_common.cpp).
constexpr uint32_t PYRO_TEXEL_BUFFER_ELEMENTS = 16u * 1024u * 1024u;

// Why a custom Adreno driver (Snapdragon GPU driver setting) could not run PyroWave, ahead of the
// device checks. They follow Readiness in pyrowave_renderer.cpp, whose codes 1-9 are the device
// checks', and PyroWaveDecoderRenderer.customDriverFailureSummary turns them into the notice.
constexpr int PYRO_DRIVER_NOT_OPENED = 10;   // the system Vulkan loader did not open with the hooks
constexpr int PYRO_DRIVER_NOT_LOADED = 11;   // the hook did not load the driver library
constexpr int PYRO_DRIVER_NOT_STARTED = 12;  // the library loaded, but Vulkan did not start on it
constexpr int PYRO_DRIVER_NO_GPU = 13;       // Vulkan started on it and listed no GPU
constexpr int PYRO_DRIVER_NOT_VULKAN = 14;   // the library opened but is not an Android Vulkan driver

// How far a readiness probe got: through the loader and vkCreateInstance, through listing the GPUs,
// or to a device check whose own reason stands.
enum PyroProbeStage { PYRO_STAGE_LOADER, PYRO_STAGE_DEVICES, PYRO_STAGE_CHECKED };

// A custom driver's readiness. opened: the hooked loader opened; loaded: its hook loaded the driver
// library, so the system driver was not quietly used instead; notVulkan: the hook opened the library
// and turned it away. A library that loaded and then stopped before any GPU was checked names that
// stage, not the device check it never reached.
inline int pyroCustomDriverReadiness(bool opened, bool loaded, bool notVulkan, PyroProbeStage stage, int readiness) {
    if (!opened) return PYRO_DRIVER_NOT_OPENED;
    if (notVulkan) return PYRO_DRIVER_NOT_VULKAN;
    if (!loaded) return PYRO_DRIVER_NOT_LOADED;
    if (stage == PYRO_STAGE_LOADER) return PYRO_DRIVER_NOT_STARTED;
    if (stage == PYRO_STAGE_DEVICES) return PYRO_DRIVER_NO_GPU;
    return readiness;
}

// Vulkan 1.3 where the loader has it, else 1.1. forceVulkan11 is the test switch that puts a newer
// device on the Vulkan 1.1 path.
inline uint32_t pyroInstanceApiVersion(uint32_t loaderVersion, bool forceVulkan11) {
    return loaderVersion >= PYRO_VULKAN_1_3 && !forceVulkan11 ? PYRO_VULKAN_1_3 : PYRO_VULKAN_1_1;
}

// Granite takes the same lower of the two, so renderer and SDK agree on which structs a device reads.
inline uint32_t pyroCoreVersion(uint32_t deviceVersion, uint32_t instanceVersion) {
    return std::min(deviceVersion, instanceVersion);
}

inline bool pyroLegacySync(uint32_t coreVersion) {
    return coreVersion < PYRO_VULKAN_1_3;
}

// Without timeline semaphores submission order on one queue carries the decoder's writes to the draw.
inline uint32_t pyroQueueCount(uint32_t familyQueues, bool legacySync) {
    return familyQueues >= 2 && !legacySync ? 2 : 1;
}

// Vulkan 1.3 devices keep their requirement of 8-bit storage.
inline bool pyroPayloadReadable(uint32_t coreVersion, bool storage8, uint32_t maxTexelBufferElements) {
    return storage8 || (coreVersion < PYRO_VULKAN_1_3 && maxTexelBufferElements >= PYRO_TEXEL_BUFFER_ELEMENTS);
}

inline bool pyroSyncUsable(uint32_t coreVersion, bool timelineSemaphore, bool synchronization2) {
    return coreVersion < PYRO_VULKAN_1_3 || (timelineSemaphore && synchronization2);
}

// Granite needs render pass 2 below Vulkan 1.2.
inline bool pyroNeedsRenderPass2(uint32_t coreVersion) {
    return coreVersion < PYRO_VULKAN_1_2;
}

inline bool pyroNeedsSubgroupSizeControlExtension(uint32_t coreVersion) {
    return coreVersion < PYRO_VULKAN_1_3;
}

// Adreno and Mali depend on PyroWave's fragment decode path and its driver workarounds, which go by
// Granite's driver id. Below Vulkan 1.2 that id takes VK_KHR_driver_properties: without it those
// GPUs would decode on the wrong path with no sign of it, so they are refused.
constexpr uint32_t PYRO_VENDOR_QUALCOMM = 0x5143;
constexpr uint32_t PYRO_VENDOR_ARM = 0x13B5;

inline bool pyroDriverIdKnown(uint32_t coreVersion, uint32_t vendorId, bool driverProperties) {
    return coreVersion >= PYRO_VULKAN_1_2 || driverProperties ||
           (vendorId != PYRO_VENDOR_QUALCOMM && vendorId != PYRO_VENDOR_ARM);
}

// The device extensions the path adds, none on Vulkan 1.3. Granite only sees what is enabled on the
// device it is lent: without VK_KHR_driver_properties below Vulkan 1.2 it has no driver id, so
// Adreno and Mali would lose PyroWave's fragment decode path and their driver workarounds.
inline std::vector<const char *> pyroPathExtensions(uint32_t coreVersion, bool storage8, bool float16, bool driverProperties) {
    std::vector<const char *> names;
    if (pyroNeedsSubgroupSizeControlExtension(coreVersion)) names.push_back("VK_EXT_subgroup_size_control");
    if (coreVersion < PYRO_VULKAN_1_2) {
        names.push_back("VK_KHR_create_renderpass2");
        if (storage8) names.push_back("VK_KHR_8bit_storage");
        if (float16) names.push_back("VK_KHR_shader_float16_int8");
        if (driverProperties) names.push_back("VK_KHR_driver_properties");
    }
    return names;
}
