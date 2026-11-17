// A plain line diff for the approval card: what an agent is about to change, removed lines before the lines that replace them.
export type Row = { kind: ' ' | '+' | '-' | '…'; text: string; a?: number; b?: number }
/** Line diff by longest common subsequence; big inputs fall back to "all removed, then all added" */
export function lineDiff(before: string, after: string): Row[] {
  const lines = (s: string): string[] => (s === '' ? [] : s.replace(/\r?\n$/, '').split(/\r?\n/))
  const a = lines(before)
  const b = lines(after)
  const rows: Row[] = []
  if (a.length * b.length > 400_000) {
    a.forEach((text, i) => rows.push({ kind: '-', text, a: i + 1 }))
    b.forEach((text, i) => rows.push({ kind: '+', text, b: i + 1 }))
    return rows
  }
  const m = a.length
  const n = b.length
  const lcs: Uint32Array[] = Array.from({ length: m + 1 }, () => new Uint32Array(n + 1))
  for (let i = m - 1; i >= 0; i--) for (let j = n - 1; j >= 0; j--) lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1])
  let i = 0
  let j = 0
  while (i < m || j < n) {
    if (i < m && j < n && a[i] === b[j]) rows.push({ kind: ' ', text: a[i], a: ++i, b: ++j })
    else if (i < m && (j === n || lcs[i + 1][j] >= lcs[i][j + 1])) rows.push({ kind: '-', text: a[i], a: ++i })
    else rows.push({ kind: '+', text: b[j], b: ++j })
  }
  return rows
}

/** Changed lines with three lines of context around them */
export function withContext(rows: Row[]): Row[] {
  const keep = new Uint8Array(rows.length)
  rows.forEach((r, i) => {
    if (r.kind === ' ') return
    for (let k = Math.max(0, i - 3); k <= Math.min(rows.length - 1, i + 3); k++) keep[k] = 1
  })
  const out: Row[] = []
  let gap = false
  rows.forEach((r, i) => {
    if (keep[i]) {
      if (gap && out.length) out.push({ kind: '…', text: '' })
      gap = false
      out.push(r)
    } else gap = true
  })
  return out
}
