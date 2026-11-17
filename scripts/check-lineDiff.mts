// The approval card's diff: node --experimental-strip-types --import ./scripts/ts-resolve.mjs scripts/check-lineDiff.mts
import assert from 'node:assert'
import { lineDiff, withContext } from '../src/shared/lineDiff'

const show = (before: string, after: string): string[] => lineDiff(before, after).map((r) => r.kind + r.text)
// A changed line: the old one first, and the file's last newline is not a line
assert.deepEqual(show('hello\n', 'hello world\nbye\n'), ['-hello', '+hello world', '+bye'])
assert.deepEqual(show('a\nb\nc', 'a\nB\nc'), [' a', '-b', '+B', ' c'])
assert.deepEqual(show('', 'x\ny\n'), ['+x', '+y'])
assert.deepEqual(show('x\r\ny\r\n', ''), ['-x', '-y'])
assert.deepEqual(show('same\n', 'same\n'), [' same'])
// Line numbers follow each side
assert.deepEqual(lineDiff('a\nb', 'a\nc\nb').map((r) => [r.a, r.b]), [[1, 1], [undefined, 2], [2, 3]])
// Far-apart changes keep three lines around each and a gap between
const many = Array.from({ length: 30 }, (_, i) => `l${i}`)
const edited = many.map((l, i) => (i === 2 || i === 25 ? l + '!' : l))
const ctx = withContext(lineDiff(many.join('\n'), edited.join('\n')))
assert.equal(ctx.filter((r) => r.kind === '…').length, 1)
assert.equal(ctx.filter((r) => r.kind === '-').length, 2)
assert.ok(ctx.length < 20, String(ctx.length))
// Too big for the table: still every line, removed then added
const big = Array.from({ length: 700 }, (_, i) => `x${i}`).join('\n')
assert.equal(lineDiff(big, big + '\nmore').length, 1401)
console.log('lineDiff: order, line numbers, context, large input ok')
