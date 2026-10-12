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
# wayland://<socket 名称>?width=&height=&fps=&bitrate=
# 所有参数均可省略（默认值：socket 为 live777-desktop，
# 1280x720@30，2 Mbps）。
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

- **仅支持 SHM 客户端** —— CPU 共享内存 buffer（wl_shm）。GPU buffer
  传递（dmabuf/EGL）尚未实现。
- **不支持输入注入** —— 提供了带键盘/指针的 seat 以便客户端绑定，
  但不会产生任何输入事件。
- **没有音频** —— 该流只有视频。
- **子表面（subsurface）和 xdg popup 不参与合成** —— 只绘制 xdg
  toplevel（popup 会收到 configure，但会被跳过）。
- **buffer scale ≠ 1 和 buffer 旋转/翻转按原样绘制**（尽力而为）。

## H.264 专利说明

编码器为 OpenH264。H.264 在部分司法辖区受专利约束：Cisco 的专利许可
只覆盖其**预编译**二进制——自行从源码构建 live777 时，编解码器专利
许可的责任由该二进制的分发方承担。
