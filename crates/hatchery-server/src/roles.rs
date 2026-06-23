//! Role classification — pure §1.3 structural matching of a decoded `Datum`
//! against the frozen marker conventions (`vocab`). This is the layer-above
//! reading structure; the kernel never tells us a datum's "role" (§14.3).

use lakearch_core::{ContentId, Datum};

use crate::vocab;

/// Classify a datum into a (role, label) pair for the viz. `resolve` decodes an
/// owned context id (used by the accessors that must inspect sub-contexts, e.g.
/// permission/membership).
pub fn classify<F>(id: ContentId, datum: &Datum, mut resolve: F) -> (&'static str, String)
where
    F: FnMut(ContentId) -> Option<Datum>,
{
    if datum.is_leaf() {
        if let Some(name) = vocab::marker_name(id) {
            return ("marker", name.to_string());
        }
        let payload = datum.payload().unwrap_or(&[]);
        return ("leaf", vocab::leaf_label(payload));
    }

    // --- node roles (most specific first) ---
    if datum.is_anchor() {
        return ("anchor", "⚓ anchor".to_string());
    }
    if let Some(s) = datum.identity_strength() {
        return ("identity", format!("≈ {}", vocab::identity_strength_name(s)));
    }
    if datum.is_active_marker() {
        return ("active-marker", "✓ active-marker".to_string());
    }
    if datum.recording_time_value().is_some() {
        return ("time-recording", "⏲ recording-time".to_string());
    }
    if datum.validity_time_value().is_some() {
        return ("time-validity", "⏲ validity-time".to_string());
    }
    if datum.supersedes_target().is_some() {
        return ("supersedes", "⤳ supersedes".to_string());
    }
    if datum.area_membership_target().is_some() {
        return ("area-membership", "▣ ∈area".to_string());
    }
    if datum.revocation_target().is_some() {
        return ("revocation", "⊘ revocation".to_string());
    }
    if datum.curation_hide_target().is_some() {
        return ("curation", "🙈 hide".to_string());
    }
    if datum.curation_unhide_target().is_some() {
        return ("curation", "👁 unhide".to_string());
    }
    if datum.curation_replace_targets().is_some() {
        return ("curation", "↔ replace".to_string());
    }
    if datum.erasure_audit_target().is_some() {
        return ("provenance", "🗑 erasure-audit".to_string());
    }
    if datum.origin_target().is_some() {
        return ("provenance", "↤ origin".to_string());
    }
    if datum.permission_subject_area(&mut resolve).is_some() {
        return ("permission", "🔑 permission".to_string());
    }
    if datum.membership_anchor(&mut resolve).is_some() {
        return ("membership", "∈ membership".to_string());
    }
    if datum.is_placeholder() {
        return ("placeholder", "▢ placeholder".to_string());
    }

    // type context: owns the hatchery type marker (§4)
    let type_marker = ContentId::of_datum(&vocab::hatchery_type_marker());
    if let Some(owns) = datum.owns() {
        if owns.binary_search(&type_marker).is_ok() {
            return ("type-context", "⌖ type".to_string());
        }
        return ("node", format!("⊳{}", owns.len()));
    }
    ("node", "⊳".to_string())
}
