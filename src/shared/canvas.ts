// Cowork 2.0 canvas: the contract between wb-core (canvas.rs) and the UI. Spec: cowork2.0-implement.md §3.
// Persisted shapes first, then runtime state that the backend owns and never writes to disk, then the IPC ops.

import type { Transcript } from './agui'

export type CanvasResult<T> = { ok: true; data: T } | { ok: false; code: string; message?: string; data?: unknown }

// ---------- persisted: <git common dir>/cowork/channels/<channelId>.json ----------

export interface Channel {
  schemaVersion: 1
  id: string
  name: string
  nodes: CanvasNode[]
  /** Newest last, truncated by the backend; Feed and phone read this */
  events: ChannelEvent[]
  /** The channel's group: at most one entry, kept by the backend (id "main"). A channel is a group of agents. */
  teams?: Team[]
  /** What each node works on, keyed by node id; set by assign_task, report or the user */
  tasks?: Record<string, NodeTask>
  /** Tasks the lead posted for any member to claim (cowork-agui-implement.md §6.8) */
  board?: BoardTask[]
}

export interface BoardTask {
  id: string
  title: string
  details: string
  doneWhen: string
  /** returned: nobody claimed it in 5 minutes; the lead decides */
  status: 'open' | 'claimed' | 'returned'
  claimedBy?: string
  at: number
}

/** cowork-agui-implement.md §11: daily/weekly at a local HH:MM (day 0 = Monday), every N minutes (15 or more), or new commits on a branch */
export type RoutineSchedule = { kind: 'daily'; time: string } | { kind: 'weekly'; day: number; time: string } | { kind: 'every'; minutes: number } | { kind: 'commit'; branch: string }

export interface Routine {
  /** '' when new; the backend assigns it */
  id: string
  name: string
  /** The first message of each run's conversation */
  prompt: string
  agent: string
  /** Set by the backend: the repository the routine was made in */
  repo: string
  /** report: read-only, the finding goes to Needs you; change: may change code in its own worktree */
  scope: 'report' | 'change'
  schedule: RoutineSchedule
  model?: string
  effort?: string
  enabled: boolean
  nextAt: number
  nextAtText?: string
  failures: number
  lastRun?: { channel: string; at: number; status: 'running' | 'ok' | 'failed' | 'stopped'; summary?: string; seen: boolean }
}

/** A colleague invited into one channel (§13) */
export interface GuestInfo {
  id: string
  name: string
  createdAt: number
  lastSeenAt: number
  online: boolean
}

export interface GuestInvite {
  /** One use, expires after 7 days */
  url: string
  expiresAt: number
  /** Where a colleague's browser gets this computer's certificate, so it stops warning */
  caUrl?: string
}

/** An ask_person question waiting for the user (§6.4) */
export interface PersonAsk {
  id: string
  node: string
  question: string
  choices: string[]
  at: number
}

export type RelayMode = 'approve' | 'lead-auto' | 'auto'

/** A channel's group. Its members are the channel's nodes; the backend keeps it in step, nothing creates or starts it. */
export interface Team {
  /** Always "main" */
  id: string
  /** The channel's name */
  name: string
  /** Unused since the channel became the group */
  goal: string
  /** One of memberIds; the first agent node unless the user picked another */
  leadId: string
  /** Every node of the channel */
  memberIds: string[]
  /** approve: every agent message waits for the user; lead-auto: only members' do; auto: none do, and the lead merges members into its own worktree itself */
  relay: RelayMode
  status: 'running' | 'paused'
  /** Why it stopped running: the user, the stop, or a cap (only in lead-auto and auto) */
  pausedReason?: string
  /** Members may message each other directly (cowork-agui-implement.md §8.3) */
  membersTalk?: boolean
  /** The user's messages without an @ go to every Bot; each answers PASS when it is not for its role (§6.8) */
  freeTalk?: boolean
  limits: { maxMessages: number; perMemberPer10Min: number }
  startedAt?: number
  messageCount: number
}

export interface NodeTask {
  title: string
  status: 'idle' | 'working' | 'blocked' | 'done' | 'failed'
  summary?: string
  updatedAt: number
  /** Who gave the task: the lead (reports go back to the lead) or the user (reports go to the chat) */
  source: 'assign' | 'user'
  /** A member's suggestion for the shared memory; the lead writes it (§12.1) */
  memory?: string
}

export interface TeamMessage {
  id: string
  teamId: string
  /** A node id, 'user' (the chat box) or 'app' */
  from: string
  /** A node id, or 'user' for an agent's report into the chat */
  to: string
  /** free: the user's message to every Bot in free talk */
  kind: 'task' | 'message' | 'report' | 'system' | 'free'
  /** Untrusted when from an agent; never shown as HTML */
  text: string
  at: number
  state: 'pending-approval' | 'queued' | 'notified' | 'read' | 'rejected'
  /** One free-talk message to several Bots shares this id */
  group?: string
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
  /** Absent = terminal */
  mode?: NodeMode
  /** The CLI's own session id (chat nodes), for resume */
  session?: string
  /** What this Bot does in the group; claiming and PASS go by it */
  role?: string
  /** A report-only routine's Bot: the CLI's read-only mode */
  readOnly?: boolean
  /** Unset = the CLI's own default (P7) */
  model?: string
  effort?: string
}

/** What a Bot can run with (agent.models): suggestions, any name the CLI knows also works */
export interface AgentModels {
  models: { id: string; name: string; efforts?: string[] }[]
  /** Empty: the model id carries the effort (agy) */
  efforts: string[]
}

export type ChannelEvent =
  | { t: 'node-started' | 'node-exited'; at: number; node: string }
  | { t: 'approval'; at: number; node: string; requestId: string; status: 'pending' | 'resolved' | 'expired'; decision?: 'approved' | 'rejected' }
  | { t: 'handoff'; at: number; edge: string; from: string; to: string; text: string }
  | { t: 'merged' | 'discarded'; at: number; node: string }
  | { t: 'stop-requested' | 'stop-cleared'; at: number }
  | { t: 'team-resumed'; at: number; team: string }
  | { t: 'team-paused'; at: number; team: string; reason: string }
  /** Who, to whom and how long; the text lives in the messages file only */
  | { t: 'message'; at: number; team: string; id: string; from: string; to: string; kind: TeamMessage['kind']; len: number; held: boolean }
  | { t: 'relay'; at: number; id: string; decision: 'approved' | 'rejected'; via?: 'phone' }
  | { t: 'task'; at: number; node: string; status: NodeTask['status']; team?: string }
  | { t: 'team-merge'; at: number; team: string; member: string; decision: 'requested' | 'approved' | 'rejected' | 'auto'; id?: string }
  | { t: 'board'; at: number; id: string; status: 'open' | 'claimed'; node?: string }
  | { t: 'ask'; at: number; node: string; id: string }

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
  /** A chat node is in the middle of a turn */
  inRun?: boolean
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
  /** ask_person questions of running sessions */
  asks?: PersonAsk[]
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
  /** The conversation's own agent: its first node */
  lead: { nodeId: string; agent: string; mode: NodeMode; status: NodeStatus; inRun: boolean; branch?: string } | null
}

/** chat: headless CLI behind AG-UI (cowork-agui-implement.md); terminal: the CLI's own TUI in a PTY */
export type NodeMode = 'chat' | 'terminal'

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
  'node.create': { req: { channelId: string; agent: string; workspace: 'worktree' | 'root'; mode?: NodeMode; role?: string; model?: string; effort?: string }; res: CanvasNode }
  'agent.models': { req: { agent: string }; res: AgentModels }
  /** A chat Bot restarts on the same session with it (between turns); a terminal one takes it at its next restart */
  'node.model': { req: { channelId: string; nodeId: string; model: string; effort: string }; res: { restarted: boolean } }
  'node.role': { req: { channelId: string; nodeId: string; role: string }; res: void }
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
  /** Stage 1 for every Cowork node of this repo, all channels and windows: one Esc each */
  'stop': { req: void; res: StopState }
  /** Stage 2, only while stopped: kill_pty and confirm exit */
  'stop.killAll': { req: void; res: { failed: { channelId: string; nodeId: string; error: string }[] } }
  'stop.clear': { req: void; res: StopState }
  'legacy.inventory': { req: void; res: LegacyWorktree[] }
  /** The last lines on a node's screen, for the Take Over panel */
  'node.screen': { req: { channelId: string; nodeId: string }; res: string }
  // ---------- groups (cowork-team-implement.md): the channel is the group ----------
  /** The user's own message: a chat node's next turn, or typed into a terminal and sent. `to` defaults to the lead (every Bot in
   *  free talk). Resumes a paused group. */
  'chat.send': { req: { channelId: string; to?: string; text: string }; res: void }
  'node.takeControl': { req: { channelId: string; nodeId: string }; res: void }
  'node.handBack': { req: { channelId: string; nodeId: string }; res: void }
  'chat.prompt': { req: { channelId: string; nodeId: string; text: string }; res: void }
  'chat.interrupt': { req: { channelId: string; nodeId: string }; res: void }
  'chat.answer': { req: { channelId: string; nodeId: string; approvalId: string; allow: boolean; forSession?: boolean }; res: void }
  /** Without nodeId: every chat node's, keyed by node id */
  'chat.transcript': { req: { channelId: string; nodeId?: string }; res: Transcript }
  /** Desktop only; needs Remote control on: colleagues connect through it */
  'guest.invite': { req: { channelId: string }; res: GuestInvite }
  'guest.list': { req: { channelId: string }; res: GuestInfo[] }
  /** Ends their access at once and closes their connections */
  'guest.revoke': { req: { channelId: string; guestId: string }; res: boolean }
  'routine.list': { req: void; res: Routine[] }
  /** Creates (id '') or updates; refused over 20 enabled or under 15 minutes */
  'routine.save': { req: { routine: Routine }; res: Routine }
  'routine.delete': { req: { id: string }; res: void }
  'routine.runNow': { req: { id: string }; res: void }
  /** The finding was seen: off Needs you */
  'routine.dismiss': { req: { id: string }; res: void }
  /** The answer goes back to the Bot as the user's next message */
  'ask.answer': { req: { channelId: string; askId: string; text: string }; res: void }
  /** Who leads, how agent messages are relayed, and the caps */
  'team.update': { req: { channelId: string; leadId?: string; relay?: RelayMode; limits?: Team['limits']; membersTalk?: boolean; freeTalk?: boolean }; res: void }
  'team.resume': { req: { channelId: string }; res: void }
  'team.pause': { req: { channelId: string }; res: void }
  /** Newest first, at most 200 */
  'team.messages': { req: { channelId: string }; res: TeamMessage[] }
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
