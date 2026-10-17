@REM Copyright 2026 Bloudraad.
@REM SPDX-License-Identifier: Apache-2.0

@echo off
if not "%~3"=="" goto bad_options
if not "%~1"=="" if not "%~1"=="GenerationChecks" goto bad_options
if "%~1"=="" if not "%~2"=="" goto bad_options
if "%~1"=="GenerationChecks" (
    set "WINDOWS_GENERATION_CHECKS=1"
    set "WINDOWS_GENERATION_PROBE_SHA256=%~2"
)
mode COM1: BAUD=115200 PARITY=n DATA=8 STOP=1
C:\xp-connect.exe >C:\xp-legacy-probe.log 2>&1
set RESULT=%ERRORLEVEL%
>>C:\xp-legacy-probe.log echo CONNECT_RESULT=%RESULT%
type C:\xp-legacy-probe.log >COM1
shutdown -s -t 0
exit /b
:bad_options
echo XPLEGACY_FAIL invalid_generation_options >COM1
shutdown -s -t 0
