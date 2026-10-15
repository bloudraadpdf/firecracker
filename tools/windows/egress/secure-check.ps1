# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
param([Parameter(Mandatory=$true)][ValidateRange(1, 3)][int]$Iteration)
$ErrorActionPreference = 'Stop'
if (-not (Confirm-SecureBootUEFI)) { throw 'Secure Boot is disabled' }
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
$baseline = 'C:\Users\Public\fc-tpm-state.txt'
if ($Iteration -eq 1) {
    $state | Set-Content -Encoding ASCII $baseline
} elseif (($state -join "`n") -cne ((Get-Content $baseline) -join "`n")) {
    throw 'TPM PCR7 or persistent object changed after restore'
}
"PASS SECURE_STATE iteration=$Iteration SecureBoot GetTpm TBS_GetRandom"
