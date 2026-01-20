@echo off
echo ========================================
echo Complete Liveion UDP Bridge Demo Startup Script
echo ========================================
echo.

REM Check if bridge program exists
if not exist "target\release\liveion-udp-bridge.exe" (
    echo Error: Bridge program not found, please compile the project first
    echo Run: build_simple.bat
    pause
    exit /b 1
)

REM Check if liveion program exists
if not exist "target\release\live777.exe" (
    echo Error: liveion program not found, please compile the entire project first
    echo Run: cargo build --release
    pause
    exit /b 1
)

echo 1. Starting liveion server...
start "Liveion Server" cmd /k "target\release\live777.exe --config conf\live777.toml"

echo Waiting for liveion server to start...
timeout /t 5 /nobreak >nul

echo.
echo 2. Starting liveion UDP bridge program...
start "Liveion UDP Bridge" cmd /k "target\release\liveion-udp-bridge.exe -v"

echo Waiting for bridge program to start...
timeout /t 3 /nobreak >nul

echo.
echo 4. Starting Python HTTP server...
python --version >nul 2>&1
if %errorlevel% equ 0 (
    start "Python HTTP Server" cmd /k "python -m http.server 8080"
    echo Waiting for HTTP server to start...
    timeout /t 3 /nobreak >nul
) else (
    echo Warning: Python not detected, skipping HTTP server
)

echo.
echo 5. System startup completed
echo   UDP bridge program will forward DataChannel messages to UDP port 8889
echo   You can use any UDP client to listen on that port to receive messages

echo.
echo 6. Opening Web interface...
python --version >nul 2>&1
if %errorlevel% equ 0 (
    start "" "http://localhost:8080/examples/liveion_udp_control.html"
    timeout /t 2 /nobreak >nul
) else (
    echo Warning: Please manually open examples\liveion_udp_control.html
)

echo.
echo ========================================
echo Startup Complete!
echo ========================================
echo.
echo Service Status:
echo - Liveion Server: http://localhost:7777
echo - UDP Bridge Program: Listening on port 8888
echo - Python HTTP Server: http://localhost:8080
echo - Web Control Interface: http://localhost:8080/examples/liveion_udp_control.html
echo.
echo Usage Instructions:
echo 1. Wait for all services to start completely (about 10 seconds)
echo 2. Click "Connect" button in the Web Control Interface
echo 3. Use the control panel to send PTZ control commands
echo 4. UDP messages will be automatically sent to port 8889
echo.
echo Test UDP Reception:
echo You can use any UDP client tool to listen on port 8889 to receive control messages
echo Example: netcat, socat, or custom UDP client
echo.
echo Important Notes:
echo - Must access pages through HTTP server (http://localhost:8080/...)
echo - Do not directly double-click HTML files (file:// protocol will be blocked by CORS)
echo - UDP messages will be sent to 127.0.0.1:8889
echo.
echo To stop all services, close all command line windows
echo.
pause