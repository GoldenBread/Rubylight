# HDR SDR content brightness and capture logging
- **Desktop Duplication SDR-in-HDR brightness matches Windows settings.** When capturing an SDR (`BGRA8`) surface during an HDR stream, AMF, NVENC, and PyroWave now scale SDR white by the display's Windows HDR SDR content brightness (`DISPLAYCONFIG_SDR_WHITE_LEVEL`) rather than a fixed 100-nit default.
- **Capture open logs show surface format and HDR state.** Desktop Duplication and Windows Graphics Capture openings now record the returned DXGI/DirectX pixel format and whether the captured display is in HDR.
