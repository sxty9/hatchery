//! hatchery-server — a visual probe & benchmark service for the lakearch data
//! model. It embeds ONE `LakearchKernel` in-process (the lakearch instance under
//! test) and exposes it through a graph API + live channel + an AI Traverser that
//! turns natural language into kernel writes. hatchery is the §1.5 layer above
//! lakearch: it computes/places/decides; lakearch stores/traverses/matches.

mod ai;
mod api;
mod live;
mod roles;
mod scenario;
mod state;
mod util;
mod viz_model;
mod vocab;

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post};
use axum::Router;
use base64::Engine;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;

use crate::state::{AiConfig, AppState, SessionManager};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cfg = Config::from_args();
    std::fs::create_dir_all(&cfg.data_dir)?;

    let (tx, _rx) = tokio::sync::broadcast::channel(2048);

    // Each session is its own lakearch bestand under <data-dir>/sessions. Stale
    // instances from a previous run are wiped (sessions are ephemeral).
    let sessions_base = std::path::Path::new(&cfg.data_dir).join("sessions");
    let _ = std::fs::remove_dir_all(&sessions_base);
    std::fs::create_dir_all(&sessions_base)?;
    let sessions = Arc::new(SessionManager::new(sessions_base, tx.clone()));
    sessions
        .create(Some("Session 1".to_string()))
        .map_err(|e| anyhow::anyhow!("could not open initial session: {e}"))?;
    tracing::info!(dir = %cfg.data_dir, "session manager ready (one bestand per session, §2.3)");

    let api_key = std::env::var("ANTHROPIC_API_KEY").ok().or_else(|| {
        std::fs::read_to_string("/etc/hatchery/anthropic-key")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    });
    if api_key.is_some() {
        tracing::info!("AI Traverser enabled (Claude key present)");
    } else {
        tracing::warn!("no ANTHROPIC_API_KEY — /api/chat will return an error; scenarios & manual appends still work");
    }
    let ai = AiConfig {
        api_key,
        model: std::env::var("HATCHERY_MODEL").unwrap_or_else(|_| "claude-sonnet-4-6".to_string()),
        max_continuations: 16,
    };

    // Basic Auth password for a public (sxgate preview) deployment. From
    // HATCHERY_PASSWORD or the file /etc/hatchery/password. Unset ⇒ open.
    let password = std::env::var("HATCHERY_PASSWORD")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hatchery/password")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });
    if password.is_some() {
        tracing::info!("HTTP Basic Auth enabled (all routes except /healthz)");
    } else {
        tracing::warn!("no HATCHERY_PASSWORD — server is OPEN to anyone who can reach it");
    }

    let state = AppState {
        sessions,
        tx,
        ai: Arc::new(ai),
        password,
        semantics_dir: cfg.semantics_dir.clone(),
    };

    let app = Router::new()
        .route("/api/graph", get(api::graph))
        .route("/api/node/{id}", get(api::node))
        .route("/api/append/leaf", post(api::append_leaf))
        .route("/api/append/node", post(api::append_node))
        .route("/api/metrics", get(api::metrics))
        .route("/api/subject", post(api::set_subject))
        .route("/api/reset", post(api::reset_view))
        .route("/api/chat", post(ai::chat))
        .route("/api/scenarios", get(scenario::list))
        .route("/api/scenario/{id}", post(scenario::run))
        .route("/api/spec", get(api::spec_list))
        .route("/api/spec/{id}", get(api::spec_get))
        .route("/api/sessions", get(api::sessions_list).post(api::sessions_create))
        .route("/api/sessions/{id}", delete(api::sessions_delete))
        .route("/api/sessions/{id}/reset", post(api::sessions_reset))
        .route("/healthz", get(|| async { "ok" }))
        .route("/ws", any(live::ws_handler))
        .fallback_service(ServeDir::new(&cfg.frontend_dir))
        .layer(CorsLayer::permissive())
        .layer(middleware::from_fn_with_state(state.clone(), basic_auth))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cfg.addr).await?;
    tracing::info!(addr = %cfg.addr, frontend = %cfg.frontend_dir, "hatchery listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// HTTP Basic Auth gate. When a password is configured, every request except the
/// `/healthz` readiness probe must present it. This is how a public sxgate preview
/// is protected (auth is the service's concern). The browser caches the credential
/// per origin, so same-origin `/api/*` and the `/ws` upgrade carry it too.
async fn basic_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.password.clone() else {
        return next.run(req).await;
    };
    if req.uri().path() == "/healthz" {
        return next.run(req).await;
    }
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Basic "))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .and_then(|d| String::from_utf8(d).ok())
        .map(|s| s.split_once(':').map(|(_, p)| p.to_string()).unwrap_or_default())
        .map(|pass| pass == expected)
        .unwrap_or(false);
    if ok {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"hatchery\"")],
            "authentication required",
        )
            .into_response()
    }
}

struct Config {
    data_dir: String,
    addr: String,
    frontend_dir: String,
    semantics_dir: String,
}

impl Config {
    fn from_args() -> Self {
        let mut data_dir = "./hatchery-data".to_string();
        let mut addr = "127.0.0.1:8799".to_string();
        let mut frontend_dir = "frontend/dist".to_string();
        let mut semantics_dir = "/home/nanu/lakearch/semantics".to_string();
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            match a.as_str() {
                "--data-dir" => data_dir = args.next().unwrap_or(data_dir),
                "--addr" => addr = args.next().unwrap_or(addr),
                "--frontend" => frontend_dir = args.next().unwrap_or(frontend_dir),
                "--semantics-dir" => semantics_dir = args.next().unwrap_or(semantics_dir),
                other => tracing::warn!(arg = other, "ignoring unknown argument"),
            }
        }
        Config { data_dir, addr, frontend_dir, semantics_dir }
    }
}
