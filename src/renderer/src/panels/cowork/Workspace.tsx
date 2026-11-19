// 右側 Workspace 面板（cowork-agui-implement.md §8.2）：改動（worktree diff、提交、合併）與活動（跑過的指令、讀改過的檔案）。
import { useState } from 'react'
import { useTranslation } from '@/i18n'
import type { ChannelView } from '../../../../shared/canvas'
import { toolSummary } from '../../../../shared/agui'
import Changes from './Changes'
import { useTranscripts } from './Conversation'
import type { CanvasCall } from './types'

function Activity({ view, call, nodeId }: { view: ChannelView; call: CanvasCall; nodeId: string }): JSX.Element {
  const { t } = useTranslation()
  const isChat = view.channel.nodes.find((n) => n.id === nodeId)?.mode === 'chat'
  const tools = (useTranscripts(call, view.channel.id, isChat)[nodeId] ?? []).filter((e) => e.kind === 'tool')
  if (!isChat) return <p className="cw-muted">{t('cw.activityTerminal')}</p>
  if (tools.length === 0) return <p className="cw-muted">{t('cw.activityEmpty')}</p>
  return (
    <ul className="cw-activity">
      {tools
        .slice()
        .reverse()
        .map((e) => (
          <li key={e.id}>
            <span className={`cw-dot st-${e.status === 'running' ? 'busy' : e.status === 'done' ? 'running' : 'exited'}`} />
            <span className="cw-act-name">{e.name}</span>
            <span className="cw-mono">{toolSummary(e)}</span>
          </li>
        ))}
    </ul>
  )
}

/** For the Bot shown in the conversation (`nodeId`): the lead, or the member the user opened */
export default function Workspace({ view, call, nodeId, onClose }: { view: ChannelView; call: CanvasCall; nodeId: string; onClose: () => void }): JSX.Element {
  const { t } = useTranslation()
  const [tab, setTab] = useState<'changes' | 'activity'>('changes')
  const lead = view.channel.nodes.find((n) => n.id === nodeId)
  return (
    <aside className="cw-panel" aria-label={t('cw.workspace')}>
      <div className="cw-row">
        <b>{t('cw.workspace')}</b>
        <div className="cw-grow" />
        <button className="cw-icon" aria-label={t('common.close')} onClick={onClose}>✕</button>
      </div>
      <div className="cw-tabs" role="tablist">
        {(['changes', 'activity'] as const).map((k) => (
          <button key={k} role="tab" aria-selected={tab === k} className={`cw-tab ${tab === k ? 'on' : ''}`} onClick={() => setTab(k)}>
            {t(`cw.tab_${k}`)}
          </button>
        ))}
      </div>
      <div className="cw-panel-body">
        {lead && tab === 'changes' && <Changes key={lead.id} view={view} nodeId={lead.id} call={call} />}
        {lead && tab === 'activity' && <Activity view={view} call={call} nodeId={lead.id} />}
      </div>
    </aside>
  )
}
