@echo off
echo ========================================
echo Build Liveion UDP Bridge
echo ========================================
echo.

REM Check Rust toolchain
echo Checking Rust toolchain...
rustc --version >nul 2>&1
if %errorlevel% neq 0 (
    echo Error: Rust toolchain not detected
    echo Please visit https://rustup.rs/ to install Rust
    pause
    exit /b 1
)

cargo --version >nul 2>&1
if %errorlevel% neq 0 (
    echo Error: Cargo not detected
    pause
    exit /b 1
)

echo Rust toolchain check passed
echo.

echo Starting build...

echo Cleaning previous build...
cargo clean -p liveion-udp-bridge

echo Building Release version...
cargo build --release -p liveion-udp-bridge

if %errorlevel% equ 0 (
    echo.
    echo ========================================
    echo Build Successful!
    echo ========================================
    echo.
    
    if exist "target\release\liveion-udp-bridge.exe" (
        echo Executable generated: target\release\liveion-udp-bridge.exe
        dir "target\release\liveion-udp-bridge.exe"
    )
    
    echo.
    echo Usage:
    echo 1. Edit configuration file: liveion_udp_bridge\bridge.toml
    echo 2. Run program: target\release\liveion-udp-bridge.exe
    echo 3. Or use startup script: start_complete_demo.bat
    
) else (
    echo.
    echo ========================================
    echo Build Failed!
    echo ========================================
    echo.
    echo Please check the error messages above and fix the issues
)

echo.
pause