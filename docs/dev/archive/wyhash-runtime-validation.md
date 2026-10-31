# Wyhash Runtime Validation

Executed on 2026-10-30 from Windows x64, using Ubuntu 24.04 WSL for Linux
i686 execution. These are runtime results, not cross-target type checks.

## Real Streams

`crates/nia-cli/tests/support/wyhash_large_stream.nia` uses the public std
implementation without modifying its state. A deterministic 64 KiB buffer is
repeated to supply 4,295,032,833 bytes per stream. The cumulative position is
`u64`; each individual slice and offset fits a 32-bit `usize`.

Two independent streams use full-buffer chunks and a repeating irregular
schedule of 1, 7, 47, 48, 49, 4093, 65521, and 65536 bytes. Chunks are also
clipped at buffer and checkpoint boundaries. Both check the digest at these
offsets from 2^32: -49, -48, -17, -16, -1, 0, 1, 15, 16, 17, 47, 48, 49,
65, and 65537. Each checkpoint includes an empty update, two calls to
`finish`, and continued input afterwards. This processes over 8 GiB per run
without allocating a large input.

| Target | Optimization | Result | Observed elapsed time |
| --- | --- | --- | --- |
| Windows x64 | `-O2` | Both streams, all 15 checkpoints passed | 2.37 s for the initial standalone run |
| Linux i686 under WSL | `-O2` | Both streams, all 15 checkpoints passed | 3.74 s for the retained fixture |
| Linux i686 under WSL | `-O0` | Both streams, all 15 checkpoints passed | 10.53 s |

An initial i686 run reported 264 KiB peak RSS through `/usr/bin/time -v`.
`file` identified the output as an ELF 32-bit Intel 80386 static-pie executable;
the program also reported `pointerWidth=32`. WSL ran the executable directly.
Timings are observations on this machine, not benchmark guarantees.

The retained expensive integration test is ignored by default:

```powershell
cargo test -p nia-cli --test std_hash_map emit_exe_std_wyhash_real_stream_crosses_4gib -- --ignored --nocapture
```

To repeat the 32-bit execution from the repository root on Windows with WSL:

```powershell
New-Item -ItemType Directory -Force target/wyhash-runtime-validation | Out-Null
target/debug/nia.exe --resource-root lib --target x86-unknown-linux -O2 emit --exe crates/nia-cli/tests/support/wyhash_large_stream.nia -o target/wyhash-runtime-validation/linux-x86
wsl -d Ubuntu-24.04 --exec /usr/bin/time -v /mnt/d/Projects/nia/target/wyhash-runtime-validation/linux-x86
```

Adjust the checkout mount and distribution name; replace `-O2` with `-O0`
for the unoptimized run. The Nia executable is freestanding and needs no libc.

## Independent Oracle

Expected digests come from the unmodified official historical
[`old_versions/wyhash_final4.h`](https://github.com/wangyi-fudan/wyhash/blob/e4764a0b637d34d3421a7760affada9288b625a8/old_versions/wyhash_final4.h).
Its SHA-256 is
`c5e06492d44be7b9143568674317118e49f673bb223fd725479d2defe728a9c2`.
Use this historical header: the current default header changes both secrets
and the exact 48-byte boundary rule.

`crates/nia-cli/tests/support/wyhash_large_stream_reference.c` calls that
header's one-shot function for every full checkpoint length, with seed 9 and
the original final4 secrets. It maps the same 64 KiB file into repeated
adjacent virtual regions. The reference reads the complete logical input on
each call, without storing 4 GiB of distinct physical data. This requires a
64-bit Linux address space and enough available virtual memory areas (about
65,538 mappings); the 32-bit program does not use this strategy.

Run in a 64-bit Linux shell from the repository root:

```sh
mkdir -p target/wyhash-runtime-validation
curl -fL https://raw.githubusercontent.com/wangyi-fudan/wyhash/e4764a0b637d34d3421a7760affada9288b625a8/old_versions/wyhash_final4.h -o target/wyhash-runtime-validation/wyhash_final4.h
sha256sum target/wyhash-runtime-validation/wyhash_final4.h
gcc -O3 -I target/wyhash-runtime-validation crates/nia-cli/tests/support/wyhash_large_stream_reference.c -o target/wyhash-runtime-validation/reference
target/wyhash-runtime-validation/reference
```

Output pairs are cumulative length and decimal digest. All 15 outputs matched
the retained Nia fixture, including a second run of the retained C source.
The upstream header is downloaded for this manual check, not vendored.

## Algorithm-Width Overflow

The generated source from
`emit_exe_std_wyhash_preserves_long_input_after_length_wrap` was also retained
temporarily, cross-linked for i686 at `-O0` and `-O2`, and executed through WSL.
Both exited zero. This covers the actual implementation seeded near
`u64::MAX`, 66 continuation lengths with repeated finish, crossing 2^32, and
25 wrapping-add boundary pairs against the independent reference.

No real 2^64-byte stream was processed. That boundary remains a seeded-state
test; the 4 GiB boundary above was crossed through actual repeated updates.
No new correctness defect was found during these runtime experiments.
