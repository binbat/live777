@echo off
echo ========================================
echo 完整的 Liveion UDP Bridge Demo 启动脚本
echo ========================================
echo.

REM 检查是否存在桥接程序
if not exist "target\release\liveion-udp-bridge.exe" (
    echo 错误: 桥接程序未找到，请先编译项目
    echo 运行: build_simple.bat
    pause
    exit /b 1
)

REM 检查是否存在 liveion 程序
if not exist "target\release\live777.exe" (
    echo 错误: liveion 程序未找到，请先编译整个项目
    echo 运行: cargo build --release
    pause
    exit /b 1
)

echo 1. 启动 liveion 服务器...
start "Liveion Server" cmd /k "target\release\live777.exe --config conf\live777.toml"

echo 等待 liveion 服务器启动...
timeout /t 5 /nobreak >nul

echo.
echo 2. 启动 liveion UDP 桥接程序...
start "Liveion UDP Bridge" cmd /k "target\release\liveion-udp-bridge.exe -v"

echo 等待桥接程序启动...
timeout /t 3 /nobreak >nul

echo.
echo 4. 启动 Python HTTP 服务器...
python --version >nul 2>&1
if %errorlevel% equ 0 (
    start "Python HTTP Server" cmd /k "python -m http.server 8080"
    echo 等待 HTTP 服务器启动...
    timeout /t 3 /nobreak >nul
) else (
    echo 警告: 未检测到 Python，跳过 HTTP 服务器
)

echo.
echo 5. 系统已启动完成
echo   UDP 桥接程序会将 DataChannel 消息转发到 UDP 端口 8889
echo   你可以使用任何 UDP 客户端监听该端口来接收消息

echo.
echo 6. 打开 Web 界面...
python --version >nul 2>&1
if %errorlevel% equ 0 (
    start "" "http://localhost:8080/examples/liveion_udp_control.html"
    timeout /t 2 /nobreak >nul
) else (
    echo 警告: 请手动打开 examples\liveion_udp_control.html
)

echo.
echo ========================================
echo 启动完成！
echo ========================================
echo.
echo 服务状态:
echo - Liveion 服务器: http://localhost:7777
echo - UDP 桥接程序: 监听端口 8888
echo - Python HTTP 服务器: http://localhost:8080
echo - Web 控制界面: http://localhost:8080/examples/liveion_udp_control.html
echo.
echo 使用说明:
echo 1. 等待所有服务启动完成（约10秒）
echo 2. 在"Web 控制界面"中点击"连接"按钮
echo 3. 使用控制面板发送云台控制指令
echo 4. UDP 消息会自动发送到端口 8889
echo.
echo 测试 UDP 接收:
echo 你可以使用任何 UDP 客户端工具监听端口 8889 来接收控制消息
echo 例如: netcat, socat, 或自定义 UDP 客户端
echo.
echo 重要提示:
echo - 必须通过 HTTP 服务器访问页面 (http://localhost:8080/...)
echo - 不要直接双击打开 HTML 文件 (file:// 协议会被 CORS 阻止)
echo - UDP 消息会发送到 127.0.0.1:8889
echo.
echo 要停止所有服务，关闭所有命令行窗口
echo.
pause