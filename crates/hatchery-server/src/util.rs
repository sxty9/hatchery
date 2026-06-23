//! Small helpers shared across handlers.

use lakearch_core::ContentId;

/// Parse a 64-char lowercase/uppercase hex string into a `ContentId` (§5.2).
pub fn parse_cid(s: &str) -> anyhow::Result<ContentId> {
    let s = s.trim();
    if s.len() != 64 {
        anyhow::bail!("content id must be exactly 64 hex chars (§5.2)");
    }
    let mut b = [0u8; 32];
    for i in 0..32 {
        b[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)?;
    }
    Ok(ContentId::from_bytes(b))
}

pub fn cid_hex(id: ContentId) -> String {
    id.to_hex()
}
