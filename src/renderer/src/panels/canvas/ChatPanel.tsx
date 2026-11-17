// 右欄「Agent Collaboration & Control」（cowork-demo.jpg）：上面是 Take Over，下面是群組的對話串與輸入框。
// 頻道就是群組：你在輸入框說的話會打進目標 Agent 的終端並送出；Agent 之間的訊息走收件匣（cowork-team-implement.md）。
// 對話串裡 Agent 寫的字是不可信文字，一律當純文字顯示。
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import type { ChannelView, RelayMode, TeamMessage } from '../../../../shared/canvas'
import type { CanvasCall } from './CanvasView'

type MarkId = Parameters<typeof AgentMark>[0]['agent']

interface Props {
  view: ChannelView
  call: CanvasCall
  /** The node the user selected on the canvas or in the cards, if any */
  selectedNode: string | null
  nodeName: (id: string) => string
  markOfNode: (id: string) => MarkId
  onPickNode: (id: string) => void
  onTakeOver: (id: string) => void
}

/** What the lead's task mail adds for the member; the chat shows the task itself */
const TASK_FOOTER = '\n\nWork in your current directory'

function TakeOver({ view, nodeId, nodeName, markOfNode, onTakeOver }: { view: ChannelView; nodeId: string | null; nodeName: Props['nodeName']; markOfNode: Props['markOfNode']; onTakeOver: Props['onTakeOver'] }): JSX.Element {
  const { t } = useTranslation()
  const rt = nodeId ? view.runtime[nodeId] : undefined
  const running = rt?.status === 'running'
  const [screen, setScreen] = useState('')
  useEffect(() => {
    setScreen('')
    if (!nodeId || !running) return
    let alive = true
    const read = (): void => {
      void window.api.canvas.call('node.screen', { channelId: view.channel.id, nodeId }).then((r) => alive && r.ok && setScreen(r.data || ''))
    }
    read()
    // A glance at what the agent is doing, not a second terminal: twice a second would be wasted work
    const timer = window.setInterval(read, 1500)
    return () => {
      alive = false
      window.clearInterval(timer)
    }
  }, [nodeId, running, view.channel.id])
  if (!nodeId) {
    return (
      <div className="cv-take">
        <div className="cv-take-title">{t('canvas.takeOverPanel')}</div>
        <p className="cv-muted">{t('canvas.takeOverEmpty')}</p>
      </div>
    )
  }
  const state = !rt || rt.status !== 'running' ? 'off' : rt.approval ? 'waiting' : rt.busy ? 'running' : 'idle'
  const lines = screen.split('\n').filter((l) => l.trim()).slice(-7)
  return (
    <div className="cv-take">
      <div className="cv-take-title">
        <AgentMark agent={markOfNode(nodeId)} size={14} />
        <span className="cv-take-name">{nodeName(nodeId)}</span>
        {rt?.role && <span className="cv-pill">{t(`canvas.role_${rt.role}`)}</span>}
      </div>
      <div className={`cv-take-screen st-${state}`}>
        <pre>{lines.length ? lines.join('\n') : t(`canvas.take_${state}`)}</pre>
        {state === 'running' && (
          <span className="cv-take-badge">
            <span className="cv-dot st-running" /> {t('canvas.agentOperating')}
          </span>
        )}
      </div>
      {rt?.task && <div className="cv-take-task">{rt.task.title}</div>}
      <p className="cv-muted">{t('canvas.takeOverHint')}</p>
      <button className="cv-take-btn" disabled={!running} onClick={() => onTakeOver(nodeId)}>
        {t('canvas.takeOver')}
      </button>
    </div>
  )
}

export default function ChatPanel({ view, call, selectedNode, nodeName, markOfNode, onPickNode, onTakeOver }: Props): JSX.Element {
  const { t } = useTranslation()
  const group = view.teams[0]
  const ch = view.channel.id
  const [text, setText] = useState('')
  const [to, setTo] = useState('')
  const [open, setOpen] = useState<Record<string, boolean>>({})
  const end = useRef<HTMLDivElement>(null)
  const who = (id: string): string => (id === 'user' ? t('canvas.you') : id === 'app' ? 'Agent Workbench' : nodeName(id))
  const target = to && group?.memberIds.includes(to) ? to : selectedNode && group?.memberIds.includes(selectedNode) ? selectedNode : group?.leadId || ''
  const focusNode = selectedNode && view.runtime[selectedNode] ? selectedNode : group?.leadId || null

  // Everything that happened in the group, oldest first: messages, plus what waits on the user right now
  const items = useMemo(() => {
    const list: { at: number; key: string; el: 'msg' | 'approval' | 'merge'; id: string }[] = []
    for (const m of view.messages) list.push({ at: m.at, key: `m:${m.id}`, el: 'msg', id: m.id })
    for (const [node, r] of Object.entries(view.runtime)) if (r.approval) list.push({ at: r.approval.createdAt, key: `a:${r.approval.requestId}`, el: 'approval', id: node })
    for (const r of view.mergeRequests) list.push({ at: r.at, key: `g:${r.id}`, el: 'merge', id: r.id })
    return list.sort((a, b) => a.at - b.at)
  }, [view])
  const byId = useMemo(() => new Map(view.messages.map((m) => [m.id, m])), [view.messages])
  useEffect(() => {
    end.current?.scrollIntoView({ block: 'end' })
  }, [items.length])

  const send = async (): Promise<void> => {
    const body = text.trim()
    if (!body || !target) return
    const r = await call('chat.send', { channelId: ch, to: target, text: body })
    if (r !== undefined) setText('')
  }

  const bubble = (m: TeamMessage): JSX.Element => {
    const mine = m.from === 'user'
    const system = m.from === 'app'
    const body = m.kind === 'task' ? m.text.split(TASK_FOOTER)[0] : m.text
    const long = body.length > 420
    const shown = long && !open[m.id] ? body.slice(0, 420) + '…' : body
    return (
      <div key={m.id} className={`cv-chat-msg ${mine ? 'mine' : ''} ${system ? 'system' : ''} st-${m.state}`}>
        {!mine && !system && (
          <button className="cv-chat-avatar" title={who(m.from)} onClick={() => onPickNode(m.from)}>
            <AgentMark agent={markOfNode(m.from)} size={18} />
          </button>
        )}
        <div className="cv-chat-bubble">
          <div className="cv-chat-meta">
            <b>{who(m.from)}</b>
            {m.to !== 'user' && <span> → {who(m.to)}</span>}
            <span className="cv-chat-kind">{t(`canvas.kind_${m.kind}`)}</span>
            <span className="cv-chat-time">{new Date(m.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
          </div>
          <div className="cv-chat-text">{shown}</div>
          {long && (
            <button className="cv-link" onClick={() => setOpen((o) => ({ ...o, [m.id]: !o[m.id] }))}>
              {open[m.id] ? t('canvas.showLess') : t('canvas.showAll')}
            </button>
          )}
          {m.state === 'pending-approval' && (
            <div className="cv-chat-actions">
              <span className="cv-chat-wait">{t('canvas.heldForYou')}</span>
              <button className="cv-btn primary" disabled={view.stop.requested} onClick={() => void call('relay.approve', { channelId: ch, messageId: m.id })}>
                {t('canvas.teamApprove')}
              </button>
              <button className="cv-btn" onClick={() => void call('relay.reject', { channelId: ch, messageId: m.id })}>
                {t('canvas.teamReject')}
              </button>
            </div>
          )}
          {m.state === 'rejected' && <div className="cv-chat-wait">{t('canvas.rejectedNote')}</div>}
          {(m.state === 'queued' || (m.state === 'notified' && !mine)) && <div className="cv-chat-wait">{t(`canvas.state_${m.state}`)}</div>}
        </div>
      </div>
    )
  }

  return (
    <aside className="cv-right cv-collab">
      <TakeOver view={view} nodeId={focusNode} nodeName={nodeName} markOfNode={markOfNode} onTakeOver={onTakeOver} />

      {group && (
        <div className="cv-chat-bar">
          <label title={t(`canvas.relayHint_${group.relay.replace('-', '_')}`)}>
            <span className="cv-muted">{t('canvas.teamRelay')}</span>
            <select value={group.relay} onChange={(e) => void call('team.update', { channelId: ch, relay: e.target.value as RelayMode })}>
              {(['approve', 'lead-auto', 'auto'] as const).map((r) => (
                <option key={r} value={r}>{t(`canvas.relay_${r.replace('-', '_')}`)}</option>
              ))}
            </select>
          </label>
          {group.status === 'paused' && (
            <span className="cv-chat-paused" title={group.pausedReason}>
              {t('canvas.teamStatus_paused')}
              <button className="cv-link" disabled={view.stop.requested} onClick={() => void call('team.resume', { channelId: ch })}>
                {t('canvas.teamResume')}
              </button>
            </span>
          )}
        </div>
      )}

      <div className="cv-chat">
        {items.length === 0 && <p className="cv-muted cv-chat-empty">{group ? t('canvas.chatEmpty') : t('canvas.chatNoAgents')}</p>}
        {items.map((it) => {
          if (it.el === 'msg') {
            const m = byId.get(it.id)
            return m ? bubble(m) : null
          }
          if (it.el === 'approval') {
            const r = view.runtime[it.id]
            const a = r?.approval
            if (!a) return null
            return (
              <div key={it.key} className="cv-chat-msg st-pending-approval">
                <button className="cv-chat-avatar" onClick={() => onPickNode(it.id)}>
                  <AgentMark agent={markOfNode(it.id)} size={18} />
                </button>
                <div className="cv-chat-bubble">
                  <div className="cv-chat-meta">
                    <b>{who(it.id)}</b>
                    <span className="cv-chat-kind">{t('canvas.kind_approval')}</span>
                  </div>
                  <div className="cv-chat-text">{a.title}</div>
                  <div className="cv-chat-actions">
                    {a.canApprove && !view.stop.requested && (
                      <button className="cv-btn primary" onClick={() => void call('node.approve', { channelId: ch, nodeId: it.id, requestId: a.requestId, sessionGeneration: a.sessionGeneration })}>
                        {t('canvas.approve')}
                      </button>
                    )}
                    {a.canReject && (
                      <button className="cv-btn" onClick={() => void call('node.reject', { channelId: ch, nodeId: it.id, requestId: a.requestId, sessionGeneration: a.sessionGeneration })}>
                        {t('canvas.reject')}
                      </button>
                    )}
                    {!a.canApprove && !a.canReject && (
                      <button className="cv-btn" onClick={() => onTakeOver(it.id)}>{t('canvas.answerInTerminal')}</button>
                    )}
                  </div>
                </div>
              </div>
            )
          }
          const r = view.mergeRequests.find((x) => x.id === it.id)
          if (!r) return null
          return (
            <div key={it.key} className="cv-chat-msg st-pending-approval">
              <button className="cv-chat-avatar" onClick={() => onPickNode(r.lead)}>
                <AgentMark agent={markOfNode(r.lead)} size={18} />
              </button>
              <div className="cv-chat-bubble">
                <div className="cv-chat-meta">
                  <b>{who(r.lead)}</b>
                  <span className="cv-chat-kind">{t('canvas.kind_merge')}</span>
                </div>
                <div className="cv-chat-text">{t('canvas.mergeRequestText', { lead: who(r.lead), member: who(r.member) })}</div>
                {r.commits.length > 0 && <pre className="cv-msg">{r.commits.join('\n')}</pre>}
                <div className="cv-chat-actions">
                  <button className="cv-btn primary" onClick={() => void call('teammerge.approve', { channelId: ch, requestId: r.id })}>{t('canvas.teamApprove')}</button>
                  <button className="cv-btn" onClick={() => void call('teammerge.reject', { channelId: ch, requestId: r.id })}>{t('canvas.teamReject')}</button>
                </div>
              </div>
            </div>
          )
        })}
        <div ref={end} />
      </div>

      <div className="cv-chat-input">
        <select className="cv-chat-to" value={target} disabled={!group} onChange={(e) => setTo(e.target.value)} title={t('canvas.chatTo')}>
          {(group?.memberIds || []).map((id) => (
            <option key={id} value={id}>
              @{nodeName(id)}
              {id === group?.leadId ? ' ★' : ''}
            </option>
          ))}
        </select>
        <textarea
          rows={2}
          value={text}
          disabled={!group || view.stop.requested}
          placeholder={group ? t('canvas.chatPlaceholder') : t('canvas.chatNoAgents')}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Enter sends; Shift+Enter is a new line; an IME still composing keeps its Enter
            if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault()
              void send()
            }
          }}
        />
        <button className="cv-chat-send" disabled={!text.trim() || !target || view.stop.requested} onClick={() => void send()} title={t('canvas.chatSend')}>
          ➤
        </button>
      </div>
    </aside>
  )
}
