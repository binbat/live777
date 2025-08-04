use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use tokio::time;
use tracing::{debug, error, info};

use crate::service::recording_sessions::RecordingSessionsService;
use crate::store::Server;
use crate::AppState;
use api::recorder::PullRecordingsRequest;

/// Configuration for recording session pulling
#[derive(Debug, Clone)]
pub struct PullConfig {
    /// Interval between pulls in seconds
    pub pull_interval: u64,
    /// Maximum sessions to pull per request
    pub pull_limit: u32,
    /// Whether to enable session pulling
    pub enabled: bool,
}

impl Default for PullConfig {
    fn default() -> Self {
        Self {
            pull_interval: 30, // 30 seconds
            pull_limit: 100,
            enabled: true,
        }
    }
}

/// Track last pulled timestamp for each node
type LastPulledMap = HashMap<String, i64>;

/// Start the recording session pulling background task
pub async fn start_recording_puller(mut state: AppState, config: PullConfig) {
    if !config.enabled {
        info!("[recording-puller] Recording session pulling is disabled");
        return;
    }

    info!(
        "[recording-puller] Starting recording session puller with interval: {}s, limit: {}",
        config.pull_interval, config.pull_limit
    );

    let mut interval = time::interval(Duration::from_secs(config.pull_interval));
    let mut last_pulled: LastPulledMap = HashMap::new();

    loop {
        interval.tick().await;

        debug!("[recording-puller] Starting pull cycle");

        let nodes = state.storage.nodes().await;
        if nodes.is_empty() {
            debug!("[recording-puller] No nodes available, skipping pull cycle");
            continue;
        }

        for node in nodes {
            if let Err(e) = pull_from_node(&state, &node, &config, &mut last_pulled).await {
                error!(
                    "[recording-puller] Failed to pull recording sessions from node {}: {}",
                    node.alias, e
                );
            }
        }

        debug!("[recording-puller] Pull cycle completed");
    }
}

async fn pull_from_node(
    state: &AppState,
    node: &Server,
    config: &PullConfig,
    last_pulled: &mut LastPulledMap,
) -> Result<()> {
    let node_key = node.alias.clone();
    let since_ts = last_pulled.get(&node_key).copied();

    debug!(
        "[recording-puller] Pulling recording sessions from node {} since timestamp {:?}",
        node.alias, since_ts
    );

    // Build pull request
    let request = PullRecordingsRequest {
        stream: None, // Pull all streams
        since_ts,
        limit: config.pull_limit,
    };

    // Make HTTP request to the Live777 node
    let url = format!("{}{}", node.url, api::path::recordings_pull());
    let response = state.client.get(&url).query(&request).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "HTTP {} from node {}: {}",
            status,
            node.alias,
            body
        ));
    }

    let pull_response: api::recorder::PullRecordingsResponse = response.json().await?;

    if pull_response.sessions.is_empty() {
        debug!(
            "[recording-puller] No new recording sessions from node {}",
            node.alias
        );
        return Ok(());
    }

    info!(
        "[recording-puller] Pulled {} recording sessions from node {}",
        pull_response.sessions.len(),
        node.alias
    );

    // Store sessions in database
    match RecordingSessionsService::create_sessions_from_pull(
        state.database.get_connection(),
        node.alias.clone(),
        pull_response.sessions,
    )
    .await
    {
        Ok(created_sessions) => {
            info!(
                "[recording-puller] Successfully stored {} recording sessions from node {}",
                created_sessions.len(),
                node.alias
            );

            // Update last pulled timestamp
            if let Some(last_ts) = pull_response.last_ts {
                last_pulled.insert(node_key, last_ts);
            }
        }
        Err(e) => {
            error!(
                "[recording-puller] Failed to store recording sessions from node {}: {}",
                node.alias, e
            );
        }
    }

    Ok(())
}
