#version 450
// Nightjar temporal upscaler, scene-cut pass: one fragment averages the motion confidence over a
// fixed 32x18 grid of blocks. The accumulate pass resets history when the mean is under 0.25.
// Vulkan port of assets/shaders/temporal_cut.frag.
layout(set = 0, binding = 0) uniform sampler2D motion;
layout(location = 0) out vec4 result;

void main() {
    ivec2 size = textureSize(motion, 0);
    float sum = 0.0;
    for (int y = 0; y < 18; y++) {
        for (int x = 0; x < 32; x++) {
            ivec2 b = ivec2((vec2(x, y) + 0.5) * vec2(size) / vec2(32.0, 18.0));
            sum += texelFetch(motion, min(b, size - 1), 0).b;
        }
    }
    result = vec4(sum / 576.0, 0.0, 0.0, 1.0);
}
