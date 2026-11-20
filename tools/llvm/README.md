# LLVM Toolchain

`build-static.sh` (Linux and macOS) and `build-static.ps1` (Windows) build the
static LLVM prefix that a release links. The prefix holds static LLVM
libraries and LLD; the release archive bundles its `ld.lld`. Source and build
trees stay under `target/` by default, so LLVM is not part of the repository
or the release archives.

Both scripts check out the `llvmorg-23.1.2` tag from `llvm-project` and reject
an existing source tree at another tag; override `LLVM_RELEASE_TAG` only when
updating the pinned toolchain. They disable optional compression, XML,
terminal, and performance-monitoring libraries, so a static `llvm-sys` link
does not inherit distribution-specific `-lz`, `-lzstd`, or `-lxml2`
requirements.

Select the resulting prefix with `LLVM_SYS_231_PREFIX`. Release builds
consume it through [`tools/release/build.sh`](../release/README.md), which
uses a static-only `llvm-sys` configuration and fails when the prefix does not
provide static archives. Development builds may keep using a distribution
LLVM installation.

## Variables

| Variable | Default | Meaning |
| --- | --- | --- |
| `LLVM_RELEASE_TAG` | `llvmorg-23.1.2` | the LLVM tag to check out |
| `LLVM_REPOSITORY` | `https://github.com/llvm/llvm-project.git` | the repository cloned when the source tree is missing |
| `LLVM_SOURCE_ROOT` | `target/llvm-source` | the LLVM source checkout |
| `LLVM_BUILD_ROOT` | `target/llvm-static/build` | the CMake build tree |
| `LLVM_INSTALL_ROOT` | `target/llvm-static/install` | the installed prefix, which `tools/release/build.sh` uses when `LLVM_SYS_231_PREFIX` is unset |
| `LLVM_ENABLE_PROJECTS` | `lld` | the CMake semicolon list of LLVM projects to build |
| `LLVM_TARGETS_TO_BUILD` | `X86;AArch64` | the LLVM targets (`build-static.sh` only; the PowerShell script always builds both) |
| `LLVM_BUILD_JOBS` | the host CPU count | build parallelism |
| `LLVM_CMAKE_FRESH` | unset | `1` regenerates the CMake cache (`build-static.ps1` only) |

Default paths are relative to the repository root. For example:

```sh
LLVM_TARGETS_TO_BUILD=X86 LLVM_BUILD_JOBS=8 tools/llvm/build-static.sh
```

A validation workflow that needs the matching Clang driver, such as the C ABI
and interoperability tests, may set `LLVM_ENABLE_PROJECTS='clang;lld'`; the
project list is part of that workflow's toolchain identity.

Under WSL, or on any host with limited memory, lower `LLVM_BUILD_JOBS`, or
raise the WSL memory and swap allocation before using a higher value.

## Windows

On Windows, use the PowerShell entry point. It locates Visual Studio and the
Windows SDK itself, configures the static MSVC runtime
(`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`), and builds with Ninja:

```powershell
$env:LLVM_SOURCE_ROOT = 'D:\Downloads\llvm-project'
$env:LLVM_BUILD_ROOT = 'D:\Tools\llvm-build'
$env:LLVM_INSTALL_ROOT = 'D:\Tools\llvm'
$env:LLVM_ENABLE_PROJECTS = 'clang;lld'
.\tools\llvm\build-static.ps1
```

Including Clang in this prefix also provides the compiler required by the C
ABI and interoperability tests. The source root must be a standalone checkout
at the pinned tag. When moving an existing build tree or changing its source
path, set `LLVM_CMAKE_FRESH=1` for the first invocation to regenerate its
CMake cache.

## Release Contents

The release archive contains the compiler, the language server, the `lib`
resources, and the prefix's `ld.lld`, not the LLVM source tree or the rest of
the prefix. LLVM build inputs and the resolved source revision are inputs of
the release build.
