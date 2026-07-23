use super::{InternalSourceConfig, MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
use anyhow::Result;
use async_trait::async_trait;
use rtsp::RtspMode;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, error, info, trace};

#[cfg(feature = "source")]
use tokio::sync::mpsc;

#[cfg(feature = "source")]
use rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters;

#[cfg(feature = "source")]
type RtcpSender = Arc<RwLock<Option<mpsc::Sender<(u8, Vec<u8>)>>>>;

#[cfg(feature = "source")]
struct RtspClientContext {
    stream_id: String,
    rtsp_url: String,
    config: InternalSourceConfig,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state: Arc<std::sync::RwLock<StreamSourceState>>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    media_info_store: Arc<RwLock<Option<rtsp::MediaInfo>>>,
    rtcp_tx_store: RtcpSender,
}

pub struct RtspSource {
    config: InternalSourceConfig,
    rtsp_url: String,
    state: Arc<std::sync::RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    task_handle: Option<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    #[cfg(feature = "source")]
    media_info: Arc<RwLock<Option<rtsp::MediaInfo>>>,
    #[cfg(feature = "source")]
    rtcp_tx: RtcpSender,
}

impl RtspSource {
    pub fn new(config: InternalSourceConfig, rtsp_url: String) -> Result<Self> {
        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Ok(Self {
            config,
            rtsp_url,
            state: Arc::new(std::sync::RwLock::new(StreamSourceState::Initializing)),
            rtp_tx,
            state_tx,
            task_handle: None,
            shutdown_tx: None,
            #[cfg(feature = "source")]
            media_info: Arc::new(RwLock::new(None)),
            #[cfg(feature = "source")]
            rtcp_tx: Arc::new(RwLock::new(None)),
        })
    }

    #[cfg(feature = "source")]
    pub async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        // Always hand out the wrapper, even while the RTSP client is down:
        // the forwarding task resolves the current RTCP channel per message,
        // so subscriber feedback (e.g. PLI keyframe requests) keeps flowing
        // across reconnects instead of dying with the connection that was
        // active when the bridge was created — same pattern as the WHEP
        // source's peer_store.
        let (wrapper_tx, mut wrapper_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let rtcp_tx = self.rtcp_tx.clone();
        let stream_id = self.config.stream_id.clone();

        tokio::spawn(async move {
            while let Some(data) = wrapper_rx.recv().await {
                let tx = rtcp_tx.read().await.clone();
                match tx {
                    Some(tx) => {
                        if let Err(e) = tx.send((1, data)).await {
                            // The connection is gone; the reconnect installs
                            // a fresh channel.
                            debug!(
                                "[{}] Dropping RTCP: connection channel closed ({})",
                                stream_id, e
                            );
                        }
                    }
                    None => {
                        debug!("[{}] Dropping RTCP: RTSP client not connected", stream_id);
                    }
                }
            }
        });

        Some(wrapper_tx)
    }

    async fn set_state(&self, new_state: StreamSourceState, error: Option<String>) {
        let changed = {
            let mut state = self.state.write().unwrap();
            let old_state = *state;

            if old_state != new_state {
                *state = new_state;
                Some(old_state)
            } else {
                None
            }
        };

        if let Some(old_state) = changed {
            let _ = self.state_tx.send(StateChangeEvent {
                old_state,
                new_state,
                error,
            });

            info!(
                "[{}] State changed: {:?} -> {:?}",
                self.config.stream_id, old_state, new_state
            );
        }
    }

    #[cfg(feature = "source")]
    async fn run_rtsp_client(
        ctx: RtspClientContext,
        mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        let mut reconnect_count = 0u32;

        loop {
            info!(
                "[{}] Connecting to {}",
                ctx.stream_id,
                super::redact_url(&ctx.rtsp_url)
            );

            {
                let mut s = ctx.state.write().unwrap();
                *s = if reconnect_count > 0 {
                    StreamSourceState::Reconnecting
                } else {
                    StreamSourceState::Initializing
                };
            }

            let parsed_url = match url::Url::parse(&ctx.rtsp_url) {
                Ok(url) => url,
                Err(e) => {
                    error!("[{}] Invalid URL: {}", ctx.stream_id, e);
                    Self::emit_state_change(
                        &ctx.state,
                        &ctx.state_tx,
                        StreamSourceState::Error,
                        Some(format!("Invalid URL: {}", e)),
                    )
                    .await;
                    break;
                }
            };

            let target_host = match parsed_url.host_str() {
                Some(host) => host,
                None => {
                    error!("[{}] No host in URL", ctx.stream_id);
                    Self::emit_state_change(
                        &ctx.state,
                        &ctx.state_tx,
                        StreamSourceState::Error,
                        Some("No host in URL".to_string()),
                    )
                    .await;
                    break;
                }
            };

            debug!("[{}] Extracted host: {}", ctx.stream_id, target_host);

            match rtsp::client::setup_rtsp_session(
                &ctx.rtsp_url,
                None,
                target_host,
                RtspMode::Pull,
                true,
            )
            .await
            {
                Ok((media_info, Some((tx, mut rx)))) => {
                    info!(
                        "[{}] Connected successfully, media: video={}, audio={}",
                        ctx.stream_id,
                        media_info.video_codec.is_some(),
                        media_info.audio_codec.is_some()
                    );

                    {
                        let mut store = ctx.media_info_store.write().await;
                        *store = Some(media_info);

                        let mut rtcp_store = ctx.rtcp_tx_store.write().await;
                        *rtcp_store = Some(tx.clone());

                        info!("[{}] RTCP sender stored", ctx.stream_id);
                        drop(rtcp_store);

                        let verify_store = ctx.rtcp_tx_store.read().await;
                        if verify_store.is_some() {
                            info!("[{}] RTCP sender verification: OK", ctx.stream_id);
                        } else {
                            error!("[{}] RTCP sender verification: FAILED", ctx.stream_id);
                        }
                    }

                    Self::emit_state_change(
                        &ctx.state,
                        &ctx.state_tx,
                        StreamSourceState::Connected,
                        None,
                    )
                    .await;

                    reconnect_count = 0;

                    let result = Self::receive_rtp_loop(
                        &ctx.stream_id,
                        &mut rx,
                        &ctx.rtp_tx,
                        &mut shutdown_rx,
                    )
                    .await;

                    // The connection's RTCP channel dies with it; drop the
                    // store so RTCP forwarding falls quiet until the
                    // reconnect installs a fresh one.
                    *ctx.rtcp_tx_store.write().await = None;

                    match result {
                        Ok(()) => {
                            info!("[{}] Gracefully stopped", ctx.stream_id);
                            break;
                        }
                        Err(e) => {
                            error!("[{}] RTP receive error: {}", ctx.stream_id, e);
                        }
                    }
                }
                Ok((_, None)) => {
                    error!("[{}] UDP mode not supported", ctx.stream_id);
                    Self::emit_state_change(
                        &ctx.state,
                        &ctx.state_tx,
                        StreamSourceState::Error,
                        Some("UDP mode not supported".to_string()),
                    )
                    .await;
                    break;
                }
                Err(e) => {
                    error!("[{}] Connection failed: {}", ctx.stream_id, e);

                    Self::emit_state_change(
                        &ctx.state,
                        &ctx.state_tx,
                        StreamSourceState::Disconnected,
                        Some(format!("Connection failed: {}", e)),
                    )
                    .await;
                }
            }

            if !ctx.config.reconnect_enabled() {
                info!("[{}] Reconnect disabled, exiting", ctx.stream_id);
                break;
            }

            reconnect_count += 1;

            if ctx.config.max_reconnect_attempts() > 0
                && reconnect_count > ctx.config.max_reconnect_attempts()
            {
                error!(
                    "[{}] Max reconnect attempts ({}) reached",
                    ctx.stream_id,
                    ctx.config.max_reconnect_attempts()
                );
                Self::emit_state_change(
                    &ctx.state,
                    &ctx.state_tx,
                    StreamSourceState::Error,
                    Some("Max reconnect attempts reached".to_string()),
                )
                .await;
                break;
            }

            info!(
                "[{}] Reconnecting in {}ms (attempt {}/{})",
                ctx.stream_id,
                ctx.config.reconnect_delay_ms(reconnect_count),
                reconnect_count,
                if ctx.config.max_reconnect_attempts() == 0 {
                    "∞".to_string()
                } else {
                    ctx.config.max_reconnect_attempts().to_string()
                }
            );

            tokio::select! {
                _ = &mut shutdown_rx => {
                    info!(
                        "[{}] Shutdown signal received during reconnect wait",
                        ctx.stream_id
                    );
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(
                    ctx.config.reconnect_delay_ms(reconnect_count),
                )) => {}
            }
        }

        Self::emit_state_change(
            &ctx.state,
            &ctx.state_tx,
            StreamSourceState::Disconnected,
            None,
        )
        .await;
        info!("[{}] Task exited", ctx.stream_id);
    }

    #[cfg(not(feature = "source"))]
    async fn run_rtsp_client(
        _ctx: RtspClientContext,
        _shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        // Placeholder for non-source builds
    }

    async fn receive_rtp_loop(
        stream_id: &str,
        rx: &mut tokio::sync::mpsc::Receiver<(u8, Vec<u8>)>,
        rtp_tx: &broadcast::Sender<MediaPacket>,
        shutdown_rx: &mut tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        let mut packet_count = 0u64;

        loop {
            tokio::select! {
                _ = &mut *shutdown_rx => {
                    info!("[{}] Shutdown requested", stream_id);
                    return Ok(());
                }
                result = rx.recv() => {
                    match result {
                        Some((channel, data)) => {
                            packet_count += 1;

                            let packet = MediaPacket::Rtp {
                                channel,
                                data: data.into(),
                            };

                            if rtp_tx.send(packet).is_err() {
                                // No subscribers, suppress warning
                            }

                            if packet_count.is_multiple_of(1000){
                                trace!(
                                    "[{}] Received {} packets",
                                    stream_id,
                                    packet_count
                                );
                            }
                        }
                        None => {
                            error!("[{}] Channel closed", stream_id);
                            return Err(anyhow::anyhow!("Channel closed"));
                        }
                    }
                }
            }
        }
    }

    async fn emit_state_change(
        state: &Arc<std::sync::RwLock<StreamSourceState>>,
        state_tx: &broadcast::Sender<StateChangeEvent>,
        new_state: StreamSourceState,
        error: Option<String>,
    ) {
        let event = {
            let mut s = state.write().unwrap();
            let old_state = *s;
            *s = new_state;

            StateChangeEvent {
                old_state,
                new_state,
                error,
            }
        };

        let _ = state_tx.send(event);
    }

    #[cfg(feature = "source")]
    fn video_codec_to_rtc(codec: &rtsp::VideoCodecParams) -> RTCRtpCodecParameters {
        crate::rtsp_codec::video_codec_to_rtc(codec)
    }
}

#[async_trait]
impl StreamSource for RtspSource {
    fn stream_id(&self) -> &str {
        &self.config.stream_id
    }

    fn state(&self) -> StreamSourceState {
        *self.state.read().unwrap()
    }

    async fn start(&mut self) -> Result<()> {
        if self.task_handle.is_some() {
            anyhow::bail!("Source already started");
        }

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        #[cfg(feature = "source")]
        let ctx = RtspClientContext {
            stream_id: self.config.stream_id.clone(),
            rtsp_url: self.rtsp_url.clone(),
            config: self.config.clone(),
            rtp_tx: self.rtp_tx.clone(),
            state: self.state.clone(),
            state_tx: self.state_tx.clone(),
            media_info_store: self.media_info.clone(),
            rtcp_tx_store: self.rtcp_tx.clone(),
        };

        #[cfg(not(feature = "source"))]
        let ctx = RtspClientContext {
            stream_id: self.config.stream_id.clone(),
            rtsp_url: self.rtsp_url.clone(),
            config: self.config.clone(),
            rtp_tx: self.rtp_tx.clone(),
            state: self.state.clone(),
            state_tx: self.state_tx.clone(),
        };

        let handle = tokio::spawn(async move {
            Self::run_rtsp_client(ctx, shutdown_rx).await;
        });

        self.task_handle = Some(handle);

        info!("[{}] Started", self.config.stream_id);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }

        self.set_state(StreamSourceState::Disconnected, None).await;

        info!("[{}] Stopped", self.config.stream_id);
        Ok(())
    }

    fn subscribe_rtp(&self) -> broadcast::Receiver<MediaPacket> {
        self.rtp_tx.subscribe()
    }

    fn subscribe_state(&self) -> broadcast::Receiver<StateChangeEvent> {
        self.state_tx.subscribe()
    }

    #[cfg(feature = "source")]
    async fn get_video_codec(&self) -> Option<RTCRtpCodecParameters> {
        if let Ok(media_info) = self.media_info.try_read()
            && let Some(ref info) = *media_info
            && let Some(ref video_codec) = info.video_codec
        {
            return Some(Self::video_codec_to_rtc(video_codec));
        }

        None
    }

    #[cfg(feature = "source")]
    async fn get_audio_codec(&self) -> Option<RTCRtpCodecParameters> {
        if let Ok(media_info) = self.media_info.try_read()
            && let Some(ref info) = *media_info
            && let Some(ref audio_codec) = info.audio_codec
        {
            return Some(crate::rtsp_codec::audio_codec_to_rtc(audio_codec));
        }

        None
    }

    #[cfg(feature = "source")]
    async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        self.get_rtcp_sender().await
    }
}
