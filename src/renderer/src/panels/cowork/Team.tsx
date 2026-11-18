// 群組（cowork-agui-implement.md §6.8、§8.3）：一段對話裡的 Bot 們、要你處理的卡片（轉送審批、合併、提問）、
// 委派出去的任務卡與任務板。Agent 寫的字一律當純文字。
import { useState } from 'react'
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import type { CliLauncher } from '../../../../preload/index'
import type { CanvasNode, ChannelView, NodeMode } from '../../../../shared/canvas'
import { markOf, type CanvasCall } from './types'

/** `@name` for each node: the Bot's name without spaces, numbered when two share it */
export function handles(nodes: CanvasNode[], agentName: (id: string) => string): Map<string, string> {
  const out = new Map<string, string>()
  const seen = new Map<string, number>()
  for (const n of nodes) {
    const base = agentName(n.agent).replace(/\s+/g, '').toLowerCase()
    const k = (seen.get(base) ?? 0) + 1
    seen.set(base, k)
    out.set(n.id, k === 1 ? base : `${base}-${k}`)
  }
  return out
}

export function MembersBar({
  view,
  call,
  agentName,
  launchers,
  canChat,
  focus,
  onFocus
}: {
  view: ChannelView
  call: CanvasCall
  agentName: (id: string) => string
  launchers: CliLauncher[]
  canChat: (agent: string) => boolean
  focus: string
  onFocus: (nodeId: string) => void
}): JSX.Element {
  const { t } = useTranslation()
  const [adding, setAdding] = useState(false)
  const [agent, setAgent] = useState('codex')
  const [mode, setMode] = useState<NodeMode>('chat')
  const [role, setRole] = useState('')
  const [busy, setBusy] = useState(false)
  const team = view.teams[0]
  const lead = team?.leadId ?? view.channel.nodes[0]?.id
  const tags = handles(view.channel.nodes, agentName)
  const md: NodeMode = canChat(agent) ? mode : 'terminal'

  const add = async (): Promise<void> => {
    setBusy(true)
    const node = await call('node.create', { channelId: view.channel.id, agent, workspace: 'worktree', mode: md, role: role.trim() || undefined })
    setBusy(false)
    if (node) {
      setAdding(false)
      setRole('')
    }
  }

  return (
    <div className="cw-members">
      <div className="cw-row">
        {view.channel.nodes.map((n) => {
          const rt = view.runtime[n.id]
          const state = rt?.approval ? 'waiting' : rt?.inRun || rt?.busy ? 'busy' : rt?.status === 'running' ? 'idle' : 'ended'
          return (
            <button key={n.id} className={`cw-member ${focus === n.id ? 'on' : ''}`} aria-pressed={focus === n.id} title={n.role || ''} onClick={() => onFocus(n.id)}>
              <AgentMark agent={markOf(n.agent)} size={16} />
              <span>@{tags.get(n.id)}</span>
              {n.id === lead && <span className="cw-badge">{t('cw.lead')}</span>}
              {n.role && <span className="cw-muted cw-member-role">{n.role}</span>}
              {(rt?.unread ?? 0) > 0 && <span className="cw-badge warn">{rt?.unread}</span>}
              <span className={`cw-dot st-${state}`} />
            </button>
          )
        })}
        <button className="cw-btn" aria-expanded={adding} onClick={() => setAdding(!adding)}>＋ {t('cw.addBot')}</button>
        <div className="cw-grow" />
        {team && view.channel.nodes.length > 1 && (
          <label className="cw-toggle" title={t('cw.freeTalkHint')}>
            <input type="checkbox" checked={!!team.freeTalk} onChange={(e) => void call('team.update', { channelId: view.channel.id, freeTalk: e.target.checked })} />
            {t('cw.freeTalk')}
          </label>
        )}
      </div>
      {team?.freeTalk && view.channel.nodes.length > 1 && <div className="cw-muted">{t('cw.freeTalkOn')}</div>}
      {adding && (
        <div className="cw-row cw-addbot">
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
              <button key={m} role="radio" aria-checked={md === m} className={md === m ? 'on' : ''} disabled={m === 'chat' && !canChat(agent)} onClick={() => setMode(m)}>
                {t(`cw.mode_${m}`)}
              </button>
            ))}
          </div>
          <input className="cw-input cw-grow" value={role} maxLength={200} placeholder={t('cw.rolePlaceholder')} aria-label={t('cw.role')} onChange={(e) => setRole(e.target.value)} />
          <button className="cw-btn dark" disabled={busy} onClick={() => void add()}>{t('cw.add')}</button>
        </div>
      )}
    </div>
  )
}

/** Cards that wait for the user: agent messages held for approval, merges into the lead, ask_person questions */
export function NeedsYou({ view, call, agentName }: { view: ChannelView; call: CanvasCall; agentName: (id: string) => string }): JSX.Element | null {
  const { t } = useTranslation()
  const [answers, setAnswers] = useState<Record<string, string>>({})
  /** Answered here: hidden at once, so a slow merge or a double click never answers a card twice */
  const [gone, setGone] = useState<Set<string>>(new Set())
  const once = (id: string, fn: () => Promise<unknown>): void => {
    setGone((s) => new Set(s).add(id))
    void fn().then((r) => r === undefined && setGone((s) => (s.delete(id), new Set(s))))
  }
  const ch = view.channel.id
  const tags = handles(view.channel.nodes, agentName)
  const who = (id: string): string => (id === 'app' ? 'Agent Workbench' : id === 'user' ? t('cw.you') : `@${tags.get(id) ?? id}`)
  const relays = view.relays.filter((m) => !gone.has(m.id))
  const merges = view.mergeRequests.filter((r) => !gone.has(r.id))
  const asks = (view.asks ?? []).filter((a) => !gone.has(a.id))
  if (!relays.length && !merges.length && !asks.length) return null
  return (
    <div className="cw-needs">
      {relays.map((m) => (
        <div key={m.id} className="cw-card cw-approval">
          <b>{t('cw.relayTitle', { from: who(m.from), to: who(m.to), kind: m.kind })}</b>
          <pre className="cw-pre">{m.text}</pre>
          <div className="cw-row">
            <button className="cw-btn dark" onClick={() => once(m.id, () => call('relay.approve', { channelId: ch, messageId: m.id }))}>{t('cw.relaySend')}</button>
            <button className="cw-btn" onClick={() => once(m.id, () => call('relay.reject', { channelId: ch, messageId: m.id }))}>{t('cw.relayReject')}</button>
          </div>
        </div>
      ))}
      {merges.map((r) => (
        <div key={r.id} className="cw-card cw-approval">
          <b>{t('cw.mergeTitle', { member: who(r.member), lead: who(r.lead) })}</b>
          <pre className="cw-pre">{r.commits.join('\n') || t('cw.noCommits')}</pre>
          <div className="cw-row">
            <button className="cw-btn dark" onClick={() => once(r.id, () => call('teammerge.approve', { channelId: ch, requestId: r.id }))}>{t('cw.mergeOk')}</button>
            <button className="cw-btn" onClick={() => once(r.id, () => call('teammerge.reject', { channelId: ch, requestId: r.id }))}>{t('cw.relayReject')}</button>
          </div>
        </div>
      ))}
      {asks.map((a) => {
        const reply = (text: string): void => once(a.id, () => call('ask.answer', { channelId: ch, askId: a.id, text }))
        return (
          <div key={a.id} className="cw-card cw-ask">
            <b>{t('cw.askTitle', { who: who(a.node) })}</b>
            <div className="cw-ask-q">{a.question}</div>
            {a.choices.length > 0 && (
              <div className="cw-row">
                {a.choices.map((c) => (
                  <button key={c} className="cw-btn" onClick={() => reply(c)}>{c}</button>
                ))}
              </div>
            )}
            <div className="cw-row">
              <input className="cw-input cw-grow" value={answers[a.id] ?? ''} aria-label={t('cw.askAnswer')} placeholder={t('cw.askAnswer')} onChange={(e) => setAnswers({ ...answers, [a.id]: e.target.value })} onKeyDown={(e) => e.key === 'Enter' && answers[a.id]?.trim() && reply(answers[a.id].trim())} />
              <button className="cw-btn dark" disabled={!answers[a.id]?.trim()} onClick={() => reply(answers[a.id].trim())}>{t('cw.send')}</button>
            </div>
          </div>
        )
      })}
    </div>
  )
}

/** Delegated work: each member's task (OpenBot's task thread), and board tasks nobody has yet */
export function TaskCards({ view, agentName, onOpen }: { view: ChannelView; agentName: (id: string) => string; onOpen: (nodeId: string) => void }): JSX.Element | null {
  const { t } = useTranslation()
  const lead = view.teams[0]?.leadId
  const tags = handles(view.channel.nodes, agentName)
  const members = view.channel.nodes.filter((n) => n.id !== lead && view.channel.tasks?.[n.id])
  const board = (view.channel.board ?? []).filter((b) => b.status !== 'claimed')
  if (!members.length && !board.length) return null
  return (
    <div className="cw-tasks">
      {members.map((n) => {
        const k = view.channel.tasks![n.id]
        return (
          <div key={n.id} className="cw-card cw-task">
            <div className="cw-row">
              <AgentMark agent={markOf(n.agent)} size={16} />
              <b>@{tags.get(n.id)}</b>
              <span className={`cw-badge st-${k.status}`}>{t(`cw.task_${k.status}`)}</span>
              {n.worktree && <span className="cw-chip cw-mono">⑂ {n.worktree.branch}</span>}
              <div className="cw-grow" />
              <button className="cw-link" onClick={() => onOpen(n.id)}>{t('cw.openTask')}</button>
            </div>
            <div className="cw-task-title">{k.title}</div>
            {k.summary && <div className="cw-muted cw-pre-line">{k.summary}</div>}
            {k.memory && <div className="cw-memory"><b>{t('cw.memorySuggestion')}</b> <span className="cw-pre-line">{k.memory}</span></div>}
          </div>
        )
      })}
      {board.map((b) => (
        <div key={b.id} className="cw-card cw-task">
          <div className="cw-row">
            <b>{t('cw.board')}</b>
            <span className={`cw-badge ${b.status === 'returned' ? 'warn' : ''}`}>{t(`cw.board_${b.status}`)}</span>
          </div>
          <div className="cw-task-title">{b.title}</div>
          {b.doneWhen && <div className="cw-muted">{t('cw.doneWhen')}: {b.doneWhen}</div>}
        </div>
      ))}
    </div>
  )
}
