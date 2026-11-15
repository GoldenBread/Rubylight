# NVIDIA NVENC defaults

- **Both NVENC engines when one is not enough.** `nvenc_split_encode = auto` now keeps the driver's implicit split and also splits HEVC or AV1 frames when one engine's estimated frame time misses the frame budget. `driver_decides` keeps only the driver's implicit split, and the **Lowest latency** stream preset asks for split at every preset. Splitting is faster and can cost some picture quality.
- **The frame time guard no longer counts engines that cannot split.** It accounts for H.264, drivers before NVENC API 12.2, one-engine GPUs and HEVC weighted prediction, and reports the split decision in the host log.
