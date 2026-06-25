import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import ForceGraph2D from 'react-force-graph-2d'
import { marked } from 'marked'

// Color per role — the *role* is the part a datum's owned contexts make it play
// (§3.1). A context is not a separate entity; the SPA can hide marker leaves
// ("collapsed") or show them as their own nodes ("expanded", reification §3.4).
const ROLE_COLORS = {
  leaf: '#7fb2ff',
  node: '#c3ccd9',
  marker: '#5b6573',
  'type-context': '#ffd166',
  identity: '#ef476f',
  'time-recording': '#06d6a0',
  'time-validity': '#26c6da',
  supersedes: '#f78c6b',
  'area-membership': '#b388ff',
  permission: '#ffd700',
  revocation: '#ff5252',
  provenance: '#80cbc4',
  membership: '#a5d6a7',
  anchor: '#ff6b6b',
  curation: '#9e9e9e',
  placeholder: '#bdbdbd',
  'active-marker': '#69f0ae',
}
const roleColor = (r) => ROLE_COLORS[r] || '#c3ccd9'

// Plain-language explanations for newcomers (shown on hover).
const ROLE_HELP = {
  leaf: ['Blatt (atomares Daten)', 'Ein primitives Daten mit opaken Bytes (z. B. Text). Existiert genau einmal — gleiche Bytes = dasselbe Daten (§5.3).'],
  node: ['Knoten', 'Ein Daten, dessen Identität allein aus der Menge seiner besessenen Kontexte entsteht (§4.3). Es besitzt andere Daten als Kontexte.'],
  marker: ['Marker-Atom (Vokabular)', "Ein eingefrorenes Blatt (z. B. „supersedes“), das einem Kontext seine Rolle gibt. In der „collapsed“-Sicht ausgeblendet."],
  'type-context': ['Typ-Kontext (§4)', 'Ein Kontext, der auf ein Typ-Daten zeigt. Typ ist nichts Besonderes — nur ein Kontext, kein Schema, kein Meta-Typ.'],
  identity: ['Gradierte Identität (§5.5)', "Aussage „A ist (teils) dasselbe wie B“ mit einer Stärke. lakearch entscheidet Identität nie — die Schicht darüber liest die Stärke."],
  'time-recording': ['Aufzeichnungszeit (§6.2)', 'Wann das System den Sachverhalt erfuhr. Zeit ist selbst Daten; der Kernel ordnet/vergleicht sie nie.'],
  'time-validity': ['Gültigkeitszeit (§6.2)', 'Wann der Sachverhalt in der Welt gilt. Darf von der Aufzeichnungszeit abweichen.'],
  supersedes: ['Ersetzungs-Kontext (§6.3)', 'Markiert ein älteres Daten als überholt — ohne es zu löschen (append-only).'],
  'area-membership': ['Bereichs-Zugehörigkeit (§11)', 'Ordnet ein Daten einem Bereich zu. Bereiche trennen logisch und steuern Sichtbarkeit.'],
  permission: ['Berechtigung (§11)', 'Gewährt einem Subjekt Zugriff auf einen Bereich. Auditierbar und bitemporal.'],
  revocation: ['Entzug (§11.4)', 'Hebt eine Berechtigung für künftige Lesevorgänge auf. Bereits Gelesenes bleibt.'],
  provenance: ['Herkunft (§10)', 'Bindet ein berechnetes Ergebnis an seine Eingaben. Ändert sich eine Eingabe, findet Rückwärts-Traversierung die betroffenen Ergebnisse.'],
  membership: ['Anker-Mitgliedschaft (§9)', 'Verweist einen Repräsentanten gradiert auf seinen Anker (die Klasse).'],
  anchor: ['Anker (§9.1)', 'Ein eigenes Daten als Klasse, auf das mehrere Repräsentanten verweisen. Anderes zeigt auf den Anker, nie auf einen Repräsentanten.'],
  curation: ['Kuratierung (§9.5)', 'Reversibles Verbergen/Ersetzen. Die Leseseite filtert es; gelöscht wird nie.'],
  placeholder: ['Platzhalter (§3.6)', 'Steht für ein noch nicht eingetroffenes Ziel. Verweise bleiben geschlossen — keine baumelnden Kanten.'],
  'active-marker': ['Aktiv-Marker (§13)', 'Ein einziges Schreiben, das einen mehrteiligen Umbau GEMEINSAM sichtbar macht. Atomarität ohne Transaktionen.'],
}

// What each axiom-lab test does, and what to watch for (newcomer-friendly).
const SCENARIO_HELP = {
  dedup: 'Hängt zweimal dasselbe Blatt an. Erwartung: nur EIN Knoten entsteht (gleiche ContentId); der zweite „Append“ schreibt nichts. Der Dedup-Zähler oben steigt.',
  type: 'Legt ein Daten „Alice“ an und gibt ihm den Typ „Person“ — als Kontext, der auf das Typ-Daten zeigt. Kein Schema, kein Meta-Typ.',
  traversal: 'Baut eine Kette aus 5 Knoten und traversiert sie. Erwartung: 4 Schritte, beschränkt und zyklensicher (terminiert immer).',
  supersession: 'Schreibt v2, das v1 überholt. v1 bleibt erhalten (gedimmt), v2 zeigt per Ersetzungs-Kontext darauf. Welche „gilt“, entscheidet erst das Lesen (§6.4).',
  gate: 'Legt ein geheimes Daten in einem Bereich an + ein berechtigtes Subjekt. Ohne Recht VERSCHWINDET das Daten (VANISH). Danach „Subjekt-Sicht“ wählen, um es ein-/ausblenden zu sehen.',
  provenance: 'Berechnet ein Ergebnis aus zwei Eingaben (mit Herkunft). find_dependents findet rückwärts alle vom Input abhängigen Ergebnisse.',
  anchor: 'Erzeugt einen Anker (Klasse „Person“) und einen Repräsentanten „Alice“, der gradiert darauf verweist. Anderes würde auf den Anker zeigen.',
  atomicity: 'Stellt zwei Daten „inaktiv“ bereit und macht sie mit EINEM Aktiv-Marker gemeinsam sichtbar. Vorher unsichtbar, nachher beide da — atomar.',
  federation: 'Öffnet einen zweiten lakearch-Bestand mit überlappenden Daten und nimmt ihn auf. Inhaltsgleiche Daten kollabieren automatisch über den Inhalts-Hash (§12.3).',
}

async function api(path, opts) {
  const res = await fetch(path, { headers: { 'content-type': 'application/json' }, ...opts })
  return res.json()
}

export default function App() {
  const [graph, setGraph] = useState({ nodes: [], links: [] })
  const [mode, setMode] = useState('collapsed')
  const [scenarios, setScenarios] = useState([])
  const [results, setResults] = useState({})
  const [metrics, setMetrics] = useState(null)
  const [subject, setSubject] = useState(null)
  const [aiLog, setAiLog] = useState([])
  const [chat, setChat] = useState('')
  const [busy, setBusy] = useState(false)
  const [detail, setDetail] = useState(null)
  const [size, setSize] = useState({ w: 800, h: 600 })
  const [tip, setTip] = useState(null) // { x, y, title, desc }
  const [toast, setToast] = useState(null) // last scenario result
  const [intro, setIntro] = useState(() => !localStorage.getItem('hatchery_intro_seen'))
  const [spec, setSpec] = useState(null) // { docs, id, title, html }

  const fgRef = useRef(null)
  const hostRef = useRef(null)
  const nodesById = useRef(new Map())
  const pulses = useRef(new Map())
  const highlight = useRef(new Set())
  const animUntil = useRef(0)
  const mouse = useRef({ x: 0, y: 0 })
  const hoverNode = useRef(null)

  // ---- animation driver (pulses + highlight rings) ----
  const kick = useCallback((ms) => {
    animUntil.current = Math.max(animUntil.current, performance.now() + ms)
    const loop = () => {
      const now = performance.now()
      for (const [k, exp] of pulses.current) if (exp < now) pulses.current.delete(k)
      if (fgRef.current) fgRef.current.refresh()
      if (now < animUntil.current || pulses.current.size) requestAnimationFrame(loop)
    }
    requestAnimationFrame(loop)
  }, [])

  // ---- data loading ----
  const loadGraph = useCallback(async () => {
    const data = await api('/api/graph')
    const incoming = new Map(data.nodes.map((n) => [n.id, n]))
    const nodes = data.nodes.map((n) => {
      const ex = nodesById.current.get(n.id)
      if (ex) { Object.assign(ex, n); return ex }
      const obj = { ...n }
      nodesById.current.set(n.id, obj)
      return obj
    })
    for (const id of [...nodesById.current.keys()]) if (!incoming.has(id)) nodesById.current.delete(id)
    const links = data.edges.map((e) => ({ source: e.from, target: e.to, kind: e.kind }))
    setGraph({ nodes, links })
    setSubject(data.subject || null)
  }, [])

  const loadMetrics = useCallback(async () => setMetrics(await api('/api/metrics')), [])
  const loadScenarios = useCallback(async () => setScenarios((await api('/api/scenarios')).scenarios || []), [])

  // ---- websocket ----
  useEffect(() => {
    loadGraph(); loadMetrics(); loadScenarios()
    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    const ws = new WebSocket(`${proto}://${location.host}/ws`)
    let timer = null
    const refresh = () => { clearTimeout(timer); timer = setTimeout(() => { loadGraph(); loadMetrics() }, 120) }
    ws.onmessage = (ev) => {
      let m; try { m = JSON.parse(ev.data) } catch { return }
      switch (m.type) {
        case 'ai_step': setAiLog((l) => [...l.slice(-80), m]); refresh(); break
        case 'dedup': pulses.current.set(m.id, performance.now() + 1300); kick(1300); refresh(); break
        case 'node_added': case 'changed': case 'scenario': case 'subject_changed': refresh(); break
        default: break
      }
    }
    return () => { clearTimeout(timer); ws.close() }
  }, [loadGraph, loadMetrics, loadScenarios, kick])

  // ---- sizing ----
  useEffect(() => {
    const el = hostRef.current
    if (!el) return
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }))
    ro.observe(el); setSize({ w: el.clientWidth, h: el.clientHeight })
    return () => ro.disconnect()
  }, [])

  // ---- collapsed/expanded projection ----
  const view = useMemo(() => {
    if (mode === 'expanded') return graph
    const visible = new Set(graph.nodes.filter((n) => !n.is_marker).map((n) => n.id))
    const nodes = graph.nodes.filter((n) => visible.has(n.id))
    const links = graph.links.filter((l) => {
      const s = typeof l.source === 'object' ? l.source.id : l.source
      const t = typeof l.target === 'object' ? l.target.id : l.target
      return visible.has(s) && visible.has(t)
    })
    return { nodes, links }
  }, [graph, mode])

  // ---- tooltips ----
  const tipFor = (title, desc) => ({
    onMouseEnter: (e) => setTip({ x: e.clientX, y: e.clientY, title, desc }),
    onMouseMove: (e) => setTip((t) => (t ? { ...t, x: e.clientX, y: e.clientY } : t)),
    onMouseLeave: () => setTip(null),
  })

  // ---- focus the nodes a test touched, so newcomers SEE where it happens ----
  const focusNodes = useCallback((ids) => {
    if (!ids || !ids.length) return
    highlight.current = new Set(ids)
    kick(9000)
    const set = highlight.current
    setTimeout(() => { if (fgRef.current) fgRef.current.zoomToFit(700, 90, (n) => set.has(n.id)) }, 650)
    setTimeout(() => { if (fgRef.current) fgRef.current.zoomToFit(500, 90, (n) => set.has(n.id)) }, 1500)
    setTimeout(() => { highlight.current = new Set(); if (fgRef.current) fgRef.current.refresh() }, 9000)
  }, [kick])

  // ---- actions ----
  const runScenario = async (id) => {
    const r = await api(`/api/scenario/${id}`, { method: 'POST' })
    setResults((m) => ({ ...m, [id]: r }))
    setToast(r)
    await loadGraph()
    focusNodes(r.created)
    if (r.subject) setToast((t) => ({ ...t, subject: r.subject }))
  }
  const runAll = async () => { for (const s of scenarios) { await runScenario(s.id); await new Promise((r) => setTimeout(r, 250)) } }

  const setView = async (hex) => {
    await api('/api/subject', { method: 'POST', body: JSON.stringify({ subject: hex }) })
    setSubject(hex || null); loadGraph()
  }
  const sendChat = async () => {
    const msg = chat.trim(); if (!msg) return
    setChat(''); setBusy(true)
    setAiLog((l) => [...l, { type: 'ai_step', phase: 'you', note: msg }])
    try {
      const r = await api('/api/chat', { method: 'POST', body: JSON.stringify({ message: msg }) })
      if (r.error) setAiLog((l) => [...l, { type: 'ai_step', phase: 'error', note: r.error }])
    } finally { setBusy(false); loadGraph() }
  }
  const appendLeaf = async () => {
    const t = prompt('Blatt-Inhalt (Text):'); if (t == null) return
    const r = await api('/api/append/leaf', { method: 'POST', body: JSON.stringify({ text: t }) })
    await loadGraph(); if (r.id) focusNodes([r.id])
  }
  const showNode = async (n) => setDetail(await api(`/api/node/${n.id}`).then((d) => ({ id: n.id, role: n.role, label: n.label, ...d })))

  // ---- Gesetzbuch (spec) drawer ----
  const openSpec = async (id) => {
    const docs = spec?.docs || (await api('/api/spec')).docs
    const wanted = id || docs[0].id
    const d = await api(`/api/spec/${wanted}`)
    setSpec({ docs, id: wanted, title: d.title, html: marked.parse(d.markdown || '') })
  }

  // ---- canvas paint ----
  const paintNode = useCallback((node, ctx, scale) => {
    const r = node.role === 'anchor' ? 7 : node.is_marker ? 3 : 5
    const now = performance.now()
    const hot = highlight.current.has(node.id)
    // dedup pulse (expanding blue ring)
    const exp = pulses.current.get(node.id)
    if (exp && exp > now) {
      const t = 1 - (exp - now) / 1300
      ctx.beginPath(); ctx.arc(node.x, node.y, r + 2 + t * 16, 0, 2 * Math.PI)
      ctx.strokeStyle = `rgba(126,178,255,${1 - t})`; ctx.lineWidth = 2 / scale; ctx.stroke()
    }
    // highlight ring for a test's touched nodes (steady gold, gentle pulse)
    if (hot) {
      const p = 0.6 + 0.4 * Math.sin(now / 180)
      ctx.beginPath(); ctx.arc(node.x, node.y, r + 5, 0, 2 * Math.PI)
      ctx.strokeStyle = `rgba(255,209,102,${p})`; ctx.lineWidth = 3 / scale; ctx.stroke()
    }
    ctx.beginPath(); ctx.arc(node.x, node.y, r, 0, 2 * Math.PI)
    ctx.fillStyle = roleColor(node.role)
    ctx.globalAlpha = node.superseded ? 0.4 : 1; ctx.fill()
    if (node.role === 'anchor') { ctx.strokeStyle = '#fff'; ctx.lineWidth = 1.5 / scale; ctx.stroke() }
    ctx.globalAlpha = 1
    if (hot || scale > 1.3) {
      if (!node.is_marker || hot) {
        ctx.font = `${(hot ? 11 : 10) / scale}px ui-sans-serif`
        ctx.fillStyle = hot ? '#ffe6a8' : '#aeb8c6'
        ctx.fillText(node.label, node.x + r + 2, node.y + 3 / scale)
      }
    }
  }, [])

  return (
    <div className="app">
      <div className="header">
        <h1>hatchery <span className="dim">· lakearch lab</span></h1>
        <div className="row" style={{ width: 'auto', gap: 6 }}>
          <button className={mode === 'collapsed' ? 'primary' : ''} onClick={() => setMode('collapsed')}
            {...tipFor('Kompakte Sicht', 'Kontext-Marker werden ausgeblendet; eine Beziehung erscheint als beschriftete Kante. Gut für den Überblick.')}>collapsed</button>
          <button className={mode === 'expanded' ? 'primary' : ''} onClick={() => setMode('expanded')}
            {...tipFor('Reifikation (§3.4)', "Jeder Kontext wird selbst als Knoten mit eigenen Kindern gezeigt. Macht sichtbar, dass ein „Kontext“ nur die Rolle eines Daten ist.")}>expanded (reified)</button>
        </div>
        <div className="spacer" />
        <button onClick={() => openSpec()} {...tipFor('Gesetzbuch', 'Die vollständige lakearch-Spezifikation (§1–§15) und die kanonische Kodierung — direkt hier lesen.')}>📖 Gesetzbuch</button>
        {metrics && (
          <div className="metric">
            <span {...tipFor('Daten', 'Physisch geschriebene Daten (ohne Dedup-Treffer).')}>data <b>{metrics.append_count}</b></span> ·{' '}
            <span {...tipFor('Dedup-Treffer (§5.3)', 'Inhaltsgleiche Appends, die NICHTS Neues schrieben — gleiche Bytes = dasselbe Daten.')}>dedup <b>{metrics.dedup_hit_count}</b></span> ·{' '}
            <span {...tipFor('Kanten', 'Indizierte Besitz-/Verweis-Kanten (abgeleitet, neu-baubar).')}>edges <b>{metrics.edge_count}</b></span> ·{' '}
            <span {...tipFor('Watermark (§13)', 'Committete Log-Bytes — die Linearisierungsstelle der durablen Sicht.')}>W <b>{metrics.committed_bytes}</b>B</span>
          </div>
        )}
      </div>

      <div className="main"
        onMouseMove={(e) => { mouse.current = { x: e.clientX, y: e.clientY }; if (hoverNode.current) setTip((t) => (t ? { ...t, x: e.clientX, y: e.clientY } : t)) }}>
        <div className="graph-host" ref={hostRef}>
          <ForceGraph2D
            ref={fgRef}
            width={size.w}
            height={size.h}
            graphData={view}
            backgroundColor="#0c0f14"
            nodeId="id"
            nodeCanvasObject={paintNode}
            nodePointerAreaPaint={(node, color, ctx) => { ctx.beginPath(); ctx.arc(node.x, node.y, 8, 0, 2 * Math.PI); ctx.fillStyle = color; ctx.fill() }}
            linkColor={() => 'rgba(130,150,180,0.35)'}
            linkDirectionalArrowLength={3}
            linkDirectionalArrowRelPos={1}
            onNodeClick={showNode}
            onNodeHover={(node) => {
              hoverNode.current = node
              if (node) {
                const [t, d] = ROLE_HELP[node.role] || [node.role, '']
                setTip({ x: mouse.current.x, y: mouse.current.y, title: `${t} · „${node.label}“`, desc: `${d}\n\nClick für Details · id ${node.id.slice(0, 12)}…` })
              } else setTip(null)
            }}
            cooldownTicks={120}
          />
        </div>

        {/* LEFT: axiom lab + controls */}
        <div className="overlay left">
          <div className="panel-title" {...tipFor('Lesen geht durchs Tor (§11)', "„admin“ sieht alles. Als Subjekt siehst du nur, wozu es berechtigt ist — der Rest VERSCHWINDET (VANISH), ununterscheidbar von „existiert nicht“.")}>view as subject (§11)</div>
          <div className="row">
            <select value={subject || ''} onChange={(e) => setView(e.target.value || null)}>
              <option value="">admin · all areas</option>
              {subject && <option value={subject}>{subject.slice(0, 16)}…</option>}
            </select>
          </div>
          <input style={{ marginTop: 6 }} placeholder="subject content-id (hex)…"
            onKeyDown={(e) => { if (e.key === 'Enter') setView(e.target.value.trim() || null) }} />

          <hr />
          <div className="panel-title" {...tipFor('Axiom-Lab', 'Jeder Test legt echte Daten in lakearch ab, prüft ein Axiom und HEBT die betroffenen Knoten im Graph hervor (gold). Fahre über einen Test für Details.')}>axiom lab · hover für Erklärung</div>
          <div className="row" style={{ marginBottom: 6 }}>
            <button onClick={runAll} {...tipFor('Alle Tests', 'Führt alle Szenarien nacheinander aus.')}>run all</button>
            <button onClick={() => api('/api/reset', { method: 'POST' }).then(loadGraph)} {...tipFor('Sicht zurücksetzen', 'Setzt Subjekt + Bereichs-Grants zurück (löscht KEINE Daten — append-only §7.1).')}>reset view</button>
            <button onClick={appendLeaf} {...tipFor('Blatt anlegen', 'Hängt ein atomares Blatt mit deinem Text an (§7.1).')}>+ leaf</button>
          </div>
          {scenarios.map((s) => {
            const r = results[s.id]
            return (
              <div className="scn" key={s.id} {...tipFor(`${s.axiom} · ${s.title}`, SCENARIO_HELP[s.id] || '')}>
                <span className="ax">{s.axiom}</span>
                <span className="name">{s.title}</span>
                {r && <span className={`badge ${r.passed ? 'pass' : 'fail'}`}>{r.passed ? 'PASS' : 'FAIL'}</span>}
                {r && r.subject && <button onClick={() => setView(r.subject)} {...tipFor('Subjekt-Sicht', 'Als das berechtigte Subjekt lesen — beobachte, wie das geschützte Daten erscheint/verschwindet (VANISH).')}>👁</button>}
                <button onClick={() => runScenario(s.id)} {...tipFor('Test ausführen', 'Führt das Szenario aus und zoomt auf die betroffenen Knoten.')}>▶</button>
              </div>
            )
          })}

          <hr />
          <div className="panel-title" {...tipFor('Rollen', 'Die Farbe eines Knotens zeigt die Rolle, die seine Kontexte ihm geben (§3.1). Fahre über einen Eintrag für die Bedeutung.')}>legend · hover für Bedeutung</div>
          <div className="legend">
            {Object.entries(ROLE_COLORS).map(([role, c]) => {
              const [t, d] = ROLE_HELP[role] || [role, '']
              return (
                <span className="item" key={role} {...tipFor(t, d)}>
                  <span className="dot" style={{ background: c }} />{role}
                </span>
              )
            })}
          </div>

          {detail && (
            <>
              <hr />
              <div className="panel-title">node</div>
              <div className="detail">
                <div><span className="k">role</span> {detail.role} — {(ROLE_HELP[detail.role] || ['', ''])[1]}</div>
                <div><span className="k">id</span> {detail.id.slice(0, 28)}…</div>
                <div><span className="k">kind</span> {detail.kind}</div>
                {detail.payload != null && <div><span className="k">payload</span> {detail.payload}</div>}
                {detail.owns && detail.owns.length > 0 && <div><span className="k">owns</span> {detail.owns.length} Kontext(e)</div>}
              </div>
            </>
          )}
        </div>

        {/* RIGHT: AI traverser log */}
        <div className="overlay right">
          <div className="panel-title" {...tipFor('Die Schicht über lakearch (§1.5)', 'Die KI rechnet, platziert und entscheidet; lakearch speichert/traversiert/matcht nur. Sie orientiert sich, entscheidet, und schreibt per append.')}>AI traverser (§1.5)</div>
          <div className="ailog">
            {aiLog.length === 0 && <div className="hint">Tippe unten einen Satz. Die KI orientiert sich, entscheidet und legt Daten in lakearch ab — als Schicht über dem Kernel.</div>}
            {aiLog.map((s, i) => (
              <div className={`step ${s.phase === 'error' ? 'err' : ''}`} key={i}>
                {s.phase === 'you' && <div><b>du:</b> {s.note}</div>}
                {s.phase === 'start' && <div className="hint">denkt nach…</div>}
                {s.phase === 'tool' && <div><span className="tool">{s.tool}</span> <span className="io">{JSON.stringify(s.input)}</span></div>}
                {s.phase === 'done' && <div>{s.note}</div>}
                {s.phase === 'error' && <div>{s.note}</div>}
              </div>
            ))}
          </div>
        </div>

        {/* TOAST: what the last test did + where to look */}
        {toast && (
          <div className={`toast ${toast.passed ? 'ok' : 'bad'}`}>
            <div className="t-head">
              <span className="t-ax">{toast.axiom}</span>
              <span className="t-title">{toast.title}</span>
              <span className={`badge ${toast.passed ? 'pass' : 'fail'}`}>{toast.passed ? 'PASS' : 'FAIL'}</span>
              <button className="t-x" onClick={() => setToast(null)}>✕</button>
            </div>
            <div className="t-detail">{toast.detail}</div>
            <div className="t-foot">👇 Die betroffenen Knoten sind im Graph <b>gold hervorgehoben</b>.
              {toast.subject && <> · <a onClick={() => setView(toast.subject)}>als Subjekt ansehen (VANISH)</a></>}
            </div>
          </div>
        )}

        {/* INTRO hint for first-time visitors */}
        {intro && (
          <div className="intro">
            <b>Willkommen.</b> hatchery testet das Datenmodell <i>lakearch</i> sichtbar. Fahre mit der Maus über <b>Tests</b>, <b>Knoten</b> und die <b>Legende</b> für Erklärungen.
            Ein Test hebt die betroffenen Knoten <b>gold</b> hervor. Das ganze Regelwerk steht unter <b>📖 Gesetzbuch</b>.
            <button onClick={() => { setIntro(false); localStorage.setItem('hatchery_intro_seen', '1') }}>verstanden</button>
          </div>
        )}
      </div>

      <div className="chatbar">
        <input placeholder='z. B. „Lege eine Person Alice an, die in Berlin wohnt“' value={chat} disabled={busy}
          onChange={(e) => setChat(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') sendChat() }} />
        <button className="primary" disabled={busy} onClick={sendChat}>{busy ? '…' : 'traverse'}</button>
      </div>

      {/* GESETZBUCH drawer */}
      {spec && (
        <div className="drawer-backdrop" onClick={() => setSpec(null)}>
          <div className="drawer" onClick={(e) => e.stopPropagation()}>
            <div className="drawer-head">
              <div className="tabs">
                {spec.docs.map((d) => (
                  <button key={d.id} className={d.id === spec.id ? 'primary' : ''} onClick={() => openSpec(d.id)}>{d.id === 'lakearch' ? 'Gesetzbuch' : 'Kanonische Kodierung'}</button>
                ))}
              </div>
              <button className="t-x" onClick={() => setSpec(null)}>✕</button>
            </div>
            <div className="doc" dangerouslySetInnerHTML={{ __html: spec.html }} />
          </div>
        </div>
      )}

      {/* floating tooltip */}
      {tip && (
        <div className="tip" style={{ left: Math.min(tip.x + 14, window.innerWidth - 320), top: Math.min(tip.y + 16, window.innerHeight - 120) }}>
          <div className="tip-title">{tip.title}</div>
          <div className="tip-desc">{tip.desc}</div>
        </div>
      )}
    </div>
  )
}
