## en
- New setting Fractional V-Sync (off by default): on a screen at 2 to 4 times the stream's frame rate, such as 60 fps on 120 Hz, every frame stays for the same number of refreshes, so motion no longer judders when V-Sync is on and VRR is off. On Android it works with Lowest latency frame pacing; Balanced already keeps that cadence. Measured on the Windows desktop client, it adds 1 to 2 ms of delay on most frames and 2.5 to 4 ms on the slowest ones. Where it holds nothing, for example at a refresh that isn't 2 to 4 times the frame rate, with Render through Vulkan or with Frame Warp, the setting says why.

## desktop
- Fractional V-Sync holds frames on Windows, and on Linux with Wayland or KMS. On X11 and macOS, and in Windows Fullscreen when the mode it switches to runs at the stream's own frame rate, the row says it is not used.
