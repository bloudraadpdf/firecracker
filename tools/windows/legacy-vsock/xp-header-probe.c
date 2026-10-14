#include <ntddk.h>

LONGLONG Probe(volatile LONGLONG *value)
{
    return InterlockedCompareExchange64(value, 1, 0);
}
