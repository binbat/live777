# 快速开始

## 最小化运行

### 可以使用 Docker 来运行 Live777:

::: danger 注意
**需要用 host 模式的网络**
:::

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

**打开浏览器，输入网址：`http://localhost:7777/`**
