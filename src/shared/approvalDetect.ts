// 偵測 CLI 終端輸出是否停在「等待使用者審批」的提示上。
//
// ponytail: 目前畫面的選單啟發式，天花板是 CLI 換皮就失準。
// 升級路徑：哪天某家 CLI 走結構化輸出（如 Claude Code 的 stream-json），
// 就替該家改用事件流判斷，不必動這裡的其他家。

import type { Terminal } from '@xterm/headless'

const ANSI = /\[[0-9;?]*[A-Za-z]|\][^]*/g

/** 去掉 ANSI 控制碼，留下人看得到的字 */
export function stripAnsi(s: string): string {
  return s.replace(ANSI, '')
}

/** 只讀當前可見畫面，不讀 scrollback，也不以可能停在 spinner 的游標當結尾。 */
export function readApprovalScreen(term: Pick<Terminal, 'buffer' | 'rows'>): string {
  const buf = term.buffer.active
  return Array.from({ length: term.rows }, (_, y) => buf.getLine(buf.baseY + y)?.translateToString(true) || '').join('\n')
}

const BOX = /[│┃║╭╮╰╯┌┐└┘├┤─━═┏┓┗┛]/g
const OPTION = /^\s*([❯›>▶→]\s*)?(\d)[.)]\s+\S/
const FOOTER = /^(?:(?:press\s+)?(?:enter|esc(?:ape)?|tab|space)(?:\s+to\b|[\/·])|use\s+(?:the\s+)?arrow\s+keys\b|[↑↓]+.*\bto\b)/i
const INPUT = /^[❯›>]\s*(?!\d[.)]\s)/

/**
 * 呼叫端必須先讓 xterm 處理完 ANSI，再傳入 readApprovalScreen 的當前畫面。
 * 一般文字提到 approve / 是否繼續，不代表 CLI 正在等待輸入。
 */
export function looksLikeApprovalPrompt(screen: string): boolean {
  const lines = stripAnsi(screen).split(/\r?\n/).map(line => line.replace(BOX, ' ').trimEnd())
  const inCode = (end: number): boolean => lines.slice(0, end).filter(line => /^\s*(?:```|~~~)/.test(line)).length % 2 === 1
  const tail = lines.filter(line => line.trim()).at(-1)?.trim() || ''
  // y/n 必須是畫面最後的實際問句，不是段落裡提到這種提示。
  if (!inCode(lines.length) && /[([]\s*y\s*\/\s*n\s*[)\]]\s*[?？:：]?\s*[yn]?$/i.test(tail) &&
    /[?？]|\b(?:overwrite|continue|proceed|confirm|allow|approve)\b|是否|確認/i.test(tail)) return true
  const numbered = lines.map((line, index) => ({ index, match: OPTION.exec(line) })).filter(row => row.match)
  const last = numbered.at(-1)
  if (last) {
    const start = numbered.findLastIndex(row => row.match![2] === '1')
    if (start === -1) return false
    const options = numbered.slice(start)
    if (options.length < 2 || options.some((row, i) => Number(row.match![2]) !== i + 1)) return false
    const first = options[0].index
    // 避免把 agent 說明中的範例選單當成正在等待回覆。
    if (inCode(first)) return false
    const after = lines.slice(last.index + 1)
    if (!options.some(row => row.match![1]) && !after.some(line => FOOTER.test(line.trim()))) return false
    return after.every(line => !line.trim() || FOOTER.test(line.trim()) ||
      (/^\s+/.test(line) && !INPUT.test(line.trim()) && !/^\s*(?:```|~~~)/.test(line)))
  }
  return unnumberedMenu(lines) !== -1
}

/**
 * 沒有編號的選單（例如 Claude 的資料夾信任畫面）：有標記的選項，後面只有縮排行和提到 Enter 的操作提示。
 * 要求提到 Enter，才不會把輸入列下方的「esc to interrupt」當成選單。與 wb-pty approval.rs 的 unnumbered_menu 相同。
 */
function unnumberedMenu(lines: string[]): number {
  const fence = /^\s*(?:```|~~~)/
  const m = lines.findLastIndex(line => /^\s*[❯›>▶→]\s*\S/.test(line))
  if (m === -1 || lines.slice(0, m).filter(line => fence.test(line)).length % 2 === 1) return -1
  const after = lines.slice(m + 1)
  // 「Enter to confirm · Esc to cancel」（Claude）或「↑/↓ Navigate · enter Confirm」（agy）
  const ok = after.some(line => /\benter\b/i.test(line) && (FOOTER.test(line.trim()) || /[·↑↓]/.test(line))) &&
    after.every(line => !line.trim() || FOOTER.test(line.trim()) || (/^\s+/.test(line) && !INPUT.test(line.trim()) && !fence.test(line)))
  return ok ? m : -1
}
