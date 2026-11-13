// Cowork 2.0 畫布模式（cowork2.0-implement.md §2）：左欄 Channels／Agents、中央 React Flow 畫布或 Feed、右欄 Inspector。
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
import Inspector, { type Selection } from './Inspector'
import { TeamFeed, TeamFrame, type TeamFrameData } from './TeamPanels'

const NODE_SIZE = { width: 640, height: 400 }
const nodeTypes = { terminal: TerminalNode, team: TeamFrame }
type CanvasNodeData = TerminalNodeData | TeamFrameData

export type CanvasCall = <K extends CanvasOp>(op: K, req?: CanvasOps[K]['req']) => Promise<CanvasOps[K]['res'] | undefined>

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
  const [mode, setMode] = useState<'canvas' | 'feed'>('canvas')
  const [inRoot, setInRoot] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [nodes, setNodes] = useState<Node<CanvasNodeData>[]>([])
  const [zoom, setZoom] = useState(1)
  const [recentEdge, setRecentEdge] = useState<string | null>(null)
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

  // 只在 App 有焦點時作用；Claude Code 自己用 Esc／雙 Esc，所以不用 Esc（§2.3）
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.ctrlKey && e.shiftKey && e.key === 'Backspace') {
        e.preventDefault()
        void call('stop')
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
          data: { node: n, rt: view.runtime[n.id], channelId: view.channel.id, name: agentName(n.agent), mark: markOf(agents.find((a) => a.id === n.agent)?.cli || n.agent), zoom, stopped: view.stop.requested, call }
        }
      })
      // A frame behind each team, sized from its members as drawn; it never moves on its own
      const frames: Node<CanvasNodeData>[] = view.teams.flatMap((team) => {
        const rects = terminals.filter((n) => team.memberIds.includes(n.id)).map((n) => ({ x: n.position.x, y: n.position.y, w: Number(n.style?.width), h: Number(n.style?.height) }))
        if (rects.length === 0) return []
        const x = Math.min(...rects.map((r) => r.x)) - 24
        const y = Math.min(...rects.map((r) => r.y)) - 56
        const w = Math.max(...rects.map((r) => r.x + r.w)) - x + 24
        const h = Math.max(...rects.map((r) => r.y + r.h)) - y + 24
        const pending = view.relays.filter((m) => m.teamId === team.id).length + view.mergeRequests.filter((m) => m.team === team.id).length
        return [{ id: `team:${team.id}`, type: 'team', position: { x, y }, style: { width: w, height: h, pointerEvents: 'none' }, zIndex: -1, draggable: false, selectable: false, focusable: false, selected: selection?.kind === 'team' && selection.id === team.id, data: { team, pending, onPick: (id: string) => setSelection({ kind: 'team', id }) } }]
      })
      return [...frames, ...terminals]
    })
  }, [view, selection, agentName, agents, zoom, call])

  const edges: Edge[] = useMemo(
    () => [
      ...(view?.channel.edges || []).map((e) => ({
        id: e.id,
        source: e.from,
        target: e.to,
        label: t('canvas.handoff'),
        animated: recentEdge === e.id,
        selected: selection?.kind === 'edge' && selection.id === e.id,
        className: 'cv-edge'
      })),
      // Lead to member: the routes the team's messages may take
      ...(view?.teams || []).flatMap((team) =>
        team.memberIds.filter((m) => m !== team.leadId).map((m) => ({ id: `team:${team.id}:${m}`, source: team.leadId, target: m, selectable: false, focusable: false, animated: team.status === 'running', className: 'cv-edge-team' }))
      )
    ],
    [view, recentEdge, selection, t]
  )

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
    const pos = position || flow.screenToFlowPosition({ x: window.innerWidth / 2 - NODE_SIZE.width / 2, y: window.innerHeight / 2 - NODE_SIZE.height / 2 })
    const n = await call('node.create', { channelId: view.channel.id, agent, workspace: inRoot ? 'root' : 'worktree', position: { x: Math.round(pos.x), y: Math.round(pos.y) }, size: NODE_SIZE })
    if (n) setSelection({ kind: 'node', id: n.id })
  }

  const onConnect = async (c: Connection): Promise<void> => {
    if (!view || !c.source || !c.target || c.source === c.target) return
    const edge = await call('handoff.create', { channelId: view.channel.id, from: c.source, to: c.target, note: '' })
    if (!edge) return
    setSelection({ kind: 'edge', id: edge.id })
    // 流動光點只在交接後幾秒播放（§3.5）
    setRecentEdge(edge.id)
    window.setTimeout(() => setRecentEdge((x) => (x === edge.id ? null : x)), 4000)
  }

  const pickNode = (id: string): void => {
    setSelection({ kind: 'node', id })
    setMode('canvas')
    const n = view?.channel.nodes.find((x) => x.id === id)
    if (n) void flow.setCenter(n.position.x + n.size.width / 2, n.position.y + n.size.height / 2, { zoom: 1, duration: 300 })
  }

  const nodeName = (id: string): string => {
    const n = view?.channel.nodes.find((x) => x.id === id)
    return n ? `${agentName(n.agent)} · ${n.id.slice(0, 4)}` : id.slice(0, 4)
  }

  const stop = view?.stop
  // Everything that waits on the user in this channel: CLI approvals, held messages, merge requests
  const pendingCount = view ? Object.values(view.runtime).filter((r) => r.approval).length + view.relays.length + view.mergeRequests.length : 0
  const createChannel = async (): Promise<void> => {
    const name = window.prompt(t('canvas.channelName'), '')
    if (!name?.trim()) return
    const v = await call('channel.create', { name })
    if (v) setChannelId(v.channel.id)
  }

  return (
    <div className="cv-root">
      <div className="cv-topbar">
        <div className="segmented">
          {(['developer', 'vibe', 'cowork'] as const).map((m) => (
            <button key={m} className={uiMode === m ? 'on' : ''} onClick={() => setUiMode(m)}>
              {t(`vibe.${m}Title`)}
            </button>
          ))}
        </div>
        <div className="segmented">
          <button className={mode === 'canvas' ? 'on' : ''} onClick={() => setMode('canvas')}>{t('canvas.canvas')}</button>
          <button className={mode === 'feed' ? 'on' : ''} onClick={() => setMode('feed')}>{t('canvas.feed')}</button>
        </div>
        <span className="cv-spacer" />
        {pendingCount > 0 && (
          <button className="cv-btn primary" onClick={() => setMode('feed')}>
            {t('canvas.pending', { n: pendingCount })}
          </button>
        )}
        {error && <span className="cv-error" role="alert">{error}</span>}
        {stop?.requested ? (
          <>
            <span className="cv-stop-note" title={t('canvas.stopRequestedHint')}>{t('canvas.stopRequested')}</span>
            <button className="cv-btn danger" onClick={() => window.confirm(t('canvas.killAllConfirm')) && void call('stop.killAll')}>{t('canvas.killAll')}</button>
            <button className="cv-btn" onClick={() => void call('stop.clear')}>{t('canvas.clearStop')}</button>
          </>
        ) : (
          <button className="cv-btn danger" title={t('canvas.stopHint')} onClick={() => void call('stop')} disabled={!view}>
            ■ {t('canvas.stop')}
          </button>
        )}
      </div>

      <div className="cv-body">
        <aside className="cv-left">
          <div className="cv-section-head">
            <span>{t('canvas.channels')}</span>
            <button className="cv-icon-btn" title={t('canvas.newChannel')} onClick={createChannel}>＋</button>
          </div>
          <div className="cv-list">
            {channels.map((c) => (
              <button
                key={c.id}
                className={`cv-list-item ${c.id === channelId ? 'on' : ''}`}
                onClick={() => setChannelId(c.id)}
                onDoubleClick={async () => {
                  const name = window.prompt(t('canvas.channelName'), c.name)
                  if (name?.trim()) await call('channel.rename', { channelId: c.id, name })
                }}
              >
                <span># {c.name}</span>
                {c.pendingApprovals > 0 && <span className="cv-badge">{c.pendingApprovals}</span>}
              </button>
            ))}
          </div>
          <div className="cv-section-head">
            <span>{t('canvas.agents')}</span>
          </div>
          <label className="cv-root-toggle" title={t('canvas.rootHint')}>
            <input type="checkbox" checked={inRoot} onChange={(e) => setInRoot(e.target.checked)} />
            {t('canvas.inRoot')}
          </label>
          <div className="cv-list">
            {agents.map((a) => (
              <div
                key={a.id}
                className="cv-agent"
                draggable
                onDragStart={(e) => e.dataTransfer.setData('application/x-canvas-agent', a.id)}
                title={t('canvas.agentHint')}
              >
                <AgentMark agent={markOf(a.cli || a.id)} size={16} />
                <span className="cv-agent-main">
                  <span>{a.name}</span>
                  {/* 實際會執行的指令：yaml 不能拿名稱偽裝（§2.3） */}
                  <span className="cv-agent-cmd">{[a.command, ...(a.args || [])].join(' ')}</span>
                </span>
                <button className="cv-icon-btn" onClick={() => void addNode(a.id)} disabled={!view || !!stop?.requested} title={t('canvas.addNode')}>＋</button>
              </div>
            ))}
          </div>
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
            void addNode(agent, flow.screenToFlowPosition({ x: e.clientX, y: e.clientY }))
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
              onEdgeClick={(_, e) => !e.id.startsWith('team:') && setSelection({ kind: 'edge', id: e.id })}
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
              <TeamFeed
                view={view}
                call={call}
                nodeName={nodeName}
                onPick={pickNode}
                timeline={<FeedView events={view.channel.events} nodeName={nodeName} teamName={(id) => view.teams.find((x) => x.id === id)?.name || id} onPick={pickNode} />}
              />
            )
          )}
          {view && view.channel.nodes.length === 0 && mode === 'canvas' && <div className="cv-empty">{t('canvas.empty')}</div>}
        </main>

        <Inspector view={view} selection={selection} call={call} agentName={agentName} nodeName={nodeName} onPickTeam={(id) => setSelection({ kind: 'team', id })} />
      </div>
    </div>
  )
}

function FeedView({ events, nodeName, teamName, onPick }: { events: ChannelEvent[]; nodeName: (id: string) => string; teamName: (id: string) => string; onPick: (node: string) => void }): JSX.Element {
  const { t } = useTranslation()
  const who = (id: string): string => (id === 'app' ? 'Agent Workbench' : id === 'user' ? '—' : nodeName(id))
  const text = (e: ChannelEvent): string => {
    switch (e.t) {
      case 'team-started':
      case 'team-resumed':
      case 'team-done':
        return t(`canvas.ev_${e.t.replace('-', '_')}`, { team: teamName(e.team) })
      case 'team-paused':
        return t('canvas.ev_team_paused', { team: teamName(e.team), reason: e.reason })
      case 'message':
        return t(e.held ? 'canvas.ev_message_held' : 'canvas.ev_message', { from: who(e.from), to: who(e.to), kind: e.kind })
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
  const nodeOf = (e: ChannelEvent): string | null => ('node' in e ? e.node : 'to' in e ? e.to : null)
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
