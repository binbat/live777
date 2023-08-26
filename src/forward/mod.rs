use std::io::Cursor;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::sdp::{MediaDescription, SessionDescription};

use constant::*;

use crate::forward::forward_internal::{get_peer_key, PeerForwardInternal};

pub(crate) mod constant;
mod forward_internal;

#[derive(Clone)]
pub struct PeerForward {
    anchor_lock: Arc<Mutex<()>>,
    internal: Arc<PeerForwardInternal>,
}

impl PeerForward {
    pub fn new(id: impl ToString) -> Self {
        PeerForward {
            anchor_lock: Arc::new(Mutex::new(())),
            internal: Arc::new(PeerForwardInternal::new(id)),
        }
    }

    pub fn get_id(&self) -> String {
        self.internal.id.clone()
    }

    pub async fn set_anchor(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<(RTCSessionDescription, String)> {
        if self.internal.anchor_is_some().await {
            return Err(anyhow::anyhow!("anchor is set"));
        }
        let _ = self.anchor_lock.lock();
        if self.internal.anchor_is_some().await {
            return Err(anyhow::anyhow!("anchor is set"));
        }
        let _ = check_session_description(offer.unmarshal()?)?;
        let peer = self.internal.new_peer(true).await?;
        let internal = Arc::downgrade(&self.internal);
        let pc = Arc::downgrade(&peer);
        peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            let internal = internal.upgrade();
            let pc = pc.upgrade();
            if internal.is_some() && pc.is_some() {
                let internal = internal.unwrap();
                let peer = pc.unwrap();
                tokio::spawn(async move {
                    println!(
                        "[{}] [anchor] [{}] connection state changed: {}",
                        internal.id,
                        peer.get_stats_id(),
                        s
                    );
                    match s {
                        RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                            let _ = peer.close().await;
                        }
                        RTCPeerConnectionState::Closed => {
                            let _ = internal.remove_anchor(peer).await;
                        }
                        _ => {}
                    };
                });
            }

            Box::pin(async {})
        }));
        let internal = Arc::downgrade(&self.internal);
        let pc = Arc::downgrade(&peer);
        peer.on_track(Box::new(move |track, _, _| {
            let internal = internal.upgrade();
            let peer = pc.upgrade();
            if internal.is_some() && peer.is_some() {
                let internal = internal.unwrap();
                let peer = peer.unwrap();
                tokio::spawn(async move {
                    if let Ok(_) = internal.anchor_track_up(peer, track.clone()).await {
                        let _ = internal.anchor_track_forward(track).await;
                    }
                });
            };
            Box::pin(async {})
        }));
        let description = peer_complete(offer, peer.clone()).await?;
        self.internal.set_anchor(peer.clone()).await?;
        Ok((description, get_peer_key(peer)))
    }

    pub async fn add_subscribe(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<(RTCSessionDescription, String)> {
        if !self.internal.anchor_is_some().await {
            return Err(anyhow::anyhow!("anchor is not set"));
        }
        let peer = self.internal.new_peer(false).await?;
        let internal = self.internal.clone();
        let pc = peer.clone();
        peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            let internal = Arc::downgrade(&internal);
            let pc = Arc::downgrade(&pc);
            tokio::spawn(async move {
                let internal = internal.upgrade();
                let pc = pc.upgrade();
                if internal.is_some() && pc.is_some() {
                    let internal = internal.unwrap();
                    let pc = pc.unwrap();
                    println!(
                        "[{}] [subscribe] [{}] connection state changed: {}",
                        internal.id,
                        pc.get_stats_id(),
                        s
                    );
                    match s {
                        RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                            let _ = pc.close().await;
                        }
                        RTCPeerConnectionState::Closed => {
                            let _ = internal.remove_subscribe(pc).await;
                        }
                        _ => {}
                    }
                }
            });
            Box::pin(async {})
        }));
        let _ = self.internal.add_subscribe(peer.clone()).await;
        Ok((
            peer_complete(offer, peer.clone()).await?,
            get_peer_key(peer),
        ))
    }

    pub async fn add_ice_candidate(&self, key: String, body: String, whip: bool) -> Result<()> {
        let ice_candidates = parse_ice_candidate(body)?;
        if ice_candidates.is_empty() {
            return Ok(());
        }
        self.internal
            .add_ice_candidate(key, ice_candidates, whip)
            .await
    }
}

async fn peer_complete(
    offer: RTCSessionDescription,
    peer: Arc<RTCPeerConnection>,
) -> Result<RTCSessionDescription> {
    let _ = peer.set_remote_description(offer).await?;
    let answer = peer.create_answer(None).await?;
    let mut gather_complete = peer.gathering_complete_promise().await;
    let _ = peer.set_local_description(answer).await?;
    let _ = gather_complete.recv().await;
    let description = peer
        .local_description()
        .await
        .ok_or(anyhow::anyhow!("failed to get local description"))?;
    Ok(description)
}

fn parse_ice_candidate(content: String) -> Result<Vec<RTCIceCandidateInit>> {
    let content = format!(
        "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n{}",
        content
    );
    let mut reader = Cursor::new(content);
    let session_desc = SessionDescription::unmarshal(&mut reader)?;
    let mut ice_candidates = Vec::new();
    for media_descriptions in session_desc.media_descriptions {
        let attributes = media_descriptions.attributes;
        let mid = attributes
            .iter()
            .filter(|attr| attr.key == "mid")
            .map(|attr| attr.value.clone())
            .last();
        let mid = mid
            .ok_or_else(|| anyhow::anyhow!("no mid"))?
            .ok_or_else(|| anyhow::anyhow!("no mid"))?;
        let mline_index = mid.parse::<u16>()?;
        for attr in attributes {
            if attr.is_ice_candidate() {
                if let Some(value) = attr.value {
                    ice_candidates.push(RTCIceCandidateInit {
                        candidate: value,
                        sdp_mid: Some(mid.clone()),
                        sdp_mline_index: Some(mline_index),
                        username_fragment: None,
                    });
                }
            }
        }
    }
    Ok(ice_candidates)
}

fn check_session_description(sd: SessionDescription) -> Result<Vec<MediaDescription>> {
    let mut video = false;
    let mut audio = false;
    for md in &sd.media_descriptions {
        let media = md.media_name.media.clone();
        match media.as_str() {
            VIDEO_KIND => {
                if video {
                    return Err(anyhow::anyhow!("only one video media is supported"));
                }
                video = true;
            }
            AUDIO_KIND => {
                if audio {
                    return Err(anyhow::anyhow!("only one audio media is supported"));
                }
                audio = true;
            }
            _ => {
                return Err(anyhow::anyhow!("unknown media kind: {}", media));
            }
        }
    }

    Ok(sd.media_descriptions)
}

#[cfg(test)]
mod test {
    use crate::forward::parse_ice_candidate;

    #[test]
    fn test_parse_ice_candidate() -> anyhow::Result<()> {
        let body = "a=ice-ufrag:EsAw
a=ice-pwd:P2uYro0UCOQ4zxjKXaWCBui1
m=audio 9 RTP/AVP 0
a=mid:0
a=candidate:1387637174 1 udp 2122260223 192.0.2.1 61764 typ host generation 0 ufrag EsAw network-id 1
a=candidate:3471623853 1 udp 2122194687 198.51.100.1 61765 typ host generation 0 ufrag EsAw network-id 2
a=candidate:473322822 1 tcp 1518280447 192.0.2.1 9 typ host tcptype active generation 0 ufrag EsAw network-id 1
a=candidate:2154773085 1 tcp 1518214911 198.51.100.2 9 typ host tcptype active generation 0 ufrag EsAw network-id 2
a=end-of-candidates";
        parse_ice_candidate(body.to_owned())?;
        Ok(())
    }
}
