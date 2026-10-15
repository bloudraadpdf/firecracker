// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: BSD-2-Clause-Patent

#include <Uefi.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Library/SerialPortLib.h>
#include <Library/PrintLib.h>

STATIC UINT64 MapBuffer[8192];

STATIC VOID
Report (CONST CHAR8 *Name, UINT64 First, UINT64 Second)
{
  CHAR8 Line[160];
  UINTN Length;

  Length = AsciiSPrint (Line, sizeof (Line), "MEMORY-PROBE %a %Lx %Lx\r\n", Name, First, Second);
  SerialPortWrite ((UINT8 *)Line, Length);
}

STATIC BOOLEAN
Allocatable (UINT32 Type)
{
  return Type == EfiLoaderCode || Type == EfiLoaderData ||
         Type == EfiBootServicesCode || Type == EfiBootServicesData ||
         Type == EfiConventionalMemory;
}

STATIC BOOLEAN
Intersects (UINT64 Start, UINT64 End, UINT64 OtherStart, UINT64 OtherEnd)
{
  return Start < OtherEnd && OtherStart < End;
}

EFI_STATUS EFIAPI
UefiMain (IN EFI_HANDLE ImageHandle, IN EFI_SYSTEM_TABLE *SystemTable)
{
  EFI_STATUS Status;
  EFI_MEMORY_DESCRIPTOR *Entry;
  EFI_MEMORY_DESCRIPTOR *Other;
  UINTN Size = sizeof (MapBuffer);
  UINTN Key;
  UINTN Stride;
  UINT32 Version;
  UINTN Offset;
  UINTN Previous;
  UINT64 Start;
  UINT64 End;
  UINT64 OtherEnd;
  UINT64 PlatformReserved = 0;
  UINT64 SmbiosReserved = 0;
  BOOLEAN LowRam = FALSE;
  BOOLEAN HighRam = FALSE;

  (VOID)ImageHandle;
  (VOID)SystemTable;
  Status = gBS->GetMemoryMap (&Size, (EFI_MEMORY_DESCRIPTOR *)MapBuffer, &Key, &Stride, &Version);
  if (EFI_ERROR (Status)) {
    Report ("FAIL-get-map", Status, Size);
    return Status;
  }
  if (Size == 0 || Size > sizeof (MapBuffer) || Stride < sizeof (*Entry) ||
      Size % Stride != 0 || Version != EFI_MEMORY_DESCRIPTOR_VERSION) {
    Report ("FAIL-map-shape", Size, Stride);
    return EFI_COMPROMISED_DATA;
  }
  for (Offset = 0; Offset < Size; Offset += Stride) {
    Entry = (EFI_MEMORY_DESCRIPTOR *)((UINT8 *)MapBuffer + Offset);
    Start = Entry->PhysicalStart;
    if (Entry->NumberOfPages == 0 || (Start & EFI_PAGE_MASK) != 0 ||
        Entry->NumberOfPages > (MAX_UINT64 - Start) / EFI_PAGE_SIZE ||
        Entry->Type >= EfiMaxMemoryType) {
      Report ("FAIL-descriptor", Start, Entry->NumberOfPages);
      return EFI_COMPROMISED_DATA;
    }
    End = Start + Entry->NumberOfPages * EFI_PAGE_SIZE;
    for (Previous = 0; Previous < Offset; Previous += Stride) {
      Other = (EFI_MEMORY_DESCRIPTOR *)((UINT8 *)MapBuffer + Previous);
      OtherEnd = Other->PhysicalStart + Other->NumberOfPages * EFI_PAGE_SIZE;
      if (Intersects (Start, End, Other->PhysicalStart, OtherEnd)) {
        Report ("FAIL-overlap", Start, Other->PhysicalStart);
        return EFI_COMPROMISED_DATA;
      }
    }
    if (Allocatable (Entry->Type)) {
      if (Intersects (Start, End, 0x9fc00, 0x100000) ||
          Intersects (Start, End, 0x2000000, 0x2100000)) {
        Report ("FAIL-reserved-allocatable", Start, End);
        return EFI_COMPROMISED_DATA;
      }
      LowRam |= Intersects (Start, End, 0x100000, 0x2000000);
      HighRam |= Intersects (Start, End, 0x100000000ULL, 0x140000000ULL);
    }
    if (Entry->Type == EfiReservedMemoryType) {
      if (Intersects (Start, End, 0x2000000, 0x2100000)) {
        PlatformReserved += MIN (End, 0x2100000) - MAX (Start, 0x2000000);
      }
      if (Intersects (Start, End, 0xf0000, 0xf1000)) {
        SmbiosReserved += MIN (End, 0xf1000) - MAX (Start, 0xf0000);
      }
    }
    Report ("descriptor", ((UINT64)Entry->Type << 56) | Start, End);
  }
  if (PlatformReserved != 0x100000 || SmbiosReserved != EFI_PAGE_SIZE || !LowRam || !HighRam) {
    Report ("FAIL-coverage", PlatformReserved, SmbiosReserved);
    Report ("FAIL-ram", LowRam, HighRam);
    return EFI_COMPROMISED_DATA;
  }
  Report ("PASS-reservations", PlatformReserved, SmbiosReserved);
  Report ("PASS-ram-below-above-4G", LowRam, HighRam);
  return EFI_SUCCESS;
}
