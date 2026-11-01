# Nia Release Build

`build.sh` is the release compiler build entry point. It requires the pinned
LLVM 23.1.2 static prefix produced by [`../llvm/build-static.sh`](../llvm/README.md),
builds `nia-cli` with the `llvm-static` feature, and checks the result. It
rejects a prefix whose `llvm-config --shared-mode` is not `static`, and on ELF
hosts it also rejects a binary with a shared LLVM or LLD dependency (the check
reads the dynamic section with `readelf`). Standard-library sources remain part
of the toolchain resources and are compiled through the normal source pipeline.

Run it from any directory:

```sh
tools/release/build.sh
```

Set `LLVM_SYS_231_PREFIX` when the static prefix is outside the repository.
The release compiler still has normal host dependencies such as libc and the
C++ runtime. Target programs may additionally require a target sysroot, CRT,
and linker runtime; those are separate from the compiler's LLVM linkage.

## Platform packages

`package.sh` assembles relocatable Unix archives for `linux-x86_64`,
`linux-aarch64`, `darwin-x86_64`, and `darwin-aarch64`. It runs on the matching
host and invokes `build.sh`:

```sh
NIA_RELEASE_PLATFORM=darwin-aarch64 tools/release/package.sh
```

`package.ps1` assembles the matching `windows-x86_64` ZIP archive on a Windows
MSVC host. It does not build the compiler; build
`target\x86_64-pc-windows-msvc\release\nia.exe` first with the static LLVM
prefix:

```powershell
cargo build --release -p nia-cli --no-default-features --features llvm-static --target x86_64-pc-windows-msvc
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
├── lib/                   toolchain.meta, std/, runtime/, imports/
└── libexec/ld.lld         (libexec/ld.lld.exe on Windows)
```

Keep that layout intact so compiler relocation remains supported. The bundled
LLD links every target: the compiler selects the ELF, COFF, or Mach-O flavor
for the artifact target.
