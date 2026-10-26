@echo off
rem Compila con el entorno de Visual C++ cargado (Rust no siempre detecta Visual Studio 18 solo).
rem Uso: build.cmd            -> debug
rem      build.cmd --release  -> optimizado
setlocal
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if exist "%VSWHERE%" for /f "usebackq delims=" %%i in (`"%VSWHERE%" -latest -prerelease -products * -property installationPath`) do set "VSDIR=%%i"
if not defined VSDIR for /d %%d in ("%ProgramFiles%\Microsoft Visual Studio\*") do for /d %%e in ("%%d\*") do if exist "%%e\VC\Auxiliary\Build\vcvars64.bat" set "VSDIR=%%e"
if not defined VSDIR (
    echo No se encontro Visual Studio con las herramientas de C++ ^(Desktop development with C++^).
    exit /b 1
)
call "%VSDIR%\VC\Auxiliary\Build\vcvars64.bat" >nul
rem Carpeta de compilacion corta: en rutas muy profundas link.exe pasa el limite de 260 caracteres.
if not defined CARGO_TARGET_DIR set "CARGO_TARGET_DIR=%TEMP%\pega-target"
cargo build %*
