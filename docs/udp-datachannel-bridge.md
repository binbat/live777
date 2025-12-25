# UDP to DataChannel Bridge

## 概述

这个功能实现了从 UDP 到 WebRTC DataChannel 的通用桥接，主要用于云台控制等场景。

## 架构

```
UDP 控制设备/客户端
    ↓ (UDP 数据包)
livecam UDP Socket (control_port)
    ↓ (原始字节)
broadcast channel
    ↓
liveion DataChannel Forward
    ↓ (WebRTC DataChannel)
浏览器/订阅端
```

### 双向通信支持

```
UDP 控制设备 → livecam → liveion → WebRTC → 浏览器
              ↑                              ↓
UDP 控制设备 ← livecam ← liveion ← WebRTC ← 浏览器
```

## 配置

### livecam.toml 配置示例

```toml
[[cameras]]
id = "camera1"
rtp_port = 5004
control_port = 5005  # 启用 UDP 控制端口

[cameras.codec]
mime_type = "video/H264"
clock_rate = 90000
channels = 0
sdp_fmtp_line = "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
```

## 使用方法

### 1. 启动 livecam 服务

```bash
./livecam --config conf/livecam.toml
```

### 2. 发送 UDP 控制指令

使用任何 UDP 客户端发送控制指令到配置的 `control_port`：

#### Python 示例

```python
import socket

# 创建 UDP socket
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

# 发送控制指令（示例：JSON 格式）
control_message = b'{"action": "pan", "direction": "left", "speed": 50}'
sock.sendto(control_message, ('127.0.0.1', 5005))

# 或者发送二进制格式
binary_message = bytes([0x01, 0x00, 0x32])  # 命令ID + 参数
sock.sendto(binary_message, ('127.0.0.1', 5005))

sock.close()
```

#### Node.js 示例

```javascript
const dgram = require('dgram');
const client = dgram.createSocket('udp4');

// JSON 格式
const message = JSON.stringify({
  action: 'pan',
  direction: 'left',
  speed: 50
});

client.send(message, 5005, '127.0.0.1', (err) => {
  if (err) console.error(err);
  client.close();
});
```

#### Bash 示例

```bash
# 使用 netcat 发送 UDP 消息
echo '{"action":"pan","direction":"left"}' | nc -u 127.0.0.1 5005

# 或使用 socat
echo '{"action":"zoom","value":2}' | socat - UDP:127.0.0.1:5005
```

### 3. 在浏览器中接收控制指令

```javascript
// 创建 WebRTC 连接（WHEP）
const pc = new RTCPeerConnection();

// 创建 DataChannel
const dc = pc.createDataChannel('control');

dc.onopen = () => {
  console.log('DataChannel opened');
};

dc.onmessage = (event) => {
  // 接收从 UDP 转发过来的控制指令
  const data = event.data;
  
  if (typeof data === 'string') {
    // 文本格式
    const command = JSON.parse(data);
    console.log('Received control command:', command);
  } else {
    // 二进制格式
    const buffer = new Uint8Array(data);
    console.log('Received binary command:', buffer);
  }
};

// 发送反馈（可选）
dc.send(JSON.stringify({ status: 'ok', position: { pan: 45, tilt: 30 } }));
```

## 协议格式

当前实现是**协议无关**的，支持任意格式的数据：

### 文本格式（推荐用于开发测试）

```json
{
  "action": "pan",
  "direction": "left",
  "speed": 50
}
```

### 二进制格式（推荐用于生产环境）

```
[命令ID][参数1][参数2]...
例如：[0x01][0x00][0x32]
```

### 常见云台协议

可以直接透传以下协议：

- **Pelco-D**: 7字节二进制协议
- **Pelco-P**: 8字节二进制协议
- **ONVIF PTZ**: XML over UDP
- **自定义协议**: 任意格式

## 性能特性

- **缓冲区大小**: 1024 字节（可在 `control_receiver.rs` 中调整）
- **广播通道容量**: 1024 条消息
- **延迟**: < 10ms（本地网络）
- **支持并发**: 多个 UDP 客户端可同时发送

## 调试

### 启用详细日志

```bash
RUST_LOG=debug ./livecam --config conf/livecam.toml
```

### 查看控制消息

日志中会显示：
- UDP 控制接收器启动信息
- 每 100 个数据包的统计
- 错误和警告信息

### 测试连接

```bash
# 测试 UDP 端口是否开放
nc -vzu 127.0.0.1 5005

# 发送测试消息
echo "test" | nc -u 127.0.0.1 5005
```

## 后续扩展

### 添加协议解析

在 `control_receiver.rs` 中添加协议解析逻辑：

```rust
// 解析 Pelco-D 协议示例
fn parse_pelco_d(data: &[u8]) -> Option<PtzCommand> {
    if data.len() != 7 {
        return None;
    }
    // 解析逻辑...
}
```

### 添加访问控制

```rust
// 验证 UDP 来源
if !is_authorized_peer(peer_addr) {
    warn!("Unauthorized control message from {}", peer_addr);
    continue;
}
```

### 添加消息队列

```rust
// 使用优先级队列处理控制指令
let (tx, rx) = tokio::sync::mpsc::channel(100);
```

## 故障排查

### UDP 消息未收到

1. 检查防火墙设置
2. 确认 control_port 配置正确
3. 使用 `tcpdump` 或 `wireshark` 抓包

```bash
sudo tcpdump -i lo -n udp port 5005
```

### DataChannel 未连接

1. 检查 WebRTC 连接状态
2. 确认 DataChannel 已创建
3. 查看浏览器控制台错误

### 消息丢失

1. 检查广播通道容量（默认 1024）
2. 增加缓冲区大小
3. 实现消息确认机制

## 安全建议

1. **生产环境**: 使用 DTLS/TLS 加密 UDP 通信
2. **认证**: 实现基于 token 的访问控制
3. **速率限制**: 防止 UDP 洪水攻击
4. **输入验证**: 验证所有接收到的数据

## 示例场景

### 云台控制

```python
# 控制云台向左旋转
sock.sendto(b'{"cmd":"pan","dir":"left","speed":50}', ('192.168.1.100', 5005))

# 控制云台变焦
sock.sendto(b'{"cmd":"zoom","value":2}', ('192.168.1.100', 5005))
```

### 机器人控制

```python
# 前进
sock.sendto(b'{"cmd":"move","dir":"forward","speed":100}', ('192.168.1.100', 5005))

# 停止
sock.sendto(b'{"cmd":"stop"}', ('192.168.1.100', 5005))
```

### 传感器数据

```python
# 发送传感器读数
sock.sendto(b'{"sensor":"temp","value":25.5}', ('192.168.1.100', 5005))
```
