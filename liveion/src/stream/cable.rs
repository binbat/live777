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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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

/// Identifier of a running cable endpoint bridge.
pub(crate) type BridgeId = usize;

/// Which cable seat an endpoint bridge takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seat {
    /// The broadcast (source) seat: inbound packets are broadcast to all
    /// subscribers, and subscribers' replies come back to the target.
    Source,
    /// The subscriber (target) seat: inbound packets are delivered to the
    /// publisher, and the publisher's messages come back to the target.
    /// Constructed only when the `target` feature enables DC targets; the
    /// runtime `attach_target` foundation keeps it reachable.
    #[cfg_attr(not(feature = "target"), allow(dead_code))]
    Target,
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
    /// Running endpoint bridges keyed by a monotonically increasing id, so a
    /// later dynamic API can detach them individually. tokio Mutex: the
    /// guard is held across bridge spawns (which await), so it must stay
    /// Send to keep the HTTP handler futures Send.
    bridges: tokio::sync::Mutex<HashMap<BridgeId, crate::forward::channel::ChannelHandle>>,
    /// Next bridge id; never reused within a hub's lifetime.
    next_id: AtomicUsize,
    /// Set once the stream's statically configured DC endpoints have been
    /// attached, so a provisioned-stream reset (which reuses the hub) never
    /// re-spawns them.
    static_attached: AtomicBool,
}

impl CableHub {
    /// Anonymous pair only (no declared cables).
    pub(crate) fn bare() -> Self {
        Self {
            default: CableBus::new(DEFAULT_CABLE_BUS_CAPACITY),
            cables: HashMap::new(),
            cancel: CancellationToken::new(),
            bridges: tokio::sync::Mutex::new(HashMap::new()),
            next_id: AtomicUsize::new(0),
            static_attached: AtomicBool::new(false),
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

    /// Attach a datachannel UDP endpoint at the broadcast (source) seat:
    /// inbound packets are broadcast to every subscriber, and subscribers'
    /// replies are sent to the endpoint's target.
    pub(crate) async fn attach_source(
        &self,
        stream: &str,
        label: &str,
        endpoint: &crate::config::DcUdpEndpoint,
    ) -> anyhow::Result<BridgeId> {
        self.attach(stream, label, endpoint, Seat::Source).await
    }

    /// Attach a datachannel UDP endpoint at the subscriber (target) seat:
    /// the publisher's messages are sent to the endpoint's target, and
    /// inbound packets are delivered to the publisher. This is the runtime
    /// foundation for a future dynamic DC-target API; statically configured
    /// DC targets go through [`attach_static_endpoints`].
    #[cfg_attr(not(feature = "target"), allow(dead_code))]
    pub(crate) async fn attach_target(
        &self,
        stream: &str,
        label: &str,
        endpoint: &crate::config::DcUdpEndpoint,
    ) -> anyhow::Result<BridgeId> {
        self.attach(stream, label, endpoint, Seat::Target).await
    }

    /// Detach a previously attached endpoint bridge. Idempotent: an unknown
    /// id is a no-op. This is the runtime foundation for a future dynamic
    /// DC-endpoint API.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn detach(&self, id: BridgeId) {
        if let Some(handle) = self.bridges.lock().await.remove(&id) {
            handle.stop().await;
        }
    }

    /// Attach `endpoint` to the cable with `label` at `seat`.
    async fn attach(
        &self,
        stream: &str,
        label: &str,
        endpoint: &crate::config::DcUdpEndpoint,
        seat: Seat,
    ) -> anyhow::Result<BridgeId> {
        let Some(cable) = self.cables.get(label) else {
            anyhow::bail!("cable '{label}' is not declared on stream '{stream}'");
        };
        let (dc_rx, dc_tx) = match seat {
            Seat::Source => (
                endpoint.target.map(|_| cable.bus.to_publisher.subscribe()),
                endpoint.listen.map(|_| cable.bus.to_subscribers.clone()),
            ),
            Seat::Target => (
                endpoint
                    .target
                    .map(|_| cable.bus.to_subscribers.subscribe()),
                endpoint.listen.map(|_| cable.bus.to_publisher.clone()),
            ),
        };
        let handle = crate::forward::channel::spawn_channel(
            format!("{stream}/{label}"),
            endpoint,
            dc_rx,
            dc_tx,
            self.cancel.clone(),
        )
        .await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.bridges.lock().await.insert(id, handle);
        Ok(id)
    }

    /// Attach the stream's statically configured `udp://` source/target
    /// endpoints, exactly once per hub. A provisioned-stream reset reuses the
    /// hub, so the second call is a no-op. Per-endpoint failures are logged
    /// and skipped — one bad endpoint must not take the others down with it.
    pub(crate) async fn attach_static_endpoints(
        &self,
        stream: &str,
        entry: &crate::config::StreamEntry,
    ) {
        if self.static_attached.swap(true, Ordering::Relaxed) {
            return;
        }
        // DC sources take the broadcast seat.
        for source in &entry.sources {
            let Some(url) = source.url.as_deref() else {
                continue;
            };
            if !url.to_ascii_lowercase().starts_with("udp://") {
                continue;
            }
            let Some(label) = source.cable_label.as_deref() else {
                continue;
            };
            let Ok(endpoint) = crate::config::DcUdpEndpoint::parse(url) else {
                continue;
            };
            if let Err(err) = self.attach_source(stream, label, &endpoint).await {
                tracing::warn!("dc source [{stream}/{label}] failed to attach: {err:?}");
            }
        }
        // DC targets take the subscriber seat.
        #[cfg(feature = "target")]
        for target in &entry.targets {
            if !target.url.trim().to_ascii_lowercase().starts_with("udp://") {
                continue;
            }
            let Some(label) = target.cable_label.as_deref() else {
                continue;
            };
            let Ok(endpoint) = crate::config::DcUdpEndpoint::parse(&target.url) else {
                continue;
            };
            if let Err(err) = self.attach_target(stream, label, &endpoint).await {
                tracing::warn!("dc target [{stream}/{label}] failed to attach: {err:?}");
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

    /// A source endpoint bridge holds its listen port and releases it on
    /// detach.
    #[tokio::test]
    async fn attach_source_then_detach_releases_port() {
        let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let listen = probe.local_addr().unwrap();
        drop(probe);

        let hub = CableHub::from_configs(&[cable("control")]);
        let endpoint = crate::config::DcUdpEndpoint {
            listen: Some(listen),
            target: Some("127.0.0.1:9".parse().unwrap()),
        };

        let id = hub
            .attach_source("test", "control", &endpoint)
            .await
            .unwrap();
        assert!(
            UdpSocket::bind(listen).await.is_err(),
            "bridge must hold its port"
        );

        hub.detach(id).await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while UdpSocket::bind(listen).await.is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "port still held after detach"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Attaching to an undeclared cable label fails.
    #[tokio::test]
    async fn attach_to_undeclared_cable_fails() {
        let hub = CableHub::from_configs(&[cable("control")]);
        let endpoint = crate::config::DcUdpEndpoint {
            listen: None,
            target: Some("127.0.0.1:9".parse().unwrap()),
        };
        assert!(
            hub.attach_source("test", "missing", &endpoint)
                .await
                .is_err()
        );
    }
}
