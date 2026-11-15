#pragma once
#include <stddef.h>
#include <stdint.h>

// Internal packet envelope. A zero container count distinguishes records from ordinary framing.
#define PYRO_PACKET_CONTIGUOUS 1u
#define PYRO_PACKET_RECORD_START 2u
#define PYRO_PACKET_FRAME_START 4u
#define PYRO_PACKET_FRAME_END 8u
// The largest packet a stream can negotiate (rubylight_protocol::video_packet) and the largest
// frame 4092 such packets build. A stream uses its own negotiated size (pyroMaxFrameBytes).
#define PYRO_MAX_PACKET_BYTES 8192u
#define PYRO_MAX_FRAME_BYTES (4092u * PYRO_MAX_PACKET_BYTES)

// The packet payload cap of a stream whose STREAM_CONFIGURATION.packetSize is packetSize: stock
// streams keep 1400 even though an encrypted stream negotiates 1360, and a jumbo stream gets the
// size it announced.
static inline uint32_t pyroMaxPacketBytes(int packetSize) {
    if (packetSize < 1400) return 1400u;
    if (packetSize > (int)PYRO_MAX_PACKET_BYTES) return PYRO_MAX_PACKET_BYTES;
    return (uint32_t)packetSize;
}

// The records envelope cap for a stream: 4092 packets of its negotiated size.
static inline size_t pyroMaxFrameBytes(int packetSize) {
    return 4092u * (size_t)pyroMaxPacketBytes(packetSize);
}

static inline void pyroWriteLe32(uint8_t *p, uint32_t value) {
    for (int i = 0; i < 4; ++i) p[i] = (uint8_t)(value >> (8 * i));
}

static inline uint32_t pyroReadLe32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}
