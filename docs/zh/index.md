---
# https://vitepress.dev/reference/default-theme-home-page
layout: home

hero:
  name: "Live777"
  text: "一个实时流媒体服务器"
  tagline: 简单，高性能，WebRTC SFU
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
  - title: 📚 支持 WHIP / WHEP
    details: 标准 WebRTC 协议，省去适配烦恼
  - title: 🚀 P2P-SFU 融合架构
    details: 同时具有 P2P 的优点和 SFU 的优点
  - title: 🌐 多平台的支持
    details: Linux, MacOS, Windows, Android 等多种架构的支持
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
