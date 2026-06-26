//! The AI **Traverser** — the §1.5 layer above lakearch. It reads the user's
//! natural-language line, orients by traversing/matching, decides the §5.7 path
//! itself, and writes via `append` (§7.1). lakearch never computes or decides.

mod anthropic;
mod tools;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::SessionQ;
use crate::state::{AppError, AppState};

const SYSTEM_PROMPT: &str = "\
Du bist der TRAVERSER — die Rechen- und Platzierungs-Schicht ÜBER dem \
append-only-Datenmodell lakearch (Schicht darüber, §1.5/§7.2). \
\n\nlakearch SPEICHERT, TRAVERSIERT und MATCHT nur (§1.1–§1.3). Es RECHNET nicht, \
WERTET nicht, SORTIERT nicht und ENTSCHEIDET keine Identität (§1.4). Das alles \
tust DU. \
\n\nModell-Grundbegriffe: Es gibt genau eine Entität, das 'Daten' (§2.1). Ein Daten \
ist ENTWEDER ein atomares Blatt (Bytes) ODER ein Knoten, der eine Menge anderer \
Daten als KONTEXTE besitzt (A ⊳ K, §3.1). Ein Kontext ist KEINE eigene Sorte — nur \
die Rolle eines besessenen Daten. Verweise, Typ, Zeit, Identität sind alles Daten \
mit Kontexten. \
\n\nArbeitsweise pro Eingabe: (1) ORIENTIERE dich mit get/traverse/Prädikaten. \
(2) ENTSCHEIDE, was du anlegst und woran es sich bindet. (3) SCHREIBE: baue immer \
KINDER VOR ELTERN — erst Blätter/Kontexte (append_leaf, relate, set_type, set_time, \
add_member, …), dann den besitzenden Knoten mit append_node(owns=[…]). Schreibe \
sparsam und gezielt. \
\n\nBeispiel 'Alice wohnt in Berlin': append_leaf 'Alice'; append_leaf 'Berlin'; \
set_type 'Person' → context; set_type 'City' → context; relate 'lives-in' → context \
auf Berlin; dann append_node für Alice mit owns=[Alice-Blatt, Person-Typ-Kontext, \
lives-in-Kontext]. \
\n\nNutze deine Werkzeuge. Antworte am Ende kurz auf Deutsch, was du angelegt hast \
(mit den wichtigsten content-ids).";

#[derive(Deserialize)]
pub struct ChatReq {
    pub message: String,
}

/// Run the Traverser loop for one user message. Streams `ai_step` events live and
/// returns the final assistant text plus the list of tool steps taken.
pub async fn chat(
    State(state): State<AppState>,
    Query(q): Query<SessionQ>,
    Json(req): Json<ChatReq>,
) -> Result<Json<Value>, AppError> {
    let session = state.session(q.s.as_deref())?;
    let key = state
        .ai
        .api_key
        .clone()
        .ok_or_else(|| anyhow::anyhow!("AI Traverser disabled: set ANTHROPIC_API_KEY or /etc/hatchery/anthropic-key"))?;
    let model = state.ai.model.clone();
    let client = anthropic::Client::new();
    let tools = tools::tool_specs();

    session.emit(json!({ "type": "ai_step", "phase": "start", "note": req.message }));

    let mut messages: Vec<Value> = vec![json!({ "role": "user", "content": req.message })];
    let mut steps: Vec<Value> = Vec::new();
    let mut final_text = String::new();

    for _ in 0..state.ai.max_continuations {
        let resp = client
            .create_message(&key, &model, SYSTEM_PROMPT, &tools, &messages)
            .await?;
        let stop = resp.get("stop_reason").and_then(|v| v.as_str()).unwrap_or("");
        let content = resp.get("content").cloned().unwrap_or_else(|| json!([]));
        let blocks = content.as_array().cloned().unwrap_or_default();

        let mut tool_results: Vec<Value> = Vec::new();
        for b in &blocks {
            match b.get("type").and_then(|v| v.as_str()) {
                Some("text") => {
                    if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                        final_text.push_str(t);
                    }
                }
                Some("tool_use") => {
                    let tname = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let tuid = b.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let tinput = b.get("input").cloned().unwrap_or_else(|| json!({}));
                    session.emit(json!({ "type": "ai_step", "phase": "tool", "tool": tname, "input": tinput }));

                    match tools::dispatch(&session, tname, &tinput).await {
                        Ok(result) => {
                            steps.push(json!({ "tool": tname, "input": tinput, "result": result }));
                            tool_results.push(json!({
                                "type": "tool_result",
                                "tool_use_id": tuid,
                                "content": result.to_string(),
                            }));
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            steps.push(json!({ "tool": tname, "input": tinput, "error": msg }));
                            tool_results.push(json!({
                                "type": "tool_result",
                                "tool_use_id": tuid,
                                "is_error": true,
                                "content": msg,
                            }));
                        }
                    }
                }
                _ => {}
            }
        }

        // Append the assistant turn verbatim, then the tool results (one user msg).
        messages.push(json!({ "role": "assistant", "content": content }));

        if stop == "tool_use" && !tool_results.is_empty() {
            messages.push(json!({ "role": "user", "content": tool_results }));
            continue;
        }
        break;
    }

    session.emit(json!({ "type": "ai_step", "phase": "done", "note": final_text }));
    session.emit(json!({ "type": "changed" }));
    Ok(Json(json!({ "text": final_text, "steps": steps })))
}
