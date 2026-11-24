//! Procedural world, HUD, and barcode HLSL shader and compiler for `look_probe`.

use anyhow::{Context, Result, bail};
use windows::{Win32::Graphics::Direct3D::Fxc::*, core::PCSTR};

const SHADER: &str = r#"
cbuffer Frame : register(b0) {
    float cameraX;      // wrapped into [0, worldWidth)
    float cameraY;      // pixels, down; 0 puts the horizon at the screen's centre
    float worldWidth;   // four screen widths; the world wraps there
    float screenWidth;
    float screenHeight;
    uint hud;
    uint frame;         // the frames.csv frame number, from 1
    uint barcode;
    uint ticksLow;      // QueryPerformanceCounter at render time
    uint ticksHigh;
    float focal;        // the perspective camera's focal length in pixels; 0 slides a flat world
    uint unused1;
};
float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
uint hash(uint x) {
    x ^= x >> 16; x *= 0x7feb352du;
    x ^= x >> 15; x *= 0x846ca68bu;
    x ^= x >> 16;
    return x;
}
float random(uint x, uint y, uint seed) {
    uint h = hash(x * 0x8da6b343u ^ hash(y * 0xd8163841u ^ seed * 0xcb1ab31fu));
    return float(h >> 8) / 16777215.0;
}
// Smooth value noise with cells of about `cell` pixels. A whole number of cells spans the
// world and the x lattice wraps there, so the world wraps without a seam. x is never negative.
float noise(float2 w, float cell, uint seed) {
    uint period = uint(max(1.0, round(worldWidth / cell)));
    float2 offset = float2(random(1, 2, seed), random(3, 4, seed)) * 61.0;
    float2 p = w * (float(period) / worldWidth) + offset;
    float2 i = floor(p);
    float2 f = p - i;
    float2 u = f * f * (3.0 - 2.0 * f);
    uint x0 = uint(i.x) % period;
    uint x1 = (x0 + 1) % period;
    uint y0 = asuint(int(i.y));
    uint y1 = asuint(int(i.y) + 1);
    float a = random(x0, y0, seed), b = random(x1, y0, seed);
    float c = random(x0, y1, seed), d = random(x1, y1, seed);
    return lerp(lerp(a, b, u.x), lerp(c, d, u.x), u.y);
}
float fbm(float2 w, float cell, uint seed, uint octaves) {
    float sum = 0, weight = 0, amplitude = 1;
    [loop] for (uint k = 0; k < octaves; k++) {
        sum += amplitude * noise(w, cell, seed + k * 977u);
        weight += amplitude;
        amplitude *= 0.5;
        cell *= 0.5;
    }
    return sum / weight;
}
float3 sky(float2 w) {
    float up = saturate(-w.y / screenHeight);
    float3 color = lerp(float3(0.72, 0.80, 0.87), float3(0.16, 0.36, 0.70), sqrt(up));
    // Clouds, flatter than wide.
    float cloud = fbm(float2(w.x, w.y * 2.5), 320.0, 11u, 4u);
    float cover = smoothstep(0.46, 0.72, cloud);
    float shade = 0.78 + 0.22 * noise(w, 40.0, 13u);
    color = lerp(color, float3(0.96, 0.96, 0.98) * shade, 0.9 * cover);
    // Faint grain, so open sky still has texture to match.
    return color * (0.94 + 0.12 * noise(w, 5.0, 17u));
}
float3 ground(float2 w) {
    float near = saturate(w.y / screenHeight);
    float patches = fbm(w, 288.0, 23u, 5u);
    float detail = fbm(w, 72.0, 29u, 3u);
    float3 grass = lerp(float3(0.14, 0.27, 0.07), float3(0.36, 0.48, 0.15), detail);
    float3 dirt = lerp(float3(0.28, 0.21, 0.12), float3(0.50, 0.40, 0.25), detail);
    float3 color = lerp(grass, dirt, smoothstep(0.40, 0.64, patches));
    color *= 0.78 + 0.44 * noise(w, 4.0, 31u);
    // Haze toward the horizon.
    return lerp(color, float3(0.66, 0.72, 0.76), 0.55 * (1.0 - smoothstep(0.0, 0.15, near)));
}
// Poles and towers standing just below the horizon, at most one in each slot of about 150
// pixels. A whole number of slots spans the world, so they wrap with it.
bool structure(float2 w, out float3 color) {
    color = 0;
    uint slots = uint(max(1.0, round(worldWidth / 150.0)));
    float slotWidth = worldWidth / float(slots);
    uint slot = min(uint(w.x / slotWidth), slots - 1);
    uint h = hash(slot * 0x9e3779b9u + 0x632be5abu);
    if ((h & 15u) < 7u) return false;
    bool tower = ((h >> 4) & 3u) == 0;
    float r1 = float((h >> 8) & 255u) / 255.0;
    float r2 = float((h >> 16) & 255u) / 255.0;
    float r3 = float(h >> 24) / 255.0;
    float width = tower ? lerp(36.0, 110.0, r1) : lerp(4.0, 16.0, r1);
    float height = screenHeight * (tower ? lerp(0.2, 0.8, r2) : lerp(0.08, 0.55, r2));
    float base = lerp(6.0, 0.1 * screenHeight, r3);
    uint g = hash(h);
    float left = float(slot) * slotWidth + (slotWidth - width) * float(g & 1023u) / 1023.0;
    float x = w.x - left;
    if (x < 0 || x >= width || w.y > base || w.y < base - height) return false;
    float tone = lerp(0.10, 0.50, float((g >> 10) & 255u) / 255.0);
    float3 tint = lerp(float3(1.0, 1.0, 1.0), float3(1.0, 0.90, 0.80), float((g >> 18) & 255u) / 255.0);
    // Lit from the left like a cylinder, weathered so it has texture of its own.
    float across = x / width;
    float light = 1.15 - 0.55 * across * across;
    float wear = 0.82 + 0.36 * noise(w, tower ? 10.0 : 4.0, h);
    color = tone * tint * light * wear;
    return true;
}
bool inside(int2 p, int2 at, int2 size) {
    return all(p >= at) && all(p < at + size);
}
// The crosshair's four arms and centre dot, each grown by `grow` pixels.
bool crosshair(int2 p, int2 c, int k, int grow) {
    int2 g = int2(grow, grow);
    return inside(p, c + int2(-13 * k, -k) - g, int2(10 * k, 2 * k) + 2 * g)
        || inside(p, c + int2(3 * k, -k) - g, int2(10 * k, 2 * k) + 2 * g)
        || inside(p, c + int2(-k, -13 * k) - g, int2(2 * k, 10 * k) + 2 * g)
        || inside(p, c + int2(-k, 3 * k) - g, int2(2 * k, 10 * k) + 2 * g)
        || inside(p, c + int2(-k, -k) - g, int2(2 * k, 2 * k) + 2 * g);
}
static const uint SEGMENTS[10] = { 0x3Fu, 0x06u, 0x5Bu, 0x4Fu, 0x66u, 0x6Du, 0x7Du, 0x07u, 0x7Fu, 0x6Fu };
// A seven-segment digit, 14k wide and 26k tall, at `at`.
bool digit(int2 p, int2 at, uint value, int k) {
    int2 q = p - at;
    int w = 14 * k, h = 26 * k, t = 3 * k, m = 13 * k - (t >> 1);
    uint s = SEGMENTS[value];
    return ((s & 1u) != 0 && inside(q, int2(t, 0), int2(w - 2 * t, t)))
        || ((s & 2u) != 0 && inside(q, int2(w - t, t), int2(t, m - t)))
        || ((s & 4u) != 0 && inside(q, int2(w - t, m + t), int2(t, h - m - 2 * t)))
        || ((s & 8u) != 0 && inside(q, int2(t, h - t), int2(w - 2 * t, t)))
        || ((s & 16u) != 0 && inside(q, int2(0, m + t), int2(t, h - m - 2 * t)))
        || ((s & 32u) != 0 && inside(q, int2(0, t), int2(t, m - t)))
        || ((s & 64u) != 0 && inside(q, int2(t, m), int2(w - 2 * t, t)));
}
// The HUD: fixed in screen space in exact colours, so it stays pixel-identical while the
// camera turns. Sizes are in units of k pixels, 1 per 540 rows.
bool overlay(int2 p, out float3 color) {
    color = 0;
    int width = int(screenWidth), height = int(screenHeight);
    int k = max(1, int(screenHeight / 540.0));
    int margin = 16 * k;
    int2 centre = int2(width >> 1, height >> 1);
    if (crosshair(p, centre, k, 0)) { color = float3(0.20, 1.00, 0.30); return true; }
    if (crosshair(p, centre, k, k)) { color = float3(0.02, 0.02, 0.02); return true; }
    // Health bar, bottom left, 72% full.
    int2 barSize = int2(200 * k, 16 * k);
    int2 barAt = int2(margin, height - margin - barSize.y);
    if (inside(p, barAt, barSize)) {
        int2 q = p - barAt;
        if (!inside(q, int2(2 * k, 2 * k), barSize - 4 * k)) color = float3(0.04, 0.04, 0.04);
        else if (q.x < 2 * k + int(uint((barSize.x - 4 * k) * 72) / 100u)) color = float3(0.86, 0.13, 0.10);
        else color = float3(0.28, 0.06, 0.05);
        return true;
    }
    // Ammo counter, bottom right: 30 | 90.
    int2 ammoSize = int2(112 * k, 44 * k);
    int2 ammoAt = int2(width - margin - ammoSize.x, height - margin - ammoSize.y);
    if (inside(p, ammoAt, ammoSize)) {
        int2 q = p - ammoAt;
        if (!inside(q, int2(2 * k, 2 * k), ammoSize - 4 * k)) color = float3(0.75, 0.75, 0.78);
        else if (digit(q, int2(10 * k, 9 * k), 3, k) || digit(q, int2(28 * k, 9 * k), 0, k)) color = float3(1.00, 0.84, 0.28);
        else if (inside(q, int2(52 * k, 8 * k), int2(2 * k, 28 * k))) color = float3(0.75, 0.75, 0.78);
        else if (digit(q, int2(62 * k, 9 * k), 9, k) || digit(q, int2(80 * k, 9 * k), 0, k)) color = float3(0.80, 0.80, 0.80);
        else color = float3(0.09, 0.10, 0.12);
        return true;
    }
    // Minimap, top right: a static map 170 units on a side.
    int mapSize = 170 * k;
    int2 mapAt = int2(width - margin - mapSize, margin);
    if (inside(p, mapAt, int2(mapSize, mapSize))) {
        int2 q = int2(uint2(p - mapAt) / uint(k));
        if (!inside(q, int2(2, 2), int2(166, 166))) color = float3(0.80, 0.80, 0.82);
        else if (inside(q, int2(80, 80), int2(10, 10))) color = float3(1.00, 1.00, 1.00);
        else if (inside(q, int2(120, 112), int2(48, 56))) color = float3(0.14, 0.30, 0.55);
        else if (inside(q, int2(30, 24), int2(34, 26)) || inside(q, int2(104, 20), int2(28, 44))
            || inside(q, int2(20, 108), int2(26, 40)) || inside(q, int2(78, 128), int2(30, 22)))
            color = float3(0.46, 0.46, 0.50);
        else if (inside(q, int2(2, 92), int2(166, 8)) || inside(q, int2(66, 2), int2(8, 166)))
            color = float3(0.36, 0.33, 0.27);
        else color = float3(0.13, 0.19, 0.12);
        return true;
    }
    return false;
}
// motion_probe's barcode, drawn as it draws it: four rows, sequence, QPC low, QPC high and a
// signature, with black and white references at x=12/36 for the reader.
bool stamp(float4 p, out float4 color) {
    color = float4(0, 0, 0, 1);
    if (p.y < 8 || p.y >= 104) return false;
    if (p.x < 24) return true;
    if (p.x < 48) { color = float4(1.25, 1.25, 1.25, 1); return true; }
    if (p.x >= 64 && p.x < 576) {
        uint row = uint(p.y - 8) / 24;
        uint word = row == 0 ? frame : row == 1 ? ticksLow : row == 2 ? ticksHigh : 0xB17E2212;
        float value = ((word >> (uint(p.x - 64) / 16)) & 1) * 1.25;
        color = float4(value, value, value, 1);
        return true;
    }
    return false;
}
float4 picture(float4 position : SV_Position) : SV_Target {
    float3 color;
    float4 bars;
    if (barcode != 0 && stamp(position, bars)) return bars;
    if (hud != 0 && overlay(int2(position.xy), color)) return float4(color, 1);
    // The world point under this pixel is the screen's plus the camera's, so the content
    // moves against the camera. World y 0 is the horizon.
    float2 w = position.xy + float2(cameraX, cameraY - 0.5 * screenHeight);
    if (focal > 0) {
        // Through a lens: the pixel's ray, pitched down by the camera's y and turned by its x,
        // meets the world on a cylinder around the camera.
        float2 p = position.xy - 0.5 * float2(screenWidth, screenHeight);
        float pitch = cameraY / focal;
        float y = p.y * cos(pitch) + focal * sin(pitch);
        float z = focal * cos(pitch) - p.y * sin(pitch);
        w = float2(cameraX + focal * atan2(p.x, z), focal * y / length(float2(p.x, z)));
        if (w.x < 0) w.x += worldWidth;
    }
    if (w.x >= worldWidth) w.x -= worldWidth;
    if (structure(w, color)) return float4(color, 1);
    if (w.y < 0) return float4(sky(w), 1);
    return float4(ground(w), 1);
}
"#;

pub(crate) unsafe fn compile(entry: &'static [u8], target: &'static [u8]) -> Result<Vec<u8>> {
    unsafe {
        let mut code = None;
        let mut errors = None;
        let result = D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(target.as_ptr()),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        );
        if let Err(error) = result {
            let details = errors
                .map(|blob| {
                    String::from_utf8_lossy(std::slice::from_raw_parts(
                        blob.GetBufferPointer().cast(),
                        blob.GetBufferSize(),
                    ))
                    .into_owned()
                })
                .unwrap_or_default();
            bail!("look shader: {error}: {details}");
        }
        let code = code.context("shader compiler returned no bytecode")?;
        Ok(
            std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize())
                .to_vec(),
        )
    }
}
