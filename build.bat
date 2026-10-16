@echo off
setlocal
cd /d "%~dp0"
cargo build --release --locked
if errorlevel 1 exit /b 1
echo Built: target\release\mabipng-studio.exe
