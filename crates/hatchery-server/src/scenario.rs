//! The **Axiom Lab** — deterministic, self-asserting scenarios, one per axiom.
//! Each drives the embedded kernel with the exact `Datum::*` constructors and
//! verbs the spec prescribes, asserts the expected behavior, and leaves the data
//! in the bestand so it shows up in the graph. Payloads are **stable, realistic
//! values** (no per-run salt): re-running a scenario therefore dedups against the
//! prior run exactly as a real source system would (§5.3). The few scenarios that
//! demonstrate a first-time *state transition* (dedup-hit, active-marker, content
//! collapse) assert robustly so they hold whether or not the data already exists;
//! reset the session (↻) to replay the transition from a clean bestand.

use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::{Path, Query, State};
use axum::Json;
use lakearch_core::{
    CancelFlag, ContentId, Datum, Direction, GrantedScopes, Kernel as _, KernelError,
    LakearchKernel, TraversalParams,
};
use serde_json::{json, Value};

use crate::api::SessionQ;
use crate::state::{AppError, AppState, Kernel};
use crate::util::cid_hex;
use crate::vocab;

fn empty_scopes() -> GrantedScopes {
    GrantedScopes::from_scope_ids(Vec::<ContentId>::new())
}

/// Per-call counter for the federation scenario's *foreign* bestand directory.
/// This varies only the on-disk **path** of a second kernel (so concurrent
/// sessions never collide on the same redb file) — it is **not** a content salt;
/// the federated payloads stay stable so the §12.3 hash-collapse is genuine.
static FED_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

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

pub async fn run(
    State(state): State<AppState>,
    Query(q): Query<SessionQ>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let session = state.session(q.s.as_deref())?;
    let (result, areas) = session
        .read(move |k| match id.as_str() {
            "dedup" => scn_dedup(k),
            "type" => scn_type(k),
            "traversal" => scn_traversal(k),
            "supersession" => scn_supersession(k),
            "gate" => scn_gate(k),
            "provenance" => scn_provenance(k),
            "anchor" => scn_anchor(k),
            "atomicity" => scn_atomicity(k),
            "federation" => scn_federation(k),
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

fn scn_type(k: &Kernel) -> ScnOut {
    let tm = k.append(&vocab::hatchery_type_marker())?;
    let tname = k.append(&Datum::leaf(b"Person".to_vec()))?;
    let tctx = Datum::node([tm, tname]).ok_or(KernelError::Inconsistent)?;
    let tctx_id = k.append(&tctx)?;
    let content = k.append(&Datum::leaf(b"Alice".to_vec()))?;
    let entity = Datum::node([content, tctx_id]).ok_or(KernelError::Inconsistent)?;
    let entity_id = k.append(&entity)?;
    let snap = k.pin_snapshot()?;
    let passed = k.context_points_to(tctx_id, tname, snap)?;
    Ok((
        json!({
            "id":"type","axiom":"§4","title":"Typ als Kontext","passed":passed,
            "detail":"Typ ist ein Kontext, der auf ein Typ-Daten zeigt (kein Meta-Typ-Regress).",
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
    let v1 = k.append(&Datum::leaf(b"doc-v1".to_vec()))?;
    let sup = k.append(&Datum::supersedes(v1))?;
    let v2c = k.append(&Datum::leaf(b"doc-v2".to_vec()))?;
    let v2 = Datum::node([v2c, sup]).ok_or(KernelError::Inconsistent)?;
    let v2_id = k.append(&v2)?;
    let snap = k.pin_snapshot()?;
    let cap = k.authorize(empty_scopes(), snap)?;
    let older = k.supersedes_visible(v2_id, &cap, snap)?;
    let passed = older.contains(&v1);
    Ok((
        json!({
            "id":"supersession","axiom":"§6.3","title":"Ersetzung (append-only)","passed":passed,
            "detail":"v2 überholt v1; nichts gelöscht — der Kernel wählt NICHT 'die aktuelle' (§6.4).",
            "created":[cid_hex(v2_id), cid_hex(v1)]
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
    let grade = k.append(&Datum::leaf(b"0.9".to_vec()))?;
    k.append(&Datum::membership_grade(grade))?;
    let mem = k.append(&Datum::membership(anchor_id, grade))?;
    let rep_c = k.append(&Datum::leaf(b"rep-Alice".to_vec()))?;
    let rep = Datum::node([rep_c, mem]).ok_or(KernelError::Inconsistent)?;
    let rep_id = k.append(&rep)?;
    let snap = k.pin_snapshot()?;
    let cap = k.authorize(empty_scopes(), snap)?;
    let members = k.anchor_members_visible(anchor_id, &cap, snap)?;
    let passed = members.contains(&rep_id);
    Ok((
        json!({
            "id":"anchor","axiom":"§9.1","title":"Anker / Repräsentanten","passed":passed,
            "detail":"Repräsentanten verweisen per gradierter Mitgliedschaft auf den Anker (die Klasse); der Kernel wertet den Grad nie.",
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

fn scn_federation(k: &Kernel) -> ScnOut {
    // The foreign bestand is a second on-disk kernel; it only needs a private path.
    // Give it a per-call unique sub-dir (a counter, not a content salt — the payloads
    // below stay stable so the hash-collapse is genuine) and clean it up afterwards.
    let seq = FED_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("hatchery-fed-{seq}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|_| KernelError::Io)?;
    let foreign = LakearchKernel::open(&tmp)?;
    let shared = Datum::leaf(b"shared".to_vec());
    let foreign_only = Datum::leaf(b"foreign-only".to_vec());
    let shared_id = k.append(&shared)?; // present locally first
    foreign.append(&shared)?; // identical content ⇒ same ContentId
    let foreign_only_id = foreign.append(&foreign_only)?;
    let (new_count, dedup_count) = k.federate(&foreign)?;
    drop(foreign);
    let _ = std::fs::remove_dir_all(&tmp);
    // Robust invariant (holds on the first run AND on re-runs): after federation the
    // foreign-only datum is now locally resolvable and the shared content collapsed
    // by hash (≥1 dedup). The counts are reported for the first-run story.
    let snap = k.pin_snapshot()?;
    let cap = k.authorize(empty_scopes(), snap)?;
    let foreign_only_local = k.get_by_content_id(foreign_only_id, &cap, snap)?.is_some();
    let shared_local = k.get_by_content_id(shared_id, &cap, snap)?.is_some();
    let passed = foreign_only_local && shared_local && dedup_count >= 1;
    Ok((
        json!({
            "id":"federation","axiom":"§12.3","title":"Föderation / Inhalts-Kollaps","passed":passed,
            "detail": format!("Fremdbestand aufgenommen: {new_count} neu, {dedup_count} per Inhalts-Hash kollabiert."),
            "created":[cid_hex(shared_id), cid_hex(foreign_only_id)]
        }),
        vec![],
    ))
}
