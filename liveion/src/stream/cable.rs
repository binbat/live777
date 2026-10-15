//! Per-stream datachannel cable topology (`[[stream.<name>.cables]]`).
//!
//! A cable is a named, bidirectional message pipe: every datachannel whose
//! label matches the cable's name — on the publisher peer and on any
//! subscriber peer alike — is bridged onto the cable's two broadcast buses:
//! `to_publisher` (messages towards the publisher) and `to_subscribers`
//! (towards all subscribers). Channels whose label matches no declared cable
//! fall back to the anonymous default pair, preserving the pre-cable
//! everything-mixes behavior.
//!
//! The hub is owned by the stream (via `stream::manager::Manager`), not by
//! the forward: a provisioned-stream reset recreates the forward but the
//! hub — buses, UDP bridges and their listen ports — survives untouched, so
//! a reset never races a bridge rebind. Bridges are cancelled when the hub
//! is dropped (stream deleted).

use std::collections::HashMap;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::new_broadcast_channel;

/// Broadcast capacity of the anonymous default bus pair, in messages.
/// Declared cables carry their own `capacity` (same default).
pub(crate) const DEFAULT_CABLE_BUS_CAPACITY: usize = 1024;

/// One cable's (or the default pair's) two direction buses.
#[derive(Clone)]
pub(crate) struct CableBus {
    /// Messages towards the publisher (produced by subscribers).
    pub(crate) to_publisher: broadcast::Sender<Vec<u8>>,
    /// Messages towards all subscribers (produced by the publisher).
    pub(crate) to_subscribers: broadcast::Sender<Vec<u8>>,
}

impl CableBus {
    fn new(capacity: usize) -> Self {
        Self {
            to_publisher: new_broadcast_channel!(capacity),
            to_subscribers: new_broadcast_channel!(capacity),
        }
    }
}

/// A declared cable: its buses plus the declaring config (channel parameters
/// for server-side creation, UDP bridges).
pub(crate) struct Cable {
    pub(crate) bus: CableBus,
    pub(crate) config: crate::config::CableConfig,
}

/// The stream-level owner of all cable state: the anonymous default bus
/// pair, every declared cable's buses, and the cable <-> UDP bridges.
pub(crate) struct CableHub {
    default: CableBus,
    cables: HashMap<String, Cable>,
    /// Cancels every UDP bridge of this stream when the hub is dropped:
    /// bridges must not outlive the stream, but equally must not churn when
    /// the forward is reset underneath them.
    cancel: CancellationToken,
    /// Running bridge tasks keyed by (cable label, udp index), so a
    /// partially-failed start retries only the bridges that never came up on
    /// a later `start_bridges` call (e.g. after a provisioned-stream reset
    /// frees a held port) instead of re-spawning the healthy ones. tokio
    /// Mutex: the guard is held across bridge spawns (which await), so it
    /// must stay Send to keep the HTTP handler futures Send.
    bridges: tokio::sync::Mutex<HashMap<(String, usize), crate::forward::channel::ChannelHandle>>,
}

impl CableHub {
    /// Anonymous pair only (no declared cables).
    pub(crate) fn bare() -> Self {
        Self {
            default: CableBus::new(DEFAULT_CABLE_BUS_CAPACITY),
            cables: HashMap::new(),
            cancel: CancellationToken::new(),
            bridges: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Anonymous pair plus one cable per declared config.
    pub(crate) fn from_configs(configs: &[crate::config::CableConfig]) -> Self {
        let mut hub = Self::bare();
        hub.cables = configs
            .iter()
            .map(|c| {
                (
                    c.label.clone(),
                    Cable {
                        bus: CableBus::new(c.capacity),
                        config: c.clone(),
                    },
                )
            })
            .collect();
        hub
    }

    /// The bus pair a channel with `label` joins: its declared cable, or the
    /// anonymous default pair.
    pub(crate) fn route(&self, label: &str) -> &CableBus {
        self.cables
            .get(label)
            .map(|c| &c.bus)
            .unwrap_or(&self.default)
    }

    /// The declared config for `label`, if any.
    pub(crate) fn declared(&self, label: &str) -> Option<&crate::config::CableConfig> {
        self.cables.get(label).map(|c| &c.config)
    }

    /// Declared cables whose channels the server creates on every session.
    pub(crate) fn server_created(&self) -> impl Iterator<Item = &crate::config::CableConfig> + '_ {
        self.cables.values().map(|c| &c.config).filter(|c| c.create)
    }

    /// Start every configured UDP bridge. Idempotent per bridge: a bridge
    /// that already started successfully (e.g. after a provisioned-stream
    /// reset reuses the hub) is left alone, while any bridge that failed to
    /// start is retried on the next call. Per-bridge failures are logged and
    /// skipped — one bad bridge must not take the others down with it.
    pub(crate) async fn start_bridges(&self, stream: &str) {
        let mut bridges = self.bridges.lock().await;
        for cable in self.cables.values() {
            for (udp_index, udp) in cable.config.udp.iter().enumerate() {
                let key = (cable.config.label.clone(), udp_index);
                if bridges.contains_key(&key) {
                    // This bridge already started successfully; leave it.
                    continue;
                }
                let (dc_rx, dc_tx) = match udp.role {
                    crate::config::CableRole::Publisher => (
                        udp.target.map(|_| cable.bus.to_publisher.subscribe()),
                        udp.listen.map(|_| cable.bus.to_subscribers.clone()),
                    ),
                    crate::config::CableRole::Subscriber => (
                        udp.target.map(|_| cable.bus.to_subscribers.subscribe()),
                        udp.listen.map(|_| cable.bus.to_publisher.clone()),
                    ),
                };
                match crate::forward::channel::spawn_channel(
                    format!("{stream}/{}", cable.config.label),
                    udp,
                    dc_rx,
                    dc_tx,
                    self.cancel.clone(),
                )
                .await
                {
                    Ok(handle) => {
                        bridges.insert(key, handle);
                    }
                    Err(err) => {
                        tracing::warn!(
                            "cable bridge [{stream}/{}] failed to start: {err:?}",
                            cable.config.label
                        );
                    }
                }
            }
        }
    }
}

impl Drop for CableHub {
    fn drop(&mut self) {
        // Cancel the bridges; the tasks release their sockets on exit. The
        // exit is async, so a same-named stream recreated immediately may
        // briefly see its listen port still held — `bind_with_retry` in the
        // bridge absorbs that window.
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;

    fn cable(label: &str) -> crate::config::CableConfig {
        crate::config::CableConfig::new(label)
    }

    #[tokio::test]
    async fn route_falls_back_to_default_for_undeclared_labels() {
        let hub = CableHub::from_configs(&[cable("control")]);
        let declared = hub.route("control");
        let default = hub.route("chat");
        // Distinct bus pairs: sending on the declared cable must not appear
        // on the default pair.
        let mut declared_rx = declared.to_subscribers.subscribe();
        let mut default_rx = default.to_subscribers.subscribe();
        declared.to_subscribers.send(b"a".to_vec()).unwrap();
        assert_eq!(declared_rx.try_recv().unwrap(), b"a");
        assert!(default_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn capacity_is_per_cable() {
        let mut big = cable("big");
        big.capacity = 7;
        let hub = CableHub::from_configs(&[big, cable("small")]);

        // Overflow the big cable's bus: capacity 7, send 9 without polling
        // the receiver -> it must lag (the exact count is tokio-internal).
        let big_bus = &hub.route("big").to_publisher;
        let mut rx = big_bus.subscribe();
        for i in 0..9u8 {
            big_bus.send(vec![i]).unwrap();
        }
        let first = rx.try_recv();
        assert!(
            matches!(first, Err(broadcast::error::TryRecvError::Lagged(n)) if n >= 1),
            "expected the small-capacity bus to lag, got {first:?}"
        );

        // The default-capacity cable absorbs the same burst without lagging.
        let small_bus = &hub.route("small").to_publisher;
        let mut rx = small_bus.subscribe();
        for i in 0..9u8 {
            small_bus.send(vec![i]).unwrap();
        }
        assert_eq!(rx.try_recv().unwrap(), vec![0]);
    }

    /// A bridge whose start failed must be retried on a later `start_bridges`
    /// call, while the bridge that already started is left alone.
    #[tokio::test]
    async fn start_bridges_retries_only_the_failed_bridge() {
        // Hold a port so the first bridge's bind fails on the first pass.
        let blocker = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let blocked_addr = blocker.local_addr().unwrap();

        // The second bridge gets a free port so it starts on the first pass.
        let free_probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let free_addr = free_probe.local_addr().unwrap();
        drop(free_probe);

        let mut control = crate::config::CableConfig::new("control");
        control.udp = vec![
            crate::config::CableUdpConfig {
                role: crate::config::CableRole::Publisher,
                listen: Some(blocked_addr),
                target: Some("127.0.0.1:9".parse().unwrap()),
            },
            crate::config::CableUdpConfig {
                role: crate::config::CableRole::Publisher,
                listen: Some(free_addr),
                target: Some("127.0.0.1:9".parse().unwrap()),
            },
        ];

        let hub = CableHub::from_configs(&[control]);

        // First pass: the blocked bridge fails and is skipped; the free one
        // starts and holds its port.
        hub.start_bridges("test").await;
        assert!(
            UdpSocket::bind(free_addr).await.is_err(),
            "the healthy bridge must hold its port"
        );

        // Free the blocked port, then start again: only the failed bridge is
        // retried, and it now binds successfully.
        drop(blocker);
        hub.start_bridges("test").await;
        assert!(
            UdpSocket::bind(blocked_addr).await.is_err(),
            "the previously failed bridge must hold its port after the retry"
        );
    }
}
