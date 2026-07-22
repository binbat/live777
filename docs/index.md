---
# https://vitepress.dev/reference/default-theme-home-page
layout: home

hero:
  name: "Live777"
  text: "A Real-time Audio and Video Streaming Media Server"
  tagline: A very simple, high performance, lightweight, verge WebRTC SFU
  image:
    src: logo.svg
    alt: Live777
  actions:
    - theme: brand
      text: What is Live777 ?
      link: /guide/what-is-live777
    - theme: alt
      text: Getting Started
      link: /guide/getting-started

features:
  - title: 📚 Support WHIP / WHEP
    details: The WHIP/WHEP protocol is implemented to improve interoperability with other WebRTC application modules without the need for custom adaptations.
  - title: 🚀 P2P-SFU integration architecture
    details: Only responsible for forwarding, do not do confluence, transcoding and other resource overhead of the media processing work, the encoding and decoding work are respectively placed on the sender and the receiver.
  - title: 🌐 Multiple platform support
    details: Linux, MacOS, Windows, Android and arm, x86 with multi-platform native support.
  - title: 🕸️ Cluster & cascade
    details: The companion liveman manager turns multiple Live777 nodes into a cluster — proxying client requests, managing cascade state between nodes, and coordinating recording across the cluster.
  - title: 🎥 Stream recording
    details: Record published streams as fragmented MP4 segments to the local filesystem or S3-compatible object storage, controlled through the REST API.
  - title: 📊 WebUI & observability
    details: An embedded WebUI, admin and session REST APIs, and Prometheus metrics are built in for easy operation and observability in production.
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
