// The local CA and TLS certificates. iOS only runs a home-screen app, its service worker and Web Push over trusted HTTPS, so the
// desktop acts as a CA the phone trusts once, then re-issues the server certificate whenever the LAN address changes.
//
// A CA a phone trusts could sign anything if it leaked, so it carries critical Name Constraints: private IPv4 and *.local only
// (plus 100.64/10 when mesh VPN is on). It stays compatible with what the Electron app already installed on phones.
use std::net::IpAddr;

use base64::Engine;
use rand::RngCore;
use rcgen::{BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints, SanType, SerialNumber};
use regex::Regex;
use rsa::pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey, LineEnding};
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use rsa::RsaPrivateKey;
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use url::Url;

#[derive(Clone, Debug)]
pub struct CertPair {
    pub cert_pem: String,
    pub key_pem: String,
}

type Range = ([u8; 4], [u8; 4]);

// RFC 1918 plus link-local: all a LAN will show
const PRIVATE_V4: [Range; 4] = [([10, 0, 0, 0], [255, 0, 0, 0]), ([172, 16, 0, 0], [255, 240, 0, 0]), ([192, 168, 0, 0], [255, 255, 0, 0]), ([169, 254, 0, 0], [255, 255, 0, 0])];
// RFC 6598: NordVPN Meshnet and Tailscale, and also carrier NAT, so being in this range alone proves nothing (is_mesh_connection)
const MESH_V4: Range = ([100, 64, 0, 0], [255, 192, 0, 0]);

fn in_range(ip: &str, (net, mask): &Range) -> bool {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().enumerate().all(|(i, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u32>().is_ok_and(|x| x & mask[i] as u32 == net[i] as u32))
}

pub fn is_private_ipv4(ip: &str) -> bool {
    PRIVATE_V4.iter().any(|r| in_range(ip, r))
}

pub fn is_mesh_ipv4(ip: &str) -> bool {
    in_range(ip, &MESH_V4)
}

fn strip_mapped(a: &str) -> &str {
    a.strip_prefix("::ffff:").unwrap_or(a)
}

pub fn is_allowed_remote(addr: &str) -> bool {
    let a = strip_mapped(addr);
    a == "127.0.0.1" || a == "::1" || is_private_ipv4(a)
}

/// The peer has to be in the mesh range and have reached this machine's mesh address. A neighbour on the same carrier NAT reaches
/// the public interface instead; only a device inside the tunnel can reach the mesh address.
pub fn is_mesh_connection(remote: &str, local: &str, mesh_addrs: &[String]) -> bool {
    is_mesh_ipv4(strip_mapped(remote)) && mesh_addrs.iter().any(|m| m == strip_mapped(local))
}

/// Our own app: the page served from here, or the same app on another computer on the LAN (the phone switches computers within one
/// page, so its WebSocket is cross-origin). Passing only earns a connection, which still has five seconds to show a valid token.
pub fn is_allowed_origin(origin: Option<&str>, host: &str, allow_mesh: bool) -> bool {
    let Some(origin) = origin.filter(|o| !o.is_empty()) else { return false };
    if origin == format!("https://{host}") {
        return true;
    }
    let Ok(u) = Url::parse(origin) else { return false };
    if u.scheme() != "https" {
        return false;
    }
    let h = u.host_str().unwrap_or("").trim_start_matches('[').trim_end_matches(']');
    is_private_ipv4(h) || (allow_mesh && is_mesh_ipv4(h)) || Regex::new(r"(?i)^[a-z0-9][a-z0-9-]*\.local$").unwrap().is_match(h)
}

fn interfaces() -> Vec<(String, String)> {
    if_addrs::get_if_addrs().unwrap_or_default().into_iter().filter(|i| !i.is_loopback()).filter_map(|i| match i.ip() {
        IpAddr::V4(ip) => Some((i.name, ip.to_string())),
        IpAddr::V6(_) => None,
    }).collect()
}

/// ponytail: only these two by adapter name; add others when someone uses them. A 100.64/10 address on any other adapter is carrier NAT.
pub fn mesh_from(list: &[(String, String)]) -> Vec<String> {
    let name = Regex::new(r"(?i)nordlynx|tailscale").unwrap();
    list.iter().filter(|(n, ip)| name.is_match(n) && is_mesh_ipv4(ip)).map(|(_, ip)| ip.clone()).collect()
}

pub fn mesh_addresses() -> Vec<String> {
    mesh_from(&interfaces())
}

/// Every private IPv4 here, 192.168 first since that is usually home Wi-Fi
pub fn lan_addresses() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, ip) in interfaces() {
        if is_private_ipv4(&ip) && !ip.starts_with("169.254.") && !out.contains(&ip) {
            out.push(ip);
        }
    }
    out.sort_by_key(|ip| if ip.starts_with("192.168.") { 0 } else if ip.starts_with("10.") { 1 } else { 2 });
    out
}

pub fn os_hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// A name that fits *.local; anything odd is left out and the certificate relies on addresses
pub fn local_hostname() -> Option<String> {
    let h = os_hostname();
    let h = h.split('.').next().unwrap_or("").to_lowercase();
    Regex::new(r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$").unwrap().is_match(&h).then(|| format!("{h}.local"))
}

fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    assert!(body.len() < 128, "short-form length only");
    let mut out = vec![tag, body.len() as u8];
    out.extend_from_slice(body);
    out
}

/// The exact bytes of the extension value, as phones already have it
pub fn name_constraints_der(mesh: bool) -> Vec<u8> {
    let mut subtrees = Vec::new();
    for (net, mask) in PRIVATE_V4.iter().chain(mesh.then_some(&MESH_V4)) {
        let mut ip = net.to_vec();
        ip.extend_from_slice(mask);
        subtrees.extend(tlv(0x30, &tlv(0x87, &ip)));
    }
    subtrees.extend(tlv(0x30, &tlv(0x82, b"local")));
    tlv(0x30, &tlv(0xa0, &subtrees))
}

pub fn cert_der(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem.lines().map(str::trim).filter(|l| !l.starts_with("-----")).collect();
    base64::engine::general_purpose::STANDARD.decode(body).map_err(|e| e.to_string())
}

/// A CA from an older version may not allow the mesh range, and a phone cannot change that: it needs a new CA
pub fn ca_permits_mesh(ca_pem: &str) -> bool {
    let needle = name_constraints_der(true);
    cert_der(ca_pem).is_ok_and(|der| der.windows(needle.len()).any(|w| w == needle))
}

fn random_serial() -> SerialNumber {
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b[0] &= 0x7f; // positive
    SerialNumber::from_slice(&b)
}

/// RSA, because that is what the phones' existing chain uses and iOS is the one verifier that cannot be tested here.
/// The rsa crate only generates; signing goes through ring.
fn new_rsa() -> Result<(RsaPrivateKey, KeyPair), String> {
    let key = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).map_err(|e| e.to_string())?;
    let pair = signing_key(&key)?;
    Ok((key, pair))
}

fn signing_key(key: &RsaPrivateKey) -> Result<KeyPair, String> {
    let der = key.to_pkcs8_der().map_err(|e| e.to_string())?;
    KeyPair::try_from(der.as_bytes()).map_err(|e| e.to_string())
}

/// PKCS#1 on disk, same as before, so a rollback to an older version can still read it
fn key_pem(key: &RsaPrivateKey) -> Result<String, String> {
    key.to_pkcs1_pem(LineEnding::LF).map(|s| s.to_string()).map_err(|e| e.to_string())
}

fn read_key(pem: &str) -> Result<RsaPrivateKey, String> {
    RsaPrivateKey::from_pkcs1_pem(pem).or_else(|_| RsaPrivateKey::from_pkcs8_pem(pem)).map_err(|e| e.to_string())
}

pub fn create_ca(label: &str, mesh: bool) -> Result<CertPair, String> {
    let (key, pair) = new_rsa()?;
    let mut p = CertificateParams::default();
    p.serial_number = Some(random_serial());
    let now = OffsetDateTime::now_utc();
    p.not_before = now - Duration::seconds(60);
    p.not_after = now + Duration::days(5 * 365);
    let mut dn = DistinguishedName::new();
    // The mesh one gets another name, so the phone shows which one is old and can go
    dn.push(DnType::CommonName, format!("Agent Workbench Local CA{} ({label})", if mesh { " + VPN" } else { "" }));
    dn.push(DnType::OrganizationName, "Agent Workbench");
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let mut permitted: Vec<GeneralSubtree> = PRIVATE_V4.iter().chain(mesh.then_some(&MESH_V4)).map(|(n, m)| GeneralSubtree::IpAddress(CidrSubnet::V4(*n, *m))).collect();
    permitted.push(GeneralSubtree::DnsName("local".into()));
    p.name_constraints = Some(NameConstraints { permitted_subtrees: permitted, excluded_subtrees: vec![] });
    let cert = p.self_signed(&pair).map_err(|e| e.to_string())?;
    Ok(CertPair { cert_pem: cert.pem(), key_pem: key_pem(&key)? })
}

/// What iOS 13+ asks of a TLS certificate: RSA 2048, SHA-256, SAN, EKU serverAuth, at most 825 days (397 here)
pub fn issue_server_cert(ca: &CertPair, ips: &[String], dns_names: &[String]) -> Result<CertPair, String> {
    let ca_key = signing_key(&read_key(&ca.key_pem)?)?;
    // Keeps the CA's subject bytes and key identifier as they are, including one the old app issued
    let issuer = Issuer::from_ca_cert_pem(&ca.cert_pem, ca_key).map_err(|e| e.to_string())?;
    let (key, pair) = new_rsa()?;
    let mut p = CertificateParams::default();
    p.serial_number = Some(random_serial());
    let now = OffsetDateTime::now_utc();
    p.not_before = now - Duration::seconds(60);
    p.not_after = now + Duration::days(397);
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Agent Workbench Remote");
    p.distinguished_name = dn;
    p.is_ca = IsCa::ExplicitNoCa;
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    p.use_authority_key_identifier_extension = true;
    for ip in ips {
        p.subject_alt_names.push(SanType::IpAddress(ip.parse().map_err(|_| format!("bad address {ip}"))?));
    }
    for name in dns_names {
        p.subject_alt_names.push(SanType::DnsName(name.as_str().try_into().map_err(|_| format!("bad name {name}"))?));
    }
    let cert = p.signed_by(&pair, &issuer).map_err(|e| e.to_string())?;
    Ok(CertPair { cert_pem: cert.pem(), key_pem: key_pem(&key)? })
}

/// Does the certificate still name these addresses, with over 30 days left? Compared entry by entry: a prefix match would take
/// .10 for .1, skip the re-issue, and the phone would then fail with a name mismatch.
pub fn cert_covers(cert_pem: &str, ips: &[String], dns_names: &[String]) -> bool {
    use x509_parser::prelude::*;
    let Ok(der) = cert_der(cert_pem) else { return false };
    let Ok((_, x)) = X509Certificate::from_der(&der) else { return false };
    if x.validity().not_after.timestamp() - OffsetDateTime::now_utc().unix_timestamp() < 30 * 24 * 3600 {
        return false;
    }
    let Ok(Some(san)) = x.subject_alternative_name() else { return ips.is_empty() && dns_names.is_empty() };
    let mut have_ip = Vec::new();
    let mut have_dns = Vec::new();
    for name in &san.value.general_names {
        match name {
            GeneralName::IPAddress(b) if b.len() == 4 => have_ip.push(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])),
            GeneralName::DNSName(d) => have_dns.push(d.to_string()),
            _ => {}
        }
    }
    ips.iter().all(|i| have_ip.contains(i)) && dns_names.iter().all(|d| have_dns.contains(d))
}

/// SHA-256 as AA:BB:…, what the settings page shows and the setup page repeats so they can be compared
pub fn fingerprint256(cert_pem: &str) -> Option<String> {
    let der = cert_der(cert_pem).ok()?;
    Some(Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        for ok in ["192.168.1.20", "10.1.2.3", "172.31.0.1", "169.254.0.1"] {
            assert!(is_private_ipv4(ok), "{ok}");
        }
        for no in ["172.32.0.1", "8.8.8.8", "100.64.0.1", "", "1.2.3", "a.b.c.d", "192.168.1.1.1", "::1"] {
            assert!(!is_private_ipv4(no), "{no}");
        }
        assert!(is_allowed_remote("::ffff:192.168.1.1") && is_allowed_remote("::1") && !is_allowed_remote("8.8.8.8") && !is_allowed_remote(""));
        let mesh = vec!["100.64.1.2".to_string()];
        assert!(is_mesh_connection("100.64.9.9", "100.64.1.2", &mesh));
        assert!(!is_mesh_connection("100.64.9.9", "203.0.113.5", &mesh), "carrier NAT neighbour");
        assert!(!is_mesh_connection("8.8.8.8", "100.64.1.2", &mesh));
    }

    #[test]
    fn mesh_needs_the_adapter_name() {
        let list = |n: &str, ip: &str| vec![(n.to_string(), ip.to_string())];
        assert_eq!(mesh_from(&list("NordLynx", "100.70.0.1")), ["100.70.0.1"]);
        assert_eq!(mesh_from(&list("Tailscale", "100.70.0.1")), ["100.70.0.1"]);
        assert!(mesh_from(&list("Ethernet", "100.70.0.1")).is_empty());
        assert!(mesh_from(&list("NordLynx", "10.0.0.1")).is_empty());
    }

    #[test]
    fn origins() {
        let host = "192.168.1.2:47600";
        assert!(is_allowed_origin(Some("https://192.168.1.2:47600"), host, false));
        assert!(is_allowed_origin(Some("https://10.0.0.5:47600"), host, false));
        assert!(is_allowed_origin(Some("https://desk.local:47600"), host, false));
        for bad in ["http://192.168.1.2:47600", "https://evil.com", "https://192.168.1.2@evil.com", "https://evil.local.com", "null", ""] {
            assert!(!is_allowed_origin(Some(bad), host, false), "{bad}");
        }
        assert!(!is_allowed_origin(None, host, false));
        assert!(!is_allowed_origin(Some("https://100.70.0.1:47600"), host, false));
        assert!(is_allowed_origin(Some("https://100.70.0.1:47600"), host, true));
    }

    #[test]
    fn a_new_ca_carries_the_constraints_a_phone_expects() {
        let ca = create_ca("t", false).unwrap();
        assert!(!ca_permits_mesh(&ca.cert_pem));
        let needle = name_constraints_der(false);
        assert!(cert_der(&ca.cert_pem).unwrap().windows(needle.len()).any(|w| w == needle));
        assert!(ca_permits_mesh(&create_ca("t", true).unwrap().cert_pem));
        let leaf = issue_server_cert(&ca, &["192.168.50.70".to_string()], &["box.local".to_string()]).unwrap();
        assert!(cert_covers(&leaf.cert_pem, &["192.168.50.70".to_string()], &["box.local".to_string()]));
        assert!(!cert_covers(&leaf.cert_pem, &["192.168.50.7".to_string()], &[]), "a prefix is not a match");
        assert!(!cert_covers(&leaf.cert_pem, &[], &["other.local".to_string()]));
    }
}
