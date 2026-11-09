// 設定視窗的 Cowork 分頁：主席、預設與會者、主席是否接任務、規劃參考資料、每場會議的上限。
// 只改 SettingsModal 的 settings 狀態，由底部「儲存」一起寫回（不自己存檔，避免被 modal 的舊狀態蓋掉）。
import { useEffect, useState } from 'react'
import AgentMark from '@/components/AgentMark'
import { useTranslation } from '@/i18n'
import ModelPicker, { useModelCatalogs } from '@panels/cowork/ModelPicker'
import type { ExtItem } from '../../../preload/index'
import {
  COWORK_AGENTS,
  agentLabel,
  sanitizeCoworkSettings,
  type CoworkAgent,
  type CoworkCapability,
  type CoworkSettings as Settings
} from '../../../shared/cowork'

const LIMIT_FIELDS = [
  { key: 'maxPlanningCalls', label: 'settings.coworkMaxCalls', sub: 'settings.coworkMaxCallsSub', min: 3, max: 30 },
  { key: 'maxPlanningMinutes', label: 'settings.coworkMaxMinutes', sub: 'settings.coworkMaxMinutesSub', min: 1, max: 120 },
  { key: 'maxExecutionMinutes', label: 'settings.coworkMaxExecMinutes', sub: 'settings.coworkMaxExecMinutesSub', min: 1, max: 600 }
] as const

export default function CoworkSettings({ value, onChange }: { value: unknown; onChange: (v: Settings) => void }): JSX.Element {
  const { t } = useTranslation()
  const cw = sanitizeCoworkSettings(value)
  const [caps, setCaps] = useState<CoworkCapability[]>([])
  const { catalogs, refresh } = useModelCatalogs()
  // 規劃時可附上的 skill：沿用擴充功能頁的盤點（有 SKILL.md 的才列）
  const [skills, setSkills] = useState<ExtItem[] | null>(null)

  useEffect(() => {
    window.api.cowork.capabilities(true).then((r) => {
      if (r.ok) setCaps(r.data.agents)
    })
    window.api.ext
      .inventory()
      .then((items) => setSkills(items.filter((i) => i.kind === 'skill' && i.agents.some((a) => a.detail?.endsWith('SKILL.md')))))
      .catch(() => setSkills([]))
  }, [])

  const capOf = (a: CoworkAgent): CoworkCapability | undefined => caps.find((c) => c.agent === a)
  const usable = (a: CoworkAgent): boolean => {
    const c = capOf(a)
    return !c || (c.enabled && c.planning)
  }
  const sub = (a: CoworkAgent): string => {
    const c = capOf(a)
    if (!c) return ''
    if (!c.enabled) return t('cowork.reason_disabled')
    if (c.reason) return t(`cowork.reason_${c.reason}`)
    return a === 'antigravity' ? t('cowork.eligibleSlow') : ''
  }

  const setChair = (a: CoworkAgent): void => {
    if (!usable(a)) return
    onChange({ ...cw, chair: a, participants: cw.participants.includes(a) ? cw.participants : [...cw.participants, a] })
  }
  const toggleParticipant = (a: CoworkAgent, on: boolean): void => {
    const participants = on ? [...new Set([...cw.participants, a])] : cw.participants.filter((x) => x !== a)
    onChange({ ...cw, participants, chair: cw.chair && participants.includes(cw.chair) ? cw.chair : null })
  }

  return (
    <div className="macos-cowork-settings">
      <div className="macos-section">
        <span className="macos-section-header">{t('settings.coworkChairSection')}</span>
        <div className="macos-inset-group" style={{ padding: '14px 16px' }}>
          <p style={{ margin: '0 0 10px', color: 'var(--fg-dim)', fontSize: 'var(--text-size-caption)', lineHeight: 1.5 }}>
            {t('settings.coworkChairSub')}
          </p>
          <div className="macos-lang-cards">
            {COWORK_AGENTS.map((a) => (
              <button
                key={a}
                type="button"
                className={`macos-lang-card ${cw.chair === a ? 'active' : ''}`}
                onClick={() => setChair(a)}
                disabled={!usable(a)}
                style={usable(a) ? undefined : { opacity: 0.5, cursor: 'not-allowed' }}
              >
                <div className="macos-lang-flag">
                  <AgentMark agent={a} size={16} />
                </div>
                <div className="macos-lang-info">
                  <span className="macos-lang-name">{agentLabel(a)}</span>
                  <span className="macos-lang-sub" title={sub(a)}>
                    {sub(a)}
                  </span>
                </div>
                <span className="macos-radio-dot" />
              </button>
            ))}
          </div>
        </div>
      </div>

      <div className="macos-section">
        <span className="macos-section-header">{t('settings.coworkParticipantsSection')}</span>
        <div className="macos-inset-group">
          {COWORK_AGENTS.map((a) => (
            <div key={a} className="macos-row">
              <div className="macos-row-main">
                <div className="macos-row-left">
                  <AgentMark agent={a} size={18} />
                  <div className="macos-row-info">
                    <div className="macos-row-title-row">
                      <span className="macos-row-title">{agentLabel(a)}</span>
                      {cw.chair === a && <span className="macos-type-pill">★ {t('cowork.chair')}</span>}
                    </div>
                    <span className="macos-row-sub">{sub(a)}</span>
                  </div>
                </div>
                <div className="macos-row-right">
                  <label className="apple-toggle">
                    <input
                      type="checkbox"
                      checked={cw.participants.includes(a)}
                      disabled={!usable(a)}
                      onChange={(e) => toggleParticipant(a, e.target.checked)}
                    />
                    <span className="apple-toggle-slider" />
                  </label>
                </div>
              </div>
            </div>
          ))}
          <div className="macos-row">
            <div className="macos-row-main">
              <p style={{ margin: 0, color: 'var(--fg-dim)', fontSize: 'var(--text-size-caption)', lineHeight: 1.5 }}>
                {t('settings.coworkParticipantsSub')}
              </p>
            </div>
          </div>
        </div>
      </div>

      <div className="macos-section">
        <span className="macos-section-header">{t('settings.coworkModelsSection')}</span>
        <div className="macos-inset-group">
          {COWORK_AGENTS.map((a) => {
            const cat = catalogs?.find((c) => c.agent === a)
            const status = !catalogs ? t('cowork.modelLoading') : cat?.error ? t('cowork.modelListError') : ''
            return (
              <div key={a} className="macos-row">
                <div className="macos-row-main macos-cowork-model-row">
                  <div className="macos-row-left">
                    <AgentMark agent={a} size={18} />
                    <div className="macos-row-info">
                      <span className="macos-row-title">{agentLabel(a)}</span>
                      {status && (
                        <span className="macos-row-sub" title={cat?.error}>
                          {status}
                        </span>
                      )}
                    </div>
                  </div>
                  <div className="macos-row-right">
                    <ModelPicker
                      agent={a}
                      catalog={cat}
                      value={cw.models[a] || { model: '', effort: '' }}
                      onChange={(v) => {
                        const models = { ...cw.models }
                        if (v.model || v.effort) models[a] = v
                        else delete models[a]
                        onChange({ ...cw, models })
                      }}
                    />
                  </div>
                </div>
              </div>
            )
          })}
          <div className="macos-row">
            <div className="macos-row-main">
              <p style={{ margin: 0, color: 'var(--fg-dim)', fontSize: 'var(--text-size-caption)', lineHeight: 1.5 }}>
                {t('settings.coworkModelsSub')}
              </p>
              <div className="macos-row-right">
                <button type="button" className="macos-btn-secondary macos-btn-sm" onClick={refresh}>
                  {t('cowork.refreshModels')}
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div className="macos-section">
        <div className="macos-inset-group">
          <div className="macos-row">
            <div className="macos-row-main">
              <div className="macos-row-info">
                <span className="macos-row-title">{t('settings.coworkChairExecutes')}</span>
                <span className="macos-row-sub">{t('settings.coworkChairExecutesSub')}</span>
              </div>
              <div className="macos-row-right">
                <label className="apple-toggle">
                  <input
                    type="checkbox"
                    checked={cw.chairExecutes}
                    onChange={(e) => onChange({ ...cw, chairExecutes: e.target.checked })}
                  />
                  <span className="apple-toggle-slider" />
                </label>
              </div>
            </div>
            <div className="macos-row">
              <div className="macos-row-left">
                <span className="macos-row-title">{t('settings.coworkExecReview')}</span>
                <span className="macos-row-sub">{t('settings.coworkExecReviewSub')}</span>
              </div>
              <div className="macos-row-right">
                <label className="apple-toggle">
                  <input type="checkbox" checked={cw.execReview} onChange={(e) => onChange({ ...cw, execReview: e.target.checked })} />
                  <span className="apple-toggle-slider" />
                </label>
              </div>
            </div>
            <div className="macos-row">
              <div className="macos-row-left">
                <span className="macos-row-title">{t('settings.coworkWorktreeRoot')}</span>
                <span className="macos-row-sub">{t('settings.coworkWorktreeRootSub')}</span>
              </div>
              <div className="macos-row-right">
                <input type="text" value={cw.worktreeRoot} placeholder=".cowork" onChange={(e) => onChange({ ...cw, worktreeRoot: e.target.value })} />
              </div>
            </div>
          </div>
        </div>
      </div>

      <div className="macos-section">
        <span className="macos-section-header">{t('settings.coworkContextSection')}</span>
        <div className="macos-inset-group">
          <div className="macos-row">
            <div className="macos-row-main">
              <div className="macos-row-info">
                <span className="macos-row-title">{t('settings.coworkProjectInstructions')}</span>
                <span className="macos-row-sub">{t('settings.coworkProjectInstructionsSub')}</span>
              </div>
              <div className="macos-row-right">
                <label className="apple-toggle">
                  <input
                    type="checkbox"
                    checked={cw.projectInstructions}
                    onChange={(e) => onChange({ ...cw, projectInstructions: e.target.checked })}
                  />
                  <span className="apple-toggle-slider" />
                </label>
              </div>
            </div>
          </div>
          <div style={{ padding: '12px 16px' }}>
            <div className="macos-row-title">{t('settings.coworkSkills')}</div>
            <p style={{ margin: '2px 0 8px', color: 'var(--fg-dim)', fontSize: 'var(--text-size-caption)', lineHeight: 1.5 }}>
              {t('settings.coworkSkillsSub')}
            </p>
            {skills === null ? (
              <span className="macos-row-sub">…</span>
            ) : skills.length === 0 ? (
              <span className="macos-row-sub">{t('settings.coworkNoSkills')}</span>
            ) : (
              <div className="cw-skill-list">
                {skills.map((s) => (
                  <label key={s.id} className="cw-skill" title={s.description}>
                    <input
                      type="checkbox"
                      checked={cw.skills.includes(s.id)}
                      onChange={(e) =>
                        onChange({ ...cw, skills: e.target.checked ? [...cw.skills, s.id] : cw.skills.filter((k) => k !== s.id) })
                      }
                    />
                    <span className="cw-skill-name">{s.name}</span>
                    {s.description && <span className="cw-skill-desc">{s.description}</span>}
                  </label>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>

      <div className="macos-section">
        <span className="macos-section-header">{t('settings.coworkSummarySection')}</span>
        <div className="macos-inset-group">
          <div className="macos-row"><div className="macos-row-main macos-cowork-option-row">
            <div className="macos-row-info"><span className="macos-row-title">{t('cowork.summarizer')}</span><span className="macos-row-sub">{t('cowork.summaryHint')}</span></div>
            <div className="macos-row-right"><select className="macos-input" aria-label={t('cowork.summarizer')} value={cw.summarizer || ''} onChange={(e) => onChange({ ...cw, summarizer: (e.target.value || null) as CoworkAgent | null })}>
              <option value="">{t('cowork.followChair')}</option>
              {COWORK_AGENTS.map((a) => <option key={a} value={a} disabled={!usable(a)}>{agentLabel(a)}</option>)}
            </select></div>
          </div></div>
          <div className="macos-row"><div className="macos-row-main macos-cowork-option-row">
            <div className="macos-row-info"><span className="macos-row-title">{t('cowork.summaryTiming')}</span><span className="macos-row-sub">{t('cowork.summaryBudgetHint')}</span></div>
            <div className="macos-row-right"><select className="macos-input" aria-label={t('cowork.summaryTiming')} value={cw.summarizeEachRound ? 'round' : 'context'} onChange={(e) => onChange({ ...cw, summarizeEachRound: e.target.value === 'round' })}>
              <option value="context">{t('cowork.summaryOnDemand')}</option><option value="round">{t('cowork.summaryEachRound')}</option>
            </select></div>
          </div></div>
        </div>
      </div>

      <div className="macos-section">
        <span className="macos-section-header">{t('settings.coworkLimitsSection')}</span>
        <div className="macos-inset-group">
          <div className="macos-row"><div className="macos-row-main"><p className="macos-row-sub" style={{ margin: 0 }}>{t('settings.coworkLimitsSub')}</p></div></div>
          {LIMIT_FIELDS.map((f) => (
            <div key={f.key} className="macos-row">
              <div className="macos-row-main">
                <div className="macos-row-info">
                  <span className="macos-row-title">{t(f.label)}</span>
                  <span className="macos-row-sub">{t(f.sub)}</span>
                </div>
                <div className="macos-row-right">
                  <input
                    type="number"
                    aria-label={t(f.label)}
                    className="macos-input"
                    style={{ width: 76, flex: '0 0 76px', textAlign: 'right' }}
                    min={f.min}
                    max={f.max}
                    value={cw.limits[f.key]}
                    onChange={(e) => {
                      const n = Number(e.target.value)
                      if (!Number.isFinite(n)) return
                      onChange({ ...cw, limits: { ...cw.limits, [f.key]: Math.min(f.max, Math.max(f.min, Math.round(n))) } })
                    }}
                  />
                </div>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
