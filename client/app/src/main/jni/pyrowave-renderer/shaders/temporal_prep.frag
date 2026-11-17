#version 450
#extension GL_GOOGLE_include_directive : require
// Nightjar temporal upscaler, first pass; Vulkan port of assets/shaders/temporal_prep.frag. Converts
// PyroWave's 8-bit SDR YCbCr planes at stream size into a plain RGBA8 image (so later passes can
// fetch texels) with planar_csc.frag's conversion, and writes its luma for motion search. Rows are
// stored bottom row first, as the GLES port's textures hold them, so the later passes run the GLES
// port's math unchanged and temporal_detail.frag maps back as sgsr.frag does.
layout(location = 0) out vec4 color;
layout(location = 1) out vec4 luma;
layout(set = 0, binding = 0) uniform sampler2D u_y;
layout(set = 0, binding = 1) uniform sampler2D u_cb;
layout(set = 0, binding = 2) uniform sampler2D u_cr;
// planar_csc.frag's push constants (the rotation is unused: the frame stays in stream orientation),
// then the stream size.
layout(push_constant) uniform Params {
    int ten_bit;
    int hdr;
    int full_range;
    int rotationQuarterTurns;
    vec2 renderSize;
} params;
#include "ycbcr.glsl"

void main() {
    vec2 p = gl_FragCoord.xy / params.renderSize;
    vec3 rgb = clamp(pyroPlanesToRgb(vec2(p.x, 1.0 - p.y), params.ten_bit, params.hdr, params.full_range), 0.0, 1.0);
    color = vec4(rgb, 1.0);
    luma = vec4(dot(rgb, vec3(0.299, 0.587, 0.114)), 0.0, 0.0, 1.0);
}
