//! `k5 fed pair-start`, `pair-join`, `pair-finish` and `pair-verify`: the
//! pairing tokens as the shell's two Python blocks read and write them
//! (`cli/src/federation.sh`).
//!
//! A token is a prefix (`k2pair1:`, `k2pair2:`, `k2pair3:`; wire values,
//! the same in every brand) and the URL-safe base64 of a compact JSON
//! value. The reading side decodes it as CPython's `binascii.a2b_base64`
//! does when not strict (bytes outside the alphabet skipped, the first
//! full pad ends it) and words each failure as Python's exception does.

use super::super::{py_space, raised, Raise, Shown};
use crate::cards::Decode;
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use k2_client::cli_name;

/// The token step's prefix and the key its object is sent under.
pub fn want(step: &str) -> Option<(&'static str, &'static str)> {
    match step {
        "pair-join" => Some(("k2pair1:", "offer")),
        "pair-finish" => Some(("k2pair2:", "answer")),
        "pair-verify" => Some(("k2pair3:", "reveal")),
        _ => None,
    }
}

/// The token as the shell's Python reads it from stdin: bytes, decoded
/// by the locale (`Decode`). `Err`: the traceback of
/// `sys.stdin.read()` (line 2), which `|| exit 2` turns into exit 2.
pub fn decode_stdin(bytes: &[u8], decode: Decode) -> Result<Vec<u32>, Shown> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.chars().map(u32::from).collect()),
        Err(_) if decode == Decode::Lossy => Ok(escaped(bytes)),
        Err(e) => {
            let mut shown = raised(
                String::new(),
                String::new(),
                &Raise::at(2, format!("UnicodeDecodeError: {}", utf8_error(bytes, &e))),
            );
            shown.exit = Exit::Usage;
            Err(shown)
        }
    }
}

/// `surrogateescape`: each byte that is not part of UTF-8 is U+DC00 plus
/// the byte.
fn escaped(bytes: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.extend(s.chars().map(u32::from));
                break;
            }
            Err(e) => {
                let n = e.valid_up_to();
                out.extend(
                    std::str::from_utf8(&rest[..n])
                        .unwrap_or_default()
                        .chars()
                        .map(u32::from),
                );
                let bad = e.error_len().unwrap_or(rest.len() - n);
                out.extend(rest[n..n + bad].iter().map(|b| 0xdc00 + u32::from(*b)));
                rest = &rest[n + bad..];
            }
        }
    }
    out
}

/// `str(e)` of the `UnicodeDecodeError` Python's strict UTF-8 decoder
/// raises on `bytes`, at the first bad place.
pub fn utf8_error(bytes: &[u8], e: &std::str::Utf8Error) -> String {
    let at = e.valid_up_to();
    let b = bytes[at];
    match e.error_len() {
        Some(_) => {
            let why = if (0x80..=0xc1).contains(&b) || b >= 0xf5 {
                "invalid start byte"
            } else {
                "invalid continuation byte"
            };
            format!("'utf-8' codec can't decode byte 0x{b:02x} in position {at}: {why}")
        }
        None if bytes.len() - at == 1 => format!(
            "'utf-8' codec can't decode byte 0x{b:02x} in position {at}: unexpected end of data"
        ),
        None => format!(
            "'utf-8' codec can't decode bytes in position {at}-{}: unexpected end of data",
            bytes.len() - 1
        ),
    }
}

/// Code points as Python's `sys.stderr` writes them (`backslashreplace`
/// for a lone surrogate).
fn shown(points: &[u32]) -> String {
    points
        .iter()
        .map(|cp| char::from_u32(*cp).map_or_else(|| format!("\\u{cp:04x}"), String::from))
        .collect()
}

/// The token step's Python, from the token's code points: the JSON body
/// to send, or what it prints and exit 2.
pub fn token_body(step: &str, token: &[u32]) -> Result<String, Shown> {
    let (prefix, key) = want(step).expect("a token step");
    let start = token.iter().position(|c| !is_space(*c));
    let t: &[u32] = match start {
        None => &[],
        Some(s) => {
            let end = token.iter().rposition(|c| !is_space(*c)).unwrap_or(s);
            &token[s..=end]
        }
    };
    let p: Vec<u32> = prefix.chars().map(u32::from).collect();
    if !t.starts_with(&p) {
        let head = &t[..t.len().min(8)];
        return Err(Shown::err(
            format!(
                "{} fed {step}: expected a {prefix}... token (got {}...)\n",
                cli_name!(),
                shown(head)
            ),
            Exit::Usage,
        ));
    }
    let raw = &t[p.len()..];
    let damaged = |why: String| {
        Shown::err(
            format!("{} fed {step}: the token is damaged ({why})\n", cli_name!()),
            Exit::Usage,
        )
    };
    // `raw.encode()`: a lone surrogate can't be UTF-8.
    if let Some(i) = raw.iter().position(|c| char::from_u32(*c).is_none()) {
        return Err(damaged(format!(
            "'utf-8' codec can't encode character '\\u{:04x}' in position {i}: surrogates not allowed",
            raw[i]
        )));
    }
    let mut bytes: Vec<u8> = shown(raw).into_bytes();
    let pads = (4 - raw.len() % 4) % 4;
    bytes.extend(std::iter::repeat_n(b'=', pads));
    let decoded = a2b_base64(&translate(&bytes)).map_err(damaged)?;
    let text = std::str::from_utf8(&decoded)
        .map_err(|e| damaged(utf8_error(&decoded, &e)))?
        .to_string();
    let obj = json::parse(&text).map_err(|e| {
        damaged(super::super::tunnel::pyjson::decode_error(&text).unwrap_or_else(|| e.to_string()))
    })?;
    let body = Value::Object(vec![(Text::from(key), obj)]);
    Ok(json::to_string(&body, &Options::default()).unwrap_or_default())
}

fn is_space(cp: u32) -> bool {
    char::from_u32(cp).is_some_and(py_space)
}

/// `urlsafe_b64decode`'s translation: `-` to `+`, `_` to `/`.
fn translate(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|b| match b {
            b'-' => b'+',
            b'_' => b'/',
            other => *other,
        })
        .collect()
}

fn sextet(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// CPython's `binascii.a2b_base64(s, strict_mode=False)`: the bytes, or
/// `str(binascii.Error)`.
pub fn a2b_base64(s: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let (mut quad, mut left, mut pads) = (0u8, 0u8, 0u8);
    for &c in s {
        if c == b'=' {
            if quad >= 2 {
                pads += 1;
                if quad + pads >= 4 {
                    return Ok(out);
                }
            }
            continue;
        }
        let Some(v) = sextet(c) else { continue };
        pads = 0;
        match quad {
            0 => {
                quad = 1;
                left = v;
            }
            1 => {
                quad = 2;
                out.push((left << 2) | (v >> 4));
                left = v & 0x0f;
            }
            2 => {
                quad = 3;
                out.push((left << 4) | (v >> 2));
                left = v & 0x03;
            }
            _ => {
                quad = 0;
                out.push((left << 6) | v);
                left = 0;
            }
        }
    }
    match quad {
        0 => Ok(out),
        1 => Err(format!(
            "Invalid base64-encoded string: number of data characters ({}) cannot be 1 more than a multiple of 4",
            out.len() / 3 * 4 + 1
        )),
        _ => Err("Incorrect padding".to_string()),
    }
}

/// `base64.urlsafe_b64encode(b).decode().rstrip("=")`.
pub fn b64url(b: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut s = String::new();
    for chunk in b.chunks(3) {
        let n = chunk.len();
        let v = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..=n {
            s.push(char::from(ALPHA[((v >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    s
}

/// `tok(prefix, obj)`.
fn tok(prefix: &str, obj: &Value) -> String {
    let opts = Options {
        compact: true,
        ..Options::default()
    };
    let text = json::to_string(obj, &opts).unwrap_or_default();
    format!("{prefix}{}", b64url(text.as_bytes()))
}

/// `d[key]`, or the `KeyError` at `line`.
fn key<'a>(pairs: &'a [(Text, Value)], k: &str, line: u32) -> Result<&'a Value, Raise> {
    super::super::get(pairs, k)
        .ok_or_else(|| Raise::at(line, format!("KeyError: {}", py::repr(&super::super::s(k)))))
}

/// The pairing answer's reader (`step` is the verb), on what `$(…)` held.
pub fn reader(step: &str, raw: &[u8]) -> Shown {
    let cli = cli_name!();
    let d = std::str::from_utf8(raw)
        .ok()
        .and_then(|t| json::parse(t).ok());
    let Some(d) = d else {
        return Shown::err(
            format!("{cli} fed {step}: no answer from the daemon (is \"Connect agents across servers\" on? {cli} fed enable)\n"),
            Exit::Error,
        );
    };
    let pairs = match &d {
        Value::Object(pairs) => match super::super::get(pairs, "error") {
            Some(e) if py::truthy(e) => {
                return Shown::err(
                    format!("{cli} fed {step}: {}\n", py::str_of(e)),
                    Exit::Error,
                )
            }
            _ => pairs,
        },
        other => {
            return Shown::err(
                format!("{cli} fed {step}: {}\n", py::str_of(other)),
                Exit::Error,
            )
        }
    };
    let mut out = String::new();
    let mut line = |s: &str| {
        out.push_str(s);
        out.push('\n');
    };
    let r = (|| -> Result<(), Raise> {
        let code = |line: &mut dyn FnMut(&str)| -> Result<(), Raise> {
            let (n, m) = if step == "pair-finish" {
                (24, 25)
            } else {
                (27, 28)
            };
            line(&format!(
                "Your pairing code: {}",
                py::str_of(key(pairs, "sas", n)?)
            ));
            let fp = py::str_of(key(pairs, "peer_fingerprint", m)?);
            let sas = py::str_of(key(pairs, "sas", m)?);
            line(&format!(
                "If both codes match, run: {cli} fed confirm {fp} {sas}"
            ));
            Ok(())
        };
        match step {
            "pair-start" => {
                line("Send this to the other server owner (valid 10 minutes, single use):");
                line(&tok("k2pair1:", key(pairs, "offer", 14)?));
                line(&format!("They run: {cli} fed pair-join --token-stdin (and paste it), and send you back a k2pair2: token."));
            }
            "pair-join" => {
                line("Send this back to the owner who started the pairing:");
                line(&tok("k2pair2:", key(pairs, "answer", 18)?));
                line(&format!("They run: {cli} fed pair-finish --token-stdin (and paste it), and send you a k2pair3: token."));
            }
            "pair-finish" => {
                line("Send this to the other owner:");
                line(&tok("k2pair3:", key(pairs, "reveal", 22)?));
                line(&format!(
                    "They run: {cli} fed pair-verify --token-stdin (and paste it)."
                ));
                code(&mut line)?;
            }
            _ => code(&mut line)?,
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(out),
        Err(e) => raised(out, String::new(), &e),
    }
}
