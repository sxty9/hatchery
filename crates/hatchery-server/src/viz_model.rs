//! The graph model sent to the SPA. A **node is a datum** (§2.1); an **edge is
//! ownership** `A ⊳ K` (§3.1) — the spine of the graph. A "context" is not a
//! separate entity, only the *role* an owned datum plays (§3.1): the SPA can fold
//! marker+target into a labeled edge ("collapsed") or show the context as its own
//! node with its own children ("expanded", reification §3.4).

use serde::Serialize;

/// One datum, ready to float.
#[derive(Serialize, Clone)]
pub struct Node {
    pub id: String,
    /// "leaf" | "node".
    pub kind: &'static str,
    /// Role class (drives color): leaf, marker, node, type-context, identity,
    /// time-recording, time-validity, supersedes, area-membership, permission,
    /// revocation, provenance, membership, anchor, curation, placeholder,
    /// active-marker.
    pub role: &'static str,
    pub label: String,
    /// Owned context ids (hex) — the SPA builds ownership edges from these and
    /// decides collapse/expand.
    pub owns: Vec<String>,
    /// True for a well-known marker leaf (hidden in collapsed mode).
    pub is_marker: bool,
    /// True if some other (visible) datum supersedes this one (§6.3) — rendered dim.
    pub superseded: bool,
}

/// An ownership edge `from ⊳ to` (§3.1). `kind` is "owns".
#[derive(Serialize, Clone)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: &'static str,
}

/// A full projection of the active, visible bestand (§13/§11) for the current view.
#[derive(Serialize, Clone)]
pub struct GraphSnapshot {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// The active read subject (hex) or null for the admin projection.
    pub subject: Option<String>,
}
