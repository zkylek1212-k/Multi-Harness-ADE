// 核准偵測：node --experimental-strip-types scripts/check-approval-parity.mts
// TS 與 Rust 跑同一組畫面，判定要完全一樣。手機的待審批卡片靠這個。
import './ts-resolve.mjs'
import assert from 'node:assert'

const { looksLikeApprovalPrompt } = await import('../src/shared/approvalDetect.ts')
const { coreCall } = await import('./lib/core-call.mts')

const screens = [
  // 一般輸出
  'I will approve the changes after the tests pass.',
  'Would you like to see the implementation details?',
  '等待測試確認結果，繼續執行。',
  '1. Yes, the tests pass.\n2. No regressions were found.',
  'A confirmation prompt looks like this:\n```text\nAllow command?\n› 1. Yes\n  2. No\n```',
  'A prompt example is still streaming:\n```text\nOverwrite file? [y/N]',
  'Do you want to proceed?\n❯ 1. Yes\n  2. No\n\n❯ ',
  'Overwrite file? [y/N]\nDone.\n› ',
  '',
  '   \n\n  ',
  'just one line',
  '1. only one option',
  '2. starts at two\n3. then three',
  '1. one\n3. skipped two\nPress Enter to confirm',
  // 真的在等
  'Do you want to proceed?\n❯ 1. Yes\n  2. No',
  '1. First result\n2. Second result\n\nOverwrite file? [y/N]',
  'Which login method?\n❯ 1. Email\n  Details.\n\n  2. Company account\n  More details.',
  'Allow this?\n> 1. Allow\n  2. Deny\n\nPress Enter to confirm · Esc to cancel',
  '  1) Yes\n  2) No\n\nUse the arrow keys to choose',
  '1. Yes\n2. No\n↑↓ to navigate',
  '是否覆寫？ [y/n]',
  '確認刪除 (y/N):',
  'Continue? (Y/n) ',
  'continue [y/n] y',
  // 方框與結尾
  '╭────────────╮\n│ Do you want to proceed? │\n╰────────────╯\n ❯ 1. Yes\n   2. No',
  '│ ❯ 1. Yes │\n│   2. No  │\n╰──────────╯',
  '❯ 1. Yes\n  2. No\nmore text at column zero',
  '❯ 1. Yes\n  2. No\n```',
  '❯ 1. Yes\n  2. No\n  ❯ typing here',
  '❯ 1. Yes\n  2. No\n  ❯ 3. Maybe',
  '```\ncode\n```\n❯ 1. Yes\n  2. No',
  'tab\tseparated [y/n]?',
  'Windows line\r\n❯ 1. Yes\r\n  2. No\r\n'
]

let mismatches = 0
for (const s of screens) {
  const ts = looksLikeApprovalPrompt(s)
  const rust = coreCall('looksLikeApprovalPrompt', s)
  if (ts !== rust) {
    mismatches++
    console.log(`MISMATCH ts=${ts} rust=${rust}: ${JSON.stringify(s)}`)
  }
}
assert.equal(mismatches, 0)
console.log(`approval parity ok (${screens.length} screens)`)
