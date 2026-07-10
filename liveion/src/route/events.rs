use std::convert::Infallible;

use axum::Router;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::route::AppState;

pub fn route() -> Router<AppState> {
    Router::new().route(api::path::events_sse(), get(events_sse))
}

async fn events_sse(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let recv = state.stream_manager.subscribe_event();
    let stream = BroadcastStream::new(recv).filter_map(|result| {
        let event = result.ok()?;
        let body = api::event::EventBody {
            metrics: crate::metrics::node_metrics(),
            event: event.convert_api_event(),
        };
        let data = serde_json::to_string(&body).ok()?;
        Some(Ok(Event::default().data(data)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
