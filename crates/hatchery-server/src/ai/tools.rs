//! The tools exposed to the Traverser. Read tools orient (§1.3/§1.2); write tools
//! append (§7.1). Each write builds children-before-parents itself — that placement
//! work is precisely the §7.2 job of the layer above. Every tool maps to a kernel
//! verb or a frozen `Datum::*` constructor; lakearch never computes or decides.

use lakearch_core::{
    CancelFlag, ContentId, Datum, Direction, GrantedScopes, IdentityStrength, Kernel as _,
    TraversalParams,
};
use serde_json::{json, Value};

use std::sync::Arc;

use crate::state::{decode_visible, Kernel, Session};
use crate::util::{cid_hex, parse_cid};
use crate::vocab;

// --- input parsing helpers -------------------------------------------------

fn req_str(input: &Value, k: &str) -> anyhow::Result<String> {
    input
        .get(k)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("missing string parameter '{k}'"))
}

fn opt_str(input: &Value, k: &str) -> Option<String> {
    input.get(k).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn req_cid(input: &Value, k: &str) -> anyhow::Result<ContentId> {
    parse_cid(&req_str(input, k)?)
}

fn list_cids(input: &Value, k: &str) -> anyhow::Result<Vec<ContentId>> {
    match input.get(k).and_then(|v| v.as_array()) {
        None => Ok(vec![]),
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(parse_cid)
            .collect(),
    }
}

fn opt_u64(input: &Value, k: &str, default: u64) -> u64 {
    input.get(k).and_then(|v| v.as_u64()).unwrap_or(default)
}

fn parse_strength(s: &str) -> IdentityStrength {
    match s.to_ascii_lowercase().replace('_', "-").as_str() {
        "deckungsgleich" | "same" | "equal" => IdentityStrength::Deckungsgleich,
        "ergaenzt" | "ergänzt" | "complements" => IdentityStrength::Ergaenzt,
        "widerspricht-in" | "contradicts" => IdentityStrength::WidersprichtIn,
        "bekannt-verschieden" | "known-different" => IdentityStrength::BekanntVerschieden,
        _ => IdentityStrength::VerwandtMit,
    }
}

// --- dispatch --------------------------------------------------------------

/// Execute a tool by name. Returns a small JSON result fed back to Claude as a
/// `tool_result`. Write tools emit a `changed` live event so the SPA refreshes.
pub async fn dispatch(session: &Arc<Session>, name: &str, input: &Value) -> anyhow::Result<Value> {
    let admin_areas: Vec<ContentId> = session.known_areas_vec();

    let out: anyhow::Result<Value> = match name {
        // ---------------- read / orient ----------------
        "get" => {
            let id = req_cid(input, "id")?;
            let areas = admin_areas.clone();
            Ok(session
                .read(move |k| {
                    let snap = k.pin_snapshot()?;
                    let cap = k.authorize(GrantedScopes::from_scope_ids(areas), snap)?;
                    Ok(match decode_visible(k, id, &cap, snap)? {
                        None => json!({ "exists": false }),
                        Some(d) => json!({
                            "exists": true,
                            "kind": if d.is_leaf() { "leaf" } else { "node" },
                            "payload": d.payload().map(|p| String::from_utf8_lossy(p).to_string()),
                            "owns": d.owns().map(|o| o.iter().map(|c| cid_hex(*c)).collect::<Vec<_>>()).unwrap_or_default(),
                        }),
                    })
                })
                .await?)
        }
        "traverse" => {
            let start = req_cid(input, "start")?;
            let dir = match opt_str(input, "direction").as_deref() {
                Some("backward") => Direction::Backward,
                Some("both") => Direction::Both,
                _ => Direction::Forward,
            };
            let max_depth = opt_u64(input, "max_depth", 6) as u32;
            let max_nodes = opt_u64(input, "max_nodes", 256);
            let areas = admin_areas.clone();
            Ok(session
                .read(move |k| {
                    let snap = k.pin_snapshot()?;
                    let cap = k.authorize(GrantedScopes::from_scope_ids(areas), snap)?;
                    let params = TraversalParams::new(start, dir, max_depth, max_nodes, None);
                    let stream = k.traverse_with(params, &cap, snap, &CancelFlag::new())?;
                    let mut steps = Vec::new();
                    for s in stream {
                        let s = s?;
                        steps.push(json!({
                            "from": cid_hex(s.from), "edge_ctx": cid_hex(s.edge_ctx),
                            "to": cid_hex(s.to), "depth": s.depth,
                        }));
                    }
                    Ok(json!({ "steps": steps }))
                })
                .await?)
        }
        "content_equal" => {
            let a = req_cid(input, "a")?;
            let b = req_cid(input, "b")?;
            Ok(session
                .read(move |k| {
                    let snap = k.pin_snapshot()?;
                    Ok(json!({ "equal": k.content_equal(a, b, snap)? }))
                })
                .await?)
        }
        "context_points_to" => {
            let ctx = req_cid(input, "ctx")?;
            let target = req_cid(input, "target")?;
            let areas = admin_areas.clone();
            Ok(session
                .read(move |k| {
                    let snap = k.pin_snapshot()?;
                    let cap = k.authorize(GrantedScopes::from_scope_ids(areas), snap)?;
                    Ok(json!({ "points_to": k.context_points_to_visible(ctx, target, &cap, snap)? }))
                })
                .await?)
        }
        "is_member_of_set" => {
            let elem = req_cid(input, "elem")?;
            let set_ctx = req_cid(input, "set_ctx")?;
            let areas = admin_areas.clone();
            Ok(session
                .read(move |k| {
                    let snap = k.pin_snapshot()?;
                    let cap = k.authorize(GrantedScopes::from_scope_ids(areas), snap)?;
                    Ok(json!({ "member": k.is_member_of_set_visible(elem, set_ctx, &cap, snap)? }))
                })
                .await?)
        }
        "find_dependents" => {
            let id = req_cid(input, "input")?;
            let areas = admin_areas.clone();
            Ok(session
                .read(move |k| {
                    let snap = k.pin_snapshot()?;
                    let cap = k.authorize(GrantedScopes::from_scope_ids(areas), snap)?;
                    let deps = k.dependents_visible(id, &cap, snap)?;
                    Ok(json!({ "dependents": deps.iter().map(|c| cid_hex(*c)).collect::<Vec<_>>() }))
                })
                .await?)
        }

        // ---------------- write / append (§7.1) ----------------
        "append_leaf" => {
            let text = req_str(input, "text")?;
            let r = session
                .read(move |k| {
                    let d = Datum::leaf(text.into_bytes());
                    let id = ContentId::of_datum(&d);
                    let existed = k.content_set()?.binary_search(&id).is_ok();
                    k.append(&d)?;
                    Ok(json!({ "id": cid_hex(id), "deduped": existed }))
                })
                .await?;
            changed(session);
            Ok(r)
        }
        "append_node" => {
            let owns = list_cids(input, "owns")?;
            anyhow::ensure!(!owns.is_empty(), "a node must own at least one context (§K2.1)");
            let r = session
                .read(move |k| {
                    let d = Datum::node(owns).ok_or(lakearch_core::KernelError::Inconsistent)?;
                    let id = ContentId::of_datum(&d);
                    let existed = k.content_set()?.binary_search(&id).is_ok();
                    k.append(&d)?;
                    Ok(json!({ "id": cid_hex(id), "deduped": existed }))
                })
                .await?;
            changed(session);
            Ok(r)
        }
        "relate" => {
            // Build a context {relation, target}; return its id for the owner to own.
            let relation = req_str(input, "relation")?;
            let target = req_cid(input, "target")?;
            let r = session
                .read(move |k| {
                    let rel_id = k.append(&Datum::leaf(relation.into_bytes()))?;
                    let ctx = Datum::node([rel_id, target]).ok_or(lakearch_core::KernelError::Inconsistent)?;
                    let ctx_id = k.append(&ctx)?;
                    Ok(json!({ "relation_id": cid_hex(rel_id), "context_id": cid_hex(ctx_id) }))
                })
                .await?;
            changed(session);
            Ok(r)
        }
        "set_type" => {
            // Type is a context pointing at a type datum (§4); tag it with the
            // hatchery type marker so the viz colors it.
            let type_name = req_str(input, "type_name")?;
            let r = session
                .read(move |k| {
                    let m = k.append(&vocab::hatchery_type_marker())?;
                    let t = k.append(&Datum::leaf(type_name.into_bytes()))?;
                    let ctx = Datum::node([m, t]).ok_or(lakearch_core::KernelError::Inconsistent)?;
                    let ctx_id = k.append(&ctx)?;
                    Ok(json!({ "type_id": cid_hex(t), "context_id": cid_hex(ctx_id) }))
                })
                .await?;
            changed(session);
            Ok(r)
        }
        "supersede" => {
            let older = req_cid(input, "older")?;
            let new_owns = list_cids(input, "new_owns")?;
            let r = session
                .read(move |k| {
                    k.append(&Datum::supersession_marker())?;
                    let sup_id = k.append(&Datum::supersedes(older))?;
                    let mut owns = new_owns;
                    owns.push(sup_id);
                    let newer = Datum::node(owns).ok_or(lakearch_core::KernelError::Inconsistent)?;
                    let newer_id = k.append(&newer)?;
                    Ok(json!({ "newer_id": cid_hex(newer_id), "supersedes_context": cid_hex(sup_id) }))
                })
                .await?;
            changed(session);
            Ok(r)
        }
        "set_time" => {
            // Returns a time context for the carrier to own (§6.2).
            let axis = opt_str(input, "axis").unwrap_or_else(|| "validity".to_string());
            let time_value = req_str(input, "time_value")?;
            let r = session
                .read(move |k| {
                    let tv = k.append(&Datum::leaf(time_value.into_bytes()))?;
                    let (marker, ctx) = if axis == "recording" {
                        (Datum::recording_time_marker(), Datum::recording_time(tv))
                    } else {
                        (Datum::validity_time_marker(), Datum::validity_time(tv))
                    };
                    k.append(&marker)?;
                    let ctx_id = k.append(&ctx)?;
                    Ok(json!({ "time_context": cid_hex(ctx_id), "time_value": cid_hex(tv), "axis": axis }))
                })
                .await?;
            changed(session);
            Ok(r)
        }
        "assert_identity" => {
            let a = req_cid(input, "a")?;
            let b = req_cid(input, "b")?;
            let strength = parse_strength(&opt_str(input, "strength").unwrap_or_default());
            let subs = list_cids(input, "sub_contexts")?;
            let r = session
                .read(move |k| {
                    k.append(&Datum::identity_strength_marker(strength))?;
                    let id = k.append(&Datum::graded_identity(a, b, strength, subs))?;
                    Ok(json!({ "identity_id": cid_hex(id) }))
                })
                .await?;
            changed(session);
            Ok(r)
        }
        "make_anchor" => {
            let payload = list_cids(input, "payload")?;
            let r = session
                .read(move |k| {
                    k.append(&Datum::anchor_marker())?;
                    let id = k.append(&Datum::anchor(payload))?;
                    Ok(json!({ "anchor_id": cid_hex(id) }))
                })
                .await?;
            changed(session);
            Ok(r)
        }
        "add_member" => {
            // Returns a membership context for the representative to own (§9.1/§9.3).
            let anchor = req_cid(input, "anchor")?;
            let grade = req_str(input, "grade").unwrap_or_else(|_| "1.0".to_string());
            let r = session
                .read(move |k| {
                    k.append(&Datum::membership_grade_marker())?;
                    k.append(&Datum::membership_marker())?;
                    let grade_value = k.append(&Datum::leaf(grade.into_bytes()))?;
                    k.append(&Datum::membership_grade(grade_value))?;
                    let mem = k.append(&Datum::membership(anchor, grade_value))?;
                    Ok(json!({ "membership_context": cid_hex(mem) }))
                })
                .await?;
            changed(session);
            Ok(r)
        }
        "materialize" => {
            let payload = list_cids(input, "payload")?;
            let inputs = list_cids(input, "inputs")?;
            let replaces = opt_str(input, "replaces").and_then(|s| parse_cid(&s).ok());
            let r = session
                .read(move |k| {
                    k.append(&Datum::origin_marker())?;
                    for inp in &inputs {
                        k.append(&Datum::origin(*inp))?;
                    }
                    let result = Datum::computed_result(payload, inputs)
                        .ok_or(lakearch_core::KernelError::Inconsistent)?;
                    let (rid, link) = k.materialize(&result, replaces)?;
                    Ok(json!({ "result_id": cid_hex(rid), "link": link.map(cid_hex) }))
                })
                .await?;
            changed(session);
            Ok(r)
        }
        other => Err(anyhow::anyhow!("unknown tool '{other}'")),
    };
    out
}

fn changed(session: &Arc<Session>) {
    session.emit(json!({ "type": "changed" }));
}

/// The JSON tool schemas advertised to Claude.
pub fn tool_specs() -> Vec<Value> {
    vec![
        spec("get", "Fetch a datum by content id (hex). Returns kind, payload, owns.", json!({
            "type": "object", "properties": {"id": {"type":"string"}}, "required": ["id"]
        })),
        spec("traverse", "Walk ownership/context edges from a start datum (bounded, cycle-safe §1.7a).", json!({
            "type":"object","properties":{
                "start":{"type":"string"},
                "direction":{"type":"string","enum":["forward","backward","both"]},
                "max_depth":{"type":"integer"},"max_nodes":{"type":"integer"}
            },"required":["start"]
        })),
        spec("content_equal", "Address equality of two content ids (§1.3 i).", json!({
            "type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"required":["a","b"]
        })),
        spec("context_points_to", "Does context ctx point to target? (§1.3 ii)", json!({
            "type":"object","properties":{"ctx":{"type":"string"},"target":{"type":"string"}},"required":["ctx","target"]
        })),
        spec("is_member_of_set", "Is elem a member of the set spanned by set_ctx? (§1.3 iii)", json!({
            "type":"object","properties":{"elem":{"type":"string"},"set_ctx":{"type":"string"}},"required":["elem","set_ctx"]
        })),
        spec("find_dependents", "Materialized results that depend (one hop) on input (§10.3).", json!({
            "type":"object","properties":{"input":{"type":"string"}},"required":["input"]
        })),
        spec("append_leaf", "Append an atomic leaf datum with UTF-8 text payload. Returns its content id (deduped if already present, §5.3).", json!({
            "type":"object","properties":{"text":{"type":"string"}},"required":["text"]
        })),
        spec("append_node", "Append a node datum owning the given context ids (build children first). Returns its content id.", json!({
            "type":"object","properties":{"owns":{"type":"array","items":{"type":"string"}}},"required":["owns"]
        })),
        spec("relate", "Build a relation context {relation, target}. Returns context_id to include in the owner's owns (§3.3).", json!({
            "type":"object","properties":{"relation":{"type":"string"},"target":{"type":"string"}},"required":["relation","target"]
        })),
        spec("set_type", "Build a type context pointing at a type datum named type_name. Returns context_id for the entity to own (§4).", json!({
            "type":"object","properties":{"type_name":{"type":"string"}},"required":["type_name"]
        })),
        spec("supersede", "Create a newer datum that supersedes 'older' (§6.3). new_owns are the newer datum's other contexts.", json!({
            "type":"object","properties":{"older":{"type":"string"},"new_owns":{"type":"array","items":{"type":"string"}}},"required":["older"]
        })),
        spec("set_time", "Build a time context (axis: recording|validity) carrying time_value. Returns time_context for the carrier to own (§6.2).", json!({
            "type":"object","properties":{"axis":{"type":"string","enum":["recording","validity"]},"time_value":{"type":"string"}},"required":["time_value"]
        })),
        spec("assert_identity", "Assert graded referential identity between a and b (§5.5). strength: deckungsgleich|ergaenzt|widerspricht-in|verwandt-mit|bekannt-verschieden.", json!({
            "type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"},"strength":{"type":"string"},"sub_contexts":{"type":"array","items":{"type":"string"}}},"required":["a","b"]
        })),
        spec("make_anchor", "Create an anchor datum (a class §9.1). payload: optional content ids (e.g. a name leaf).", json!({
            "type":"object","properties":{"payload":{"type":"array","items":{"type":"string"}}}
        })),
        spec("add_member", "Build a graded membership context pointing at an anchor. Returns membership_context for the representative to own (§9.1/§9.3).", json!({
            "type":"object","properties":{"anchor":{"type":"string"},"grade":{"type":"string"}},"required":["anchor"]
        })),
        spec("materialize", "Store a computed result bound to its inputs by provenance (§10.2). payload: content ids of the result body.", json!({
            "type":"object","properties":{"payload":{"type":"array","items":{"type":"string"}},"inputs":{"type":"array","items":{"type":"string"}},"replaces":{"type":"string"}},"required":["inputs"]
        })),
    ]
}

fn spec(name: &str, description: &str, input_schema: Value) -> Value {
    json!({ "name": name, "description": description, "input_schema": input_schema })
}

// Keep `Kernel` referenced for clarity even though closures use `&Kernel` directly.
#[allow(dead_code)]
fn _kernel_type_marker(_k: &Kernel) {}
