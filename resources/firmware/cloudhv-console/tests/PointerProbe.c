// SPDX-License-Identifier: BSD-2-Clause-Patent
#include <Uefi.h>
#include <Protocol/AbsolutePointer.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Library/UefiLib.h>
#include <Library/SerialPortLib.h>
#include <Library/PrintLib.h>
#include <Library/MemoryAllocationLib.h>

EFI_STATUS
EFIAPI
UefiMain (IN EFI_HANDLE ImageHandle, IN EFI_SYSTEM_TABLE *SystemTable)
{
  EFI_HANDLE *Handles;
  UINTN Count;
  UINTN Index;
  UINTN Frame;
  EFI_ABSOLUTE_POINTER_PROTOCOL *Pointer;
  EFI_ABSOLUTE_POINTER_STATE State;
  EFI_STATUS Status;
  CHAR8 Line[160];
  UINTN Length;

  Status = gBS->LocateHandleBuffer (AllHandles, NULL, NULL, &Count, &Handles);
  if (!EFI_ERROR (Status)) {
    for (Index = 0; Index < Count; Index++) {
      gBS->ConnectController (Handles[Index], NULL, NULL, TRUE);
    }
    FreePool (Handles);
  }
  for (Frame = 0; Frame < 12000; Frame++) {
    Status = gBS->LocateHandleBuffer (ByProtocol, &gEfiAbsolutePointerProtocolGuid, NULL, &Count, &Handles);
    if (!EFI_ERROR (Status)) {
      for (Index = 0; Index < Count; Index++) {
        Status = gBS->HandleProtocol (Handles[Index], &gEfiAbsolutePointerProtocolGuid, (VOID **)&Pointer);
        if (!EFI_ERROR (Status) && !EFI_ERROR (Pointer->GetState (Pointer, &State))) {
          Length = AsciiSPrint (Line, sizeof (Line), "POINTER %Lu %Lu buttons=%u max=%Lu,%Lu\r\n",
            State.CurrentX, State.CurrentY, State.ActiveButtons,
            Pointer->Mode->AbsoluteMaxX, Pointer->Mode->AbsoluteMaxY);
          SerialPortWrite ((UINT8 *)Line, Length);
          Print (L"%a", Line);
        }
      }
      FreePool (Handles);
    }
    gBS->Stall (10000);
  }
  return EFI_SUCCESS;
}
