// Cowork 2.0 canvas: the contract between wb-core (canvas.rs) and the UI. Spec: cowork2.0-implement.md §3.
// Persisted shapes first, then runtime state that the backend owns and never writes to disk, then the IPC ops.

export type CanvasResult<T> = { ok: true; data: T } | { ok: false; code: string; message?: string; data?: unknown }

// ---------- persisted: <git common dir>/cowork/channels/<channelId>.json ----------

export interface Channel {
  schemaVersion: 1
  id: string
  name: string
  nodes: CanvasNode[]
  edges: HandoffEdge[]
  /** Newest last, truncated by the backend; Feed and phone read this */
  events: ChannelEvent[]
}

export interface CanvasNode {
  id: string
  /** Launcher id from pty:launchers (claude / codex / antigravity / custom-agent / yaml id) */
  agent: string
  /** Absent = works in the repo root (one per repo, enforced by the backend) */
  worktree?: { path: string; branch: string; baseCommit: string }
  position: { x: number; y: number }
  size: { width: number; height: number }
}

export interface HandoffEdge {
  id: string
  from: string
  to: string
  at: number
  /** The draft built at the time; not proof it was pasted or received */
  text: string
}

export type ChannelEvent =
  | { t: 'node-started' | 'node-exited'; at: number; node: string }
  | { t: 'approval'; at: number; node: string; requestId: string; status: 'pending' | 'resolved' | 'expired'; decision?: 'approved' | 'rejected' }
  | { t: 'handoff'; at: number; edge: string; from: string; to: string; text: string }
  | { t: 'merged' | 'discarded'; at: number; node: string }
  | { t: 'stop-requested' | 'stop-cleared'; at: number }

// ---------- runtime (backend-owned, not persisted) ----------

export type NodeStatus = 'creating' | 'running' | 'closing' | 'exited' | 'pending-cleanup' | 'error'

export interface NodeRuntime {
  status: NodeStatus
  /** Why creating/starting/cleanup failed; the node and its worktree stay for retry */
  error?: string
  /** PTY session id while running; subscribe with the existing pty:attach / terminalSession */
  ptyId?: string
  /** New random id per start; every approve/reject/message must carry it */
  sessionGeneration?: string
  /** Recent output only; never proof the agent has stopped */
  busy: boolean
  exitCode?: number
  /** True once the CLI shows its input prompt; paste is offered only then */
  inputReady: boolean
  /** At most one current card per node */
  approval: ApprovalRequest | null
  /** Proxy mode the session started with; differs from settings => UI says restart needed */
  proxyMode?: 'inherit' | 'custom' | 'direct'
}

export interface ApprovalFile {
  path: string
  added: number
  removed: number
  /** Present when the hook gave full text (Claude Write/Edit); absent for patches */
  before?: string
  after?: string
  /** Codex apply_patch text for this file */
  patch?: string
}

export interface ApprovalRequest {
  requestId: string
  sessionGeneration: string
  source: 'screen' | 'hook'
  createdAt: number
  expiresAt: number
  /** Screen layer: the prompt text and screen tail */
  title: string
  tail: string
  /** Only true when the backend recognized the question and its options; otherwise the UI offers "open terminal" only */
  canApprove: boolean
  canReject: boolean
  /** Hook layer details; raw is shown when parsing failed */
  hook?: { tool: string; files: ApprovalFile[]; command?: string; raw: string }
}

export interface StopState {
  /** Stage 1 requested for this repo: new nodes, pastes and app-side approvals are refused */
  requested: boolean
  at?: number
}

export interface ChannelView {
  channel: Channel
  runtime: Record<string, NodeRuntime>
  stop: StopState
}

export interface ChannelSummary {
  id: string
  name: string
  nodeCount: number
  pendingApprovals: number
  updatedAt: number
}

// ---------- worktree operations (§3.3) ----------

export interface ChangedFile {
  path: string
  status: 'added' | 'modified' | 'deleted' | 'renamed' | 'untracked' | 'ignored'
  binary?: boolean
  /** Content too large to show; the file is still listed */
  truncated?: boolean
}

export interface ChangesSummary {
  baseCommit: string
  commits: { sha: string; subject: string }[]
  committed: ChangedFile[]
  staged: ChangedFile[]
  unstaged: ChangedFile[]
  untracked: ChangedFile[]
  /** True when the branch has commits and nothing uncommitted */
  clean: boolean
}

export interface MergePreview {
  token: string
  source: { branch: string; commit: string }
  target: { branch: string; head: string }
  commits: { sha: string; subject: string }[]
  files: ChangedFile[]
}

export interface DiscardPreview {
  token: string
  unmergedCommits: { sha: string; subject: string }[]
  tracked: ChangedFile[]
  untracked: ChangedFile[]
  ignored: ChangedFile[]
}

export interface LegacyWorktree {
  path: string
  branch: string
  baseCommit?: string
  /** Only provable ones may ever be offered for cleanup */
  appOwned: boolean
  dirty: boolean
  untracked: number
  source: 'git' | 'manifest' | 'custom-root'
}

// ---------- IPC: one generic channel per op, ipcRenderer.invoke(`canvas:${op}`, req) ----------

export interface CanvasOps {
  'channels.list': { req: void; res: ChannelSummary[] }
  'channel.get': { req: { channelId: string }; res: ChannelView }
  'channel.create': { req: { name: string }; res: ChannelView }
  'channel.rename': { req: { channelId: string; name: string }; res: void }
  /** Refused while any node still has a process or a worktree */
  'channel.delete': { req: { channelId: string }; res: void }
  'node.create': { req: { channelId: string; agent: string; workspace: 'worktree' | 'root'; position: CanvasNode['position']; size: CanvasNode['size'] }; res: CanvasNode }
  /** Layout only; size change also resizes the PTY once (resize_ordered) */
  'node.layout': { req: { channelId: string; nodeId: string; position: CanvasNode['position']; size: CanvasNode['size'] }; res: void }
  'node.restart': { req: { channelId: string; nodeId: string }; res: void }
  /** Invalidates approvals, kills the PTY and waits for exit; keeps the worktree (pending-cleanup) */
  'node.close': { req: { channelId: string; nodeId: string }; res: void }
  'node.changes': { req: { channelId: string; nodeId: string }; res: ChangesSummary }
  'node.commitAll': { req: { channelId: string; nodeId: string; files: string[]; message: string }; res: { commit: string } }
  'node.mergePreview': { req: { channelId: string; nodeId: string; target: string }; res: MergePreview }
  'node.merge': { req: { channelId: string; nodeId: string; token: string }; res: { commit: string } }
  /** Only merged, clean, exited, app-owned worktrees; removes the node */
  'node.cleanup': { req: { channelId: string; nodeId: string }; res: void }
  'node.discardPreview': { req: { channelId: string; nodeId: string }; res: DiscardPreview }
  'node.discard': { req: { channelId: string; nodeId: string; token: string }; res: void }
  'node.approve': { req: { channelId: string; nodeId: string; requestId: string; sessionGeneration: string }; res: void }
  'node.reject': { req: { channelId: string; nodeId: string; requestId: string; sessionGeneration: string; reason?: string }; res: void }
  /** Bracketed paste without Enter; refused unless running, inputReady, no pending approval, not stopped */
  'node.message': { req: { channelId: string; nodeId: string; sessionGeneration: string; text: string }; res: void }
  /** Builds the draft from git (branch, commits, files, uncommitted state) + note; never writes to a PTY */
  'handoff.create': { req: { channelId: string; from: string; to: string; note: string }; res: HandoffEdge }
  'edge.remove': { req: { channelId: string; edgeId: string }; res: void }
  /** Stage 1 for every Cowork node of this repo, all channels and windows: one Esc each */
  'stop': { req: void; res: StopState }
  /** Stage 2, only while stopped: kill_pty and confirm exit */
  'stop.killAll': { req: void; res: { failed: { channelId: string; nodeId: string; error: string }[] } }
  'stop.clear': { req: void; res: StopState }
  'legacy.inventory': { req: void; res: LegacyWorktree[] }
}

export type CanvasOp = keyof CanvasOps

/** Pushed with send_all after any change; the UI replaces its copy of that channel */
export const CANVAS_UPDATE_EVENT = 'canvas:update'
export interface CanvasUpdate {
  repoId: string
  channelId: string
  /** Absent when the channel was deleted */
  view?: ChannelView
}

/** Error codes the UI branches on */
export type CanvasErrorCode =
  | 'not-found'
  | 'invalid'
  | 'stopped'
  | 'stale-request'
  | 'stale-preview'
  | 'not-ready'
  | 'busy'
  | 'dirty'
  | 'conflict'
  | 'root-node-exists'
  | 'not-owned'
  | 'unknown-schema'
