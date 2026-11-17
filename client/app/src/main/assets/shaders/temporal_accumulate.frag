#version 300 es
// Copyright (c) 2024, Qualcomm Innovation Center, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause
// Nightjar temporal upscaler, accumulate pass at output size. Adapted from the upscale pass of
// Snapdragon Game Super Resolution 2 (sgsr2_upscale.fs, commit d926f074; notice in
// ../NOTICE-SGSR2.txt): its 5-tap sample weight, neighbourhood box and history blend. Decoded
// video has no depth or jitter, so jitter is zero and SGSR 2's depth-edge term is replaced by
// 1 - motion confidence. The history holds the plain bilinear picture; the detail pass adds
// SGSR 1's edge detail on the way out. Reference: client/tools/temporal_upscaler/upscalers.py.
precision highp float;
precision highp int;
uniform highp sampler2D frame;     // RGBA8, stream size, linear filtering
uniform highp sampler2D motion;    // RGBA16F per 8x8 block, linear filtering
uniform highp sampler2D cut;       // 1x1 mean confidence
uniform highp sampler2D history;   // RGBA16F, output size, linear filtering
uniform vec2 renderSize;
uniform vec2 outputSize;
uniform float reset;               // 1 on the first frame after (re)initialisation
uniform float boxStatic;           // SGSR 2 scaleRatio.y: min(20, (area ratio)^3)
out vec4 result;

const float ALPHA_SCALE = 10.0;
const float CUT_THRESHOLD = 0.25;

float fastLanczos(float base) {
    float y = base - 1.0;
    float y2 = y * y;
    return (0.75 * y + y2) * y2;
}

vec3 catmullRom(vec2 pos) {
    // Nine bilinear fetches; exact, as the two middle weights of each axis share a sign.
    vec2 texel = pos - 0.5;
    vec2 i0 = floor(texel);
    vec2 t = texel - i0;
    vec2 t2 = t * t;
    vec2 t3 = t2 * t;
    vec2 w0 = -0.5 * t3 + t2 - 0.5 * t;
    vec2 w1 = 1.5 * t3 - 2.5 * t2 + 1.0;
    vec2 w2 = -1.5 * t3 + 2.0 * t2 + 0.5 * t;
    vec2 w3 = 0.5 * t3 - 0.5 * t2;
    vec2 w12 = w1 + w2;
    vec2 p0 = (i0 - 0.5) / outputSize;
    vec2 p12 = (i0 + 0.5 + w2 / w12) / outputSize;
    vec2 p3 = (i0 + 2.5) / outputSize;
    vec3 c = vec3(0.0);
    c += (texture(history, vec2(p0.x, p0.y)).rgb * w0.x + texture(history, vec2(p12.x, p0.y)).rgb * w12.x +
          texture(history, vec2(p3.x, p0.y)).rgb * w3.x) * w0.y;
    c += (texture(history, vec2(p0.x, p12.y)).rgb * w0.x + texture(history, vec2(p12.x, p12.y)).rgb * w12.x +
          texture(history, vec2(p3.x, p12.y)).rgb * w3.x) * w12.y;
    c += (texture(history, vec2(p0.x, p3.y)).rgb * w0.x + texture(history, vec2(p12.x, p3.y)).rgb * w12.x +
          texture(history, vec2(p3.x, p3.y)).rgb * w3.x) * w3.y;
    return c;
}

void main() {
    vec2 scale = outputSize / renderSize;
    vec2 p = gl_FragCoord.xy / scale;               // stream texel coordinates
    ivec2 ip = ivec2(floor(p));
    vec2 off = floor(p) + 0.5 - p;

    vec4 mc = texture(motion, p / (8.0 * vec2(textureSize(motion, 0))));
    vec2 mv = mc.xy;
    float depthfactor = 1.0 - mc.z;
    float motionLen = 2.0 * length(mv) * scale.x;   // SGSR 2's NDC length in output pixels

    float biasmax = scale.x;
    float biasmin = max(1.0, 0.3 + 0.3 * biasmax);
    float kernelbias = mix(biasmax, biasmin, 0.25 * depthfactor) * 0.5;
    float kb2 = kernelbias * kernelbias;
    float curvebias = -2.0 - clamp(motionLen * 0.02, 0.0, 1.0);

    ivec2 maxTexel = ivec2(renderSize) - 1;
    float wsum = 0.0;
    vec3 boxCenter = vec3(0.0);
    vec3 boxVar = vec3(0.0);
    float boxWeight = 0.0;
    vec3 boxMin = vec3(1.0e30);
    vec3 boxMax = vec3(-1.0e30);
    ivec2 taps[5] = ivec2[5](ivec2(0, 1), ivec2(1, 0), ivec2(-1, 0), ivec2(0, 0), ivec2(0, -1));
    for (int k = 0; k < 5; k++) {
        vec3 s = texelFetch(frame, clamp(ip + taps[k], ivec2(0), maxTexel), 0).rgb;
        vec2 o = off + vec2(taps[k]);
        float d2 = dot(o, o);
        wsum += fastLanczos(clamp(d2 * kb2, 0.0, 1.0));
        float bw = exp(d2 * curvebias);
        boxCenter += s * bw;
        boxVar += s * s * bw;
        boxWeight += bw;
        boxMin = min(boxMin, s);
        boxMax = max(boxMax, s);
    }
    boxCenter /= boxWeight;
    boxVar = sqrt(abs(boxVar / boxWeight - boxCenter * boxCenter));

    vec3 current = texture(frame, p / renderSize).rgb;
    float weight = wsum * (1.0 / 3.0) * ALPHA_SCALE;

    vec2 h = (p + mv) * scale;
    bool inside = h.x >= 0.0 && h.x <= outputSize.x && h.y >= 0.0 && h.y <= outputSize.y;
    if (reset > 0.5 || texelFetch(cut, ivec2(0), 0).r < CUT_THRESHOLD || !inside) {
        result = vec4(current, 1.0);
        return;
    }
    vec3 hist = catmullRom(h);

    float baseupdate = 1.0 - depthfactor;
    baseupdate = min(baseupdate, mix(baseupdate, weight * 10.0, clamp(10.0 * motionLen, 0.0, 1.0)));
    baseupdate = min(baseupdate, mix(baseupdate, weight, clamp(motionLen * 0.05, 0.0, 1.0)));
    float basealpha = baseupdate;

    float boxscale = max(depthfactor, clamp(motionLen * 0.05, 0.0, 1.0));
    float boxsize = mix(boxStatic, 1.0, boxscale);
    vec3 sbox = boxVar * boxsize;
    vec3 rmax = min(boxMax, boxCenter + sbox);
    vec3 rmin = max(boxMin, boxCenter - sbox);
    bool outside = any(greaterThan(rmin, hist)) || any(greaterThan(hist, rmax));
    hist = clamp(hist, rmin, rmax);  // minLerpContribution is 0: outside history is clamped
    if (outside) basealpha = min(basealpha, 0.1);

    float alpha = clamp(weight / max(1.192e-7, basealpha + weight), 0.0, 1.0);
    result = vec4(mix(hist, current, alpha), 1.0);
}
