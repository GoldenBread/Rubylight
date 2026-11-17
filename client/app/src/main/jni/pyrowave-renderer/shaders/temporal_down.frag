#version 450
// Nightjar temporal upscaler: halves a luma level with a 2x2 box (the next pyramid level).
// Vulkan port of assets/shaders/temporal_down.frag.
layout(set = 0, binding = 0) uniform sampler2D level;
layout(location = 0) out vec4 luma;

void main() {
    ivec2 p = ivec2(gl_FragCoord.xy) * 2;
    float s = texelFetch(level, p, 0).r + texelFetch(level, p + ivec2(1, 0), 0).r +
              texelFetch(level, p + ivec2(0, 1), 0).r + texelFetch(level, p + ivec2(1, 1), 0).r;
    luma = vec4(0.25 * s, 0.0, 0.0, 1.0);
}
