// 一段對話（cowork-agui-implement.md §8.2）。對話模式：使用者訊息靠右、Bot 的動作收成一行「▸」、回答是純文字、
// 審批卡在對話裡。終端模式：中間就是 CLI 自己的畫面。Agent 寫的字一律當純文字（Markdown 不開 HTML）。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import type { ChannelView, NodeRuntime } from '../../../../shared/canvas'
import { toolSummary, type AguiUpdate, type TranscriptEntry } from '../../../../shared/agui'
import TerminalPane from './TerminalPane'
import { markOf, type CanvasCall } from './types'

type Tool = Extract<TranscriptEntry, { kind: 'tool' }>
type ApprovalEntry = Extract<TranscriptEntry, { kind: 'approval' }>

/** The conversation's entries: loaded once, then kept current from `canvas:agui` */
export function useTranscript(call: CanvasCall, channelId: string, nodeId: string, isChat: boolean): TranscriptEntry[] {
  const [entries, setEntries] = useState<TranscriptEntry[]>([])
  const load = useCallback(async () => {
    const t = await call('chat.transcript', { channelId, nodeId })
    if (t) setEntries(t.entries)
  }, [call, channelId, nodeId])
  useEffect(() => {
    setEntries([])
    if (!isChat) return
    void load()
    return window.api.canvas.onAgui((u: AguiUpdate) => {
      if (u.channelId !== channelId || u.nodeId !== nodeId) return
      const e = u.entry
      if (e) {
        setEntries((list) => {
          const i = list.findIndex((x) => x.id === e.id && x.kind === e.kind)
          if (i < 0) return [...list, e]
          const next = list.slice()
          next[i] = e
          return next
        })
      }
      // A run's end settles whatever it left running; an approval appears or is answered: read the whole record again
      if (u.approval || u.settled || u.event?.type === 'RUN_FINISHED' || u.event?.type === 'RUN_ERROR') void load()
    })
  }, [channelId, nodeId, isChat, load])
  return entries
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
        {t(`cw.approval_${e.status}`)} · <span className="cw-mono">{e.summary}</span>
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

function Entry({ e, answer, busy }: { e: TranscriptEntry; answer: (id: string, allow: boolean, forSession?: boolean) => void; busy: boolean }): JSX.Element | null {
  const { t } = useTranslation()
  switch (e.kind) {
    case 'user':
      return (
        <div className="cw-user">
          {e.from !== 'user' && <div className="cw-from">{t('cw.fromAgent', { id: e.from })}</div>}
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
}

/** A terminal node's hook or screen approval (Cowork 2.0 path), shown above its terminal */
function TerminalApproval({ view, nodeId, rt, call }: { view: ChannelView; nodeId: string; rt: NodeRuntime; call: CanvasCall }): JSX.Element | null {
  const { t } = useTranslation()
  const a = rt.approval
  if (!a) return null
  const ids = { channelId: view.channel.id, nodeId, requestId: a.requestId, sessionGeneration: a.sessionGeneration }
  const what = a.hook?.command || a.hook?.files.map((f) => f.path).join(', ') || a.title
  return (
    <div className="cw-card cw-approval">
      <b>{t('cw.waitingApproval', { tool: a.hook?.tool || 'CLI' })}</b>
      <div className="cw-mono cw-approval-sum">{what}</div>
      <div className="cw-row">
        {a.canApprove && <button className="cw-btn dark" onClick={() => void call('node.approve', ids)}>{t('cw.allowOnce')}</button>}
        {a.canReject && <button className="cw-btn" onClick={() => void call('node.reject', ids)}>{t('cw.deny')}</button>}
        {!a.canApprove && <span className="cw-muted">{t('cw.answerInTerminal')}</span>}
      </div>
    </div>
  )
}

export default function Conversation({
  view,
  call,
  agentName,
  panelOpen,
  onTogglePanel
}: {
  view: ChannelView
  call: CanvasCall
  agentName: (id: string) => string
  panelOpen: boolean
  onTogglePanel: () => void
}): JSX.Element {
  const { t } = useTranslation()
  const lead = view.channel.nodes[0]
  const rt = lead ? view.runtime[lead.id] : undefined
  const isChat = lead?.mode === 'chat'
  const channelId = view.channel.id
  const entries = useTranscript(call, channelId, lead?.id ?? '', isChat && !!lead)
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

  const ids = useMemo(() => ({ channelId, nodeId: lead?.id ?? '' }), [channelId, lead?.id])
  const act = async (fn: () => Promise<unknown>): Promise<void> => {
    setBusy(true)
    await fn()
    setBusy(false)
  }
  const send = (): void => {
    const v = text.trim()
    if (!v || !running || inRun) return
    void act(async () => {
      if ((await call('chat.prompt', { ...ids, text: v })) !== undefined) setText('')
    })
  }
  const answer = (approvalId: string, allow: boolean, forSession?: boolean): void => {
    setAnswered((a) => ({ ...a, [approvalId]: allow ? 'allowed' : 'denied' }))
    void act(() => call('chat.answer', { ...ids, approvalId, allow, forSession }))
  }

  if (!lead) return <main className="cw-main" />
  const status = rt?.status ?? 'exited'

  return (
    <main className="cw-main">
      <header className="cw-head">
        <AgentMark agent={markOf(lead.agent)} size={18} />
        <b>{agentName(lead.agent)}</b>
        <span className="cw-chip cw-mono" title={lead.worktree?.path}>{lead.worktree ? `⑂ ${lead.worktree.branch}` : t('canvas.repoRoot')}</span>
        <span className={`cw-dot st-${inRun ? 'busy' : status}`} />
        <span className="cw-muted">{inRun ? t('cw.working') : t(`canvas.status_${status.replace('-', '_')}`)}</span>
        <div className="cw-grow" />
        {isChat && running && !taken && lead.session && (
          <button className="cw-btn" disabled={busy || inRun} title={inRun ? t('cw.takeControlBusy') : ''} onClick={() => void act(() => call('node.takeControl', ids))}>
            {t('cw.takeControl')}
          </button>
        )}
        {running ? (
          <button className="cw-icon" title={t('canvas.close')} onClick={() => window.confirm(t('canvas.closeConfirm')) && void act(() => call('node.close', ids))}>✕</button>
        ) : (
          status !== 'creating' && status !== 'closing' && <button className="cw-icon" disabled={busy || stopped} title={t('canvas.restart')} onClick={() => void act(() => call('node.restart', ids))}>↻</button>
        )}
        <button className={`cw-icon ${panelOpen ? 'on' : ''}`} title={t('cw.workspace')} aria-pressed={panelOpen} onClick={onTogglePanel}>▭</button>
      </header>

      {taken && rt?.ptyId ? (
        <div className="cw-takeover">
          <TerminalPane ptyId={rt.ptyId} />
          <aside className="cw-card cw-takeover-side">
            <b>{agentName(lead.agent)}</b>
            <p className="cw-muted">{t('cw.youHaveControl')}</p>
            <button className="cw-btn dark" disabled={busy} onClick={() => void act(() => call('node.handBack', ids))}>{t('cw.handBack')}</button>
          </aside>
        </div>
      ) : !isChat ? (
        <div className="cw-termwrap">
          {rt && <TerminalApproval view={view} nodeId={lead.id} rt={rt} call={call} />}
          {running && rt?.ptyId ? <TerminalPane ptyId={rt.ptyId} /> : <div className="cw-ended">{rt?.error ? <span className="cw-error">{rt.error}</span> : t(`canvas.status_${status.replace('-', '_')}`)}</div>}
        </div>
      ) : (
        <>
          <div className="cw-scroll">
            <div className="cw-thread">
              {entries.map((e) => (
                <Entry key={`${e.kind}:${e.id}`} e={e.kind === 'approval' && e.status === 'pending' && answered[e.id] ? { ...e, status: answered[e.id] } : e} answer={answer} busy={busy} />
              ))}
              {inRun && <div className="cw-working"><span className="cw-spin" />{t('cw.working')}</div>}
              {!running && status !== 'creating' && <div className="cw-ended">{rt?.error ? <span className="cw-error">{rt.error}</span> : t('cw.ended')}</div>}
              <div ref={end} />
            </div>
          </div>
          <div className="cw-composer-wrap">
            <div className="cw-composer">
              <textarea
                rows={2}
                value={text}
                placeholder={t('cw.askAnything')}
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
                <span className="cw-muted">{agentName(lead.agent)}</span>
                <div className="cw-grow" />
                {inRun ? (
                  <button className="cw-send" aria-label={t('cw.stop')} title={t('cw.stop')} onClick={() => void call('chat.interrupt', ids)}>■</button>
                ) : (
                  <button className="cw-send" aria-label={t('cw.send')} title={t('cw.send')} disabled={!running || !text.trim() || busy} onClick={send}>↑</button>
                )}
              </div>
            </div>
          </div>
        </>
      )}
    </main>
  )
}
