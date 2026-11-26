---
name: isolated-e2e
description: Run isolated end-to-end streaming, protocol, soak, bottleneck, and hardware tests against a packaged Rubylight host without touching the installed service.
---

# Isolated End-to-End & Hardware Tests

Use this skill when validating a packaged Rubylight host end to end (`e2e.py`, `protocol.py`, `interop.py`, `soak.py`, `bottleneck.py`, `tools/hw/run.py`).

## Critical safety rules

1. **Always isolate from the installed host.** Device and E2E tests spawn a test-owned host on its own profile and ports (`48518-48544`, default HTTP `--port 48523`, bound to `127.0.0.1` only). Never edit or restart the installed service unless explicitly running an install check (`check.ps1`).
2. **Installed host must be idle** before starting any GPU/E2E stream (`e2e_host.installed_idle()` checks it).
3. **Exit code 3 = AMD video-engine hang — STOP, DO NOT RETRY.**
   - `e2e.py` and `check.ps1` (via `rust/release/check_streams.ps1`) exit with code `3` (`rust/release/gpu_hang.py`) when an encoder call stalls >=2 s, stall watch remains unresolved after 5 s, or a new LiveKernelReport appears in `C:\Windows\LiveKernelReports`.
   - Inspect `<work>/e2e-<name>/result.json` (`gpu_hang`) and `<work>/e2e-<name>/config/logs/butterpollo.log` (installed host log: `C:\ProgramData\Butterpollo\config\logs\butterpollo.log`).
   - Do NOT launch another stream on a resetting GPU; check Device Manager for Code 31 and reboot if wedged.
4. **Respect shared machine locks (`C:\src\butterpollo-release`):** Never delete or break `GPU-LOCK`, `BUILD-LOCK`, `RELEASE-LOCK`, or `PRIORITY`. Run Cargo with `python tools/dev.py cargo -- <args>` and wrap Web commands with `python tools/build_lock.py`. Always set `$env:TEMP="D:\t"; $env:TMP="D:\t"`.

## Common isolated E2E commands

Machine settings (`$BUTTERPOLLO_TEST_PYTHON`, fixtures in `C:\src\butterpollo-release\fixtures`) live in `C:\src\butterpollo-release\settings.ps1`.

- **Single isolated stream (`e2e.py`):**
  ```powershell
  $env:TEMP="D:\t"; $env:TMP="D:\t"
  python rust/release/e2e.py --package <pkg-dir> --work <work-dir> `
      --client C:\src\butterpollo-release\fixtures\moonlight-client.exe `
      --codec hevc --mode 1920x1080x60 --seconds 12 --record
  ```
  Useful flags: `--vrr`, `--recovery 16`, `--motion-at-rate`, `--hw-decoder d3d11va`, `--config key=value`, `--content <clip.y4m>`, `--port 48523`.
- **Pairing, launch & web API checks:**
  - `python rust/release/protocol.py --package <pkg-dir> --work <work-dir>` (isolated pairing & launch protocol)
  - `python rust/tests/interop.py <artifact-dir> [codec]`
  - `python rust/tests/web_api.py`
- **Shaped slow-link test (`bottleneck.py`):**
  ```powershell
  python tools/dev.py cargo -- build --release -p butterpollo-probes --bin udp_shaper
  New-Item -ItemType Directory -Force <work-dir> | Out-Null
  Copy-Item <target>\release\udp_shaper.exe <work-dir>
  python rust/release/bottleneck.py --package <pkg-dir> --work <work-dir> `
      --client C:\src\butterpollo-release\fixtures\moonlight-client.exe `
      --shaper <work-dir>\udp_shaper.exe `
      --codec av1 --mode 2560x1440x60 --bitrate 40000 --rate-kbps 100000 --queue-kib 64 --rfi
  ```
  `<target>` is the pool folder `dev.py cargo` prints (`dev cargo: target ...`). Copy the shaper right away: the slot's lease ends with the build, and another session may rebuild or prune it.
- **Bounded soak & fault injection:** `pwsh rust/release/soak.ps1` (or `rust/release/soak.py`).
- **Stage timing report:** `python rust/release/timing_report.py <frames.csv> --host-record <host-record>`.
- **Hardware batch / laptop host:** `tools\hw\run.ps1 -Batch <device>` (`tools/hw/catalog.yaml`), `pwsh tools\hw\laptop-host.ps1 -Out <dir>`.

## Reading results

- Each `e2e.py` case writes `<work>/e2e-<name>/result.json`, `frames.csv`, host timing record (with `--record`), and `config/logs/butterpollo.log`.
- Check `quiet_wait`, `machine_load`, `audio_sink`, `warnings`, `timing`, and `gpu_hang` in `result.json` before attributing a failure to a code regression.
