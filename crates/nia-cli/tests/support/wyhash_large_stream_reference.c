// SPDX-License-Identifier: GPL-3.0-or-later
// Build on 64-bit Linux with the pinned upstream final4 header; see
// docs/dev/wyhash-runtime-validation.md. No streaming algorithm is reimplemented.
#define _GNU_SOURCE
#include <inttypes.h>
#include <stdio.h>
#include <sys/mman.h>
#include <unistd.h>
#include "wyhash_final4.h"

int main(void) {
    const size_t period = 65536;
    const int offsets[] = {-49, -48, -17, -16, -1, 0, 1, 15, 16, 17, 47, 48, 49, 65, 65537};
    const size_t size = (UINT64_C(1) << 32) + 2 * period;
    int fd = memfd_create("wyhash-period", 0);
    if (fd < 0 || ftruncate(fd, period)) return 1;
    unsigned char *block = mmap(NULL, period, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (block == MAP_FAILED) return 2;
    for (unsigned i = 0; i < period; ++i) {
        uint64_t j = i;
        block[i] = (unsigned char)((j * j + 17 * j + (j >> 8)) ^ 0xa5);
    }
    // Reserve a contiguous virtual input, then alias each period to the same
    // physical data. Each upstream one-shot call still reads the entire input.
    unsigned char *data = mmap(NULL, size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (data == MAP_FAILED) return 3;
    for (size_t pos = 0; pos < size; pos += period) {
        if (mmap(data + pos, period, PROT_READ, MAP_SHARED | MAP_FIXED, fd, 0) == MAP_FAILED) return 4;
    }
    for (unsigned k = 0; k < sizeof(offsets) / sizeof(*offsets); ++k) {
        uint64_t len = (UINT64_C(1) << 32) + offsets[k];
        printf("%" PRIu64 " %" PRIu64 "\n", len, wyhash(data, len, 9, _wyp));
    }
    return 0;
}
