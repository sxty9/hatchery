//! REST handlers. Most are scoped to a session via `?s=<sid>` (each session is
//! its own lakearch instance). Session management lives at `/api/sessions`. The
//! AI Traverser (`/api/chat`) and Axiom Lab (`/api/scenario/*`) reuse `build_graph`.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::Json;
use lakearch_core::{ContentId, Datum, GrantedScopes, Kernel as _, KernelError, SnapshotToken};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::roles::classify;
use crate::state::{append_tracked, decode_visible, AppError, AppState, Kernel, Session};
use crate::util::{cid_hex, parse_cid};
use crate::viz_model::{Edge, GraphSnapshot, Node};
use crate::vocab;

/// `?s=<session id>` carried by every session-scoped request.
#[derive(Deserialize)]
pub struct SessionQ {
    pub s: Option<String>,
}

// ---------------------------------------------------------------------------
// Sessions (tabs) — each holds its own bestand
// ---------------------------------------------------------------------------

pub async fn sessions_list(State(state): State<AppState>) -> Json<Value> {
    let list = state.sessions.list();
    Json(json!({
        "sessions": list.iter().map(|(id, title)| json!({ "id": id, "title": title })).collect::<Vec<_>>()
    }))
}

#[derive(Deserialize)]
pub struct CreateSession {
    pub title: Option<String>,
}

pub async fn sessions_create(
    State(state): State<AppState>,
    Json(req): Json<CreateSession>,
) -> Result<Json<Value>, AppError> {
    let s = state
        .sessions
        .create(req.title)
        .map_err(|e| anyhow::anyhow!("could not create session: {e}"))?;
    state.emit_global(json!({ "type": "sessions_changed" }));
    Ok(Json(json!({ "id": s.sid, "title": s.title() })))
}

pub async fn sessions_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<Value> {
    let removed = state.sessions.remove(&id);
    state.emit_global(json!({ "type": "sessions_changed" }));
    Json(json!({ "removed": removed }))
}

pub async fn sessions_reset(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let reset = state
        .sessions
        .reset(&id)
        .map_err(|e| anyhow::anyhow!("could not reset session: {e}"))?
        .is_some();
    state.emit_global(json!({ "type": "sessions_changed" }));
    state.emit_global(json!({ "type": "changed", "s": id }));
    Ok(Json(json!({ "id": id, "reset": reset })))
}

// ---------------------------------------------------------------------------
// Graph projection (§13 active set, filtered through the gate §11)
// ---------------------------------------------------------------------------

pub fn build_graph(
    kernel: &Kernel,
    subject: Option<ContentId>,
    areas: Vec<ContentId>,
) -> Result<GraphSnapshot, KernelError> {
    let snap: SnapshotToken = kernel.pin_snapshot()?;
    let cap = match subject {
        Some(s) => kernel.authorize_subject(s, snap)?,
        None => kernel.authorize(GrantedScopes::from_scope_ids(areas), snap)?,
    };

    let ids = kernel.content_set()?;
    let mut map: HashMap<ContentId, Datum> = HashMap::new();
    for id in &ids {
        if let Some(d) = decode_visible(kernel, *id, &cap, snap)? {
            map.insert(*id, d);
        }
    }

    let mut superseded: HashSet<ContentId> = HashSet::new();
    for d in map.values() {
        if let Some(older) = d.supersedes_target() {
            superseded.insert(older);
        }
    }

    let entries: Vec<(ContentId, Datum)> = map.iter().map(|(k, v)| (*k, v.clone())).collect();
    let mut nodes = Vec::with_capacity(entries.len());
    let mut edges = Vec::new();
    for (id, d) in &entries {
        let resolve = |c: ContentId| map.get(&c).cloned();
        let (role, label) = classify(*id, d, resolve);
        let owns: Vec<String> = d
            .owns()
            .map(|o| o.iter().map(|c| cid_hex(*c)).collect())
            .unwrap_or_default();
        if let Some(o) = d.owns() {
            for c in o {
                if map.contains_key(c) {
                    edges.push(Edge { from: cid_hex(*id), to: cid_hex(*c), kind: "owns" });
                }
            }
        }
        nodes.push(Node {
            id: cid_hex(*id),
            kind: if d.is_leaf() { "leaf" } else { "node" },
            role,
            label,
            owns,
            is_marker: vocab::marker_name(*id).is_some(),
            superseded: superseded.contains(id),
        });
    }

    Ok(GraphSnapshot { nodes, edges, subject: subject.map(cid_hex) })
}

pub async fn graph(
    State(state): State<AppState>,
    Query(q): Query<SessionQ>,
) -> Result<Json<GraphSnapshot>, AppError> {
    let session = state.session(q.s.as_deref())?;
    let subject = session.snapshot_subject();
    let areas = session.known_areas_vec();
    let snap = session.read(move |k| build_graph(k, subject, areas)).await?;
    Ok(Json(snap))
}

// ---------------------------------------------------------------------------
// Single-datum reads
// ---------------------------------------------------------------------------

pub async fn node(
    State(state): State<AppState>,
    Query(q): Query<SessionQ>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let session = state.session(q.s.as_deref())?;
    let cid = parse_cid(&id)?;
    let subject = session.snapshot_subject();
    let areas = session.known_areas_vec();
    let out = session
        .read(move |kernel| {
            let snap = kernel.pin_snapshot()?;
            let cap = match subject {
                Some(s) => kernel.authorize_subject(s, snap)?,
                None => kernel.authorize(GrantedScopes::from_scope_ids(areas), snap)?,
            };
            match decode_visible(kernel, cid, &cap, snap)? {
                None => Ok(json!({ "exists": false })),
                Some(d) => Ok(json!({
                    "exists": true,
                    "kind": if d.is_leaf() { "leaf" } else { "node" },
                    "payload": d.payload().map(|p| String::from_utf8_lossy(p).to_string()),
                    "owns": d.owns().map(|o| o.iter().map(|c| cid_hex(*c)).collect::<Vec<_>>()).unwrap_or_default(),
                })),
            }
        })
        .await?;
    Ok(Json(out))
}

// ---------------------------------------------------------------------------
// Appends (§7.1)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AppendLeaf {
    pub text: Option<String>,
    pub hex: Option<String>,
}

pub async fn append_leaf(
    State(state): State<AppState>,
    Query(q): Query<SessionQ>,
    Json(req): Json<AppendLeaf>,
) -> Result<Json<Value>, AppError> {
    let session = state.session(q.s.as_deref())?;
    let bytes: Vec<u8> = if let Some(t) = req.text {
        t.into_bytes()
    } else if let Some(h) = req.hex {
        let h = h.trim();
        if h.len() % 2 != 0 {
            return Err(anyhow::anyhow!("hex must have even length").into());
        }
        (0..h.len() / 2)
            .map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16))
            .collect::<Result<Vec<u8>, _>>()?
    } else {
        Vec::new()
    };
    let (id, deduped) = session.read(move |k| append_tracked(k, &Datum::leaf(bytes))).await?;
    emit_append(&session, id, deduped);
    Ok(Json(json!({ "id": cid_hex(id), "deduped": deduped })))
}

#[derive(Deserialize)]
pub struct AppendNode {
    pub owns: Vec<String>,
}

pub async fn append_node(
    State(state): State<AppState>,
    Query(q): Query<SessionQ>,
    Json(req): Json<AppendNode>,
) -> Result<Json<Value>, AppError> {
    let session = state.session(q.s.as_deref())?;
    let ids: Vec<ContentId> = req.owns.iter().map(|s| parse_cid(s)).collect::<anyhow::Result<Vec<_>>>()?;
    let datum =
        Datum::node(ids).ok_or_else(|| anyhow::anyhow!("a node must own at least one context (§K2.1)"))?;
    let (id, deduped) = session.read(move |k| append_tracked(k, &datum)).await?;
    emit_append(&session, id, deduped);
    Ok(Json(json!({ "id": cid_hex(id), "deduped": deduped })))
}

pub fn emit_append(session: &Session, id: ContentId, deduped: bool) {
    session.emit(json!({ "type": if deduped { "dedup" } else { "node_added" }, "id": cid_hex(id) }));
}

// ---------------------------------------------------------------------------
// Metrics (§Betrieb) — for the benchmark panel
// ---------------------------------------------------------------------------

pub async fn metrics(
    State(state): State<AppState>,
    Query(q): Query<SessionQ>,
) -> Result<Json<Value>, AppError> {
    let session = state.session(q.s.as_deref())?;
    let m = session.read(|k| k.stats()).await?;
    Ok(Json(json!({
        "append_count": m.append_count,
        "dedup_hit_count": m.dedup_hit_count,
        "edge_count": m.edge_count,
        "batch_count": m.batch_count,
        "fsync_count": m.fsync_count,
        "segment_count": m.segment_count,
        "committed_bytes": m.committed_bytes,
        "fail_closed_count": m.fail_closed_count,
    })))
}

// ---------------------------------------------------------------------------
// The lakearch spec ("Gesetzbuch") — global, read-only
// ---------------------------------------------------------------------------

pub async fn spec_list() -> Json<Value> {
    Json(json!({ "docs": [
        { "id": "lakearch", "title": "lakearch — Das Datenmodell (Gesetzbuch §1–§15)" },
        { "id": "encoding", "title": "Kanonische Kodierung & Inhalts-Identität (v1)" }
    ]}))
}

pub async fn spec_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let (file, title) = match id.as_str() {
        "lakearch" => ("lakearch.md", "lakearch — Das Datenmodell"),
        "encoding" => ("canonical-encoding.md", "Kanonische Kodierung & Inhalts-Identität (v1)"),
        _ => return Err(anyhow::anyhow!("unknown spec id").into()),
    };
    let path = std::path::Path::new(&state.semantics_dir).join(file);
    let markdown = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| anyhow::anyhow!("spec not readable ({}): {e}", path.display()))?;
    Ok(Json(json!({ "id": id, "title": title, "markdown": markdown })))
}

// ---------------------------------------------------------------------------
// Active subject (§11) + view reset (per session)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SetSubject {
    pub subject: Option<String>,
}

pub async fn set_subject(
    State(state): State<AppState>,
    Query(q): Query<SessionQ>,
    Json(req): Json<SetSubject>,
) -> Result<Json<Value>, AppError> {
    let session = state.session(q.s.as_deref())?;
    let cid = match req.subject {
        Some(s) if !s.trim().is_empty() => Some(parse_cid(&s)?),
        _ => None,
    };
    session.set_subject(cid);
    session.emit(json!({ "type": "subject_changed", "subject": cid.map(cid_hex) }));
    Ok(Json(json!({ "subject": cid.map(cid_hex) })))
}

/// View reset for a session: clears subject + admin area grants. Does NOT delete
/// data (use the session reset / ↻ tab control for an empty bestand).
pub async fn reset_view(
    State(state): State<AppState>,
    Query(q): Query<SessionQ>,
) -> Result<Json<Value>, AppError> {
    let session = state.session(q.s.as_deref())?;
    session.clear_view();
    session.emit(json!({ "type": "changed" }));
    Ok(Json(json!({ "ok": true })))
}
