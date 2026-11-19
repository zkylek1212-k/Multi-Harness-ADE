// Routines（cowork-agui-implement.md §11）：照時間或 main 有新 commit，由 Bot 自己開一段新對話。只在 App 開著時跑；
// 錯過的時段跳過；需要核准的動作停在 Needs you；連續失敗 10 次自動關閉。
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import type { CliLauncher } from '../../../../preload/index'
import type { Routine, RoutineSchedule } from '../../../../shared/canvas'
import ModelPicker, { modelLabel } from './ModelPicker'
import { ago, markOf, type CanvasCall } from './types'

const blank = (agent: string): Routine => ({ id: '', name: '', prompt: '', agent, repo: '', scope: 'report', schedule: { kind: 'daily', time: '09:00' }, enabled: true, nextAt: 0, failures: 0 })

export function useRoutines(call: CanvasCall): [Routine[], () => void] {
  const [list, setList] = useState<Routine[]>([])
  const load = useCallback(() => void call('routine.list').then((l) => l && setList(l)), [call])
  useEffect(() => {
    load()
    return window.api.canvas.onRoutines(load)
  }, [load])
  return [list, load]
}

/** Routine findings and failures for the Bots page's Needs you */
export function RoutineFindings({ routines, call, onOpen }: { routines: Routine[]; call: CanvasCall; onOpen: (channelId: string) => void }): JSX.Element | null {
  const { t } = useTranslation()
  const shown = routines.filter((r) => r.lastRun && !r.lastRun.seen && (r.lastRun.status === 'failed' || (r.lastRun.status === 'ok' && r.scope === 'report')))
  if (!shown.length) return null
  return (
    <>
      <div className="cw-muted">{t('cw.fromRoutines')}</div>
      {shown.map((r) => (
        <div key={r.id} className="cw-card">
          <div className="cw-row">
            <b>{r.name}</b>
            <span className={`cw-badge ${r.lastRun!.status === 'failed' ? 'warn' : 'st-done'}`}>{t(`cw.run_${r.lastRun!.status}`)}</span>
            <span className="cw-muted">{ago(r.lastRun!.at)}</span>
          </div>
          {r.lastRun!.summary && <div className="cw-pre-line">{r.lastRun!.summary}</div>}
          <div className="cw-row">
            {r.lastRun!.channel && <button className="cw-btn dark" onClick={() => onOpen(r.lastRun!.channel)}>{t('cw.workOnIt')}</button>}
            <button className="cw-btn" onClick={() => void call('routine.dismiss', { id: r.id })}>{t('cw.dismiss')}</button>
          </div>
        </div>
      ))}
    </>
  )
}

function ScheduleEdit({ s, onChange }: { s: RoutineSchedule; onChange: (s: RoutineSchedule) => void }): JSX.Element {
  const { t } = useTranslation()
  const kind = (k: RoutineSchedule['kind']): RoutineSchedule =>
    k === 'daily' ? { kind: 'daily', time: '09:00' } : k === 'weekly' ? { kind: 'weekly', day: 0, time: '09:00' } : k === 'every' ? { kind: 'every', minutes: 60 } : { kind: 'commit', branch: 'main' }
  return (
    <div className="cw-row">
      <label className="cw-select">
        <span className="cw-sr">{t('cw.when')}</span>
        <select value={s.kind} onChange={(e) => onChange(kind(e.target.value as RoutineSchedule['kind']))}>
          {(['daily', 'weekly', 'every', 'commit'] as const).map((k) => <option key={k} value={k}>{t(`cw.sched_${k}`)}</option>)}
        </select>
      </label>
      {s.kind === 'weekly' && (
        <label className="cw-select">
          <span className="cw-sr">{t('cw.day')}</span>
          <select value={s.day} onChange={(e) => onChange({ ...s, day: Number(e.target.value) })}>
            {[0, 1, 2, 3, 4, 5, 6].map((d) => <option key={d} value={d}>{t(`cw.day_${d}`)}</option>)}
          </select>
        </label>
      )}
      {(s.kind === 'daily' || s.kind === 'weekly') && <input className="cw-input" type="time" aria-label={t('cw.time')} value={s.time} onChange={(e) => onChange({ ...s, time: e.target.value })} />}
      {s.kind === 'every' && (
        <label className="cw-row">
          <input className="cw-input" type="number" min={15} step={5} aria-label={t('cw.minutes')} value={s.minutes} onChange={(e) => onChange({ ...s, minutes: Number(e.target.value) })} />
          <span className="cw-muted">{t('cw.minutes')}</span>
        </label>
      )}
      {s.kind === 'commit' && <input className="cw-input" aria-label={t('cw.branch')} value={s.branch} onChange={(e) => onChange({ ...s, branch: e.target.value.trim() })} />}
    </div>
  )
}

function schedText(s: RoutineSchedule, t: (k: string, v?: Record<string, string | number>) => string): string {
  switch (s.kind) {
    case 'daily':
      return t('cw.schedText_daily', { time: s.time })
    case 'weekly':
      return t('cw.schedText_weekly', { day: t(`cw.day_${s.day}`), time: s.time })
    case 'every':
      return t('cw.schedText_every', { n: s.minutes })
    case 'commit':
      return t('cw.schedText_commit', { branch: s.branch })
  }
}

export default function RoutinesView({ call, launchers, canChat, routines, onOpen }: { call: CanvasCall; launchers: CliLauncher[]; canChat: (agent: string) => boolean; routines: Routine[]; onOpen: (channelId: string) => void }): JSX.Element {
  const { t } = useTranslation()
  const bots = launchers.filter((l) => canChat(l.id))
  const [edit, setEdit] = useState<Routine | null>(null)
  const [busy, setBusy] = useState(false)
  const save = async (r: Routine): Promise<void> => {
    setBusy(true)
    const ok = await call('routine.save', { routine: r })
    setBusy(false)
    if (ok) setEdit(null)
  }
  return (
    <main className="cw-main cw-bots-page">
      <div className="cw-home-inner wide">
        <div className="cw-row">
          <h1 className="cw-grow">{t('cw.routines')}</h1>
          <button className="cw-btn dark" onClick={() => setEdit(blank(bots[0]?.id ?? 'claude'))}>＋ {t('cw.newRoutine')}</button>
        </div>
        <p className="cw-muted">{t('cw.routinesHint')}</p>
        {edit && (
          <section className="cw-sec" aria-label={t('cw.newRoutine')}>
            <input className="cw-input" maxLength={80} placeholder={t('cw.routineName')} aria-label={t('cw.routineName')} value={edit.name} onChange={(e) => setEdit({ ...edit, name: e.target.value })} />
            <textarea className="cw-textarea" rows={4} placeholder={t('cw.routinePrompt')} aria-label={t('cw.routinePrompt')} value={edit.prompt} onChange={(e) => setEdit({ ...edit, prompt: e.target.value })} />
            <div className="cw-row">
              <label className="cw-select">
                <span className="cw-sr">{t('cw.bot')}</span>
                <select value={edit.agent} onChange={(e) => setEdit({ ...edit, agent: e.target.value })}>
                  {bots.map((l) => <option key={l.id} value={l.id}>{l.name}</option>)}
                </select>
              </label>
              <div className="cw-seg" role="radiogroup" aria-label={t('cw.scope')}>
                {(['report', 'change'] as const).map((k) => (
                  <button key={k} role="radio" aria-checked={edit.scope === k} className={edit.scope === k ? 'on' : ''} onClick={() => setEdit({ ...edit, scope: k })}>{t(`cw.scope_${k}`)}</button>
                ))}
              </div>
            </div>
            <ModelPicker call={call} agent={edit.agent} model={edit.model ?? ''} effort={edit.effort ?? ''} onChange={(model, effort) => setEdit({ ...edit, model: model || undefined, effort: effort || undefined })} />
            <ScheduleEdit s={edit.schedule} onChange={(schedule) => setEdit({ ...edit, schedule })} />
            <p className="cw-muted">{t(`cw.scopeHint_${edit.scope}`)}</p>
            <div className="cw-row">
              <button className="cw-btn dark" disabled={busy || !edit.name.trim() || !edit.prompt.trim()} onClick={() => void save(edit)}>{t('common.save')}</button>
              <button className="cw-btn" onClick={() => setEdit(null)}>{t('common.cancel')}</button>
            </div>
          </section>
        )}
        {routines.length === 0 && !edit && <p className="cw-muted">{t('cw.noRoutines')}</p>}
        {routines.map((r) => (
          <div key={r.id} className="cw-card cw-routine">
            <div className="cw-row">
              <AgentMark agent={markOf(r.agent)} size={18} />
              <b>{r.name}</b>
              <span className="cw-chip">{t(`cw.scope_${r.scope}`)}</span>
              <span className="cw-muted">{schedText(r.schedule, t)}</span>
              {modelLabel(r.model, r.effort) && <span className="cw-muted">{modelLabel(r.model, r.effort)}</span>}
              <div className="cw-grow" />
              <label className="cw-toggle">
                <input type="checkbox" checked={r.enabled} onChange={(e) => void call('routine.save', { routine: { ...r, enabled: e.target.checked } })} />
                {t('cw.on')}
              </label>
            </div>
            <div className="cw-row cw-muted">
              <span>{r.repo.split(/[\\/]/).pop()}</span>
              {r.enabled && r.nextAtText && <span>· {t('cw.nextRun', { at: r.nextAtText })}</span>}
              {r.lastRun && <span>· {t(`cw.run_${r.lastRun.status}`)} {ago(r.lastRun.at)}</span>}
              {r.failures > 0 && <span className="cw-error">· {t('cw.failures', { n: r.failures })}</span>}
            </div>
            <div className="cw-row">
              <button className="cw-btn" disabled={r.lastRun?.status === 'running'} onClick={() => void call('routine.runNow', { id: r.id })}>{t('cw.runNow')}</button>
              {r.lastRun?.channel && <button className="cw-btn" onClick={() => onOpen(r.lastRun!.channel)}>{t('cw.lastRunOpen')}</button>}
              <button className="cw-btn" onClick={() => setEdit(r)}>{t('cw.edit')}</button>
              <button className="cw-btn danger" onClick={() => window.confirm(t('cw.deleteRoutine')) && void call('routine.delete', { id: r.id })}>{t('cw.delete')}</button>
            </div>
          </div>
        ))}
      </div>
    </main>
  )
}
