#!/usr/bin/env bash
set -euo pipefail

# Assemble a relocatable Unix release archive after building the compiler.
# The archive intentionally contains the compiler, Nia resources, and bundled
# linker only; LLVM's build tree remains a release-build input.
repo_root="${NIA_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
repo_root="$(cd "${repo_root}" && pwd)"
output_root="${NIA_RELEASE_OUTPUT:-${repo_root}/target/release-package}"
platform="${NIA_RELEASE_PLATFORM:-linux-x86_64}"

if [[ "${output_root}" != /* ]]; then
    output_root="${repo_root}/${output_root}"
fi
mkdir -p "${output_root}"
output_root="$(cd "${output_root}" && pwd)"

case "${platform}" in
    linux-x86_64)
        expected_os=Linux
        expected_arch=x86_64
        ;;
    linux-aarch64)
        expected_os=Linux
        expected_arch=aarch64
        ;;
    darwin-x86_64)
        expected_os=Darwin
        expected_arch=x86_64
        ;;
    darwin-aarch64)
        expected_os=Darwin
        expected_arch=arm64
        ;;
    *)
        printf 'unsupported release platform: %s\n' "${platform}" >&2
        exit 1
        ;;
esac
# The archive name and bundled linker describe the host build.
if [[ "$(uname -s)" != "${expected_os}" || "$(uname -m)" != "${expected_arch}" ]]; then
    printf '%s archives must be packaged on a %s %s host\n' \
        "${platform}" "${expected_os}" "${expected_arch}" >&2
    exit 1
fi

version="$(cd "${repo_root}" && cargo pkgid -p nia-cli | sed -n 's/.*#//p')"
if [[ -z "${version}" ]]; then
    printf 'failed to determine nia-cli version\n' >&2
    exit 1
fi

"${repo_root}/tools/release/build.sh"

binary="${repo_root}/target/release/nia"
lld_name=ld.lld
lld="${LLVM_SYS_231_PREFIX:-${repo_root}/target/llvm-static/install}/bin/${lld_name}"
resource_root="${repo_root}/lib"
for required in "${binary}" "${lld}" "${resource_root}/toolchain.meta" \
    "${resource_root}/std/pkg.nia" "${resource_root}/runtime/start.nia"; do
    if [[ ! -f "${required}" || ! -r "${required}" ]]; then
        printf 'release input is missing: %s\n' "${required}" >&2
        exit 1
    fi
done
stage_root="${output_root}/nia-${version}-${platform}"
archive="${output_root}/nia-${version}-${platform}.tar.gz"
checksums="${output_root}/SHA256SUMS-${platform}"
rm -rf "${stage_root}" "${archive}" "${checksums}"
mkdir -p "${stage_root}/bin" "${stage_root}/lib" "${stage_root}/libexec"

install -m 0755 "${binary}" "${stage_root}/bin/nia"
cp -a "${resource_root}/." "${stage_root}/lib/"
install -m 0755 "${lld}" "${stage_root}/libexec/${lld_name}"
install -m 0644 "${repo_root}/README.md" "${stage_root}/README.md"
install -m 0644 "${repo_root}/LICENSE.md" "${stage_root}/LICENSE.md"

git_revision="$(git -C "${repo_root}" rev-parse HEAD)"
llvm_version="$("${LLVM_SYS_231_PREFIX:-${repo_root}/target/llvm-static/install}/bin/llvm-config" --version)"
{
    printf 'nia-version=%s\n' "${version}"
    printf 'git-revision=%s\n' "${git_revision}"
    printf 'platform=%s\n' "${platform}"
    printf 'llvm-version=%s\n' "${llvm_version}"
} > "${stage_root}/release-manifest.txt"

if tar --version 2>/dev/null | grep -q 'GNU tar'; then
    tar --sort=name --mtime='UTC 1970-01-01' --owner=0 --group=0 --numeric-owner \
        -czf "${archive}" -C "${output_root}" "$(basename "${stage_root}")"
else
    tar -czf "${archive}" -C "${output_root}" "$(basename "${stage_root}")"
fi
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${archive}" | sed "s#${output_root}/##" > "${checksums}"
else
    shasum -a 256 "${archive}" | sed "s#${output_root}/##" > "${checksums}"
fi

printf 'release archive: %s\n' "${archive}"
printf 'checksums: %s\n' "${checksums}"
