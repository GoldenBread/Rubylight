## en
- Fixed intermittent 120 Hz judder and Presented FPS drops in Balanced pacing on both the Vulkan and standard Android renderers by aligning Vulkan present targets to the display's hardware scanout phase, keeping Balanced's vsync cadence monotonic across frame-timeline boundaries, and accounting for compositor report lag in Presented FPS.
