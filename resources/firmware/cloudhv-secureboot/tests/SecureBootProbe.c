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
