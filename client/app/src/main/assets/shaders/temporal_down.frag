#version 300 es
// Nightjar temporal upscaler: halves a luma level with a 2x2 box (the next pyramid level).
precision highp float;
uniform highp sampler2D level;
out vec4 luma;

void main() {
    ivec2 p = ivec2(gl_FragCoord.xy) * 2;
    float s = texelFetch(level, p, 0).r + texelFetch(level, p + ivec2(1, 0), 0).r +
              texelFetch(level, p + ivec2(0, 1), 0).r + texelFetch(level, p + ivec2(1, 1), 0).r;
    luma = vec4(0.25 * s, 0.0, 0.0, 1.0);
}
