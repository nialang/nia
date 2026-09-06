#!/usr/bin/env bash
set -euo pipefail

# Build the LLVM prefix used by a hermetic Nia release build. The source tree
# is fetched by tag and kept outside the repository; only the installed prefix
# is consumed by llvm-sys.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
llvm_repository="${LLVM_REPOSITORY:-https://github.com/llvm/llvm-project.git}"
llvm_release_tag="${LLVM_RELEASE_TAG:-llvmorg-22.1.0}"
llvm_source_root="${LLVM_SOURCE_ROOT:-${repo_root}/target/llvm-source}"
llvm_build_root="${LLVM_BUILD_ROOT:-${repo_root}/target/llvm-static/build}"
llvm_install_root="${LLVM_INSTALL_ROOT:-${repo_root}/target/llvm-static/install}"
llvm_targets="${LLVM_TARGETS_TO_BUILD:-X86}"
detected_jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)"
default_jobs="${detected_jobs}"
if (( default_jobs > 8 )); then
    default_jobs=8
fi
llvm_jobs="${LLVM_BUILD_JOBS:-${default_jobs}}"

if [[ ! -f "${llvm_source_root}/llvm/CMakeLists.txt" ]]; then
    mkdir -p "$(dirname "${llvm_source_root}")"
    git clone --depth 1 --branch "${llvm_release_tag}" \
        "${llvm_repository}" "${llvm_source_root}"
fi

actual_tag="$(git -C "${llvm_source_root}" describe --tags --exact-match 2>/dev/null || true)"
if [[ "${actual_tag}" != "${llvm_release_tag}" ]]; then
    printf 'LLVM source tree is not checked out at %s (found %s)\n' \
        "${llvm_release_tag}" "${actual_tag:-unknown}" >&2
    exit 1
fi

if command -v ninja >/dev/null 2>&1; then
    generator="Ninja"
else
    generator="Unix Makefiles"
fi

cmake -S "${llvm_source_root}/llvm" -B "${llvm_build_root}" \
    -G "${generator}" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="${llvm_install_root}" \
    -DLLVM_ENABLE_PROJECTS=lld \
    -DLLVM_TARGETS_TO_BUILD="${llvm_targets}" \
    -DLLVM_BUILD_LLVM_DYLIB=OFF \
    -DLLVM_LINK_LLVM_DYLIB=OFF \
    -DLLVM_ENABLE_ZLIB=OFF \
    -DLLVM_ENABLE_ZSTD=OFF \
    -DLLVM_ENABLE_LIBXML2=OFF \
    -DLLVM_ENABLE_LIBEDIT=OFF \
    -DLLVM_ENABLE_TERMINFO=OFF \
    -DLLVM_ENABLE_LIBPFM=OFF \
    -DLLVM_INCLUDE_BENCHMARKS=OFF \
    -DLLVM_INCLUDE_EXAMPLES=OFF \
    -DLLVM_INCLUDE_TESTS=OFF \
    -DLLVM_BUILD_TESTS=OFF \
    -DLLVM_BUILD_BENCHMARKS=OFF \
    -DLLVM_BUILD_EXAMPLES=OFF

cmake --build "${llvm_build_root}" --parallel "${llvm_jobs}"
cmake --install "${llvm_build_root}"

llvm_config="${llvm_install_root}/bin/llvm-config"
test -x "${llvm_config}"
printf 'LLVM prefix: %s\n' "${llvm_install_root}"
printf 'LLVM version: '
"${llvm_config}" --version
printf 'Static system libraries: '
"${llvm_config}" --link-static --system-libs
