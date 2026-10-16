# Nia LLVM Toolchain

`build-static.sh` builds the LLVM prefix used by a hermetic Nia release. The
script keeps the source and build trees under `target/` by default, so LLVM is
not part of the Nia repository or its release archives.

The default source is the `llvmorg-23.1.2` tag from `llvm-project`. Override
`LLVM_RELEASE_TAG` only when updating the pinned toolchain. The build disables
optional compression, XML, terminal, and performance-monitoring libraries so a
static `llvm-sys` link does not inherit distro-specific `-lz`, `-lzstd`, or
`-lxml2` requirements.

The resulting prefix is selected by `LLVM_SYS_231_PREFIX`. A release build
must use a static-only `llvm-sys` configuration and fail when the prefix does
not provide static archives. Development builds may continue using the distro
LLVM installation; release builds consume this prefix through
`tools/release/build.sh`.

Useful overrides:

```sh
LLVM_TARGETS_TO_BUILD=X86 LLVM_BUILD_JOBS=8 tools/llvm/build-static.sh
```

The default build includes LLD. A validation workflow that needs the matching
Clang driver may set `LLVM_ENABLE_PROJECTS='clang;lld'`; the project list is a
CMake semicolon list and participates in that workflow's toolchain identity.

When `LLVM_BUILD_JOBS` is omitted, the script uses the full host CPU count.
WSL users should lower it when the VM has limited memory; an explicit value can
be set after increasing the WSL memory and swap allocation.

The release package contains the Nia compiler and its `lib` resources, not
this LLVM source tree. LLVM build inputs and the resolved source revision are
release-build inputs and should be recorded in the release evidence.

On Windows, use the PowerShell build entry point. It pins the source tree to
the same LLVM release, configures the MSVC static runtime, and disables
optional libraries that are not needed by Nia:

    $env:LLVM_SOURCE_ROOT = 'D:\Downloads\llvm-project'
    $env:LLVM_BUILD_ROOT = 'D:\Tools\llvm-build'
    $env:LLVM_INSTALL_ROOT = 'D:\Tools\llvm'
    .\tools\llvm\build-static.ps1

The source root is a standalone checkout at the `llvmorg-23.1.2` tag; the LLVM
main branch is not used by this workspace. When moving an existing build tree
or changing its source path, set `LLVM_CMAKE_FRESH=1` for the first invocation
to regenerate its CMake cache.
