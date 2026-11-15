# Wayland 合成器信源

live777 可以充当一个**无头 Wayland 显示服务器**：普通的 GUI
应用程序直接渲染进去，合成后的桌面画面经过 H.264 编码，作为 WebRTC
流发布出去。不需要物理显示器、GPU 或已有的 X11/Wayland 会话。

这是一种流*信源*：使用 `source-wayland` feature 构建 live777
（仅支持 Linux，不包含在 `source-all` 中），然后给流配置一个
`wayland://` URL：

```bash
cargo build --release --bin live777 --features source-wayland
```

链接时需要 `libxkbcommon` 共享库（Debian/Ubuntu 上为
`libxkbcommon-dev`）；没有其他系统依赖。

## 配置

```toml
# 该流是 provisioned（预置）的：启动时即存在，即使还没有任何应用连接，
# 也会出现在 API/Dashboard 中。
[stream.desktop]
[[stream.desktop.sources]]
# wayland://<socket 名称>?width=&height=&fps=&bitrate=&xwayland=&xdisplay=
# 所有参数均可省略（默认值：socket 为 live777-desktop，
# 1280x720@30，2 Mbps，xwayland=1，xdisplay 自动分配）。
url = "wayland://desktop-0?width=1920&height=1080&fps=30&bitrate=4000000"
```

合成器在启动时创建 Wayland socket `$XDG_RUNTIME_DIR/desktop-0`，因此
**启动 live777 时必须设置 `XDG_RUNTIME_DIR`**。按名称把任意 Wayland
应用指向这个显示：

```bash
export XDG_RUNTIME_DIR=/run/user/$(id -u)
live777 &                                  # 提供流服务
WAYLAND_DISPLAY=desktop-0 my-application   # 渲染进这条流
```

订阅端和观看其他流完全一样：浏览器打开
`http://localhost:7777/`，或任意 WHEP 播放器。在第一个应用连接之前，
流会输出一个移动的测试图案，因此发布会立即建立，订阅永远不需要
等待客户端。

## X11 应用（XWayland）

X11 应用同样可以渲染进来：`xwayland=1`（默认）时合成器会启动一个
**XWayland** 服务器并充当其 X11 窗口管理器。这需要 `PATH` 上存在
`Xwayland` 二进制（如 `xwayland`/`xorg-xwayland` 软件包）；没有它时
X11 支持会跳过并给出警告，原生 Wayland 客户端不受影响。

显示编号自动分配并记录在启动日志中
（`XWayland ready, X11 clients use DISPLAY=:N`）；也可以用 `xdisplay=`
固定：

```toml
url = "wayland://desktop-0?xdisplay=0"
```

```bash
DISPLAY=:0 xterm &      # 与 Wayland 客户端渲染进同一条流
```

窗口管理刻意保持最小——这是一台虚拟显示器，不是桌面：窗口保持自己
请求的几何尺寸（没有指针输入来移动或缩放它们），全屏/最大化请求
映射为整个画布，最后映射的窗口绘制在最上层。X11 与 Wayland 客户端
可以混在同一条流上；X11 窗口绘制在 xdg toplevel 之上，
override-redirect 窗口（菜单、工具提示）绘制在最顶层。

X11 socket 位于 `/tmp/.X11-unix/`（不存在时会创建）。要让容器内的
X11 客户端连进来，把它 bind-mount 进去（或共享 network namespace
以使用抽象 socket）：

```bash
docker run -v /tmp/.X11-unix:/tmp/.X11-unix -e DISPLAY=:0 ...
```

设置 `LIVE777_WAYLAND_XWAYLAND_LOG=1` 可以继承 XWayland 的 stderr
而不是丢弃它（X11 客户端一直黑屏时排查用）。

## 远程输入（键盘与鼠标）

在 wayland URL 上加 `input=1` 后，观众可以驱动桌面：浏览器播放器
通过 `input` DataChannel cable 发送 JSON 事件，合成器将它们注入
seat——指针移动/按键带命中测试的焦点，键盘事件发给最后点击的窗口，
滚轮作为 wl_pointer axis 帧。

```toml
url = "wayland://desktop-0?input=1"
```

用独立播放器在浏览器里边看边操作
（`http://localhost:7777/tools/player.html?id=desktop-0&input`）——
先点击视频一次给它键盘焦点，然后照常输入和点击。任何 WHEP 客户端
都可以用一个名为 `input` 的 DataChannel 做到同样的事；线路格式见
`web/player-core/input-capture.ts`
（`{"t":"key"|"move"|"button"|"wheel", ...}`）。

**安全**：默认关闭。开启后，该流的*每个*订阅者都能控制桌面——
请配合 auth token 使用，只在确实需要远程控制时开启。

## 桌面音频

`audio=1`（默认）时流还会携带桌面的声音：live777 连接 **PipeWire**
（守护进程 socket 位于 `$XDG_RUNTIME_DIR/pipewire-0`），抽取一个
`Audio/Sink` 节点的 monitor，以 Opus track 发布（48 kHz 立体声，
`audio_bitrate=` 单位 bps，默认 64000）。如果没有可到达的 PipeWire
守护进程，流保持纯视频并给出警告。

无头容器里默认没有音频服务器；运行 PipeWire 加会话管理器和一个
虚拟 sink：

```bash
pipewire & wireplumber & pipewire-pulse &
pw-cli create-node adapter '{ factory.name=support.null-audio-sink
  node.name="live777-out" media.class=Audio/Sink }'
# 应用程序照常播放声音（默认 sink），声音随即进入这条流
```

有多个 sink 时，按名称子串选择要捕获的那个：
`audio_sink=live777-out`。会话管理器（wireplumber）是必需的——
正是它把捕获流链接到 sink 的 monitor 端口。

## 以 systemd 服务方式运行

系统服务默认没有 `XDG_RUNTIME_DIR`。给该服务分配一个 runtime
目录，并让 live777 和应用程序都指向它：

```ini
[Service]
RuntimeDirectory=live777
Environment=XDG_RUNTIME_DIR=/run/live777
```

`RuntimeDirectory=live777` 会创建属于服务用户的 `/run/live777`；
客户端随之用 `XDG_RUNTIME_DIR=/run/live777 WAYLAND_DISPLAY=desktop-0`
连接。

## 当前限制

- **GPU 客户端需要宿主机上有可用的 GPU/EGL 驱动** —— 满足条件时会广播
  `zwp_linux_dmabuf_v1`，GL 客户端（Qt Quick、浏览器、mpv 等）
  可以直接渲染进流；不满足时合成器回退到 CPU 合成，仅支持 SHM
  （wl_shm）客户端。DRM 设备默认依次尝试 `/dev/dri/renderD128` 和
  `/dev/dri/card0`，可用 `LIVE777_WAYLAND_DRM_DEVICE` 覆盖。X11 GL
  应用同理（XWayland glamor → DRI3 → dmabuf）。
- **默认不做输入注入** —— 除非在 wayland URL 上设置 `input=1`：此后
  DataChannel 事件（`input` cable）会注入 seat，焦点跟随点击；X11
  窗口在点击时 raise+activate。未开启时桌面是只读的。
- **音频依赖 PipeWire** —— 桌面声音从 PipeWire sink monitor 捕获；
  没有守护进程时流为纯视频。
- **子表面（subsurface）和 xdg popup 不参与合成** —— 只绘制 xdg
  toplevel（popup 会收到 configure，但会被跳过）。X11 的
  override-redirect 窗口*会*参与合成，因此 X11 菜单/工具提示可见。
- **X11 窗口装饰只有客户端自绘** —— 内置 XWM 不绘制标题栏，且 X11
  与 Wayland 客户端之间没有剪贴板同步。
- **buffer scale ≠ 1 和 buffer 旋转/翻转按原样绘制**（尽力而为）。

## H.264 专利说明

编码器为 OpenH264。H.264 在部分司法辖区受专利约束：Cisco 的专利许可
只覆盖其**预编译**二进制——自行从源码构建 live777 时，编解码器专利
许可的责任由该二进制的分发方承担。
