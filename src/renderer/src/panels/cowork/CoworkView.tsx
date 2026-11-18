// Cowork（cowork-agui-implement.md §8，照 OpenBot README 的 demo 影片）：左欄是對話清單，中間是首頁「Start a
// conversation」或一段對話，右側是 Workspace 面板。一段對話＝一個 Channel＝一個 worktree＋一個 Bot session。
// 狀態一律以後端（wb-core canvas.rs）推來的 view 為準；這裡只送操作。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import './cowork.css'
import { useWorkbench, setUiMode } from '@/store'
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import type { CliLauncher } from '../../../../preload/index'
import type { ChannelSummary, ChannelView, NodeMode, StopState } from '../../../../shared/canvas'
import BotsView from './BotsView'
import Conversation, { leadOf } from './Conversation'
import Workspace from './Workspace'
import { ago, BUILTIN_AGENTS, CHAT_AGENTS, markOf, recall, remember, type CanvasCall } from './types'

/** A terminal CLI gets the first message once its prompt is up; after this it is left for the user to paste */
const FIRST_MESSAGE_WAIT_MS = 90_000

function titleOf(text: string): string {
  const line = text.trim().split('\n')[0].replace(/\s+/g, ' ')
  return line.length > 40 ? `${line.slice(0, 39)}…` : line
}

function Home({
  launchers,
  canChat,
  onStart,
  busy
}: {
  launchers: CliLauncher[]
  canChat: (agent: string) => boolean
  onStart: (agent: string, mode: NodeMode, text: string) => Promise<boolean>
  busy: boolean
}): JSX.Element {
  const { t } = useTranslation()
  const { workspaceRoot } = useWorkbench()
  const agentKey = `wb-cowork-agent:${workspaceRoot}`
  const modeKey = `wb-cowork-mode:${workspaceRoot}`
  // 預設：這個 repo 上次用的 Bot，沒有就 Claude Code；模式預設終端（使用者 2026-11-18 決定）
  const [agent, setAgent] = useState(() => recall(agentKey) || 'claude')
  const [mode, setMode] = useState<NodeMode>(() => (recall(modeKey) === 'chat' ? 'chat' : 'terminal'))
  const [text, setText] = useState('')
  const chatOk = canChat(agent)
  const effective: NodeMode = chatOk ? mode : 'terminal'

  const submit = async (): Promise<void> => {
    let body = text.trim()
    let pick = agent
    // "@Codex fix…" picks the Bot and is not part of the message
    const m = /^@(\S+)\s*/.exec(body)
    if (m) {
      const want = m[1].toLowerCase()
      const hit = launchers.find((l) => l.id.toLowerCase() === want || l.name.replace(/\s+/g, '').toLowerCase() === want)
      if (hit) {
        pick = hit.id
        body = body.slice(m[0].length)
      }
    }
    if (!body) return
    const md: NodeMode = canChat(pick) ? mode : 'terminal'
    remember(agentKey, pick)
    remember(modeKey, mode)
    if (await onStart(pick, md, body)) setText('')
  }

  return (
    <main className="cw-main cw-home">
      <div className="cw-home-inner">
        <h1>{t('cw.startTitle')}</h1>
        <div className="cw-row center">
          <span className="cw-chip" title={workspaceRoot}>{t('cw.repo')}: {workspaceRoot.split(/[\\/]/).pop()}</span>
          <span className="cw-chip">{t('cw.newWorktree')}</span>
        </div>
        <div className="cw-composer big">
          <textarea
            rows={3}
            value={text}
            placeholder={t('cw.askAnything')}
            aria-label={t('cw.askAnything')}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault()
                void submit()
              }
            }}
          />
          <div className="cw-row">
            <label className="cw-select">
              <span className="cw-sr">{t('cw.bot')}</span>
              <select value={agent} onChange={(e) => setAgent(e.target.value)}>
                {launchers.map((l) => (
                  <option key={l.id} value={l.id}>{l.name}</option>
                ))}
              </select>
            </label>
            <div className="cw-seg" role="radiogroup" aria-label={t('cw.mode')}>
              {(['chat', 'terminal'] as const).map((m) => (
                <button key={m} role="radio" aria-checked={effective === m} className={effective === m ? 'on' : ''} disabled={m === 'chat' && !chatOk} title={m === 'chat' && !chatOk ? t('cw.noChatMode') : ''} onClick={() => setMode(m)}>
                  {t(`cw.mode_${m}`)}
                </button>
              ))}
            </div>
            <div className="cw-grow" />
            <button className="cw-send" aria-label={t('cw.send')} disabled={busy || !text.trim()} onClick={() => void submit()}>↑</button>
          </div>
        </div>
        <p className="cw-muted center">{t('cw.startHint')}</p>
        <h2>{t('cw.exploreBots')}</h2>
        <div className="cw-bots">
          {launchers.map((l) => (
            <button key={l.id} className={`cw-bot ${agent === l.id ? 'on' : ''}`} onClick={() => setAgent(l.id)}>
              <AgentMark agent={markOf(l.cli)} size={26} />
              <span>
                <b>{l.name}</b>
                <span className="cw-muted">{canChat(l.id) ? t('cw.botBoth') : t('cw.botTerminal')}</span>
              </span>
            </button>
          ))}
        </div>
      </div>
    </main>
  )
}

export default function CoworkView(): JSX.Element {
  const { t } = useTranslation()
  const { workspaceRoot } = useWorkbench()
  const storeKey = `wb-cowork-conversation:${workspaceRoot}`
  const [channels, setChannels] = useState<ChannelSummary[]>([])
  const [active, setActive] = useState<string | null>(() => recall(storeKey))
  const [view, setView] = useState<ChannelView | null>(null)
  const [launchers, setLaunchers] = useState<CliLauncher[]>(BUILTIN_AGENTS)
  const [chatCustom, setChatCustom] = useState(false)
  const [panel, setPanel] = useState(() => recall('wb-cowork-panel') !== 'closed')
  const [stop, setStop] = useState<StopState | null>(null)
  const [query, setQuery] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  /** The Bot shown in the conversation; null = its lead */
  const [focus, setFocus] = useState<string | null>(null)
  /** With no conversation open: the home page or the Bots page */
  const [page, setPage] = useState<'home' | 'bots'>('home')
  /** Bumped when a Bot was saved, so its name and chat mode show up without a reload */
  const [botsVersion, setBotsVersion] = useState(0)
  const activeRef = useRef(active)
  activeRef.current = active
  /** A terminal conversation's first message, waiting for its CLI prompt */
  const first = useRef<{ channelId: string; nodeId: string; text: string; until: number } | null>(null)

  const call: CanvasCall = useCallback(async (op, req) => {
    setError(null)
    const r = await window.api.canvas.call(op, req)
    if (r.ok) return (r.data ?? null) as never
    const key = `canvas.err_${r.code.replace(/-/g, '_')}`
    const known = t(key)
    setError(known !== key ? (r.message && r.code === 'invalid' ? `${known}: ${r.message}` : known) : r.message || r.code)
    return undefined
  }, [t])

  const loadList = useCallback(async () => {
    const list = await call('channels.list')
    if (list) setChannels(list)
  }, [call])

  useEffect(() => {
    let alive = true
    void loadList()
    window.api.pty
      .launchers()
      .then((l) => alive && setLaunchers([...BUILTIN_AGENTS, ...l.filter((x) => !BUILTIN_AGENTS.some((b) => b.id === x.id))]))
      .catch(() => {})
    window.api.settings
      .get()
      .then((s) => alive && setChatCustom(['claude-stream-json', 'codex-app-server', 'acp', 'agui-http'].includes((s as { customAgent?: { protocol?: string } }).customAgent?.protocol ?? '')))
      .catch(() => {})
    return () => {
      alive = false
    }
  }, [workspaceRoot, loadList, botsVersion])

  useEffect(() => {
    remember(storeKey, active)
    setView(null)
    setFocus(null)
    if (!active) return
    void call('channel.get', { channelId: active }).then((v) => {
      if (v && activeRef.current === active) {
        setView(v)
        setStop(v.stop)
      } else if (!v) setActive(null)
    })
  }, [active, call, storeKey])

  const deliverFirst = useCallback(
    (v: ChannelView) => {
      const f = first.current
      if (!f || f.channelId !== v.channel.id) return
      const rt = v.runtime[f.nodeId]
      if (Date.now() > f.until) {
        first.current = null
        void navigator.clipboard.writeText(f.text).catch(() => {})
        setError(t('cw.firstNotSent'))
        return
      }
      if (rt?.status === 'running' && rt.inputReady && rt.sessionGeneration) {
        first.current = null
        void call('node.message', { channelId: f.channelId, nodeId: f.nodeId, sessionGeneration: rt.sessionGeneration, text: f.text })
      }
    },
    [call, t]
  )

  useEffect(
    () =>
      window.api.canvas.onUpdate((u) => {
        if (u.view) setStop(u.view.stop)
        if (u.channelId === activeRef.current) {
          if (u.view) setView(u.view)
          else setActive(null)
        }
        if (u.view) deliverFirst(u.view)
        void loadList()
      }),
    [loadList, deliverFirst]
  )

  const canChat = useCallback((agent: string) => CHAT_AGENTS.includes(agent) || (agent === 'custom-agent' && chatCustom), [chatCustom])
  const agentName = useCallback((id: string) => launchers.find((l) => l.id === id)?.name ?? id, [launchers])

  const start = async (agent: string, mode: NodeMode, text: string): Promise<boolean> => {
    setBusy(true)
    try {
      const v = await call('channel.create', { name: titleOf(text) })
      if (!v) return false
      const channelId = v.channel.id
      setActive(channelId)
      const node = await call('node.create', { channelId, agent, workspace: 'worktree', mode })
      if (!node) return false
      // Through the group's inbox, so the first turn carries the brief and the team tools are explained
      if (mode === 'chat') await call('chat.send', { channelId, text })
      else first.current = { channelId, nodeId: node.id, text, until: Date.now() + FIRST_MESSAGE_WAIT_MS }
      void loadList()
      return true
    } finally {
      setBusy(false)
    }
  }

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase()
    return q ? channels.filter((c) => c.name.toLowerCase().includes(q) || (c.lead && agentName(c.lead.agent).toLowerCase().includes(q))) : channels
  }, [channels, query, agentName])

  const togglePanel = (): void => {
    remember('wb-cowork-panel', panel ? 'closed' : 'open')
    setPanel(!panel)
  }

  return (
    <div className="cw-root">
      <nav className="cw-side" aria-label={t('cw.conversations')}>
        <div className="cw-row">
          <button className="cw-modeback" title={t('cw.backToDeveloper')} onClick={() => setUiMode('developer')}>←</button>
          <b>Cowork</b>
          <div className="cw-grow" />
          <button className="cw-icon" aria-label={t('cw.newConversation')} title={t('cw.newConversation')} onClick={() => { setPage('home'); setActive(null) }}>＋</button>
        </div>
        <label className="cw-search">
          <span className="cw-sr">{t('cw.search')}</span>
          <input value={query} placeholder={t('cw.search')} onChange={(e) => setQuery(e.target.value)} />
        </label>
        <div className="cw-list">
          {shown.length === 0 && <p className="cw-muted">{t('cw.noConversations')}</p>}
          {shown.map((c) => {
            const l = c.lead
            const state = c.pendingApprovals > 0 ? 'waiting' : l?.inRun ? 'busy' : l?.status === 'running' ? 'idle' : 'ended'
            return (
              <button key={c.id} className={`cw-conv ${c.id === active ? 'on' : ''}`} onClick={() => setActive(c.id)}>
                {l ? <AgentMark agent={markOf(l.agent)} size={20} /> : <span className="cw-orb" />}
                <span className="cw-conv-text">
                  <span className="cw-conv-meta">{l ? `${agentName(l.agent)} · ${l.mode === 'chat' ? t('cw.mode_chat') : t('cw.mode_terminal')}` : ''}</span>
                  <span className="cw-conv-name">{c.name}</span>
                </span>
                <span className="cw-conv-right">
                  <span className="cw-muted">{c.updatedAt ? ago(c.updatedAt) : ''}</span>
                  {state !== 'ended' && <span className={`cw-dot st-${state}`} title={t(`cw.state_${state}`)} />}
                </span>
              </button>
            )
          })}
        </div>
        <div className="cw-side-foot">
          <button className={`cw-nav ${!active && page === 'bots' ? 'on' : ''}`} onClick={() => { setPage('bots'); setActive(null) }}>
            <span>{t('cw.bots')}</span>
            {channels.some((c) => c.pendingApprovals > 0) && <span className="cw-badge warn">{t('cw.needsYou')}</span>}
          </button>
          {stop?.requested ? (
            <div className="cw-stopped">
              <b>{t('cw.stoppedAll')}</b>
              <div className="cw-row">
                <button className="cw-btn" onClick={() => void call('stop.clear').then((s) => s && setStop(s))}>{t('cw.clearStop')}</button>
                <button className="cw-btn danger" onClick={() => window.confirm(t('canvas.killAllConfirm')) && void call('stop.killAll')}>{t('cw.killAll')}</button>
              </div>
            </div>
          ) : (
            <button className="cw-btn danger wide" onClick={() => void call('stop').then((s) => s && setStop(s))}>■ {t('cw.stopAll')}</button>
          )}
        </div>
      </nav>

      {active && view ? (
        <Conversation view={view} call={call} agentName={agentName} launchers={launchers} canChat={canChat} focusId={focus} onFocus={setFocus} panelOpen={panel} onTogglePanel={togglePanel} />
      ) : page === 'bots' ? (
        <BotsView
          launchers={launchers}
          canChat={canChat}
          channels={channels}
          agentName={agentName}
          onOpen={(id) => setActive(id)}
          onSaved={() => setBotsVersion((v) => v + 1)}
          onStart={(agent) => {
            remember(`wb-cowork-agent:${workspaceRoot}`, agent)
            setPage('home')
          }}
        />
      ) : (
        <Home launchers={launchers} canChat={canChat} onStart={start} busy={busy} />
      )}
      {active && view && panel && <Workspace view={view} call={call} nodeId={focus && view.channel.nodes.some((n) => n.id === focus) ? focus : leadOf(view)} onClose={togglePanel} />}
      {error && (
        <div className="cw-toast" role="alert">
          {error}
          <button className="cw-icon" aria-label={t('common.close')} onClick={() => setError(null)}>✕</button>
        </div>
      )}
    </div>
  )
}
