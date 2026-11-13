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
  /** Teams (cowork-team-implement.md §2); absent in files from before teams */
  teams?: Team[]
  /** What each node works on, keyed by node id; set by assign_task, report or the user */
  tasks?: Record<string, NodeTask>
}

export type RelayMode = 'approve' | 'lead-auto' | 'auto'

export interface Team {
  id: string
  name: string
  goal: string
  /** One of memberIds */
  leadId: string
  /** Includes the lead; a node is in at most one team */
  memberIds: string[]
  /** approve: every agent message waits for the user; lead-auto: only members' do; auto: none do, and the lead merges members into its own worktree itself */
  relay: RelayMode
  status: 'draft' | 'running' | 'paused' | 'done' | 'failed'
  /** Why it stopped running: the user, the stop, or a cap (only in lead-auto and auto) */
  pausedReason?: string
  bounds: { x: number; y: number; width: number; height: number }
  limits: { maxMessages: number; perMemberPer10Min: number }
  startedAt?: number
  messageCount: number
}

export interface NodeTask {
  title: string
  status: 'idle' | 'working' | 'blocked' | 'done' | 'failed'
  summary?: string
  updatedAt: number
  source: 'assign' | 'report' | 'user'
}

export interface TeamMessage {
  id: string
  teamId: string
  /** A node id, or 'app' / 'user' */
  from: string
  to: string
  kind: 'task' | 'message' | 'report' | 'system'
  /** Untrusted when from an agent; never shown as HTML */
  text: string
  at: number
  state: 'pending-approval' | 'queued' | 'notified' | 'read' | 'rejected'
}

/** A lead's request to merge a member's branch into its own worktree, waiting for the user */
export interface TeamMergeRequest {
  id: string
  team: string
  lead: string
  member: string
  /** `<short sha> <subject>` */
  commits: string[]
  at: number
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
  | { t: 'team-started' | 'team-resumed'; at: number; team: string }
  | { t: 'team-paused'; at: number; team: string; reason: string }
  | { t: 'team-done'; at: number; team: string; by: 'lead' | 'user' }
  /** Who, to whom and how long; the text lives in the messages file only */
  | { t: 'message'; at: number; team: string; id: string; from: string; to: string; kind: TeamMessage['kind']; len: number; held: boolean }
  | { t: 'relay'; at: number; id: string; decision: 'approved' | 'rejected'; via?: 'phone' }
  | { t: 'task'; at: number; node: string; status: NodeTask['status']; team?: string }
  | { t: 'team-merge'; at: number; team: string; member: string; decision: 'requested' | 'approved' | 'rejected' | 'auto'; id?: string }

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
  /** Present when the hook gave full text (Write/Edit); a new file has no `before` */
  before?: string | null
  after?: string | null
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
  /** Hook layer details; raw is shown when parsing failed. The phone never gets raw, command or file contents. */
  hook?: { tool: string; files: ApprovalFile[]; command?: string | null; raw?: string } | null
}

export interface StopState {
  /** Stage 1 requested for this repo: new nodes, pastes and app-side approvals are refused */
  requested: boolean
  at?: number
}

export interface ChannelView {
  channel: Channel
  runtime: Record<string, NodeRuntime & NodeTeamRuntime>
  stop: StopState
  teams: Team[]
  /** Agent messages waiting for the user (relay cards) */
  relays: TeamMessage[]
  mergeRequests: TeamMergeRequest[]
  /** The channel's latest team messages, newest first, at most 100 */
  messages: TeamMessage[]
}

export interface NodeTeamRuntime {
  teamId?: string
  role?: 'lead' | 'member'
  task?: NodeTask | null
  /** Team messages queued or notified but not read yet */
  unread: number
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
  /** git status letter: A M D R T, `?` untracked, `index-flags` hidden by assume-unchanged/skip-worktree */
  status: string
  /** The diff (or untracked content), secrets masked; empty when binary or truncated */
  text: string
  binary: boolean
  /** Content too large to show; the file is still listed */
  truncated: boolean
}

export interface ChangesSummary {
  baseCommit: string
  /** `<sha> <subject>`, newest first */
  commits: string[]
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
  commits: string[]
  files: { path: string; status: string }[]
}

export interface DiscardPreview {
  token: string
  unmergedCommits: string[]
  changes: Omit<ChangesSummary, 'baseCommit' | 'clean'>
  /** Ignored files with data in the worktree; they go too */
  ignored: string[]
}

export interface LegacyWorktree {
  path: string
  branch: string
  baseCommit: string | null
  /** Only provable ones may ever be offered for cleanup */
  ownershipProvable: boolean
  dirty: string[]
  untracked: string[]
  ignored: string[]
  error: string | null
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
  // ---------- teams (cowork-team-implement.md) ----------
  'team.create': { req: { channelId: string; name: string; goal: string; leadId: string; memberIds: string[]; relay: RelayMode; bounds: Team['bounds'] }; res: Team }
  /** Members, lead and goal only while not running; name, relay, limits and bounds any time */
  'team.update': { req: { channelId: string; teamId: string; name?: string; goal?: string; leadId?: string; memberIds?: string[]; relay?: RelayMode; limits?: Team['limits']; bounds?: Team['bounds'] }; res: void }
  'team.delete': { req: { channelId: string; teamId: string }; res: void }
  /** Sends the kickoff to the lead the first time (or after done); refused while stopped */
  'team.start': { req: { channelId: string; teamId: string }; res: void }
  'team.resume': { req: { channelId: string; teamId: string }; res: void }
  'team.pause': { req: { channelId: string; teamId: string }; res: void }
  'team.finish': { req: { channelId: string; teamId: string }; res: void }
  /** Newest first, at most 200; teamId empty = the whole channel */
  'team.messages': { req: { channelId: string; teamId: string }; res: TeamMessage[] }
  'relay.approve': { req: { channelId: string; messageId: string }; res: void }
  'relay.reject': { req: { channelId: string; messageId: string }; res: void }
  'teammerge.approve': { req: { channelId: string; requestId: string }; res: void }
  'teammerge.reject': { req: { channelId: string; requestId: string }; res: void }
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
  /** A team tool or relay refused because the team is not running (paused by the user, the stop or a cap) */
  | 'paused'
  /** Not for this caller: the phone, a member using a lead tool, or a route the team does not allow */
  | 'not-allowed'
