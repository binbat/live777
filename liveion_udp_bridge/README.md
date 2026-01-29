# Liveion Multi-Port UDP Bridge

这是一个用于连接 liveion WebRTC DataChannel 和多个 UDP 端口的消息路由桥接程序。它通过解析消息类型，将不同类型的控制消息路由到不同的 UDP 端口，实现了 PTZ 控制、媒体控制和通用控制的完全分离。

## 🎯 核心功能

- **消息路由**: 基于 `message_type` 字段智能路由消息到不同 UDP 端口
- **多端口支持**: 同时支持多个 UDP 端口的独立通信
- **类型分离**: PTZ控制、媒体控制、通用控制使用独立通道
- **优先级处理**: PTZ 控制消息获得最高优先级
- **双向通信**: 支持 DataChannel ↔ UDP 的双向消息转发
- **自动重连**: 当连接断开时自动重连 liveion 服务器

## 🏗️ 架构设计

```
Web控制界面 (working_multiport_control.html)
    ↓ WHIP DataChannel (message_type字段)
liveion服务器 (WebRTC处理)
    ↓ DataChannel消息转发
UDP桥接器 (消息路由核心)
    ├─ 解析message_type字段
    ├─ 路由到对应UDP端口
    └─ 多端口输出:
        ├─ 端口8888: 媒体控制 → hardware_media_controller.py
        ├─ 端口8890: PTZ控制  → hardware_ptz_controller.py
        └─ 端口8892: 通用控制 → test_multiport_udp_listener.py
```

## 📋 消息类型路由

| 消息类型 | UDP端口 | 用途 | 优先级 |
|---------|---------|------|--------|
| `ptz_control` | 8890 | 云台控制 (pan/tilt/zoom) | 最高 |
| `media_control` | 8888 | 媒体流控制 (码率/帧率) | 中等 |
| `general_control` | 8892 | 通用控制 (状态/配置) | 最低 |

## 🚀 安装和编译

1. 确保已安装 Rust 工具链
2. 克隆或下载项目代码
3. 编译项目：

```bash
cd liveion_udp_bridge
cargo build --release
```

## ⚙️ 配置

编辑 `bridge_multiport.toml` 配置文件：

```toml
[udp]
listen = "0.0.0.0"
port = 8888
# 目标地址包含所有端口
target_addresses = [
    "127.0.0.1:8888",  # Media control
    "127.0.0.1:8890",  # PTZ control  
    "127.0.0.1:8892"   # General control
]

[liveion]
url = "http://localhost:7777"
stream = "webcontrol"

[bridge]
reconnect_interval = 5
max_message_size = 16384
enable_logging = true
```

## 🎮 使用方法

### 1. 启动完整系统

使用提供的启动脚本：

```bash
# 硬件集成环境 (推荐)
start_hardware_integration.bat

# 消息路由演示
start_multiport_routing_demo.bat
```

### 2. 手动启动

```bash
# 1. 启动 liveion 服务器
target/release/live777.exe --config conf/live777.toml

# 2. 启动多端口桥接器
target/release/liveion-udp-bridge.exe -v

# 3. 启动硬件控制器
python hardware_ptz_controller.py      # PTZ控制器
python hardware_media_controller.py    # 媒体控制器
python test_multiport_udp_listener.py  # 通用控制器

# 4. 打开Web控制界面
# 浏览器访问: http://localhost:8080/examples/working_multiport_control.html
```

## 📝 消息格式

### Web界面发送的消息

#### PTZ控制消息 (路由到端口8890)
```json
{
  "message_type": "ptz_control",
  "action": "pan",
  "direction": "left",
  "speed": 50,
  "timestamp": 1769483281574
}
```

#### 媒体控制消息 (路由到端口8888)
```json
{
  "message_type": "media_control",
  "command": "start_stream",
  "quality": "high",
  "timestamp": 1769483285971
}
```

#### 通用控制消息 (路由到端口8892)
```json
{
  "message_type": "general_control",
  "command": "status",
  "param": "",
  "timestamp": 1769483288236
}
```

### 消息路由日志示例

```
🎯 [Bridge Router] Processing DataChannel message: {"message_type":"ptz_control",...}
📍 [Message Router] Detected message type: ptz_control
🎮 [PTZ Router] Routing PTZ control message to UDP port 8890
✅ [Port Router] Successfully sent message to UDP port 8890
```

## 🎯 硬件控制器

### PTZ控制器 (`hardware_ptz_controller.py`)
- **支持协议**: 串口(Pelco-D)、HTTP(海康威视/大华)、ONVIF、模拟器
- **监听端口**: 8890
- **控制功能**: 水平转动、垂直转动、变焦、预设位置

### 媒体控制器 (`hardware_media_controller.py`)
- **视频源**: RTSP摄像头、USB摄像头、测试图案
- **监听端口**: 8888
- **控制功能**: 启动/停止流、调整码率/帧率/分辨率

### 通用控制器 (`test_multiport_udp_listener.py`)
- **监听端口**: 8892
- **功能**: 系统状态查询、配置管理、连接测试

## 🧪 测试验证

### 1. 消息路由测试
启动系统后，在Web界面点击不同的控制按钮，观察消息是否路由到正确的UDP端口：

- PTZ控制 → 端口8890的控制器窗口显示消息
- 媒体控制 → 端口8888的控制器窗口显示消息  
- 通用控制 → 端口8892的控制器窗口显示消息

### 2. 路由验证
每个UDP监听器会显示路由验证信息：
```
✅ 路由正确: ptz_control -> 端口 8890
✅ 路由正确: media_control -> 端口 8888
✅ 路由正确: general_control -> 端口 8892
```

## 🔧 故障排除

### 1. 消息路由问题
- 检查消息是否包含正确的 `message_type` 字段
- 查看桥接器日志中的路由信息
- 确认目标UDP端口的监听器正在运行

### 2. DataChannel连接问题
- 确保使用WHIP模式连接 (`/whip/webcontrol`)
- 检查liveion服务器状态
- 查看浏览器开发者工具的WebRTC连接状态

### 3. UDP通信问题
- 确保防火墙允许UDP端口 8888、8890、8892
- 检查各个硬件控制器是否正常启动
- 使用测试脚本验证UDP通信

## 🎉 解决的问题

这个多端口消息路由架构成功解决了原始问题：

✅ **端口冲突消除**: PTZ控制和媒体控制现在使用完全独立的UDP端口  
✅ **消息分离**: 不同类型的消息不再相互干扰  
✅ **并发控制**: 可以同时进行视频流观看和云台控制  
✅ **实时响应**: PTZ控制获得最高优先级，确保实时响应  

**原问题**: "音视频流传输和云台控制消息传输占据了同一个pc端口，占据了同一个流。所以不能同时接收到云台画面和对云台进行控制。"

**现在**: 可以同时观看高质量视频流和实时控制云台，不同类型的控制消息使用完全独立的传输通道！

## 📚 开发和扩展

### 添加新的消息类型
1. 在Web界面中添加新的 `message_type`
2. 在 `bridge.rs` 的 `route_message_by_type` 函数中添加新的路由规则
3. 创建对应的UDP监听器处理新消息类型

### 自定义硬件控制器
参考现有的 `hardware_ptz_controller.py` 和 `hardware_media_controller.py`，创建自定义的硬件控制器。

## 📄 许可证

本项目采用与 live777 相同的许可证。