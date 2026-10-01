//! Multicast-aware UDP socket builders for plain-RTP/RTCP I/O.
//!
//! Shared by the RTP receiver (liveion's SDP file source, live777#465) and
//! the RTP sender (liveion's `rtp://` output target): a sender towards a
//! multicast group needs TTL / outbound-interface setsockopts, a receiver
//! needs membership joins, and both need the same interface resolution
//! rules — IPv4 groups take an interface *address*, IPv6 groups an
//! interface *index or name* (indexes are not stable across reboots; names
//! are the durable identifier).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use anyhow::Result;
use tokio::net::UdpSocket;

/// Resolve an interface name to its index for IPv6 multicast.
#[cfg(unix)]
pub fn if_name_to_index(name: &str) -> Result<u32> {
    let name_c = std::ffi::CString::new(name)
        .map_err(|_| anyhow::anyhow!("interface name contains a NUL byte"))?;
    // SAFETY: name_c is a valid NUL-terminated C string; the returned index
    // is 0 when no interface has that name.
    let index = unsafe { libc::if_nametoindex(name_c.as_ptr()) };
    if index == 0 {
        anyhow::bail!("interface '{name}' not found");
    }
    Ok(index)
}

#[cfg(not(unix))]
pub fn if_name_to_index(name: &str) -> Result<u32> {
    anyhow::bail!(
        "interface names are not supported on this platform; \
         use an interface index instead of '{name}'"
    )
}

/// Resolve an IPv6 multicast interface given as an interface index or name.
pub fn resolve_v6_interface(value: &str, group: &Ipv6Addr) -> Result<u32> {
    if let Ok(index) = value.parse::<u32>() {
        return Ok(index);
    }
    if_name_to_index(value).map_err(|e| {
        anyhow::anyhow!(
            "multicast_interface '{value}' must be an interface index or name for IPv6 group {group}: {e}"
        )
    })
}

/// Resolve an IPv4 multicast interface given as the interface's IPv4
/// address (the index/name form is the IPv6 convention).
pub fn resolve_v4_interface(value: &str, group: &Ipv4Addr) -> Result<Ipv4Addr> {
    value.parse::<Ipv4Addr>().map_err(|_| {
        anyhow::anyhow!(
            "multicast_interface '{value}' must be an IPv4 address for IPv4 group {group}"
        )
    })
}

/// The resolved interface for a multicast group, family-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MulticastInterface {
    V4(Ipv4Addr),
    V6(u32),
}

/// Resolve `value` as the interface for `group`: an IPv4 address for an
/// IPv4 group, an interface index or name for an IPv6 group.
pub fn resolve_interface(group: &IpAddr, value: &str) -> Result<MulticastInterface> {
    match group {
        IpAddr::V4(group) => resolve_v4_interface(value, group).map(MulticastInterface::V4),
        IpAddr::V6(group) => resolve_v6_interface(value, group).map(MulticastInterface::V6),
    }
}

/// Build the UDP socket sending towards `dest`: multicast TTL / outbound
/// interface options when `dest` is a group, a plain bound socket otherwise.
pub fn sender_socket(
    dest: SocketAddr,
    multicast_interface: Option<&str>,
    ttl: Option<u32>,
) -> Result<UdpSocket> {
    let domain = match dest.ip() {
        IpAddr::V4(_) => socket2::Domain::IPV4,
        IpAddr::V6(_) => socket2::Domain::IPV6,
    };
    let socket = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    socket.set_nonblocking(true)?;

    let bind_ip = match dest.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    socket
        .bind(&SocketAddr::new(bind_ip, 0).into())
        .map_err(|e| anyhow::anyhow!("Failed to bind UDP socket {bind_ip}:0: {e}"))?;

    let interface = multicast_interface.map(str::trim).filter(|s| !s.is_empty());
    match dest.ip() {
        IpAddr::V4(group) if group.is_multicast() => {
            socket.set_multicast_ttl_v4(ttl.unwrap_or(1))?;
            if let Some(interface) = interface {
                socket.set_multicast_if_v4(&resolve_v4_interface(interface, &group)?)?;
            }
        }
        IpAddr::V6(group) if group.is_multicast() => {
            socket.set_multicast_hops_v6(ttl.unwrap_or(1))?;
            if let Some(interface) = interface {
                socket.set_multicast_if_v6(resolve_v6_interface(interface, &group)?)?;
            }
        }
        _ => {}
    }

    Ok(UdpSocket::from_std(socket.into())?)
}

/// Multicast group membership for a receiver socket, e.g. derived from an
/// SDP connection address naming a group (`c=IN IP4 230.1.1.1`); unicast
/// and unspecified addresses yield no join.
#[derive(Debug, Clone, Copy)]
pub enum MulticastJoin {
    /// IPv4 group joined on the interface owning `interface`
    /// (`0.0.0.0` lets the kernel choose).
    V4 {
        group: Ipv4Addr,
        interface: Ipv4Addr,
    },
    /// IPv6 group joined on the interface with index `interface`
    /// (`0` lets the kernel choose).
    V6 { group: Ipv6Addr, interface: u32 },
}

impl std::fmt::Display for MulticastJoin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MulticastJoin::V4 { group, interface } => {
                write!(f, "{group} (interface {interface})")
            }
            MulticastJoin::V6 { group, interface } => {
                write!(f, "{group} (interface index {interface})")
            }
        }
    }
}

/// Bind a UDP receiver socket on `bind`, joining the multicast group when
/// `join` names one. Multicast receivers set SO_REUSEADDR so several
/// receivers of the same group:port can coexist on one host (a second
/// stream, a monitoring tool); SO_REUSEPORT would load-balance the
/// datagrams instead of duplicating them.
pub async fn receiver_socket(bind: SocketAddr, join: Option<MulticastJoin>) -> Result<UdpSocket> {
    let Some(join) = join else {
        return UdpSocket::bind(bind)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind UDP socket {bind}: {e}"));
    };

    let domain = match bind.ip() {
        IpAddr::V4(_) => socket2::Domain::IPV4,
        IpAddr::V6(_) => socket2::Domain::IPV6,
    };
    let socket = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket
        .bind(&bind.into())
        .map_err(|e| anyhow::anyhow!("Failed to bind UDP socket {bind}: {e}"))?;
    let socket = UdpSocket::from_std(socket.into())?;

    match join {
        MulticastJoin::V4 { group, interface } => {
            socket.join_multicast_v4(group, interface).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to join multicast group {group} (interface {interface}): {e}"
                )
            })?;
        }
        MulticastJoin::V6 { group, interface } => {
            socket.join_multicast_v6(&group, interface).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to join multicast group {group} (interface index {interface}): {e}"
                )
            })?;
        }
    }
    Ok(socket)
}

/// Bind a UDP sender socket dual-stack (`[::]:0`, `IPV6_V6ONLY` off), so a
/// unicast IPv6 destination is reachable; hosts without IPv6 fall back to
/// an IPv4 socket. The bool reports whether sends must map v4 destinations
/// to v4-mapped IPv6 (an AF_INET6 socket rejects a plain AF_INET
/// destination).
pub async fn dual_stack_sender_socket() -> Result<(UdpSocket, bool)> {
    let dual_stack = || -> Result<UdpSocket> {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        socket.set_only_v6(false)?;
        socket.set_nonblocking(true)?;
        socket.bind(&SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0).into())?;
        Ok(UdpSocket::from_std(socket.into())?)
    };
    match dual_stack() {
        Ok(socket) => Ok((socket, true)),
        Err(e) => {
            tracing::debug!("dual-stack UDP socket unavailable ({e}); falling back to IPv4");
            Ok((
                UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)).await?,
                false,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_interface_checks_the_address_family() {
        let v4_group = IpAddr::V4(Ipv4Addr::new(230, 1, 1, 1));
        // IPv4 groups take an IPv4 interface address, not a name or index.
        assert_eq!(
            resolve_interface(&v4_group, "192.168.1.10").unwrap(),
            MulticastInterface::V4(Ipv4Addr::new(192, 168, 1, 10))
        );
        for interface in ["eth0", "2"] {
            let err = resolve_interface(&v4_group, interface)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("multicast_interface"),
                "error must name multicast_interface: {err}"
            );
        }

        // IPv6 groups take an index or name, not an IPv4 address.
        let v6_group = IpAddr::V6("ff12::1".parse().unwrap());
        assert_eq!(
            resolve_interface(&v6_group, "2").unwrap(),
            MulticastInterface::V6(2)
        );
        assert!(resolve_interface(&v6_group, "192.168.1.10").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resolve_v6_interface_accepts_name_and_index() {
        // "lo" always exists on Linux; indexes are family-agnostic.
        let group = "ff12::1".parse().unwrap();
        assert!(resolve_v6_interface("lo", &group).is_ok());
        assert!(resolve_v6_interface("2", &group).is_ok());
    }

    /// The data plane: a datagram sent to the group out of the configured
    /// interface must surface on a receiver joined on loopback — what
    /// separates a real sender socket from one that merely bound, and a
    /// real membership from a bind that merely succeeded.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn multicast_sender_reaches_joined_loopback_receiver() {
        let group_ip = Ipv4Addr::new(230, 1, 1, 1);
        let group = SocketAddr::new(IpAddr::V4(group_ip), 1720);

        let receiver = receiver_socket(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 1720),
            Some(MulticastJoin::V4 {
                group: group_ip,
                interface: Ipv4Addr::LOCALHOST,
            }),
        )
        .await
        .unwrap();
        let sender = sender_socket(group, Some("127.0.0.1"), None).unwrap();

        let payload = b"livetwo-multicast-loopback";
        let mut buf = [0u8; 64];
        // Membership propagation is not instantaneous; resend until one
        // datagram makes it through.
        let received = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                sender.send_to(payload, group).await.unwrap();
                if let Ok(Ok((n, _))) = tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    receiver.recv_from(&mut buf),
                )
                .await
                {
                    break n;
                }
            }
        })
        .await
        .expect("datagram sent to the group must reach the joined receiver");
        assert_eq!(&buf[..received], payload);
    }
}
