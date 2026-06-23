//! REST handlers: the graph projection, single appends, metrics, the active
//! subject (§11), and a view reset. The AI Traverser (`/api/chat`) and the Axiom
//! Lab (`/api/scenario/*`) live in their own modules and reuse `build_graph`.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};
use axum::Json;
use lakearch_core::{
    ContentId, Datum, GrantedScopes, Kernel as _, KernelError, SnapshotToken,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::roles::classify;
use crate::state::{append_tracked, decode_visible, AppError, AppState, Kernel};
use crate::util::{cid_hex, parse_cid};
use crate::viz_model::{Edge, GraphSnapshot, Node};
use crate::vocab;

// ---------------------------------------------------------------------------
// Graph projection (§13 active set, filtered through the gate §11)
// ---------------------------------------------------------------------------

/// Build the full visible-graph projection for a view. `subject = None` ⇒ admin
/// (all `areas` granted); `Some` ⇒ that subject's structurally-derived scopes.
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

    // Superseded set (§6.3): every datum named by a supersedes-context's target.
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
        // Ownership edges A ⊳ K, only to visible targets (VANISH-honest).
        if let Some(o) = d.owns() {
            for c in o {
                if map.contains_key(c) {
                    edges.push(Edge {
                        from: cid_hex(*id),
                        to: cid_hex(*c),
                        kind: "owns",
                    });
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

    Ok(GraphSnapshot {
        nodes,
        edges,
        subject: subject.map(cid_hex),
    })
}

/// Gather the current view parameters, then build the graph on the blocking pool.
pub async fn graph(State(state): State<AppState>) -> Result<Json<GraphSnapshot>, AppError> {
    let subject = state.snapshot_subject();
    let areas: Vec<ContentId> = state
        .known_areas
        .lock()
        .map(|g| g.iter().copied().collect())
        .unwrap_or_default();
    let snap = state
        .read(move |k| build_graph(k, subject, areas))
        .await?;
    Ok(Json(snap))
}

// ---------------------------------------------------------------------------
// Single-datum reads
// ---------------------------------------------------------------------------

pub async fn node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let cid = parse_cid(&id)?;
    let subject = state.snapshot_subject();
    let areas: Vec<ContentId> = state
        .known_areas
        .lock()
        .map(|g| g.iter().copied().collect())
        .unwrap_or_default();
    let out = state
        .read(move |kernel| {
            let snap = kernel.pin_snapshot()?;
            let cap = match subject {
                Some(s) => kernel.authorize_subject(s, snap)?,
                None => kernel.authorize(GrantedScopes::from_scope_ids(areas), snap)?,
            };
            match decode_visible(kernel, cid, &cap, snap)? {
                None => Ok(json!({ "exists": false })),
                Some(d) => {
                    let payload = d
                        .payload()
                        .map(|p| String::from_utf8_lossy(p).to_string());
                    let owns: Vec<String> = d
                        .owns()
                        .map(|o| o.iter().map(|c| cid_hex(*c)).collect())
                        .unwrap_or_default();
                    Ok(json!({
                        "exists": true,
                        "kind": if d.is_leaf() { "leaf" } else { "node" },
                        "payload": payload,
                        "owns": owns,
                    }))
                }
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
    Json(req): Json<AppendLeaf>,
) -> Result<Json<Value>, AppError> {
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
    let (id, deduped) = state
        .read(move |k| append_tracked(k, &Datum::leaf(bytes)))
        .await?;
    emit_append(&state, id, deduped);
    Ok(Json(json!({ "id": cid_hex(id), "deduped": deduped })))
}

#[derive(Deserialize)]
pub struct AppendNode {
    pub owns: Vec<String>,
}

pub async fn append_node(
    State(state): State<AppState>,
    Json(req): Json<AppendNode>,
) -> Result<Json<Value>, AppError> {
    let ids: Vec<ContentId> = req
        .owns
        .iter()
        .map(|s| parse_cid(s))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let datum = Datum::node(ids)
        .ok_or_else(|| anyhow::anyhow!("a node must own at least one context (§K2.1)"))?;
    let (id, deduped) = state.read(move |k| append_tracked(k, &datum)).await?;
    emit_append(&state, id, deduped);
    Ok(Json(json!({ "id": cid_hex(id), "deduped": deduped })))
}

pub fn emit_append(state: &AppState, id: ContentId, deduped: bool) {
    state.emit(json!({
        "type": if deduped { "dedup" } else { "node_added" },
        "id": cid_hex(id),
    }));
}

// ---------------------------------------------------------------------------
// Metrics (§Betrieb) — for the benchmark panel
// ---------------------------------------------------------------------------

pub async fn metrics(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let m = state.read(|k| k.stats()).await?;
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
// Active subject (§11) + view reset
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SetSubject {
    /// hex content id of the subject, or null for the admin projection.
    pub subject: Option<String>,
}

pub async fn set_subject(
    State(state): State<AppState>,
    Json(req): Json<SetSubject>,
) -> Result<Json<Value>, AppError> {
    let cid = match req.subject {
        Some(s) if !s.trim().is_empty() => Some(parse_cid(&s)?),
        _ => None,
    };
    if let Ok(mut g) = state.active_subject.lock() {
        *g = cid;
    }
    state.emit(json!({ "type": "subject_changed", "subject": cid.map(cid_hex) }));
    Ok(Json(json!({ "subject": cid.map(cid_hex) })))
}

/// View reset: clears the active subject and the admin area grants. Does NOT
/// delete data (lakearch is append-only, §7.1) — restart with a fresh --data-dir
/// for an empty bestand.
pub async fn reset_view(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    if let Ok(mut g) = state.active_subject.lock() {
        *g = None;
    }
    if let Ok(mut g) = state.known_areas.lock() {
        g.clear();
    }
    state.emit(json!({ "type": "changed" }));
    Ok(Json(json!({ "ok": true })))
}
