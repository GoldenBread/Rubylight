#pragma once

#include <stdint.h>

// Jumbo video packets (Rubylight's `x-rl-max-packet-size`), kept in step with
// rubylight_protocol::video_packet: the largest packet this client asks for, stock
// Moonlight's size, and the encryption header a packet size includes.
#define RL_MAX_VIDEO_PACKET 8192u
#define RL_STOCK_MAX_VIDEO_PACKET 1400u
#define RL_STOCK_PACKET_SIZE 1392
#define RL_ENC_VIDEO_HEADER 32

// The offer this connection's DESCRIBE named; 0 without one, and stock packets then.
extern uint16_t RlMaxVideoPacketOffered;

// An offer as the host sends it: a multiple of 16, capped, and 0 when it is no more than stock.
static inline uint16_t rlVideoPacketOffer(uint32_t value) {
    uint32_t packet = value > RL_MAX_VIDEO_PACKET ? RL_MAX_VIDEO_PACKET : value;
    packet &= ~0xFu;
    return packet > RL_STOCK_MAX_VIDEO_PACKET ? (uint16_t)packet : 0;
}

// The packet size the ANNOUNCE asks for, including the encryption header: a stock request is
// unchanged; a jumbo one is no larger than the stream's offer, rounded down to a multiple of 16,
// and falls back to stock's 1392 without an offer. The caller subtracts the encryption header.
static inline int rlAnnouncedPacketSize(int configured, uint16_t offered) {
    if (configured <= (int)RL_STOCK_MAX_VIDEO_PACKET) return configured;
    int offer = offered == 0 ? RL_STOCK_PACKET_SIZE : (int)offered;
    int packet = configured < offer ? configured : offer;
    return packet - packet % 16;
}
