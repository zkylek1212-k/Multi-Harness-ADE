// node --experimental-strip-types scripts/check-remote.mts
// What the phone page decides by itself. The server side is in check-remote-rust.mts.
import assert from 'node:assert'
import { parsePrompt, questionPreview } from '../src/renderer/remote/prompt.ts'
import { sessionStatus } from '../src/shared/remoteProtocol.ts'

// —— 審批提示解析：手機把選項顯示成有文字的按鈕 ——
const claudeScreen = [
  '╭──────────────────────────────────────────────╮',
  '│ Bash command                                  │',
  '│                                               │',
  '│   rm -rf build/                               │',
  '│   Remove the build directory                  │',
  '│                                               │',
  '│ Do you want to proceed?                       │',
  '│ ❯ 1. Yes                                      │',
  "│   2. Yes, and don't ask again for rm commands │",
  '│      in this project                          │',
  '│   3. No, and tell Claude what to do           │',
  '│      differently (esc)                        │',
  '╰──────────────────────────────────────────────╯'
].join('\n')
const cp = parsePrompt(claudeScreen)!
assert.equal(cp.question, 'Do you want to proceed?')
assert.deepEqual(cp.details, ['rm -rf build/', 'Remove the build directory'])
assert.deepEqual(
  cp.options.map((o) => [o.key, o.label, o.selected]),
  [
    ['1', 'Yes', true],
    ['2', "Yes, and don't ask again for rm commands in this project", false],
    ['3', 'No, and tell Claude what to do differently', false]
  ]
)
const codex = parsePrompt('Allow command?\n$ npm test\n› 1. Yes, proceed (y)\n  2. Always (a)\n  3. No (esc)')!
assert.deepEqual(codex.options.map((o) => o.key), ['y', 'a', '3'])
assert.equal(codex.options[2].label, 'No')
assert.equal(codex.question, 'Allow command?')
const yn = parsePrompt('some output\nOverwrite file? [y/N]')!
assert.deepEqual(yn.options.map((o) => [o.key, o.selected]), [['y', false], ['n', true]])
assert.equal(parsePrompt('just output\n1. not a menu'), null)
assert.equal(questionPreview('foo\n│ Do you want to proceed? │\n│ ❯ 1. Yes │'), 'Do you want to proceed?')

// 互動問答的選項間有空行，每個選項另有多行說明。
const questionScreen = [
  '要使用哪種登入方式？', '',
  '❯ 1. 電子郵件（建議）',
  '     使用信箱收取登入連結。',
  '     不需要記住密碼，',
  '     適合一般使用者。', '',
  '  2. 公司帳號',
  '     使用公司提供的單一登入。', '',
  '  3. 其他方式',
  '     輸入你偏好的方式。', '',
  'Enter to select · Tab/Arrow keys to navigate · Esc to cancel'
].join('\n')
const qp = parsePrompt(questionScreen)!
assert.ok(qp, '空行與多行說明不應讓問答選項退回數字鍵')
assert.equal(qp.question, '要使用哪種登入方式？')
assert.deepEqual(qp.options.map(o => [o.key, o.label, o.selected]), [
  ['1', '電子郵件（建議） 使用信箱收取登入連結。 不需要記住密碼， 適合一般使用者。', true],
  ['2', '公司帳號 使用公司提供的單一登入。', false],
  ['3', '其他方式 輸入你偏好的方式。', false]
])
assert.equal(parsePrompt('old output\n1. Old item\n2. Old item\n\nNew question?\n3. Incomplete menu'), null)
assert.deepEqual(parsePrompt('old output\n1. Old\n2. Old\n\nNew question?\n1. New\n\n2. Other')?.options.map(o => o.label), ['New', 'Other'])

// —— 終端狀態：只看桌面送來的 busy，不看時鐘 ——
const base = { id: 'a', needsApproval: false, busy: false }
assert.equal(sessionStatus(base), 'idle')
assert.equal(sessionStatus({ ...base, busy: true }), 'running')
// 等你回覆優先於執行中
assert.equal(sessionStatus({ ...base, needsApproval: true, busy: true }), 'waiting')
// activity 事件（live）蓋掉 state 的初值，兩個方向都要蓋得掉
assert.equal(sessionStatus(base, { a: true }), 'running')
assert.equal(sessionStatus({ ...base, busy: true }, { a: false }), 'idle')
// 別的 session 的 activity 不會影響這個
assert.equal(sessionStatus(base, { b: true }), 'idle')

console.log('remote ok')
