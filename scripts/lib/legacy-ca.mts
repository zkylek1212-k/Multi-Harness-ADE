// How the Electron versions made their CA and server certificates. Phones in the field trust CAs made exactly like this, so the
// Rust code has to keep working with them. Test only; generated fresh each run, nothing stored.
import * as crypto from 'node:crypto'
import forge from 'node-forge'

type Pair = { certPem: string; keyPem: string }
const PRIVATE: [number[], number[]][] = [[[10, 0, 0, 0], [255, 0, 0, 0]], [[172, 16, 0, 0], [255, 240, 0, 0]], [[192, 168, 0, 0], [255, 255, 0, 0]], [[169, 254, 0, 0], [255, 255, 0, 0]]]
const MESH: [number[], number[]] = [[100, 64, 0, 0], [255, 192, 0, 0]]

function keys(): forge.pki.rsa.KeyPair {
  const { privateKey } = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 })
  const priv = forge.pki.privateKeyFromPem(privateKey.export({ type: 'pkcs1', format: 'pem' }) as string) as forge.pki.rsa.PrivateKey
  return { privateKey: priv, publicKey: forge.pki.setRsaPublicKey(priv.n, priv.e) }
}

function serial(): string {
  const b = crypto.randomBytes(16)
  b[0] &= 0x7f
  return b.toString('hex')
}

function constraints(mesh: boolean): string {
  const { asn1 } = forge
  const sub = (name: forge.asn1.Asn1): forge.asn1.Asn1 => asn1.create(asn1.Class.UNIVERSAL, asn1.Type.SEQUENCE, true, [name])
  const permitted = asn1.create(asn1.Class.CONTEXT_SPECIFIC, 0, true, [
    ...[...PRIVATE, ...(mesh ? [MESH] : [])].map(([n, m]) => sub(asn1.create(asn1.Class.CONTEXT_SPECIFIC, 7, false, String.fromCharCode(...n, ...m)))),
    sub(asn1.create(asn1.Class.CONTEXT_SPECIFIC, 2, false, 'local'))
  ])
  return asn1.toDer(asn1.create(asn1.Class.UNIVERSAL, asn1.Type.SEQUENCE, true, [permitted])).getBytes()
}

export function createCa(label: string, mesh = false): Pair {
  const k = keys()
  const cert = forge.pki.createCertificate()
  cert.publicKey = k.publicKey
  cert.serialNumber = serial()
  cert.validity.notBefore = new Date(Date.now() - 60_000)
  cert.validity.notAfter = new Date(Date.now() + 5 * 365 * 24 * 3600_000)
  const subject = [{ name: 'commonName', value: `Agent Workbench Local CA${mesh ? ' + VPN' : ''} (${label})` }, { name: 'organizationName', value: 'Agent Workbench' }]
  cert.setSubject(subject)
  cert.setIssuer(subject)
  cert.setExtensions([{ name: 'basicConstraints', cA: true, pathLenConstraint: 0, critical: true }, { name: 'keyUsage', keyCertSign: true, cRLSign: true, critical: true }, { name: 'subjectKeyIdentifier' }, { id: '2.5.29.30', critical: true, value: constraints(mesh) }])
  cert.sign(k.privateKey, forge.md.sha256.create())
  return { certPem: forge.pki.certificateToPem(cert), keyPem: forge.pki.privateKeyToPem(k.privateKey) }
}

export function issueServerCert(ca: Pair, ips: string[], dnsNames: string[]): Pair {
  const caCert = forge.pki.certificateFromPem(ca.certPem)
  const caKey = forge.pki.privateKeyFromPem(ca.keyPem)
  const k = keys()
  const cert = forge.pki.createCertificate()
  cert.publicKey = k.publicKey
  cert.serialNumber = serial()
  cert.validity.notBefore = new Date(Date.now() - 60_000)
  cert.validity.notAfter = new Date(Date.now() + 397 * 24 * 3600_000)
  cert.setSubject([{ name: 'commonName', value: 'Agent Workbench Remote' }])
  cert.setIssuer(caCert.subject.attributes)
  cert.setExtensions([{ name: 'basicConstraints', cA: false, critical: true }, { name: 'keyUsage', digitalSignature: true, keyEncipherment: true, critical: true }, { name: 'extKeyUsage', serverAuth: true }, { name: 'subjectAltName', altNames: [...ips.map((ip) => ({ type: 7, ip })), ...dnsNames.map((value) => ({ type: 2, value }))] }, { name: 'subjectKeyIdentifier' }, { name: 'authorityKeyIdentifier', keyIdentifier: caCert.generateSubjectKeyIdentifier().getBytes() }])
  cert.sign(caKey, forge.md.sha256.create())
  return { certPem: forge.pki.certificateToPem(cert), keyPem: forge.pki.privateKeyToPem(k.privateKey) }
}

/** A VAPID key as Node made it: phones are subscribed against these */
export function legacyVapid(): { publicKey: string; privateKeyPem: string } {
  const { publicKey, privateKey } = crypto.generateKeyPairSync('ec', { namedCurve: 'prime256v1' })
  const jwk = publicKey.export({ format: 'jwk' })
  const raw = Buffer.concat([Buffer.from([0x04]), Buffer.from(jwk.x as string, 'base64url'), Buffer.from(jwk.y as string, 'base64url')])
  return { publicKey: raw.toString('base64url'), privateKeyPem: privateKey.export({ type: 'pkcs8', format: 'pem' }) as string }
}
