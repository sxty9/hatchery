import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import ForceGraph2D from 'react-force-graph-2d'

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

async function api(path, opts) {
  const res = await fetch(path, {
    headers: { 'content-type': 'application/json' },
    ...opts,
  })
  return res.json()
}

export default function App() {
  const [graph, setGraph] = useState({ nodes: [], links: [] })
  const [mode, setMode] = useState('collapsed') // collapsed | expanded
  const [scenarios, setScenarios] = useState([])
  const [results, setResults] = useState({}) // id -> result
  const [metrics, setMetrics] = useState(null)
  const [subject, setSubject] = useState(null) // hex or null (admin)
  const [aiLog, setAiLog] = useState([])
  const [chat, setChat] = useState('')
  const [busy, setBusy] = useState(false)
  const [detail, setDetail] = useState(null)
  const [size, setSize] = useState({ w: 800, h: 600 })

  const fgRef = useRef(null)
  const hostRef = useRef(null)
  const nodesById = useRef(new Map())
  const pulses = useRef(new Map())

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
    for (const id of [...nodesById.current.keys()]) {
      if (!incoming.has(id)) nodesById.current.delete(id)
    }
    const links = data.edges.map((e) => ({ source: e.from, target: e.to, kind: e.kind }))
    setGraph({ nodes, links })
    setSubject(data.subject || null)
  }, [])

  const loadMetrics = useCallback(async () => setMetrics(await api('/api/metrics')), [])
  const loadScenarios = useCallback(async () => {
    const d = await api('/api/scenarios')
    setScenarios(d.scenarios || [])
  }, [])

  // ---- websocket ----
  useEffect(() => {
    loadGraph(); loadMetrics(); loadScenarios()
    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    const ws = new WebSocket(`${proto}://${location.host}/ws`)
    let timer = null
    const refresh = () => { clearTimeout(timer); timer = setTimeout(() => { loadGraph(); loadMetrics() }, 120) }
    ws.onmessage = (ev) => {
      let m
      try { m = JSON.parse(ev.data) } catch { return }
      switch (m.type) {
        case 'ai_step':
          setAiLog((l) => [...l.slice(-80), m]); refresh(); break
        case 'dedup':
          pulse(m.id); refresh(); break
        case 'node_added':
        case 'changed':
        case 'scenario':
        case 'subject_changed':
          refresh(); break
        default: break
      }
    }
    return () => { clearTimeout(timer); ws.close() }
  }, [loadGraph, loadMetrics, loadScenarios])

  // ---- sizing ----
  useEffect(() => {
    const el = hostRef.current
    if (!el) return
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }))
    ro.observe(el)
    setSize({ w: el.clientWidth, h: el.clientHeight })
    return () => ro.disconnect()
  }, [])

  // ---- pulse animation (dedup §5.3) ----
  const pulse = (id) => {
    pulses.current.set(id, performance.now() + 1300)
    const tick = () => {
      const now = performance.now()
      for (const [k, exp] of pulses.current) if (exp < now) pulses.current.delete(k)
      fgRef.current && fgRef.current.refresh()
      if (pulses.current.size) requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  }

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

  // ---- actions ----
  const runScenario = async (id) => {
    const r = await api(`/api/scenario/${id}`, { method: 'POST' })
    setResults((m) => ({ ...m, [id]: r }))
  }
  const runAll = async () => { for (const s of scenarios) await runScenario(s.id) }

  const setView = async (hex) => {
    await api('/api/subject', { method: 'POST', body: JSON.stringify({ subject: hex }) })
    setSubject(hex || null); loadGraph()
  }
  const sendChat = async () => {
    const msg = chat.trim()
    if (!msg) return
    setChat(''); setBusy(true)
    setAiLog((l) => [...l, { type: 'ai_step', phase: 'you', note: msg }])
    try {
      const r = await api('/api/chat', { method: 'POST', body: JSON.stringify({ message: msg }) })
      if (r.error) setAiLog((l) => [...l, { type: 'ai_step', phase: 'error', note: r.error }])
    } finally { setBusy(false); loadGraph() }
  }
  const appendLeaf = async () => {
    const t = prompt('Leaf payload (text):')
    if (t == null) return
    await api('/api/append/leaf', { method: 'POST', body: JSON.stringify({ text: t }) })
  }
  const showNode = async (n) => setDetail(await api(`/api/node/${n.id}`).then((d) => ({ id: n.id, ...d })))

  // ---- canvas paint ----
  const paintNode = useCallback((node, ctx, scale) => {
    const r = node.role === 'anchor' ? 7 : node.is_marker ? 3 : 5
    const now = performance.now()
    const exp = pulses.current.get(node.id)
    if (exp && exp > now) {
      const t = 1 - (exp - now) / 1300
      ctx.beginPath()
      ctx.arc(node.x, node.y, r + 2 + t * 14, 0, 2 * Math.PI)
      ctx.strokeStyle = `rgba(126,178,255,${1 - t})`
      ctx.lineWidth = 2 / scale
      ctx.stroke()
    }
    ctx.beginPath()
    ctx.arc(node.x, node.y, r, 0, 2 * Math.PI)
    ctx.fillStyle = roleColor(node.role)
    ctx.globalAlpha = node.superseded ? 0.4 : 1
    ctx.fill()
    if (node.role === 'anchor') { ctx.strokeStyle = '#fff'; ctx.lineWidth = 1.5 / scale; ctx.stroke() }
    ctx.globalAlpha = 1
    if (scale > 1.3 && !node.is_marker) {
      ctx.font = `${10 / scale}px ui-sans-serif`
      ctx.fillStyle = '#aeb8c6'
      ctx.fillText(node.label, node.x + r + 1.5, node.y + 3 / scale)
    }
  }, [])

  return (
    <div className="app">
      <div className="header">
        <h1>hatchery <span className="dim">· lakearch lab</span></h1>
        <div className="row" style={{ width: 'auto', gap: 6 }}>
          <button className={mode === 'collapsed' ? 'primary' : ''} onClick={() => setMode('collapsed')}>collapsed</button>
          <button className={mode === 'expanded' ? 'primary' : ''} onClick={() => setMode('expanded')}>expanded (reified)</button>
        </div>
        <div className="spacer" />
        {metrics && (
          <div className="metric">
            data <b>{metrics.append_count}</b> · dedup <b>{metrics.dedup_hit_count}</b> · edges <b>{metrics.edge_count}</b> · W <b>{metrics.committed_bytes}</b>B · fsync <b>{metrics.fsync_count}</b>
          </div>
        )}
      </div>

      <div className="main">
        <div className="graph-host" ref={hostRef}>
          <ForceGraph2D
            ref={fgRef}
            width={size.w}
            height={size.h}
            graphData={view}
            backgroundColor="#0c0f14"
            nodeId="id"
            nodeCanvasObject={paintNode}
            nodePointerAreaPaint={(node, color, ctx) => {
              ctx.beginPath(); ctx.arc(node.x, node.y, 7, 0, 2 * Math.PI); ctx.fillStyle = color; ctx.fill()
            }}
            linkColor={() => 'rgba(130,150,180,0.35)'}
            linkDirectionalArrowLength={3}
            linkDirectionalArrowRelPos={1}
            onNodeClick={showNode}
            cooldownTicks={120}
          />
        </div>

        {/* LEFT: axiom lab + controls */}
        <div className="overlay left">
          <div className="panel-title">view as subject (§11)</div>
          <div className="row">
            <select value={subject || ''} onChange={(e) => setView(e.target.value || null)}>
              <option value="">admin · all areas</option>
              {subject && <option value={subject}>{subject.slice(0, 16)}…</option>}
            </select>
          </div>
          <input style={{ marginTop: 6 }} placeholder="subject content-id (hex)…"
            onKeyDown={(e) => { if (e.key === 'Enter') setView(e.target.value.trim() || null) }} />

          <hr />
          <div className="panel-title">axiom lab</div>
          <div className="row" style={{ marginBottom: 6 }}>
            <button onClick={runAll}>run all</button>
            <button onClick={() => api('/api/reset', { method: 'POST' }).then(loadGraph)}>reset view</button>
            <button onClick={appendLeaf}>+ leaf</button>
          </div>
          {scenarios.map((s) => {
            const r = results[s.id]
            return (
              <div className="scn" key={s.id}>
                <span className="ax">{s.axiom}</span>
                <span className="name">{s.title}</span>
                {r && <span className={`badge ${r.passed ? 'pass' : 'fail'}`}>{r.passed ? 'PASS' : 'FAIL'}</span>}
                {r && r.subject && <button onClick={() => setView(r.subject)} title="view as the granted subject">👁</button>}
                <button onClick={() => runScenario(s.id)}>▶</button>
              </div>
            )
          })}

          <hr />
          <div className="panel-title">legend</div>
          <div className="legend">
            {Object.entries(ROLE_COLORS).map(([role, c]) => (
              <span className="item" key={role}><span className="dot" style={{ background: c }} />{role}</span>
            ))}
          </div>

          {detail && (
            <>
              <hr />
              <div className="panel-title">node</div>
              <div className="detail">
                <div><span className="k">id</span> {detail.id.slice(0, 24)}…</div>
                <div><span className="k">kind</span> {detail.kind}</div>
                {detail.payload != null && <div><span className="k">payload</span> {detail.payload}</div>}
                {detail.owns && detail.owns.length > 0 && <div><span className="k">owns</span> {detail.owns.length} ctx</div>}
              </div>
            </>
          )}
        </div>

        {/* RIGHT: AI traverser log */}
        <div className="overlay right">
          <div className="panel-title">AI traverser (§1.5)</div>
          <div className="ailog">
            {aiLog.length === 0 && <div className="hint">Type a sentence below. The AI orients, decides, and appends to lakearch as the layer above the kernel.</div>}
            {aiLog.map((s, i) => (
              <div className={`step ${s.phase === 'error' ? 'err' : ''}`} key={i}>
                {s.phase === 'you' && <div><b>you:</b> {s.note}</div>}
                {s.phase === 'start' && <div className="hint">thinking…</div>}
                {s.phase === 'tool' && <div><span className="tool">{s.tool}</span> <span className="io">{JSON.stringify(s.input)}</span></div>}
                {s.phase === 'done' && <div>{s.note}</div>}
                {s.phase === 'error' && <div>{s.note}</div>}
              </div>
            ))}
          </div>
        </div>
      </div>

      <div className="chatbar">
        <input
          placeholder='e.g. "Lege eine Person Alice an, die in Berlin wohnt"'
          value={chat}
          disabled={busy}
          onChange={(e) => setChat(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') sendChat() }}
        />
        <button className="primary" disabled={busy} onClick={sendChat}>{busy ? '…' : 'traverse'}</button>
      </div>
    </div>
  )
}
