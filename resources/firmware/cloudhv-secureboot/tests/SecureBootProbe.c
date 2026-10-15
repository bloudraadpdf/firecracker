// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: BSD-2-Clause-Patent
#include <Uefi.h>
#include <Guid/GlobalVariable.h>
#include <Protocol/LoadedImage.h>
#include <Protocol/Tcg2Protocol.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Library/UefiRuntimeServicesTableLib.h>
#include <Library/UefiLib.h>
#include <Library/DevicePathLib.h>
#include <Library/SerialPortLib.h>
#include <Library/PrintLib.h>
#include <Library/MemoryAllocationLib.h>

STATIC VOID Report (CONST CHAR8 *Name, UINT64 Value) {
  CHAR8 Line[128];
  UINTN Length = AsciiSPrint (Line, sizeof (Line), "SECURE-PROBE %a=%Lx\r\n", Name, Value);
  SerialPortWrite ((UINT8 *)Line, Length);
  Print (L"%a", Line);
}
STATIC UINT32 NvCommand (EFI_TCG2_PROTOCOL *Tcg, UINT8 *Command, UINT32 Length) {
  UINT8 Response[512] = {0};
  EFI_STATUS Status = Tcg->SubmitCommand (Tcg, Length, Command, sizeof (Response), Response);
  Report ("nv-submit-status", Status);
  if (EFI_ERROR (Status)) { return MAX_UINT32; }
  UINT32 Size = ((UINT32)Response[2] << 24) | ((UINT32)Response[3] << 16) |
                ((UINT32)Response[4] << 8) | Response[5];
  if (Response[0] != 0x80 || (Response[1] != 1 && Response[1] != 2) ||
      Size < 10 || Size > sizeof (Response)) { return MAX_UINT32; }
  return ((UINT32)Response[6] << 24) | ((UINT32)Response[7] << 16) |
         ((UINT32)Response[8] << 8) | Response[9];
}
STATIC VOID NvClearMarker (EFI_TCG2_PROTOCOL *Tcg, EFI_GUID *Guid) {
  UINT8 ReadPublic[] = {0x80,1,0,0,0,14,0,0,1,0x69,1,0x50,0,0x20};
  UINT8 DefineSpace[] = {
    0x80,2,0,0,0,45,0,0,1,0x2a, 0x40,0,0,1, 0,0,0,9,
    0x40,0,0,9,0,0,0,0,0, 0,0, 0,14,
    1,0x50,0,0x20,0,0x0b,2,2,0,2,0,0,0,1
  };
  UINT8 Established = 0;
  UINTN Size = sizeof (Established);
  UINT32 Result = NvCommand (Tcg, ReadPublic, sizeof (ReadPublic));
  EFI_STATUS Status = gRT->GetVariable (L"FirecrackerNvMarker", Guid, NULL, &Size, &Established);
  Report ("nv-read-public", Result);
  if (Status == EFI_NOT_FOUND && Result == 0x18b) {
    Result = NvCommand (Tcg, DefineSpace, sizeof (DefineSpace));
    Report ("nv-define", Result);
    if (Result == 0) {
      Established = 1;
      Status = gRT->SetVariable (L"FirecrackerNvMarker", Guid, 7, sizeof (Established), &Established);
      Report ("nv-marker-write", Status);
      Report ("nv-read-public-after-define", NvCommand (Tcg, ReadPublic, sizeof (ReadPublic)));
    }
  }
}
EFI_STATUS EFIAPI UefiMain (IN EFI_HANDLE ImageHandle, IN EFI_SYSTEM_TABLE *SystemTable) {
  EFI_STATUS Status;
  EFI_LOADED_IMAGE_PROTOCOL *Loaded;
  EFI_DEVICE_PATH_PROTOCOL *Path;
  EFI_HANDLE Child = NULL;
  EFI_TCG2_PROTOCOL *Tcg;
  EFI_TCG2_BOOT_SERVICE_CAPABILITY Cap = {0};
  EFI_PHYSICAL_ADDRESS Log = 0, Last = 0;
  BOOLEAN Truncated = FALSE;
  UINT8 Secure = 0, Setup = 1, Zero = 0;
  UINTN Size = sizeof (Secure);
  UINT32 Counter = 0;
  UINT32 PpiFlags = 0;
  UINT8 PpiRequest[16] = {0};
  EFI_GUID PpiGuid = {0xaeb9c5c1,0x94f1,0x4d02,{0xbf,0xd9,0x46,0x02,0xdb,0x2d,0x3c,0x54}};
  EFI_GUID ProbeGuid = {0xcb36c34d,0xea2c,0x4d88,{0x83,0xd5,0x25,0x9c,0xfa,0x6b,0x49,0x11}};
  (VOID)SystemTable;
  Status = gRT->GetVariable (L"SecureBoot", &gEfiGlobalVariableGuid, NULL, &Size, &Secure);
  Report ("secure-status", Status); Report ("SecureBoot", Secure);
  Size = sizeof (Setup);
  Status = gRT->GetVariable (L"SetupMode", &gEfiGlobalVariableGuid, NULL, &Size, &Setup);
  Report ("setup-status", Status); Report ("SetupMode", Setup);
  Status = gRT->SetVariable (L"PK", &gEfiGlobalVariableGuid, 0x27, 0, NULL);
  Report ("unsigned-PK-delete", Status);
  Status = gRT->SetVariable (L"SecureBoot", &gEfiGlobalVariableGuid, 6, 1, &Zero);
  Report ("derived-state-write", Status);
  Status = gBS->HandleProtocol (ImageHandle, &gEfiLoadedImageProtocolGuid, (VOID **)&Loaded);
  if (!EFI_ERROR (Status)) {
    Path = FileDevicePath (Loaded->DeviceHandle, L"\\EFI\\unsigned.efi");
    Status = gBS->LoadImage (FALSE, ImageHandle, Path, NULL, 0, &Child);
    Report ("unsigned-image-load", Status);
    if (Child != NULL) { gBS->UnloadImage (Child); }
    FreePool (Path);
  }
  Status = gBS->LocateProtocol (&gEfiTcg2ProtocolGuid, NULL, (VOID **)&Tcg);
  Report ("tcg2-protocol", Status);
  if (!EFI_ERROR (Status)) {
    Cap.Size = sizeof (Cap);
    Status = Tcg->GetCapability (Tcg, &Cap);
    Report ("tcg2-capability", Status); Report ("tpm-present", Cap.TPMPresentFlag);
    Status = Tcg->GetEventLog (Tcg, EFI_TCG2_EVENT_LOG_FORMAT_TCG_2, &Log, &Last, &Truncated);
    Report ("event-log-status", Status); Report ("event-log-present", Log != 0 && Last >= Log);
    NvClearMarker (Tcg, &ProbeGuid);
  }
  Size = sizeof (PpiFlags);
  Status = gRT->GetVariable (L"Tcg2PhysicalPresenceFlags", &PpiGuid, NULL, &Size, &PpiFlags);
  Report ("ppi-flags-status", Status); Report ("ppi-flags", PpiFlags);
  if (!EFI_ERROR (Status)) {
    Status = gRT->SetVariable (L"Tcg2PhysicalPresenceFlags", &PpiGuid, 7, sizeof (PpiFlags), &PpiFlags);
    Report ("ppi-flags-write", Status);
  }
  Size = sizeof (PpiRequest);
  Status = gRT->GetVariable (L"Tcg2PhysicalPresence", &PpiGuid, NULL, &Size, PpiRequest);
  Report ("ppi-request-status", Status);
  if (!EFI_ERROR (Status) && Size == sizeof (PpiRequest)) {
    Report ("ppi-request", PpiRequest[0]); Report ("ppi-last", PpiRequest[8]);
    Report ("ppi-response", (UINT32)PpiRequest[12] | ((UINT32)PpiRequest[13] << 8) |
            ((UINT32)PpiRequest[14] << 16) | ((UINT32)PpiRequest[15] << 24));
  }
  Size = sizeof (Counter);
  Status = gRT->GetVariable (L"FirecrackerProbe", &ProbeGuid, NULL, &Size, &Counter);
  if (Status == EFI_NOT_FOUND) { Counter = 0; }
  else if (EFI_ERROR (Status)) { Report ("counter-read-error", Status); }
  Counter++;
  Status = gRT->SetVariable (L"FirecrackerProbe", &ProbeGuid, 7, sizeof (Counter), &Counter);
  Report ("counter-write", Status); Report ("counter", Counter);
  for (UINTN Tick = 0; Tick < 30; Tick++) {
    gBS->Stall (1000000);
    Size = sizeof (Counter);
    Status = gRT->GetVariable (L"FirecrackerProbe", &ProbeGuid, NULL, &Size, &Counter);
    if (EFI_ERROR (Status)) { Report ("heartbeat-read-error", Status); }
  }
  Report ("shutdown", Counter);
  gRT->ResetSystem (EfiResetShutdown, EFI_SUCCESS, 0, NULL);
  return EFI_SUCCESS;
}
