//! Shared application state and kernel-access helpers.
//!
//! hatchery embeds **one** `LakearchKernel` in-process (the lakearch instance under
//! test). The kernel's mutations take `&self` and are serialized by its internal
//! `RwLock`, so we hold an `Arc<Kernel>` and run every read *and* write on the
//! blocking pool (`spawn_blocking`) — the async axum edge stays responsive while
//! the sync kernel does the work (mirrors `lakearchd::Bestand`'s sync-core /
//! async-edge split). hatchery is the **§1.5 layer above lakearch**: it computes,
//! places, and decides; lakearch only stores, traverses, matches.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use lakearch_core::{
    gate, Capability, ContentId, Datum, Kernel as _, KernelError, LakearchKernel, RedbEdgeIndex,
    SnapshotToken,
};
use serde_json::json;
use tokio::sync::broadcast;

/// The concrete in-process kernel (default redb engine), exactly as `lakearchd`
/// uses it.
pub type Kernel = LakearchKernel<RedbEdgeIndex>;

/// Configuration for the AI Traverser (the §1.5 compute layer driven by Claude).
#[derive(Clone)]
pub struct AiConfig {
    pub api_key: Option<String>,
    pub model: String,
    pub max_continuations: usize,
}

/// The shared, cheaply-clonable application state.
#[derive(Clone)]
pub struct AppState {
    pub kernel: Arc<Kernel>,
    /// Live graph-event bus (WS). Carries opaque JSON events the SPA applies.
    pub tx: broadcast::Sender<serde_json::Value>,
    /// Areas (§11.1) created through the API/scenarios/AI — granted to the default
    /// "admin" projection so the operator sees everything they created. Subject
    /// views derive their own scopes from permissions via `authorize_subject`.
    pub known_areas: Arc<Mutex<HashSet<ContentId>>>,
    /// The active read subject (§11): `None` ⇒ admin projection (all known areas
    /// granted); `Some(id)` ⇒ that subject's structurally-derived scopes (drives
    /// VANISH, §11.3).
    pub active_subject: Arc<Mutex<Option<ContentId>>>,
    pub ai: Arc<AiConfig>,
    /// Optional HTTP Basic Auth password. When set, every request except
    /// `/healthz` must present it (this is how a public sxgate preview is gated —
    /// auth is the service's concern, §preview). `None` ⇒ open.
    pub password: Option<String>,
    /// Directory holding the lakearch spec markdown (the "Gesetzbuch") shown in
    /// the UI: `lakearch.md` + `canonical-encoding.md`.
    pub semantics_dir: String,
}

impl AppState {
    /// Broadcast a live graph event to all connected SPA clients (best-effort).
    pub fn emit(&self, event: serde_json::Value) {
        let _ = self.tx.send(event);
    }

    pub fn register_area(&self, area: ContentId) {
        if let Ok(mut g) = self.known_areas.lock() {
            g.insert(area);
        }
    }

    pub fn snapshot_subject(&self) -> Option<ContentId> {
        self.active_subject.lock().ok().and_then(|g| *g)
    }

    /// Run a read closure on the blocking pool against the shared kernel.
    pub async fn read<T, F>(&self, f: F) -> Result<T, KernelError>
    where
        F: FnOnce(&Kernel) -> Result<T, KernelError> + Send + 'static,
        T: Send + 'static,
    {
        let kernel = Arc::clone(&self.kernel);
        tokio::task::spawn_blocking(move || f(&kernel))
            .await
            .map_err(|_| KernelError::Poisoned)?
    }

}

/// Decode a single datum *through the gate* (§11): returns `None` on VANISH or
/// absence (indistinguishable, §11.3), otherwise the strict-decoded `Datum`.
pub fn decode_visible(
    kernel: &Kernel,
    id: ContentId,
    cap: &Capability,
    snap: SnapshotToken,
) -> Result<Option<Datum>, KernelError> {
    match kernel.get_by_content_id(id, cap, snap)? {
        None => Ok(None),
        Some(sealed) => match gate::open(&sealed, cap) {
            Some(visible) => Ok(Some(lakearch_core::strict_decode(visible.canonical_bytes())?)),
            None => Ok(None),
        },
    }
}

/// Append a datum and report whether it deduplicated (§5.3) — determined from the
/// pinned snapshot's active set (race-free, unlike reading the global counter).
pub fn append_tracked(kernel: &Kernel, datum: &Datum) -> Result<(ContentId, bool), KernelError> {
    let id = ContentId::of_datum(datum);
    // `content_set()` is sorted ascending (address order) — binary_search is valid.
    let existed = kernel.content_set()?.binary_search(&id).is_ok();
    let written = kernel.append(datum)?;
    debug_assert_eq!(written, id, "of_datum must match the kernel's content id (§K5)");
    Ok((written, existed))
}

/// Anything fallible in a handler maps to a 500 with a visibility-blind message
/// (§11.3: never name concrete data/ids/areas in errors).
pub struct AppError(pub anyhow::Error);

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(e: E) -> Self {
        AppError(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::warn!(error = %self.0, "request failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}
