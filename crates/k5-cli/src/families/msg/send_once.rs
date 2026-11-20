//! Idempotent sends (0.46.15), as the shell's `BEGIN_CLI_SEND_ONCE` block
//! does them (`cli/src/cell-socket.sh`): `k5 msg` and `k5 thread` send one
//! key per run, a new UUID or the one `--idempotency-key` names, and the
//! daemon answers a repeat of a key with its first answer.
//!
//! - [`init`] (`_k2_idem_init`): a key the caller gave must be 8-64 of
//!   `A-Za-z0-9_-` ([`KEY_USAGE`], exit 2); then one probe asks whether
//!   the daemon drops a repeated send ([`Client::send_once_supported`]);
//!   then, with no key given, a new one.
//! - [`send`] (`_k2_idem_send`): `cli_send` with the no-answer lines held
//!   back ([`send_soft`]), asked again with the SAME key while unanswered
//!   ([`unanswered`]: curl 28, 52 or 56, an empty answer, or the daemon's
//!   `in_progress`), `_K2_IDEM_TRIES` tries (3), a second apart; then the
//!   line that it may have been delivered and exit 1. A daemon that
//!   doesn't drop repeats gets the send once, and on no answer the line
//!   that says so.
//!
//! `cli_send` for a POST ([`send_soft`]): the cell socket when there is a
//! passport with a live socket and the route is one it serves; again over
//! TCP only when the socket took none of it (curl 7, or 75) or said "not
//! served on cell socket". Any other end there goes back to [`send`] as
//! it came, as `_cli_send_unanswered` hands it to a caller that reports
//! for itself.

use super::super::without_nuls;
use crate::exit::Exit;
use crate::out;
use crate::prelude::Closing;
use crate::transport::client::{uds_eligible, LOCAL_TRANSPORT_DENIED, NOT_SERVED};
use crate::transport::wire::curl;
use crate::transport::{Call, Channel, Client, Response};
use k2_client::product_name;
use std::time::{Duration, Instant};

/// `_k2_idem_init`'s line for a key it won't take.
pub const KEY_USAGE: &str = "Error: --idempotency-key takes 8-64 characters of A-Z, a-z, 0-9, '-' or '_' (the key a 'may have been delivered' line printed)";

/// The tries when `_K2_IDEM_TRIES` doesn't name a number.
pub const TRIES: u64 = 3;

/// The setting that changes the tries.
pub const TRIES_SETTING: &str = "_K2_IDEM_TRIES";

/// The daemon's answer while the first try with this key still runs.
pub const IN_PROGRESS: &str = r#""code":"in_progress""#;

/// The key a run sends, and whether the daemon drops repeats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub key: String,
    pub supported: bool,
}

/// `_k2_idem_valid`.
pub fn valid(key: &str) -> bool {
    (8..=64).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `_k2_idem_init [KEY]` (`given`: empty for none).
pub fn init(client: &mut Client, given: &str) -> Result<Key, Exit> {
    if !given.is_empty() {
        if !valid(given) {
            out::stderr_line(KEY_USAGE);
            return Err(Exit::Usage);
        }
        let supported = client.send_once_supported();
        return Ok(Key {
            key: given.to_string(),
            supported,
        });
    }
    let supported = client.send_once_supported();
    Ok(Key {
        key: uuid4(),
        supported,
    })
}

/// A random UUID, as Python's `uuid.uuid4()` prints it. The bits come
/// from the standard library's per-process random hash keys, which the
/// OS seeds: a send key needs to be unique, not secret.
pub fn uuid4() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let half = |n: u64| {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(n);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.finish()
    };
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&half(1).to_le_bytes());
    b[8..].copy_from_slice(&half(2).to_le_bytes());
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// What `out="$(_K2_NOANSWER_SOFT=1 cli_send …)" || rc=$?` leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Soft {
    /// `$out`: the answer's bytes with the NULs and trailing newlines
    /// gone, or nothing.
    pub out: Vec<u8>,
    /// `$rc`: 0, or curl's code.
    pub rc: i32,
}

/// `_k2_idem_unanswered RC OUT`.
pub fn unanswered(s: &Soft) -> bool {
    matches!(
        s.rc,
        curl::OPERATION_TIMEDOUT | curl::GOT_NOTHING | curl::RECV_ERROR
    ) || (s.rc == 0 && s.out.is_empty())
        || contains(&s.out, IN_PROGRESS.as_bytes())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// What `$(…)` holds of an answer: NULs and trailing newlines gone.
pub fn held(body: &[u8]) -> Vec<u8> {
    let mut b = without_nuls(body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

/// `_K2_NOANSWER_SOFT=1 cli_send <post> <endpoint> <args>` (see the
/// module's notes). Each answer goes through `closing`.
pub fn send_soft(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    call: Call<'_>,
) -> Soft {
    let passport = client.passport().is_some();
    let mut answer = |r: &Response| closing.answer(r, passport);
    if !client.force_tcp && client.has_cell_socket() && uds_eligible(endpoint) {
        match client.call(Channel::Uds, endpoint, call) {
            Ok(r) => {
                answer(&r);
                let out = held(&r.body);
                if !contains(&r.body, NOT_SERVED.as_bytes()) {
                    return Soft { out, rc: 0 };
                }
            }
            Err(e) => {
                let rc = e.curl_code();
                if rc != curl::COULDNT_CONNECT && rc != LOCAL_TRANSPORT_DENIED {
                    return Soft {
                        out: Vec::new(),
                        rc,
                    };
                }
            }
        }
    }
    match client.call(Channel::Tcp, endpoint, call) {
        Ok(r) => {
            answer(&r);
            Soft {
                out: held(&r.body),
                rc: 0,
            }
        }
        Err(e) => Soft {
            out: Vec::new(),
            rc: e.curl_code(),
        },
    }
}

/// `$_K2_IDEM_TRIES` as `_k2_idem_send` reads it: digits that are not
/// exactly `0`, else [`TRIES`].
pub fn tries(setting: Option<&str>) -> u64 {
    match setting {
        Some(t) if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) && t != "0" => {
            t.parse().unwrap_or(u64::MAX)
        }
        _ => TRIES,
    }
}

/// The line when a daemon that doesn't drop repeats didn't answer.
pub fn once_line(endpoint: &str, waited: u64) -> String {
    format!(
        concat!(
            "Error: ",
            product_name!(),
            "'s server didn't answer {} ({} s). It may have been delivered; check before resending (this server doesn't say it drops a repeated send, so ",
            k2_client::cli_name!(),
            " didn't try again)."
        ),
        endpoint, waited
    )
}

/// The line after the last try.
pub fn tries_line(endpoint: &str, n: u64, waited: u64, key: &str) -> String {
    format!(
        concat!(
            "Error: ",
            product_name!(),
            "'s server didn't answer {} ({} tries, {} s); it may have been delivered. It is safe to run the same command again with --idempotency-key {}: ",
            product_name!(),
            " delivers one key once."
        ),
        endpoint, n, waited, key
    )
}

/// `_k2_idem_send <post> <endpoint> <args>`: the answer as `cli_send`
/// left it, or exit 1 after the may-have-been-delivered line.
pub fn send(
    client: &mut Client,
    closing: &mut Closing,
    key: &Key,
    endpoint: &str,
    call: Call<'_>,
) -> Result<Soft, Exit> {
    let t0 = Instant::now();
    if !key.supported {
        let s = send_soft(client, closing, endpoint, call);
        if !unanswered(&s) {
            return Ok(s);
        }
        out::stderr_line(&once_line(endpoint, t0.elapsed().as_secs()));
        return Err(Exit::Error);
    }
    let setting = std::env::var(TRIES_SETTING).ok();
    let tries = tries(setting.as_deref());
    let mut n = 1;
    loop {
        let s = send_soft(client, closing, endpoint, call);
        if !unanswered(&s) {
            return Ok(s);
        }
        if n >= tries {
            break;
        }
        n += 1;
        std::thread::sleep(Duration::from_secs(1));
    }
    out::stderr_line(&tries_line(endpoint, n, t0.elapsed().as_secs(), &key.key));
    Err(Exit::Error)
}

/// `_k2_idem_replay_note`: a repeat the daemon answered from its first
/// try.
pub fn replay_note(response: &[u8]) -> Option<String> {
    (contains(response, br#""replayed":true"#) || contains(response, br#""duplicate":true"#)).then(
        || {
            concat!(
                "note: ",
                product_name!(),
                " already had this send (same idempotency key); it was not delivered again."
            )
            .to_string()
        },
    )
}
