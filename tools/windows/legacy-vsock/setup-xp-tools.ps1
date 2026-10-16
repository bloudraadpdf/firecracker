# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0

$ErrorActionPreference = 'Stop'
$transcriptStarted = $false
try {
    Start-Transcript -Path C:\LegacyVsock\setup-xp-tools.log -Force
    $transcriptStarted = $true
    $media = Get-Volume | Where-Object { $_.DriveLetter -and (Test-Path "$($_.DriveLetter):\WDK\headers.msi") } | Select-Object -First 1
    if (-not $media) { throw 'WDK 7.1 media not found' }
    $destination = 'C:\LegacyVsock\wdk7'
    New-Item -ItemType Directory -Force $destination | Out-Null
    foreach ($package in 'headers', 'libs_x86fre', 'wxplibs_x86fre', 'buildtools_x86fre', 'wdftools_x86fre', 'wcoinstallers') {
        $source = "$($media.DriveLetter):\WDK\$package.msi"
        $process = Start-Process msiexec.exe -ArgumentList @('/a', $source, "TARGETDIR=$destination", '/qn', '/l*v', "C:\LegacyVsock\wdk7-$package.log") -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw "$package extraction failed: $($process.ExitCode)" }
    }
    Get-ChildItem $destination -Recurse -File | Where-Object { $_.Name -in 'ntddk.h', 'wdf.h', 'wdfldr.lib', 'ntoskrnl.lib', 'tracewpp.exe', 'stampinf.exe' } | Select-Object -ExpandProperty FullName | Set-Content C:\LegacyVsock\wdk7-paths.txt
    Set-Content C:\LegacyVsock\setup-xp-tools-result.txt 0
} catch {
    $_ | Out-File C:\LegacyVsock\setup-xp-tools-error.txt
    Set-Content C:\LegacyVsock\setup-xp-tools-result.txt 1
} finally {
    try {
        if ($transcriptStarted) { Stop-Transcript }
    } finally {
        & shutdown.exe /s /t 0
    }
}
