// Version helpers for the two update channels: the build's own repository and the upstream project.
// A fork build is versioned "<upstream base>-fork.<n>" (e.g. 0.1.41-fork.2), so the upstream
// version it is based on can be read straight from its own version.

export const UPSTREAM_REPO = 'zkylek1212-k/Multi-Harness-ADE'

interface Parsed {
  core: [number, number, number]
  pre: (string | number)[]
}

function parse(v: string): Parsed | null {
  const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(v.trim())
  if (!m) return null
  const pre = m[4] ? m[4].split('.').map((p) => (/^\d+$/.test(p) ? Number(p) : p)) : []
  return { core: [Number(m[1]), Number(m[2]), Number(m[3])], pre }
}

/** Semver precedence: -1 if a < b, 1 if a > b, 0 if equal. Unparseable versions compare as equal. */
export function compareVersions(a: string, b: string): number {
  const x = parse(a)
  const y = parse(b)
  if (!x || !y) return 0
  for (let i = 0; i < 3; i++) if (x.core[i] !== y.core[i]) return x.core[i] < y.core[i] ? -1 : 1
  // A version without a prerelease ranks above the same version with one (1.0.0 > 1.0.0-fork.1)
  if (!x.pre.length || !y.pre.length) return x.pre.length === y.pre.length ? 0 : x.pre.length ? -1 : 1
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i]
    const q = y.pre[i]
    if (p === undefined) return -1
    if (q === undefined) return 1
    if (p === q) continue
    if (typeof p === 'number' && typeof q === 'number') return p < q ? -1 : 1
    if (typeof p === 'number') return -1
    if (typeof q === 'number') return 1
    return p < q ? -1 : 1
  }
  return 0
}

export function isValidVersion(v: string): boolean {
  return parse(v) !== null
}

/** The upstream version a fork build is based on ("0.1.41-fork.2" -> "0.1.41"); null if not a fork version */
export function upstreamBaseOf(version: string): string | null {
  const m = /^v?(\d+\.\d+\.\d+)-fork\.\d+$/.exec(version.trim())
  return m ? m[1] : null
}

/** "owner/repo" only; anything else is rejected so it cannot be turned into another URL */
export function isRepoSlug(s: unknown): s is string {
  return typeof s === 'string' && /^[A-Za-z0-9-]{1,39}\/[A-Za-z0-9._-]{1,100}$/.test(s)
}
