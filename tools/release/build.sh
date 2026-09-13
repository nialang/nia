#!/usr/bin/env bash
set -euo pipefail

# Build the release compiler against the pinned, static LLVM prefix and reject
# a release artifact that accidentally carries a shared LLVM dependency.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
llvm_prefix="${LLVM_SYS_221_PREFIX:-${repo_root}/target/llvm-static/install}"
binary="${repo_root}/target/release/nia"

llvm_config="${llvm_prefix}/bin/llvm-config"
lld="${llvm_prefix}/bin/ld.lld"
if [[ ! -x "${llvm_config}" ]]; then
    printf 'static LLVM prefix is missing: %s\n' "${llvm_prefix}" >&2
    printf 'run tools/llvm/build-static.sh first or set LLVM_SYS_221_PREFIX\n' >&2
    exit 1
fi
if [[ ! -x "${lld}" ]]; then
    printf 'static LLVM prefix is missing ld.lld: %s\n' "${lld}" >&2
    exit 1
fi

version="$("${llvm_config}" --version)"
if [[ "${version}" != 22.1.0 ]]; then
    printf 'expected LLVM 22.1.0, found %s\n' "${version}" >&2
    exit 1
fi
if [[ "$("${llvm_config}" --shared-mode)" != static ]]; then
    printf 'release LLVM prefix is not static: %s\n' "${llvm_prefix}" >&2
    exit 1
fi

cd "${repo_root}"
LLVM_SYS_221_PREFIX="${llvm_prefix}" \
    cargo build --release -p nia-cli --no-default-features --features llvm-static

if [[ ! -x "${binary}" ]]; then
    printf 'release compiler was not produced: %s\n' "${binary}" >&2
    exit 1
fi
if readelf -d "${binary}" 2>/dev/null | grep -Eqi 'libLLVM|liblld'; then
    printf 'release compiler has a shared LLVM/LLD dependency\n' >&2
    exit 1
fi

package_cache="${repo_root}/target/release-package-cache"
"${binary}" emit --package "${repo_root}/lib/std/pkg.nia" --std \
    --debug -O0 --resource-root "${repo_root}/lib" --cache-dir "${package_cache}"
"${binary}" emit --package "${repo_root}/lib/std/pkg.nia" --std \
    --release -O2 --resource-root "${repo_root}/lib" --cache-dir "${package_cache}"

for context in debug release; do
    mapfile -t std_artifacts < <(find "${repo_root}/lib/std/.nia-cache/packages" \
        -type f -path "*/${context}/normal/package.niapkg" -size +0c -print | sort)
    if [[ "${#std_artifacts[@]}" -eq 0 ]]; then
        printf 'missing %s/normal standard-library package artifact\n' "${context}" >&2
        exit 1
    fi
    printf 'standard-library artifact: %s\n' "${std_artifacts[-1]}"
done

printf 'release compiler: %s\n' "${binary}"
printf 'LLVM linkage: static (%s)\n' "${llvm_prefix}"
printf 'bundled linker candidate: %s\n' "${lld}"
