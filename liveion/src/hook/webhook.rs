use std::{net::SocketAddr, str::FromStr, time::Duration};

use async_trait::async_trait;

use live777_http::event::{EventBody, NodeMetaData, NodeMetrics};
use reqwest::{header::HeaderMap, Client, Method};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use super::{Event, EventHook, NodeEvent};
use crate::{error::AppError, metrics, result::Result};

#[derive(Clone, Debug)]
pub struct WebHook {
    url: String,
    addr: SocketAddr,
    metadata: NodeMetaData,
    client: Client,
}

impl WebHook {
    pub fn new(url: String, addr: SocketAddr, metadata: NodeMetaData) -> Self {
        WebHook {
            url,
            addr,
            metadata,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_millis(300))
                .timeout(Duration::from_millis(500))
                .build()
                .unwrap(),
        }
    }

    async fn event_handler(&self, event: Event) -> Result<()> {
        let event = event.convert_live777_http_event(self.metadata.clone());
        let event_body = EventBody {
            addr: self.addr,
            metrics: node_metrics(),
            event,
        };
        let req_body = serde_json::to_string(&event_body)?;
        let mut headers = HeaderMap::new();
        headers.append("Content-Type", "application/json".parse().unwrap());
        match self
            .client
            .request(Method::from_str("POST")?, self.url.clone())
            .headers(headers)
            .body(req_body.clone())
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                let success = response.status().is_success();
                let res_body = response.text().await?;
                if success {
                    debug!(
                        url = self.url,
                        ?status,
                        req_body,
                        res_body,
                        "event webhook success"
                    );
                    Ok(())
                } else {
                    warn!(
                        url = self.url,
                        ?status,
                        req_body,
                        res_body,
                        "event webhook error"
                    );
                    Err(AppError::throw(res_body))
                }
            }
            Err(err) => {
                warn!(url = self.url, req_body, ?err, "event webhook error");
                Err(err.into())
            }
        }
    }
}

#[async_trait]
impl EventHook for WebHook {
    async fn hook(&self, mut event_receiver: broadcast::Receiver<Event>) {
        let mut is_down = false;
        while let Ok(event) = event_receiver.recv().await {
            if let Event::Node(NodeEvent::Down) = &event {
                is_down = true;
            };
            let _ = self.event_handler(event).await;
            if is_down {
                break;
            }
        }
    }
}

fn node_metrics() -> NodeMetrics {
    NodeMetrics {
        stream: metrics::STREAM.get() as u64,
        publish: metrics::PUBLISH.get() as u64,
        subscribe: metrics::SUBSCRIBE.get() as u64,
        reforward: metrics::REFORWARD.get() as u64,
    }
}
