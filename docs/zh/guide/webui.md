# Web UI

​该 Web 界面内置于 live777 系统中​​

## Web WHIP/WHEP 客户端​​

**​打开浏览器并访问以下地址： `http://localhost:7777/`**

```
http://localhost:7777/
```

## ​调试工具​​

```
http://localhost:7777/tools/debugger.html
```

您可通过此功能测试 WebRTC-SVC

## ​单页面播放器​​

示例地址：

```
http://localhost:7777/tools/player.html?id=web-0&autoplay&controls&muted&reconnect=3000
```

​URL 参数说明：​​

- `id`: string, live777 流 ID
- `autoplay`: boolean
- `controls`: boolean
- `muted`: boolean, 是否默认静音
- `reconnect`: number, 重连超时时间（毫秒）

