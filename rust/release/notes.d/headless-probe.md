# Codec check without a display
- **With no display attached, the host reports the codecs of the graphics card that encodes the stream.** On a PC with an AMD integrated GPU and an NVIDIA card, the check ran on the integrated GPU, so clients were offered its codecs (for example no 4:4:4) while the NVIDIA card encoded.
