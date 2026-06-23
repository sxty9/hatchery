//! The hatchery **derivation** (§14): a small vocabulary of well-known marker
//! data, plus a reverse map from a marker's `ContentId` to a human name so the
//! visualization can classify and label nodes by the *role* their owned contexts
//! give them (§1.3 structural matching only — never a model change, §14.3).
//!
//! All the kernel's frozen marker atoms live here too, so the SPA can show a
//! supersession / permission / anchor / time context as what it is.

use std::collections::HashMap;
use std::sync::OnceLock;

use lakearch_core::{ContentId, Datum, IdentityStrength};

/// hatchery's own "type" marker leaf (§4: type is just a context pointing at a
/// type datum; this marker tags such a context so the viz can color it).
pub fn hatchery_type_marker() -> Datum {
    Datum::leaf(b"hatchery/type/v1".to_vec())
}

fn build_marker_map() -> HashMap<ContentId, &'static str> {
    let mut m: HashMap<ContentId, &'static str> = HashMap::new();
    let mut add = |d: Datum, name: &'static str| {
        m.insert(ContentId::of_datum(&d), name);
    };

    add(Datum::unresolved_marker(), "unresolved");
    add(Datum::area_membership_marker(), "area-membership");
    add(Datum::permission_subject_marker(), "perm-subject");
    add(Datum::permission_area_marker(), "perm-area");
    add(Datum::permission_marker(), "permission");
    add(Datum::revocation_marker(), "revocation");
    add(Datum::recording_time_marker(), "recording-time");
    add(Datum::validity_time_marker(), "validity-time");
    add(Datum::supersession_marker(), "supersedes");
    add(Datum::anchor_marker(), "anchor");
    add(Datum::membership_marker(), "membership");
    add(Datum::membership_grade_marker(), "grade");
    add(Datum::curation_hide_marker(), "curation-hide");
    add(Datum::curation_unhide_marker(), "curation-unhide");
    add(Datum::curation_replace_marker(), "curation-replace");
    add(Datum::origin_marker(), "origin");
    add(Datum::active_marker(), "active-marker");
    add(Datum::reconcile_rule_marker(), "reconcile-rule");
    add(Datum::erasure_right_marker(), "erasure-right");
    add(Datum::erasure_audit_marker(), "erasure-audit");

    for s in [
        IdentityStrength::Deckungsgleich,
        IdentityStrength::Ergaenzt,
        IdentityStrength::WidersprichtIn,
        IdentityStrength::VerwandtMit,
        IdentityStrength::BekanntVerschieden,
    ] {
        add(Datum::identity_strength_marker(s), identity_strength_name(s));
    }

    add(hatchery_type_marker(), "type");
    m
}

pub fn identity_strength_name(s: IdentityStrength) -> &'static str {
    match s {
        IdentityStrength::Deckungsgleich => "identity:deckungsgleich",
        IdentityStrength::Ergaenzt => "identity:ergaenzt",
        IdentityStrength::WidersprichtIn => "identity:widerspricht-in",
        IdentityStrength::VerwandtMit => "identity:verwandt-mit",
        IdentityStrength::BekanntVerschieden => "identity:bekannt-verschieden",
    }
}

pub fn marker_map() -> &'static HashMap<ContentId, &'static str> {
    static MAP: OnceLock<HashMap<ContentId, &'static str>> = OnceLock::new();
    MAP.get_or_init(build_marker_map)
}

/// If `id` is a known marker leaf, its human name; else `None`.
pub fn marker_name(id: ContentId) -> Option<&'static str> {
    marker_map().get(&id).copied()
}

/// A short, human label for an opaque leaf payload: UTF-8 text if it is clean,
/// otherwise a hex prefix. lakearch never interprets payloads (§1.4) — this is
/// purely the layer-above's display choice.
pub fn leaf_label(payload: &[u8]) -> String {
    if payload.is_empty() {
        return "∅".to_string();
    }
    if let Ok(s) = std::str::from_utf8(payload) {
        if s.chars().all(|c| !c.is_control()) {
            return truncate(s, 32);
        }
    }
    let hex: String = payload
        .iter()
        .take(4)
        .map(|b| format!("{:02x}", b))
        .collect();
    format!("0x{}…", hex)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{}…", cut)
    }
}
