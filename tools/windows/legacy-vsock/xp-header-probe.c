// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#include <ntddk.h>

LONGLONG Probe(volatile LONGLONG *value)
{
    return InterlockedCompareExchange64(value, 1, 0);
}
