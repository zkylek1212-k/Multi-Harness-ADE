// 畫布上的終端節點（§2.3、§3.8）：就是一個既有的 PTY session，換一種排版。
// 這裡只建立／休眠／銷毀「呈現」；行程屬於後端，關閉節點要明確按鈕送 node.close，卸下元件絕不 kill。
import { memo, useEffect, useRef, useState } from 'react'
import { Handle, NodeResizer, Position, type Node, type NodeProps } from '@xyflow/react'
import { useWorkbench } from '@/store'
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import { createTerminalSession, xtermTheme, type TerminalSession } from '../terminal/terminalSession'
import type { CanvasNode, NodeRuntime, NodeTeamRuntime } from '../../../../shared/canvas'
import { FOCUS_EVENT, type CanvasCall } from './CanvasView'

export type TerminalNodeData = {
  node: CanvasNode
  rt: (NodeRuntime & Partial<NodeTeamRuntime>) | undefined
  channelId: string
  name: string
  mark: Parameters<typeof AgentMark>[0]['agent']
  zoom: number
  stopped: boolean
  call: CanvasCall
}

/** 低於這個縮放就看不清字，改成靜態樣子並退訂畫面資料（休眠） */
const SLEEP_ZOOM = 0.5

function TerminalNode({ data, selected }: NodeProps<Node<TerminalNodeData>>): JSX.Element {
  const { t } = useTranslation()
  const { theme } = useWorkbench()
  const { node, rt, channelId, call } = data
  const host = useRef<HTMLDivElement>(null)
  const session = useRef<TerminalSession | null>(null)
  const [busyAction, setBusyAction] = useState(false)
  const ptyId = rt?.status === 'running' ? rt.ptyId : undefined
  const asleep = data.zoom < SLEEP_ZOOM

  useEffect(() => {
    if (!ptyId || !host.current) return
    const s = createTerminalSession({
      theme: xtermTheme(theme),
      zoomCompensation: true,
      minimumContrastRatio: theme.startsWith('light') ? 4.5 : 1,
      unsubscribe: (id) => window.api.pty.detach(id)
    })
    session.current = s
    s.attach(host.current)
    s.start({ ptyId })
    return () => {
      session.current = null
      // ponytail: unmount does not detach — IPC calls run concurrently, so a quick remount's attach could be undone by a
      // late detach. An off-screen node keeps receiving bytes it does not draw; sleep() (zoomed out) does detach, safely
      // serialized inside the session. Upgrade: a per-pty attach/detach queue if 8+ node memory shows it matters.
      s.dispose()
    }
    // theme 另外套用，不重建 session
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ptyId])

  useEffect(() => {
    const s = session.current
    if (!s) return
    s.term.options.theme = xtermTheme(theme)
  }, [theme])

  useEffect(() => {
    const s = session.current
    if (!s) return
    if (asleep) s.sleep()
    else s.wake()
  }, [asleep])

  // Take Over and "answer in the terminal" hand the keyboard to this node
  useEffect(() => {
    const onFocus = (e: Event): void => {
      if ((e as CustomEvent<string>).detail === node.id) session.current?.focus()
    }
    window.addEventListener(FOCUS_EVENT, onFocus)
    return () => window.removeEventListener(FOCUS_EVENT, onFocus)
  }, [node.id])

  const act = async (fn: () => Promise<unknown>): Promise<void> => {
    setBusyAction(true)
    await fn()
    setBusyAction(false)
  }
  const ids = { channelId, nodeId: node.id }
  const a = rt?.approval
  const status = !rt ? 'exited' : a ? 'waiting' : rt.status === 'running' ? (rt.busy ? 'running' : 'idle') : rt.status

  return (
    <div className={`cv-node st-${status} ${selected ? 'selected' : ''}`}>
      <NodeResizer
        isVisible={selected}
        minWidth={200}
        minHeight={120}
        onResizeEnd={(_, p) =>
          void call('node.layout', { ...ids, position: { x: Math.round(p.x), y: Math.round(p.y) }, size: { width: Math.round(p.width), height: Math.round(p.height) } })
        }
      />
      <Handle type="target" position={Position.Left} className="cv-handle" />
      <div className="cv-node-head">
        <span className="cv-node-prompt" aria-hidden="true">&gt;_</span>
        <AgentMark agent={data.mark} size={14} />
        <span className="cv-node-title">{data.name}</span>
        {rt?.role === 'lead' && <span className="cv-pill lead" title={t('canvas.role_lead')}>★</span>}
        {(rt?.unread ?? 0) > 0 && <span className="cv-badge" title={t('canvas.unread', { n: rt?.unread ?? 0 })}>{rt?.unread}</span>}
        <span className="cv-node-branch" title={node.worktree?.path || t('canvas.repoRoot')}>
          {node.worktree ? node.worktree.branch : t('canvas.repoRoot')}
        </span>
        <span className={`cv-dot st-${status}`} title={t(`canvas.status_${status.replace('-', '_')}`)} />
        <span className="cv-node-status">{t(`canvas.status_${status.replace('-', '_')}`)}</span>
        {rt?.status === 'running' ? (
          <button className="cv-icon-btn nodrag" disabled={busyAction} title={t('canvas.close')} onClick={() => window.confirm(t('canvas.closeConfirm')) && void act(() => call('node.close', ids))}>
            ✕
          </button>
        ) : (
          rt?.status !== 'creating' &&
          rt?.status !== 'closing' && (
            <button className="cv-icon-btn nodrag" disabled={busyAction || data.stopped} title={t('canvas.restart')} onClick={() => void act(() => call('node.restart', ids))}>
              ↻
            </button>
          )
        )}
      </div>
      <div className="cv-node-body nodrag nowheel nopan">
        {ptyId ? (
          <>
            <div ref={host} className="cv-term" hidden={asleep} />
            {asleep && <div className="cv-term-asleep">{t('canvas.zoomIn')}</div>}
          </>
        ) : (
          <div className="cv-term-ended">
            {rt?.error ? <span className="cv-error">{rt.error}</span> : <span>{t(`canvas.status_${status.replace('-', '_')}`)}</span>}
            {rt?.exitCode !== undefined && rt?.exitCode !== null && <span className="cv-muted">exit {rt.exitCode}</span>}
          </div>
        )}
      </div>
      <Handle type="source" position={Position.Right} className="cv-handle" />
    </div>
  )
}

export default memo(TerminalNode)
