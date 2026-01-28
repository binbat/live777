@echo off
echo ========================================
echo Multi-Port Message Routing Demo Startup Script
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
echo 2. Starting liveion UDP bridge with multi-port routing...
start "Multi-Port UDP Bridge" cmd /k "target\release\liveion-udp-bridge.exe -v"

echo Waiting for bridge program to start...
timeout /t 3 /nobreak >nul

echo.
echo 3. Starting Python HTTP server...
python --version >nul 2>&1
if %errorlevel% equ 0 (
    start "Python HTTP Server" cmd /k "python -m http.server 8080"
    echo Waiting for HTTP server to start...
    timeout /t 3 /nobreak >nul
) else (
    echo Warning: Python not detected, skipping HTTP server
)

echo.
echo 4. Starting multi-port UDP listeners...
python --version >nul 2>&1
if %errorlevel% equ 0 (
    start "Multi-Port UDP Listeners" cmd /k "python test_multiport_udp_listener.py"
) else (
    echo Warning: Python not detected, skipping UDP listeners
)

echo.
echo 5. Opening Web interface...
python --version >nul 2>&1
if %errorlevel% equ 0 (
    start "" "http://localhost:8080/examples/working_multiport_control.html"
    timeout /t 2 /nobreak >nul
) else (
    echo Warning: Please manually open examples\working_multiport_control.html
)

echo.
echo ========================================
echo Multi-Port Routing Demo Started!
echo ========================================
echo.
echo Service Status:
echo - Liveion Server: http://localhost:7777
echo - Multi-Port UDP Bridge: Message routing enabled
echo - Multi-Port UDP Listeners:
echo   * Port 8888: Media Control Messages
echo   * Port 8890: PTZ Control Messages  
echo   * Port 8892: General Control Messages
echo - Python HTTP Server: http://localhost:8080
echo - Web Control Interface: http://localhost:8080/examples/working_multiport_control.html
echo.
echo Message Routing Architecture:
echo 1. Web Interface sends DataChannel messages with 'message_type' field
echo 2. Bridge receives DataChannel messages from liveion server
echo 3. Bridge parses 'message_type' and routes to appropriate UDP port:
echo    - ptz_control     -> UDP port 8890 (PTZ Control)
echo    - media_control   -> UDP port 8888 (Media Control)  
echo    - general_control -> UDP port 8892 (General Control)
echo 4. Multi-port UDP listeners receive and display routed messages
echo.
echo Usage Instructions:
echo 1. Wait for all services to start completely (about 15 seconds)
echo 2. Click "Connect" button in the Web Control Interface
echo 3. Use different control panels to send different message types
echo 4. Check the Multi-Port UDP Listeners window to verify routing
echo.
echo Expected Behavior:
echo - PTZ controls (arrows, stop) -> Messages appear on port 8890
echo - Media controls (start/stop stream, bitrate) -> Messages appear on port 8888
echo - General controls (status, ping, reset) -> Messages appear on port 8892
echo.
echo This solves the original problem: PTZ and media controls now use separate UDP channels!
echo.
pause