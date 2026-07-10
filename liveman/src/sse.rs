use api::response::Stream;
use http::header;
use reqwest::header::{HeaderMap, HeaderValue};
use tokio::io::AsyncBufReadExt;
use tokio_stream::StreamExt;
use tokio_util::io::StreamReader;
use tracing::{debug, error, warn};

use crate::store::Storage;

pub async fn subscribe_streams(base_url: String, token: String, alias: String, storage: Storage) {
    let url = format!(
        "{}{}",
        base_url.trim_end_matches('/'),
        api::path::streams_sse()
    );
    let client = reqwest::Client::new();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
    );

    loop {
        match client.get(&url).headers(headers.clone()).send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!(
                        alias,
                        status = %resp.status(),
                        "sse subscribe failed"
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }

                let stream = resp
                    .bytes_stream()
                    .map(|result| result.map_err(std::io::Error::other));
                let reader = StreamReader::new(stream);
                let mut lines = reader.lines();
                let mut data_buf = String::new();

                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => {
                            if line.is_empty() {
                                if !data_buf.is_empty() {
                                    match serde_json::from_str::<Vec<Stream>>(&data_buf) {
                                        Ok(streams) => {
                                            debug!(
                                                alias,
                                                count = streams.len(),
                                                "sse streams update"
                                            );
                                            if let Err(e) =
                                                update_storage(&storage, &alias, streams).await
                                            {
                                                error!(alias, error = ?e, "sse storage update failed");
                                            }
                                        }
                                        Err(e) => {
                                            warn!(alias, error = ?e, data = %data_buf, "sse parse failed");
                                        }
                                    }
                                    data_buf.clear();
                                }
                            } else if let Some(value) = line.strip_prefix("data:") {
                                let value = value.trim_start_matches(' ');
                                if !data_buf.is_empty() {
                                    data_buf.push('\n');
                                }
                                data_buf.push_str(value);
                            }
                        }
                        Ok(None) => {
                            warn!(alias, "sse stream closed");
                            break;
                        }
                        Err(e) => {
                            warn!(alias, error = ?e, "sse read error");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                warn!(alias, error = ?e, "sse request error");
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

pub async fn update_storage(
    storage: &Storage,
    alias: &str,
    streams: Vec<Stream>,
) -> crate::result::Result<()> {
    storage.clear_alias(alias).await?;

    storage.info_put(alias.to_string(), streams.clone()).await?;
    for stream in streams {
        storage
            .stream_put(stream.id.clone(), alias.to_string())
            .await?;
        for session in stream.subscribe.sessions {
            storage
                .session_put(
                    api::path::session(&stream.id, &session.id),
                    alias.to_string(),
                )
                .await?;
        }
    }
    Ok(())
}
