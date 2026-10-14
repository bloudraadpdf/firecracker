/* SPDX-License-Identifier: GPL-2.0-or-later */
#pragma once
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>
#include <inttypes.h>
#include <errno.h>
#include <assert.h>
#include <glib.h>
#define ARRAY_SIZE(x) (sizeof(x) / sizeof((x)[0]))
#define QEMU_PACKED __attribute__((packed))
#define QEMU_BUILD_BUG_ON(condition) _Static_assert(!(condition), #condition)
static inline bool uadd64_overflow(uint64_t a, uint64_t b, uint64_t *out) {
    return __builtin_add_overflow(a, b, out);
}
typedef struct VMStateDescription VMStateDescription;
typedef struct Object Object;
typedef struct Error Error;
