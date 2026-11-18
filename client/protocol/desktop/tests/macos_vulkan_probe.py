"""List the Vulkan devices a loader finds through one ICD manifest, as Nightjar.app does on a Mac.

    python3 macos_vulkan_probe.py <libvulkan.1.dylib> <icd.json>

Prints macOS's version and each device's name, driver and API version. Exits 0 with or without
a device (a Mac older than 26, Intel, or a virtual GPU KosmicKrisp does not take); 1 when the
loader itself fails to load or create an instance.
"""
import ctypes
import os
import platform
import subprocess
import sys

VK_STRUCTURE_TYPE_APPLICATION_INFO = 0
VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO = 1
VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2 = 1000059001
VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_DRIVER_PROPERTIES = 1000196000
VK_INSTANCE_CREATE_ENUMERATE_PORTABILITY_BIT_KHR = 1


class AppInfo(ctypes.Structure):
    _fields_ = [("sType", ctypes.c_int), ("pNext", ctypes.c_void_p), ("pApplicationName", ctypes.c_char_p),
                ("applicationVersion", ctypes.c_uint32), ("pEngineName", ctypes.c_char_p),
                ("engineVersion", ctypes.c_uint32), ("apiVersion", ctypes.c_uint32)]


class InstanceInfo(ctypes.Structure):
    _fields_ = [("sType", ctypes.c_int), ("pNext", ctypes.c_void_p), ("flags", ctypes.c_uint32),
                ("pApplicationInfo", ctypes.POINTER(AppInfo)), ("enabledLayerCount", ctypes.c_uint32),
                ("ppEnabledLayerNames", ctypes.c_void_p), ("enabledExtensionCount", ctypes.c_uint32),
                ("ppEnabledExtensionNames", ctypes.POINTER(ctypes.c_char_p))]


class Properties(ctypes.Structure):
    # VkPhysicalDeviceProperties up to deviceName; the rest is room for the limits and sparse
    # properties, which are not read.
    _fields_ = [("apiVersion", ctypes.c_uint32), ("driverVersion", ctypes.c_uint32), ("vendorID", ctypes.c_uint32),
                ("deviceID", ctypes.c_uint32), ("deviceType", ctypes.c_int), ("deviceName", ctypes.c_char * 256),
                ("rest", ctypes.c_ubyte * 1024)]


class DriverProperties(ctypes.Structure):
    _fields_ = [("sType", ctypes.c_int), ("pNext", ctypes.c_void_p), ("driverID", ctypes.c_int),
                ("driverName", ctypes.c_char * 256), ("driverInfo", ctypes.c_char * 256),
                ("conformanceVersion", ctypes.c_uint32)]


class Properties2(ctypes.Structure):
    _fields_ = [("sType", ctypes.c_int), ("pNext", ctypes.c_void_p), ("properties", Properties)]


def version(v):
    return f"{v >> 22}.{(v >> 12) & 0x3FF}.{v & 0xFFF}"


def main():
    loader, icd = sys.argv[1], sys.argv[2]
    print("macOS", platform.mac_ver()[0], platform.machine())
    print(subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip())
    os.environ["VK_DRIVER_FILES"] = icd
    os.environ.pop("VK_ICD_FILENAMES", None)
    vk = ctypes.CDLL(loader)
    app = AppInfo(VK_STRUCTURE_TYPE_APPLICATION_INFO, None, b"nightjar-probe", 1, b"nightjar", 1, (1 << 22) | (3 << 12))
    extensions = (ctypes.c_char_p * 1)(b"VK_KHR_portability_enumeration")
    info = InstanceInfo(VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, None, VK_INSTANCE_CREATE_ENUMERATE_PORTABILITY_BIT_KHR,
                        ctypes.pointer(app), 0, None, 1, extensions)
    instance = ctypes.c_void_p()
    result = vk.vkCreateInstance(ctypes.byref(info), None, ctypes.byref(instance))
    if result != 0:
        # Without the portability extension (a loader with no driver offers none).
        info.flags, info.enabledExtensionCount = 0, 0
        result = vk.vkCreateInstance(ctypes.byref(info), None, ctypes.byref(instance))
    if result != 0:
        print(f"vkCreateInstance failed: {result}")
        return 1
    count = ctypes.c_uint32()
    result = vk.vkEnumeratePhysicalDevices(instance, ctypes.byref(count), None)
    print(f"devices: {count.value} (vkEnumeratePhysicalDevices {result})")
    devices = (ctypes.c_void_p * count.value)()
    vk.vkEnumeratePhysicalDevices(instance, ctypes.byref(count), devices)
    for device in devices:
        driver = DriverProperties(VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_DRIVER_PROPERTIES, None)
        props = Properties2(VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2, ctypes.cast(ctypes.pointer(driver), ctypes.c_void_p))
        vk.vkGetPhysicalDeviceProperties2(ctypes.c_void_p(device), ctypes.byref(props))
        p = props.properties
        print(f"device: {p.deviceName.decode()} | Vulkan {version(p.apiVersion)} | driver {driver.driverName.decode()} "
              f"{driver.driverInfo.decode()}")
    vk.vkDestroyInstance(instance, None)
    return 0


if __name__ == "__main__":
    sys.exit(main())
