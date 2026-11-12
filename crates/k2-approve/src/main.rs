//! k2-approve: check a signed approval as a relying party (`k2 approve verify`).
//! Exit codes: 0 approved / ok, 2 usage or a bad key set, 3 refused.

use k2_approve::verify::{verify, verify_once, DirNonceStore, Require};
use k2_approve::{describe, keys, payload, time, Check, View};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
Usage:
  k2 approve verify --approval <record.json> --keys <dir> [options]
      Check a signed approval against the keys YOU trust. Exit 0 approved,
      3 refused (one line says why), 2 usage or a bad key folder.
      --keys <dir>        approvers.tsv (name, person, check, view; tabs) and
                          <name>.pem for each key. Required: nothing asks the
                          daemon whose key it is.
      --payload <file>    the payload bytes you wrote and kept for this request.
                          Strongly advised: the approval must carry exactly
                          these bytes. Without it the record's own copy is used.
      --rp <name>         the relying party name you wrote into the payload
      --kind <kind>       the kind you expect, e.g. k2.release
      --check <c>         weakest check accepted: credential (default) or biometric
      --view <v>          weakest view accepted: page (default), host or device
      --nonce <hex>       the nonce you issued
      --used-nonces <dir> mark the nonce used in <dir> (one empty file per
                          nonce); a second approval with it is refused
      --at <time>         check as at this UTC time (2026-11-11T14:05:00Z)
                          instead of now
      --json              print the result as one JSON line
  k2 approve show <payload-file>
      Print what an approval screen shows for these bytes (or why they are
      refused). Signs nothing.
  k2 approve key-id <public-key.pem>
      Print a key's id (SHA-256 of its public key) and its six fingerprint
      words, to compare with the device's screen.
";

fn usage(msg: &str) -> ExitCode {
    eprintln!("k2 approve: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}

fn refused(r: &k2_approve::Refusal) -> ExitCode {
    eprintln!("k2 approve: refused ({}): {}", r.code.as_str(), r.detail);
    ExitCode::from(3)
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn cmd_verify(args: &[String]) -> ExitCode {
    let mut approval = None;
    let mut keys_dir = None;
    let mut stored = None;
    let (mut rp, mut kind, mut nonce, mut used, mut at) = (None, None, None, None, None);
    let (mut check, mut view, mut as_json) = (Check::Credential, View::Page, false);
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let val = |i: usize| args.get(i + 1).cloned();
        match a {
            "--json" => {
                as_json = true;
                i += 1;
                continue;
            }
            "--approval" | "--keys" | "--payload" | "--rp" | "--kind" | "--check" | "--view" | "--nonce" | "--used-nonces" | "--at" => {
                let Some(v) = val(i) else { return usage(&format!("{a} needs a value")) };
                match a {
                    "--approval" => approval = Some(PathBuf::from(v)),
                    "--keys" => keys_dir = Some(PathBuf::from(v)),
                    "--payload" => stored = Some(PathBuf::from(v)),
                    "--rp" => rp = Some(v),
                    "--kind" => kind = Some(v),
                    "--check" => match Check::parse(&v) {
                        Some(c) => check = c,
                        None => return usage("--check is biometric or credential"),
                    },
                    "--view" => match View::parse(&v) {
                        Some(x) => view = x,
                        None => return usage("--view is device, host or page"),
                    },
                    "--nonce" => nonce = Some(v),
                    "--used-nonces" => used = Some(PathBuf::from(v)),
                    _ => at = Some(v),
                }
                i += 2;
            }
            other => return usage(&format!("unknown argument {other}")),
        }
    }
    let Some(approval) = approval else { return usage("--approval is required") };
    let Some(keys_dir) = keys_dir else { return usage("--keys is required (the folder of keys you trust)") };
    let trusted = match keys::load_dir(&keys_dir) {
        Ok(t) => t,
        Err(e) => return usage(&format!("key folder: {e}")),
    };
    let record = match std::fs::read(&approval) {
        Ok(b) => b,
        Err(e) => return usage(&format!("{}: {e}", approval.display())),
    };
    let stored_bytes = match &stored {
        Some(p) => match std::fs::read(p) {
            Ok(b) => Some(b),
            Err(e) => return usage(&format!("{}: {e}", p.display())),
        },
        None => None,
    };
    let now = match &at {
        Some(t) => match time::parse_utc(t) {
            Some(n) => n,
            None => return usage("--at takes a UTC time like 2026-11-11T14:05:00Z"),
        },
        None => match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs() as i64,
            Err(_) => return usage("the clock is before 1970"),
        },
    };
    let req = Require { rp: rp.as_deref(), kind: kind.as_deref(), check, view, now, stored_payload: stored_bytes.as_deref(), nonce: nonce.as_deref() };
    let result = match &used {
        Some(d) => verify_once(&record, &trusted, &req, &mut DirNonceStore(d.clone())),
        None => verify(&record, &trusted, &req),
    };
    let v = match result {
        Ok(v) => v,
        Err(r) => return refused(&r),
    };
    let mut unchecked = Vec::new();
    if stored.is_none() {
        unchecked.push("--payload (the record's own bytes were used)");
    }
    if rp.is_none() {
        unchecked.push("--rp");
    }
    if kind.is_none() {
        unchecked.push("--kind");
    }
    if nonce.is_none() {
        unchecked.push("--nonce");
    }
    if used.is_none() {
        unchecked.push("--used-nonces (single use is yours to enforce)");
    }
    if as_json {
        println!(
            "{{\"ok\":true,\"key_name\":{},\"person\":{},\"key_id\":\"{}\",\"alg\":\"{}\",\"check\":\"{}\",\"view\":\"{}\",\"kind\":{},\"rp\":{},\"nonce\":\"{}\",\"answer_by\":\"{}\"}}",
            json_str(&v.key_name),
            json_str(&v.person),
            v.key_id,
            v.alg.as_str(),
            v.check.as_str(),
            v.view.as_str(),
            json_str(&v.payload.kind),
            json_str(&v.payload.rp),
            v.payload.nonce,
            v.payload.answer_by
        );
    } else {
        println!(
            "approved by {} with key {} ({}, {} on {}): {} for {}, nonce {}",
            v.person,
            v.key_name,
            v.alg.as_str(),
            v.check.as_str(),
            v.view.as_str(),
            v.payload.kind,
            v.payload.rp,
            v.payload.nonce
        );
    }
    if !unchecked.is_empty() {
        eprintln!("k2 approve: note: not checked because not given: {}", unchecked.join(", "));
    }
    ExitCode::SUCCESS
}

fn cmd_show(args: &[String]) -> ExitCode {
    let [path] = args else { return usage("show takes one payload file") };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return usage(&format!("{path}: {e}")),
    };
    match payload::parse(&bytes) {
        Ok(p) => {
            print!("{}", describe::text(&p));
            ExitCode::SUCCESS
        }
        Err(r) => refused(&r),
    }
}

fn cmd_key_id(args: &[String]) -> ExitCode {
    let [path] = args else { return usage("key-id takes one PEM file") };
    let pem = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return usage(&format!("{path}: {e}")),
    };
    let key = match keys::pem_to_der(&pem).and_then(|d| keys::parse_spki(&d)) {
        Ok(k) => k,
        Err(e) => return usage(&format!("{path}: {e}")),
    };
    let id = keys::key_id(&key.spki);
    println!("key id: {id}");
    println!("words:  {}", keys::words(&id).expect("a key id"));
    println!("alg:    {}", key.alg.as_str());
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("verify") => cmd_verify(&args[1..]),
        Some("show") => cmd_show(&args[1..]),
        Some("key-id") => cmd_key_id(&args[1..]),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => usage("say verify, show or key-id"),
    }
}
