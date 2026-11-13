// Cowork 團隊（cowork-team-implement.md §3）：畫布上的團隊框、Inspector 的團隊面板與轉送／合併卡、Feed 卡片。
// 狀態一律以後端推來的 ChannelView 為準；Agent 寫的訊息是不可信文字，只用純文字顯示。
import { useEffect, useState } from 'react'
import type { NodeProps, Node } from '@xyflow/react'
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import type { ChannelView, RelayMode, Team, TeamMessage } from '../../../../shared/canvas'
import type { CanvasCall } from './CanvasView'
import { markOf } from './CanvasView'

export type TeamFrameData = { team: Team; pending: number; onPick: (teamId: string) => void }

/** The frame behind a team's nodes; sized from the members by CanvasView, never dragged */
export function TeamFrame({ data, selected }: NodeProps<Node<TeamFrameData>>): JSX.Element {
  const { t } = useTranslation()
  const { team, pending } = data
  return (
    <div className={`cv-team st-${team.status}${selected ? ' selected' : ''}`}>
      <button className="cv-team-head" onClick={() => data.onPick(team.id)}>
        <span className="cv-team-name">{team.name}</span>
        <span className={`cv-pill st-${team.status}`}>{t(`canvas.teamStatus_${team.status}`)}</span>
        <span className="cv-muted">{t(`canvas.relay_${team.relay.replace('-', '_')}`)}</span>
        {team.relay !== 'approve' && <span className="cv-muted">{t('canvas.teamMessages', { n: team.messageCount, max: team.limits.maxMessages })}</span>}
        {pending > 0 && <span className="cv-badge">{pending}</span>}
      </button>
    </div>
  )
}

const nameOf = (view: ChannelView, nodeName: (id: string) => string, id: string): string => (id === 'app' ? 'Agent Workbench' : id === 'user' ? 'you' : nodeName(id))

export function RelayCards({ view, teamId, call, nodeName }: { view: ChannelView; teamId?: string; call: CanvasCall; nodeName: (id: string) => string }): JSX.Element | null {
  const { t } = useTranslation()
  const relays = view.relays.filter((m) => !teamId || m.teamId === teamId)
  const merges = view.mergeRequests.filter((m) => !teamId || m.team === teamId)
  if (relays.length === 0 && merges.length === 0) return null
  return (
    <section className="cv-sec">
      {relays.length > 0 && <h4 className="cv-h4">{t('canvas.relays')}</h4>}
      {relays.map((m) => (
        <div key={m.id} className="cv-relay">
          <div className="cv-muted">
            {t('canvas.relayFromTo', { from: nameOf(view, nodeName, m.from), to: nameOf(view, nodeName, m.to) })} · {m.kind}
          </div>
          <pre className="cv-msg">{m.text}</pre>
          <div className="cv-row">
            <button className="cv-btn primary" onClick={() => void call('relay.approve', { channelId: view.channel.id, messageId: m.id })}>{t('canvas.teamApprove')}</button>
            <button className="cv-btn" onClick={() => void call('relay.reject', { channelId: view.channel.id, messageId: m.id })}>{t('canvas.teamReject')}</button>
          </div>
        </div>
      ))}
      {merges.length > 0 && <h4 className="cv-h4">{t('canvas.mergeRequests')}</h4>}
      {merges.map((r) => (
        <div key={r.id} className="cv-relay">
          <div>{t('canvas.mergeRequestText', { lead: nodeName(r.lead), member: nodeName(r.member) })}</div>
          <pre className="cv-msg">{r.commits.join('\n') || '—'}</pre>
          <div className="cv-row">
            <button className="cv-btn primary" onClick={() => void call('teammerge.approve', { channelId: view.channel.id, requestId: r.id })}>{t('canvas.teamApprove')}</button>
            <button className="cv-btn" onClick={() => void call('teammerge.reject', { channelId: view.channel.id, requestId: r.id })}>{t('canvas.teamReject')}</button>
          </div>
        </div>
      ))}
    </section>
  )
}

function Messages({ view, list, nodeName }: { view: ChannelView; list: TeamMessage[]; nodeName: (id: string) => string }): JSX.Element {
  const { t } = useTranslation()
  if (list.length === 0) return <p className="cv-muted">{t('canvas.noMessages')}</p>
  return (
    <div className="cv-msgs">
      {list.slice(0, 30).map((m) => (
        <details key={m.id} className="cv-msg-row">
          <summary>
            <span className="cv-feed-time">{new Date(m.at).toLocaleTimeString()}</span> {nameOf(view, nodeName, m.from)} → {nameOf(view, nodeName, m.to)} · {m.kind} · {m.state}
          </summary>
          <pre className="cv-msg">{m.text}</pre>
        </details>
      ))}
    </div>
  )
}

export function TeamInspector({ view, teamId, call, nodeName }: { view: ChannelView; teamId: string; call: CanvasCall; nodeName: (id: string) => string }): JSX.Element {
  const { t } = useTranslation()
  const team = view.teams.find((x) => x.id === teamId)!
  const ids = { channelId: view.channel.id, teamId }
  const running = team.status === 'running'
  const [name, setName] = useState(team.name)
  const [goal, setGoal] = useState(team.goal)
  const [lead, setLead] = useState(team.leadId)
  const [members, setMembers] = useState<string[]>(team.memberIds)
  const [maxMessages, setMaxMessages] = useState(team.limits.maxMessages)
  const [perMember, setPerMember] = useState(team.limits.perMemberPer10Min)
  const [bypass, setBypass] = useState(false)
  useEffect(() => {
    window.api.settings.get().then((s) => setBypass(!!s.cliBypassPermissions)).catch(() => {})
  }, [])
  const others = new Set(view.teams.filter((x) => x.id !== teamId).flatMap((x) => x.memberIds))
  const dirty = name !== team.name || goal !== team.goal || lead !== team.leadId || members.join() !== team.memberIds.join() || maxMessages !== team.limits.maxMessages || perMember !== team.limits.perMemberPer10Min
  const save = async (): Promise<void> => {
    const req: Parameters<CanvasCall>[1] & object = { ...ids, name, limits: { maxMessages, perMemberPer10Min: perMember } }
    // Members, lead and goal only while not running; the backend refuses them otherwise
    const full = running ? req : { ...req, goal, leadId: lead, memberIds: members.includes(lead) ? members : [lead, ...members] }
    await call('team.update', full as never)
  }
  const relay = (r: RelayMode): Promise<unknown> => call('team.update', { ...ids, relay: r })
  return (
    <aside className="cv-right">
      <h3 className="cv-h">{t('canvas.team')} · {team.name}</h3>
      <div className="cv-row">
        <span className={`cv-pill st-${team.status}`}>{t(`canvas.teamStatus_${team.status}`)}</span>
        {team.relay !== 'approve' && <span className="cv-muted">{t('canvas.teamMessages', { n: team.messageCount, max: team.limits.maxMessages })}</span>}
      </div>
      {team.pausedReason && team.status === 'paused' && <p className="cv-muted">{t('canvas.teamPausedReason', { reason: team.pausedReason })}</p>}
      {bypass && <p className="cv-warn">{t('canvas.teamBypassNote')}</p>}
      <div className="cv-row">
        {team.status === 'draft' || team.status === 'done' || team.status === 'failed' ? (
          <button className="cv-btn primary" disabled={!team.goal.trim() || view.stop.requested} onClick={() => void call('team.start', ids)}>{t('canvas.teamStart')}</button>
        ) : running ? (
          <button className="cv-btn" onClick={() => void call('team.pause', ids)}>{t('canvas.teamPause')}</button>
        ) : (
          <button className="cv-btn primary" disabled={view.stop.requested} onClick={() => void call('team.resume', ids)}>{t('canvas.teamResume')}</button>
        )}
        {team.status !== 'done' && team.status !== 'draft' && <button className="cv-btn" onClick={() => void call('team.finish', ids)}>{t('canvas.teamFinish')}</button>}
      </div>

      <RelayCards view={view} teamId={teamId} call={call} nodeName={nodeName} />

      <section className="cv-sec">
        <h4 className="cv-h4">{t('canvas.teamRelay')}</h4>
        <div className="segmented">
          {(['approve', 'lead-auto', 'auto'] as const).map((r) => (
            <button key={r} className={team.relay === r ? 'on' : ''} onClick={() => void relay(r)}>{t(`canvas.relay_${r.replace('-', '_')}`)}</button>
          ))}
        </div>
        <p className="cv-muted">{t(`canvas.relayHint_${team.relay.replace('-', '_')}`)}</p>
      </section>

      <section className="cv-sec">
        <label className="cv-field">
          <span>{t('canvas.teamName')}</span>
          <input value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="cv-field">
          <span>{t('canvas.teamGoal')}</span>
          <textarea className="cv-textarea" rows={5} value={goal} disabled={running} placeholder={t('canvas.teamGoalHint')} onChange={(e) => setGoal(e.target.value)} />
        </label>
        <label className="cv-field">
          <span>{t('canvas.teamLead')}</span>
          <select value={lead} disabled={running} onChange={(e) => setLead(e.target.value)}>
            {members.map((m) => <option key={m} value={m}>{nodeName(m)}</option>)}
          </select>
        </label>
        <div className="cv-field">
          <span>{t('canvas.teamMembers')}</span>
          {view.channel.nodes.map((n) => (
            <label key={n.id} className="cv-check">
              <input
                type="checkbox"
                checked={members.includes(n.id)}
                disabled={running || others.has(n.id) || n.id === lead}
                onChange={(e) => setMembers((ms) => (e.target.checked ? [...ms, n.id] : ms.filter((x) => x !== n.id)))}
              />
              {nodeName(n.id)}
              {n.id === lead && <span className="cv-muted"> · {t('canvas.role_lead')}</span>}
            </label>
          ))}
          <p className="cv-muted">{t('canvas.teamMembersHint')}</p>
        </div>
        {team.relay !== 'approve' && (
          <div className="cv-field">
            <span>{t('canvas.teamLimits')}</span>
            <label className="cv-check">
              <input type="number" min={1} value={maxMessages} onChange={(e) => setMaxMessages(Math.max(1, Number(e.target.value) || 1))} /> {t('canvas.limitMessages')}
            </label>
            <label className="cv-check">
              <input type="number" min={1} value={perMember} onChange={(e) => setPerMember(Math.max(1, Number(e.target.value) || 1))} /> {t('canvas.limitPerMember')}
            </label>
            <p className="cv-muted">{t('canvas.limitsHint')}</p>
          </div>
        )}
        <div className="cv-row">
          <button className="cv-btn primary" disabled={!dirty} onClick={() => void save()}>{t('canvas.teamSave')}</button>
          <button className="cv-btn danger" disabled={running} onClick={() => window.confirm(t('canvas.teamDeleteConfirm')) && void call('team.delete', ids)}>{t('canvas.teamDelete')}</button>
        </div>
      </section>

      <section className="cv-sec">
        <h4 className="cv-h4">{t('canvas.recentMessages')}</h4>
        <Messages view={view} list={view.messages.filter((m) => m.teamId === teamId)} nodeName={nodeName} />
      </section>
    </aside>
  )
}

/** In the node Inspector: the node's team, role and task, or a button to start a team with it as lead */
export function NodeTeamSection({ view, nodeId, call, onPickTeam }: { view: ChannelView; nodeId: string; call: CanvasCall; onPickTeam: (teamId: string) => void }): JSX.Element {
  const { t } = useTranslation()
  const rt = view.runtime[nodeId]
  const team = view.teams.find((x) => x.memberIds.includes(nodeId))
  const task = rt?.task
  return (
    <section className="cv-sec">
      <dl className="cv-dl">
        <dt>{t('canvas.inTeam')}</dt>
        <dd>
          {team ? (
            <button className="cv-link" onClick={() => onPickTeam(team.id)}>
              {team.name} · {t(`canvas.role_${rt?.role || 'member'}`)}
            </button>
          ) : (
            <button
              className="cv-btn"
              onClick={async () => {
                const n = view.channel.nodes.find((x) => x.id === nodeId)!
                const created = await call('team.create', {
                  channelId: view.channel.id,
                  name: `${t('canvas.team')} ${view.teams.length + 1}`,
                  goal: '',
                  leadId: nodeId,
                  memberIds: [nodeId],
                  relay: 'approve',
                  bounds: { x: n.position.x, y: n.position.y, width: n.size.width, height: n.size.height }
                })
                if (created) onPickTeam(created.id)
              }}
            >
              {t('canvas.newTeam')}
            </button>
          )}
        </dd>
        <dt>{t('canvas.task')}</dt>
        <dd>{task ? `${task.title} · ${t(`canvas.task_${task.status}`)}` : t('canvas.noTask')}</dd>
        {task?.summary && (
          <>
            <dt />
            <dd className="cv-muted">{task.summary}</dd>
          </>
        )}
        {(rt?.unread ?? 0) > 0 && (
          <>
            <dt />
            <dd>{t('canvas.unread', { n: rt!.unread })}</dd>
          </>
        )}
      </dl>
    </section>
  )
}

/** Feed: one card per session, grouped by team, then the activity timeline */
export function TeamFeed({
  view,
  call,
  nodeName,
  onPick,
  timeline
}: {
  view: ChannelView
  call: CanvasCall
  nodeName: (id: string) => string
  onPick: (node: string) => void
  timeline: JSX.Element
}): JSX.Element {
  const { t } = useTranslation()
  const lastAt = (node: string): number | undefined => {
    for (let i = view.channel.events.length - 1; i >= 0; i--) {
      const e = view.channel.events[i] as { at: number; node?: string; from?: string; to?: string }
      if (e.node === node || e.from === node || e.to === node) return e.at
    }
    return undefined
  }
  const card = (id: string): JSX.Element | null => {
    const n = view.channel.nodes.find((x) => x.id === id)
    if (!n) return null
    const rt = view.runtime[id]
    const state = !rt || rt.status !== 'running' ? (rt?.status || 'exited') : rt.approval ? 'waiting' : rt.busy ? 'running' : 'idle'
    const at = lastAt(id)
    return (
      <div key={id} className={`cv-card st-${state}`}>
        <button className="cv-card-head" onClick={() => onPick(id)}>
          <AgentMark agent={markOf(n.agent)} size={16} />
          <span className="cv-card-title">{nodeName(id)}</span>
          {rt?.role && <span className="cv-pill">{t(`canvas.role_${rt.role}`)}</span>}
          <span className={`cv-dot st-${state === 'waiting' ? 'waiting' : state === 'running' ? 'running' : state === 'error' ? 'error' : 'idle'}`} />
        </button>
        <div className="cv-card-task">
          {rt?.task ? (
            <>
              <strong>{rt.task.title}</strong> · {t(`canvas.task_${rt.task.status}`)}
              {rt.task.summary && <div className="cv-muted">{rt.task.summary}</div>}
            </>
          ) : (
            <span className="cv-muted">{t('canvas.noTask')}</span>
          )}
        </div>
        <div className="cv-card-foot cv-muted">
          <span>{n.worktree ? n.worktree.branch : t('canvas.repoRoot')}</span>
          {at && <span>{t('canvas.lastActive', { time: new Date(at).toLocaleTimeString() })}</span>}
          {(rt?.unread ?? 0) > 0 && <span>{t('canvas.unread', { n: rt!.unread })}</span>}
        </div>
        {rt?.approval && (rt.approval.canApprove || rt.approval.canReject) && (
          <div className="cv-row">
            <span className="cv-muted">{rt.approval.title}</span>
            {rt.approval.canApprove && (
              <button className="cv-btn primary" disabled={view.stop.requested} onClick={() => void call('node.approve', { channelId: view.channel.id, nodeId: id, requestId: rt.approval!.requestId, sessionGeneration: rt.approval!.sessionGeneration })}>
                {t('canvas.approve')}
              </button>
            )}
            {rt.approval.canReject && (
              <button className="cv-btn" onClick={() => void call('node.reject', { channelId: view.channel.id, nodeId: id, requestId: rt.approval!.requestId, sessionGeneration: rt.approval!.sessionGeneration })}>
                {t('canvas.reject')}
              </button>
            )}
          </div>
        )}
      </div>
    )
  }
  const grouped = new Set(view.teams.flatMap((x) => x.memberIds))
  const loose = view.channel.nodes.filter((n) => !grouped.has(n.id))
  return (
    <div className="cv-feed">
      {view.teams.map((team) => (
        <section key={team.id} className="cv-feed-team">
          <div className="cv-feed-team-head">
            <strong>{team.name}</strong>
            <span className={`cv-pill st-${team.status}`}>{t(`canvas.teamStatus_${team.status}`)}</span>
            {team.goal && <span className="cv-muted cv-ellipsis">{team.goal}</span>}
          </div>
          <RelayCards view={view} teamId={team.id} call={call} nodeName={nodeName} />
          <div className="cv-cards">{[team.leadId, ...team.memberIds.filter((m) => m !== team.leadId)].map(card)}</div>
        </section>
      ))}
      {loose.length > 0 && (
        <section className="cv-feed-team">
          {view.teams.length > 0 && <div className="cv-feed-team-head"><strong>{t('canvas.ungrouped')}</strong></div>}
          <div className="cv-cards">{loose.map((n) => card(n.id))}</div>
        </section>
      )}
      <h4 className="cv-h4">{t('canvas.feedTimeline')}</h4>
      {timeline}
    </div>
  )
}
