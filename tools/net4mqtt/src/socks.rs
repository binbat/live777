use anyhow::{anyhow, Error};
use socks5_server::{
    connection::connect::state::Ready,
    connection::connect::Connect,
    connection::state::NeedAuthenticate,
    proto::{Address, Reply},
    Command, IncomingConnection,
};
use tokio::io::AsyncWriteExt;

pub(crate) async fn handle(
    conn: IncomingConnection<(), NeedAuthenticate>,
) -> Result<(Option<String>, Connect<Ready>), Error> {
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
            let target = match addr {
                Address::DomainAddress(domain, _port) => String::from_utf8(domain).ok(),
                Address::SocketAddress(_) => None,
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
            return Ok((target, conn));
        }
        Err((err, mut conn)) => {
            let _ = conn.shutdown().await;
            return Err(anyhow!(err));
        }
    }
    Err(anyhow!("unknown error"))
}
