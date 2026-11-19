#!/usr/bin/env bash
set -euo pipefail

# Build the release compiler against the pinned, static LLVM prefix and reject
# a release artifact that accidentally carries a shared LLVM dependency.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
llvm_prefix="${LLVM_SYS_231_PREFIX:-${repo_root}/target/llvm-static/install}"
binary="${repo_root}/target/release/nia"
server="${repo_root}/target/release/nia-lsp"

llvm_config="${llvm_prefix}/bin/llvm-config"
lld_name=ld.lld
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) lld_name=ld.lld.exe ;;
esac
lld="${llvm_prefix}/bin/${lld_name}"
if [[ ! -x "${llvm_config}" ]]; then
    printf 'static LLVM prefix is missing: %s\n' "${llvm_prefix}" >&2
    printf 'run tools/llvm/build-static.sh first or set LLVM_SYS_231_PREFIX\n' >&2
    exit 1
fi
if [[ ! -x "${lld}" ]]; then
    printf 'static LLVM prefix is missing %s: %s\n' "${lld_name}" "${lld}" >&2
    exit 1
fi

version="$("${llvm_config}" --version)"
if [[ "${version}" != 23.1.2 ]]; then
    printf 'expected LLVM 23.1.2, found %s\n' "${version}" >&2
    exit 1
fi
if [[ "$("${llvm_config}" --link-static --shared-mode)" != static ]]; then
    printf 'release LLVM prefix is not static: %s\n' "${llvm_prefix}" >&2
    exit 1
fi

cd "${repo_root}"
LLVM_SYS_231_PREFIX="${llvm_prefix}" \
    cargo build --locked --release -p nia-cli -p nia-lsp --no-default-features \
        --features nia-cli/llvm-static,nia-lsp/llvm-static

for executable in "${binary}" "${server}"; do
    if [[ ! -x "${executable}" ]]; then
        printf 'release executable was not produced: %s\n' "${executable}" >&2
        exit 1
    fi
    case "$(uname -s)" in
        Darwin) dependencies="$(otool -L "${executable}")" ;;
        *) dependencies="$(readelf -d "${executable}")" ;;
    esac
    if grep -Eqi 'libLLVM|liblld' <<<"${dependencies}"; then
        printf 'release executable has a shared LLVM/LLD dependency: %s\n' "${executable}" >&2
        exit 1
    fi
done

printf 'release compiler: %s\n' "${binary}"
printf 'release language server: %s\n' "${server}"
printf 'LLVM linkage: static (%s)\n' "${llvm_prefix}"
printf 'bundled linker candidate: %s\n' "${lld}"
