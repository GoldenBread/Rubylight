#version 450
#extension GL_GOOGLE_include_directive : require
#include "rotation.glsl"
// Adapted from joemossjr16/artemis-android-pyrowave (GPL-3.0).
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 frag;
layout(set = 0, binding = 0) uniform sampler2D u_y;
layout(set = 0, binding = 1) uniform sampler2D u_cb;
layout(set = 0, binding = 2) uniform sampler2D u_cr;
layout(push_constant) uniform Color { int ten_bit; int hdr; int full_range; int rotationQuarterTurns; } color;
#include "ycbcr.glsl"
#ifdef FRAME_WARP
#include "frame_warp.glsl"
#endif

void main() {
    vec2 uv = pyroSourceUv(v_uv, color.rotationQuarterTurns);
#ifdef FRAME_WARP
    float warpFade;
    uv = frameWarpUv(uv, warpFade);
#endif
    vec3 rgb = pyroPlanesToRgb(uv, color.ten_bit, color.hdr, color.full_range);
#ifdef FRAME_WARP
    rgb *= warpFade;
#endif
    frag = vec4(clamp(rgb, 0.0, 1.0), 1.0);
}
