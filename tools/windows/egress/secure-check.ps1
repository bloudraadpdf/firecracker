# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
param([Parameter(Mandatory=$true)][int]$Iteration)
$ErrorActionPreference = 'Stop'
if (-not (Confirm-SecureBootUEFI)) { throw 'Secure Boot is disabled' }
$tpm = Get-Tpm
$tpm | Format-List TpmPresent, TpmReady, TpmEnabled, TpmActivated
if (-not ($tpm.TpmPresent -and $tpm.TpmReady -and $tpm.TpmEnabled -and $tpm.TpmActivated)) {
    throw 'TPM is not ready'
}
& "$PSScriptRoot\tpm-runtime.exe" | Out-String | Write-Output
if ($LASTEXITCODE -ne 0) { throw "TBS probe exit $LASTEXITCODE" }
"PASS SECURE_STATE iteration=$Iteration SecureBoot GetTpm TBS_GetRandom"
