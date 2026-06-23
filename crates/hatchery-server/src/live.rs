//! Live graph-event channel (WebSocket). The server broadcasts opaque JSON
//! events (`node_added`, `dedup`, `ai_step`, `subject_changed`, `changed`,
//! `scenario`) and the SPA applies them (mostly: re-fetch `/api/graph`). One
//! producer (the mutation paths), many subscribers (open browser tabs).

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle(socket, state))
}

async fn handle(mut socket: WebSocket, state: AppState) {
    let mut rx = state.tx.subscribe();
    let _ = socket
        .send(Message::Text(json!({ "type": "hello" }).to_string().into()))
        .await;

    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(v) => {
                    if socket.send(Message::Text(v.to_string().into())).await.is_err() {
                        break;
                    }
                }
                // We dropped behind the broadcast buffer — tell the client to resync.
                Err(RecvError::Lagged(_)) => {
                    let _ = socket
                        .send(Message::Text(json!({ "type": "changed" }).to_string().into()))
                        .await;
                }
                Err(RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(_)) => { /* ignore client chatter */ }
                _ => break,
            },
        }
    }
}
