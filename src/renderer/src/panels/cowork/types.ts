import type AgentMark from '@/components/AgentMark'
import type { CliLauncher } from '../../../../preload/index'
import type { CanvasOp, CanvasOps } from '../../../../shared/canvas'

/** Resolves to the op's data, or undefined when it failed (the error is already shown) */
export type CanvasCall = <K extends CanvasOp>(op: K, req?: CanvasOps[K]['req']) => Promise<CanvasOps[K]['res'] | undefined>

// pty:launchers 只回 agents/*.yaml 與 Custom Agent；內建三家在這裡補上
export const BUILTIN_AGENTS: CliLauncher[] = [
  { id: 'claude', name: 'Claude Code', cli: 'claude', command: 'claude', args: [] },
  { id: 'codex', name: 'Codex', cli: 'codex', command: 'codex', args: [] },
  { id: 'antigravity', name: 'agy', cli: 'antigravity', command: 'agy', args: [] }
]

/** Agents that can run as a chat node (wb-core canvas/chat.rs `protocol`); the custom agent depends on its settings */
export const CHAT_AGENTS = ['claude', 'codex', 'antigravity']

type MarkId = Parameters<typeof AgentMark>[0]['agent']
const MARKS = ['claude', 'codex', 'antigravity', 'powershell', 'cmd', 'bash', 'custom-agent']
/** AgentMark 只認得內建 CLI；yaml launcher 用它實際跑的 CLI 的圖示 */
export const markOf = (cli: string): MarkId => (MARKS.includes(cli) ? cli : 'shell') as MarkId

export function remember(key: string, value: string | null): void {
  try {
    if (value) localStorage.setItem(key, value)
    else localStorage.removeItem(key)
  } catch {
    /* 忽略 */
  }
}

export function recall(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000))
  if (s < 60) return 'now'
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86400) return `${Math.floor(s / 3600)}h`
  return `${Math.floor(s / 86400)}d`
}
