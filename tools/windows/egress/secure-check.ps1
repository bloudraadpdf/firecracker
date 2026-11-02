# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
param([Parameter(Mandatory=$true)][ValidateRange(1, 3)][int]$Iteration)
$ErrorActionPreference = 'Stop'
function Test-RestoredState([string]$Path, [string[]]$State) {
    if ($Iteration -eq 1) {
        $State | Set-Content -Encoding ASCII $Path
    } elseif (($State -join "`n") -cne ((Get-Content $Path) -join "`n")) {
        throw "State changed after restore: $Path"
    }
}
if (-not (Confirm-SecureBootUEFI)) { throw 'Secure Boot is disabled' }
$uefiState = @()
$hash = [Security.Cryptography.SHA256]::Create()
try {
    foreach ($name in @('PK', 'KEK', 'db', 'dbx')) {
        $variable = Get-SecureBootUEFI -Name $name
        if ($null -eq $variable.Bytes -or $variable.Bytes.Length -eq 0) {
            throw "Empty firmware variable: $name"
        }
        "UEFI_SIZE $name=$($variable.Bytes.Length)"
        $digest = [BitConverter]::ToString($hash.ComputeHash($variable.Bytes)).Replace('-', '').ToLowerInvariant()
        $uefiState += "UEFI_STATE $name=$digest"
    }
} finally {
    $hash.Dispose()
}
$uefiState | Write-Output
Test-RestoredState 'C:\Users\Public\fc-uefi-state.txt' $uefiState
$tpm = Get-Tpm
$tpm | Format-List TpmPresent, TpmReady, TpmEnabled, TpmActivated
if (-not ($tpm.TpmPresent -and $tpm.TpmReady -and $tpm.TpmEnabled -and $tpm.TpmActivated)) {
    throw 'TPM is not ready'
}
$output = @(& "$PSScriptRoot\tpm-runtime.exe")
$output | Write-Output
if ($LASTEXITCODE -ne 0) { throw "TBS probe exit $LASTEXITCODE" }
$state = @($output | Where-Object { $_ -cmatch '^TPM_STATE ' })
$patterns = @(
    '^TPM_STATE PCR7_SHA256=[0-9a-f]{64}$',
    '^TPM_STATE HANDLE=81[0-9a-f]{6}$',
    '^TPM_STATE PUBLIC=([0-9a-f]{2})+$',
    '^TPM_STATE NAME=([0-9a-f]{2})+$'
)
if ($state.Count -ne $patterns.Count) { throw 'Incomplete TPM state' }
for ($index = 0; $index -lt $patterns.Count; $index++) {
    if ($state[$index] -cnotmatch $patterns[$index]) { throw 'Malformed TPM state' }
}
Test-RestoredState 'C:\Users\Public\fc-tpm-state.txt' $state
& "$PSScriptRoot\uefi-runtime.exe" $Iteration
if ($LASTEXITCODE -ne 0) { throw "UEFI runtime probe exit $LASTEXITCODE" }
"PASS SECURE_STATE iteration=$Iteration SecureBoot GetTpm TBS_GetRandom"
