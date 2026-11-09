// node --experimental-strip-types scripts/check-remote-rust.mts
// The phone bridge in Rust, over real sockets. Needs: cargo build -p wb-ipc, and a built phone bundle (out/desktop) for the static checks.
// Certificates are also checked against ones the old Electron code made, since phones in the field trust those.
import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as http from 'node:http'
import * as https from 'node:https'
import * as net from 'node:net'
import * as os from 'node:os'
import * as path from 'node:path'
import * as tls from 'node:tls'
import { createInterface } from 'node:readline'
import { WebSocket, WebSocketServer } from 'ws'

import * as legacy from './lib/legacy-ca.mts'
import { PHONE_AUDITED_OPS, PHONE_COWORK_OPS } from '../src/shared/remoteProtocol.ts'
import { coreCall as rc, exe } from './lib/core-call.mts'

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'wb-remote-e2e-'))
assert.deepEqual([...rc('phoneCoworkOps')].sort(), [...PHONE_COWORK_OPS].sort(), 'the phone allowlist matches what the phone page expects')
assert.deepEqual([...rc('phoneAuditedOps')].sort(), [...PHONE_AUDITED_OPS].sort())

// ── certificates ──
async function handshake(cert: { certPem: string; keyPem: string }, servername: string, caPem: string): Promise<string> {
  const server = tls.createServer({ cert: cert.certPem, key: cert.keyPem }, (s) => s.end())
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', () => r()))
  const port = (server.address() as net.AddressInfo).port
  try {
    return await new Promise<string>((resolve) => {
      const s = tls.connect({ host: '127.0.0.1', port, servername, ca: caPem }, () => {
        s.end()
        resolve('ok')
      })
      s.on('error', (e) => resolve(e.message))
    })
  } finally {
    server.close()
  }
}
const ca = rc('createCa', 'check', false)
const x = new crypto.X509Certificate(ca.certPem)
assert.ok(x.ca, 'it is a CA')
assert.ok(x.subject.includes('Agent Workbench Local CA (check)'))
assert.equal(rc('fingerprint256', ca.certPem), x.fingerprint256, 'same fingerprint format the settings page always showed')
const good = rc('issueServerCert', ca, ['192.168.50.7'], ['box.local'])
assert.equal(await handshake(good, 'box.local', ca.certPem), 'ok')
assert.match(await handshake(rc('issueServerCert', ca, [], ['www.google.com']), 'www.google.com', ca.certPem), /subtree/, 'the constraints hold')
const meshCa = rc('createCa', 'check', true)
assert.ok(new crypto.X509Certificate(meshCa.certPem).subject.includes('+ VPN'))
assert.ok(!rc('caPermitsMesh', ca.certPem) && rc('caPermitsMesh', meshCa.certPem))
const SAN = [['192.168.50.7', '100.85.1.2'], ['box.local']]
assert.match(await handshake(rc('issueServerCert', ca, ...SAN), 'box.local', ca.certPem), /subtree|permitted/)
assert.equal(await handshake(rc('issueServerCert', meshCa, ...SAN), 'box.local', meshCa.certPem), 'ok')
assert.match(await handshake(rc('issueServerCert', meshCa, [], ['www.google.com']), 'www.google.com', meshCa.certPem), /subtree/)

// A CA and key from the Electron code: what phones in the field already trust. New leaves must chain to it.
const oldCa = legacy.createCa('old-pc')
assert.equal(await handshake(rc('issueServerCert', oldCa, ['192.168.50.7'], ['box.local']), 'box.local', oldCa.certPem), 'ok', 'a new leaf chains to an existing CA')
assert.ok(rc('certCovers', legacy.issueServerCert(oldCa, ['192.168.50.7'], ['box.local']).certPem, ['192.168.50.7'], ['box.local']), 'reads what the old code issued')
assert.equal(rc('caPermitsMesh', oldCa.certPem), false)
assert.equal(rc('caPermitsMesh', legacy.createCa('old', true).certPem), true, 'recognises an old mesh CA, so phones are not asked to reinstall')
console.log('certificates ok')

// ── Web Push ──
function decrypt(body: Buffer, ua: crypto.ECDH, auth: Buffer): string {
  const salt = body.subarray(0, 16)
  const idlen = body[20]
  const asPublic = body.subarray(21, 21 + idlen)
  const ct = body.subarray(21 + idlen)
  const h = (k: Buffer, d: Buffer): Buffer => crypto.createHmac('sha256', k).update(d).digest()
  const prkKey = h(auth, ua.computeSecret(asPublic))
  const ikm = h(prkKey, Buffer.concat([Buffer.from('WebPush: info\0'), ua.getPublicKey(), asPublic, Buffer.from([1])]))
  const prk = h(salt, ikm)
  const cek = h(prk, Buffer.from('Content-Encoding: aes128gcm\0\x01')).subarray(0, 16)
  const nonce = h(prk, Buffer.from('Content-Encoding: nonce\0\x01')).subarray(0, 12)
  const d = crypto.createDecipheriv('aes-128-gcm', cek, nonce)
  d.setAuthTag(ct.subarray(-16))
  const pt = Buffer.concat([d.update(ct.subarray(0, -16)), d.final()])
  assert.equal(pt[pt.length - 1], 2)
  return pt.subarray(0, -1).toString()
}
{
  const ua = crypto.createECDH('prime256v1')
  ua.generateKeys()
  const auth = crypto.randomBytes(16)
  const sub = { endpoint: 'https://web.push.apple.com/QGx', keys: { p256dh: ua.getPublicKey().toString('base64url'), auth: auth.toString('base64url') } }
  const msg = JSON.stringify({ title: '⏳ Claude Code 等待你的回覆' })
  assert.equal(decrypt(Buffer.from(rc('encryptPayload', sub, Buffer.from(msg).toString('base64')), 'base64'), ua, auth), msg)
  const verify = (keys: any): void => {
    const m = /^vapid t=([^.]+)\.([^.]+)\.([^,]+), k=(.+)$/.exec(rc('vapidAuthHeader', sub.endpoint, keys, 'https://example.com'))!
    const pub = Buffer.from(m[4], 'base64url')
    assert.equal(pub.length, 65)
    const jwk = { kty: 'EC', crv: 'P-256', x: pub.subarray(1, 33).toString('base64url'), y: pub.subarray(33).toString('base64url') }
    assert.ok(crypto.verify('sha256', Buffer.from(`${m[1]}.${m[2]}`), { key: crypto.createPublicKey({ key: jwk, format: 'jwk' }), dsaEncoding: 'ieee-p1363' }, Buffer.from(m[3], 'base64url')))
    assert.equal(JSON.parse(Buffer.from(m[2], 'base64url').toString()).aud, 'https://web.push.apple.com')
  }
  verify(rc('generateVapidKeys'))
  // Keys Electron's Node made: phones are subscribed against these
  verify(legacy.legacyVapid())
  console.log('web push ok')
}

// ── files ──
const ws = path.join(tmp, 'workspace')
const outside = path.join(tmp, 'outside')
fs.mkdirSync(path.join(ws, 'docs'), { recursive: true })
fs.mkdirSync(path.join(ws, 'node_modules'))
fs.mkdirSync(outside)
fs.writeFileSync(path.join(outside, 'private.txt'), 'outside workspace')
fs.symlinkSync(outside, path.join(ws, 'escape'), 'junction')
fs.writeFileSync(path.join(ws, 'README.md'), '# 手機唯讀預覽')
fs.writeFileSync(path.join(ws, 'docs', 'view.html'), '<link rel="stylesheet" href="theme.css"><h1>Preview</h1>')
fs.writeFileSync(path.join(ws, 'docs', 'theme.css'), 'h1 { color: teal }')
fs.writeFileSync(path.join(ws, 'docs', 'report.pdf'), '%PDF-1.4\nPDF bytes for range checks\n%%EOF')
fs.writeFileSync(path.join(ws, 'binary.dat'), Buffer.from([0, 1, 2]))
fs.writeFileSync(path.join(ws, 'large.txt'), Buffer.alloc(1024 * 1024 + 1, 65))
{
  const list = rc('listWorkspaceFiles', ws, '')
  assert.equal(list[0].name, 'docs')
  assert.ok(!list.some((e: any) => e.name === 'escape' || e.name === 'node_modules'), 'no junctions, no ignored folders')
  assert.equal(rc('readWorkspaceText', ws, 'README.md'), '# 手機唯讀預覽')
  for (const bad of ['../outside/private.txt', path.join(outside, 'private.txt'), 'escape/private.txt', 'docs/../../outside/private.txt', '..\\outside\\private.txt', 'C:private.txt', '/Windows/win.ini', 'a\0b']) {
    assert.throws(() => rc('workspacePath', ws, bad), undefined, bad)
  }
  assert.throws(() => rc('readWorkspaceText', ws, 'binary.dat'), /binary/)
  assert.throws(() => rc('readWorkspaceText', ws, 'large.txt'), /1 MB/)
  console.log('file rules ok')
}

// ── the running bridge ──
const data = path.join(tmp, 'data')
fs.mkdirSync(path.join(data, 'remote'), { recursive: true })
const child = spawn(exe, [], { env: { ...process.env, WB_DATA_DIR: data, USERPROFILE: path.join(tmp, 'home'), WB_REMOTE_STATIC: path.resolve('out/desktop') }, stdio: ['pipe', 'pipe', 'inherit'] })
const events: any[] = []
const replies = new Map<number, (m: any) => void>()
createInterface({ input: child.stdout! }).on('line', (line) => {
  const m = JSON.parse(line)
  if (m.event) events.push(m)
  else if (m.id !== undefined) {
    replies.get(m.id)?.(m)
    replies.delete(m.id)
  }
})
let next = 1
const ipc = (channel: string, ...args: unknown[]): Promise<any> => new Promise((resolve, reject) => {
  const id = next++
  replies.set(id, (m) => (m.error !== undefined ? reject(new Error(m.error)) : resolve(m.result)))
  child.stdin!.write(JSON.stringify({ id, window: 2, channel, args }) + '\n')
})
// Below the range outgoing connections use (49152 and up), or one of them can take a port between choosing it and binding it
let port = 0
const pickPort = (): void => {
  port = 30000 + Math.floor(Math.random() * 15000)
}
pickPort()
const req = (scheme: 'http' | 'https', p: number, urlPath: string, o: { method?: string; headers?: Record<string, string>; body?: string | Buffer } = {}): Promise<{ status: number; headers: http.IncomingHttpHeaders; body: Buffer }> => new Promise((resolve, reject) => {
  const r = (scheme === 'https' ? https : http).request({ host: '127.0.0.1', port: p, path: urlPath, method: o.method || 'GET', headers: o.headers, rejectUnauthorized: false, agent: false } as any, (res) => {
    const chunks: Buffer[] = []
    res.on('data', (c) => chunks.push(c))
    res.on('end', () => resolve({ status: res.statusCode || 0, headers: res.headers, body: Buffer.concat(chunks) }))
    res.on('error', reject)
  })
  r.on('error', reject)
  r.end(o.body)
})
const api = (urlPath: string, o: any = {}) => req('https', port, urlPath, o)
const ORIGIN = () => `https://127.0.0.1:${port}`
function open(o: { origin?: string | null; path?: string } = {}): WebSocket {
  const origin = o.origin === undefined ? ORIGIN() : o.origin
  return new WebSocket(`wss://127.0.0.1:${port}${o.path || '/ws'}`, { rejectUnauthorized: false, ...(origin ? { origin } : {}), maxPayload: 64 * 1024 * 1024 })
}
class Phone {
  ws: WebSocket
  inbox: any[] = []
  closed: number | null = null
  constructor(ws: WebSocket) {
    this.ws = ws
    ws.on('message', (raw) => this.inbox.push(JSON.parse(raw.toString())))
    ws.on('close', (c) => (this.closed = c))
    ws.on('error', () => {})
  }
  static async connect(o?: any): Promise<Phone> {
    const ws = open(o)
    await new Promise<void>((res, rej) => ws.once('open', () => res()).once('error', rej))
    return new Phone(ws)
  }
  send(m: unknown): void {
    this.ws.send(JSON.stringify(m))
  }
  async take(pred: (m: any) => boolean, what: string, ms = 15000): Promise<any> {
    const t0 = Date.now()
    for (;;) {
      const i = this.inbox.findIndex(pred)
      if (i >= 0) return this.inbox.splice(i, 1)[0]
      if (Date.now() - t0 > ms) throw new Error(`timeout waiting for ${what}; have ${JSON.stringify(this.inbox.map((m) => m.t))}`)
      await new Promise((r) => setTimeout(r, 25))
    }
  }
  async closedWith(ms = 7000): Promise<number> {
    const t0 = Date.now()
    while (this.closed === null) {
      if (Date.now() - t0 > ms) throw new Error('still open')
      await new Promise((r) => setTimeout(r, 25))
    }
    return this.closed
  }
}
const refused = (o: any): Promise<boolean> => new Promise((resolve) => {
  const w = open(o)
  w.once('open', () => {
    w.close()
    resolve(false)
  })
  w.once('error', () => resolve(true))
  w.once('unexpected-response', () => resolve(true))
})
const auditLog = (): string => {
  try {
    return fs.readFileSync(path.join(data, 'remote', 'audit.log'), 'utf8')
  } catch {
    return ''
  }
}
const until = async (cond: () => boolean | Promise<boolean>, what: string, ms = 15000): Promise<void> => {
  const t0 = Date.now()
  while (!(await cond())) {
    if (Date.now() - t0 > ms) throw new Error(`timeout: ${what}`)
    await new Promise((r) => setTimeout(r, 50))
  }
}

try {
  await ipc('test:openWindow', ws)
  assert.equal((await ipc('remote:status')).running, false)
  assert.equal(await ipc('remote:createPairing'), null, 'no pairing while off')
  // Some other program may hold these three ports. That is not what this checks, so try others.
  await ipc('remote:setPort', port)
  let st = await ipc('remote:setEnabled', true)
  for (let i = 0; i < 5 && !st.running && /socket address/.test(st.error || ''); i++) {
    pickPort()
    await ipc('remote:setPort', port)
    st = await ipc('remote:setEnabled', true)
  }
  assert.equal(st.running, true, JSON.stringify(st))
  assert.equal(st.port, port)
  assert.match(st.caFingerprint, /^([0-9A-F]{2}:){31}[0-9A-F]{2}$/)
  const stored = JSON.parse(fs.readFileSync(path.join(data, 'remote', 'secrets.json'), 'utf8'))
  assert.equal(stored.caKey.enc, 'dpapi', 'private keys are sealed')
  assert.ok(!JSON.stringify(stored).includes('PRIVATE KEY'), 'no key in the clear')
  assert.ok(events.some((e) => e.event === 'remote:status'), 'the settings page hears about it')
  assert.match((await ipc('remote:qr', 'https://x/')), /^data:image\/svg\+xml;base64,/)

  // Setup page: only the CA and instructions over plain http
  const setup = await req('http', port + 1, '/', { headers: { host: '192.168.1.2:1' } })
  assert.equal(setup.status, 200)
  assert.ok(setup.body.toString().includes(st.caFingerprint))
  assert.ok(setup.body.toString().includes(`https://192.168.1.2:${port}/`))
  const caFile = await req('http', port + 1, '/ca.crt')
  assert.equal(new crypto.X509Certificate(caFile.body).fingerprint256, st.caFingerprint)
  const away = await req('http', port + 1, '/anything', { headers: { host: '192.168.1.2:1' } })
  assert.equal(away.status, 302)
  assert.equal(away.headers.location, `https://192.168.1.2:${port}/`)

  // TLS below 1.2 is refused
  const old = await new Promise<string>((resolve) => {
    const s = tls.connect({ host: '127.0.0.1', port, rejectUnauthorized: false, minVersion: 'TLSv1', maxVersion: 'TLSv1.1' }, () => {
      s.end()
      resolve('connected')
    })
    s.on('error', () => resolve('refused'))
  })
  assert.equal(old, 'refused')

  // When there is a LAN address, the certificate really covers it and chains to the CA the phone installs
  if (st.addresses[0]) {
    const verified = await new Promise<string>((resolve) => {
      const s = tls.connect({ host: st.addresses[0], port, ca: new crypto.X509Certificate(caFile.body).toString() }, () => {
        s.end()
        resolve('ok')
      })
      s.on('error', (e) => resolve(e.message))
    })
    assert.equal(verified, 'ok', `verified against the CA at ${st.addresses[0]}`)
  }

  // Static files
  const page = await api('/')
  if (fs.existsSync('out/desktop/remote.html')) {
    assert.equal(page.status, 200)
    assert.match(String(page.headers['content-type']), /text\/html/)
    assert.match(String(page.headers['content-security-policy']), /frame-ancestors 'none'/)
    assert.equal(page.headers['x-content-type-options'], 'nosniff')
    const asset = /assets\/[\w.-]+\.js/.exec(page.body.toString())
    if (asset) {
      const a = await api('/' + asset[0])
      assert.equal(a.status, 200)
      assert.match(String(a.headers['cache-control']), /immutable/)
    }
  } else {
    assert.equal(page.status, 503)
  }
  for (const bad of ['/assets/..', '/assets/../remote.html', '/assets/%2e%2e/x.js', '/..%2f..%2fetc', '/settings.json', '/remote/../../x']) {
    assert.equal((await api(bad)).status, 404, bad)
  }

  // API without a token
  assert.equal((await api('/api/vapid')).status, 401)
  assert.equal((await api('/api/vapid', { headers: { authorization: 'Bearer nope' } })).status, 401)
  assert.equal((await api('/api/push', { method: 'POST', body: '{}' })).status, 401)
  assert.equal((await api('/api/other')).status, 404)
  assert.equal((await api('/api/pair', { method: 'POST', body: JSON.stringify({ code: 'x' }) })).status, 403, 'nothing to pair with yet')
  assert.equal((await api('/api/pair', { method: 'POST', body: Buffer.alloc(65 * 1024, 32) })).status, 500, 'an oversized body is refused')

  // Pairing: one-time, and ten wrong tries void the code
  const pairing = await ipc('remote:createPairing')
  assert.match(pairing.code, /^[A-HJKMNP-Z2-9]{8}$/)
  for (let i = 0; i < 10; i++) assert.equal((await api('/api/pair', { method: 'POST', body: JSON.stringify({ code: 'WRONGONE', name: 'x' }) })).status, 403)
  assert.equal((await api('/api/pair', { method: 'POST', body: JSON.stringify({ code: pairing.code, name: 'x' }) })).status, 403, 'void after ten failures')
  const p2 = await ipc('remote:createPairing')
  const paired = await api('/api/pair', { method: 'POST', body: JSON.stringify({ code: p2.code.toLowerCase().replace(/(.{4})/, '$1-'), name: 'My Phone' }) })
  assert.equal(paired.status, 200, 'format does not matter')
  const { token, deviceId } = JSON.parse(paired.body.toString())
  assert.ok(token.length >= 40 && deviceId)
  assert.equal((await api('/api/pair', { method: 'POST', body: JSON.stringify({ code: p2.code, name: 'x' }) })).status, 403, 'one use')
  assert.ok(!fs.readFileSync(path.join(data, 'remote', 'devices.json'), 'utf8').includes(token), 'token is not stored')
  assert.equal((await api('/api/vapid', { headers: { authorization: `Bearer ${token}` } })).status, 200)
  const vapidKey = JSON.parse((await api('/api/vapid', { headers: { authorization: `Bearer ${token}` } })).body.toString()).publicKey
  assert.equal(Buffer.from(vapidKey, 'base64url').length, 65)

  // Push subscriptions only to known services
  const authBearer = { authorization: `Bearer ${token}` }
  const keys = { p256dh: 'a', auth: 'b' }
  assert.equal((await api('/api/push', { method: 'POST', headers: authBearer, body: JSON.stringify({ subscription: { endpoint: 'https://evil.example.com/', keys } }) })).status, 400)
  assert.equal((await api('/api/push', { method: 'POST', headers: authBearer, body: JSON.stringify({ subscription: { endpoint: 'http://web.push.apple.com/', keys } }) })).status, 400)
  assert.equal((await api('/api/push', { method: 'POST', headers: authBearer, body: JSON.stringify({ subscription: { endpoint: 'https://web.push.apple.com/x' } }) })).status, 400)
  assert.equal((await api('/api/push', { method: 'POST', headers: authBearer, body: JSON.stringify({ subscription: { endpoint: 'https://web.push.apple.com/x', keys } }) })).status, 200)
  assert.equal((await ipc('remote:devices'))[0].pushEnabled, true)
  await api('/api/push', { method: 'POST', headers: authBearer, body: JSON.stringify({ subscription: null }) })
  assert.equal((await ipc('remote:devices'))[0].pushEnabled, false)

  // WebSocket doors
  assert.ok(await refused({ origin: 'https://evil.example.com' }), 'cross-site')
  assert.ok(await refused({ origin: 'http://192.168.1.5:1' }), 'plain http origin')
  assert.ok(await refused({ origin: null }), 'no origin')
  assert.ok(await refused({ path: '/other' }), 'wrong path')
  assert.ok(await refused({ path: '/ws?x=1' }), 'query')
  const silent = await Phone.connect()
  assert.equal(await silent.closedWith(), 4001, 'no token in five seconds')
  const wrong = await Phone.connect()
  wrong.send({ t: 'auth', token: 'wrong' })
  assert.equal(await wrong.closedWith(), 4001)
  const nosy = await Phone.connect()
  nosy.send({ t: 'kill', id: 'x' })
  assert.equal(await nosy.closedWith(), 4001, 'anything else before auth')
  const lan = await Phone.connect({ origin: 'https://192.168.1.77:47600' })
  lan.send({ t: 'auth', token })
  await lan.take((m) => m.t === 'authed', 'authed from another computer origin')
  lan.ws.close()

  // Pair over WebSocket
  const p3 = await ipc('remote:createPairing')
  const pw = await Phone.connect()
  pw.send({ t: 'pairRequest', code: 'BADBADBA', name: 'n' })
  await pw.take((m) => m.t === 'error', 'bad code')
  pw.send({ t: 'pairRequest', code: p3.code, name: 'Second' })
  const pd = await pw.take((m) => m.t === 'paired', 'paired')
  await pw.take((m) => m.t === 'authed', 'authed')
  await pw.take((m) => m.t === 'state', 'state')
  pw.ws.close()
  await ipc('remote:revokeDevice', pd.deviceId)
  assert.equal((await ipc('remote:devices')).length, 1)

  // The main session
  const phone = await Phone.connect()
  phone.send({ t: 'auth', token })
  const authed = await phone.take((m) => m.t === 'authed', 'authed')
  assert.equal(authed.deviceName, 'My Phone')
  const state = await phone.take((m) => m.t === 'state', 'state')
  assert.equal(state.windows[0].id, 2)
  assert.equal(state.windows[0].workspace, ws)
  assert.ok(state.windows[0].launchers.some((l: any) => l.key === 'cmd'))
  assert.equal(state.bypass, false)
  phone.send({ t: 'ping' })
  await phone.take((m) => m.t === 'pong', 'pong')
  await until(async () => (await ipc('remote:status')).connectedDevices.includes('My Phone'), 'desktop shows the device')

  // Terminal
  phone.send({ t: 'spawn', windowId: 99, launcherKey: 'cmd' })
  await phone.take((m) => m.t === 'error' && /window/.test(m.message), 'bad window')
  phone.send({ t: 'spawn', windowId: 2, launcherKey: 'not-a-launcher' })
  await phone.take((m) => m.t === 'error' && /launcher/.test(m.message), 'bad launcher')
  phone.send({ t: 'spawn', windowId: 2, launcherKey: 'cmd' })
  const { id } = await phone.take((m) => m.t === 'spawned', 'spawned')
  await until(() => events.some((e) => e.event === 'pty:remoteSpawned' && e.args[0].ptyId === id), 'the desktop gets a tab')
  phone.send({ t: 'attach', id })
  const snap = await phone.take((m) => m.t === 'snapshot' && m.id === id, 'snapshot')
  assert.equal(typeof snap.data, 'string')
  await phone.take((m) => m.t === 'state' && m.sessions.some((s: any) => s.id === id), 'state lists it')
  const SECRET = 'typed-secret-value'
  phone.send({ t: 'input', id, data: `echo ${SECRET}\r` })
  await until(() => phone.inbox.some((m) => m.t === 'data' && m.d.includes(SECRET)), 'output arrives', 15000)
  phone.send({ t: 'input', id, data: 'x'.repeat(64 * 1024 + 1) })
  phone.send({ t: 'attach', id: 'no-such' })
  await phone.take((m) => m.t === 'error' && /session/.test(m.message), 'unknown session')
  phone.inbox.length = 0

  // An older phone page still sends resize: the desktop owns the size
  const sizeBefore = (await ipc('test:ptyInfo', id)).cols
  phone.send({ t: 'resize', id, cols: 30, rows: 12 })
  phone.send({ t: 'ping' })
  await phone.take((m) => m.t === 'pong', 'after resize')
  assert.equal((await ipc('test:ptyInfo', id)).cols, sizeBefore, 'a phone cannot change the desktop terminal size')

  // Preview: a dev server the terminal printed
  const dev = http.createServer((q, s) => {
    s.writeHead(200, { 'Content-Type': 'text/plain', 'X-Frame-Options': 'DENY' })
    s.end(`hello ${q.url} cookie=${q.headers.cookie ?? ''} host=${q.headers.host}`)
  })
  const hmr = new WebSocketServer({ server: dev })
  hmr.on('connection', (s) => s.on('message', (m) => s.send(`echo:${m}`)))
  await new Promise<void>((r) => dev.listen(0, '127.0.0.1', () => r()))
  const devPort = (dev.address() as net.AddressInfo).port
  phone.send({ t: 'preview', id })
  assert.match((await phone.take((m) => m.t === 'preview', 'no dev server yet')).error, /no dev server/)
  phone.send({ t: 'input', id, data: `echo http://localhost:${devPort}/\r` })
  await until(async () => (phone.send({ t: 'preview', id }), await phone.take((m) => m.t === 'preview', 'preview')).url != null, 'dev port detected', 15000)
  phone.send({ t: 'preview', id })
  const pv = await phone.take((m) => m.t === 'preview' && m.url, 'preview url')
  const purl = new URL(pv.url)
  assert.equal(purl.port, String(port + 2))
  const preview = (p: string, o: any = {}) => req('https', port + 2, p, o)
  assert.equal((await preview('/')).status, 403)
  assert.equal((await preview('/', { headers: { cookie: `aw_preview=${devPort}.bad` } })).status, 403)
  assert.equal((await preview('/__aw?t=forged')).status, 403)
  const swapped = await preview(purl.pathname + purl.search)
  assert.equal(swapped.status, 302)
  const setCookie = String(swapped.headers['set-cookie']?.[0])
  assert.ok(setCookie.includes('HttpOnly') && setCookie.includes('Secure') && setCookie.includes('SameSite=Lax'))
  const cookie = setCookie.split(';')[0]
  assert.equal((await preview(purl.pathname + purl.search)).status, 403, 'a ticket works once')
  assert.equal((await preview('/', { headers: { cookie: cookie.replace(String(devPort), String(devPort + 1)) } })).status, 403, 'the signature covers the port')
  const shown = await preview('/index.html?x=1', { headers: { cookie: `other=1; ${cookie}` } })
  assert.equal(shown.status, 200)
  assert.equal(shown.body.toString(), `hello /index.html?x=1 cookie= host=127.0.0.1:${devPort}`, 'our cookie stays here')
  assert.equal(shown.headers['x-frame-options'], undefined)
  const hws = new WebSocket(`wss://127.0.0.1:${port + 2}/hmr`, { rejectUnauthorized: false, headers: { cookie } })
  await new Promise<void>((res, rej) => hws.once('open', () => res()).once('error', rej))
  hws.send('ping')
  assert.equal(await new Promise((res) => hws.once('message', (m) => res(m.toString()))), 'echo:ping', 'HMR goes through')
  hws.close()
  const noCookieWs = new WebSocket(`wss://127.0.0.1:${port + 2}/hmr`, { rejectUnauthorized: false })
  assert.ok(await new Promise((res) => {
    noCookieWs.once('open', () => res(false))
    noCookieWs.once('error', () => res(true))
    noCookieWs.once('unexpected-response', () => res(true))
  }), 'no session, no upgrade')
  assert.equal((await preview('/', { headers: { cookie } })).status, 200)
  const p9 = await ipc('remote:createPairing')
  const other = JSON.parse((await api('/api/pair', { method: 'POST', body: JSON.stringify({ code: p9.code, name: 'Lost' }) })).body.toString())
  await ipc('remote:revokeDevice', other.deviceId)
  assert.equal((await preview('/', { headers: { cookie } })).status, 403, 'revoking a phone ends preview cookies')
  hmr.close()
  await new Promise<void>((r) => dev.close(() => r()))
  phone.inbox.length = 0
  console.log('terminal and preview ok')

  // Files over the socket
  phone.send({ t: 'files', windowId: 2, path: '' })
  const listed = await phone.take((m) => m.t === 'files', 'files')
  assert.ok(listed.entries.some((e: any) => e.name === 'README.md') && !listed.entries.some((e: any) => e.name === 'escape'))
  phone.send({ t: 'files', windowId: 2, path: '../outside' })
  assert.ok((await phone.take((m) => m.t === 'files', 'traversal')).error)
  phone.send({ t: 'files', windowId: 2, path: 'escape' })
  assert.ok((await phone.take((m) => m.t === 'files', 'junction')).error)
  phone.send({ t: 'files', windowId: 99, path: '' })
  assert.equal((await phone.take((m) => m.t === 'files', 'no window')).error, 'no workspace')
  phone.send({ t: 'file', windowId: 2, path: 'README.md' })
  const md = await phone.take((m) => m.t === 'file', 'markdown')
  assert.equal(md.kind, 'markdown')
  assert.equal(md.text, '# 手機唯讀預覽')
  phone.send({ t: 'file', windowId: 2, path: 'docs/view.html' })
  const htmlFile = await phone.take((m) => m.t === 'file', 'html')
  assert.equal(htmlFile.kind, 'html')
  assert.equal(htmlFile.text, null)
  const hp = new URL(htmlFile.url).pathname
  const html = await api(hp)
  assert.equal(html.status, 200)
  assert.match(String(html.headers['content-security-policy']), /sandbox allow-scripts/)
  assert.match(String(html.headers['content-security-policy']), /connect-src 'none'/)
  assert.equal(html.headers['access-control-allow-origin'], '*')
  const grant = hp.split('/').slice(0, 3).join('/')
  assert.match((await api(`${grant}/docs/theme.css`)).body.toString(), /teal/)
  assert.equal((await api(`${grant}/README.md`)).headers['content-type'], 'text/plain; charset=utf-8', 'an unknown type never gets sniffed')
  phone.send({ t: 'file', windowId: 2, path: 'docs/report.pdf' })
  const pdf = await phone.take((m) => m.t === 'file', 'pdf')
  const pp = new URL(pdf.url).pathname
  const part = await api(pp, { headers: { range: 'bytes=0-7' } })
  assert.equal(part.status, 206)
  assert.equal(part.headers['content-type'], 'application/pdf')
  assert.match(String(part.headers['content-range']), /^bytes 0-7\//)
  assert.equal(part.body.toString(), '%PDF-1.4')
  assert.equal((await api(pp, { headers: { range: 'bytes=99999-' } })).status, 416)
  assert.equal((await api(pp, { method: 'HEAD' })).status, 200)
  assert.equal((await api(hp, { method: 'POST', body: 'edit' })).status, 405)
  assert.ok(!fs.readFileSync(path.join(ws, 'docs', 'view.html'), 'utf8').includes('edit'))
  for (const bad of ['/escape/private.txt', '/..%2f..%2foutside%2fprivate.txt', '/%2e%2e/%2e%2e/outside/private.txt', '/..\\..\\outside\\private.txt']) {
    assert.equal((await api(grant + bad)).status, 404, bad)
  }
  assert.equal((await api('/_files/forged/README.md')).status, 403)
  phone.inbox.length = 0

  // Status, handoff, git
  fs.mkdirSync(path.join(ws, '.project-memory'))
  fs.writeFileSync(path.join(ws, '.project-memory', 'handoff.md'), 'handoff text')
  phone.send({ t: 'handoff', windowId: 2 })
  assert.equal((await phone.take((m) => m.t === 'handoff', 'handoff')).text, 'handoff text')
  await ipc('remote:vibeStatus', { workspace: ws, agentBusy: true, agentCount: 1, devUrl: null, changedFiles: [] })
  phone.send({ t: 'status', windowId: 2 })
  assert.equal((await phone.take((m) => m.t === 'status', 'status')).status.agentBusy, true)
  await ipc('remote:vibeStatus', { workspace: 'C:/other', agentBusy: false })
  phone.send({ t: 'status', windowId: 2 })
  assert.equal((await phone.take((m) => m.t === 'status', 'status')).status.agentBusy, true, 'a report about another folder is ignored')
  execFileSync('git', ['init', '-q', '-b', 'main'], { cwd: ws })
  phone.send({ t: 'git', windowId: 2 })
  const g = await phone.take((m) => m.t === 'git', 'git')
  assert.equal(g.branch, 'main')
  assert.ok(g.files.some((f: any) => f.path === 'README.md' && f.index === '?' && f.workingDir === '?'))

  // Cowork: an allowlist, and an audit line with no free text
  phone.send({ t: 'cowork', reqId: 1, windowId: 2, op: 'editBoard', args: [] })
  assert.equal((await phone.take((m) => m.t === 'cowork' && m.reqId === 1, 'blocked op')).result.code, 'not-allowed')
  phone.send({ t: 'cowork', reqId: 2, windowId: 2, op: 'capabilities', args: [] })
  assert.equal((await phone.take((m) => m.t === 'cowork' && m.reqId === 2, 'allowed op', 60000)).result.ok, true)
  phone.send({ t: 'cowork', reqId: 3, windowId: 99, op: 'list', args: [] })
  assert.equal((await phone.take((m) => m.t === 'cowork' && m.reqId === 3, 'no workspace')).result.code, 'no-workspace')
  phone.send({ t: 'cowork', reqId: 4, windowId: 2, op: 'delete', args: ['no-such-run'] })
  assert.equal((await phone.take((m) => m.t === 'cowork' && m.reqId === 4, 'audited op')).result.ok, false)
  assert.match(auditLog(), /"event":"cowork\.delete"/)

  // Opening a folder only from the desktop's own recent list
  phone.send({ t: 'openWorkspace', path: outside })
  assert.match((await phone.take((m) => m.t === 'error', 'unknown folder')).message, /not available/)

  // Bypass from the phone: everyone sees it
  phone.inbox.length = 0
  phone.send({ t: 'setBypass', enabled: true })
  assert.equal((await phone.take((m) => m.t === 'state', 'state after bypass')).bypass, true)
  assert.match(auditLog(), /"event":"bypass\.enable"/)
  phone.send({ t: 'setBypass', enabled: false })
  await until(() => auditLog().includes('bypass.disable'), 'bypass off')

  // Oversized message: the connection ends instead of the app
  const big = await Phone.connect()
  big.send({ t: 'auth', token })
  await big.take((m) => m.t === 'authed', 'authed')
  big.ws.send('x'.repeat(1024 * 1024 + 10))
  await big.closedWith()

  // Malformed input does nothing bad
  phone.ws.send('not json')
  phone.ws.send('null')
  phone.ws.send('[1,2]')
  phone.send({ t: 'attach' })
  phone.send({ t: 'input', id: id })
  phone.send({ t: 'nonsense' })
  phone.send({ t: 'ping' })
  await phone.take((m) => m.t === 'pong', 'still alive after garbage')

  // Kill ends it for the phone
  phone.inbox.length = 0
  phone.send({ t: 'kill', id })
  const gone = await phone.take((m) => m.t === 'exit' && m.id === id, 'exit')
  assert.equal(typeof gone.code, 'number')

  const log = auditLog()
  for (const e of ['bridge.start', 'ca.created', 'pair.fail', 'pair.ok', 'device.connect', 'session.spawn', 'session.kill', 'preview.open', 'device.revoke']) assert.ok(log.includes(`"${e}"`), e)
  assert.ok(!log.includes(SECRET), 'typed text never reaches the log')
  assert.ok(!log.includes(token), 'nor a token')

  // Revoking a phone: connection and file links end
  await ipc('remote:revokeDevice', deviceId)
  assert.equal(await phone.closedWith(), 4001)
  assert.equal((await api(hp)).status, 403, 'revoked device cannot reuse a preview URL')
  assert.equal((await api('/api/vapid', { headers: authBearer })).status, 401)
  console.log('sessions, files, cowork gate, audit and revocation ok')

  // Stop and start: same CA, so a phone stays trusted
  const fp = st.caFingerprint
  await ipc('remote:setEnabled', false)
  await assert.rejects(api('/'))
  const back = await ipc('remote:setEnabled', true)
  assert.equal(back.running, true)
  assert.equal(back.caFingerprint, fp, 'CA survives a restart')

  // An existing Electron install's data: its CA keeps working, unchanged
  await ipc('remote:setEnabled', false)
  const before = JSON.parse(fs.readFileSync(path.join(data, 'remote', 'secrets.json'), 'utf8'))
  fs.writeFileSync(path.join(data, 'remote', 'secrets.json'), JSON.stringify({ ...before, caCert: oldCa.certPem, caKey: { enc: 'plain', data: oldCa.keyPem }, serverCert: undefined, serverKey: undefined }))
  const reused = await ipc('remote:setEnabled', true)
  assert.equal(reused.running, true, JSON.stringify(reused))
  assert.equal(reused.caFingerprint, new crypto.X509Certificate(oldCa.certPem).fingerprint256, 'the old CA stays')
  if (reused.addresses[0]) {
    const r = await new Promise<string>((resolve) => {
      const s = tls.connect({ host: reused.addresses[0], port, ca: oldCa.certPem }, () => {
        s.end()
        resolve('ok')
      })
      s.on('error', (e) => resolve(e.message))
    })
    assert.equal(r, 'ok', 'a phone that trusts the old CA verifies the new server')
  }
  assert.ok(!fs.readFileSync(path.join(data, 'remote', 'secrets.json'), 'utf8').includes('PRIVATE KEY'), 'resealed on save')

  // A key that cannot be read is an error, not a reason to make a new CA
  await ipc('remote:setEnabled', false)
  const sealed = JSON.parse(fs.readFileSync(path.join(data, 'remote', 'secrets.json'), 'utf8'))
  fs.writeFileSync(path.join(data, 'remote', 'secrets.json'), JSON.stringify({ ...sealed, caKey: { enc: 'dpapi', data: 'AAAA' } }))
  const broken = await ipc('remote:setEnabled', true)
  assert.equal(broken.running, false)
  assert.match(broken.error, /decrypt/)
  assert.equal(JSON.parse(fs.readFileSync(path.join(data, 'remote', 'secrets.json'), 'utf8')).caCert, sealed.caCert, 'nothing replaced')
  fs.writeFileSync(path.join(data, 'remote', 'secrets.json'), JSON.stringify(sealed))

  // Reset trust: new CA, no devices
  const reset = await ipc('remote:resetTrust')
  assert.equal(reset.running, true)
  assert.notEqual(reset.caFingerprint, new crypto.X509Certificate(oldCa.certPem).fingerprint256)
  assert.equal((await ipc('remote:devices')).length, 0)

  // Port in use: an error shows, nothing else breaks
  await ipc('remote:setEnabled', false)
  const blocker = net.createServer()
  await new Promise<void>((r) => blocker.listen(port, '0.0.0.0', () => r()))
  const busy = await ipc('remote:setEnabled', true)
  assert.equal(busy.running, false)
  assert.ok(busy.error)
  await new Promise<void>((r) => blocker.close(() => r()))
  await ipc('remote:setEnabled', false)
  console.log('remote bridge ok')
} finally {
  child.stdin!.end()
  child.kill()
  await new Promise((r) => setTimeout(r, 300))
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 3 })
}
