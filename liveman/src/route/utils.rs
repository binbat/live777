use std::time::Duration;

use anyhow::{anyhow, Error};
use reqwest::header::HeaderMap;
use tracing::{debug, error, info, trace, warn};

use api::{
    request::Cascade,
    response::{RTCPeerConnectionState, Stream},
};

use crate::store::Server;

pub async fn force_check_times(
    client: reqwest::Client,
    server: Server,
    stream: String,
    count: u8,
) -> Result<u8, Error> {
    for i in 0..count {
        let timeout = tokio::time::sleep(Duration::from_millis(1000));
        tokio::pin!(timeout);
        let _ = timeout.as_mut().await;
        match force_check(client.clone(), server.clone(), stream.clone()).await {
            Ok(()) => return Ok(i),
            Err(e) => warn!("force_check failed {:?}", e),
        };
    }
    Err(anyhow!("reforward check failed"))
}

async fn force_check(client: reqwest::Client, server: Server, stream: String) -> Result<(), Error> {
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

pub async fn cascade_push(
    public: String,
    client: reqwest::Client,
    server_src: Server,
    server_dst: Server,
    stream: String,
) -> Result<(), Error> {
    let mut headers = HeaderMap::new();
    headers.append("Content-Type", "application/json".parse().unwrap());
    let url = format!("{}{}", server_src.url, &api::path::cascade(&stream));
    let body = serde_json::to_string(&Cascade {
        target_url: Some(format!(
            "{}{}",
            public,
            api::path::whip_with_node(&stream, &server_dst.alias)
        )),
        token: None,
        source_url: None,
    })
    .unwrap();
    trace!("{:?}", body);

    let response = client
        .post(url.clone())
        .headers(headers)
        .body(body)
        .send()
        .await?;

    if response.status().is_success() {
        Ok(())
    } else {
        error!(
            "url: {:?}, [{:?}], response: {:?}",
            url,
            response.status(),
            response.text().await?
        );
        Err(anyhow!("http status not success"))
    }
}

pub async fn session_delete(
    client: reqwest::Client,
    server: Server,
    stream: String,
    session: String,
) -> Result<(), Error> {
    let url = format!("{}/session/{}/{}", server.url, stream, session);

    let response = client.delete(url).send().await?;

    if response.status().is_success() {
        Ok(())
    } else {
        error!("{:?} {:?}", response.status(), response.text().await?);
        Err(anyhow!("http status not success"))
    }
}
pub async fn cascade_pull(
    client: reqwest::Client,
    server_src: Server,
    server_dst: Server,
    stream: String,
) -> Result<(), Error> {
    let mut headers = HeaderMap::new();
    headers.append("Content-Type", "application/json".parse().unwrap());

    let url = format!("{}{}", server_dst.url, &api::path::cascade(&stream));

    let body = serde_json::to_string(&Cascade {
        source_url: Some(format!("{}/whep/{}", server_src.url, stream)),
        token: Some(server_src.token.clone()),
        target_url: None,
    })
    .unwrap();

    trace!("cascade pull request: {:?}", body);

    let response = client
        .post(url.clone())
        .headers(headers)
        .body(body)
        .send()
        .await?;

    if response.status().is_success() {
        Ok(())
    } else {
        error!(
            "url: {:?}, [{:?}], response: {:?}",
            url,
            response.status(),
            response.text().await?
        );
        Err(anyhow!("http status not success"))
    }
}
