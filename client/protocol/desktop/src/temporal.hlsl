// Nightjar temporal upscaler (Upscaling "Temporal (SGSR 2)"), the six fragment passes of
// app/src/main/assets/shaders/temporal_*.frag ported to HLSL with the same math. One pass per
// build: Direct3D 11 prepends `#define TEMPORAL_<PASS>` (stream/temporal.rs); the SDL GPU
// modules are glslang 16.3.0's (MSYS2's) output, one per pass, for example
//   glslangValidator -V -D -e fragment -S frag --auto-sampled-textures --target-env vulkan1.0 -DTEMPORAL_PREP temporal.hlsl -o temporal-prep.spv
// and likewise TEMPORAL_DOWN, _MOTION, _CUT, _ACCUMULATE and _DETAIL into temporal-down.spv,
// temporal-motion.spv, temporal-cut.spv, temporal-accumulate.spv and temporal-detail.spv;
// metal/generate.py translates them for macOS.
// Accumulate: Copyright (c) 2024, Qualcomm Innovation Center, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause. Adapted from the upscale pass of Snapdragon Game Super
// Resolution 2 (sgsr2_upscale.fs, commit d926f074; notice in app/src/main/assets/NOTICE-SGSR2.txt).
// Detail: Copyright (c) 2025, Qualcomm Innovation Center, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause. SGSR 1's luma delta (sgsr.frag; notice in
// app/src/main/assets/NOTICE-SGSR1.txt).
// Pixel positions are SV_Position's, which address texels as the GLSL passes' gl_FragCoord does
// in their framebuffers (row 0 is the texture's first row), so no coordinate is flipped.
// Every texture is fetched by texel (Load) except where the GLSL samples (texture()), which here
// is SampleLevel with the one linear clamp-to-edge sampler; the SDL GPU build binds each texture
// with its own sampler (linear for frame, motion and history, nearest for luma and cut).
#ifdef VULKAN
#define TEXTURE(name, slot) Texture2D<float4> name : register(t##slot, space2)
SamplerState linearClamp : register(s0, space2);
#define UNIFORMS cbuffer Uniforms : register(b0, space3)
#else
#define TEXTURE(name, slot) Texture2D<float4> name : register(t##slot)
SamplerState linearClamp : register(s0);
#define UNIFORMS cbuffer Uniforms : register(b0)
#endif

float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}

int2 sizeOf(Texture2D<float4> t) {
    uint w, h;
    t.GetDimensions(w, h);
    return int2(w, h);
}

#ifdef TEMPORAL_PREP
// First pass: the luma of the decoded frame for motion search (the frame itself is already the
// renderer's RGBA8 copy, which the later passes read).
TEXTURE(source, 0);

float4 fragment(float4 position : SV_Position) : SV_Target {
    float3 rgb = source.Load(int3(int2(position.xy), 0)).rgb;
    return float4(dot(rgb, float3(0.299, 0.587, 0.114)), 0.0, 0.0, 1.0);
}
#endif

#ifdef TEMPORAL_DOWN
// Halves a luma level with a 2x2 box (the next pyramid level).
TEXTURE(level, 0);

float4 fragment(float4 position : SV_Position) : SV_Target {
    int2 p = int2(position.xy) * 2;
    float s = level.Load(int3(p, 0)).r + level.Load(int3(p + int2(1, 0), 0)).r +
              level.Load(int3(p + int2(0, 1), 0)).r + level.Load(int3(p + int2(1, 1), 0)).r;
    return float4(0.25 * s, 0.0, 0.0, 1.0);
}
#endif

#ifdef TEMPORAL_MOTION
// Motion pass: one fragment per 8x8 block of the stream. Hierarchical block matching on the luma
// pyramid (full, 1/2, 1/4): a full search of +-4 texels at quarter resolution over a 4x4 window
// plus last frame's vector of this block, +-1 refinements at half and full resolution over 8x8
// windows, then a parabolic sub-pixel fit. Writes (vx, vy, confidence, 1): prev(p + v) matches
// cur(p), v in stream pixels. Reference: client/tools/temporal_upscaler/upscalers.py.
TEXTURE(cur0, 0);
TEXTURE(cur1, 1);
TEXTURE(cur2, 2);
TEXTURE(prev0, 3);
TEXTURE(prev1, 4);
TEXTURE(prev2, 5);
TEXTURE(prevMotion, 6);

static const int R = 4;
static const float LAMBDA = 0.5 / 255.0;
static const float NOISE_FLOOR = 3.0 / 255.0;
static const float CONF_LO = 0.35;
static const float CONF_HI = 0.8;

float at(Texture2D<float4> t, int2 p) {
    return t.Load(int3(clamp(p, int2(0, 0), sizeOf(t) - 1), 0)).r;
}

// GLSL's roundEven (halves to even), written out: an integer vector of 2 is an exact half here.
float roundHalfEven(float x) {
    float r = floor(x + 0.5);
    if (x + 0.5 == r && fmod(abs(r), 2.0) == 1.0) r -= 1.0;
    return r;
}

float4 fragment(float4 position : SV_Position) : SV_Target {
    int2 block = int2(position.xy);

    // Quarter resolution: 4x4 window from texel block * 2 - 1.
    int2 o2 = block * 2 - 1;
    float c2[16];
    [loop] for (int i = 0; i < 16; i++) c2[i] = at(cur2, o2 + int2(i % 4, i / 4));
    float bestCost = 1.0e30;
    int2 best = int2(0, 0);
    [loop] for (int dy = -R; dy <= R; dy++) {
        [loop] for (int dx = -R; dx <= R; dx++) {
            float s = 0.0;
            [loop] for (int i = 0; i < 16; i++) s += abs(c2[i] - at(prev2, o2 + int2(i % 4, i / 4) + int2(dx, dy)));
            float cost = s / 16.0 + LAMBDA * float(abs(dx) + abs(dy));
            if (cost < bestCost) { bestCost = cost; best = int2(dx, dy); }
        }
    }
    float2 predictor = prevMotion.Load(int3(block, 0)).xy / 4.0;
    int2 pq = int2(roundHalfEven(predictor.x), roundHalfEven(predictor.y));
    float s = 0.0;
    [loop] for (int i = 0; i < 16; i++) s += abs(c2[i] - at(prev2, o2 + int2(i % 4, i / 4) + pq));
    bool predicted = s / 16.0 + LAMBDA * float(abs(pq.x) + abs(pq.y)) < bestCost;
    if (predicted) best = pq;
    bool atEdge = !predicted && (abs(best.x) >= R || abs(best.y) >= R);

    // Half resolution: 8x8 window from texel block * 4 - 2, +-1, absolute differences.
    int2 d = best * 2;
    int2 o1 = block * 4 - 2;
    float c1[64];
    [loop] for (int i = 0; i < 64; i++) c1[i] = at(cur1, o1 + int2(i % 8, i / 8));
    bestCost = 1.0e30;
    int2 base = d;
    [loop] for (int dy = -1; dy <= 1; dy++) {
        [loop] for (int dx = -1; dx <= 1; dx++) {
            int2 off = base + int2(dx, dy);
            float a = 0.0;
            [loop] for (int i = 0; i < 64; i++) a += abs(c1[i] - at(prev1, o1 + int2(i % 8, i / 8) + off));
            float cost = a / 64.0 + (0.25 / 255.0) * float(abs(dx) + abs(dy));
            if (cost < bestCost) { bestCost = cost; d = off; }
        }
    }

    // Full resolution: the block itself, +-1, squared differences.
    d *= 2;
    int2 o0 = block * 8;
    float c0[64];
    float mean = 0.0;
    [loop] for (int i = 0; i < 64; i++) { c0[i] = at(cur0, o0 + int2(i % 8, i / 8)); mean += c0[i]; }
    mean /= 64.0;
    float ssd = 1.0e30;
    base = d;
    [loop] for (int dy = -1; dy <= 1; dy++) {
        [loop] for (int dx = -1; dx <= 1; dx++) {
            int2 off = base + int2(dx, dy);
            float a = 0.0;
            [loop] for (int i = 0; i < 64; i++) {
                float e = c0[i] - at(prev0, o0 + int2(i % 8, i / 8) + off);
                a += e * e;
            }
            a /= 64.0;
            if (a < ssd) { ssd = a; d = off; }
        }
    }

    // Sub-pixel: a parabola through the squared costs either side of the best vector.
    float costs[4];
    const int2 steps[4] = { int2(-1, 0), int2(1, 0), int2(0, -1), int2(0, 1) };
    [loop] for (int k = 0; k < 4; k++) {
        float a = 0.0;
        [loop] for (int i = 0; i < 64; i++) {
            float e = c0[i] - at(prev0, o0 + int2(i % 8, i / 8) + d + steps[k]);
            a += e * e;
        }
        costs[k] = a / 64.0;
    }
    float2 v = float2(d);
    [unroll] for (int axis = 0; axis < 2; axis++) {
        float cm = costs[axis * 2];
        float cp = costs[axis * 2 + 1];
        float denom = cm - 2.0 * ssd + cp;
        float off = denom > 1.0e-9 ? 0.5 * (cm - cp) / denom : 0.0;
        v[axis] += clamp(off, -0.5, 0.5);
    }

    // Confidence: residual against the block's own contrast.
    float var = 0.0;
    [loop] for (int i = 0; i < 64; i++) var += (c0[i] - mean) * (c0[i] - mean);
    float sigma = sqrt(var / 64.0);
    float ratio = sqrt(ssd) / (sigma * 1.41421356 + NOISE_FLOOR);
    float conf = clamp((CONF_HI - ratio) / (CONF_HI - CONF_LO), 0.0, 1.0);
    if (atEdge) conf *= 0.5;
    return float4(v, conf, 1.0);
}
#endif

#ifdef TEMPORAL_CUT
// Scene-cut pass: one fragment averages the motion confidence over a fixed 32x18 grid of blocks.
// The accumulate pass resets history when the mean is under 0.25.
TEXTURE(motion, 0);

float4 fragment(float4 position : SV_Position) : SV_Target {
    int2 size = sizeOf(motion);
    float sum = 0.0;
    [loop] for (int y = 0; y < 18; y++) {
        [loop] for (int x = 0; x < 32; x++) {
            int2 b = int2((float2(x, y) + 0.5) * float2(size) / float2(32.0, 18.0));
            sum += motion.Load(int3(min(b, size - 1), 0)).b;
        }
    }
    return float4(sum / 576.0, 0.0, 0.0, 1.0);
}
#endif

#ifdef TEMPORAL_ACCUMULATE
// Accumulate pass at output size: SGSR 2's 5-tap sample weight, neighbourhood box and history
// blend. Decoded video has no depth or jitter, so jitter is zero and SGSR 2's depth-edge term is
// replaced by 1 - motion confidence. The history holds the plain bilinear picture; the detail
// pass adds SGSR 1's edge detail on the way out.
TEXTURE(frame, 0);     // RGBA8, stream size, linear filtering
TEXTURE(motion, 1);    // RGBA16F per 8x8 block, linear filtering
TEXTURE(cut, 2);       // 1x1 mean confidence
TEXTURE(history, 3);   // RGBA16F, output size, linear filtering
UNIFORMS {
    float2 renderSize;
    float2 outputSize;
    float reset;       // 1 on the first frame after (re)initialisation
    float boxStatic;   // SGSR 2 scaleRatio.y: min(20, (area ratio)^3)
};

static const float ALPHA_SCALE = 10.0;
static const float CUT_THRESHOLD = 0.25;

float fastLanczos(float base) {
    float y = base - 1.0;
    float y2 = y * y;
    return (0.75 * y + y2) * y2;
}

float3 historyAt(float2 uv) {
    return history.SampleLevel(linearClamp, uv, 0).rgb;
}

float3 catmullRom(float2 pos) {
    // Nine bilinear fetches; exact, as the two middle weights of each axis share a sign.
    float2 texel = pos - 0.5;
    float2 i0 = floor(texel);
    float2 t = texel - i0;
    float2 t2 = t * t;
    float2 t3 = t2 * t;
    float2 w0 = -0.5 * t3 + t2 - 0.5 * t;
    float2 w1 = 1.5 * t3 - 2.5 * t2 + 1.0;
    float2 w2 = -1.5 * t3 + 2.0 * t2 + 0.5 * t;
    float2 w3 = 0.5 * t3 - 0.5 * t2;
    float2 w12 = w1 + w2;
    float2 p0 = (i0 - 0.5) / outputSize;
    float2 p12 = (i0 + 0.5 + w2 / w12) / outputSize;
    float2 p3 = (i0 + 2.5) / outputSize;
    float3 c = float3(0.0, 0.0, 0.0);
    c += (historyAt(float2(p0.x, p0.y)) * w0.x + historyAt(float2(p12.x, p0.y)) * w12.x +
          historyAt(float2(p3.x, p0.y)) * w3.x) * w0.y;
    c += (historyAt(float2(p0.x, p12.y)) * w0.x + historyAt(float2(p12.x, p12.y)) * w12.x +
          historyAt(float2(p3.x, p12.y)) * w3.x) * w12.y;
    c += (historyAt(float2(p0.x, p3.y)) * w0.x + historyAt(float2(p12.x, p3.y)) * w12.x +
          historyAt(float2(p3.x, p3.y)) * w3.x) * w3.y;
    return c;
}

float4 fragment(float4 position : SV_Position) : SV_Target {
    float2 scale = outputSize / renderSize;
    float2 p = position.xy / scale;               // stream texel coordinates
    int2 ip = int2(floor(p));
    float2 off = floor(p) + 0.5 - p;

    float4 mc = motion.SampleLevel(linearClamp, p / (8.0 * float2(sizeOf(motion))), 0);
    float2 mv = mc.xy;
    float depthfactor = 1.0 - mc.z;
    float motionLen = 2.0 * length(mv) * scale.x;   // SGSR 2's NDC length in output pixels

    float biasmax = scale.x;
    float biasmin = max(1.0, 0.3 + 0.3 * biasmax);
    float kernelbias = lerp(biasmax, biasmin, 0.25 * depthfactor) * 0.5;
    float kb2 = kernelbias * kernelbias;
    float curvebias = -2.0 - clamp(motionLen * 0.02, 0.0, 1.0);

    int2 maxTexel = int2(renderSize) - 1;
    float wsum = 0.0;
    float3 boxCenter = float3(0.0, 0.0, 0.0);
    float3 boxVar = float3(0.0, 0.0, 0.0);
    float boxWeight = 0.0;
    float3 boxMin = float3(1.0e30, 1.0e30, 1.0e30);
    float3 boxMax = float3(-1.0e30, -1.0e30, -1.0e30);
    const int2 taps[5] = { int2(0, 1), int2(1, 0), int2(-1, 0), int2(0, 0), int2(0, -1) };
    [unroll] for (int k = 0; k < 5; k++) {
        float3 s = frame.Load(int3(clamp(ip + taps[k], int2(0, 0), maxTexel), 0)).rgb;
        float2 o = off + float2(taps[k]);
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

    float3 current = frame.SampleLevel(linearClamp, p / renderSize, 0).rgb;
    float weight = wsum * (1.0 / 3.0) * ALPHA_SCALE;

    float2 h = (p + mv) * scale;
    bool inside = h.x >= 0.0 && h.x <= outputSize.x && h.y >= 0.0 && h.y <= outputSize.y;
    if (reset > 0.5 || cut.Load(int3(0, 0, 0)).r < CUT_THRESHOLD || !inside) {
        return float4(current, 1.0);
    }
    float3 hist = catmullRom(h);

    float baseupdate = 1.0 - depthfactor;
    baseupdate = min(baseupdate, lerp(baseupdate, weight * 10.0, clamp(10.0 * motionLen, 0.0, 1.0)));
    baseupdate = min(baseupdate, lerp(baseupdate, weight, clamp(motionLen * 0.05, 0.0, 1.0)));
    float basealpha = baseupdate;

    float boxscale = max(depthfactor, clamp(motionLen * 0.05, 0.0, 1.0));
    float boxsize = lerp(boxStatic, 1.0, boxscale);
    float3 sbox = boxVar * boxsize;
    float3 rmax = min(boxMax, boxCenter + sbox);
    float3 rmin = max(boxMin, boxCenter - sbox);
    bool outside = any(rmin > hist) || any(hist > rmax);
    hist = clamp(hist, rmin, rmax);  // minLerpContribution is 0: outside history is clamped
    if (outside) basealpha = min(basealpha, 0.1);

    float alpha = clamp(weight / max(1.192e-7, basealpha + weight), 0.0, 1.0);
    return float4(lerp(hist, current, alpha), 1.0);
}
#endif

#ifdef TEMPORAL_DETAIL
// Last pass: the accumulated history plus SGSR 1's edge detail of the current frame. The detail
// is never fed back into history.
TEXTURE(frame, 0);     // RGBA8, stream size, linear filtering
TEXTURE(history, 1);   // RGBA16F, output size
UNIFORMS {
    float4 viewportInfo;   // 1 / stream size, stream size
    float2 outputSize;
    float edgeSharpness;
};

float3 loadColor(float2 p) {
    p = clamp(p, 0.5 * viewportInfo.xy, float2(1.0, 1.0) - 0.5 * viewportInfo.xy);
    return frame.SampleLevel(linearClamp, p, 0).rgb;
}

float loadGreen(float2 pixel) {
    return loadColor((pixel + 0.5) * viewportInfo.xy).g;
}

float fastLanczos2(float x) {
    float wA = x - 4.0;
    float wB = x * wA - wA;
    wA *= wA;
    return wB * wA;
}

float2 weightY(float dx, float dy, float c, float std) {
    float x = (dx * dx + dy * dy) * 0.55 + clamp(abs(c) * std, 0.0, 1.0);
    float w = fastLanczos2(x);
    return float2(w, w * c);
}

float4 fragment(float4 position : SV_Position) : SV_Target {
    float2 uv = position.xy / outputSize;
    int2 out_texel = clamp(int2(uv * outputSize), int2(0, 0), int2(outputSize) - 1);
    float3 base = history.Load(int3(out_texel, 0)).rgb;
    float3 rgb = loadColor(uv);
    float2 imgCoord = uv * viewportInfo.zw + float2(-0.5, 0.5);
    float2 pixel = floor(imgCoord);
    float2 pl = imgCoord - pixel;

    float4 left = float4(loadGreen(pixel + float2(-1.0, 0.0)), loadGreen(pixel),
                         loadGreen(pixel + float2(0.0, -1.0)), loadGreen(pixel + float2(-1.0, -1.0)));
    float edgeVote = abs(left.z - left.y) + abs(rgb.g - left.y) + abs(rgb.g - left.z);
    float3 detail = float3(0.0, 0.0, 0.0);
    if (edgeVote > 8.0 / 255.0) {
        float4 right = float4(loadGreen(pixel + float2(1.0, 0.0)), loadGreen(pixel + float2(2.0, 0.0)),
                              loadGreen(pixel + float2(2.0, -1.0)), loadGreen(pixel + float2(1.0, -1.0)));
        float4 upDown = float4(loadGreen(pixel + float2(0.0, -2.0)), loadGreen(pixel + float2(1.0, -2.0)),
                               loadGreen(pixel + float2(1.0, 1.0)), loadGreen(pixel + float2(0.0, 1.0)));
        float mean = (left.y + left.z + right.x + right.w) * 0.25;
        left -= mean;
        right -= mean;
        upDown -= mean;
        float center = rgb.g - mean;
        float sum = dot(abs(left) + abs(right) + abs(upDown), float4(1.0, 1.0, 1.0, 1.0));
        float std = 2.181818 / max(sum, 1.0e-8);

        float2 aWY = weightY(pl.x, pl.y + 1.0, upDown.x, std);
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
        finalY = clamp(edgeSharpness * finalY, minY, maxY);
        float deltaY = clamp(finalY - center, -23.0 / 255.0, 23.0 / 255.0);
        detail = clamp(rgb + deltaY, 0.0, 1.0) - rgb;
    }
    return float4(clamp(base + detail, 0.0, 1.0), 1.0);
}
#endif
