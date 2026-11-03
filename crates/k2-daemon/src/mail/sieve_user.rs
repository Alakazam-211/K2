//! One active user Sieve per minted mailbox (RFC 9661 `SieveScript`).
//!
//! OOO (`vacation`) and forward (`redirect`) share the named script
//! `k2` with marked blocks (`# k2-ooo` / `# k2-forward`). Compose
//! **vacation then redirect** (RFC 5230 §4.5). `ooo unset` strips only
//! the vacation block; `forward unset` strips only redirect. Empty
//! remainder destroys + deactivates.
//!
//! A sibling catchall-alias-forward cut may have written a separate
//! `k2-forward` script — load migrates it into this composer.

use super::jmap::{SieveScriptInfo, StalwartClient};

pub const SCRIPT_NAME: &str = "k2";
pub const LEGACY_FORWARD_NAME: &str = "k2-forward";
pub const OOO_MARK: &str = "# k2-ooo";
pub const FORWARD_MARK: &str = "# k2-forward";

pub const VACATION_DAYS_DEFAULT: u32 = 7;
pub const VACATION_DAYS_MIN: u32 = 1;
pub const VACATION_DAYS_MAX: u32 = 31;
pub const TEXT_CAP: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserSieve {
    pub ooo: Option<OooBlock>,
    pub forward: Option<ForwardBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OooBlock {
    pub days: u32,
    pub text: String,
    /// UTC `YYYY-MM-DD` (RFC 5260 `currentdate` wrap). None = no expiry.
    pub until: Option<String>,
    pub from: String,
}

/// A mailbox's forward block. One destination writes exactly the bytes it
/// always did. Several are one `redirect` each (Stalwart queues each as its
/// own message; `forward_sync::REDIRECT_LIMIT` is why that works):
///
/// ```text
/// # k2-forward
/// redirect "same-box@…";       same_box: no guard, as a same-box forward has
/// if … { keep; stop; }         the guard (bounces, loops, spam, DMARC)
/// addheader "X-K2-Forwarded" …
/// redirect "outside-1@…";      dest
/// redirect "outside-2@…";      more
/// ```
///
/// `keep` covers the whole block: mixing `redirect` with `redirect :copy`
/// would drop the local copy (one bare `redirect` cancels the implicit keep).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ForwardBlock {
    pub dest: String,
    pub keep: bool,
    /// An outside destination's safety steps (`None`: a same-box forward,
    /// written exactly as before).
    pub guard: Option<ForwardGuard>,
    /// More destinations treated the way `dest` is.
    pub more: Vec<String>,
    /// Same-box destinations written before the guard (only with a guard;
    /// with no guard they are `dest` and `more`).
    pub same_box: Vec<String>,
}

impl ForwardBlock {
    /// A same-box forward (no guard).
    pub fn internal(dest: impl Into<String>, keep: bool) -> Self {
        ForwardBlock { dest: dest.into(), keep, ..Default::default() }
    }

    /// How many `redirect`s this block writes.
    pub fn redirects(&self) -> usize {
        1 + self.more.len() + self.same_box.len()
    }
}

/// The steps an outside forward runs before its redirect
/// (prd-hostmail-external-forwarding-v1 §3.4; vs-live FW-A3, FW-A11,
/// FW-A25), in this order:
///
/// 0. an empty envelope sender (a bounce, e.g. the destination refusing
///    an earlier forward) stays home, and so does mail From one of K2's system
///    senders (password resets, codes, notices): K2's own mail is never
///    forwarded out;
/// 1. a message that already carries this box's `X-K2-Forwarded` marker
///    stays home (loop stop);
/// 2. spam stays home (`spamtest`, or the mail server's own
///    `X-Spam-Score: spam…`; never `X-Spam-Status`, which is added after
///    Sieve);
/// 3. a message whose sender nothing vouches for stays home: `dmarc=fail` in any
///    `Authentication-Results`, or this box's own (authserv-id = `host`) saying
///    the lookup broke (`permerror`, `temperror`), `none` for no domain (a mail
///    with several From addresses, or none), or nothing about DMARC. Mail with
///    no stamp at all, which is mail between this box's mailboxes, still forwards;
/// 4. add the marker, then redirect.
///
/// Every test can only stop a forward, never cause one, so a sender
/// forging these headers gains nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardGuard {
    /// This box's mail host name: the loop marker's value and the
    /// authserv-id of its own `Authentication-Results`.
    pub host: String,
    /// K2's system senders: their mail stays home. Empty (no system sender)
    /// writes no clause.
    pub stay_home: Vec<String>,
}

/// The loop marker header.
pub const LOOP_HEADER: &str = "X-K2-Forwarded";
/// `spamtest` at or above this (1–10 scale) is spam.
pub const SPAMTEST_MIN: &str = "5";

/// What a guard step does with a message that stays home.
const STAY_HOME: &str = "{\nkeep;\nstop;\n}\n";

/// Is `host` safe to put into a Sieve string and a `:matches` pattern?
pub fn guard_host_ok(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.contains('.')
        && host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SievePatch {
    SetOoo(OooBlock),
    UnsetOoo,
    /// Catch-all/alias-forward composer entry (O9 merge).
    SetForward(ForwardBlock),
    UnsetForward,
}

pub fn days_in_range(days: u32) -> Result<u32, String> {
    if (VACATION_DAYS_MIN..=VACATION_DAYS_MAX).contains(&days) {
        Ok(days)
    } else {
        Err(format!(
            "--days must be {VACATION_DAYS_MIN}–{VACATION_DAYS_MAX} (RFC 5230 / Stalwart vacation interval), got {days}"
        ))
    }
}

/// UTC calendar date `YYYY-MM-DD`. Rejects non-canonical forms.
pub fn parse_until_date(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.len() != 10 {
        return Err(format!("--until must be UTC YYYY-MM-DD, got '{raw}'"));
    }
    let date = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
        format!("--until must be a valid UTC calendar date YYYY-MM-DD, got '{raw}'")
    })?;
    let out = date.format("%Y-%m-%d").to_string();
    if out != raw {
        return Err(format!("--until must be UTC YYYY-MM-DD, got '{raw}'"));
    }
    Ok(out)
}

pub fn reject_lone_dot_line(text: &str, what: &str) -> Result<(), String> {
    for line in text.lines() {
        if line.trim_end_matches('\r') == "." {
            return Err(format!(
                "{what} must not contain a lone '.' line (RFC 5228 text: terminator)"
            ));
        }
    }
    Ok(())
}

pub fn reject_html(text: &str) -> Result<(), String> {
    let lower = text.to_ascii_lowercase();
    for tag in [
        "<html",
        "<body",
        "<div",
        "<span",
        "<br",
        "<p>",
        "<p ",
        "<table",
        "<style",
        "<script",
        "<head",
        "<!doctype",
    ] {
        if lower.contains(tag) {
            return Err("plain text only — HTML footer/body is not supported this cut".to_string());
        }
    }
    Ok(())
}

pub fn cap_text(text: &str, what: &str) -> Result<(), String> {
    if text.len() > TEXT_CAP {
        return Err(format!("{what} is {} bytes; max is {TEXT_CAP}", text.len()));
    }
    Ok(())
}

/// Compose one `k2` script. `None` when both blocks are absent (destroy).
pub fn compose(s: &UserSieve) -> Option<String> {
    if s.ooo.is_none() && s.forward.is_none() {
        return None;
    }
    let mut req: Vec<&str> = Vec::new();
    if s.ooo.is_some() {
        req.push("vacation");
    }
    if s.ooo.as_ref().is_some_and(|o| o.until.is_some()) {
        req.push("date");
        req.push("relational");
    }
    if s.forward.as_ref().is_some_and(|f| f.keep) {
        req.push("copy");
    }
    if s.forward.as_ref().is_some_and(|f| f.guard.is_some()) {
        for r in ["envelope", "editheader", "spamtest", "relational", "comparator-i;ascii-numeric"] {
            if !req.contains(&r) {
                req.push(r);
            }
        }
    }
    let mut out = String::new();
    if !req.is_empty() {
        out.push_str("require [");
        for (i, r) in req.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push('"');
            out.push_str(r);
            out.push('"');
        }
        out.push_str("];\n");
    }
    if let Some(ooo) = &s.ooo {
        out.push_str(OOO_MARK);
        out.push('\n');
        out.push_str(&compose_ooo(ooo));
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    if let Some(fwd) = &s.forward {
        out.push_str(FORWARD_MARK);
        out.push('\n');
        out.push_str(&compose_forward(fwd));
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    Some(out)
}

fn compose_ooo(ooo: &OooBlock) -> String {
    let from = sieve_quoted(&ooo.from);
    // RFC 5228: `text:` ends at a lone `.` line; the command still needs `;`.
    // Without it Stalwart compiles `Expected ';' but found "redirect"`.
    let mut vac = format!(
        "vacation :days {} :from {} :addresses [{}] text:\n{}\n.\n;\n",
        ooo.days,
        from,
        from,
        ooo.text.trim_end_matches('\n')
    );
    if let Some(until) = &ooo.until {
        vac = format!(
            "if currentdate :zone \"+0000\" :value \"le\" \"date\" \"{until}\" {{\n{vac}}}\n"
        );
    }
    vac
}

fn compose_forward(fwd: &ForwardBlock) -> String {
    let redirect = |dest: &str| {
        let dest = sieve_quoted(dest);
        if fwd.keep { format!("redirect :copy {dest};\n") } else { format!("redirect {dest};\n") }
    };
    let mut out = String::new();
    let Some(g) = &fwd.guard else {
        out.push_str(&redirect(&fwd.dest));
        for d in &fwd.more {
            out.push_str(&redirect(d));
        }
        return out;
    };
    for d in &fwd.same_box {
        out.push_str(&redirect(d));
    }
    let host = sieve_quoted(&g.host);
    let home = STAY_HOME;
    out.push_str(&format!("if envelope :is \"from\" \"\" {home}"));
    if !g.stay_home.is_empty() {
        let list = g.stay_home.iter().map(|a| sieve_quoted(a)).collect::<Vec<_>>().join(", ");
        out.push_str(&format!("if address :is \"from\" [{list}] {home}"));
    }
    out.push_str(&format!("if header :contains \"{LOOP_HEADER}\" {host} {home}"));
    out.push_str(&format!(
        "if anyof (spamtest :value \"ge\" :comparator \"i;ascii-numeric\" \"{SPAMTEST_MIN}\", header :matches \"X-Spam-Score\" \"spam*\") {home}"
    ));
    out.push_str(&dmarc_guard(&g.host));
    out.push_str(&format!("addheader \"{LOOP_HEADER}\" {host};\n"));
    out.push_str(&redirect(&fwd.dest));
    for d in &fwd.more {
        out.push_str(&redirect(d));
    }
    out
}

/// The guard's DMARC step (see [`ForwardGuard`]).
fn dmarc_guard(host: &str) -> String {
    let mut out = String::new();
    let stamp = |rest: &str| sieve_quoted(&format!("{};*{rest}", host));
    out.push_str(&format!(
        "if anyof (header :contains \"Authentication-Results\" \"dmarc=fail\", header :matches \"Authentication-Results\" [{}, {}, {}, {}], allof (header :matches \"Authentication-Results\" {}, not header :contains \"Authentication-Results\" \"dmarc=\")) {STAY_HOME}",
        stamp("dmarc=permerror*"),
        stamp("dmarc=temperror*"),
        stamp("dmarc=none*header.from= *"),
        stamp("dmarc=none*header.from="),
        sieve_quoted(&format!("{};*", host)),
    ));
    out
}

/// The DMARC step as 0.46.7 and earlier wrote it.
fn earlier_dmarc_guard(host: &str) -> String {
    let test = sieve_quoted(&format!("{host};*dmarc=fail*"));
    format!("if header :matches \"Authentication-Results\" {test} {STAY_HOME}")
}

/// [`compose`] with the DMARC step an earlier K2 wrote. Such a script is still
/// K2's own, not someone's edit, until the next sync pass brings it up to date.
pub fn compose_with_earlier_guard(s: &UserSieve) -> Option<String> {
    let host = &s.forward.as_ref()?.guard.as_ref()?.host;
    Some(compose(s)?.replace(&dmarc_guard(host), &earlier_dmarc_guard(host)))
}

fn sieve_quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `source` with every `text:` body blanked, byte for byte (each byte a
/// space, newlines kept), so offsets still line up. An out-of-office text is
/// whatever someone typed: nothing in it is a command or a mark, whatever it
/// looks like. A body runs from a line ending in `text:` to its lone `.`
/// (RFC 5228); an unterminated one blanks to the end.
pub fn blank_text_bodies(source: &str) -> String {
    let mut out = Vec::with_capacity(source.len());
    let mut in_body = false;
    for line in source.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\r', '\n']);
        if in_body {
            if bare == "." {
                in_body = false;
                out.extend_from_slice(line.as_bytes());
            } else {
                out.extend(line.bytes().map(|b| if b == b'\n' || b == b'\r' { b } else { b' ' }));
            }
        } else {
            in_body = bare.trim_end().ends_with("text:");
            out.extend_from_slice(line.as_bytes());
        }
    }
    // Multi-byte characters were only ever blanked whole: this is ASCII there.
    String::from_utf8(out).unwrap_or_default()
}

/// Parse a `k2` script into its marked blocks. A redirect K2 didn't mark as
/// its forward block isn't a forward (and is dropped by the next write).
pub fn parse(source: &str) -> UserSieve {
    let code = blank_text_bodies(source);
    let ooo = parse_ooo_block(source, &code);
    let forward = parse_forward_block(&code);
    UserSieve { ooo, forward }
}

/// The forward in a leftover `k2-forward` script (from before the marks).
pub fn parse_legacy_forward(source: &str) -> Option<ForwardBlock> {
    let code = blank_text_bodies(source);
    parse_forward_region(&code)
}

fn parse_ooo_block(source: &str, code: &str) -> Option<OooBlock> {
    // The marks are read from the blanked copy: a mark typed into the text
    // isn't one. The text itself comes from the source.
    let region = match code.find(OOO_MARK) {
        Some(i) => {
            let start = i + OOO_MARK.len();
            let end = code[start..].find(FORWARD_MARK).map_or(source.len(), |e| start + e);
            source[start..end].to_string()
        }
        None if code.contains("vacation") => source.to_string(),
        None => return None,
    };
    if !region.contains("vacation") {
        return None;
    }
    let days = extract_days(&region).unwrap_or(VACATION_DAYS_DEFAULT);
    let from = extract_quoted_after(&region, ":from").unwrap_or_default();
    let text = extract_text_body(&region).unwrap_or_default();
    let until = extract_until(&region);
    Some(OooBlock {
        days,
        text,
        until,
        from,
    })
}

/// The marked forward block of a blanked script.
fn parse_forward_block(code: &str) -> Option<ForwardBlock> {
    parse_forward_region(&marked_region(code, FORWARD_MARK, OOO_MARK)?)
}

fn parse_forward_region(region: &str) -> Option<ForwardBlock> {
    let region = region.to_string();
    let keep = region.contains("redirect :copy") || region.contains("redirect :copy\n");
    let guard = extract_quoted_after(&region, &format!("header :contains \"{LOOP_HEADER}\""))
        .map(|host| ForwardGuard { host, stay_home: extract_stay_home(&region) });
    // Redirects before the guard's first test are the same-box ones.
    let guard_at = guard.as_ref().and_then(|_| region.find("if envelope"));
    let mut same_box = Vec::new();
    let mut rest = Vec::new();
    for (at, dest) in extract_redirect_dests(&region) {
        if guard_at.is_some_and(|g| at < g) {
            same_box.push(dest);
        } else {
            rest.push(dest);
        }
    }
    let mut rest = rest.into_iter();
    let dest = rest.next()?;
    Some(ForwardBlock { dest, keep, guard, more: rest.collect(), same_box })
}

fn marked_region(source: &str, start: &str, other: &str) -> Option<String> {
    let idx = source.find(start)?;
    let after = &source[idx + start.len()..];
    let end = after.find(other).unwrap_or(after.len());
    Some(after[..end].to_string())
}

fn extract_days(s: &str) -> Option<u32> {
    let key = ":days";
    let i = s.find(key)?;
    let rest = s[i + key.len()..].trim_start();
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse().ok()
}

fn extract_quoted_after(s: &str, key: &str) -> Option<String> {
    let i = s.find(key)?;
    extract_quoted(&s[i + key.len()..])
}

fn extract_quoted(s: &str) -> Option<String> {
    let s = s.trim_start();
    let s = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '"' => return Some(out),
            _ => out.push(c),
        }
    }
    None
}

fn extract_text_body(s: &str) -> Option<String> {
    let i = s.find("text:")?;
    let after = &s[i + 5..];
    let after = after.strip_prefix('\r').unwrap_or(after);
    let after = after.strip_prefix('\n').unwrap_or(after);
    let mut body = String::new();
    for line in after.lines() {
        let line = line.trim_end_matches('\r');
        if line == "." {
            return Some(body.trim_end_matches('\n').to_string());
        }
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(line);
    }
    None
}

fn extract_until(s: &str) -> Option<String> {
    // `currentdate ... "date" "YYYY-MM-DD"`
    let key = "\"date\"";
    let i = s.find(key)?;
    extract_quoted(&s[i + key.len()..]).filter(|d| parse_until_date(d).is_ok())
}

/// The list in `if address :is "from" ["a", "b"]`.
fn extract_stay_home(s: &str) -> Vec<String> {
    let key = "if address :is \"from\" [";
    let Some(i) = s.find(key) else { return Vec::new() };
    let mut rest = &s[i + key.len()..];
    let mut out = Vec::new();
    while let Some(q) = rest.find('"').filter(|q| rest.find(']').is_none_or(|e| *q < e)) {
        let Some(item) = extract_quoted(&rest[q..]) else { break };
        rest = &rest[q + 1 + item.len() + 1..];
        out.push(item);
    }
    out
}

/// Every `redirect "…"` in `s`, with where it starts.
fn extract_redirect_dests(s: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = s[from..].find("redirect") {
        let at = from + i;
        from = at + "redirect".len();
        let rest = s[from..].trim_start();
        let rest = rest.strip_prefix(":copy").unwrap_or(rest).trim_start();
        if let Some(dest) = extract_quoted(rest) {
            out.push((at, dest));
        }
    }
    out
}

pub fn apply_patch(current: UserSieve, patch: SievePatch) -> UserSieve {
    let mut next = current;
    match patch {
        SievePatch::SetOoo(ooo) => next.ooo = Some(ooo),
        SievePatch::UnsetOoo => next.ooo = None,
        SievePatch::SetForward(fwd) => next.forward = Some(fwd),
        SievePatch::UnsetForward => next.forward = None,
    }
    next
}

/// Load the mailbox's user Sieve, migrating a leftover `k2-forward`
/// script into the composer view.
pub fn load_user_sieve(
    client: &StalwartClient,
    account_id: &str,
) -> Result<(UserSieve, Vec<SieveScriptInfo>), String> {
    let scripts = client.sieve_scripts_list(account_id)?;
    let mut parsed = UserSieve::default();
    for sc in &scripts {
        if sc.name != SCRIPT_NAME && sc.name != LEGACY_FORWARD_NAME {
            continue;
        }
        if sc.blob_id.trim().is_empty() {
            continue;
        }
        let bytes = client.sieve_script_blob(account_id, &sc.blob_id)?;
        let src = String::from_utf8_lossy(&bytes);
        let part = parse(&src);
        if sc.name == SCRIPT_NAME {
            if part.ooo.is_some() {
                parsed.ooo = part.ooo;
            }
            if part.forward.is_some() {
                parsed.forward = part.forward;
            }
        } else if sc.name == LEGACY_FORWARD_NAME && parsed.forward.is_none() {
            parsed.forward = parse_legacy_forward(&src);
        }
    }
    Ok((parsed, scripts))
}

/// Write the composed `k2` script (or destroy it when empty). Destroys
/// a leftover `k2-forward` script after merging.
pub fn save_user_sieve(
    client: &StalwartClient,
    account_id: &str,
    scripts: &[SieveScriptInfo],
    sieve: &UserSieve,
) -> Result<(), String> {
    // RFC 9661 / Stalwart: destroy of an *active* script is `scriptIsActive`
    // ("Deactivate Sieve script before deletion"). So an empty remainder
    // replaces `k2` with a no-op `keep;` (still the active script).
    let source = compose(sieve).unwrap_or_else(|| "keep;\n".to_string());
    save_user_source(client, account_id, scripts, &source, true)
}

/// Write `source` as the `k2` script, active if `activate`, then destroy a
/// leftover `k2-forward`.
pub fn save_user_source(
    client: &StalwartClient,
    account_id: &str,
    scripts: &[SieveScriptInfo],
    source: &str,
    activate: bool,
) -> Result<(), String> {
    let k2 = scripts.iter().find(|s| s.name == SCRIPT_NAME);
    let blob_id = client.blob_upload(account_id, source.as_bytes())?;
    if activate {
        client.sieve_script_put_k2(account_id, k2.map(|s| s.id.as_str()), &blob_id, &[])?;
    } else {
        client.sieve_script_put_k2_inactive(account_id, k2.map(|s| s.id.as_str()), &blob_id)?;
    }
    let destroy_ids: Vec<String> = scripts.iter().filter(|s| s.name == LEGACY_FORWARD_NAME).map(|s| s.id.clone()).collect();
    if !destroy_ids.is_empty() {
        client.sieve_scripts_destroy(account_id, &destroy_ids)?;
    }
    Ok(())
}


/// Read, patch, write, with no lock and no rows behind it. Production code goes
/// through `forward_sync` instead; this is for tests against a mail server.
#[cfg(test)]
pub fn patch_user_sieve(
    client: &StalwartClient,
    account_id: &str,
    patch: SievePatch,
) -> Result<UserSieve, String> {
    let (current, scripts) = load_user_sieve(client, account_id)?;
    let next = apply_patch(current, patch);
    save_user_sieve(client, account_id, &scripts, &next)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ooo(text: &str) -> OooBlock {
        OooBlock {
            days: 7,
            text: text.to_string(),
            until: None,
            from: "user@domain.test".to_string(),
        }
    }

    #[test]
    fn compose_vacation_then_redirect() {
        let s = UserSieve {
            ooo: Some(ooo("I'm away.")),
            forward: Some(ForwardBlock {
                dest: "dest@domain.test".to_string(),
                keep: false,
                guard: None,
                ..Default::default()
            }),
        };
        let src = compose(&s).expect("script");
        let vac = src.find("vacation").expect("vacation");
        let redir = src.find("redirect").expect("redirect");
        assert!(vac < redir, "vacation then redirect:\n{src}");
        assert!(src.contains(OOO_MARK), "{src}");
        assert!(src.contains(FORWARD_MARK), "{src}");
        assert!(src.contains("require [\"vacation\"]"), "{src}");
        assert!(
            src.contains(".\n;"),
            "vacation text: needs a command semicolon before redirect:\n{src}"
        );
        let semi = src.find(".\n;").expect("semi");
        assert!(semi < redir, "semicolon before redirect:\n{src}");
        assert!(src.contains(":from \"user@domain.test\""), "{src}");
        assert!(src.contains(":addresses [\"user@domain.test\"]"), "{src}");
        assert!(!src.contains("VacationResponse"), "{src}");
        assert!(!src.contains("x:Sieve"), "{src}");
    }

    #[test]
    fn compose_redirect_copy_when_keep() {
        let s = UserSieve {
            ooo: None,
            forward: Some(ForwardBlock {
                dest: "dest@domain.test".to_string(),
                keep: true,
                guard: None,
                ..Default::default()
            }),
        };
        let src = compose(&s).expect("script");
        assert!(
            src.contains("redirect :copy \"dest@domain.test\";"),
            "{src}"
        );
        assert!(src.contains("require [\"copy\"]"), "{src}");
        let round = parse(&src);
        assert_eq!(round.forward.as_ref().map(|f| f.keep), Some(true));
        assert!(round.ooo.is_none());
    }

    #[test]
    fn set_forward_merges_with_ooo() {
        let s = apply_patch(
            UserSieve {
                ooo: Some(ooo("away")),
                forward: None,
            },
            SievePatch::SetForward(ForwardBlock {
                dest: "dest@domain.test".to_string(),
                keep: false,
                guard: None,
                ..Default::default()
            }),
        );
        let src = compose(&s).expect("both");
        let vac = src.find("vacation").unwrap();
        let redir = src.find("redirect").unwrap();
        assert!(vac < redir, "{src}");
        assert_eq!(s.forward.unwrap().dest, "dest@domain.test");
    }

    #[test]
    fn unset_ooo_leaves_forward() {
        let s = UserSieve {
            ooo: Some(ooo("gone")),
            forward: Some(ForwardBlock {
                dest: "dest@domain.test".to_string(),
                keep: false,
                guard: None,
                ..Default::default()
            }),
        };
        let next = apply_patch(s, SievePatch::UnsetOoo);
        assert!(next.ooo.is_none());
        assert_eq!(
            next.forward.as_ref().map(|f| f.dest.as_str()),
            Some("dest@domain.test")
        );
        let src = compose(&next).expect("forward remains");
        assert!(!src.contains("vacation"), "{src}");
        assert!(src.contains("redirect"), "{src}");
        assert!(src.contains(FORWARD_MARK), "{src}");
        assert!(!src.contains(OOO_MARK), "{src}");
    }

    #[test]
    fn unset_forward_leaves_ooo() {
        let s = UserSieve {
            ooo: Some(ooo("still away")),
            forward: Some(ForwardBlock {
                dest: "dest@domain.test".to_string(),
                keep: true,
                guard: None,
                ..Default::default()
            }),
        };
        let next = apply_patch(s, SievePatch::UnsetForward);
        assert!(next.forward.is_none());
        assert_eq!(
            next.ooo.as_ref().map(|o| o.text.as_str()),
            Some("still away")
        );
        let src = compose(&next).expect("ooo remains");
        assert!(src.contains("vacation"), "{src}");
        assert!(!src.contains("redirect"), "{src}");
    }

    #[test]
    fn empty_remainder_composes_none() {
        let s = UserSieve {
            ooo: Some(ooo("x")),
            forward: Some(ForwardBlock {
                dest: "d@x.test".to_string(),
                keep: false,
                guard: None,
                ..Default::default()
            }),
        };
        let s = apply_patch(s, SievePatch::UnsetOoo);
        let s = apply_patch(s, SievePatch::UnsetForward);
        assert!(compose(&s).is_none());
    }

    #[test]
    fn until_wraps_currentdate_utc() {
        let mut block = ooo("back soon");
        block.until = Some("2026-12-31".to_string());
        block.days = 10;
        let src = compose(&UserSieve {
            ooo: Some(block),
            forward: None,
        })
        .expect("script");
        assert!(
            src.contains("require [\"vacation\", \"date\", \"relational\"]"),
            "{src}"
        );
        assert!(
            src.contains("currentdate :zone \"+0000\" :value \"le\" \"date\" \"2026-12-31\""),
            "{src}"
        );
        let round = parse(&src);
        assert_eq!(
            round.ooo.as_ref().unwrap().until.as_deref(),
            Some("2026-12-31")
        );
        assert_eq!(round.ooo.as_ref().unwrap().days, 10);
        assert_eq!(round.ooo.as_ref().unwrap().text, "back soon");
    }

    #[test]
    fn parse_round_trip_ooo_and_forward() {
        let s = UserSieve {
            ooo: Some(OooBlock {
                days: 3,
                text: "Out.\nBack Monday.".to_string(),
                until: Some("2026-10-01".to_string()),
                from: "a@b.test".to_string(),
            }),
            forward: Some(ForwardBlock {
                dest: "c@d.test".to_string(),
                keep: false,
                guard: None,
                ..Default::default()
            }),
        };
        let src = compose(&s).unwrap();
        assert_eq!(parse(&src), s);
    }

    /// A script from before the guard changed reads the same, out-of-office
    /// included, so the sync pass can rewrite it without losing anything.
    #[test]
    fn a_script_with_the_earlier_guard_parses_to_the_same_rules() {
        let s = UserSieve { ooo: Some(ooo("away")), forward: Some(outside(true)) };
        let now = compose(&s).expect("script");
        let earlier = compose_with_earlier_guard(&s).expect("earlier");
        assert_ne!(now, earlier);
        assert!(earlier.contains("if header :matches \"Authentication-Results\" \"mail.brand.test;*dmarc=fail*\" {"), "{earlier}");
        assert!(!earlier.contains("dmarc=permerror"), "{earlier}");
        assert_eq!(parse(&earlier), s);
        assert_eq!(compose(&parse(&earlier)).as_deref(), Some(now.as_str()));
    }

    #[test]
    fn parse_legacy_k2_forward_script() {
        let src = "require [\"copy\"];\nredirect :copy \"keep@x.test\";\n";
        assert_eq!(
            parse_legacy_forward(src),
            Some(ForwardBlock {
                dest: "keep@x.test".to_string(),
                keep: true,
                guard: None,
                ..Default::default()
            })
        );
        // Unmarked in a `k2` script, the same text isn't K2's forward.
        assert_eq!(parse(src), UserSieve::default());
    }

    /// Review finding 1: text someone typed into an out-of-office never
    /// becomes a forward, however it's worded, and is kept as typed.
    #[test]
    fn out_of_office_text_never_becomes_a_forward() {
        for text in [
            "Back Monday. Please redirect \"evil@outside.test\" if urgent",
            "redirect :copy \"evil@outside.test\";",
            "# k2-forward\nredirect \"evil@outside.test\";",
            "# k2-ooo\n# k2-forward\nredirect :copy \"evil@outside.test\";\nbye",
            "caf\u{e9} \u{1f600} redirect \"evil@outside.test\";",
        ] {
            let s = UserSieve { ooo: Some(ooo(text)), forward: None };
            let src = compose(&s).expect("script");
            let round = parse(&src);
            assert_eq!(round.forward, None, "no forward in the text: {src}");
            assert_eq!(round.ooo.as_ref().map(|o| o.text.as_str()), Some(text), "kept as typed: {src}");
            assert_eq!(compose(&round).as_deref(), Some(src.as_str()), "byte-stable: {src}");
            // What `ooo unset` does: nothing left, so nothing is written.
            assert_eq!(compose(&apply_patch(round, SievePatch::UnsetOoo)), None, "{src}");
            // Beside a real forward, the real one stands and the text's doesn't.
            let both = UserSieve { ooo: Some(ooo(text)), forward: Some(outside(true)) };
            let src = compose(&both).expect("script");
            assert_eq!(parse(&src), both, "{src}");
            assert_eq!(compose(&apply_patch(parse(&src), SievePatch::UnsetOoo)).map(|o| o.contains("evil@")), Some(false), "{src}");
        }
    }

    #[test]
    fn blanking_keeps_every_offset() {
        let src = "require [\"vacation\"];\nvacation :days 7 text:\nh\u{e9}llo redirect \"x@y.test\"\n.\n;\n# k2-forward\nredirect \"a@b.test\";\n";
        let code = blank_text_bodies(src);
        assert_eq!(code.len(), src.len());
        assert!(!code.contains("x@y.test"), "{code}");
        assert!(code.contains("redirect \"a@b.test\""), "{code}");
        assert_eq!(code.find("# k2-forward"), src.find("# k2-forward"));
        // No closing dot: it blanks to the end.
        assert!(!blank_text_bodies("vacation text:\nredirect \"x@y.test\";\n").contains("x@y.test"));
    }

    // ── outside forwards (prd-hostmail-external-forwarding-v1 §3.4) ──
    // Goldens live in tests/mail/forward-goldens/ so the loopback Stalwart
    // probe (tests/mail/stalwart-forward-external-probe.py) compiles and
    // runs exactly these bytes.

    const GOLDEN_EXTERNAL_KEEP: &str = include_str!("../../../../tests/mail/forward-goldens/external-keep.sieve");
    const GOLDEN_EXTERNAL_NOKEEP_OOO: &str = include_str!("../../../../tests/mail/forward-goldens/external-nokeep-ooo.sieve");
    const GOLDEN_INTERNAL_KEEP: &str = include_str!("../../../../tests/mail/forward-goldens/internal-keep.sieve");

    fn outside(keep: bool) -> ForwardBlock {
        ForwardBlock {
            dest: "me@outside.test".to_string(),
            keep,
            guard: Some(ForwardGuard { host: "mail.brand.test".to_string(), stay_home: vec![] }),
            ..Default::default()
        }
    }

    #[test]
    fn golden_external_keep() {
        let src = compose(&UserSieve { ooo: None, forward: Some(outside(true)) }).expect("script");
        assert_eq!(src, GOLDEN_EXTERNAL_KEEP);
    }

    #[test]
    fn golden_external_no_keep_after_ooo() {
        let mut o = ooo("Away until Monday.");
        o.from = "sales@brand.test".to_string();
        let src = compose(&UserSieve { ooo: Some(o), forward: Some(outside(false)) }).expect("script");
        assert_eq!(src, GOLDEN_EXTERNAL_NOKEEP_OOO);
    }

    #[test]
    fn golden_internal_is_unchanged() {
        let src = compose(&UserSieve { ooo: None, forward: Some(ForwardBlock::internal("team@brand.test", true)) }).expect("script");
        assert_eq!(src, GOLDEN_INTERNAL_KEEP);
        assert!(!src.contains(LOOP_HEADER), "a same-box forward has no guard: {src}");
    }

    /// The guard's steps, in order, all before the redirect: bounce stays
    /// home, loop stop, spam, DMARC failure, then the marker.
    #[test]
    fn external_guard_order() {
        let src = compose(&UserSieve { ooo: Some(ooo("away")), forward: Some(outside(true)) }).expect("script");
        let at = |needle: &str| src.find(needle).unwrap_or_else(|| panic!("missing {needle}:\n{src}"));
        let steps = [
            at("vacation :days"),
            at("if envelope :is \"from\" \"\""),
            at("if header :contains \"X-K2-Forwarded\" \"mail.brand.test\""),
            at("spamtest :value \"ge\""),
            at("header :contains \"Authentication-Results\" \"dmarc=fail\""),
            at("addheader \"X-K2-Forwarded\""),
            at("redirect :copy \"me@outside.test\""),
        ];
        assert!(steps.windows(2).all(|w| w[0] < w[1]), "order {steps:?}:\n{src}");
        assert!(!src.contains("X-Spam-Status"), "X-Spam-Status is added after Sieve: {src}");
        // Every guard branch keeps and stops (never discards).
        assert_eq!(src.matches("{\nkeep;\nstop;\n}").count(), 4, "{src}");
        assert!(!src.contains("discard"), "{src}");
    }

    #[test]
    fn external_round_trips() {
        for keep in [true, false] {
            for with_ooo in [false, true] {
                let s = UserSieve { ooo: with_ooo.then(|| ooo("x")), forward: Some(outside(keep)) };
                let src = compose(&s).expect("script");
                assert_eq!(parse(&src), s, "{src}");
                assert_eq!(compose(&parse(&src)).as_deref(), Some(src.as_str()));
            }
        }
    }

    // ── several destinations ─────────────────────────────────────────

    const GOLDEN_EXTERNAL_MANY: &str = include_str!("../../../../tests/mail/forward-goldens/external-many-keep.sieve");
    const GOLDEN_MIXED_MANY: &str = include_str!("../../../../tests/mail/forward-goldens/mixed-many-nokeep-ooo.sieve");

    fn many(keep: bool) -> ForwardBlock {
        ForwardBlock { more: vec!["two@outside.test".to_string(), "three@outside.test".to_string()], ..outside(keep) }
    }

    #[test]
    fn golden_several_outside_destinations_share_one_guard() {
        let src = compose(&UserSieve { ooo: None, forward: Some(many(true)) }).expect("script");
        assert_eq!(src, GOLDEN_EXTERNAL_MANY);
        assert_eq!(src.matches("addheader").count(), 1, "one marker for every copy: {src}");
        assert_eq!(src.matches("redirect :copy").count(), 3, "{src}");
        assert_eq!(src.matches("if envelope").count(), 1, "one guard: {src}");
        let last_guard = src.find("addheader").expect("marker");
        assert!(src.find("redirect").expect("redirect") > last_guard, "every outside redirect is behind the guard: {src}");
    }

    #[test]
    fn golden_same_box_destinations_come_before_the_guard() {
        let mut o = ooo("Away until Monday.");
        o.from = "sales@brand.test".to_string();
        let fwd = ForwardBlock { same_box: vec!["team@brand.test".to_string()], ..many(false) };
        assert_eq!(fwd.redirects(), 4);
        let src = compose(&UserSieve { ooo: Some(o), forward: Some(fwd) }).expect("script");
        assert_eq!(src, GOLDEN_MIXED_MANY);
        let team = src.find("redirect \"team@brand.test\"").expect("team");
        assert!(team < src.find("if envelope").expect("guard"), "{src}");
        assert!(src.find("vacation").expect("vacation") < team, "vacation first: {src}");
        assert!(!src.contains(":copy"), "no-keep: {src}");
    }

    const GOLDEN_SYSTEM_SENDER: &str = include_str!("../../../../tests/mail/forward-goldens/external-system-sender.sieve");

    /// Review finding 10: K2's own mail stays home, and the clause round-trips.
    #[test]
    fn golden_system_senders_stay_home() {
        let mut fwd = outside(true);
        fwd.guard = Some(ForwardGuard { host: "mail.brand.test".to_string(), stay_home: vec!["k2@brand.test".to_string(), "system@notify.brand.test".to_string()] });
        let s = UserSieve { ooo: None, forward: Some(fwd) };
        let src = compose(&s).expect("script");
        assert_eq!(src, GOLDEN_SYSTEM_SENDER);
        assert_eq!(parse(&src), s, "{src}");
        let home = src.find("if address :is \"from\"").expect("clause");
        assert!(home < src.find("addheader").expect("marker") && home < src.find("redirect").expect("redirect"), "{src}");
    }

    #[test]
    fn several_destinations_keep_or_not_as_one_and_round_trip() {
        for keep in [true, false] {
            for with_ooo in [false, true] {
                let forms = [
                    many(keep),
                    ForwardBlock { same_box: vec!["team@brand.test".to_string(), "ops@brand.test".to_string()], ..many(keep) },
                    ForwardBlock { more: vec!["ops@brand.test".to_string()], ..ForwardBlock::internal("team@brand.test", keep) },
                ];
                for fwd in forms {
                    let s = UserSieve { ooo: with_ooo.then(|| ooo("x")), forward: Some(fwd) };
                    let src = compose(&s).expect("script");
                    assert_eq!(parse(&src), s, "{src}");
                    assert_eq!(compose(&parse(&src)).as_deref(), Some(src.as_str()));
                    // Never a mix: one bare redirect would cancel the kept copy.
                    let bare = src.matches("redirect \"").count();
                    let copies = src.matches("redirect :copy").count();
                    assert!(bare == 0 || copies == 0, "{src}");
                    assert_eq!(copies > 0, keep, "{src}");
                }
            }
        }
    }

    #[test]
    fn guard_host_validation() {
        assert!(guard_host_ok("mail.brand.test"));
        for bad in ["", "localhost", "mail.brand.test\"", "mail.*.test", "MAIL.brand.test", ".brand.test", "a b.test", "x;y.test"] {
            assert!(!guard_host_ok(bad), "{bad}");
        }
    }

    #[test]
    fn days_range_is_1_to_31() {
        assert_eq!(days_in_range(7).unwrap(), 7);
        assert_eq!(days_in_range(1).unwrap(), 1);
        assert_eq!(days_in_range(31).unwrap(), 31);
        assert!(days_in_range(0).unwrap_err().contains("1–31"));
        assert!(days_in_range(32).unwrap_err().contains("1–31"));
    }

    #[test]
    fn until_rejects_invalid() {
        assert!(parse_until_date("2026-13-01").is_err());
        assert!(parse_until_date("2026-02-30").is_err());
        assert!(parse_until_date("tomorrow").is_err());
        assert!(parse_until_date("26-09-18").is_err());
        assert_eq!(parse_until_date("2026-09-18").unwrap(), "2026-09-18");
    }

    #[test]
    fn lone_dot_line_rejected() {
        let err = reject_lone_dot_line("hello\n.\nworld", "vacation text").unwrap_err();
        assert!(err.contains("lone '.'"), "{err}");
        reject_lone_dot_line("hello\n.not-alone\nworld", "vacation text").unwrap();
    }

    #[test]
    fn html_rejected() {
        assert!(reject_html("<html><body>x</body></html>").is_err());
        assert!(reject_html("line<br>line").is_err());
        reject_html("Best regards,\nAcme Legal").unwrap();
    }

    #[test]
    fn text_cap_4096() {
        cap_text(&"a".repeat(4096), "footer").unwrap();
        let err = cap_text(&"a".repeat(4097), "footer").unwrap_err();
        assert!(err.contains("4096"), "{err}");
        assert!(err.contains("4097"), "{err}");
    }
}
