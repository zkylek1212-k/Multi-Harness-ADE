export const CUSTOM_AGENT_ID = 'custom-agent'
export const CUSTOM_RESOURCE_KINDS = ['skill', 'mcp', 'plugin', 'hook'] as const
export type CustomResourceKind = typeof CUSTOM_RESOURCE_KINDS[number]

export interface CustomAgentConfig {
  enabled: boolean
  name: string
  command: string
  args: string[]
  /** How a chat node talks to it; 'pty' = terminal only (cowork-agui-implement.md §2.3) */
  protocol: CustomProtocol
  /** agui-http only: `command` is the endpoint; plain http needs this (cowork-agui-implement.md §4.5) */
  allowHttp: boolean
  resources: Record<CustomResourceKind, string>
}

export const CUSTOM_PROTOCOLS = ['pty', 'codex-app-server', 'claude-stream-json', 'acp', 'agui-http'] as const
export type CustomProtocol = typeof CUSTOM_PROTOCOLS[number]

export function sanitizeCustomAgent(raw: unknown): CustomAgentConfig {
  const c = (raw && typeof raw === 'object' ? raw : {}) as Partial<CustomAgentConfig>
  const text = (v: unknown): string => typeof v === 'string' ? v.trim() : ''
  return {
    enabled: c.enabled === true,
    name: text(c.name) || 'Custom Agent',
    command: text(c.command),
    args: Array.isArray(c.args) ? c.args.filter((a): a is string => typeof a === 'string' && a.length > 0) : [],
    protocol: (CUSTOM_PROTOCOLS as readonly string[]).includes(c.protocol as string) ? (c.protocol as CustomProtocol) : 'pty',
    allowHttp: c.allowHttp === true,
    resources: {
      skill: text(c.resources?.skill), mcp: text(c.resources?.mcp),
      plugin: text(c.resources?.plugin), hook: text(c.resources?.hook)
    }
  }
}
