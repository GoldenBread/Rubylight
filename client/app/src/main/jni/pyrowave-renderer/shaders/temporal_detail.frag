#version 450
#extension GL_GOOGLE_include_directive : require
#include "rotation.glsl"
// Copyright (c) 2025, Qualcomm Innovation Center, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause
// Nightjar temporal upscaler, last pass; Vulkan port of assets/shaders/temporal_detail.frag, same
// math: the accumulated history plus SGSR 1's edge detail of the current frame (sgsr.frag's luma
// delta; license and upstream revision: assets/NOTICE-SGSR1.txt). The detail is never fed back
// into history. Rotation and viewport are handled as in sgsr.frag, so the picture lands exactly
// where SGSR 1's does.
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 color;
layout(set = 0, binding = 0) uniform sampler2D frame;     // RGBA8, stream size, linear filtering
layout(set = 0, binding = 1) uniform sampler2D history;   // RGBA16F, output size
layout(push_constant) uniform Params {
    vec4 viewportInfo;             // 1 / stream size, stream size
    vec2 outputSize;
    float edgeSharpness;
    int rotationQuarterTurns;
} params;

vec3 loadColor(vec2 p) {
    p = clamp(p, 0.5 * params.viewportInfo.xy, vec2(1.0) - 0.5 * params.viewportInfo.xy);
    return texture(frame, p).rgb;
}

float loadGreen(vec2 pixel) {
    return loadColor((pixel + 0.5) * params.viewportInfo.xy).g;
}

float fastLanczos2(float x) {
    float wA = x - 4.0;
    float wB = x * wA - wA;
    wA *= wA;
    return wB * wA;
}

vec2 weightY(float dx, float dy, float c, float std) {
    float x = (dx * dx + dy * dy) * 0.55 + clamp(abs(c) * std, 0.0, 1.0);
    float w = fastLanczos2(x);
    return vec2(w, w * c);
}

void main() {
    // As sgsr.frag: the output UV back to the unrotated picture, then into the GLES port's
    // bottom-left image space, the order frame and history hold their rows in.
    vec2 sourceUv = pyroSourceUv(v_uv, params.rotationQuarterTurns);
    vec2 uv = vec2(sourceUv.x, 1.0 - sourceUv.y);
    ivec2 out_texel = clamp(ivec2(uv * params.outputSize), ivec2(0), ivec2(params.outputSize) - 1);
    vec3 base = texelFetch(history, out_texel, 0).rgb;
    vec3 rgb = loadColor(uv);
    vec2 imgCoord = uv * params.viewportInfo.zw + vec2(-0.5, 0.5);
    vec2 pixel = floor(imgCoord);
    vec2 pl = imgCoord - pixel;

    vec4 left = vec4(loadGreen(pixel + vec2(-1.0, 0.0)), loadGreen(pixel),
                     loadGreen(pixel + vec2(0.0, -1.0)), loadGreen(pixel + vec2(-1.0, -1.0)));
    float edgeVote = abs(left.z - left.y) + abs(rgb.g - left.y) + abs(rgb.g - left.z);
    vec3 detail = vec3(0.0);
    if (edgeVote > 8.0 / 255.0) {
        vec4 right = vec4(loadGreen(pixel + vec2(1.0, 0.0)), loadGreen(pixel + vec2(2.0, 0.0)),
                         loadGreen(pixel + vec2(2.0, -1.0)), loadGreen(pixel + vec2(1.0, -1.0)));
        vec4 upDown = vec4(loadGreen(pixel + vec2(0.0, -2.0)), loadGreen(pixel + vec2(1.0, -2.0)),
                          loadGreen(pixel + vec2(1.0, 1.0)), loadGreen(pixel + vec2(0.0, 1.0)));
        float mean = (left.y + left.z + right.x + right.w) * 0.25;
        left -= mean;
        right -= mean;
        upDown -= mean;
        float center = rgb.g - mean;
        float sum = dot(abs(left) + abs(right) + abs(upDown), vec4(1.0));
        float std = 2.181818 / max(sum, 1.0e-8);

        vec2 aWY = weightY(pl.x, pl.y + 1.0, upDown.x, std);
        aWY += weightY(pl.x - 1.0, pl.y + 1.0, upDown.y, std);
        aWY += weightY(pl.x - 1.0, pl.y - 2.0, upDown.z, std);
        aWY += weightY(pl.x, pl.y - 2.0, upDown.w, std);
        aWY += weightY(pl.x + 1.0, pl.y - 1.0, left.x, std);
        aWY += weightY(pl.x, pl.y - 1.0, left.y, std);
        aWY += weightY(pl.x, pl.y, left.z, std);
        aWY += weightY(pl.x + 1.0, pl.y, left.w, std);
        aWY += weightY(pl.x - 1.0, pl.y - 1.0, right.x, std);
        aWY += weightY(pl.x - 2.0, pl.y - 1.0, right.y, std);
        aWY += weightY(pl.x - 2.0, pl.y, right.z, std);
        aWY += weightY(pl.x - 1.0, pl.y, right.w, std);

        float finalY = aWY.y / max(aWY.x, 1.0e-8);
        float maxY = max(max(left.y, left.z), max(right.x, right.w));
        float minY = min(min(left.y, left.z), min(right.x, right.w));
        finalY = clamp(params.edgeSharpness * finalY, minY, maxY);
        float deltaY = clamp(finalY - center, -23.0 / 255.0, 23.0 / 255.0);
        detail = clamp(rgb + deltaY, 0.0, 1.0) - rgb;
    }
    color = vec4(clamp(base + detail, 0.0, 1.0), 1.0);
}
