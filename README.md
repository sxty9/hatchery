# hatchery

A visual probe & benchmark service for **[lakearch](../lakearch)** — the append-only,
domain-free universal data model. hatchery embeds one lakearch kernel **in-process**,
renders its data as floating, force-directed nodes, and lets you exercise **every
axiom** (§1–§15) two ways:

- an **AI Traverser** — type a sentence; Claude acts as the *§1.5 layer above
  lakearch*: it orients by traversing, decides, and appends (the kernel only
  stores / traverses / matches);
- an **Axiom Lab** — one deterministic, self-asserting scenario per axiom
  (dedup, type, supersession, gate/VANISH, provenance, anchors, atomicity,
  federation, …) with a live PASS/FAIL badge.

A "context" is not a separate entity, only the *role* a datum plays (§3.1). The
graph toggles between **collapsed** (marker leaves folded away) and **expanded
(reified)** (every context shown as its own node with its own children, §3.4).

## Architecture

```
Browser SPA (React + react-force-graph)
   │  REST /api/*   ·   WebSocket /ws (live graph events)
   ▼
axum (tokio) ── hatchery-server (Rust, one process)
   ├─ Arc<LakearchKernel<RedbEdgeIndex>>   (reads & writes via spawn_blocking;
   │                                         the kernel's RwLock serializes writes)
   ├─ ai/  Traverser → Claude Messages API (reqwest, tool-use)
   └─ roles/vocab → datum → colored Node/Edge (pure §1.3 structural matching)
   ▼  in-process, no network
lakearch-core::LakearchKernel   (append-only bestand on --data-dir)
```

Embedding in-process (rather than the lakearchd gRPC wire, which exposes only 7
primitives) gives hatchery the **full** kernel verb surface — `set_active_marker`
(§13), `authorize_subject` (§11), `federate` (§12), `erase` — so every axiom is
reachable.

## Build & run

Prerequisites: Rust 1.96 (`~/.cargo/bin`), Node 18+. `lakearch` must sit next to
`hatchery` (path dependency `../../../lakearch/crates/lakearch-core`).

```bash
# 1. frontend (build once; served by axum from frontend/dist)
cd frontend && npm install && npm run build && cd ..

# 2. backend
cargo build --release
ANTHROPIC_API_KEY=sk-ant-...  \
  ./target/release/hatchery-server --data-dir ./hatchery-data --addr 127.0.0.1:8799
# open http://127.0.0.1:8799
```

Dev mode with hot-reload frontend (Vite proxies `/api` + `/ws` to :8799):

```bash
cargo run                     # backend on :8799
cd frontend && npm run dev    # SPA on :5173
```

### AI key

The Traverser needs a Claude key, read from `ANTHROPIC_API_KEY` or
`/etc/hatchery/anthropic-key`. Without it, manual appends and the Axiom Lab still
work; `/api/chat` returns a clear "disabled" error. Default model
`claude-sonnet-4-6` (override with `HATCHERY_MODEL`).

### Reset

lakearch is append-only (§7.1) — there is no in-place delete. "reset view" clears
the active subject and admin area grants; for an empty bestand, restart with a
fresh `--data-dir`.

## API

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/graph` | visible active bestand → nodes + ownership edges (current view) |
| GET | `/api/node/{id}` | one datum through the gate |
| POST | `/api/append/leaf` `{text}` / `/api/append/node` `{owns[]}` | append (§7.1) |
| GET | `/api/metrics` | KernelMetrics (append/dedup/edges/fsync/…) |
| POST | `/api/subject` `{subject?}` | set the read subject (§11); null = admin |
| POST | `/api/reset` | view reset (not a data wipe) |
| POST | `/api/chat` `{message}` | run the AI Traverser loop |
| GET | `/api/scenarios` · POST `/api/scenario/{id}` | list / run an axiom scenario |
| WS  | `/ws` | live events: `node_added`, `dedup`, `ai_step`, `scenario`, `changed` |

## Deploy via sxgate

hatchery is standalone (not part of holistic). Bind loopback, then on the host:

```bash
sudo sxgate service add hatchery http://localhost:8799
sudo sxgate route   add hatchery.<zone> hatchery   # zone from /etc/sxgate/sxgate.conf
```

Since this exposes AI write access publicly, gate it behind Cloudflare Access or a
token.

## Status

- M0/M1 (graph + manual append + dedup + live updates) — **done, verified**.
- M3 Axiom Lab (9 scenarios) — **done; all PASS** (incl. §11 VANISH, §13 atomic
  marker, §12 federation).
- M2 AI Traverser — **code complete**; the tool dispatch reuses the same kernel
  verbs the scenarios prove. The live Claude loop is untested here (no API key in
  the build environment).
- M4 benchmark panel (load generator + charts) and M5 asset-embedding/hardening —
  not yet built; metrics are already exposed at `/api/metrics`.
