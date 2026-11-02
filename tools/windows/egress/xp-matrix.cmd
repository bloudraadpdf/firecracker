@echo off
REM Copyright 2026 Bloudraad.
REM SPDX-License-Identifier: Apache-2.0
if not "%~3"=="" goto bad_options
if not "%~1"=="" if not "%~1"=="GenerationChecks" goto bad_options
if "%~1"=="" if not "%~2"=="" goto bad_options
if "%~1"=="GenerationChecks" (
    set "WINDOWS_GENERATION_CHECKS=1"
    set "WINDOWS_GENERATION_PROBE_SHA256=%~2"
)
set "WINDOWS_GENERATION_PREVIOUS="
set iteration=1
:loop
set WINDOWS_GENERATION_ITERATION=%iteration%
C:\restore-probe.exe >C:\fc-restore-probe.txt 2>&1
set result=%ERRORLEVEL%
type C:\fc-restore-probe.txt >COM1
if not "%result%"=="0" goto probe_failed
if "%WINDOWS_GENERATION_CHECKS%"=="1" (
    set "next_generation="
    for /f "tokens=2 delims==" %%G in ('findstr /b /c:"VMGENID=" C:\fc-restore-probe.txt') do (
        if defined next_generation goto bad_options
        set "next_generation=%%G"
    )
    if not defined next_generation goto bad_options
)
if "%WINDOWS_GENERATION_CHECKS%"=="1" set "WINDOWS_GENERATION_PREVIOUS=%next_generation%"
echo XP_CHECK iteration=%iteration% >COM1
if "%iteration%"=="3" goto done
ping -n 31 127.0.0.1 >nul
set /a iteration+=1
goto loop
:bad_options
set result=arguments_or_generation_record
:probe_failed
echo XP_CHECK_FAILED result=%result% >COM1
:done
shutdown -s -t 0
