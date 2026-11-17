// PyroWave's YCbCr planes (u_y, u_cb, u_cr, declared by the including shader) to RGB, unclamped;
// shared by planar_csc.frag and temporal_prep.frag. Adapted from joemossjr16/artemis-android-pyrowave
// (GPL-3.0).
vec3 pyroPlanesToRgb(vec2 uv, int tenBit, int hdr, int fullRange) {
    vec2 cuv = uv;
    // The Rubylight host averages each 2x2 chroma block, so its sampling is centred.

    // The host normalizes 10-bit codes by 1023, rather than shifting them into P010.
    float peak = tenBit != 0 ? 1023.0 : 255.0;
    float scale = tenBit != 0 ? 4.0 : 1.0;
    float y = texture(u_y, uv).r;
    vec2 c = vec2(texture(u_cb, cuv).r, texture(u_cr, cuv).r) - (128.0 * scale / peak);
    if (fullRange == 0) {
        y = (y - 16.0 * scale / peak) * peak / (219.0 * scale);
        c *= peak / (224.0 * scale);
    }
    if (hdr != 0) {
        // Inverse BT.2020 non-constant-luminance matrix; RGB remains PQ encoded.
        return vec3(y + 1.4746 * c.y, y - 0.164553 * c.x - 0.571353 * c.y, y + 1.8814 * c.x);
    }
    return vec3(y + 1.5748 * c.y, y - 0.187324 * c.x - 0.468124 * c.y, y + 1.8556 * c.x);
}
