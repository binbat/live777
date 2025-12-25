# UDP 控制接口快速入门

## 简介

这个功能允许你通过 UDP 协议发送控制指令到 live777，这些指令会通过 WebRTC DataChannel 转发给浏览器端。主要用于云台控制、机器人控制等场景。

## 快速开始

### 1. 配置 livecam

编辑 `conf/livecam.toml`，为摄像头添加 `control_port`：

```toml
[[cameras]]
id = "camera1"
rtp_port = 5004
control_port = 5005  # 添加这一行

[cameras.codec]
mime_type = "video/H264"
clock_rate = 90000
channels = 0
sdp_fmtp_line = "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
```

### 2. 启动服务

```bash
./livecam --config conf/livecam.toml
```

你应该看到类似的日志：

```
INFO livecam: UDP control receiver started stream_id="camera1" port=5005
```

### 3. 测试 UDP 控制

#### 方法 A: 使用 Python 测试工具

```bash
# 交互模式
python tests/udp_control_test.py --interactive

# 发送单条消息
python tests/udp_control_test.py --message "Hello"

# 发送 JSON 控制指令
python tests/udp_control_test.py --json '{"action":"pan","direction":"left","speed":50}'

# 压力测试
python tests/udp_control_test.py --stress 1000
```

#### 方法 B: 使用 Node.js 测试工具

```bash
# 交互模式
node tests/udp_control_test.js --interactive

# 发送单条消息
node tests/udp_control_test.js --message "Hello"

# 发送 JSON 控制指令
node tests/udp_control_test.js --json '{"action":"pan","direction":"left","speed":50}'
```

#### 方法 C: 使用命令行工具

```bash
# 使用 netcat
echo '{"action":"pan","direction":"left"}' | nc -u 127.0.0.1 5005

# 使用 socat
echo '{"action":"zoom","value":2}' | socat - UDP:127.0.0.1:5005
```

### 4. 在浏览器中接收控制指令

打开 live777 的 debugger 页面：`http://localhost:9999/tools/debugger.html`

在 DataChannel 部分，你会看到从 UDP 发送的消息。

或者使用自定义代码：

```javascript
// 连接到 WHEP 端点
const pc = new RTCPeerConnection();
const dc = pc.createDataChannel('control');

dc.onmessage = (event) => {
  const message = new TextDecoder().decode(event.data);
  console.log('收到控制指令:', message);
  
  // 解析 JSON
  try {
    const command = JSON.parse(message);
    handleCommand(command);
  } catch (e) {
    console.log('原始消息:', message);
  }
};

function handleCommand(command) {
  switch (command.action) {
    case 'pan':
      console.log(`云台旋转: ${command.direction}, 速度: ${command.speed}`);
      break;
    case 'tilt':
      console.log(`云台俯仰: ${command.direction}, 速度: ${command.speed}`);
      break;
    case 'zoom':
      console.log(`变焦: ${command.direction}, 值: ${command.value}`);
      break;
  }
}
```

## 交互模式使用示例

启动交互模式：

```bash
python tests/udp_control_test.py --interactive
```

然后输入命令：

```
udp> text Hello World
✓ Sent text message: Hello World

udp> json {"action":"pan","direction":"left"}
✓ Sent JSON message: {
  "action": "pan",
  "direction": "left"
}

udp> pan left
✓ Sent JSON message: {
  "action": "pan",
  "direction": "left",
  "speed": 50
}

udp> tilt up
✓ Sent JSON message: {
  "action": "tilt",
  "direction": "up",
  "speed": 50
}

udp> zoom in
✓ Sent JSON message: {
  "action": "zoom",
  "direction": "in",
  "value": 1
}

udp> quit
```

## 协议格式建议

### JSON 格式（推荐用于开发）

```json
{
  "action": "pan",
  "direction": "left",
  "speed": 50,
  "timestamp": 1234567890
}
```

### 二进制格式（推荐用于生产）

```
字节 0: 命令 ID (0x01=pan, 0x02=tilt, 0x03=zoom)
字节 1: 方向/参数
字节 2-3: 速度值 (big-endian)
```

示例：

```bash
# 发送二进制命令
python tests/udp_control_test.py --binary "01003200"
```

## 常见问题

### Q: UDP 消息发送了但浏览器收不到？

A: 检查以下几点：
1. 确认 livecam 已启动并配置了 `control_port`
2. 确认浏览器已创建 DataChannel
3. 查看 livecam 日志是否有错误
4. 使用 `tcpdump` 确认 UDP 包已到达：
   ```bash
   sudo tcpdump -i lo -n udp port 5005 -X
   ```

### Q: 如何查看详细日志？

A: 启动时设置日志级别：
```bash
RUST_LOG=debug ./livecam --config conf/livecam.toml
```

### Q: 支持多个客户端同时发送吗？

A: 支持。多个 UDP 客户端可以同时向同一个端口发送消息。

### Q: 消息大小限制是多少？

A: 当前限制为 1024 字节。如需更大，修改 `livecam/src/control_receiver.rs` 中的 `CONTROL_BUFFER_SIZE`。

### Q: 支持双向通信吗？

A: 支持。浏览器可以通过 DataChannel 发送消息，这些消息会通过 UDP 发送回最后一个发送控制指令的客户端。

## 性能测试

运行压力测试：

```bash
# 发送 10000 条消息，间隔 10ms
python tests/udp_control_test.py --stress 10000 --interval 0.01

# 发送 1000 条消息，无间隔（最大速度）
python tests/udp_control_test.py --stress 1000 --interval 0
```

预期性能：
- 本地延迟: < 5ms
- 吞吐量: > 10000 msg/s
- 丢包率: < 0.1%（本地网络）

## 下一步

- 查看完整文档：[UDP to DataChannel Bridge](./udp-datachannel-bridge.md)
- 实现自定义协议解析
- 添加访问控制和加密
- 集成到你的云台控制系统

## 示例项目

### Python 云台控制器

```python
import socket
import json
import time

class PtzController:
    def __init__(self, host='127.0.0.1', port=5005):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.addr = (host, port)
    
    def send_command(self, action, **params):
        cmd = {'action': action, **params}
        self.sock.sendto(json.dumps(cmd).encode(), self.addr)
    
    def pan_left(self, speed=50):
        self.send_command('pan', direction='left', speed=speed)
    
    def pan_right(self, speed=50):
        self.send_command('pan', direction='right', speed=speed)
    
    def tilt_up(self, speed=50):
        self.send_command('tilt', direction='up', speed=speed)
    
    def tilt_down(self, speed=50):
        self.send_command('tilt', direction='down', speed=speed)
    
    def zoom_in(self, value=1):
        self.send_command('zoom', direction='in', value=value)
    
    def zoom_out(self, value=1):
        self.send_command('zoom', direction='out', value=value)
    
    def stop(self):
        self.send_command('stop')

# 使用示例
ptz = PtzController()
ptz.pan_left(50)
time.sleep(2)
ptz.stop()
```

### Node.js 云台控制器

```javascript
const dgram = require('dgram');

class PtzController {
  constructor(host = '127.0.0.1', port = 5005) {
    this.client = dgram.createSocket('udp4');
    this.host = host;
    this.port = port;
  }

  sendCommand(action, params = {}) {
    const cmd = { action, ...params };
    const message = JSON.stringify(cmd);
    this.client.send(message, this.port, this.host);
  }

  panLeft(speed = 50) {
    this.sendCommand('pan', { direction: 'left', speed });
  }

  panRight(speed = 50) {
    this.sendCommand('pan', { direction: 'right', speed });
  }

  tiltUp(speed = 50) {
    this.sendCommand('tilt', { direction: 'up', speed });
  }

  tiltDown(speed = 50) {
    this.sendCommand('tilt', { direction: 'down', speed });
  }

  zoomIn(value = 1) {
    this.sendCommand('zoom', { direction: 'in', value });
  }

  zoomOut(value = 1) {
    this.sendCommand('zoom', { direction: 'out', value });
  }

  stop() {
    this.sendCommand('stop');
  }

  close() {
    this.client.close();
  }
}

// 使用示例
const ptz = new PtzController();
ptz.panLeft(50);
setTimeout(() => {
  ptz.stop();
  ptz.close();
}, 2000);
```
