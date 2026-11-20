# Release Build

`build.sh` builds the release compiler and language server on Linux and macOS
with the `llvm-static` feature. It requires the pinned LLVM 23.1.2 static
prefix produced by [`tools/llvm/build-static.sh`](../llvm/README.md): it
checks that the prefix reports version 23.1.2, links statically, and contains
`ld.lld`, and it rejects either binary if `readelf` (Linux) or `otool` (macOS)
shows a shared LLVM or LLD dependency. Standard-library sources remain part of
the toolchain resources and are compiled through the normal source pipeline.

Run it from any directory:

```sh
tools/release/build.sh
```

Set `LLVM_SYS_231_PREFIX` when the static prefix is not at its default
location, `target/llvm-static/install`. The release compiler still has normal
host dependencies such as libc and the C++ runtime. Target programs may
additionally require a target sysroot, CRT, and linker runtime; those are
separate from the compiler's LLVM linkage.

## Platform Packages

`package.sh` assembles relocatable `.tar.gz` archives for `linux-x86_64`,
`linux-aarch64`, `darwin-x86_64`, and `darwin-aarch64`. It must run on a host
of the named platform, and it runs `build.sh` first:

```sh
NIA_RELEASE_PLATFORM=darwin-aarch64 tools/release/package.sh
```

`package.ps1` assembles the `windows-x86_64` ZIP archive on a Windows MSVC
host. It does not build: first build the compiler and language server against
the static prefix from an x64 Developer PowerShell (see [Development
Setup](../../docs/dev/setup.md#windows-msvc)). It takes `ld.lld.exe` from
`LLVM_SYS_231_PREFIX`, or from `target/llvm-static/install` when that is unset.

```powershell
cargo build --locked --release -p nia-cli -p nia-lsp --no-default-features --features nia-cli/llvm-static,nia-lsp/llvm-static --target x86_64-pc-windows-msvc
tools\release\package.ps1
```

Both scripts read these variables:

| Variable | Default | Meaning |
| --- | --- | --- |
| `NIA_RELEASE_PLATFORM` | `linux-x86_64`, or `windows-x86_64` for `package.ps1` | the platform name in the archive name |
| `NIA_RELEASE_OUTPUT` | `target/release-package` | the output directory |
| `NIA_REPO_ROOT` | the repository containing the script | the checkout to package |

Each archive is meant to be unpacked as a complete, portable prefix, and is
written next to a `SHA256SUMS-<platform>` file:

```text
nia-<version>-<platform>/
├── README.md
├── LICENSE.md
├── release-manifest.txt   version, Git revision, platform, LLVM version
├── bin/nia                (bin/nia.exe on Windows)
├── bin/nia-lsp            (bin/nia-lsp.exe on Windows)
├── lib/                   toolchain.meta, std/, runtime/, imports/
└── libexec/ld.lld         (libexec/ld.lld.exe on Windows)
```

Keep that layout intact so compiler relocation remains supported. The bundled
LLD links every target: the compiler selects the ELF, COFF, or Mach-O flavor
for the artifact target.

`.github/workflows/release.yml` runs these scripts for every platform from a
release tag, smoke-tests each archive and its language server, builds the VS
Code extension (`nia-<version>.vsix`), and publishes them together. The tag
rules are in [Contributing](../../docs/dev/contributing.md#releases).
