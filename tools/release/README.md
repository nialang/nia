# Nia Release Build

`build.sh` is the release compiler build entry point. It requires the pinned
LLVM 23.1.2 static prefix produced by [`../llvm/build-static.sh`](../llvm/README.md),
builds `nia-cli` with the `llvm-static` feature, and rejects a binary with a
shared LLVM or LLD dependency. Standard-library sources remain part of the
toolchain resources and are compiled through the normal source pipeline.

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
`linux-aarch64`, `darwin-x86_64`, and `darwin-aarch64`. It runs on the matching host, invokes
`build.sh`, copies the compiler into `bin/nia`, the standard-library resources
into `lib/`, and the bundled LLD into `libexec/ld.lld`:

```sh
NIA_RELEASE_PLATFORM=darwin-aarch64 tools/release/package.sh
```

Set `NIA_RELEASE_OUTPUT` to choose another output directory. The package is
intended to be unpacked as a complete, portable prefix:

```text
nia-<version>-<platform>/
├── bin/nia
├── lib/toolchain.meta
├── lib/std/
├── lib/runtime/
└── libexec/ld.lld
```

`package.ps1` assembles the matching `windows-x86_64` ZIP archive on a Windows
MSVC host. It uses `bin/nia.exe`, `lib/`, and `libexec/ld.lld.exe` with the same
relative layout. Keep that layout intact so compiler relocation remains
supported. The bundled LLD links every target: the compiler selects the ELF,
COFF, or Mach-O flavor for the artifact target.
