// node --experimental-strip-types scripts/check-version.mts
// Version ordering for the fork's own update channel and the upstream notice.
import assert from 'node:assert/strict'
import { compareVersions, isRepoSlug, isValidVersion, upstreamBaseOf } from '../src/shared/version.ts'

const lt = (a: string, b: string): void => {
  assert.equal(compareVersions(a, b), -1, `${a} < ${b}`)
  assert.equal(compareVersions(b, a), 1, `${b} > ${a}`)
}

lt('0.1.41', '0.1.42')
lt('0.1.9', '0.1.10')
lt('0.1.41-fork.1', '0.1.41-fork.2')
lt('0.1.41-fork.9', '0.1.41-fork.10')
lt('0.1.41-fork.3', '0.1.42-fork.1')
lt('0.1.41-fork.1', '0.1.41') // a prerelease ranks below its release
lt('1.0.0-alpha', '1.0.0-alpha.1')
lt('1.0.0-alpha.1', '1.0.0-alpha.beta')
lt('1.0.0-beta.11', '1.0.0-rc.1')
assert.equal(compareVersions('v0.1.41', '0.1.41'), 0)
assert.equal(compareVersions('0.1.41+build.5', '0.1.41'), 0)

assert.equal(upstreamBaseOf('0.1.41-fork.2'), '0.1.41')
assert.equal(upstreamBaseOf('v0.1.41-fork.12'), '0.1.41')
assert.equal(upstreamBaseOf('0.1.41'), null)
assert.equal(upstreamBaseOf('0.1.41-beta.1'), null)

assert.ok(isValidVersion('0.1.41-fork.1') && !isValidVersion('fork-v0.1.41') && !isValidVersion('0.1'))
assert.ok(isRepoSlug('ikenican/Multi-Harness-ADE'))
for (const bad of ['ikenican', 'a/b/c', 'https://evil.example/x', '../x/y', 'a /b']) assert.ok(!isRepoSlug(bad), bad)

console.log('version ok')
