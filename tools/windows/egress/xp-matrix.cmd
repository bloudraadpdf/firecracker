@echo off
REM Copyright 2026 Bloudraad.
REM SPDX-License-Identifier: Apache-2.0
set iteration=1
:loop
C:\restore-probe.exe >C:\fc-restore-probe.txt 2>&1
set result=%ERRORLEVEL%
type C:\fc-restore-probe.txt >COM1
if not "%result%"=="0" goto failed
echo XP_CHECK iteration=%iteration% >COM1
if "%iteration%"=="3" goto done
ping -n 31 127.0.0.1 >nul
set /a iteration+=1
goto loop
:failed
echo XP_CHECK_FAILED result=%result% >COM1
:done
shutdown -s -t 0
