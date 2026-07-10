use std::time::Duration;

use api::response::Stream;
use http::header;
use reqwest::header::{HeaderMap, HeaderValue};
use tokio::io::AsyncBufReadExt;
use tokio::select;
use tokio_stream::StreamExt;
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::store::Storage;

pub async fn subscribe_streams(
    base_url: String,
    token: String,
    alias: String,
    storage: Storage,
    cancel: CancellationToken,
) {
    let url = format!(
        "{}{}",
        base_url.trim_end_matches('/'),
        api::path::streams_sse()
    );
    let mut headers = HeaderMap::new();
    let auth_value = match HeaderValue::from_str(&format!("Bearer {}", token)) {
        Ok(v) => v,
        Err(e) => {
            error!(alias, error = ?e, "invalid sse auth token");
            return;
        }
    };
    headers.insert(header::AUTHORIZATION, auth_value);

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        // SSE is a long-lived stream; do not set a per-request timeout and rely
        // on TCP keepalive + cancellation for hang detection.
        .tcp_keepalive(Duration::from_secs(30))
        .default_headers(headers)
        .build()
        .unwrap();

    loop {
        select! {
            _ = cancel.cancelled() => {
                info!(alias, "sse subscriber cancelled");
                return;
            }
            result = client.get(&url).send() => {
                match result {
                    Ok(resp) => {
                        let status = resp.status();
                        if !status.is_success() {
                            if status == http::StatusCode::UNAUTHORIZED
                                || status == http::StatusCode::FORBIDDEN
                            {
                                error!(
                                    alias,
                                    status = %status,
                                    "sse subscribe auth failed, giving up"
                                );
                                return;
                            }
                            warn!(
                                alias,
                                status = %status,
                                "sse subscribe failed"
                            );
                            select! {
                                _ = cancel.cancelled() => {
                                    info!(alias, "sse subscriber cancelled");
                                    return;
                                }
                                _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {}
                            }
                            continue;
                        }

                        let stream = resp
                            .bytes_stream()
                            .map(|result| result.map_err(std::io::Error::other));
                        let reader = StreamReader::new(stream);
                        let mut lines = reader.lines();
                        let mut data_buf = String::new();

                        loop {
                            select! {
                                _ = cancel.cancelled() => {
                                    info!(alias, "sse subscriber cancelled");
                                    return;
                                }
                                line = lines.next_line() => {
                                    match line {
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
                                                                storage.update_snapshot(&alias, streams).await
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
                        }
                    }
                    Err(e) => {
                        warn!(alias, error = ?e, "sse request error");
                    }
                }
            }
        }
        select! {
            _ = cancel.cancelled() => {
                info!(alias, "sse subscriber cancelled");
                return;
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {}
        }
    }
}
