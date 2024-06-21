use std::time::Duration;

use anyhow::{anyhow, Error};
use reqwest::header::HeaderMap;
use tracing::{debug, error, info, trace, warn};

use api::{request::Reforward, response::RTCPeerConnectionState, response::Stream};

use crate::Server;

pub async fn force_check_times(server: Server, stream: String, count: u8) -> Result<u8, Error> {
    for i in 0..count {
        let timeout = tokio::time::sleep(Duration::from_millis(1000));
        tokio::pin!(timeout);
        let _ = timeout.as_mut().await;
        match force_check(server.clone(), stream.clone()).await {
            Ok(()) => return Ok(i),
            Err(e) => warn!("force_check failed {:?}", e),
        };
    }
    Err(anyhow!("reforward check failed"))
}

async fn force_check(server: Server, stream: String) -> Result<(), Error> {
    let client = reqwest::Client::new();
    let url = format!("{}{}", server.url, &api::path::streams(""));

    let response = client.get(url).send().await?;

    trace!("{:?}", response);
    let status = response.status();
    let body = &response.text().await?;
    if status.is_success() {
        let streams = serde_json::from_str::<Vec<Stream>>(body)?;
        debug!("{:?}", streams);
        return match streams.into_iter().find(|f| f.id == stream) {
            Some(stream) => match stream.publish.sessions.first() {
                Some(session) => {
                    if session.state == RTCPeerConnectionState::Connected {
                        Ok(())
                    } else {
                        Err(anyhow!("connect state is {:?}", session.state))
                    }
                }
                None => Err(anyhow!("Not Found stream publisher")),
            },
            None => Err(anyhow!("Not Found stream")),
        };
    }
    info!("{:?} {:?}", status, *body);
    Err(anyhow!("http status not success"))
}

pub async fn reforward(
    server_src: Server,
    server_dst: Server,
    stream: String,
) -> Result<(), Error> {
    let mut headers = HeaderMap::new();
    headers.append("Content-Type", "application/json".parse().unwrap());
    let client = reqwest::Client::new();
    let url = format!("{}/admin/reforward/{}", server_src.url, stream);
    let body = serde_json::to_string(&Reforward {
        target_url: format!("{}/whip/{}", server_dst.url, stream),
        admin_authorization: None,
    })
    .unwrap();
    trace!("{:?}", body);

    let response = client.post(url).headers(headers).body(body).send().await?;

    if response.status().is_success() {
        Ok(())
    } else {
        error!("{:?} {:?}", response.status(), response.text().await?);
        Err(anyhow!("http status not success"))
    }
}

pub async fn session_delete(server: Server, stream: String, session: String) -> Result<(), Error> {
    let client = reqwest::Client::new();
    let url = format!("{}/session/{}/{}", server.url, stream, session);

    let response = client.delete(url).send().await?;

    if response.status().is_success() {
        Ok(())
    } else {
        error!("{:?} {:?}", response.status(), response.text().await?);
        Err(anyhow!("http status not success"))
    }
}
