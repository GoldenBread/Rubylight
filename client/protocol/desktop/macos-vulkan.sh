#!/usr/bin/env bash
# Build the Vulkan pieces PyroWave's GPU decoder uses on Apple silicon Macs: Mesa's KosmicKrisp
# driver (Vulkan on Metal 4, macOS 26 or later) and the Khronos Vulkan loader. The last stdout line
# is their prefix:
#
#   macos-vulkan.sh <absolute build cache>
#
# <prefix>/lib holds libvulkan.1.dylib (the loader, built for macOS 13 like the rest of the app, so
# an older Mac loads it and finds no driver) and libvulkan_kosmickrisp.dylib (macOS 26). Needs
# Xcode's command line tools and Homebrew's meson, ninja, cmake, pkg-config, bison, flex, llvm,
# libclc, spirv-llvm-translator and spirv-tools; the Python modules come from a venv made here.
# Apple silicon only: KosmicKrisp needs Metal 4, which Intel Macs do not have.
set -euo pipefail
cache=${1:?usage: macos-vulkan.sh <absolute build cache>}
[[ $(uname -m) == arm64 ]] || { echo "KosmicKrisp builds for Apple silicon only" >&2; exit 1; }
stamp=$(shasum -a 256 "$0" | cut -c1-12)
work="$cache/arm64-$stamp"
prefix="$work/prefix"
if [[ -f "$prefix/.complete" ]]; then
  echo "$prefix"
  exit 0
fi
jobs=${JOBS:-$(getconf _NPROCESSORS_ONLN)}
for tool in cmake meson ninja pkg-config; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 1; }
done
brew=$(brew --prefix)
mkdir -p "$cache/downloads" "$work" "$prefix/lib" "$prefix/licenses"
fetch() {
  local name=$1 url=$2 sha=$3 file
  file="$cache/downloads/$name-${url##*/}"
  if ! echo "$sha  $file" | shasum -a 256 --check --status 2>/dev/null; then
    curl -fLsS --retry 4 --retry-all-errors --retry-delay 3 "$url" -o "$file.part.$$"
    echo "$sha  $file.part.$$" | shasum -a 256 --check --status
    mv "$file.part.$$" "$file"
  fi
  rm -rf "$work/$name"
  mkdir -p "$work/$name"
  tar -xf "$file" -C "$work/$name" --strip-components=1
}
fetch mesa https://archive.mesa3d.org/mesa-26.2.7.tar.xz 04294f75d22e9f61cd1e050206bc48b3d39366e788f1803a3368e3d5a872ce18
fetch headers https://github.com/KhronosGroup/Vulkan-Headers/archive/refs/tags/vulkan-sdk-1.4.357.0.tar.gz e87dce08116151f6b6d7de6b6faf41498e87e6cf848ff16fa3bd5402190ad4a3
fetch loader https://github.com/KhronosGroup/Vulkan-Loader/archive/refs/tags/vulkan-sdk-1.4.357.0.tar.gz 54f2537df22313768da0317dda2abdaaab7711b4081c48c869a79db343d0ae70

# The loader, for the app's own minimum macOS: it only opens the driver the bundle names.
cmake -S "$work/headers" -B "$work/headers-build" -G Ninja -DCMAKE_INSTALL_PREFIX="$work/headers-install"
cmake --install "$work/headers-build"
cmake -S "$work/loader" -B "$work/loader-build" -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_ARCHITECTURES=arm64 -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 \
  -DVULKAN_HEADERS_INSTALL_DIR="$work/headers-install" -DCMAKE_PREFIX_PATH="$work/headers-install" \
  -DBUILD_TESTS=OFF -DBUILD_WERROR=OFF -DCMAKE_INSTALL_PREFIX="$work/loader-install" -DCMAKE_INSTALL_LIBDIR=lib
cmake --build "$work/loader-build" --parallel "$jobs"
cmake --install "$work/loader-build"
cp "$(cd "$work/loader-install/lib" && pwd -P)/$(readlink "$work/loader-install/lib/libvulkan.1.dylib" || echo libvulkan.1.dylib)" "$prefix/lib/libvulkan.1.dylib"
install_name_tool -id @rpath/libvulkan.1.dylib "$prefix/lib/libvulkan.1.dylib"

# SPIRV-Tools, static, so the driver carries it instead of linking Homebrew's dylib.
fetch spirv-headers https://github.com/KhronosGroup/SPIRV-Headers/archive/refs/tags/vulkan-sdk-1.4.357.0.tar.gz 4d703067a7e06331ccb37bdfed3f9b7879cc61969a2689ae95c95db34a47ff07
fetch spirv-tools https://github.com/KhronosGroup/SPIRV-Tools/archive/refs/tags/vulkan-sdk-1.4.357.0.tar.gz d31e7109b6ef3559067e53e520870eafed7c9534d00db9728814b6df03fa4a5e
cmake -S "$work/spirv-tools" -B "$work/spirv-tools-build" -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_ARCHITECTURES=arm64 -DCMAKE_OSX_DEPLOYMENT_TARGET=26.0 -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DSPIRV-Headers_SOURCE_DIR="$work/spirv-headers" -DSPIRV_SKIP_TESTS=ON -DSPIRV_SKIP_EXECUTABLES=ON \
  -DSPIRV_WERROR=OFF -DBUILD_SHARED_LIBS=OFF -DCMAKE_INSTALL_PREFIX="$work/spirv-install" -DCMAKE_INSTALL_LIBDIR=lib
cmake --build "$work/spirv-tools-build" --parallel "$jobs"
cmake --install "$work/spirv-tools-build"

# KosmicKrisp, as docs.mesa3d.org/drivers/kosmickrisp.html builds it, for Metal 4 (macOS 26).
python3 -m venv "$work/venv"
"$work/venv/bin/pip" install --quiet mako packaging pyyaml
export PATH="$work/venv/bin:$brew/opt/llvm/bin:$brew/opt/bison/bin:$brew/opt/flex/bin:$PATH"
export PKG_CONFIG_PATH="$work/spirv-install/lib/pkgconfig:$brew/opt/llvm/lib/pkgconfig:$brew/opt/libclc/share/pkgconfig:$brew/opt/spirv-llvm-translator/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
(
  export MACOSX_DEPLOYMENT_TARGET=26.0
  meson setup "$work/mesa-build" "$work/mesa" --prefix "$work/mesa-install" --libdir lib \
    --buildtype release -Db_ndebug=true -Dstrip=true --prefer-static \
    -Dplatforms=macos -Dvulkan-drivers=kosmickrisp -Dgallium-drivers= -Dopengl=false -Dzstd=disabled
  meson compile -C "$work/mesa-build" -j "$jobs"
  meson install -C "$work/mesa-build"
)
driver=$(find "$work/mesa-install/lib" -name 'libvulkan_kosmickrisp*.dylib' -type f | head -n1)
[[ -n $driver ]] || { echo "the Mesa build installed no KosmicKrisp driver" >&2; find "$work/mesa-install" >&2; exit 1; }
cp "$driver" "$prefix/lib/libvulkan_kosmickrisp.dylib"
install_name_tool -id @rpath/libvulkan_kosmickrisp.dylib "$prefix/lib/libvulkan_kosmickrisp.dylib"

# Both must stand alone in the app: nothing from Homebrew or the build tree.
for lib in "$prefix"/lib/*.dylib; do
  echo "$lib:"
  otool -L "$lib" | tail -n +2
  vtool -show-build "$lib" | grep -E 'minos|platform' || true
  if otool -L "$lib" | tail -n +3 | grep -vqE '^[[:space:]]+(/usr/lib/|/System/Library/)'; then
    echo "error: $lib links a library outside the system" >&2
    exit 1
  fi
done

# The license texts the app carries (client/docs/index.md): Mesa's licenses/, which its
# docs/license.rst points at instead of holding the MIT text, the KosmicKrisp copyright holders
# that licenses/MIT leaves out, and the two SPIR-V projects statically inside the driver. The otool
# check above leaves nothing linked outside the system, so nothing else from Homebrew (LLVM, libclc,
# SPIRV-LLVM-Translator, which KosmicKrisp's meson build does not depend on) is in the driver.
rm -rf "$prefix/licenses"
mkdir -p "$prefix/licenses/SPIRV-Tools" "$prefix/licenses/SPIRV-Headers" "$prefix/licenses/Vulkan-Loader"
cp -R "$work/mesa/licenses" "$prefix/licenses/Mesa"
cp "$work/mesa/docs/license.rst" "$prefix/licenses/Mesa/license.rst"
sed -n '1,/^ \*\//p' "$work/mesa/src/kosmickrisp/vulkan/kk_device.c" > "$prefix/licenses/Mesa/KosmicKrisp-copyright.txt"
cp "$work/spirv-tools/LICENSE" "$prefix/licenses/SPIRV-Tools/LICENSE"
cp "$work/spirv-headers/LICENSE" "$prefix/licenses/SPIRV-Headers/LICENSE"
cp "$work/loader/LICENSE.txt" "$prefix/licenses/Vulkan-Loader/LICENSE.txt"
{
  echo "mesa=26.2.7 (KosmicKrisp)"
  echo "vulkan_loader=vulkan-sdk-1.4.357.0"
  echo "spirv_tools=vulkan-sdk-1.4.357.0"
  echo "spirv_headers=vulkan-sdk-1.4.357.0"
  echo "llvm=$(llvm-config --version)"
  echo "xcode=$(xcodebuild -version | head -n1)"
} > "$prefix/build-info.txt"
touch "$prefix/.complete"
echo "$prefix"
