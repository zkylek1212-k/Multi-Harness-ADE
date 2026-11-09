// 自我檢查：node --experimental-strip-types scripts/check-usage.mts
import assert from 'node:assert'
import { coreCall } from './lib/core-call.mts'

const claudeDays = (t: string): any => coreCall('claudeDays', t)
const codexDays = (t: string): any => coreCall('codexDays', t)
const antigravityDays = (t: string): any => coreCall('antigravityDays', t)
const sumDays = (d: unknown, since = ''): any => coreCall('sumDays', d, since)
const dayKey = (s: string): string => coreCall('dayKey', s)

const j = (o: unknown): string => JSON.stringify(o)
const d1 = '2026-09-01T10:00:00'
const d2 = '2026-09-02T10:00:00'

// Claude：同一 message.id 拆成多行只算一次；每次呼叫的 input 要加總（不是取最大）
const claude = [
  j({ timestamp: d1, message: { id: 'a', usage: { input_tokens: 10, cache_creation_input_tokens: 5, cache_read_input_tokens: 100, output_tokens: 7 } } }),
  j({ timestamp: d1, message: { id: 'a', usage: { input_tokens: 10, cache_creation_input_tokens: 5, cache_read_input_tokens: 100, output_tokens: 7 } } }),
  j({ timestamp: d2, message: { id: 'b', usage: { input_tokens: 1, cache_read_input_tokens: 200, output_tokens: 3 } } }),
  'not json',
  j({ timestamp: d2, type: 'user', message: { content: 'hi' } })
].join('\n')
const c = claudeDays(claude)
assert.deepEqual(sumDays(c), { input: 16, cacheRead: 300, output: 10 })
assert.deepEqual(sumDays(c, dayKey(d2)), { input: 1, cacheRead: 200, output: 3 }) // 區間切日

// Codex：累計值取差值；input 已含 cached，要拆開
const codex = [
  j({ timestamp: d1, payload: { info: { total_token_usage: { input_tokens: 100, cached_input_tokens: 40, output_tokens: 10 } } } }),
  j({ timestamp: d2, payload: { info: { total_token_usage: { input_tokens: 250, cached_input_tokens: 100, output_tokens: 30 } } } })
].join('\n')
const x = codexDays(codex)
assert.deepEqual(x[dayKey(d1)], { input: 60, cacheRead: 40, output: 10 })
assert.deepEqual(x[dayKey(d2)], { input: 90, cacheRead: 60, output: 20 })

// Antigravity：字數估算，模型回覆算 output，使用者/工具算 input
const agy = [
  j({ created_at: d1, type: 'USER_INPUT', content: 'x'.repeat(35) }),
  j({ created_at: d1, source: 'MODEL', type: 'PLANNER_RESPONSE', thinking: 'y'.repeat(7), content: 'z'.repeat(7) })
].join('\n')
assert.deepEqual(sumDays(antigravityDays(agy)), { input: 10, cacheRead: 0, output: 4 })

console.log('usage parsers ok')
