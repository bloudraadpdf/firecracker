@REM Copyright 2026 Bloudraad.
@REM SPDX-License-Identifier: Apache-2.0

@echo on
setlocal
set "EWDK="
for %%D in (D E F G H) do if exist %%D:\BuildEnv\SetupBuildEnv.cmd set EWDK=%%D:
if not defined EWDK exit /b 2
call %EWDK%\BuildEnv\SetupBuildEnv.cmd
if errorlevel 1 exit /b 3
set "CL="
set "_CL_="
set CL=/DNONTOSPINTERLOCK
cd /d C:\LegacyVsock\upstream
if errorlevel 1 exit /b 4
set XP_ARGS=/m:2 /t:Build /p:Configuration="WinXP Release" /p:Platform=Win32 /p:PlatformToolset=v142 /p:UseLegacyDDK=true /p:DDKINSTALLROOT=C:\LegacyVsock\wdk7\WinDDK\ /p:DDKVER=7600.16385.win7_wdk.100208-1538 /p:SignMode=Off /p:RunCodeAnalysis=false /p:EnableInf2cat=false /v:minimal
msbuild VirtIO\VirtioLib.vcxproj %XP_ARGS%
if errorlevel 1 exit /b 10
msbuild VirtIO\WDF\VirtioLib-WDF.vcxproj %XP_ARGS%
if errorlevel 1 exit /b 11
msbuild viosock\sys\viosock.vcxproj %XP_ARGS%
if errorlevel 1 exit /b 12
exit /b 0
