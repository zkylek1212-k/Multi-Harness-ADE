// 卡片模式（使用者要的第二種檢視）：每個 session 一張卡，一眼看出誰在做什麼、卡在哪、有沒有等你處理的事。
// 與畫布模式看的是同一份 ChannelView；終端不搬到這裡，點卡片回畫布或接手。
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import type { ChannelView } from '../../../../shared/canvas'
import type { CanvasCall } from './CanvasView'

type MarkId = Parameters<typeof AgentMark>[0]['agent']

export default function CardsView({
  view,
  call,
  selectedNode,
  nodeName,
  markOfNode,
  onSelect,
  onShow,
  onTakeOver,
  timeline
}: {
  view: ChannelView
  call: CanvasCall
  selectedNode: string | null
  nodeName: (id: string) => string
  markOfNode: (id: string) => MarkId
  /** Select without leaving the cards: the right panel follows */
  onSelect: (node: string) => void
  /** Go to the node on the canvas */
  onShow: (node: string) => void
  onTakeOver: (node: string) => void
  timeline: JSX.Element
}): JSX.Element {
  const { t } = useTranslation()
  const ch = view.channel.id
  const lastAt = (node: string): number | undefined => {
    for (let i = view.channel.events.length - 1; i >= 0; i--) {
      const e = view.channel.events[i] as { at: number; node?: string; from?: string; to?: string }
      if (e.node === node || e.from === node || e.to === node) return e.at
    }
    return undefined
  }
  const lead = view.teams[0]?.leadId
  // The lead first, then the order they joined
  const nodes = [...view.channel.nodes].sort((a, b) => Number(b.id === lead) - Number(a.id === lead))
  return (
    <div className="cv-cardsview">
      {nodes.length === 0 && <p className="cv-muted">{t('canvas.empty')}</p>}
      <div className="cv-cards">
        {nodes.map((n) => {
          const rt = view.runtime[n.id]
          const a = rt?.approval
          const state = !rt || rt.status !== 'running' ? (rt?.status === 'error' ? 'error' : 'off') : a ? 'waiting' : rt.busy ? 'running' : 'idle'
          const held = view.relays.filter((m) => m.from === n.id).length + view.mergeRequests.filter((m) => m.lead === n.id).length
          const at = lastAt(n.id)
          return (
            <div key={n.id} className={`cv-card st-${state} ${selectedNode === n.id ? 'selected' : ''}`} onClick={() => onSelect(n.id)}>
              <div className="cv-card-head">
                <span className="cv-card-avatar">
                  <AgentMark agent={markOfNode(n.id)} size={22} />
                </span>
                <span className="cv-card-names">
                  <span className="cv-card-title">{nodeName(n.id)}</span>
                  <span className={`cv-card-state st-${state}`}>
                    <span className={`cv-dot st-${state === 'off' ? 'idle' : state}`} /> {t(`canvas.card_${state}`)}
                  </span>
                </span>
                {rt?.role === 'lead' && <span className="cv-pill lead">★ {t('canvas.role_lead')}</span>}
              </div>
              <div className="cv-card-task">
                {rt?.task ? (
                  <>
                    <div className="cv-card-tasktitle">{rt.task.title}</div>
                    <span className={`cv-pill st-task-${rt.task.status}`}>{t(`canvas.task_${rt.task.status}`)}</span>
                    {rt.task.summary && <div className="cv-card-summary">{rt.task.summary}</div>}
                  </>
                ) : (
                  <span className="cv-muted">{t('canvas.noTask')}</span>
                )}
              </div>
              {a && (
                <div className="cv-card-need" onClick={(e) => e.stopPropagation()}>
                  <span>{a.title}</span>
                  <span className="cv-row">
                    {a.canApprove && !view.stop.requested && (
                      <button className="cv-btn primary" onClick={() => void call('node.approve', { channelId: ch, nodeId: n.id, requestId: a.requestId, sessionGeneration: a.sessionGeneration })}>
                        {t('canvas.approve')}
                      </button>
                    )}
                    {a.canReject && (
                      <button className="cv-btn" onClick={() => void call('node.reject', { channelId: ch, nodeId: n.id, requestId: a.requestId, sessionGeneration: a.sessionGeneration })}>
                        {t('canvas.reject')}
                      </button>
                    )}
                    {!a.canApprove && !a.canReject && <button className="cv-btn" onClick={() => onTakeOver(n.id)}>{t('canvas.answerInTerminal')}</button>}
                  </span>
                </div>
              )}
              <div className="cv-card-foot cv-muted">
                <span title={n.worktree?.path}>{n.worktree ? n.worktree.branch : t('canvas.repoRoot')}</span>
                {at && <span>{t('canvas.lastActive', { time: new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) })}</span>}
                {(rt?.unread ?? 0) > 0 && <span className="cv-badge">{t('canvas.unread', { n: rt?.unread ?? 0 })}</span>}
                {held > 0 && <span className="cv-badge">{t('canvas.heldCount', { n: held })}</span>}
              </div>
              <div className="cv-card-actions" onClick={(e) => e.stopPropagation()}>
                <button className="cv-btn" onClick={() => onShow(n.id)}>{t('canvas.showOnCanvas')}</button>
                <button className="cv-btn" disabled={rt?.status !== 'running'} onClick={() => onTakeOver(n.id)}>{t('canvas.takeOver')}</button>
              </div>
            </div>
          )
        })}
      </div>
      <details className="cv-timeline">
        <summary>{t('canvas.feedTimeline')}</summary>
        {timeline}
      </details>
    </div>
  )
}
