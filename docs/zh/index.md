---
# https://vitepress.dev/reference/default-theme-home-page
layout: home

hero:
  name: "Live777"
  text: "一个实时流媒体服务器"
  tagline: 简单，高性能，WebRTC SFU Server
  image:
    src: ../logo.svg
    alt: Live777
  actions:
    - theme: brand
      text: 什么是 Live777 ？
      link: /zh/guide/what-is-live777
    - theme: alt
      text: 快速开始
      link: /zh/guide/getting-started

features:
  - title: 📚 WHIP / WHEP 优先
    details: 标准 WebRTC HTTP 信令协议，省去适配烦恼
  - title: 🚀 P2P-SFU 融合架构
    details: 同时具有 P2P 和 SFU 的优点，可以在只有一个人是使用 P2P 模式，多人使用 SFU 模式
  - title: 🌐 多平台的支持
    details: Linux, MacOS, Windows, Android 多种操作系统和多种架构的支持
  - title: 🕸️ 集群与级联
    details: 配套的 liveman 管理器可以将多个 Live777 节点组成集群，代理客户端请求、管理节点间的级联状态，并协调整个集群的录制
  - title: 🎥 流录制
    details: 将发布的流录制为分片 MP4 片段，保存到本地文件系统或兼容 S3 的对象存储，通过 REST API 控制
  - title: 📊 WebUI 与可观测性
    details: 内置嵌入式 WebUI、管理与会话 REST API 以及 Prometheus 指标，便于在生产环境中运维和观测
---

<style>
:root {
  --vp-home-hero-name-color: transparent;
  --vp-home-hero-name-background: -webkit-linear-gradient(120deg, #bd34fe 30%, #41d1ff);

  --vp-home-hero-image-background-image: linear-gradient(-45deg, #bd34fe 50%, #47caff 50%);
  --vp-home-hero-image-filter: blur(44px);
}

@media (min-width: 640px) {
  :root {
    --vp-home-hero-image-filter: blur(56px);
  }
}

@media (min-width: 960px) {
  :root {
    --vp-home-hero-image-filter: blur(68px);
  }
}
</style>
