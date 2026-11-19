// 模型與強度（P7）：跟 OpenBot 一樣，輸入框下面一顆「模型 · 強度 ▾」按鈕，按下去是下拉選單。每個 Bot 各自的設定，
// 「預設」＝CLI 自己設定檔裡的。清單來自後端 agent.models：Claude 是別名、Codex 讀 `codex debug models`（每個模型
// 有自己的強度）、agy 讀 `agy models`（模型 ID 已含強度，所以沒有強度那一段）。
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from '@/i18n'
import type { AgentModels } from '../../../../shared/canvas'
import type { CanvasCall } from './types'

const cache = new Map<string, Promise<AgentModels | undefined>>()

export function useModels(call: CanvasCall, agent: string, enabled = true): AgentModels | null {
  const [m, setM] = useState<AgentModels | null>(null)
  useEffect(() => {
    setM(null)
    if (!enabled || !agent) return
    let alive = true
    if (!cache.has(agent)) cache.set(agent, call('agent.models', { agent }))
    void cache.get(agent)!.then((v) => {
      if (!v) cache.delete(agent)
      if (alive) setM(v ?? null)
    })
    return () => {
      alive = false
    }
  }, [call, agent, enabled])
  return m
}

/** "sonnet · high" for a chip; empty when both are the CLI's default */
export const modelLabel = (model?: string, effort?: string): string => [model, effort].filter(Boolean).join(' · ')

/** What the dropdown's button says: the model's own name and the effort in words ("Sonnet (latest) · High") */
export function useModelText(call: CanvasCall, agent: string, model: string, effort: string): string {
  const { t } = useTranslation()
  const m = useModels(call, agent)
  const picked = m?.models.find((x) => x.id === model)
  const name = model ? picked?.name ?? model : t('cw.modelDefault')
  const efforts = picked?.efforts?.length ? picked.efforts : m?.efforts ?? []
  return effort && efforts.length ? `${name} · ${t(`cw.effort_${effort}`)}` : name
}

export default function ModelPicker({
  call,
  agent,
  model,
  effort,
  onChange
}: {
  call: CanvasCall
  agent: string
  model: string
  effort: string
  onChange: (model: string, effort: string) => void
}): JSX.Element | null {
  const { t } = useTranslation()
  const m = useModels(call, agent)
  const [open, setOpen] = useState(false)
  const [other, setOther] = useState('')
  /** Opens toward the side with room: rightward unless that would leave the window */
  const [toLeft, setToLeft] = useState(false)
  const box = useRef<HTMLSpanElement>(null)
  const chipText = useModelText(call, agent, model, effort)

  useEffect(() => {
    if (!open) return
    const away = (e: MouseEvent): void => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false)
    }
    const esc = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', away)
    document.addEventListener('keydown', esc)
    return () => {
      document.removeEventListener('mousedown', away)
      document.removeEventListener('keydown', esc)
    }
  }, [open])

  if (!m) return null
  const picked = m.models.find((x) => x.id === model)
  // A model with its own levels (Codex) narrows the list; agy's ids already carry the effort
  const efforts = picked?.efforts?.length ? picked.efforts : m.efforts
  const choose = (next: string): void => {
    const n = m.models.find((x) => x.id === next)
    onChange(next, n?.efforts?.length && effort && !n.efforts.includes(effort) ? '' : effort)
  }

  return (
    <span className="cw-model" ref={box}>
      <button type="button" className="cw-chip cw-model-chip" aria-haspopup="dialog" aria-expanded={open} title={t('cw.model')} onClick={() => {
          const r = box.current?.getBoundingClientRect()
          setToLeft(!!r && r.left + 356 > window.innerWidth)
          setOpen(!open)
        }}>
        {chipText} <span aria-hidden="true">▾</span>
      </button>
      {open && (
        <div className={`cw-model-menu ${toLeft ? 'to-left' : ''}`} role="dialog" aria-label={t('cw.model')}>
          <div className="cw-model-head">{t('cw.model')}</div>
          <div role="radiogroup" aria-label={t('cw.model')} className="cw-model-list">
            {[{ id: '', name: t('cw.modelDefault') }, ...m.models].map((x) => (
              <button
                key={x.id || 'default'}
                type="button"
                role="radio"
                aria-checked={model === x.id}
                className={`cw-model-item ${model === x.id ? 'on' : ''}`}
                onClick={() => {
                  choose(x.id)
                  if (!efforts.length) setOpen(false)
                }}
              >
                <span>{x.name}</span>
                {x.id && x.name !== x.id && <span className="cw-muted cw-mono">{x.id}</span>}
              </button>
            ))}
            {model && !picked && (
              <button type="button" role="radio" aria-checked className="cw-model-item on">
                <span className="cw-mono">{model}</span>
              </button>
            )}
          </div>
          <form
            className="cw-row cw-model-other"
            onSubmit={(e) => {
              e.preventDefault()
              if (other.trim()) choose(other.trim())
              setOther('')
            }}
          >
            <input className="cw-input" value={other} maxLength={100} placeholder={t('cw.modelOther')} aria-label={t('cw.modelOther')} onChange={(e) => setOther(e.target.value)} />
            <button type="submit" className="cw-btn" disabled={!other.trim()}>{t('cw.modelUse')}</button>
          </form>
          {efforts.length > 0 && (
            <>
              <div className="cw-model-head">{t('cw.effort')}</div>
              <div className="cw-seg cw-model-efforts" role="radiogroup" aria-label={t('cw.effort')}>
                {['', ...efforts].map((x) => (
                  <button key={x || 'default'} type="button" role="radio" aria-checked={effort === x} className={effort === x ? 'on' : ''} onClick={() => onChange(model, x)}>
                    {x ? t(`cw.effort_${x}`) : t('cw.effortDefault')}
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
      )}
    </span>
  )
}
