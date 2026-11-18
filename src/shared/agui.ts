// Chat nodes (cowork-agui-implement.md §3): AG-UI events from wb-core agui/*.rs, and the entries a conversation keeps
// (agui/transcript.rs). Text in entries comes from agents: show it as text, never as HTML.

export type AguiEvent =
  | { type: 'RUN_STARTED'; threadId: string; runId: string }
  | { type: 'RUN_FINISHED'; threadId: string; runId: string }
  | { type: 'RUN_ERROR'; message: string; code?: 'interrupted' }
  | { type: 'TEXT_MESSAGE_START'; messageId: string; role: string }
  | { type: 'TEXT_MESSAGE_CONTENT'; messageId: string; delta: string }
  | { type: 'TEXT_MESSAGE_END'; messageId: string }
  | { type: 'TOOL_CALL_START'; toolCallId: string; toolCallName: string }
  | { type: 'TOOL_CALL_ARGS'; toolCallId: string; delta: string }
  | { type: 'TOOL_CALL_END'; toolCallId: string }
  | { type: 'TOOL_CALL_RESULT'; messageId: string; toolCallId: string; content: string; role: 'tool' }
  | { type: 'CUSTOM'; name: string; value: unknown }

export type TranscriptEntry =
  | { kind: 'user'; id: string; text: string; from: string; at: number }
  | { kind: 'text'; id: string; text: string; at: number }
  | { kind: 'tool'; id: string; name: string; args: string; output: string; status: 'running' | 'done' | 'stopped'; at: number }
  | { kind: 'approval'; id: string; tool: string; summary: string; status: 'pending' | 'allowed' | 'denied' | 'expired'; sessionScope: boolean; at: number }
  | { kind: 'runError'; id: string; message: string; interrupted: boolean; at: number }
  /** From the app: 'took-control' | 'handed-back' */
  | { kind: 'note'; id: string; text: string; at: number }

export interface Transcript {
  entries: TranscriptEntry[]
}

export interface AguiApproval {
  id: string
  tool: string
  summary: string
  input: unknown
  sessionScope: boolean
}

/** `canvas:agui`: one live change of a chat node */
export interface AguiUpdate {
  repoId: string
  channelId: string
  nodeId: string
  event?: AguiEvent
  /** The entry the event created or changed, to upsert by id */
  entry?: TranscriptEntry | null
  approval?: AguiApproval
  settled?: { id: string; status: 'allowed' | 'denied' }
}

export const AGUI_EVENT = 'canvas:agui'

/** One line for a tool call: the command, the file, or the tool's name */
export function toolSummary(e: Extract<TranscriptEntry, { kind: 'tool' }>): string {
  let args: Record<string, unknown> = {}
  try {
    args = JSON.parse(e.args || '{}')
  } catch {
    /* streamed args may be cut */
  }
  const pick = (...keys: string[]): string | undefined => {
    for (const k of keys) if (typeof args[k] === 'string' && args[k]) return args[k] as string
    return undefined
  }
  const path = pick('file_path', 'notebook_path', 'path', 'Path', 'TargetFile', 'AbsolutePath')
  const shown = pick('command', 'CommandLine', 'cmd') ?? path?.split(/[\\/]/).slice(-2).join('/') ?? pick('pattern', 'query', 'url', 'description')
  return shown ? shown.replace(/\s+/g, ' ').slice(0, 160) : e.name
}
