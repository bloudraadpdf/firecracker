@echo on
setlocal
for %%D in (D E F G H) do if exist %%D:\BuildEnv\SetupBuildEnv.cmd set EWDK=%%D:
if not defined EWDK exit /b 2
call %EWDK%\BuildEnv\SetupBuildEnv.cmd x86
set WDK7=C:\LegacyVsock\wdk7\WinDDK\7600.16385.win7_wdk.100208-1538
cd /d C:\LegacyVsock\upstream\viosock\lib
if not exist objfre_wxp_x86 mkdir objfre_wxp_x86
cl /nologo /c /O2 /MT /Zl /GS /W4 /WX /DWIN32 /DWIN32_LEAN_AND_MEAN /D_WINDOWS /D_USRDLL /DVIOSOCKLIB_EXPORTS /DUNICODE /D_UNICODE /DNDEBUG /D_WIN32_WINNT=0x0501 /DWINVER=0x0501 /DNTDDI_VERSION=0x05010200 /I%WDK7%\inc\crt /I"%WindowsSdkDir%Include\10.0.19041.0\um" /I"%WindowsSdkDir%Include\10.0.19041.0\shared" /FI"%WindowsSdkDir%Include\10.0.19041.0\shared\sal.h" /Foobjfre_wxp_x86\ dllmain.c install.c native.c viosocklib.c utils.c
if errorlevel 1 exit /b 30
link /nologo /DLL /DEBUG /SUBSYSTEM:WINDOWS,5.01 /OSVERSION:5.1 /VERSION:5.1 /MACHINE:X86 /NODEFAULTLIB /DEF:viosocklib.def /OUT:objfre_wxp_x86\viosocklib.dll /LIBPATH:%WDK7%\lib\wxp\i386 /LIBPATH:%WDK7%\lib\Crt\i386 objfre_wxp_x86\*.obj libcmt.lib BufferOverflowU.lib kernel32.lib ws2_32.lib advapi32.lib ntdll.lib
if errorlevel 1 exit /b 31
exit /b 0
