# Liveion UDP Bridge

这是一个用于连接 liveion WebRTC DataChannel 和 UDP 协议的桥接程序。它允许 web 界面通过 DataChannel 发送控制指令，然后通过 UDP 转发给外部设备，同时也支持从 UDP 接收消息并转发到 web 界面。

## 功能特性

- **双向通信**: 支持 DataChannel ↔ UDP 的双向消息转发
- **自动重连**: 当连接断开时自动重连 liveion 服务器
- **消息格式**: 支持 JSON 和纯文本消息格式
- **客户端管理**: 自动跟踪 UDP 客户端，支持广播和定向发送
- **日志记录**: 详细的消息日志，便于调试

## 架构图

```
Web界面 <---> liveion (DataChannel) <---> UDP Bridge <---> UDP设备
```

## 安装和编译

1. 确保已安装 Rust 工具链
2. 克隆或下载项目代码
3. 编译项目：

```bash
cd liveion_udp_bridge
cargo build --release
```

## 配置

编辑 `bridge.toml` 配置文件：

```toml
[udp]
# UDP服务器监听地址
listen = "0.0.0.0"
# UDP服务器端口
port = 8888

[liveion]
# liveion服务器URL
url = "http://localhost:7777"
# 要连接的流名称
stream = "camera"

# 可选的认证配置
# [liveion.auth]
# username = "admin"
# password = "password"

[bridge]
# 重连间隔（秒）
reconnect_interval = 5
# 最大消息大小（字节）
max_message_size = 16384
# 启用详细日志
enable_logging = true
```

## 使用方法

### 1. 启动 liveion 服务器

确保 liveion 服务器正在运行并且有一个名为 "camera" 的流。

### 2. 启动 UDP 桥接程序

```bash
# 使用默认配置
./target/release/liveion-udp-bridge

# 使用自定义配置文件
./target/release/liveion-udp-bridge -c custom_bridge.toml

# 启用详细日志
./target/release/liveion-udp-bridge -v
```

### 3. 打开 web 界面

在浏览器中打开 `examples/liveion_udp_control.html`，点击"连接"按钮连接到 liveion 服务器。

### 4. 测试 UDP 通信

使用提供的测试脚本：

```bash
# 启动UDP监听器（接收来自桥接的消息）
python test_liveion_udp.py listen

# 启动UDP发送器（向桥接发送消息）
python test_liveion_udp.py send

# 发送测试命令序列
python test_liveion_udp.py test

# 同时启动监听器和发送器
python test_liveion_udp.py both
```

## 消息格式

### DataChannel 到 UDP

从 web 界面发送的消息会被包装为以下格式：

```json
{
  "type": "datachannel_to_udp",
  "data": "{\"action\":\"pan\",\"direction\":\"left\",\"speed\":50}"
}
```

桥接程序会提取 `data` 字段并通过 UDP 发送给所有连接的客户端。

### UDP 到 DataChannel

从 UDP 接收的消息会被包装为以下格式：

```json
{
  "type": "udp_to_datachannel",
  "client_id": "192.168.1.100:12345",
  "timestamp": 1640995200000,
  "data": "received message content"
}
```

### 控制指令示例

云台控制指令：

```json
{"action": "pan", "direction": "left", "speed": 50}
{"action": "tilt", "direction": "up", "speed": 30}
{"action": "zoom", "direction": "in", "value": 1}
{"action": "stop"}
```

自定义指令：

```json
{"action": "preset", "number": 1}
{"action": "custom", "value": 123, "message": "test"}
```

## 故障排除

### 1. 连接问题

- 确保 liveion 服务器正在运行
- 检查配置文件中的 URL 和流名称
- 查看桥接程序的日志输出

### 2. UDP 通信问题

- 确保防火墙允许 UDP 端口 8888
- 使用测试脚本验证 UDP 通信
- 检查 UDP 设备的网络配置

### 3. DataChannel 问题

- 确保 web 界面能够连接到 liveion
- 检查浏览器的开发者工具控制台
- 验证 WebRTC 连接状态

## 开发和扩展

### 添加新的消息类型

在 `src/bridge.rs` 中的 `handle_structured_datachannel_message` 函数中添加新的消息类型处理逻辑。

### 自定义 UDP 协议

修改 `src/udp_server.rs` 中的消息处理逻辑以支持特定的 UDP 协议格式。

### 添加认证

在配置文件中启用认证，桥接程序会自动处理 JWT token 的获取和使用。

## 许可证

本项目采用与 live777 相同的许可证。