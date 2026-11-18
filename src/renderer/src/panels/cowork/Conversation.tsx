// 一段對話（cowork-agui-implement.md §8.2、§8.3）。對話模式：使用者訊息靠右、Bot 的動作收成一行「▸」、回答是純文字、
// 審批卡在對話裡。終端模式：中間就是 CLI 自己的畫面。對話裡可以有好幾個 Bot：隊長的畫面是群組（組員的回答與審批也在），
// 點組員看它自己的那段。Agent 寫的字一律當純文字（Markdown 不開 HTML）。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import type { CliLauncher } from '../../../../preload/index'
import type { ChannelView, NodeRuntime } from '../../../../shared/canvas'
import { toolSummary, type AguiUpdate, type TranscriptEntry } from '../../../../shared/agui'
import Guests from './Guests'
import TerminalPane from './TerminalPane'
import { handles, MembersBar, NeedsYou, TaskCards } from './Team'
import { markOf, type CanvasCall } from './types'

type Tool = Extract<TranscriptEntry, { kind: 'tool' }>
type ApprovalEntry = Extract<TranscriptEntry, { kind: 'approval' }>
type Shown = { node: string; e: TranscriptEntry }

const PASS = 'PASS'

/** The group's lead: the conversation's own Bot */
export const leadOf = (view: ChannelView): string => view.teams[0]?.leadId ?? view.channel.nodes[0]?.id ?? ''

/** Every chat node's entries in this channel: loaded once, then kept current from `canvas:agui` */
export function useTranscripts(call: CanvasCall, channelId: string, enabled: boolean): Record<string, TranscriptEntry[]> {
  const [all, setAll] = useState<Record<string, TranscriptEntry[]>>({})
  const load = useCallback(async () => {
    const t = (await call('chat.transcript', { channelId })) as unknown as Record<string, { entries: TranscriptEntry[] }> | undefined
    if (t) setAll(Object.fromEntries(Object.entries(t).map(([k, v]) => [k, v.entries])))
  }, [call, channelId])
  useEffect(() => {
    setAll({})
    if (!enabled) return
    void load()
    return window.api.canvas.onAgui((u: AguiUpdate) => {
      if (u.channelId !== channelId) return
      const e = u.entry
      if (e) {
        setAll((m) => {
          const list = m[u.nodeId] ?? []
          const i = list.findIndex((x) => x.id === e.id && x.kind === e.kind)
          const next = i < 0 ? [...list, e] : list.map((x, j) => (j === i ? e : x))
          return { ...m, [u.nodeId]: next }
        })
      }
      // A run's end settles whatever it left running; an approval appears or is answered: read the whole record again
      if (u.approval || u.settled || u.event?.type === 'RUN_FINISHED' || u.event?.type === 'RUN_ERROR') void load()
    })
  }, [channelId, enabled, load])
  return all
}

/** The lead's view is the group: all of the lead, plus members' answers and approvals, and each user message once */
function groupThread(all: Record<string, TranscriptEntry[]>, lead: string): Shown[] {
  const out: Shown[] = []
  const users = new Set<string>()
  for (const [node, entries] of Object.entries(all)) {
    for (const e of entries) {
      if (e.kind === 'text' && e.text.trim() === PASS) continue
      if (e.kind === 'user' && e.from === 'user') {
        if (users.has(e.id)) continue
        users.add(e.id)
      } else if (node !== lead && !['text', 'approval', 'runError'].includes(e.kind)) continue
      out.push({ node, e })
    }
  }
  return out.sort((a, b) => a.e.at - b.e.at)
}

function ToolLine({ e }: { e: Tool }): JSX.Element {
  const [open, setOpen] = useState(false)
  return (
    <div className="cw-tool">
      <button className="cw-act" onClick={() => setOpen(!open)} aria-expanded={open}>
        <span aria-hidden="true">{open ? '▾' : '▸'}</span>
        <span className="cw-act-name">{e.name}</span>
        <span className="cw-act-sum">{toolSummary(e)}</span>
        {e.status === 'running' && <span className="cw-spin" aria-label="running" />}
      </button>
      {open && (
        <div className="cw-tool-body">
          {e.args && <pre className="cw-pre">{e.args}</pre>}
          {e.output && <pre className="cw-pre out">{e.output}</pre>}
        </div>
      )}
    </div>
  )
}

function ApprovalCard({ e, onAnswer, disabled }: { e: ApprovalEntry; onAnswer: (allow: boolean, forSession?: boolean) => void; disabled: boolean }): JSX.Element {
  const { t } = useTranslation()
  if (e.status !== 'pending') {
    return (
      <div className={`cw-settled st-${e.status}`}>
        {t(`cw.approval_${e.status}`)}{e.by ? ` (${t('cw.byGuest', { name: e.by })})` : ''} · <span className="cw-mono">{e.summary}</span>
      </div>
    )
  }
  return (
    <div className="cw-card cw-approval" role="group" aria-label={t('cw.waitingApproval', { tool: e.tool })}>
      <b>{t('cw.waitingApproval', { tool: e.tool })}</b>
      <div className="cw-mono cw-approval-sum">{e.summary}</div>
      <div className="cw-row">
        <button className="cw-btn dark" disabled={disabled} onClick={() => onAnswer(true)}>{t('cw.allowOnce')}</button>
        {e.sessionScope && <button className="cw-btn" disabled={disabled} onClick={() => onAnswer(true, true)}>{t('cw.allowSession')}</button>}
        <button className="cw-btn" disabled={disabled} onClick={() => onAnswer(false)}>{t('cw.deny')}</button>
      </div>
    </div>
  )
}

function Entry({ e, who, from, answer, busy }: { e: TranscriptEntry; who?: string; from: (id: string) => string; answer: (id: string, allow: boolean, forSession?: boolean) => void; busy: boolean }): JSX.Element | null {
  const { t } = useTranslation()
  const body = ((): JSX.Element | null => {
    switch (e.kind) {
      case 'user':
        return (
          <div className="cw-user">
            {e.from !== 'user' && <div className="cw-from">{e.from.startsWith('guest:') ? from(e.from) : t('cw.fromAgent', { id: from(e.from) })}</div>}
            {e.text}
          </div>
        )
      case 'text':
        return (
          <div className="cw-text">
            <ReactMarkdown skipHtml>{e.text}</ReactMarkdown>
          </div>
        )
      case 'tool':
        return <ToolLine e={e} />
      case 'approval':
        return <ApprovalCard e={e} disabled={busy} onAnswer={(allow, forSession) => answer(e.id, allow, forSession)} />
      case 'runError':
        return <div className="cw-runerror">{e.interrupted ? t('cw.interrupted') : `${t('cw.failed')}: ${e.message}`}</div>
      case 'note':
        return <div className="cw-note">{t(`cw.note_${e.text.replace('-', '_')}`)}</div>
    }
  })()
  if (!who || !body) return body
  return (
    <div className="cw-by">
      <div className="cw-from">{who}</div>
      {body}
    </div>
  )
}

/** A terminal node's hook or screen approval (Cowork 2.0 path) */
function TerminalApproval({ view, nodeId, rt, call, who, onOpen }: { view: ChannelView; nodeId: string; rt: NodeRuntime; call: CanvasCall; who?: string; onOpen?: () => void }): JSX.Element | null {
  const { t } = useTranslation()
  const a = rt.approval
  if (!a) return null
  const ids = { channelId: view.channel.id, nodeId, requestId: a.requestId, sessionGeneration: a.sessionGeneration }
  const what = a.hook?.command || a.hook?.files.map((f) => f.path).join(', ') || a.title
  return (
    <div className="cw-card cw-approval">
      <b>{who ? `${who} · ` : ''}{t('cw.waitingApproval', { tool: a.hook?.tool || 'CLI' })}</b>
      <div className="cw-mono cw-approval-sum">{what}</div>
      <div className="cw-row">
        {a.canApprove && <button className="cw-btn dark" onClick={() => void call('node.approve', ids)}>{t('cw.allowOnce')}</button>}
        {a.canReject && <button className="cw-btn" onClick={() => void call('node.reject', ids)}>{t('cw.deny')}</button>}
        {!a.canApprove && <span className="cw-muted">{t('cw.answerInTerminal')}</span>}
        {!a.canApprove && onOpen && <button className="cw-btn cw-open-term" onClick={onOpen}>{t('cw.openTerminal')}</button>}
      </div>
    </div>
  )
}

export default function Conversation({
  view,
  call,
  agentName,
  launchers,
  canChat,
  focusId,
  onFocus: setFocusId,
  panelOpen,
  onTogglePanel
}: {
  view: ChannelView
  call: CanvasCall
  agentName: (id: string) => string
  launchers: CliLauncher[]
  canChat: (agent: string) => boolean
  /** The Bot shown; null = the lead (the group) */
  focusId: string | null
  onFocus: (nodeId: string | null) => void
  panelOpen: boolean
  onTogglePanel: () => void
}): JSX.Element {
  const { t } = useTranslation()
  const channelId = view.channel.id
  const nodes = view.channel.nodes
  const leadId = leadOf(view)
  const node = nodes.find((n) => n.id === focusId) ?? nodes.find((n) => n.id === leadId)
  const atLead = node?.id === leadId
  const rt = node ? view.runtime[node.id] : undefined
  const isChat = node?.mode === 'chat'
  const anyChat = nodes.some((n) => n.mode === 'chat')
  const all = useTranscripts(call, channelId, anyChat)
  const tags = useMemo(() => handles(nodes, agentName), [nodes, agentName])
  const label = (id: string): string => (id === 'app' ? 'Agent Workbench' : id.startsWith('guest:') ? t('cw.guestName', { name: id.slice(6) }) : `@${tags.get(id) ?? id}`)
  const [inviting, setInviting] = useState(false)
  const entries: Shown[] = useMemo(
    () => (!node ? [] : atLead && nodes.length > 1 ? groupThread(all, leadId) : (all[node.id] ?? []).map((e) => ({ node: node.id, e }))),
    [all, node, atLead, nodes.length, leadId]
  )
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  /** Answered here; shown as settled until the record catches up, so a card is never answered twice */
  const [answered, setAnswered] = useState<Record<string, 'allowed' | 'denied'>>({})
  const end = useRef<HTMLDivElement>(null)
  const running = rt?.status === 'running'
  /** A chat conversation whose TUI the user took over: the CLI runs in a PTY on the same session */
  const taken = isChat && running && !!rt?.ptyId
  const inRun = !!rt?.inRun
  const stopped = view.stop.requested

  // Braces matter: WebView2's scrollIntoView returns a promise, and an effect must not return one
  useEffect(() => {
    const r = end.current?.scrollIntoView({ block: 'end' }) as unknown as Promise<void> | undefined
    // A newer scroll cancels the last one; that rejection is not an error
    r?.catch?.(() => {})
  }, [entries.length, inRun])

  const ids = useMemo(() => ({ channelId, nodeId: node?.id ?? '' }), [channelId, node?.id])
  const act = async (fn: () => Promise<unknown>): Promise<void> => {
    setBusy(true)
    await fn()
    setBusy(false)
  }
  /** `@name` at the start picks the Bot; otherwise the lead's view goes to the lead (or everyone, in free talk) */
  const mention = /^@([\w-]*)$/.exec(text)
  const send = (): void => {
    let body = text.trim()
    let to: string | undefined = atLead ? undefined : node?.id
    const m = /^@([\w-]+)\s+/.exec(body)
    const hit = m && [...tags].find(([, h]) => h === m[1].toLowerCase())
    if (m && hit) {
      to = hit[0]
      body = body.slice(m[0].length)
    }
    if (!body || stopped) return
    void act(async () => {
      if ((await call('chat.send', { channelId, to, text: body })) !== undefined) setText('')
    })
  }
  const answer = (nodeId: string, approvalId: string, allow: boolean, forSession?: boolean): void => {
    setAnswered((a) => ({ ...a, [approvalId]: allow ? 'allowed' : 'denied' }))
    void act(() => call('chat.answer', { channelId, nodeId, approvalId, allow, forSession }))
  }

  if (!node) return <main className="cw-main" />
  const status = rt?.status ?? 'exited'
  const others = nodes.filter((n) => n.id !== node.id && view.runtime[n.id]?.approval)

  return (
    <main className="cw-main">
      <header className="cw-head">
        {!atLead && <button className="cw-link" onClick={() => setFocusId(null)}>← {t('cw.backToGroup')}</button>}
        <AgentMark agent={markOf(node.agent)} size={18} />
        <b>{agentName(node.agent)}</b>
        {node.role && <span className="cw-muted">{node.role}</span>}
        <span className="cw-chip cw-mono" title={node.worktree?.path}>{node.worktree ? `⑂ ${node.worktree.branch}` : t('canvas.repoRoot')}</span>
        <span className={`cw-dot st-${inRun ? 'busy' : status}`} />
        <span className="cw-muted">{inRun ? t('cw.working') : t(`canvas.status_${status.replace('-', '_')}`)}</span>
        <div className="cw-grow" />
        {isChat && running && !taken && node.session && (
          <button className="cw-btn" disabled={busy || inRun} title={inRun ? t('cw.takeControlBusy') : ''} onClick={() => void act(() => call('node.takeControl', ids))}>
            {t('cw.takeControl')}
          </button>
        )}
        {running ? (
          <button className="cw-icon" title={t('canvas.close')} onClick={() => window.confirm(t('canvas.closeConfirm')) && void act(() => call('node.close', ids))}>✕</button>
        ) : (
          status !== 'creating' && status !== 'closing' && <button className="cw-icon" disabled={busy || stopped} title={t('canvas.restart')} onClick={() => void act(() => call('node.restart', ids))}>↻</button>
        )}
        <button className="cw-btn" aria-expanded={inviting} onClick={() => setInviting(!inviting)}>{t('cw.invite')}</button>
        <button className={`cw-icon ${panelOpen ? 'on' : ''}`} title={t('cw.workspace')} aria-pressed={panelOpen} onClick={onTogglePanel}>▭</button>
      </header>
      {inviting && <div className="cw-invite-wrap"><Guests call={call} channelId={channelId} onClose={() => setInviting(false)} /></div>}
      <MembersBar view={view} call={call} agentName={agentName} launchers={launchers} canChat={canChat} focus={node.id} onFocus={setFocusId} />

      {taken && rt?.ptyId ? (
        <div className="cw-takeover">
          <TerminalPane ptyId={rt.ptyId} />
          <aside className="cw-card cw-takeover-side">
            <b>{agentName(node.agent)}</b>
            <p className="cw-muted">{t('cw.youHaveControl')}</p>
            <button className="cw-btn dark" disabled={busy} onClick={() => void act(() => call('node.handBack', ids))}>{t('cw.handBack')}</button>
          </aside>
        </div>
      ) : !isChat ? (
        <div className="cw-termwrap">
          <NeedsYou view={view} call={call} agentName={agentName} />
          {others.map((n) => <TerminalApproval key={n.id} view={view} nodeId={n.id} rt={view.runtime[n.id]} call={call} who={label(n.id)} onOpen={() => setFocusId(n.id)} />)}
          {rt && <TerminalApproval view={view} nodeId={node.id} rt={rt} call={call} />}
          {running && rt?.ptyId ? <TerminalPane ptyId={rt.ptyId} /> : <div className="cw-ended">{rt?.error ? <span className="cw-error">{rt.error}</span> : t(`canvas.status_${status.replace('-', '_')}`)}</div>}
        </div>
      ) : (
        <>
          <div className="cw-scroll">
            <div className="cw-thread">
              {entries.map(({ node: n, e }) => (
                <Entry
                  key={`${n}:${e.kind}:${e.id}`}
                  e={e.kind === 'approval' && e.status === 'pending' && answered[e.id] ? { ...e, status: answered[e.id] } : e}
                  who={n !== node.id ? label(n) : undefined}
                  from={label}
                  answer={(id, allow, forSession) => answer(n, id, allow, forSession)}
                  busy={busy}
                />
              ))}
              {atLead && <TaskCards view={view} agentName={agentName} onOpen={setFocusId} />}
              {others.map((n) => <TerminalApproval key={n.id} view={view} nodeId={n.id} rt={view.runtime[n.id]} call={call} who={label(n.id)} onOpen={() => setFocusId(n.id)} />)}
              <NeedsYou view={view} call={call} agentName={agentName} />
              {inRun && <div className="cw-working"><span className="cw-spin" />{t('cw.working')}</div>}
              {!running && status !== 'creating' && <div className="cw-ended">{rt?.error ? <span className="cw-error">{rt.error}</span> : t('cw.ended')}</div>}
              <div ref={end} />
            </div>
          </div>
          <div className="cw-composer-wrap">
            <div className="cw-composer">
              {mention && (
                <div className="cw-row">
                  {[...tags].filter(([, h]) => h.startsWith(mention[1].toLowerCase())).map(([id, h]) => (
                    <button key={id} className="cw-chip cw-pick" onClick={() => setText(`@${h} `)}>@{h}</button>
                  ))}
                </div>
              )}
              <textarea
                rows={2}
                value={text}
                placeholder={atLead && nodes.length > 1 ? t('cw.askGroup') : t('cw.askAnything')}
                aria-label={t('cw.askAnything')}
                disabled={!running}
                onChange={(e) => setText(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                    e.preventDefault()
                    send()
                  }
                }}
              />
              <div className="cw-row">
                <span className="cw-muted">{atLead && view.teams[0]?.freeTalk && nodes.length > 1 ? t('cw.toEveryone') : label(node.id)}</span>
                <div className="cw-grow" />
                {inRun && <button className="cw-send" aria-label={t('cw.stop')} title={t('cw.stop')} onClick={() => void call('chat.interrupt', ids)}>■</button>}
                <button className="cw-send" aria-label={t('cw.send')} title={inRun ? t('cw.sendQueued') : t('cw.send')} disabled={!running || !text.trim() || busy || stopped} onClick={send}>↑</button>
              </div>
            </div>
          </div>
        </>
      )}
    </main>
  )
}
