//! Shared state, sessions, and kernel-access helpers.
//!
//! Each **session** holds its own in-process lakearch instance (one
//! `LakearchKernel` on its own data dir) — the UI shows sessions as tabs. The
//! kernel's mutations take `&self` and are serialized by its internal `RwLock`,
//! so reads *and* writes run on the blocking pool (`spawn_blocking`). hatchery is
//! the §1.5 layer above lakearch: it computes/places/decides; lakearch only
//! stores/traverses/matches.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

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

/// One session = one isolated lakearch bestand (§2.3) + its own view state.
pub struct Session {
    pub sid: String,
    pub title: Mutex<String>,
    pub kernel: Arc<Kernel>,
    /// On-disk instance dir (removed when the session is dropped/reset).
    pub dir: PathBuf,
    /// Areas (§11) created in this session — granted to the admin projection.
    known_areas: Mutex<HashSet<ContentId>>,
    /// Active read subject (§11): None ⇒ admin; Some ⇒ derived scopes (VANISH).
    active_subject: Mutex<Option<ContentId>>,
    /// Shared event bus; this session tags its events with its `sid`.
    tx: broadcast::Sender<serde_json::Value>,
}

impl Session {
    /// Emit a live event tagged with this session's id (clients filter by tab).
    pub fn emit(&self, mut event: serde_json::Value) {
        if let Some(obj) = event.as_object_mut() {
            obj.insert("s".to_string(), json!(self.sid));
        }
        let _ = self.tx.send(event);
    }

    pub fn title(&self) -> String {
        self.title.lock().map(|t| t.clone()).unwrap_or_default()
    }
    pub fn register_area(&self, area: ContentId) {
        if let Ok(mut g) = self.known_areas.lock() {
            g.insert(area);
        }
    }
    pub fn known_areas_vec(&self) -> Vec<ContentId> {
        self.known_areas
            .lock()
            .map(|g| g.iter().copied().collect())
            .unwrap_or_default()
    }
    pub fn snapshot_subject(&self) -> Option<ContentId> {
        self.active_subject.lock().ok().and_then(|g| *g)
    }
    pub fn set_subject(&self, s: Option<ContentId>) {
        if let Ok(mut g) = self.active_subject.lock() {
            *g = s;
        }
    }
    /// Reset the *view* (subject + admin area grants) — NOT the data (that is a
    /// session reset, which swaps in a fresh instance).
    pub fn clear_view(&self) {
        if let Ok(mut g) = self.active_subject.lock() {
            *g = None;
        }
        if let Ok(mut g) = self.known_areas.lock() {
            g.clear();
        }
    }

    /// Run a closure against this session's kernel on the blocking pool.
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

/// Owns all sessions. Tabs keep insertion order (a small Vec under a lock).
pub struct SessionManager {
    base: PathBuf,
    tx: broadcast::Sender<serde_json::Value>,
    sessions: RwLock<Vec<Arc<Session>>>,
    sid_ctr: AtomicU64,
    inst_ctr: AtomicU64,
}

impl SessionManager {
    pub fn new(base: PathBuf, tx: broadcast::Sender<serde_json::Value>) -> Self {
        SessionManager {
            base,
            tx,
            sessions: RwLock::new(Vec::new()),
            sid_ctr: AtomicU64::new(0),
            inst_ctr: AtomicU64::new(0),
        }
    }

    fn make_instance(&self, sid: &str, title: String) -> Result<Arc<Session>, KernelError> {
        let inst = self.inst_ctr.fetch_add(1, Ordering::SeqCst);
        let dir = self.base.join(format!("{sid}-{inst}"));
        std::fs::create_dir_all(&dir).map_err(|_| KernelError::Io)?;
        let kernel = Arc::new(LakearchKernel::open(&dir)?);
        Ok(Arc::new(Session {
            sid: sid.to_string(),
            title: Mutex::new(title),
            kernel,
            dir,
            known_areas: Mutex::new(HashSet::new()),
            active_subject: Mutex::new(None),
            tx: self.tx.clone(),
        }))
    }

    pub fn create(&self, title: Option<String>) -> Result<Arc<Session>, KernelError> {
        let n = self.sid_ctr.fetch_add(1, Ordering::SeqCst) + 1;
        let sid = format!("s{n}");
        let title = title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| format!("Session {n}"));
        let sess = self.make_instance(&sid, title)?;
        self.sessions
            .write()
            .map_err(|_| KernelError::Poisoned)?
            .push(Arc::clone(&sess));
        Ok(sess)
    }

    pub fn get(&self, sid: &str) -> Option<Arc<Session>> {
        self.sessions
            .read()
            .ok()?
            .iter()
            .find(|s| s.sid == sid)
            .cloned()
    }
    pub fn first(&self) -> Option<Arc<Session>> {
        self.sessions.read().ok()?.first().cloned()
    }
    pub fn list(&self) -> Vec<(String, String)> {
        self.sessions
            .read()
            .map(|v| v.iter().map(|s| (s.sid.clone(), s.title())).collect())
            .unwrap_or_default()
    }

    pub fn remove(&self, sid: &str) -> bool {
        let mut v = match self.sessions.write() {
            Ok(v) => v,
            Err(_) => return false,
        };
        if let Some(pos) = v.iter().position(|s| s.sid == sid) {
            let old = v.remove(pos);
            schedule_rm(old.dir.clone());
            true
        } else {
            false
        }
    }

    /// Swap in a fresh empty instance under the same sid (data wiped). The old
    /// instance's dir is removed once in-flight requests release it.
    pub fn reset(&self, sid: &str) -> Result<Option<Arc<Session>>, KernelError> {
        let title = {
            let v = self.sessions.read().map_err(|_| KernelError::Poisoned)?;
            match v.iter().find(|s| s.sid == sid) {
                Some(s) => s.title(),
                None => return Ok(None),
            }
        };
        let fresh = self.make_instance(sid, title)?;
        let mut v = self.sessions.write().map_err(|_| KernelError::Poisoned)?;
        if let Some(pos) = v.iter().position(|s| s.sid == sid) {
            let old = std::mem::replace(&mut v[pos], Arc::clone(&fresh));
            schedule_rm(old.dir.clone());
            Ok(Some(fresh))
        } else {
            Ok(None)
        }
    }
}

/// Best-effort, delayed removal of a dropped instance's dir (wait for in-flight
/// requests to release the kernel and its file handles).
fn schedule_rm(dir: PathBuf) {
    tokio::spawn(async move {
        for _ in 0..4 {
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            if std::fs::remove_dir_all(&dir).is_ok() {
                break;
            }
        }
    });
}

/// The shared, cheaply-clonable application state.
#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<SessionManager>,
    /// Live event bus (WS). Events carry an `s` (session id) so tabs filter.
    pub tx: broadcast::Sender<serde_json::Value>,
    pub ai: Arc<AiConfig>,
    pub password: Option<String>,
    pub semantics_dir: String,
}

impl AppState {
    /// Emit an event that is not tied to a single session (e.g. tab list change).
    pub fn emit_global(&self, event: serde_json::Value) {
        let _ = self.tx.send(event);
    }

    /// Resolve the session for a request (`?s=<sid>`), falling back to the first.
    pub fn session(&self, sid: Option<&str>) -> Result<Arc<Session>, AppError> {
        let s = match sid {
            Some(id) => self.sessions.get(id),
            None => self.sessions.first(),
        };
        s.ok_or_else(|| AppError(anyhow::anyhow!("no such session")))
    }
}

/// Decode a single datum *through the gate* (§11): `None` on VANISH or absence
/// (indistinguishable, §11.3), otherwise the strict-decoded `Datum`.
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
    let existed = kernel.content_set()?.binary_search(&id).is_ok();
    let written = kernel.append(datum)?;
    debug_assert_eq!(written, id, "of_datum must match the kernel's content id (§K5)");
    Ok((written, existed))
}

/// Anything fallible in a handler maps to a 500 with a visibility-blind message.
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
