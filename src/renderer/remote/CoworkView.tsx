import { useEffect, useRef, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import {
  agentLabel,
  currentBoard,
  discussionProjectPrompt,
  modelPickerState,
  pickModel,
  reviewersOf,
  type AgentModelChoice,
  type CoworkAgent,
  type CoworkCapability,
  type CoworkBaselineInfo,
  type CoworkModelCatalog,
  type CoworkMode,
  type CoworkPhase,
  type CoworkRun,
  type CoworkRunSummary,
  type CoworkSettings,
  mergeBlockers,
  type ExecPreview
} from '../../shared/cowork'
import type { RemoteConnection } from './conn'
import { AgentMark, IArrowUp, ICheck, IPlus, IWarning } from './icons'
import { ago, coworkLanguage, t, type Key } from './i18n'
import { ConfirmSheet, NavBar, Switch } from './ui'

// 手機版 Cowork：清單、開新會議、看會議進度、在每個階段做桌面上同樣的決定。
// 所有操作都經 conn.cowork 打到桌面同一份 ops（main/ipc/cowork.ts），手機不自己判斷能不能做，
// 桌面拒絕就把錯誤碼顯示出來。只有編輯任務板仍只在桌面（cwDesktopOnly）。

type Caps = {
  agents: CoworkCapability[]
  baseline: CoworkBaselineInfo
  defaults?: Pick<CoworkSettings, 'chair' | 'participants' | 'models' | 'summarizer' | 'summarizeEachRound'> & { limits?: CoworkSettings['limits'] }
}
type Draft = { prompt: string; mode: CoworkMode; source?: CoworkRun }

const phaseText = (p: CoworkPhase): string => t(`cwPhase_${p}` as Key)
const phaseTone = (p: CoworkPhase): string =>
  p === 'failed' ? 'danger' : ['awaiting-approval', 'blocked', 'paused', 'review'].includes(p) ? 'wait' : ['meeting', 'executing'].includes(p) ? 'run' : 'idle'

function PhasePill({ phase }: { phase: CoworkPhase }): JSX.Element {
  return (
    <span className={`pill ${phaseTone(phase)}`}>
      <span className="dot" aria-hidden="true" />
      {phaseText(phase)}
    </span>
  )
}

const Md = ({ text }: { text: string }): JSX.Element => (
  <div className="markdown cw-md">
    <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
  </div>
)

export default function CoworkView({
  conn,
  windowId,
  workspace,
  hostName,
  onBack
}: {
  conn: RemoteConnection
  windowId: number
  workspace: string
  hostName: string
  onBack: () => void
}): JSX.Element {
  const [runs, setRuns] = useState<CoworkRunSummary[] | null>(null)
  const [runId, setRunId] = useState<string | null>(null)
  const [run, setRun] = useState<CoworkRun | null>(null)
  // null＝不在開會表單；{}＝空白表單；有 source＝由討論轉成專案計畫
  const [draft, setDraft] = useState<Draft | null>(null)
  const [error, setError] = useState<string | null>(null)
  const runIdRef = useRef(runId)
  runIdRef.current = runId

  const call = async <T,>(op: string, ...args: unknown[]): Promise<T | undefined> => {
    setError(null)
    const r = await conn.cowork<T>(windowId, op, ...args)
    // 沒回傳值的操作（raiseLimits、delete…）成功時 data 是 undefined；改成 null，undefined 只代表失敗
    if (r.ok) return (r.data ?? null) as T
    setError(r.message || r.code)
    return undefined
  }

  const loadList = (): void => {
    void conn.cowork<CoworkRunSummary[]>(windowId, 'list').then((r) => r.ok && setRuns(r.data))
  }

  useEffect(() => {
    const load = (): void => {
      loadList()
      const id = runIdRef.current
      if (id) void conn.cowork<CoworkRun | null>(windowId, 'get', id).then((r) => r.ok && runIdRef.current === id && setRun(r.data))
    }
    const off = conn.onMessage((m) => {
      if (m.t !== 'coworkRun') return
      if (m.run.id === runIdRef.current) setRun(m.run)
      setRuns((prev) => prev && prev.map((x) => (x.id === m.run.id ? { ...x, phase: m.run.phase, updatedAt: m.run.updatedAt } : x)))
    })
    const offState = conn.onState((s) => s === 'open' && load())
    load()
    return () => {
      off()
      offState()
    }
  }, [conn, windowId])

  const open = (id: string | null): void => {
    setDraft(null)
    setRun(null)
    setRunId(id)
    setError(null)
    if (id) void call<CoworkRun | null>('get', id).then((r) => r !== undefined && runIdRef.current === id && setRun(r))
  }

  if (draft) {
    return (
      <NewMeeting
        key={draft.source?.id || 'new'}
        conn={conn}
        windowId={windowId}
        workspace={workspace}
        initial={draft}
        onBack={() => setDraft(null)}
        onOpenRun={open}
        onStarted={(r) => {
          setDraft(null)
          setRuns((prev) => [{ id: r.id, prompt: r.prompt, phase: r.phase, createdAt: r.createdAt, updatedAt: r.updatedAt, planRevision: r.planRevision, chair: r.chair, participants: r.participants }, ...(prev || [])])
          setRunId(r.id)
          setRun(r)
        }}
      />
    )
  }

  if (runId) {
    return (
      <div className="session">
        <NavBar title={t('cowork')} subtitle={workspace} backLabel={t('cowork')} onBack={() => open(null)} />
        {run ? (
          <RunScreen
            run={run}
            call={call}
            error={error}
            onConvert={() => setDraft({ mode: 'project', prompt: discussionProjectPrompt(run), source: run })}
            onDeleted={() => {
              open(null)
              loadList()
            }}
          />
        ) : (
          <p className="empty">{t('loading')}</p>
        )}
      </div>
    )
  }

  return (
    <div className="session">
      <NavBar title={t('cowork')} subtitle={workspace} backLabel={hostName} onBack={onBack} />
      <main className="workspace-pane">
        <div className="stack">
          <button className="add-card press" onClick={() => setDraft({ prompt: '', mode: 'discussion' })}>
            <IPlus size={20} />
            {t('cwNew')}
          </button>
          {runs === null ? (
            <p className="empty">{t('loading')}</p>
          ) : runs.length === 0 ? (
            <p className="empty">{t('cwNone')}</p>
          ) : (
            runs.map((r) => (
              <button key={r.id} className="bubble session-card press" onClick={() => open(r.id)}>
                <AgentMark launcherKey={r.chair} />
                <span className="card-main">
                  <span className="card-title">{r.prompt}</span>
                  <span className="card-sub">{ago(r.updatedAt)}</span>
                </span>
                <PhasePill phase={r.phase} />
              </button>
            ))
          )}
        </div>
        {error && <p className="form-error" role="alert"><IWarning size={18} />{error}</p>}
      </main>
    </div>
  )
}

/** 一家 agent 的模型＋強度（規則與桌面 ModelPicker 相同，見 shared modelPickerState） */
function ModelSelects({
  agent,
  catalog,
  value,
  automaticLabel,
  onChange
}: {
  agent: CoworkAgent
  catalog: CoworkModelCatalog | undefined
  value: AgentModelChoice
  automaticLabel?: string
  onChange: (v: AgentModelChoice) => void
}): JSX.Element {
  const { options, efforts, mustPickEffort, fallbackEffort } = modelPickerState(agent, catalog, value, !!automaticLabel)
  const known = !value.model || options.some((o) => o.id === value.model)
  const fallbackModel = catalog?.fallback.model
  const fallback = fallbackModel
    ? t('cwModelDefaultUser', { model: options.find((o) => o.id === fallbackModel || o.resolvedModel === fallbackModel)?.label || fallbackModel })
    : t('cwModelDefaultCli', { cli: agentLabel(agent) })
  return (
    <div className="cw-models">
      <select className="cw-select" aria-label={t('cwModel')} value={value.model} onChange={(e) => onChange(pickModel(agent, catalog, value, e.target.value, !!automaticLabel))}>
        <option value="">{fallback}</option>
        {!known && <option value={value.model} disabled>{value.model}</option>}
        {options.map((o) => <option key={o.id} value={o.id}>{o.label}</option>)}
      </select>
      <select className="cw-select" aria-label={t('cwEffort')} value={efforts.length ? value.effort : ''} disabled={efforts.length === 0} onChange={(e) => onChange({ model: value.model, effort: e.target.value })}>
        {!mustPickEffort && (
          <option value="">
            {efforts.length === 0 ? t('cwEffortInModel') : automaticLabel || (fallbackEffort ? t('cwEffortDefaultValue', { effort: fallbackEffort }) : t('cwEffortDefault'))}
          </option>
        )}
        {efforts.map((e) => <option key={e} value={e}>{e}</option>)}
      </select>
    </div>
  )
}

// 開會表單：選項與桌面 StartForm 一一對應（模式說明、與會者與不能用的原因、主席、各家模型與強度、
// 摘要 agent 與時機、依階段調強度、專案基線、上限）。預設值同樣來自桌面設定。
function NewMeeting({
  conn,
  windowId,
  workspace,
  initial,
  onBack,
  onStarted,
  onOpenRun
}: {
  conn: RemoteConnection
  windowId: number
  workspace: string
  initial: Draft
  onBack: () => void
  onStarted: (r: CoworkRun) => void
  onOpenRun: (id: string) => void
}): JSX.Element {
  const source = initial.source
  const [caps, setCaps] = useState<Caps | null>(null)
  const [catalogs, setCatalogs] = useState<CoworkModelCatalog[]>([])
  const [prompt, setPrompt] = useState(initial.prompt)
  const [mode, setMode] = useState<CoworkMode>(initial.mode)
  const [autoEffort, setAutoEffort] = useState(source?.autoEffort !== false)
  const [picked, setPicked] = useState<CoworkAgent[]>([])
  const [chair, setChair] = useState<CoworkAgent | null>(null)
  const [summarizer, setSummarizer] = useState<CoworkAgent | null>(null)
  const [summarizeEachRound, setSummarizeEachRound] = useState(false)
  const [models, setModels] = useState<Partial<Record<CoworkAgent, AgentModelChoice>>>({})
  const [showDirty, setShowDirty] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<{ text: string; runId?: string } | null>(null)

  useEffect(() => {
    void conn.cowork<Caps>(windowId, 'capabilities').then((r) => {
      if (!r.ok) return setError({ text: r.message || r.code })
      const d = r.data.defaults
      setCaps(r.data)
      const usable = r.data.agents.filter((a) => a.enabled && a.planning).map((a) => a.agent)
      // 預設：上次選的與會者（還能用的）；不夠兩位就補上能用的
      const p = (source ? source.participants.filter((a) => !source.discussion?.excluded.includes(a)) : d?.participants || []).filter((a) => usable.includes(a))
      for (const a of usable) if (p.length < 2 && !p.includes(a)) p.push(a)
      setPicked(p)
      const c = source?.chair || d?.chair
      setChair(c && p.includes(c) ? c : p[0] || null)
      const rec = source?.discussion?.summarizer || d?.summarizer
      setSummarizer(rec && p.includes(rec) ? rec : null)
      setSummarizeEachRound(d?.summarizeEachRound === true)
      setModels(source?.models || d?.models || {})
    })
    // 模型清單另外讀：agy 要連網，慢的話選單先只有「預設」
    void conn.cowork<CoworkModelCatalog[]>(windowId, 'models').then((r) => r.ok && setCatalogs(r.data))
  }, [conn, windowId])

  const usable = (a: CoworkAgent): boolean => !!caps?.agents.find((c) => c.agent === a && c.enabled && c.planning)
  const toggle = (a: CoworkAgent): void => {
    if (!usable(a)) return
    const next = picked.includes(a) ? picked.filter((x) => x !== a) : [...picked, a]
    setPicked(next)
    if (!chair || !next.includes(chair)) setChair(next[0] || null)
    if (summarizer && !next.includes(summarizer)) setSummarizer(null)
  }
  const baseline = caps?.baseline
  const canStart = !!prompt.trim() && prompt.length <= 20000 && picked.length >= 2 && !!chair && picked.includes(chair) && (mode === 'discussion' || baseline?.ok === true) && !busy

  const start = async (): Promise<void> => {
    if (!canStart) return
    setBusy(true)
    setError(null)
    const chosen = Object.fromEntries(picked.map((a) => [a, models[a] || { model: '', effort: '' }]))
    const r = await conn.cowork<CoworkRun>(windowId, 'start', {
      prompt: prompt.trim(),
      chair,
      participants: picked,
      language: coworkLanguage(),
      models: chosen,
      mode,
      autoEffort,
      ...(mode === 'discussion' ? { summarizer: summarizer || chair, summarizeEachRound } : {})
    })
    setBusy(false)
    if (r.ok) onStarted(r.data)
    else setError({ text: r.message || r.code, runId: r.code === 'active-run-exists' ? (r.data as { runId?: string } | undefined)?.runId : undefined })
  }

  const autoLabel = (a: CoworkAgent): string | undefined =>
    autoEffort ? t(mode === 'discussion' ? 'cwAutoLow' : chair === a ? 'cwAutoChair' : 'cwAutoReview') : undefined
  const limits = caps?.defaults?.limits

  return (
    <div className="session">
      <NavBar title={t('cwNew')} subtitle={workspace} backLabel={t('cowork')} onBack={onBack} />
      <main className="workspace-pane">
        <div className="segmented" style={{ ['--seg' as string]: 2 }} data-index={mode === 'project' ? 1 : 0}>
          <button aria-pressed={mode === 'discussion'} onClick={() => setMode('discussion')}>{t('cwModeDiscussion')}</button>
          <button aria-pressed={mode === 'project'} onClick={() => setMode('project')}>{t('cwModeProject')}</button>
        </div>
        <p className="footnote cw-mode-desc">{t(`cwModeDesc_${mode}`)}</p>
        <div className="bubble form-card">
          <div className="field">
            <textarea className="cw-input" rows={5} maxLength={20000} value={prompt} placeholder={t('cwPrompt')} aria-label={t('cwPrompt')} onChange={(e) => setPrompt(e.target.value)} />
          </div>
        </div>

        <div className="section-head"><h2>{t('cwParticipants')}</h2></div>
        <div className="stack">
          {(caps?.agents || []).map((c) => {
            const ok = c.enabled && c.planning
            const on = picked.includes(c.agent)
            const reason = !c.enabled ? t('cwReason_disabled') : c.reason ? t(`cwReason_${c.reason}`) : ok ? '' : t('cwUnavailable')
            const sub = reason || (c.agent === 'antigravity' ? t('cwEligibleSlow') : '')
            return (
              <div key={c.agent} className={`bubble cw-agent-card ${on ? 'on' : ''}`}>
                <div className="cw-agent-row">
                  <button className="cw-agent press" disabled={!ok} aria-pressed={on} onClick={() => toggle(c.agent)}>
                    <span className={`cw-check ${on ? 'on' : ''}`} aria-hidden="true">{on && <ICheck size={14} />}</span>
                    <AgentMark launcherKey={c.agent} size={36} />
                    <span className="card-main">
                      <span className="card-title">{agentLabel(c.agent)}</span>
                      {sub && <span className="card-sub">{sub}</span>}
                    </span>
                  </button>
                  {on && (chair === c.agent
                    ? <span className="pill run">★ {t('cwChair')}</span>
                    : <button className="pill idle press" onClick={() => setChair(c.agent)}>{t('cwMakeChair')}</button>)}
                </div>
                {on && (
                  <ModelSelects
                    agent={c.agent}
                    catalog={catalogs.find((x) => x.agent === c.agent)}
                    value={models[c.agent] || { model: '', effort: '' }}
                    automaticLabel={autoLabel(c.agent)}
                    onChange={(v) => setModels((prev) => ({ ...prev, [c.agent]: v }))}
                  />
                )}
              </div>
            )
          })}
        </div>

        <ul className="bubble list cw-options">
          {mode === 'discussion' && (
            <>
              <li className="row">
                <span className="row-main">{t('cwSummarizer')}</span>
                <select className="cw-select cw-select-inline" value={summarizer || ''} onChange={(e) => setSummarizer((e.target.value || null) as CoworkAgent | null)}>
                  <option value="">{t('cwFollowChair')}</option>
                  {picked.map((a) => <option key={a} value={a}>{agentLabel(a)}</option>)}
                </select>
              </li>
              <li className="row">
                <span className="row-main">{t('cwSummaryTiming')}</span>
                <select className="cw-select cw-select-inline" value={summarizeEachRound ? 'round' : 'context'} onChange={(e) => setSummarizeEachRound(e.target.value === 'round')}>
                  <option value="context">{t('cwSummaryOnDemand')}</option>
                  <option value="round">{t('cwSummaryEachRound')}</option>
                </select>
              </li>
            </>
          )}
          <li className="row">
            <span className="row-main">{t('cwAutoEffort')}</span>
            <Switch label={t('cwAutoEffort')} checked={autoEffort} onChange={setAutoEffort} />
          </li>
        </ul>
        <p className="footnote">
          {mode === 'discussion' && `${t('cwSummaryBudgetHint')} `}
          {t('cwAutoEffortHint')}
        </p>

        {mode === 'project' && baseline && (
          <div className={`bubble cw-card cw-baseline ${baseline.ok ? '' : 'error'}`}>
            {!baseline.ok ? (
              <span>{BASELINE_ERRORS.has(baseline.code) ? t(`cwBaselineError_${baseline.code}` as Key) : t('cwNeedRepo')}</span>
            ) : (
              <>
                <div>{t('cwBaseline', { branch: baseline.branch, commit: baseline.head.slice(0, 7) })}</div>
                {baseline.dirtyCount > 0 && (
                  <button className="text-btn press" onClick={() => setShowDirty((v) => !v)}>⚠ {t('cwDirtyExcluded', { count: baseline.dirtyCount })}</button>
                )}
                {showDirty && <ul className="cw-file-list">{baseline.dirty.map((f) => <li key={f}>{f}</li>)}</ul>}
                {baseline.warnings.map((w) => <div key={w} className="cw-warn">⚠ {WARNINGS.has(w) ? t(`cwWarn_${w}` as Key) : w}</div>)}
              </>
            )}
          </div>
        )}

        {error && (
          <p className="form-error" role="alert">
            <IWarning size={18} />
            {error.text}
            {error.runId && <button className="text-btn strong press" onClick={() => onOpenRun(error.runId!)}>{t('cwOpenActive')}</button>}
          </p>
        )}
        <p className="footnote">
          {caps && picked.length < 2 ? t('cwNeedTwo') : limits ? t('cwLimits', { calls: limits.maxPlanningCalls, minutes: limits.maxPlanningMinutes }) : ''}
        </p>
        <p className="footnote">{t('cwDesktopOnly')}</p>
        <button className="btn prominent press" disabled={!canStart} onClick={start}>{busy ? t('cwStarting') : t(`cwStart_${mode}`)}</button>
      </main>
    </div>
  )
}

// 有翻譯的基線錯誤與警告；新的代碼直接顯示原字
const BASELINE_ERRORS = new Set(['not-a-git-repo', 'no-commits'])
const WARNINGS = new Set(['merge-in-progress', 'rebase-in-progress', 'cherry-pick-in-progress', 'codex-repo-in-profile'])

type Call = <T>(op: string, ...args: unknown[]) => Promise<T | undefined>

function RunScreen({
  run,
  call,
  error,
  onConvert,
  onDeleted
}: {
  run: CoworkRun
  call: Call
  error: string | null
  onConvert: () => void
  onDeleted: () => void
}): JSX.Element {
  const [text, setText] = useState('')
  const [confirm, setConfirm] = useState<'cancel' | 'delete' | null>(null)
  const [busy, setBusy] = useState(false)
  const [preview, setPreview] = useState<ExecPreview | null>(null)
  const [waive, setWaive] = useState('')
  const [mergeWaive, setMergeWaive] = useState('')
  const [diff, setDiff] = useState<{ text: string; truncated: boolean } | null>(null)
  const d = run.discussion
  const board = currentBoard(run)
  const ex = run.execution
  const blockers = mergeBlockers(ex)
  const act = async (op: string, ...args: unknown[]): Promise<boolean> => {
    setBusy(true)
    const ok = (await call(op, run.id, ...args)) !== undefined
    setBusy(false)
    return ok
  }

  // 輸入框：跟桌面同樣依階段決定送到哪裡
  const talkable = ex && board && !ex.cleaned && ['executing', 'review'].includes(run.phase)
    ? board.tasks.filter((x) => ex.tasks[x.id] && !['pending', 'running', 'blocked'].includes(ex.tasks[x.id].status))
    : []
  const composer: { placeholder: string; send: (v: string) => Promise<boolean> } | null = d
    ? run.phase === 'completed' ? { placeholder: t('cwFollowUp'), send: (v) => act('discuss', v) } : null
    : talkable.length
      ? { placeholder: t('cwAskTask', { task: talkable[talkable.length - 1].id }), send: (v) => act('execMessage', talkable[talkable.length - 1].id, v) }
      : run.phase === 'meeting' || run.phase === 'paused' || (run.phase === 'blocked' && run.block?.kind !== 'unresolved')
        ? { placeholder: t('cwNote'), send: (v) => act('note', v) }
        : run.phase === 'awaiting-approval' || run.phase === 'approved' || (run.phase === 'blocked' && run.block?.kind === 'unresolved')
          ? { placeholder: t('cwFeedback'), send: (v) => act('feedback', v) }
          : null
  const submit = async (): Promise<void> => {
    const v = text.trim()
    if (v && composer && (await composer.send(v))) setText('')
  }

  const speaker = d && (run.pending?.step === 'summary' ? d.summarizer || run.chair : run.pending?.step === 'conclusion' ? run.chair : d.order[d.cursor])
  const kind = run.block?.kind || 'step-failed'
  const canCancel = ['meeting', 'awaiting-approval', 'blocked', 'paused'].includes(run.phase)
  const canDelete = run.phase !== 'meeting' && run.phase !== 'executing'
  const hasConclusion = !!d && d.conclusion?.through === d.messages.length

  const actions: JSX.Element[] = []
  const btn = (label: string, onClick: () => void, prominent = false): void => {
    actions.push(<button key={label} className={`btn ${prominent ? 'prominent' : ''} press`} disabled={busy} onClick={onClick}>{label}</button>)
  }
  if (d && run.phase === 'completed') {
    if (d.summaries?.at(-1)?.through !== d.messages.length) btn(t('cwSummarize'), () => act('summarize'))
    if (!hasConclusion) btn(t('cwConclude'), () => act('summarize', true), true)
    else btn(t('cwConvertProject'), onConvert, true)
  }
  if (run.phase === 'awaiting-approval') btn(t('cwApprove', { rev: run.planRevision }), () => act('approve', run.planRevision), true)
  if (run.phase === 'blocked' || run.phase === 'paused') {
    if (kind === 'unresolved') btn(t('cwSetAside'), () => act('dismiss'))
    else if (kind === 'budget')
      btn(t('cwRaiseRetry'), async () => {
        const more = { maxPlanningCalls: run.limits.maxPlanningCalls + 3, maxPlanningMinutes: run.limits.maxPlanningMinutes + 5 }
        if (await act('raiseLimits', more)) await act('retry')
      }, true)
    else btn(t('cwRetry'), () => act('retry'), true)
    if (kind === 'reviewers-failed' && reviewersOf(run).some((a) => run.reviewers[a]?.status === 'ok')) btn(t('cwContinueWithout'), () => act('drop'))
    // 討論中某位發言失敗：跟桌面一樣可以略過他繼續
    if (d && kind === 'step-failed' && run.pending?.step === 'discussion' && speaker) btn(t('cwSkipSpeaker', { name: agentLabel(speaker) }), () => act('drop'))
  }
  if (run.phase === 'approved' && !ex) {
    const ask = async (mode: 'sequential' | 'parallel'): Promise<void> => {
      setBusy(true)
      const p = (await call('execPreview', run.id, { mode })) as ExecPreview | undefined
      setBusy(false)
      setPreview(p || null)
    }
    if (preview) {
      const blocked = preview.checks.some((c) => c.error)
      btn(t('cwExecConfirm'), async () => {
        await act('execStart', { mode: preview.mode, approval: preview.hash })
        setPreview(null)
      }, !blocked)
      btn(t('cwCancel'), () => setPreview(null))
    } else {
      btn(t('cwExecSeq'), () => ask('sequential'), true)
      btn(t('cwExecPar'), () => ask('parallel'))
    }
  }
  if (run.phase === 'executing' && ex) btn(ex.paused ? t('cwResume') : t('cwPause'), () => act(ex.paused ? 'execResume' : 'execPause'), ex.paused)
  if (run.phase === 'review' && ex?.integration?.status === 'ok' && ex.targetCheck?.status !== 'conflict' && !(blockers.length && !mergeWaive.trim())) {
    btn(t('cwMerge', { branch: run.repo.sourceBranch }), () => act('execMerge', { commit: ex.integration?.commit || '', targetHead: ex.targetHead || '', ...(mergeWaive.trim() ? { waive: mergeWaive.trim() } : {}) }), true)
  }
  if ((run.phase === 'review' || run.phase === 'completed') && ex && !ex.cleaned) btn(t('cwCleanup'), () => act('execCleanup'))
  if (canCancel) actions.push(<button key="cancel" className="text-btn press" disabled={busy} onClick={() => setConfirm('cancel')}>{t('cwCancel')}</button>)
  else if (canDelete) actions.push(<button key="delete" className="text-btn press" disabled={busy} onClick={() => setConfirm('delete')}>{t('cwDelete')}</button>)

  return (
    <>
      <main className="workspace-pane">
        <section className="bubble cw-card">
          <div className="cw-head">
            <PhasePill phase={run.phase} />
            <span className="t-foot">{ago(run.updatedAt)}</span>
          </div>
          <p className="cw-prompt">{run.prompt}</p>
        </section>

        {d && (
          <ul className="bubble list cw-options">
            <li className="row">
              <span className="row-main">{t('cwSummarizer')}</span>
              <select
                className="cw-select cw-select-inline"
                value={d.summarizer || run.chair}
                disabled={busy || !['completed', 'blocked', 'paused'].includes(run.phase)}
                onChange={(e) => void act('summarizer', e.target.value)}
              >
                {run.participants.map((a) => <option key={a} value={a} disabled={d.excluded.includes(a)}>{agentLabel(a)}</option>)}
              </select>
            </li>
          </ul>
        )}

        {d?.messages.map((m) => (
          <article key={m.id} className={`bubble cw-card ${m.agent ? '' : 'cw-user'}`}>
            <div className="cw-head">
              {m.agent && <AgentMark launcherKey={m.agent} size={28} />}
              <b>{m.agent ? agentLabel(m.agent) : t('cwYou')}</b>
            </div>
            <Md text={m.message} />
          </article>
        ))}
        {d?.summaries?.length ? (
          <article className="bubble cw-card"><b>{t('cwSummary')}</b><Md text={d.summaries[d.summaries.length - 1].summary} /></article>
        ) : null}
        {d?.conclusion && <article className="bubble cw-card"><b>{t('cwConclusion')}</b><Md text={d.conclusion.message} /></article>}

        {!d && run.r1 && <article className="bubble cw-card"><b>{agentLabel(run.chair)}</b><Md text={run.r1.summary} /></article>}
        {!d && board && (
          <>
            <div className="section-head"><h2>{t('cwBoard', { rev: board.planRevision })}</h2></div>
            <div className="stack">
              {board.tasks.map((task) => {
                const te = ex?.tasks[task.id]
                const last = te?.turns.filter((x) => x.role === 'agent').at(-1)
                return (
                  <details key={task.id} className="bubble cw-card">
                    <summary className="cw-head">
                      <AgentMark launcherKey={task.assignee} size={28} />
                      <span className="card-main"><span className="card-title">{task.id} · {task.title}</span></span>
                      {te && <span className={`pill ${te.status === 'done' ? 'run' : te.status === 'failed' ? 'danger' : 'idle'}`}>{te.status}</span>}
                    </summary>
                    <Md text={task.detail} />
                    {last && <Md text={last.error || last.text} />}
                    {te?.status === 'failed' && <button className="text-btn press" disabled={busy} onClick={() => act('execRetry', task.id)}>{t('cwRetry')}</button>}
                    {te?.status === 'blocked' && te.scopeReview && !ex?.cleaned && (
                      <div className="cw-scope-review">
                        <b>{t('cwScopeTitle', { count: te.scopeReview.files.length })}</b>
                        <ul>{te.scopeReview.files.map((f) => <li key={f}><code>{f}</code></li>)}</ul>
                        <button className="text-btn press" disabled={busy} onClick={() => act('execRevertScope', task.id)}>{t('cwScopeRevert')}</button>
                        <button className="text-btn press" disabled={busy} onClick={() => act('execAcceptScope', task.id, te.scopeReview?.hash)}>{t('cwScopeAccept')}</button>
                      </div>
                    )}
                    {te?.status === 'blocked' && te.gate && !ex?.cleaned && (
                      <div className="cw-scope-review">
                        <b>{t(te.gate.kind === 'acceptance' ? 'cwGateAcceptance' : te.gate.kind === 'checks' ? 'cwGateChecks' : 'cwGateGit')}</b>
                        <pre className="cw-pre">{te.gate.items.join('\n')}</pre>
                        {te.gate.kind !== 'git-config' && (
                          <>
                            <input className="cw-select" value={waive} placeholder={t('cwWaivePlaceholder')} onChange={(e) => setWaive(e.target.value)} />
                            <button className="text-btn press" disabled={busy || !waive.trim()} onClick={() => act('execWaive', task.id, waive)}>{t('cwWaive')}</button>
                          </>
                        )}
                        <button className="text-btn press" disabled={busy} onClick={() => act('execRetry', task.id)}>{t('cwRetry')}</button>
                      </div>
                    )}
                    {te?.waiver && <p className="t-foot">{t('cwWaived', { reason: te.waiver.reason })}</p>}
                  </details>
                )
              })}
            </div>
            {board.unresolved.length > 0 && (
              <article className="bubble cw-card">
                <b>{t('cwUnresolved')}</b>
                <ul>{board.unresolved.map((u) => <li key={u.issueId}>{u.text}</li>)}</ul>
              </article>
            )}
          </>
        )}
        {preview && (
          <article className="bubble cw-card">
            <b>{t('cwExecConfirmTitle')}</b>
            <p className="t-foot">{t(preview.bypass ? 'cwExecBypassOn' : 'cwExecBypassOff')}</p>
            {preview.checks.map((c, i) => (
              <p key={i} className="t-foot"><code>{c.argv.join(' ')}</code>{c.resolved && c.resolved !== c.argv[0] ? <> → <code>{c.resolved}</code></> : null}{c.error ? ` (${c.error})` : ''}</p>
            ))}
            {preview.checks.length > 0 && <p className="t-foot">{t('cwExecChecksRisk')}</p>}
          </article>
        )}
        {ex?.integration?.stat && <pre className="bubble cw-card cw-pre">{ex.integration.stat}</pre>}
        {run.phase === 'review' && ex?.integration?.status === 'ok' && (
          <article className="bubble cw-card">
            {ex.gates && <p className="t-foot">{t(ex.integration.checks === 'passed' ? 'cwIntPassed' : ex.integration.checks === 'failed' ? 'cwIntFailed' : 'cwIntNone')}</p>}
            {ex.targetCheck && <p className="t-foot">{t('cwTargetMoved', { branch: run.repo.sourceBranch, status: ex.targetCheck.status })}</p>}
            {ex.review && (
              <>
                <b>{t('cwReview', { agent: ex.review.agent ? agentLabel(ex.review.agent) : '-' })}</b>
                <p className="t-foot">{t('cwReviewAdvisory')}</p>
                {ex.review.status === 'failed' && <p className="t-foot">{ex.review.error}</p>}
                {ex.review.message && <Md text={ex.review.message} />}
                {ex.review.findings.map((f, i) => <p key={i} className="t-foot"><b>{f.severity}</b> <code>{f.file}{f.line ? `:${f.line}` : ''}</code> {f.problem}</p>)}
              </>
            )}
            {blockers.length > 0 && (
              <div className="cw-scope-review">
                <b>{t('cwMergeBlockers')}</b>
                <ul>{blockers.map((b) => <li key={b}>{b}</li>)}</ul>
                <input className="cw-select" value={mergeWaive} placeholder={t('cwWaivePlaceholder')} onChange={(e) => setMergeWaive(e.target.value)} />
              </div>
            )}
            <button
              className="text-btn press"
              disabled={busy}
              onClick={async () => {
                setBusy(true)
                const d = (await call('execDiff', run.id)) as { text: string; truncated: boolean } | null | undefined
                setBusy(false)
                setDiff(d ? d : null)
              }}
            >{t('cwShowDiff')}</button>
            {diff && (
              <>
                {diff.truncated && <p className="t-foot">{t('cwDiffTruncated')}</p>}
                <pre className="cw-pre">{diff.text}</pre>
              </>
            )}
          </article>
        )}

        {run.phase === 'meeting' && <p className="footnote" role="status">{speaker ? t('cwSpeaking', { name: agentLabel(speaker) }) : t('cwPlanning')}</p>}
        {run.block && (run.phase === 'blocked' || run.phase === 'paused') && (
          <p className="form-error" role="alert"><IWarning size={18} />{run.block.message}</p>
        )}
        {ex?.integration?.message && ex.integration.status !== 'ok' && <p className="form-error" role="alert"><IWarning size={18} />{ex.integration.message}</p>}
        {error && <p className="form-error" role="alert"><IWarning size={18} />{error}</p>}
      </main>

      {(actions.length > 0 || composer) && (
        <div className="dock">
          <div className="dock-inner">
            {actions.length > 0 && <div className="cw-actions">{actions}</div>}
            {composer && (
              <form className="composer" onSubmit={(e) => { e.preventDefault(); void submit() }}>
                <textarea
                  value={text}
                  rows={1}
                  maxLength={12000}
                  aria-label={composer.placeholder}
                  placeholder={composer.placeholder}
                  enterKeyHint="send"
                  onChange={(e) => {
                    setText(e.target.value)
                    e.target.style.height = 'auto'
                    e.target.style.height = `${e.target.scrollHeight}px`
                  }}
                />
                <button type="submit" className="send press" aria-label={t('send')} disabled={busy || !text.trim()}>
                  <IArrowUp size={20} />
                </button>
              </form>
            )}
          </div>
        </div>
      )}

      {confirm && (
        <ConfirmSheet
          title={t(confirm === 'delete' ? 'cwDelete' : 'cwCancel')}
          message={t(confirm === 'delete' ? 'cwDeleteBody' : 'cwCancelBody')}
          action={t(confirm === 'delete' ? 'cwDelete' : 'cwCancel')}
          onCancel={() => setConfirm(null)}
          onConfirm={async () => {
            const which = confirm
            setConfirm(null)
            if (which === 'cancel') void act('cancel')
            else if (await act('delete')) onDeleted()
          }}
        />
      )}
    </>
  )
}
