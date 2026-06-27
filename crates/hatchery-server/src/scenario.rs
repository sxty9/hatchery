//! The **Axiom Lab** — deterministic, self-asserting scenarios, one per axiom.
//! Each drives the embedded kernel with the exact `Datum::*` constructors and
//! verbs the spec prescribes, asserts the expected behavior, and leaves the data
//! in the bestand so it shows up in the graph. Payloads are **stable, realistic
//! values** (no per-run salt): re-running a scenario therefore dedups against the
//! prior run exactly as a real source system would (§5.3). The few scenarios that
//! demonstrate a first-time *state transition* (dedup-hit, active-marker, content
//! collapse) assert robustly so they hold whether or not the data already exists;
//! reset the session (↻) to replay the transition from a clean bestand.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use lakearch_core::{
    CancelFlag, ContentId, Datum, Direction, GrantedScopes, Kernel as _, KernelError,
    TraversalParams,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state::{AppError, AppState, Kernel, Session};
use crate::util::cid_hex;
use crate::vocab;

fn empty_scopes() -> GrantedScopes {
    GrantedScopes::from_scope_ids(Vec::<ContentId>::new())
}

/// Title of the dedicated tab that plays the **foreign bestand** in the federation
/// scenario — a real sibling session the user can open and inspect (§12.3), not a
/// throwaway temp kernel.
const FOREIGN_TITLE: &str = "Fremdbestand";

pub async fn list(State(_state): State<AppState>) -> Json<Value> {
    Json(json!({ "scenarios": [
        {"id":"dedup","axiom":"§5.3","title":"Wert-Identität / Dedup"},
        {"id":"type","axiom":"§4","title":"Typ als Kontext"},
        {"id":"traversal","axiom":"§1.2/§1.7a","title":"Beschränkte Traversierung"},
        {"id":"supersession","axiom":"§6.3","title":"Ersetzung (append-only)"},
        {"id":"gate","axiom":"§11.3","title":"Zugriffs-Tor / VANISH"},
        {"id":"provenance","axiom":"§10.3","title":"Herkunft / find_dependents"},
        {"id":"anchor","axiom":"§9.1","title":"Anker / Repräsentanten"},
        {"id":"atomicity","axiom":"§13","title":"Aktiv-Marker / Atomarität"},
        {"id":"federation","axiom":"§12.3","title":"Föderation / Inhalts-Kollaps"}
    ]}))
}

/// Query for `POST /api/scenario/{id}`: the session plus an optional `name` that
/// the **type** scenario uses as the person's value (so re-runs add a fresh node
/// instead of deduplicating a hardcoded "Alice"); ignored by the other scenarios.
#[derive(Deserialize)]
pub struct RunQ {
    pub s: Option<String>,
    pub name: Option<String>,
}

pub async fn run(
    State(state): State<AppState>,
    Query(q): Query<RunQ>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let session = state.session(q.s.as_deref())?;

    // Federation is special: it needs a *second* bestand. We simulate that as a real
    // sibling session (a tab), so it runs outside the single-kernel `read` closure.
    if id == "federation" {
        let result = scn_federation(&state, &session).await?;
        session.emit(json!({ "type": "scenario", "id": "federation" }));
        session.emit(json!({ "type": "changed" }));
        return Ok(Json(result));
    }

    let name = q.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let (result, areas) = session
        .read(move |k| match id.as_str() {
            "dedup" => scn_dedup(k),
            "type" => scn_type(k, name.as_deref()),
            "traversal" => scn_traversal(k),
            "supersession" => scn_supersession(k),
            "gate" => scn_gate(k),
            "provenance" => scn_provenance(k),
            "anchor" => scn_anchor(k),
            "atomicity" => scn_atomicity(k),
            _ => Ok((json!({ "error": "unknown scenario" }), vec![])),
        })
        .await?;
    for a in areas {
        session.register_area(a);
    }
    session.emit(json!({ "type": "scenario", "id": result.get("id").cloned() }));
    session.emit(json!({ "type": "changed" }));
    Ok(Json(result))
}

type ScnOut = Result<(Value, Vec<ContentId>), KernelError>;

fn scn_dedup(k: &Kernel) -> ScnOut {
    let d = Datum::leaf(b"dedup-demo".to_vec());
    let id1 = k.append(&d)?;
    // Measure the dedup-hit delta around the *second* append only: `d` is certainly
    // present now, so this append is always a dedup hit — robust whether or not a
    // prior run already created `d` (no salt; the real §5.3 collapse).
    let before = k.stats()?.dedup_hit_count;
    let id2 = k.append(&d)?;
    let after = k.stats()?.dedup_hit_count;
    let passed = id1 == id2 && after == before + 1;
    Ok((
        json!({
            "id":"dedup","axiom":"§5.3","title":"Wert-Identität / Dedup","passed":passed,
            "detail": format!("zweimal angehängt → eine ContentId; dedup_hit_count +{}", after - before),
            "created":[cid_hex(id1)]
        }),
        vec![],
    ))
}

fn scn_type(k: &Kernel, name: Option<&str>) -> ScnOut {
    let person = name.unwrap_or("Alice");
    let tm = k.append(&vocab::hatchery_type_marker())?;
    let tname = k.append(&Datum::leaf(b"Person".to_vec()))?;
    let tctx = Datum::node([tm, tname]).ok_or(KernelError::Inconsistent)?;
    let tctx_id = k.append(&tctx)?;
    let content = k.append(&Datum::leaf(person.as_bytes().to_vec()))?;
    let entity = Datum::node([content, tctx_id]).ok_or(KernelError::Inconsistent)?;
    let entity_id = k.append(&entity)?;
    let snap = k.pin_snapshot()?;
    let passed = k.context_points_to(tctx_id, tname, snap)?;
    Ok((
        json!({
            "id":"type","axiom":"§4","title":"Typ als Kontext","passed":passed,
            "detail": format!("„{person}“ bekommt den Typ „Person“ — ein Kontext, der auf das Typ-Daten zeigt (kein Schema, kein Meta-Typ-Regress)."),
            "created":[cid_hex(entity_id), cid_hex(tctx_id), cid_hex(tname)]
        }),
        vec![],
    ))
}

fn scn_traversal(k: &Kernel) -> ScnOut {
    let mut prev = k.append(&Datum::leaf(b"chain-0".to_vec()))?;
    let mut created = vec![cid_hex(prev)];
    for _ in 1..=4 {
        let n = Datum::node([prev]).ok_or(KernelError::Inconsistent)?;
        prev = k.append(&n)?;
        created.push(cid_hex(prev));
    }
    let snap = k.pin_snapshot()?;
    let cap = k.authorize(empty_scopes(), snap)?;
    let params = TraversalParams::new(prev, Direction::Forward, 10, 100, None);
    let stream = k.traverse_with(params, &cap, snap, &CancelFlag::new())?;
    let mut count = 0u32;
    for step in stream {
        step?;
        count += 1;
    }
    let passed = count == 4;
    Ok((
        json!({
            "id":"traversal","axiom":"§1.2/§1.7a","title":"Beschränkte Traversierung","passed":passed,
            "detail": format!("Kette der Länge 5 → {count} Schritte; terminiert (Visited-Set, zyklensicher)."),
            "created": created
        }),
        vec![],
    ))
}

fn scn_supersession(k: &Kernel) -> ScnOut {
    k.append(&Datum::supersession_marker())?;
    // Version 1 is always the bare content leaf; ensure it is present (idempotent).
    let v1 = k.append(&Datum::leaf(b"doc-v1".to_vec()))?;

    // Walk the existing revision chain to find the latest version present, then
    // append the *next* version superseding it. Each run bumps the version — no
    // salt; the version number is genuine, monotonic content like a real revision
    // history (§7.1 append-only; nothing is ever rewritten or deleted).
    let snap0 = k.pin_snapshot()?;
    let cap0 = k.authorize(empty_scopes(), snap0)?;
    let mut latest_n = 1u32;
    let mut latest_id = v1;
    loop {
        let next_n = latest_n + 1;
        let content_id = ContentId::of_datum(&Datum::leaf(format!("doc-v{next_n}").into_bytes()));
        let sup_id = ContentId::of_datum(&Datum::supersedes(latest_id));
        let doc = Datum::node([content_id, sup_id]).ok_or(KernelError::Inconsistent)?;
        let doc_id = ContentId::of_datum(&doc);
        if k.get_by_content_id(doc_id, &cap0, snap0)?.is_some() {
            latest_n = next_n;
            latest_id = doc_id;
        } else {
            break;
        }
    }

    // Append version `latest_n + 1`, superseding the current latest.
    let new_n = latest_n + 1;
    let content = k.append(&Datum::leaf(format!("doc-v{new_n}").into_bytes()))?;
    let sup = k.append(&Datum::supersedes(latest_id))?;
    let new_doc = Datum::node([content, sup]).ok_or(KernelError::Inconsistent)?;
    let new_id = k.append(&new_doc)?;

    let snap = k.pin_snapshot()?;
    let cap = k.authorize(empty_scopes(), snap)?;
    let older = k.supersedes_visible(new_id, &cap, snap)?;
    let passed = older.contains(&latest_id);
    Ok((
        json!({
            "id":"supersession","axiom":"§6.3","title":"Ersetzung (append-only)","passed":passed,
            "detail": format!("doc-v{new_n} überholt doc-v{latest_n}; nichts gelöscht — der Kernel wählt NICHT 'die aktuelle' (§6.4)."),
            "created":[cid_hex(new_id), cid_hex(latest_id)]
        }),
        vec![],
    ))
}

fn scn_gate(k: &Kernel) -> ScnOut {
    let area = k.append(&Datum::leaf(b"area/secret".to_vec()))?;
    k.append(&Datum::area_membership_marker())?;
    let am = k.append(&Datum::area_membership(area))?;
    let content = k.append(&Datum::leaf(b"secret-doc".to_vec()))?;
    let restricted = Datum::node([content, am]).ok_or(KernelError::Inconsistent)?;
    let restricted_id = k.append(&restricted)?;
    let subject = k.append(&Datum::leaf(b"subject/alice".to_vec()))?;
    // Permission recipe (§11.1) — markers + role contexts + the permission datum.
    k.append(&Datum::permission_subject_marker())?;
    k.append(&Datum::permission_area_marker())?;
    k.append(&Datum::permission_marker())?;
    k.append(&Datum::permission_subject_role(subject))?;
    k.append(&Datum::permission_area_role(area))?;
    k.append(&Datum::permission(subject, area))?;

    let snap = k.pin_snapshot()?;
    let cap_none = k.authorize(empty_scopes(), snap)?;
    let none_hidden = k.get_by_content_id(restricted_id, &cap_none, snap)?.is_none();
    let cap_sub = k.authorize_subject(subject, snap)?;
    let sub_visible = k.get_by_content_id(restricted_id, &cap_sub, snap)?.is_some();
    let passed = none_hidden && sub_visible;
    Ok((
        json!({
            "id":"gate","axiom":"§11.3","title":"Zugriffs-Tor / VANISH","passed":passed,
            "detail":"Ohne Recht VANISHt das beschränkte Daten; das berechtigte Subjekt sieht es. 'Subjekt-Sicht' wählen, um es live zu sehen.",
            "subject": cid_hex(subject),
            "created":[cid_hex(restricted_id), cid_hex(subject)]
        }),
        vec![area],
    ))
}

fn scn_provenance(k: &Kernel) -> ScnOut {
    let i1 = k.append(&Datum::leaf(b"input-1".to_vec()))?;
    let i2 = k.append(&Datum::leaf(b"input-2".to_vec()))?;
    k.append(&Datum::origin_marker())?;
    k.append(&Datum::origin(i1))?;
    k.append(&Datum::origin(i2))?;
    let payload = k.append(&Datum::leaf(b"result".to_vec()))?;
    let result = Datum::computed_result([payload], [i1, i2]).ok_or(KernelError::Inconsistent)?;
    let (rid, _link) = k.materialize(&result, None)?;
    let snap = k.pin_snapshot()?;
    let cap = k.authorize(empty_scopes(), snap)?;
    let deps = k.dependents_visible(i1, &cap, snap)?;
    let passed = deps.contains(&rid);
    Ok((
        json!({
            "id":"provenance","axiom":"§10.3","title":"Herkunft / find_dependents","passed":passed,
            "detail":"Ein berechnetes Ergebnis trägt seine Eingaben als Herkunft; Rückwärts-Traversierung findet es (Neu-Berechnen liegt außerhalb).",
            "created":[cid_hex(rid), cid_hex(i1), cid_hex(i2)]
        }),
        vec![],
    ))
}

fn scn_anchor(k: &Kernel) -> ScnOut {
    k.append(&Datum::anchor_marker())?;
    let name = k.append(&Datum::leaf(b"Person-class".to_vec()))?;
    let anchor_id = k.append(&Datum::anchor([name]))?;
    k.append(&Datum::membership_grade_marker())?;
    k.append(&Datum::membership_marker())?;

    // How many representatives already resolve to this anchor? Each run adds the
    // next one with a slightly different confidence grade — the §9 picture of many
    // representatives collapsing onto one identity. No salt: the index `n` is
    // genuine, monotonic content (the anchor itself stays shared/deduplicated).
    let snap0 = k.pin_snapshot()?;
    let cap0 = k.authorize(empty_scopes(), snap0)?;
    let n = k.anchor_members_visible(anchor_id, &cap0, snap0)?.len() as u32 + 1;

    // Confidence walks in small 0.01 steps within (0,1); the kernel never reads it.
    let grade_str = format!("0.{}", 80 + (n - 1) % 20);
    let grade = k.append(&Datum::leaf(grade_str.as_bytes().to_vec()))?;
    k.append(&Datum::membership_grade(grade))?;
    let mem = k.append(&Datum::membership(anchor_id, grade))?;
    let rep_c = k.append(&Datum::leaf(format!("rep-Alice-{n}").into_bytes()))?;
    let rep = Datum::node([rep_c, mem]).ok_or(KernelError::Inconsistent)?;
    let rep_id = k.append(&rep)?;

    let snap = k.pin_snapshot()?;
    let cap = k.authorize(empty_scopes(), snap)?;
    let members = k.anchor_members_visible(anchor_id, &cap, snap)?;
    let passed = members.contains(&rep_id);
    Ok((
        json!({
            "id":"anchor","axiom":"§9.1","title":"Anker / Repräsentanten","passed":passed,
            "detail": format!("Repräsentant rep-Alice-{n} (Konfidenz {grade_str}) verweist per gradierter Mitgliedschaft auf denselben Anker; jetzt {} Repräsentant(en) — der Kernel wertet den Grad nie.", members.len()),
            "created":[cid_hex(anchor_id), cid_hex(rep_id)]
        }),
        vec![],
    ))
}

fn scn_atomicity(k: &Kernel) -> ScnOut {
    let c1 = Datum::leaf(b"atom-c1".to_vec());
    let c2 = Datum::leaf(b"atom-c2".to_vec());
    let c1_id = ContentId::of_datum(&c1);
    let c2_id = ContentId::of_datum(&c2);
    // Did an earlier run already activate these constituents? The active-marker is
    // append-only (§13.1), so once committed they stay visible forever — there is no
    // salt to make them fresh. Detect that so the "before" phase is asserted honestly.
    let snap0 = k.pin_snapshot()?;
    let cap0 = k.authorize(empty_scopes(), snap0)?;
    let pre_active = k.get_by_content_id(c1_id, &cap0, snap0)?.is_some();
    let handle = k.stage_restructuring(&[c1, c2])?;
    // Before the marker: constituents are inactive (§13.2) — VANISH from reads.
    let snap = k.pin_snapshot()?;
    let cap = k.authorize(empty_scopes(), snap)?;
    let before_hidden = k.get_by_content_id(c1_id, &cap, snap)?.is_none();
    let marker = Datum::active_marker_for([c1_id, c2_id]);
    let _mid = k.commit_restructuring(handle, &marker)?;
    // After the marker: both visible together (§13.1).
    let snap2 = k.pin_snapshot()?;
    let cap2 = k.authorize(empty_scopes(), snap2)?;
    let after_visible = k.get_by_content_id(c1_id, &cap2, snap2)?.is_some()
        && k.get_by_content_id(c2_id, &cap2, snap2)?.is_some();
    // Fresh bestand: hidden before, visible after. Re-run: already active from a
    // prior commit (§13.1 is idempotent) — still the correct end state.
    let passed = after_visible && (before_hidden || pre_active);
    let detail = if pre_active {
        "Konstituenten sind bereits aus einem früheren Lauf aktiv (§13.1, append-only); für die volle VANISH→sichtbar-Demo die Session zurücksetzen (↻)."
    } else {
        "Konstituenten sind unsichtbar bis der Aktiv-Marker sie GEMEINSAM freigibt — Atomarität ohne Transaktions-Maschinerie."
    };
    Ok((
        json!({
            "id":"atomicity","axiom":"§13","title":"Aktiv-Marker / Atomarität","passed":passed,
            "detail": detail,
            "created":[cid_hex(c1_id), cid_hex(c2_id)]
        }),
        vec![],
    ))
}

/// **Föderation (§12.3)** — the foreign bestand is a *real* sibling session (the
/// dedicated `FOREIGN_TITLE` tab), seeded deterministically and merged into the
/// caller's session by content hash. The tab stays around for the user to inspect.
async fn scn_federation(state: &AppState, local: &Session) -> Result<Value, AppError> {
    // Reuse the foreign tab if it exists, else spawn it — never the local session
    // (federating a kernel with itself would deadlock its RwLock).
    let foreign = match state
        .sessions
        .list()
        .into_iter()
        .find(|(sid, title)| title == FOREIGN_TITLE && sid != &local.sid)
        .and_then(|(sid, _)| state.sessions.get(&sid))
    {
        Some(s) => s,
        None => {
            let s = state.sessions.create(Some(FOREIGN_TITLE.to_string()))?;
            state.emit_global(json!({ "type": "sessions_changed" }));
            s
        }
    };

    let local_kernel = Arc::clone(&local.kernel);
    let foreign_kernel = Arc::clone(&foreign.kernel);
    let result = tokio::task::spawn_blocking(move || -> Result<Value, KernelError> {
        let shared = Datum::leaf(b"shared".to_vec());
        let foreign_only = Datum::leaf(b"foreign-only".to_vec());
        // Seed the foreign tab (idempotent: append dedups on re-run).
        foreign_kernel.append(&shared)?;
        let foreign_only_id = foreign_kernel.append(&foreign_only)?;
        // The local tab already holds `shared`.
        let shared_id = local_kernel.append(&shared)?;
        // Ingest the foreign tab into the local bestand (§12.3): content-equal data
        // collapses by hash, genuinely-new foreign data is carried over.
        let (new_count, dedup_count) = local_kernel.federate(foreign_kernel.as_ref())?;
        let snap = local_kernel.pin_snapshot()?;
        let cap = local_kernel.authorize(empty_scopes(), snap)?;
        let foreign_only_local = local_kernel
            .get_by_content_id(foreign_only_id, &cap, snap)?
            .is_some();
        let shared_local = local_kernel.get_by_content_id(shared_id, &cap, snap)?.is_some();
        let passed = foreign_only_local && shared_local && dedup_count >= 1;
        Ok(json!({
            "id":"federation","axiom":"§12.3","title":"Föderation / Inhalts-Kollaps","passed":passed,
            "detail": format!("Aus Tab „Fremdbestand“ aufgenommen: {new_count} neu, {dedup_count} per Inhalts-Hash kollabiert."),
            "created":[cid_hex(shared_id), cid_hex(foreign_only_id)]
        }))
    })
    .await
    .map_err(|_| AppError(anyhow::anyhow!("federation task panicked")))??;

    // The foreign tab was seeded — refresh it for anyone viewing it.
    foreign.emit(json!({ "type": "changed" }));
    Ok(result)
}
