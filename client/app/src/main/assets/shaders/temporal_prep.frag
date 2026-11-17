#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
// Nightjar temporal upscaler, first pass: copies the decoded frame at stream size into a plain
// RGBA8 texture (so later passes can fetch texels) and writes its luma for motion search.
precision highp float;
uniform highp samplerExternalOES source;
uniform mat4 textureTransform;
uniform vec2 renderSize;
layout(location = 0) out vec4 color;
layout(location = 1) out vec4 luma;

void main() {
    vec2 p = gl_FragCoord.xy / renderSize;
    vec3 rgb = texture(source, (textureTransform * vec4(p, 0.0, 1.0)).xy).rgb;
    color = vec4(rgb, 1.0);
    luma = vec4(dot(rgb, vec3(0.299, 0.587, 0.114)), 0.0, 0.0, 1.0);
}
