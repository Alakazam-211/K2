//! The live database check (`k2 app db check`, prd-app-database-integrations-v1
//! §4.5, R6): log in as the App's own role and run `select 1, version()`,
//! timed, with no driver and no `psql` in the App's image.
//!
//! Two clients, one protocol:
//! - [`PROBE_JS`], run **inside a container App's wall** through the App
//!   engine's exec (`node -e`: every K2 App image is node24). It reads the
//!   App's own `DATABASE_URL` (or the `--as` name) from the App's
//!   environment (the exec's, else the App's PID 1), so the path it proves
//!   is the App's: pasta's `127.0.0.1:5432` forward, the per-App nft rule,
//!   the database door's admission, SCRAM end to end and the database. The
//!   URL never leaves the wall and never reaches argv.
//! - [`probe_url`], the same exchange from the daemon, for an App that has
//!   no wall to run it in (the process exception, the unwalled engine),
//!   with the URL the vault holds for it; and for the tests.
//!
//! Both print or answer one result: `ok`, the server's `version()`, the
//! milliseconds it took, and on failure the step (`no_door`,
//! `door_refused`, `auth_failed`, `db_missing`, `upstream_unreachable`,
//! `timeout`, `no_url`, `bad_url`, `error`) with the server's message.
//! Never the URL, the password or any SQL but the probe's own.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

/// The line prefix [`PROBE_JS`] prints its JSON result after.
pub const PROBE_MARK: &str = "K2PROBE ";

/// The in-wall check, one line of JavaScript (`node -e PROBE_JS <ENV>`):
/// startup (protocol 3.0, `sslmode=disable` inside the wall), cleartext,
/// MD5 or SCRAM-SHA-256 (the server's signature checked), then
/// `select 1, version()`. It prints `K2PROBE {json}` and exits 0 whatever
/// the result, so the daemon tells "the App's database failed" from "the
/// exec failed".
pub const PROBE_JS: &str = concat!(
    r#"const net=require('net'),cr=require('crypto'),fs=require('fs');"#,
    r#"const N=process.argv[1]||'DATABASE_URL';const t0=Date.now();"#,
    r#"const done=o=>{o.ms=Date.now()-t0;process.stdout.write('K2PROBE '+JSON.stringify(o)+'\n');process.exit(0)};"#,
    r#"let s=process.env[N];"#,
    r#"if(!s){try{const e=fs.readFileSync('/proc/1/environ','latin1').split('\0').find(x=>x.startsWith(N+'='));if(e)s=e.slice(N.length+1)}catch(_){}}"#,
    r#"if(!s)done({ok:false,step:'no_url',error:N+' is not in the App environment: re-host it (k2 app rehost)'});"#,
    r#"let u;try{u=new URL(s)}catch(_){done({ok:false,step:'bad_url',error:N+' is not a URL'})}"#,
    r#"const dc=x=>decodeURIComponent(x||'');const user=dc(u.username),pw=dc(u.password),db=dc(u.pathname.slice(1))||user;"#,
    r#"const so=net.connect(+(u.port||5432),u.hostname.replace(/^\[|\]$/g,''));"#,
    r#"so.setTimeout(10000,()=>done({ok:false,step:'timeout',error:'no answer within 10 s'}));"#,
    r#"so.on('error',e=>done({ok:false,step:e.code==='ECONNREFUSED'?'no_door':'upstream_unreachable',error:String(e.code||e)}));"#,
    r#"const z=x=>Buffer.from(x+'\0');"#,
    r#"const m=(t,b)=>{const h=Buffer.alloc(5);h[0]=t.charCodeAt(0);h.writeInt32BE(b.length+4,1);return Buffer.concat([h,b])};"#,
    r#"const H=(k,d)=>cr.createHmac('sha256',k).update(d).digest();"#,
    r#"const st=Buffer.concat([Buffer.from([0,3,0,0]),z('user'),z(user),z('database'),z(db),z('application_name'),z('k2-db-check'),Buffer.from([0])]);"#,
    r#"const L=Buffer.alloc(4);L.writeInt32BE(st.length+4);so.write(Buffer.concat([L,st]));"#,
    r#"let buf=Buffer.alloc(0),cfb='',sk=null,am='',q=false,ver=null;"#,
    r#"so.on('data',d=>{buf=Buffer.concat([buf,d]);while(buf.length>=5){const n=buf.readInt32BE(1);if(buf.length<n+1)return;const t=String.fromCharCode(buf[0]),b=buf.subarray(5,n+1);buf=buf.subarray(n+1);on(t,b)}});"#,
    r#"function on(t,b){"#,
    r#"if(t==='E'){const f={};let i=0;while(i<b.length&&b[i]){const j=b.indexOf(0,i+1);f[String.fromCharCode(b[i])]=b.toString('utf8',i+1,j);i=j+1}"#,
    r#"const M=f.M||'';done({ok:false,step:/\[k2:db_door_/.test(M)?'door_refused':(f.C==='28P01'||f.C==='28000')?'auth_failed':f.C==='3D000'?'db_missing':'error',error:((f.C||'')+' '+M).trim()})}"#,
    r#"if(t==='R'){const k=b.readInt32BE(0);"#,
    r#"if(k===3)return so.write(m('p',z(pw)));"#,
    r#"if(k===5){const a=cr.createHash('md5').update(pw+user).digest('hex');return so.write(m('p',z('md5'+cr.createHash('md5').update(a).update(b.subarray(4,8)).digest('hex'))))}"#,
    r#"if(k===10){cfb='n=,r='+cr.randomBytes(18).toString('base64');const c=Buffer.from('n,,'+cfb);const l=Buffer.alloc(4);l.writeInt32BE(c.length);return so.write(m('p',Buffer.concat([z('SCRAM-SHA-256'),l,c])))}"#,
    r#"if(k===11){const sf=b.subarray(4).toString();const a={};sf.split(',').forEach(p=>{a[p[0]]=p.slice(2)});"#,
    r#"const sp=cr.pbkdf2Sync(pw,Buffer.from(a.s,'base64'),+a.i,32,'sha256');const ck=H(sp,'Client Key');sk=H(sp,'Server Key');"#,
    r#"const w='c=biws,r='+a.r;am=cfb+','+sf+','+w;const sig=H(cr.createHash('sha256').update(ck).digest(),am);"#,
    r#"const pr=Buffer.alloc(32);for(let i=0;i<32;i++)pr[i]=ck[i]^sig[i];return so.write(m('p',Buffer.from(w+',p='+pr.toString('base64'))))}"#,
    r#"if(k===12){if(b.subarray(4).toString().replace(/^v=/,'')!==H(sk,am).toString('base64'))done({ok:false,step:'auth_failed',error:'the server signature did not match'});return}"#,
    r#"if(k!==0)done({ok:false,step:'auth_failed',error:'auth method '+k+' is not one the check speaks'});return}"#,
    r#"if(t==='D'){const nc=b.readInt16BE(0);let o=2;for(let c=0;c<nc;c++){const l=b.readInt32BE(o);o+=4;if(c===1&&l>=0)ver=b.toString('utf8',o,o+l);if(l>0)o+=l}return}"#,
    r#"if(t==='Z'){if(!q){q=true;return so.write(m('Q',z('select 1, version()')))}done({ok:true,version:ver})}"#,
    r#"}"#,
);

/// The exec argv for the in-wall check of `env_name`.
pub fn probe_argv(env_name: &str) -> Vec<String> {
    vec!["node".into(), "-e".into(), PROBE_JS.into(), env_name.into()]
}

/// One check's result (the JSON the CLI and `app_db_connections` keep).
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub ok: bool,
    pub step: Option<String>,
    pub version: Option<String>,
    pub ms: u64,
    pub error: Option<String>,
}

impl Probe {
    pub fn fail(step: &str, error: impl Into<String>) -> Probe {
        Probe { ok: false, step: Some(step.to_string()), version: None, ms: 0, error: Some(error.into()) }
    }
    pub fn to_json(&self) -> J {
        json!({ "ok": self.ok, "step": self.step, "version": self.version, "latencyMs": self.ms, "error": self.error })
    }
}

/// Read [`PROBE_JS`]'s answer out of an exec's output (the last marked
/// line). `None`: the check never printed one.
pub fn parse_marked(output: &str) -> Option<Probe> {
    let line = output.lines().rev().find_map(|l| l.trim_end().strip_prefix(PROBE_MARK))?;
    let v: J = serde_json::from_str(line).ok()?;
    let ok = v["ok"].as_bool()?;
    Some(Probe {
        ok,
        step: if ok { None } else { Some(v["step"].as_str().unwrap_or("error").to_string()) },
        version: v["version"].as_str().map(str::to_string),
        ms: v["ms"].as_u64().unwrap_or(0),
        error: v["error"].as_str().map(|e| scrub(e, 300)),
    })
}

/// A server message as K2 shows it: control characters out, bounded.
pub fn scrub(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

// ── the daemon-side client ───────────────────────────────────────────

/// The parts of a Postgres URL the check needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(x) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(x);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `postgres[ql]://user:password@host:port/database[?…]`.
pub fn parse_url(url: &str) -> Option<Target> {
    let rest = url.strip_prefix("postgres://").or_else(|| url.strip_prefix("postgresql://"))?;
    let rest = rest.split('#').next().unwrap_or("");
    let pre = rest.split('?').next().unwrap_or("");
    let at = pre.rfind('@')?;
    let (userinfo, hostpath) = (&pre[..at], &pre[at + 1..]);
    let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    let (hostport, path) = hostpath.split_once('/').unwrap_or((hostpath, ""));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h.trim_start_matches('[').trim_end_matches(']'), p.parse().ok()?),
        _ => (hostport.trim_start_matches('[').trim_end_matches(']'), 5432),
    };
    let user = pct_decode(user);
    let database = match pct_decode(path) {
        d if d.is_empty() => user.clone(),
        d => d,
    };
    Some(Target { host: host.to_string(), port, user, password: pct_decode(password), database })
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = Sha256::new();
    ipad.update(k.iter().map(|b| b ^ 0x36).collect::<Vec<u8>>());
    ipad.update(data);
    let inner = ipad.finalize();
    let mut opad = Sha256::new();
    opad.update(k.iter().map(|b| b ^ 0x5c).collect::<Vec<u8>>());
    opad.update(inner);
    opad.finalize().into()
}

fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut s = salt.to_vec();
    s.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(password, &s);
    let mut out = u;
    for _ in 1..iterations {
        u = hmac(password, &u);
        for (o, x) in out.iter_mut().zip(u.iter()) {
            *o ^= x;
        }
    }
    out
}

fn msg(t: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![t];
    v.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    v.extend_from_slice(body);
    v
}

fn cstr(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

fn read_msg(s: &mut TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
    let mut h = [0u8; 5];
    s.read_exact(&mut h)?;
    let n = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
    if !(4..=1 << 24).contains(&n) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "a server message of an impossible length"));
    }
    let mut b = vec![0u8; n - 4];
    s.read_exact(&mut b)?;
    Ok((h[0], b))
}

/// The fields of an `ErrorResponse`.
fn error_fields(b: &[u8]) -> (String, String) {
    let (mut code, mut text) = (String::new(), String::new());
    let mut i = 0;
    while i < b.len() && b[i] != 0 {
        let t = b[i];
        let end = b[i + 1..].iter().position(|c| *c == 0).map(|p| i + 1 + p).unwrap_or(b.len());
        let v = String::from_utf8_lossy(&b[i + 1..end]).into_owned();
        match t {
            b'C' => code = v,
            b'M' => text = v,
            _ => {}
        }
        i = end + 1;
    }
    (code, text)
}

fn step_of(code: &str, message: &str) -> &'static str {
    if message.contains("[k2:db_door_") {
        "door_refused"
    } else if code == "28P01" || code == "28000" {
        "auth_failed"
    } else if code == "3D000" {
        "db_missing"
    } else {
        "error"
    }
}

/// Run the check against `t` (the daemon's own network).
pub fn probe(t: &Target, timeout: Duration) -> Probe {
    let t0 = Instant::now();
    let mut p = run(t, timeout);
    p.ms = t0.elapsed().as_millis() as u64;
    p
}

/// [`probe`] on a URL.
pub fn probe_url(url: &str, timeout: Duration) -> Probe {
    match parse_url(url) {
        Some(t) => probe(&t, timeout),
        None => Probe::fail("bad_url", "the App's URL isn't a Postgres URL K2 can read"),
    }
}

fn run(t: &Target, timeout: Duration) -> Probe {
    let addr = format!("{}:{}", t.host, t.port);
    let sock = match std::net::ToSocketAddrs::to_socket_addrs(&addr).ok().and_then(|mut a| a.next()) {
        Some(a) => a,
        None => return Probe::fail("upstream_unreachable", format!("{} doesn't resolve", t.host)),
    };
    let mut s = match TcpStream::connect_timeout(&sock, timeout) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => return Probe::fail("no_door", "connection refused"),
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return Probe::fail("timeout", "no answer in time"),
        Err(e) => return Probe::fail("upstream_unreachable", e.kind().to_string()),
    };
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));
    let _ = s.set_nodelay(true);
    let io_fail = |e: std::io::Error| -> Probe {
        if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
            Probe::fail("timeout", "no answer in time")
        } else {
            Probe::fail("error", format!("the connection ended: {}", e.kind()))
        }
    };
    let mut body = 196_608u32.to_be_bytes().to_vec();
    for (k, v) in [("user", t.user.as_str()), ("database", t.database.as_str()), ("application_name", "k2-db-check")] {
        body.extend(cstr(k));
        body.extend(cstr(v));
    }
    body.push(0);
    let mut start = ((body.len() + 4) as u32).to_be_bytes().to_vec();
    start.extend(body);
    if let Err(e) = s.write_all(&start) {
        return io_fail(e);
    }
    let (mut client_first_bare, mut server_key, mut auth_message) = (String::new(), [0u8; 32], String::new());
    let mut asked = false;
    let mut version: Option<String> = None;
    loop {
        let (ty, b) = match read_msg(&mut s) {
            Ok(m) => m,
            Err(e) => return io_fail(e),
        };
        match ty {
            b'E' => {
                let (code, text) = error_fields(&b);
                return Probe::fail(step_of(&code, &text), scrub(format!("{code} {text}").trim(), 300));
            }
            b'R' if b.len() >= 4 => {
                let kind = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
                let out = match kind {
                    0 => None,
                    3 => Some(msg(b'p', &cstr(&t.password))),
                    5 if b.len() >= 8 => {
                        let inner = format!("{:x}", md5_hex_input(&format!("{}{}", t.password, t.user)));
                        let mut x = inner.into_bytes();
                        x.extend_from_slice(&b[4..8]);
                        Some(msg(b'p', &cstr(&format!("md5{:x}", md5_hex_input_bytes(&x)))))
                    }
                    10 => {
                        let mut nonce = [0u8; 18];
                        if getrandom::getrandom(&mut nonce).is_err() {
                            return Probe::fail("error", "no random nonce");
                        }
                        client_first_bare = format!("n=,r={}", base64::engine::general_purpose::STANDARD.encode(nonce));
                        let first = format!("n,,{client_first_bare}");
                        let mut p = cstr("SCRAM-SHA-256");
                        p.extend_from_slice(&(first.len() as u32).to_be_bytes());
                        p.extend_from_slice(first.as_bytes());
                        Some(msg(b'p', &p))
                    }
                    11 => {
                        let server_first = String::from_utf8_lossy(&b[4..]).into_owned();
                        let get = |k: char| server_first.split(',').find_map(|p| p.strip_prefix(&format!("{k}=")).map(str::to_string));
                        let (Some(r), Some(salt), Some(iters)) = (get('r'), get('s'), get('i').and_then(|i| i.parse::<u32>().ok())) else {
                            return Probe::fail("auth_failed", "an unreadable SCRAM challenge");
                        };
                        let Ok(salt) = base64::engine::general_purpose::STANDARD.decode(salt) else {
                            return Probe::fail("auth_failed", "an unreadable SCRAM salt");
                        };
                        let salted = pbkdf2(t.password.as_bytes(), &salt, iters.clamp(1, 1_000_000));
                        let client_key = hmac(&salted, b"Client Key");
                        server_key = hmac(&salted, b"Server Key");
                        let stored: [u8; 32] = Sha256::digest(client_key).into();
                        let without_proof = format!("c=biws,r={r}");
                        auth_message = format!("{client_first_bare},{server_first},{without_proof}");
                        let sig = hmac(&stored, auth_message.as_bytes());
                        let proof: Vec<u8> = client_key.iter().zip(sig.iter()).map(|(a, b)| a ^ b).collect();
                        let fin = format!("{without_proof},p={}", base64::engine::general_purpose::STANDARD.encode(proof));
                        Some(msg(b'p', fin.as_bytes()))
                    }
                    12 => {
                        let v = String::from_utf8_lossy(&b[4..]).into_owned();
                        let want = base64::engine::general_purpose::STANDARD.encode(hmac(&server_key, auth_message.as_bytes()));
                        if v.strip_prefix("v=") != Some(want.as_str()) {
                            return Probe::fail("auth_failed", "the server's SCRAM signature didn't match");
                        }
                        None
                    }
                    other => return Probe::fail("auth_failed", format!("auth method {other} is not one the check speaks")),
                };
                if let Some(o) = out {
                    if let Err(e) = s.write_all(&o) {
                        return io_fail(e);
                    }
                }
            }
            b'D' if b.len() >= 2 => {
                let n = u16::from_be_bytes([b[0], b[1]]) as usize;
                let mut o = 2;
                for c in 0..n {
                    if o + 4 > b.len() {
                        break;
                    }
                    let l = i32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
                    o += 4;
                    if l > 0 {
                        let end = (o + l as usize).min(b.len());
                        if c == 1 {
                            version = Some(scrub(&String::from_utf8_lossy(&b[o..end]), 200));
                        }
                        o = end;
                    }
                }
            }
            b'Z' => {
                if !asked {
                    asked = true;
                    if let Err(e) = s.write_all(&msg(b'Q', &cstr("select 1, version()"))) {
                        return io_fail(e);
                    }
                } else {
                    let _ = s.write_all(&msg(b'X', b""));
                    return Probe { ok: true, step: None, version, ms: 0, error: None };
                }
            }
            _ => {}
        }
    }
}

// MD5 (RFC 1321) for the md5 auth method only: a few old clusters still
// hold md5 passwords. Small and local, no new crate.
fn md5_hex_input(s: &str) -> Md5Out {
    md5_hex_input_bytes(s.as_bytes())
}

pub struct Md5Out([u8; 16]);

impl std::fmt::LowerHex for Md5Out {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

fn md5_hex_input_bytes(input: &[u8]) -> Md5Out {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16,
        23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut m = input.to_vec();
    let bits = (input.len() as u64).wrapping_mul(8);
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&bits.to_le_bytes());
    for chunk in m.chunks(64) {
        let w: Vec<u32> = chunk.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(w[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, v) in [a0, b0, c0, d0].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    Md5Out(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_in_wall_check_fits_one_exec_argument() {
        let argv = probe_argv("DATABASE_URL");
        assert!(k2_root_proto::engine::exec_argv_ok(&argv), "PROBE_JS is {} bytes, one line, no control characters", PROBE_JS.len());
        assert!(!PROBE_JS.contains("process.env.DATABASE_URL") && PROBE_JS.contains("process.env[N]"), "the name comes from argv");
    }

    #[test]
    fn urls_parse_without_ever_printing_them() {
        let t = parse_url("postgres://a_app_x__g1:p%40ss@127.0.0.1:5432/ws_db?sslmode=disable").unwrap();
        assert_eq!(t, Target { host: "127.0.0.1".into(), port: 5432, user: "a_app_x__g1".into(), password: "p@ss".into(), database: "ws_db".into() });
        let t = parse_url("postgresql://u:p@[::1]:5433/d").unwrap();
        assert_eq!((t.host.as_str(), t.port), ("::1", 5433));
        assert!(parse_url("mysql://u:p@h/d").is_none());
        let p = probe_url("not a url", Duration::from_millis(10));
        assert_eq!(p.step.as_deref(), Some("bad_url"));
    }

    #[test]
    fn hmac_pbkdf2_and_md5_match_their_published_vectors() {
        // RFC 4231 test case 2.
        let h = hmac(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex(&h), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        // PBKDF2-HMAC-SHA256, "password"/"salt", 1 and 2 iterations.
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 1)), "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 2)), "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43");
        // RFC 1321.
        assert_eq!(format!("{:x}", md5_hex_input("")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(format!("{:x}", md5_hex_input("The quick brown fox jumps over the lazy dog")), "9e107d9d372bb6826bd81d3542a419d6");
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn the_checks_marked_line_is_read_back() {
        let out = "node: a warning\nK2PROBE {\"ok\":true,\"version\":\"PostgreSQL 17.6\",\"ms\":3}\n";
        let p = parse_marked(out).unwrap();
        assert!(p.ok && p.version.as_deref() == Some("PostgreSQL 17.6") && p.ms == 3, "{p:?}");
        let p = parse_marked("K2PROBE {\"ok\":false,\"step\":\"door_refused\",\"error\":\"28000 K2 database door: \\u001b[31mno\"}").unwrap();
        assert_eq!(p.step.as_deref(), Some("door_refused"));
        assert!(!p.error.unwrap().contains('\u{1b}'), "control characters out");
        assert!(parse_marked("nothing here").is_none());
        assert_eq!(step_of("28000", "K2 database door: x [k2:db_door_role_refused]"), "door_refused");
        assert_eq!(step_of("28P01", "password authentication failed"), "auth_failed");
        assert_eq!(step_of("3D000", "database \"x\" does not exist"), "db_missing");
    }
}
