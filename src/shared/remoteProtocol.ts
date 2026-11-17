// 遠端控制（手機 ↔ 桌面 Remote Bridge）的 WebSocket 訊息格式。
// 桌面 main（src/main/remote/server.ts）與手機端（src/renderer/remote/）共用這份型別。
// 每則訊息都是 JSON，以 `t` 區分種類。
import type { BgTask, FsEntry } from '../preload/index'
import type { CanvasOp, CanvasResult, CanvasUpdate } from './canvas'

export interface RemoteVibeStatus {
  workspace: string
  agentBusy: boolean
  agentCount: number
  devUrl: string | null
  changedFiles: string[]
}

export type RemoteFileEntry = FsEntry

export interface RemoteSession {
  id: string
  title: string
  /** 啟動用的 key：claude / codex / antigravity / powershell / cmd 或自訂 launcher id */
  launcherKey: string
  windowId: number
  workspaceName: string
  startTime: number
  cols: number
  rows: number
  needsApproval: boolean
  lastOutputAt: number
  /** 近期還在吐輸出＝執行中。由桌面判定，手機不要拿自己的時鐘去比 lastOutputAt */
  busy: boolean
  /** 這個終端印出的本機 dev server port；有值手機才顯示「預覽」分頁 */
  devPort?: number | null
  /** 等待審批時附上畫面尾段（已去 ANSI），首頁卡片用來預覽問題 */
  approvalTail?: string
}

export interface RemoteLauncher {
  key: string
  title: string
  kind: 'agent' | 'shell' | 'custom'
}

export interface RemoteWindow {
  id: number
  workspaceName: string
  workspace: string
  launchers: RemoteLauncher[]
}

/** 桌面上「最近開啟過、但目前沒有視窗」的工作區：手機可以請桌面開起來 */
export interface RemoteWorkspaceOption {
  path: string
  name: string
}

export interface RemoteGitFile {
  path: string
  index: string
  workingDir: string
}

export type ClientMessage =
  | { t: 'auth'; token: string }
  /** 配對（跨 origin 的 fetch 會被 CORS 擋，所以配對也走 WebSocket） */
  | { t: 'pairRequest'; code: string; name: string }
  | { t: 'attach'; id: string }
  | { t: 'detach'; id: string }
  | { t: 'input'; id: string; data: string }
  | { t: 'resize'; id: string; cols: number; rows: number }
  | { t: 'spawn'; windowId: number; launcherKey: string }
  | { t: 'kill'; id: string }
  | { t: 'handoff'; windowId: number }
  | { t: 'git'; windowId: number }
  | { t: 'status'; windowId: number }
  | { t: 'files'; windowId: number; path: string }
  | { t: 'file'; windowId: number; path: string }
  /** 請桌面開啟（或聚焦）某個最近用過的工作區，開起來才有視窗可以派 agent */
  | { t: 'openWorkspace'; path: string }
  /** 要一個預覽網址（桌面會發一次性 ticket，手機把它放進 iframe） */
  | { t: 'preview'; id: string }
  | { t: 'visibility'; visible: boolean }
  /** 手機切換 Bypass 模式（只影響之後啟動的 agent） */
  | { t: 'setBypass'; enabled: boolean }
  /** Cowork 2.0 畫布操作：只限 PHONE_CANVAS_OPS，req 與桌面 window.api.canvas.call 相同，工作區由 windowId 決定 */
  | { t: 'canvas'; reqId: number; windowId: number; op: CanvasOp; req: unknown }
  | { t: 'ping' }

export type ServerMessage =
  | { t: 'paired'; token: string; deviceId: string }
  | { t: 'authed'; deviceId: string; deviceName: string; hostName: string }
  | {
      t: 'state'
      sessions: RemoteSession[]
      windows: RemoteWindow[]
      /** 最近開啟過但目前沒視窗的工作區，手機可以挑一個請桌面開起來 */
      workspaces: RemoteWorkspaceOption[]
      /** Bypass 模式開著時，手機要明顯警示：agent 不會再問你就直接執行 */
      bypass: boolean
    }
  | { t: 'snapshot'; id: string; data: string; cols: number; rows: number }
  | { t: 'data'; id: string; d: string }
  | { t: 'resized'; id: string; cols: number; rows: number }
  | { t: 'exit'; id: string; code: number; title: string }
  /** 執行中／閒置翻轉時才送一則（不是每次輸出都送） */
  | { t: 'activity'; id: string; busy: boolean }
  | { t: 'approval'; id: string; title: string; tail: string }
  | { t: 'spawned'; id: string }
  | { t: 'handoff'; windowId: number; text: string | null }
  | { t: 'preview'; id: string; url: string | null; error?: string }
  | { t: 'status'; windowId: number; status: RemoteVibeStatus | null; bgTasks: BgTask[]; error?: string }
  | { t: 'files'; windowId: number; path: string; entries: RemoteFileEntry[]; error?: string }
  | { t: 'file'; windowId: number; path: string; kind: 'markdown' | 'html' | 'pdf' | 'text'; text: string | null; url: string | null; error?: string }
  | {
      t: 'git'
      windowId: number
      branch: string | null
      ahead: number
      behind: number
      files: RemoteGitFile[]
      error?: string
    }
  | { t: 'canvas'; reqId: number; result: CanvasResult<unknown> }
  /** 某個 Channel 有變化；審批卡只帶檔名與增減行數，不含程式碼內容 */
  | { t: 'canvasUpdate'; update: CanvasUpdate }
  | { t: 'error'; message: string }
  | { t: 'pong' }

/**
 * 終端狀態：一律用桌面判定並廣播的 busy（state 帶初值，activity 帶翻轉），
 * 不可以用「現在時間 − lastOutputAt」——那是拿手機時鐘比桌面時間，差一點就永遠顯示錯，
 * 而且 state 不是每次有輸出都重送，會卡在舊值。
 */
export function sessionStatus(
  s: Pick<RemoteSession, 'id' | 'needsApproval' | 'busy'>,
  live: Record<string, boolean> = {}
): 'waiting' | 'running' | 'idle' {
  if (s.needsApproval) return 'waiting'
  return (live[s.id] ?? s.busy) ? 'running' : 'idle'
}

/** HTTP API（配對與推播訂閱），不走 WebSocket */
export interface PairRequest {
  code: string
  name: string
}
export interface PairResponse {
  token: string
  deviceId: string
}

/** 內建 CLI 的顯示名稱；自訂 launcher 用 yaml 裡的 name */
export const BUILTIN_LAUNCHERS: RemoteLauncher[] = [
  { key: 'claude', title: 'Claude Code', kind: 'agent' },
  { key: 'codex', title: 'Codex', kind: 'agent' },
  { key: 'antigravity', title: 'Antigravity', kind: 'agent' },
  { key: 'powershell', title: 'PowerShell', kind: 'shell' },
  { key: 'cmd', title: 'Command Prompt', kind: 'shell' }
]

/** 手機可以呼叫的畫布操作（§3.9）；建立／刪除節點、合併、捨棄、全部結束、解除停止、開始團隊與核准合併請求只在桌面。與 wb-core PHONE_CANVAS_OPS 相同 */
export const PHONE_CANVAS_OPS: ReadonlySet<CanvasOp> = new Set<CanvasOp>(['channels.list', 'channel.get', 'node.approve', 'node.reject', 'node.message', 'stop', 'relay.approve', 'relay.reject', 'team.pause'])

/** 這些操作要留下稽核紀錄（只記識別碼與長度，不含訊息或理由內容）；白名單外的嘗試一律也記 */
export const PHONE_AUDITED_OPS: ReadonlySet<CanvasOp> = new Set<CanvasOp>(['node.approve', 'node.reject', 'node.message', 'stop', 'relay.approve', 'relay.reject', 'team.pause'])
