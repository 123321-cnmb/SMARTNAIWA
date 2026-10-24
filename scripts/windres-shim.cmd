@echo off
rem Wrapper used by tauri-winres / embed-resource.
rem
rem Those crates pick their resource compiler based on the build HOST, not the
rem target. On a msvc host they run rc.exe, which emits a .res file that the
rem GNU linker cannot read. This shim translates rc.exe-style arguments to
rem MinGW's windres so it emits an object the GNU linker understands.
rem
rem Must stay pure ASCII: cmd.exe mis-parses non-ASCII bytes in .cmd files.
setlocal enabledelayedexpansion

set "OUT="
set "INC="
set "IN="
set "WINDRES="

for /f "delims=" %%i in ('where windres 2^>nul') do if not defined WINDRES set "WINDRES=%%i"
if not defined WINDRES if exist "D:\mingw64\bin\windres.exe" set "WINDRES=D:\mingw64\bin\windres.exe"

if not defined WINDRES (
  echo [windres-shim] windres not found in PATH or D:\mingw64\bin 1>&2
  exit /b 1
)

:parse
if "%~1"=="" goto run
if /I "%~1"=="/fo" goto getout
if /I "%~1"=="/I" goto getinc
set "IN=%~1"
shift
goto parse

:getout
set "OUT=%~2"
shift
shift
goto parse

:getinc
set "INC=%~2"
shift
shift
goto parse

:run
set "ARGS="
if not "%INC%"=="" set "ARGS=-I %INC%"
"%WINDRES%" %ARGS% -i "%IN%" -o "%OUT%"
if errorlevel 1 exit /b 1
exit /b 0
