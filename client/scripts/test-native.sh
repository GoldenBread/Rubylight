#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
core=app/src/main/jni/moonlight-core
tests=app/src/test/native
out=app/build/native-tests
mkdir -p "$out"

flags=(-std=gnu11 -O2 -ffunction-sections -fdata-sections -I "$core" -Wl,--gc-sections)
sanitize=(-fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer)

for mode in plain sanitized; do
    extra=()
    if [ "$mode" = sanitized ]; then extra=("${sanitize[@]}"); fi
    for test in pyrowave_schedule pyrowave_present_timing pyrowave_presentation pyrowave_device_path pyrowave_image_hold pyrowave_present_poll vulkan_output_format vulkan_decoder_fence vulkan_hdr10plus vulkan_ycbcr_components vulkan_present_scheduler vulkan_frame_pacer window_dataspace frame_warp_mask; do
        c++ -std=c++17 -O2 -Wall -Wextra -Werror -pedantic "${extra[@]}" \
            "$tests/${test}_test.cpp" -o "$out/$test-$mode"
        "$out/$test-$mode"
        echo "$test ($mode): passed"
    done
    for test in aaudio_queue dualsense_usb_audio evdev_ipc evdev_rejected; do
        cc "${flags[@]}" "${extra[@]}" -fno-builtin "$tests/${test}_test.c" -o "$out/$test-$mode"
        "$out/$test-$mode"
        echo "$test ($mode): passed"
    done
    cc "${flags[@]}" "${extra[@]}" -I "$tests/fake_jni" -I "$tests/fake_aaudio" \
        "$tests/aaudio_output_test.c" -ldl -lpthread -o "$out/aaudio_output-$mode"
    "$out/aaudio_output-$mode"
    echo "aaudio_output ($mode): passed"
done

cargo test --locked --manifest-path protocol/Cargo.toml --workspace --exclude nightjar-desktop --exclude nightjar-desktop-media
cargo build --locked --manifest-path protocol/Cargo.toml -p rubylight-protocol-ffi --release
c++ -std=c++17 -O2 "${sanitize[@]}" "$tests/pyrowave_frame_test.cpp" \
    protocol/target/release/librubylight_protocol.a -o "$out/pyrowave_frame"
"$out/pyrowave_frame"
bash protocol/tests/differential.sh
