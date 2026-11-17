#version 300 es
// Nightjar temporal upscaler, motion pass: one fragment per 8x8 block of the stream.
// Hierarchical block matching on the luma pyramid (full, 1/2, 1/4): a full search of +-4 texels
// at quarter resolution over a 4x4 window plus last frame's vector of this block, +-1
// refinements at half and full resolution over 8x8 windows, then a parabolic sub-pixel fit.
// Writes (vx, vy, confidence, 1): prev(p + v) matches cur(p), v in stream pixels.
// Reference: client/tools/temporal_upscaler/upscalers.py, estimate_motion.
precision highp float;
precision highp int;
uniform highp sampler2D cur0;
uniform highp sampler2D cur1;
uniform highp sampler2D cur2;
uniform highp sampler2D prev0;
uniform highp sampler2D prev1;
uniform highp sampler2D prev2;
uniform highp sampler2D prevMotion;
out vec4 result;

const int R = 4;
const float LAMBDA = 0.5 / 255.0;
const float NOISE_FLOOR = 3.0 / 255.0;
const float CONF_LO = 0.35;
const float CONF_HI = 0.8;

float at(highp sampler2D t, ivec2 p) {
    return texelFetch(t, clamp(p, ivec2(0), textureSize(t, 0) - 1), 0).r;
}

void main() {
    ivec2 block = ivec2(gl_FragCoord.xy);

    // Quarter resolution: 4x4 window from texel block * 2 - 1.
    ivec2 o2 = block * 2 - 1;
    float c2[16];
    for (int i = 0; i < 16; i++) c2[i] = at(cur2, o2 + ivec2(i % 4, i / 4));
    float bestCost = 1.0e30;
    ivec2 best = ivec2(0);
    for (int dy = -R; dy <= R; dy++) {
        for (int dx = -R; dx <= R; dx++) {
            float s = 0.0;
            for (int i = 0; i < 16; i++) s += abs(c2[i] - at(prev2, o2 + ivec2(i % 4, i / 4) + ivec2(dx, dy)));
            float cost = s / 16.0 + LAMBDA * float(abs(dx) + abs(dy));
            if (cost < bestCost) { bestCost = cost; best = ivec2(dx, dy); }
        }
    }
    ivec2 pq = ivec2(roundEven(texelFetch(prevMotion, block, 0).xy / 4.0));
    float s = 0.0;
    for (int i = 0; i < 16; i++) s += abs(c2[i] - at(prev2, o2 + ivec2(i % 4, i / 4) + pq));
    bool predicted = s / 16.0 + LAMBDA * float(abs(pq.x) + abs(pq.y)) < bestCost;
    if (predicted) best = pq;
    bool atEdge = !predicted && (abs(best.x) >= R || abs(best.y) >= R);

    // Half resolution: 8x8 window from texel block * 4 - 2, +-1, absolute differences.
    ivec2 d = best * 2;
    ivec2 o1 = block * 4 - 2;
    float c1[64];
    for (int i = 0; i < 64; i++) c1[i] = at(cur1, o1 + ivec2(i % 8, i / 8));
    bestCost = 1.0e30;
    ivec2 base = d;
    for (int dy = -1; dy <= 1; dy++) {
        for (int dx = -1; dx <= 1; dx++) {
            ivec2 off = base + ivec2(dx, dy);
            float a = 0.0;
            for (int i = 0; i < 64; i++) a += abs(c1[i] - at(prev1, o1 + ivec2(i % 8, i / 8) + off));
            float cost = a / 64.0 + (0.25 / 255.0) * float(abs(dx) + abs(dy));
            if (cost < bestCost) { bestCost = cost; d = off; }
        }
    }

    // Full resolution: the block itself, +-1, squared differences.
    d *= 2;
    ivec2 o0 = block * 8;
    float c0[64];
    float mean = 0.0;
    for (int i = 0; i < 64; i++) { c0[i] = at(cur0, o0 + ivec2(i % 8, i / 8)); mean += c0[i]; }
    mean /= 64.0;
    float ssd = 1.0e30;
    base = d;
    for (int dy = -1; dy <= 1; dy++) {
        for (int dx = -1; dx <= 1; dx++) {
            ivec2 off = base + ivec2(dx, dy);
            float a = 0.0;
            for (int i = 0; i < 64; i++) {
                float e = c0[i] - at(prev0, o0 + ivec2(i % 8, i / 8) + off);
                a += e * e;
            }
            a /= 64.0;
            if (a < ssd) { ssd = a; d = off; }
        }
    }

    // Sub-pixel: a parabola through the squared costs either side of the best vector.
    float costs[4];
    ivec2 steps[4] = ivec2[4](ivec2(-1, 0), ivec2(1, 0), ivec2(0, -1), ivec2(0, 1));
    for (int k = 0; k < 4; k++) {
        float a = 0.0;
        for (int i = 0; i < 64; i++) {
            float e = c0[i] - at(prev0, o0 + ivec2(i % 8, i / 8) + d + steps[k]);
            a += e * e;
        }
        costs[k] = a / 64.0;
    }
    vec2 v = vec2(d);
    for (int axis = 0; axis < 2; axis++) {
        float cm = costs[axis * 2];
        float cp = costs[axis * 2 + 1];
        float denom = cm - 2.0 * ssd + cp;
        float off = denom > 1.0e-9 ? 0.5 * (cm - cp) / denom : 0.0;
        v[axis] += clamp(off, -0.5, 0.5);
    }

    // Confidence: residual against the block's own contrast.
    float var = 0.0;
    for (int i = 0; i < 64; i++) var += (c0[i] - mean) * (c0[i] - mean);
    float sigma = sqrt(var / 64.0);
    float ratio = sqrt(ssd) / (sigma * 1.41421356 + NOISE_FLOOR);
    float conf = clamp((CONF_HI - ratio) / (CONF_HI - CONF_LO), 0.0, 1.0);
    if (atEdge) conf *= 0.5;
    result = vec4(v, conf, 1.0);
}
