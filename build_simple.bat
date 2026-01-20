@echo off
echo ========================================
echo 构建 Liveion UDP Bridge
echo ========================================
echo.

REM 检查 Rust 工具链
echo 检查 Rust 工具链...
rustc --version >nul 2>&1
if %errorlevel% neq 0 (
    echo 错误: 未检测到 Rust 工具链
    echo 请访问 https://rustup.rs/ 安装 Rust
    pause
    exit /b 1
)

cargo --version >nul 2>&1
if %errorlevel% neq 0 (
    echo 错误: 未检测到 Cargo
    pause
    exit /b 1
)

echo Rust 工具链检查通过
echo.

echo 开始构建...

echo 清理之前的构建...
cargo clean -p liveion-udp-bridge

echo 构建 Release 版本...
cargo build --release -p liveion-udp-bridge

if %errorlevel% equ 0 (
    echo.
    echo ========================================
    echo 构建成功！
    echo ========================================
    echo.
    
    if exist "target\release\liveion-udp-bridge.exe" (
        echo 可执行文件已生成: target\release\liveion-udp-bridge.exe
        dir "target\release\liveion-udp-bridge.exe"
    )
    
    echo.
    echo 使用方法:
    echo 1. 编辑配置文件: liveion_udp_bridge\bridge.toml
    echo 2. 运行程序: target\release\liveion-udp-bridge.exe
    echo 3. 或使用启动脚本: start_simple.bat
    
) else (
    echo.
    echo ========================================
    echo 构建失败！
    echo ========================================
    echo.
    echo 请检查上面的错误信息并修复问题
)

echo.
pause