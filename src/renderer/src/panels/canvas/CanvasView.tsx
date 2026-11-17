// Cowork 模式（cowork-demo.jpg 是版面依據）：
//   上方 ── 模式切換、卡片／畫布、Proxy、緊急停止
//   左欄 ── 搜尋、Channels（一個 Channel 就是一個群組）、Bot Roster
//   中央 ── 畫布（終端節點、訊息流動的連線、浮動審批卡）或卡片模式
//   右欄 ── Agent Collaboration & Control：Take Over 與群組對話；「詳細」分頁是節點的改動與合併
// 狀態一律以後端（wb-core canvas.rs）推來的 ChannelView 為準；這裡只送操作、不回存整份 Channel。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  BackgroundVariant,
  Controls,
  MiniMap,
  useReactFlow,
  type Connection,
  type Edge,
  type Node,
  type NodeChange,
  applyNodeChanges
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import './canvas.css'
import { useWorkbench, setUiMode } from '@/store'
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import type { CliLauncher } from '../../../../preload/index'
import type { CanvasNode, ChannelEvent, ChannelSummary, ChannelView, CanvasOp, CanvasOps } from '../../../../shared/canvas'
import TerminalNode, { type TerminalNodeData } from './TerminalNode'
import ApprovalNode, { type ApprovalNodeData } from './ApprovalNode'
import Inspector, { type Selection } from './Inspector'
import ChatPanel from './ChatPanel'
import CardsView from './CardsView'

const NODE_SIZE = { width: 640, height: 400 }
const nodeTypes = { terminal: TerminalNode, approval: ApprovalNode }
type CanvasNodeData = TerminalNodeData | ApprovalNodeData

export type CanvasCall = <K extends CanvasOp>(op: K, req?: CanvasOps[K]['req']) => Promise<CanvasOps[K]['res'] | undefined>

/** A terminal node listens for this to take the keyboard (Take Over, "answer in the terminal") */
export const FOCUS_EVENT = 'wb-canvas-focus'

// pty:launchers 只回 agents/*.yaml 與 Custom Agent；內建三家在這裡補上
const BUILTIN_AGENTS: CliLauncher[] = [
  { id: 'claude', name: 'Claude Code', cli: 'claude', command: 'claude', args: [] },
  { id: 'codex', name: 'Codex', cli: 'codex', command: 'codex', args: [] },
  { id: 'antigravity', name: 'Antigravity', cli: 'antigravity', command: 'agy', args: [] }
]

type MarkId = Parameters<typeof AgentMark>[0]['agent']
const MARKS = ['claude', 'codex', 'antigravity', 'powershell', 'cmd', 'bash', 'custom-agent']
/** AgentMark 只認得內建 CLI；yaml launcher 用它實際跑的 CLI 的圖示 */
export const markOf = (cli: string): MarkId => (MARKS.includes(cli) ? cli : 'shell') as MarkId

type BotState = 'offline' | 'ended' | 'idle' | 'active' | 'waiting'

function remember(key: string, value: string | null): void {
  try {
    if (value) localStorage.setItem(key, value)
    else localStorage.removeItem(key)
  } catch {
    /* 忽略 */
  }
}

export default function CanvasView(): JSX.Element {
  return (
    <ReactFlowProvider>
      <CanvasShell />
    </ReactFlowProvider>
  )
}

function CanvasShell(): JSX.Element {
  const { t } = useTranslation()
  const { workspaceRoot, uiMode } = useWorkbench()
  const flow = useReactFlow()
  const storeKey = `wb-canvas-channel:${workspaceRoot}`
  const [channels, setChannels] = useState<ChannelSummary[]>([])
  const [channelId, setChannelId] = useState<string | null>(() => {
    try {
      return localStorage.getItem(storeKey)
    } catch {
      return null
    }
  })
  const [view, setView] = useState<ChannelView | null>(null)
  const [launchers, setLaunchers] = useState<CliLauncher[]>([])
  const [selection, setSelection] = useState<Selection>(null)
  const [mode, setMode] = useState<'canvas' | 'cards'>('canvas')
  const [tab, setTab] = useState<'chat' | 'details'>('chat')
  const [inRoot, setInRoot] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [nodes, setNodes] = useState<Node<CanvasNodeData>[]>([])
  const [zoom, setZoom] = useState(1)
  const [recentEdge, setRecentEdge] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [proxy, setProxy] = useState('')
  const search = useRef<HTMLInputElement>(null)
  const idRef = useRef(channelId)
  idRef.current = channelId

  const call: CanvasCall = useCallback(async (op, req) => {
    setError(null)
    const r = await window.api.canvas.call(op, req)
    // void ops come back as null; undefined only ever means "failed, error shown"
    if (r.ok) return (r.data ?? null) as never
    const key = `canvas.err_${r.code.replace(/-/g, '_')}`
    const known = t(key)
    setError(known !== key ? (r.message && r.code === 'invalid' ? `${known}: ${r.message}` : known) : r.message || r.code)
    return undefined
  }, [t])

  const loadList = useCallback(async () => {
    const list = await call('channels.list')
    if (list) setChannels(list)
    return list
  }, [call])

  // 第一次進來：沒有 Channel 就建一個 general；記住上次看的 Channel
  useEffect(() => {
    let alive = true
    void (async () => {
      const list = await loadList()
      if (!alive || !list) return
      if (list.length === 0) {
        const v = await call('channel.create', { name: 'general' })
        if (v && alive) {
          setChannelId(v.channel.id)
          setView(v)
          void loadList()
        }
        return
      }
      if (!idRef.current || !list.some((c) => c.id === idRef.current)) setChannelId(list[0].id)
    })()
    window.api.pty.launchers().then((l) => alive && setLaunchers(l)).catch(() => {})
    return () => {
      alive = false
    }
  }, [workspaceRoot, call, loadList])

  useEffect(() => {
    remember(storeKey, channelId)
    setView(null)
    setSelection(null)
    if (!channelId) return
    void call('channel.get', { channelId }).then((v) => v && idRef.current === channelId && setView(v))
  }, [channelId, call, storeKey])

  useEffect(
    () =>
      window.api.canvas.onUpdate((u) => {
        if (u.channelId === idRef.current) {
          if (u.view) setView(u.view)
          else setChannelId(null)
        }
        void loadList()
      }),
    [loadList]
  )

  // The proxy pill shows what new sessions will start with; read again when the window gets focus (settings may have changed)
  useEffect(() => {
    const read = (): void => {
      window.api.settings
        .get()
        .then((s) => {
          const p = s.proxy
          setProxy(!p || p.mode === 'inherit' ? t('canvas.proxyInherit') : p.mode === 'direct' ? t('canvas.proxyDirect') : (p.url || '').replace(/^\w+:\/\//, '').replace(/\/$/, ''))
        })
        .catch(() => {})
    }
    read()
    window.addEventListener('focus', read)
    return () => window.removeEventListener('focus', read)
  }, [t])

  // 只在 App 有焦點時作用；Claude Code 自己用 Esc／雙 Esc，所以緊急停止不用 Esc（§2.3）
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.ctrlKey && e.shiftKey && e.key === 'Backspace') {
        e.preventDefault()
        void call('stop')
      } else if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        search.current?.focus()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [call])

  const agents = useMemo(() => {
    const extra = launchers.filter((l) => !BUILTIN_AGENTS.some((b) => b.id === l.id))
    return [...BUILTIN_AGENTS, ...extra]
  }, [launchers])
  const agentName = useCallback((id: string) => agents.find((a) => a.id === id)?.name || id, [agents])
  const markOfAgent = useCallback((agent: string): MarkId => markOf(agents.find((a) => a.id === agent)?.cli || agent), [agents])

  const focusTerminal = useCallback((id: string): void => {
    // After React has put the node on screen
    window.setTimeout(() => window.dispatchEvent(new CustomEvent(FOCUS_EVENT, { detail: id })), 350)
  }, [])

  // 後端狀態變了才重建節點；拖曳中的位置以本地為準，放開後才送 node.layout
  const dragging = useRef(new Set<string>())
  useEffect(() => {
    if (!view) {
      setNodes([])
      return
    }
    setNodes((prev) => {
      const terminals: Node<CanvasNodeData>[] = view.channel.nodes.map((n) => {
        const old = prev.find((p) => p.id === n.id)
        const keepPos = old && dragging.current.has(n.id)
        return {
          id: n.id,
          type: 'terminal',
          position: keepPos ? old.position : n.position,
          style: { width: n.size.width, height: n.size.height },
          dragHandle: '.cv-node-head',
          selected: selection?.kind === 'node' && selection.id === n.id,
          data: { node: n, rt: view.runtime[n.id], channelId: view.channel.id, name: agentName(n.agent), mark: markOfAgent(n.agent), zoom, stopped: view.stop.requested, call }
        }
      })
      // A waiting approval floats under its terminal as its own card; where the user dragged it is kept while it lives
      const cards: Node<CanvasNodeData>[] = view.channel.nodes.flatMap((n) => {
        const approval = view.runtime[n.id]?.approval
        if (!approval) return []
        const id = `approval:${n.id}`
        const old = prev.find((p) => p.id === id)
        const width = Math.min(520, Math.max(340, n.size.width - 80))
        return [{
          id,
          type: 'approval',
          position: old ? old.position : { x: n.position.x + 40, y: n.position.y + n.size.height + 22 },
          style: { width },
          zIndex: 5,
          selectable: false,
          data: { nodeId: n.id, channelId: view.channel.id, name: `${agentName(n.agent)} · ${n.id.slice(0, 4)}`, approval, stopped: view.stop.requested, call, onFocusTerminal: focusTerminal }
        }]
      })
      return [...terminals, ...cards]
    })
  }, [view, selection, agentName, markOfAgent, zoom, call, focusTerminal])

  const edges: Edge[] = useMemo(() => {
    if (!view) return []
    const ids = new Set(view.channel.nodes.map((n) => n.id))
    const xOf = new Map(view.channel.nodes.map((n) => [n.id, n.position.x]))
    // One line per pair of agents that have talked; it glows while a message between them is still on its way
    const flows = new Map<string, { a: string; b: string; active: boolean }>()
    for (const m of view.messages) {
      if (!ids.has(m.from) || !ids.has(m.to)) continue
      const [a, b] = m.from < m.to ? [m.from, m.to] : [m.to, m.from]
      const f = flows.get(`${a}:${b}`) || { a: m.from, b: m.to, active: false }
      if (m.state === 'pending-approval' || m.state === 'queued' || m.state === 'notified') {
        f.active = true
        f.a = m.from
        f.b = m.to
      }
      flows.set(`${a}:${b}`, f)
    }
    return [
      ...view.channel.edges.map((e) => ({
        id: e.id,
        source: e.from,
        target: e.to,
        label: t('canvas.handoff'),
        animated: recentEdge === e.id,
        selected: selection?.kind === 'edge' && selection.id === e.id,
        className: 'cv-edge'
      })),
      ...[...flows.entries()].map(([k, f]) => {
        const rev = (xOf.get(f.a) ?? 0) > (xOf.get(f.b) ?? 0)
        return { id: `flow:${k}`, source: rev ? f.b : f.a, target: rev ? f.a : f.b, selectable: false, focusable: false, animated: f.active, className: `cv-edge-flow${f.active ? ' active' : ''}${rev ? ' rev' : ''}` }
      })
    ]
  }, [view, recentEdge, selection, t])

  const onNodesChange = useCallback((changes: NodeChange<Node<CanvasNodeData>>[]) => {
    for (const c of changes) {
      if (c.type === 'position') {
        if (c.dragging) dragging.current.add(c.id)
      }
    }
    setNodes((ns) => applyNodeChanges(changes, ns))
  }, [])

  const saveLayout = (node: Node<CanvasNodeData>): void => {
    dragging.current.delete(node.id)
    if (!view || node.type !== 'terminal') return
    const n = node as Node<TerminalNodeData>
    const width = Math.round(Number(n.measured?.width ?? n.width ?? n.data.node.size.width))
    const height = Math.round(Number(n.measured?.height ?? n.height ?? n.data.node.size.height))
    void call('node.layout', { channelId: view.channel.id, nodeId: n.id, position: { x: Math.round(n.position.x), y: Math.round(n.position.y) }, size: { width, height } })
  }

  const addNode = async (agent: string, position?: { x: number; y: number }): Promise<void> => {
    if (!view) return
    // A free spot: next to the nodes already there, or where the user dropped it
    const count = view.channel.nodes.length
    const pos = position || { x: (count % 3) * (NODE_SIZE.width + 60), y: Math.floor(count / 3) * (NODE_SIZE.height + 140) }
    const n = await call('node.create', { channelId: view.channel.id, agent, workspace: inRoot ? 'root' : 'worktree', position: { x: Math.round(pos.x), y: Math.round(pos.y) }, size: NODE_SIZE })
    if (n) {
      setSelection({ kind: 'node', id: n.id })
      if (!position) window.setTimeout(() => void flow.fitView({ duration: 300, maxZoom: 1 }), 200)
    }
  }

  const onConnect = async (c: Connection): Promise<void> => {
    if (!view || !c.source || !c.target || c.source === c.target || c.target.startsWith('approval:')) return
    const edge = await call('handoff.create', { channelId: view.channel.id, from: c.source, to: c.target, note: '' })
    if (!edge) return
    setSelection({ kind: 'edge', id: edge.id })
    setTab('details')
    // 流動光點只在交接後幾秒播放（§3.5）
    setRecentEdge(edge.id)
    window.setTimeout(() => setRecentEdge((x) => (x === edge.id ? null : x)), 4000)
  }

  const nodeName = useCallback((id: string): string => {
    const n = view?.channel.nodes.find((x) => x.id === id)
    return n ? `${agentName(n.agent)} · ${n.id.slice(0, 4)}` : id.slice(0, 4)
  }, [view, agentName])
  const markOfNode = useCallback((id: string): MarkId => markOfAgent(view?.channel.nodes.find((x) => x.id === id)?.agent || ''), [view, markOfAgent])

  /** Show a node on the canvas */
  const pickNode = (id: string): void => {
    setSelection({ kind: 'node', id })
    setMode('canvas')
    const n = view?.channel.nodes.find((x) => x.id === id)
    if (n) void flow.setCenter(n.position.x + n.size.width / 2, n.position.y + n.size.height / 2, { zoom: 1, duration: 300 })
  }

  /** Take Over: stop what the agent is doing, once, and hand the keyboard to the user in its terminal */
  const takeOver = (id: string): void => {
    const rt = view?.runtime[id]
    // Never while a question is on screen: Esc would answer it with "no"
    if (rt?.status === 'running' && rt.ptyId && rt.busy && !rt.approval) window.api.pty.write(rt.ptyId, '\x1b')
    pickNode(id)
    focusTerminal(id)
  }

  const stop = view?.stop
  const group = view?.teams[0]
  // Everything that waits on the user in this channel: CLI approvals, held messages, merge requests
  const pendingCount = view ? Object.values(view.runtime).filter((r) => r.approval).length + view.relays.length + view.mergeRequests.length : 0
  const createChannel = async (): Promise<void> => {
    const name = window.prompt(t('canvas.channelName'), '')
    if (!name?.trim()) return
    const v = await call('channel.create', { name })
    if (v) setChannelId(v.channel.id)
  }

  const q = query.trim().toLowerCase()
  const shownChannels = channels.filter((c) => !q || c.name.toLowerCase().includes(q))
  const shownAgents = agents.filter((a) => !q || a.name.toLowerCase().includes(q) || a.id.toLowerCase().includes(q))
  const botState = (agent: string): { state: BotState; nodes: CanvasNode[] } => {
    const mine = view?.channel.nodes.filter((n) => n.agent === agent) || []
    if (mine.length === 0) return { state: 'offline', nodes: mine }
    const rts = mine.map((n) => view?.runtime[n.id])
    const state: BotState = rts.some((r) => r?.approval) ? 'waiting' : rts.some((r) => r?.status === 'running' && r.busy) ? 'active' : rts.some((r) => r?.status === 'running') ? 'idle' : 'ended'
    return { state, nodes: mine }
  }
  const selectedNode = selection?.kind === 'node' ? selection.id : null

  return (
    <div className="cv-root">
      <div className="cv-topbar">
        <span className="cv-spacer" />
        <div className="segmented">
          {(['developer', 'vibe', 'cowork'] as const).map((m) => (
            <button key={m} className={uiMode === m ? 'on' : ''} onClick={() => setUiMode(m)}>
              {t(`vibe.${m}Title`)}
            </button>
          ))}
        </div>
        <div className="segmented">
          <button className={mode === 'cards' ? 'on' : ''} onClick={() => setMode('cards')}>{t('canvas.cards')}</button>
          <button className={mode === 'canvas' ? 'on' : ''} onClick={() => setMode('canvas')}>{t('canvas.canvas')}</button>
        </div>
        <span className="cv-spacer" />
        {error && <span className="cv-error" role="alert">{error}</span>}
        {pendingCount > 0 && (
          <button className="cv-pending" onClick={() => setTab('chat')}>
            {t('canvas.pending', { n: pendingCount })}
          </button>
        )}
        <span className="cv-proxy" title={t('canvas.proxyHint')}>
          <span className="cv-proxy-dot" /> Proxy: {proxy}
        </span>
        {stop?.requested ? (
          <>
            <span className="cv-stop-note" title={t('canvas.stopRequestedHint')}>{t('canvas.stopRequested')}</span>
            <button className="cv-break" onClick={() => window.confirm(t('canvas.killAllConfirm')) && void call('stop.killAll')}>{t('canvas.killAll')}</button>
            <button className="cv-btn" onClick={() => void call('stop.clear')}>{t('canvas.clearStop')}</button>
          </>
        ) : (
          <button className="cv-break" title={t('canvas.stopHint')} onClick={() => void call('stop')} disabled={!view}>
            <span aria-hidden="true">⚠</span> {t('canvas.emergencyBreak')}
          </button>
        )}
      </div>

      <div className="cv-body">
        <aside className="cv-left">
          <label className="cv-search">
            <span aria-hidden="true">⌕</span>
            <input ref={search} value={query} placeholder={t('canvas.search')} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === 'Escape' && setQuery('')} />
            <kbd>Ctrl K</kbd>
          </label>

          <div className="cv-section-head">
            <span>{t('canvas.channels')}</span>
            <button className="cv-icon-btn" title={t('canvas.newChannel')} onClick={createChannel}>＋</button>
          </div>
          <div className="cv-list">
            {shownChannels.map((c) => (
              <button
                key={c.id}
                className={`cv-list-item ${c.id === channelId ? 'on' : ''}`}
                onClick={() => setChannelId(c.id)}
                onDoubleClick={async () => {
                  const name = window.prompt(t('canvas.channelName'), c.name)
                  if (name?.trim()) await call('channel.rename', { channelId: c.id, name })
                }}
              >
                <span className="cv-hash">#</span>
                <span className="cv-list-name">{c.name}</span>
                {c.nodeCount > 0 && <span className="cv-list-count">{c.nodeCount}</span>}
                {c.pendingApprovals > 0 && <span className="cv-badge">{c.pendingApprovals}</span>}
              </button>
            ))}
          </div>

          <div className="cv-section-head">
            <span>{t('canvas.botRoster')}</span>
          </div>
          <div className="cv-list">
            {shownAgents.map((a) => {
              const { state, nodes: mine } = botState(a.id)
              const leads = mine.some((n) => n.id === group?.leadId)
              return (
                <div
                  key={a.id}
                  className={`cv-bot st-${state}`}
                  draggable
                  onDragStart={(e) => e.dataTransfer.setData('application/x-canvas-agent', a.id)}
                  // 實際會執行的指令：yaml 不能拿名稱偽裝（§2.3）
                  title={`${[a.command, ...(a.args || [])].join(' ')}\n${t('canvas.agentHint')}`}
                >
                  <button className="cv-bot-main" disabled={!view} onClick={() => (mine.length ? pickNode(mine[0].id) : !stop?.requested && void addNode(a.id))}>
                    <span className="cv-bot-avatar">
                      <AgentMark agent={markOf(a.cli || a.id)} size={20} />
                    </span>
                    <span className="cv-bot-text">
                      <span className="cv-bot-name">
                        {a.name}
                        {mine.length > 1 && <span className="cv-muted"> ×{mine.length}</span>}
                        {leads && <span className="cv-bot-lead" title={t('canvas.role_lead')}> ★</span>}
                      </span>
                      <span className="cv-bot-state">{t(`canvas.bot_${state}`)}</span>
                    </span>
                    <span className={`cv-bot-dot st-${state}`} />
                  </button>
                  {mine.length > 0 && !leads && (
                    <button className="cv-icon-btn" title={t('canvas.makeLead')} onClick={() => view && void call('team.update', { channelId: view.channel.id, leadId: mine[0].id })}>☆</button>
                  )}
                  <button className="cv-icon-btn" onClick={() => void addNode(a.id)} disabled={!view || !!stop?.requested} title={t('canvas.addNode')}>＋</button>
                </div>
              )
            })}
          </div>
          <label className="cv-root-toggle" title={t('canvas.rootHint')}>
            <input type="checkbox" checked={inRoot} onChange={(e) => setInRoot(e.target.checked)} />
            {t('canvas.inRoot')}
          </label>
        </aside>

        <main
          className="cv-center"
          onDragOver={(e) => {
            if (e.dataTransfer.types.includes('application/x-canvas-agent')) e.preventDefault()
          }}
          onDrop={(e) => {
            const agent = e.dataTransfer.getData('application/x-canvas-agent')
            if (!agent) return
            e.preventDefault()
            void addNode(agent, mode === 'canvas' ? flow.screenToFlowPosition({ x: e.clientX, y: e.clientY }) : undefined)
          }}
        >
          {mode === 'canvas' ? (
            <ReactFlow
              nodes={nodes}
              edges={edges}
              nodeTypes={nodeTypes}
              onNodesChange={onNodesChange}
              onNodeDragStop={(_, n) => saveLayout(n)}
              onConnect={(c) => void onConnect(c)}
              onNodeClick={(_, n) => n.type === 'terminal' && setSelection({ kind: 'node', id: n.id })}
              onEdgeClick={(_, e) => {
                if (e.id.startsWith('flow:')) return
                setSelection({ kind: 'edge', id: e.id })
                setTab('details')
              }}
              onPaneClick={() => setSelection(null)}
              onMove={(_, vp) => setZoom(vp.zoom)}
              // 離開可視範圍的節點卸下呈現（退訂畫面資料），回來時由快照接回；行程不受影響（§3.8）
              onlyRenderVisibleElements
              minZoom={0.2}
              maxZoom={2}
              fitView
              proOptions={{ hideAttribution: true }}
            >
              <Background variant={BackgroundVariant.Dots} gap={18} size={1} color="var(--canvas-dot)" />
              <Controls showInteractive={false} />
              <MiniMap pannable zoomable />
            </ReactFlow>
          ) : (
            view && (
              <CardsView
                view={view}
                call={call}
                selectedNode={selectedNode}
                nodeName={nodeName}
                markOfNode={markOfNode}
                onSelect={(id) => setSelection({ kind: 'node', id })}
                onShow={pickNode}
                onTakeOver={takeOver}
                timeline={<FeedView events={view.channel.events} nodeName={nodeName} onPick={pickNode} />}
              />
            )
          )}
          {view && view.channel.nodes.length === 0 && mode === 'canvas' && <div className="cv-empty">{t('canvas.empty')}</div>}
        </main>

        <div className="cv-rightwrap">
          <div className="cv-right-head">
            <span className="cv-right-title">{t('canvas.collab')}</span>
            <div className="segmented small">
              <button className={tab === 'chat' ? 'on' : ''} onClick={() => setTab('chat')}>{t('canvas.tabChat')}</button>
              <button className={tab === 'details' ? 'on' : ''} onClick={() => setTab('details')}>{t('canvas.tabDetails')}</button>
            </div>
          </div>
          {tab === 'chat' && view ? (
            <ChatPanel view={view} call={call} selectedNode={selectedNode} nodeName={nodeName} markOfNode={markOfNode} onPickNode={pickNode} onTakeOver={takeOver} />
          ) : (
            <Inspector view={view} selection={selection} call={call} agentName={agentName} nodeName={nodeName} />
          )}
        </div>
      </div>
    </div>
  )
}

function FeedView({ events, nodeName, onPick }: { events: ChannelEvent[]; nodeName: (id: string) => string; onPick: (node: string) => void }): JSX.Element {
  const { t } = useTranslation()
  const who = (id: string): string => (id === 'app' ? 'Agent Workbench' : id === 'user' ? t('canvas.you') : nodeName(id))
  const text = (e: ChannelEvent): string => {
    switch (e.t) {
      case 'team-resumed':
        return t('canvas.ev_team_resumed')
      case 'team-paused':
        return t('canvas.ev_team_paused', { reason: e.reason })
      case 'message':
        return t(e.held ? 'canvas.ev_message_held' : 'canvas.ev_message', { from: who(e.from), to: who(e.to), kind: t(`canvas.kind_${e.kind}`) })
      case 'task':
        return t('canvas.ev_task', { node: nodeName(e.node), status: t(`canvas.task_${e.status}`) })
      case 'team-merge':
        return t('canvas.ev_team_merge', { member: nodeName(e.member), decision: e.decision })
      case 'relay':
        return t('canvas.ev_relay', { decision: e.decision })
      case 'node-started':
      case 'node-exited':
      case 'merged':
      case 'discarded':
        return t(`canvas.ev_${e.t.replace('-', '_')}`, { node: nodeName(e.node) })
      case 'approval':
        return t(`canvas.ev_approval_${e.decision || e.status}`, { node: nodeName(e.node) })
      case 'handoff':
        return t('canvas.ev_handoff', { from: nodeName(e.from), to: nodeName(e.to) })
      default:
        return t(`canvas.ev_${e.t.replace('-', '_')}`)
    }
  }
  const nodeOf = (e: ChannelEvent): string | null => ('node' in e ? e.node : 'to' in e && e.to !== 'user' ? e.to : null)
  return (
    <div className="cv-feed">
      {events.length === 0 && <p className="cv-muted">{t('canvas.noEvents')}</p>}
      {[...events].reverse().map((e, i) => {
        const node = nodeOf(e)
        return (
          <button key={i} className="cv-feed-row" disabled={!node} onClick={() => node && onPick(node)}>
            <span className="cv-feed-time">{new Date(e.at).toLocaleTimeString()}</span>
            <span>{text(e)}</span>
            {e.t === 'handoff' && <pre className="cv-feed-text">{e.text}</pre>}
          </button>
        )
      })}
    </div>
  )
}

export type { CanvasNode }
