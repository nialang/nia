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
