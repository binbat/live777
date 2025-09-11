use anyhow::{Error, anyhow};
use socks5_server::{
    Command, IncomingConnection,
    connection::connect::Connect,
    connection::connect::state::Ready,
    connection::state::NeedAuthenticate,
    proto::{Address, Reply},
};
use tokio::io::AsyncWriteExt;

pub(crate) async fn handle(
    conn: IncomingConnection<(), NeedAuthenticate>,
    domain: Option<String>,
) -> Result<(Option<String>, Option<String>, Connect<Ready>), Error> {
    let conn = match conn.authenticate().await {
        Ok((conn, _)) => conn,
        Err((err, mut conn)) => {
            let _ = conn.shutdown().await;
            return Err(anyhow!(err));
        }
    };

    match conn.wait().await {
        Ok(Command::Associate(associate, _)) => {
            let replied = associate
                .reply(Reply::CommandNotSupported, Address::unspecified())
                .await;

            let mut conn = match replied {
                Ok(conn) => conn,
                Err((err, mut conn)) => {
                    let _ = conn.shutdown().await;
                    return Err(anyhow!(err));
                }
            };

            let _ = conn.close().await;
        }
        Ok(Command::Bind(bind, _)) => {
            let replied = bind
                .reply(Reply::CommandNotSupported, Address::unspecified())
                .await;

            let mut conn = match replied {
                Ok(conn) => conn,
                Err((err, mut conn)) => {
                    let _ = conn.shutdown().await;
                    return Err(anyhow!(err));
                }
            };

            let _ = conn.close().await;
        }
        Ok(Command::Connect(connect, addr)) => {
            let (id, target) = match addr {
                Address::DomainAddress(domain_address, _port) => {
                    match std::str::from_utf8(&domain_address) {
                        Ok(raw) => {
                            if let Some(d) = domain {
                                if raw.ends_with(&d) {
                                    (Some(crate::kxdns::Kxdns::resolver(raw).to_string()), None)
                                } else {
                                    (None, Some(raw.to_string()))
                                }
                            } else {
                                (None, Some(raw.to_string()))
                            }
                        }
                        Err(_) => (None, None),
                    }
                }
                Address::SocketAddress(ip) => (None, Some(ip.to_string())),
            };

            let replied = connect
                .reply(Reply::Succeeded, Address::unspecified())
                .await;

            let conn = match replied {
                Ok(conn) => conn,
                Err((err, mut conn)) => {
                    let _ = conn.shutdown().await;
                    return Err(anyhow!(err));
                }
            };
            return Ok((id, target, conn));
        }
        Err((err, mut conn)) => {
            let _ = conn.shutdown().await;
            return Err(anyhow!(err));
        }
    }
    Err(anyhow!("unknown error"))
}
