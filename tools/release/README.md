# Nia Release Build

`build.sh` is the release compiler build entry point. It requires the pinned
LLVM 22.1.0 static prefix produced by [`../llvm/build-static.sh`](../llvm/README.md),
builds `nia-cli` with the `llvm-static` feature, and rejects a binary with a
shared LLVM or LLD dependency.

Run it from any directory:

```sh
tools/release/build.sh
```

Set `LLVM_SYS_221_PREFIX` when the static prefix is outside the repository.
The release compiler still has normal host dependencies such as libc and the
C++ runtime. Target programs may additionally require a target sysroot, CRT,
and linker runtime; those are separate from the compiler's LLVM linkage.

## Linux package

`package.sh` assembles the relocatable `linux-x86_64` archive used by the
GitHub release workflow. It invokes `build.sh`, copies the compiler into
`bin/nia`, the standard-library resources into `lib/nia`, and the bundled LLD
into `libexec/nia/ld.lld`, then writes a reproducible tarball and `SHA256SUMS`:

```sh
tools/release/package.sh
```

Set `NIA_RELEASE_OUTPUT` to choose another output directory. The package is
intended to be unpacked as a complete prefix; keep the relative `bin`, `lib`,
and `libexec` layout intact so compiler relocation remains supported.
