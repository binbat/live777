use anyhow::{Error, anyhow};
use http::header;
use reqwest::header::HeaderMap;
use tracing::{error, trace};

use api::request::Cascade;

use crate::store::Server;

pub async fn cascade_push(
    public: String,
    client: reqwest::Client,
    server_src: Server,
    server_dst: Server,
    stream: String,
) -> Result<(), Error> {
    let mut headers = HeaderMap::new();
    headers.append(header::CONTENT_TYPE, "application/json".parse().unwrap());
    let url = format!("{}{}", server_src.url, api::path::cascade(&stream));
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
    headers.append(header::CONTENT_TYPE, "application/json".parse().unwrap());

    let url = format!("{}{}", server_dst.url, api::path::cascade(&stream));

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
