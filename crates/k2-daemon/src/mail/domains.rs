//! Domain onboarding — K2 Mail S2 (prd-email-server-v1 §6).
//!
//! The lifecycle this module owns:
//!
//! 1. **Add** ([`add_domain`]): normalize (pre-mortem #14 — ONLY the
//!    lowercase punycode A-label form ever reaches the DB or Stalwart)
//!    → Stalwart `Domain/set create` with automatic DKIM → read the
//!    server-set `dnsZoneFile` (the SSOT — K2 computes nothing itself
//!    except the relay-mode SPF adjustment, §8.3) → parse it into the
//!    per-record table ([`parse_zone_file`] + [`build_rows`]) → persist
//!    to `mail_domains.dns_status_json`.
//! 2. **Verify** (`super::dns_verify`): each required record resolves
//!    to `Valid | Missing | Wrong` with an expected-vs-live diff —
//!    NEVER a bare "missing" (pre-mortem #4). The domain flips
//!    **Verified** when MX + SPF + ≥1 DKIM are Valid; DMARC nags but
//!    never blocks (§6.3).
//! 3. **Remove** ([`remove_domain`]): explicit `confirm` required;
//!    retires the domain's addresses (plain-SQL UPDATE on
//!    `mail_addresses` — the S3 addresses slice owns the address
//!    ROUTES, this module owns this one write), destroys the Stalwart
//!    domain, deletes the K2 row. Mail-data purge is a separate flag.
//!
//! **SPF split-config (§8.3):** the SPF row's expected value is
//! computed AT READ TIME from the domain's CURRENT `send_mode` +
//! `mail_relay_configs.spf_include` ([`apply_send_mode_spf`]) — never
//! baked into the stored table — so an S5 mode switch is reflected
//! everywhere (show/check/zone download) without a rebuild, and the
//! include string always comes from the column, never a hardcoded
//! provider list (they drift).
//!
//! **DKIM >255-char TXT values** (pre-mortem #4/#16): Stalwart splits
//! long TXT rdata into multiple quoted strings. The parser KEEPS the
//! chunks ([`ZoneRecord::chunks`]); comparison joins them
//! (`dns_verify::join_chunks`) and display/copy preserves the split
//! form (`expected_display`) so a BIND-style registrar gets the exact
//! zone syntax while the copy string is the joined value most
//! registrar UIs want.

use k2_core::db::schema::MailDomain;
use rusqlite::Connection;

use super::jmap::{CreatedDomain, StalwartClient};

// ── Record-table model (persisted as `dns_status_json`) ────────────────

/// Row categories: `required` records gate verification; the PTR
/// `instruction` row can't be verified from DNS (it lives at the VPS
/// provider — pre-mortem #5); `advanced` rows are the optional drawer
/// (autoconfig / MTA-STS / TLS-RPT / TLSA…, §6.5 — shown, never gated).
pub const CAT_REQUIRED: &str = "required";
pub const CAT_INSTRUCTION: &str = "instruction";
pub const CAT_ADVANCED: &str = "advanced";

/// Per-record verification states. `pending` = not yet checked;
/// `unknown` = the last resolver attempt errored (transient — never
/// regresses a domain); `unverifiable` = the PTR instruction row;
/// `optional` = advanced rows (no verification gating in V1).
pub const ST_PENDING: &str = "pending";
pub const ST_VALID: &str = "valid";
pub const ST_MISSING: &str = "missing";
pub const ST_WRONG: &str = "wrong";
pub const ST_UNKNOWN: &str = "unknown";
pub const ST_UNVERIFIABLE: &str = "unverifiable";
pub const ST_OPTIONAL: &str = "optional";

/// One row of the DNS record table (§6.2). Wire + storage shape (the
/// routes serialize these camelCase straight into responses).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordRow {
    /// Stable row key: `mx` | `spf` | `dkim:<selector>` | `dmarc` |
    /// `ptr` | `adv:<n>`.
    pub id: String,
    pub category: String,
    #[serde(rename = "type")]
    pub rtype: String,
    /// Owner name, fully qualified, WITHOUT the trailing dot.
    pub name: String,
    /// Human label ("SPF (sender policy)", "DKIM signing key (…)").
    pub purpose: String,
    /// The COPY value: TXT chunks joined into one string (what most
    /// registrar UIs want pasted), other types verbatim rdata.
    pub expected: String,
    /// The zone-file form: TXT values >255 chars keep their quoted
    /// split (`"chunk1" "chunk2"`), pre-mortem #4/#16.
    pub expected_display: String,
    /// TXT chunks (empty for non-TXT rows).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunks: Vec<String>,
    pub status: String,
    /// What DNS actually serves at `name` — populated on Wrong (the
    /// diff) and, for context, on Missing when unrelated records exist
    /// at the same name (pre-mortem #4: show what IS there).
    #[serde(default)]
    pub live: Option<Vec<String>>,
    #[serde(default)]
    pub checked_at: Option<i64>,
}

/// The persisted `mail_domains.dns_status_json` payload: the full
/// record table + the raw Stalwart zone file it was built from.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsStatus {
    #[serde(default)]
    pub records: Vec<RecordRow>,
    /// Raw `dnsZoneFile` text from Stalwart (SSOT for rebuilds).
    #[serde(default)]
    pub zone_file: String,
}

// ── Zone-file parsing ───────────────────────────────────────────────────

/// One parsed zone-file record, pre-classification.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneRecord {
    /// Lowercased, single trailing dot stripped.
    pub name: String,
    /// Uppercased type mnemonic (MX, TXT, CNAME, SRV, TLSA, …).
    pub rtype: String,
    /// Non-TXT rdata joined with single spaces (e.g. `10 mail.x.`).
    pub rdata: String,
    /// TXT rdata chunks with quotes removed, split preserved.
    pub chunks: Vec<String>,
}

/// Zone-line token: bare word or quoted string (quotes removed).
#[derive(Debug, Clone, PartialEq)]
enum ZTok {
    Bare(String),
    Quoted(String),
}

/// Join RFC-1035 parenthesized continuations into logical lines and
/// strip comments (`;` outside quotes).
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for raw in text.lines() {
        let mut in_quotes = false;
        let mut escaped = false;
        for c in raw.chars() {
            if escaped {
                cur.push(c);
                escaped = false;
                continue;
            }
            match c {
                '\\' if in_quotes => {
                    cur.push(c);
                    escaped = true;
                }
                '"' => {
                    in_quotes = !in_quotes;
                    cur.push(c);
                }
                ';' if !in_quotes => break, // comment to end of line
                '(' if !in_quotes => depth += 1,
                ')' if !in_quotes => depth = depth.saturating_sub(1),
                _ => cur.push(c),
            }
        }
        if depth == 0 {
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        } else {
            cur.push(' '); // continuation — join with a space
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Tokenize one logical line into bare words + quoted strings
/// (`\"` and `\\` escapes honored inside quotes).
fn tokenize(line: &str) -> Vec<ZTok> {
    let mut toks = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '"' {
            chars.next();
            let mut s = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                            s.push(n);
                        }
                    }
                    '"' => break,
                    _ => s.push(c),
                }
            }
            toks.push(ZTok::Quoted(s));
        } else {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                s.push(c);
                chars.next();
            }
            toks.push(ZTok::Bare(s));
        }
    }
    toks
}

/// The DNS record types we expect to meet in a Stalwart zone file —
/// used to find the type token among `name [ttl] [class] TYPE rdata`.
fn is_record_type(tok: &str) -> bool {
    matches!(
        tok.to_ascii_uppercase().as_str(),
        "MX" | "TXT"
            | "CNAME"
            | "SRV"
            | "A"
            | "AAAA"
            | "TLSA"
            | "NS"
            | "PTR"
            | "SPF"
            | "HTTPS"
            | "SVCB"
            | "CAA"
    )
}

/// Parse a Stalwart `dnsZoneFile` into records. ROBUST by design
/// (pre-mortem: Stalwart's exact formatting is only confirmable on a
/// live box): optional TTL and class fields, `;` comments,
/// parenthesized continuations, multi-quoted TXT splits. Lines that
/// don't parse as records are SKIPPED, never fatal — a partially
/// parsed table still renders, and the raw zone file is persisted
/// alongside for the download payload.
pub fn parse_zone_file(text: &str) -> Vec<ZoneRecord> {
    let mut out = Vec::new();
    for line in logical_lines(text) {
        let toks = tokenize(&line);
        // First token must be a bare owner name.
        let Some(ZTok::Bare(raw_name)) = toks.first() else {
            continue;
        };
        // Directives ($ORIGIN/$TTL) and empty owners are skipped.
        if raw_name.starts_with('$') {
            continue;
        }
        // Find the TYPE token among the next few fields (skipping an
        // optional numeric TTL and an optional class).
        let mut type_idx = None;
        for (i, tok) in toks.iter().enumerate().skip(1).take(3) {
            if let ZTok::Bare(t) = tok {
                let up = t.to_ascii_uppercase();
                if up.chars().all(|c| c.is_ascii_digit()) {
                    continue; // TTL
                }
                if matches!(up.as_str(), "IN" | "CH" | "CS" | "HS") {
                    continue; // class
                }
                if is_record_type(&up) {
                    type_idx = Some(i);
                }
                break;
            }
        }
        let Some(ti) = type_idx else { continue };
        let ZTok::Bare(rtype) = &toks[ti] else { continue };
        let rtype = rtype.to_ascii_uppercase();
        let rest = &toks[ti + 1..];
        if rest.is_empty() {
            continue;
        }
        let mut name = raw_name.to_ascii_lowercase();
        if name.ends_with('.') {
            name.pop();
        }
        if rtype == "TXT" || rtype == "SPF" {
            let chunks: Vec<String> = rest
                .iter()
                .map(|t| match t {
                    ZTok::Quoted(s) => s.clone(),
                    ZTok::Bare(s) => s.clone(),
                })
                .collect();
            let rdata = chunks.concat();
            out.push(ZoneRecord { name, rtype: "TXT".to_string(), rdata, chunks });
        } else {
            let rdata = rest
                .iter()
                .map(|t| match t {
                    ZTok::Quoted(s) => format!("\"{s}\""),
                    ZTok::Bare(s) => s.clone(),
                })
                .collect::<Vec<_>>()
                .join(" ");
            out.push(ZoneRecord { name, rtype, rdata, chunks: Vec::new() });
        }
    }
    out
}

// ── Zone records → the record table ────────────────────────────────────

/// Quote a TXT chunk list back into zone-file display form.
fn quoted_display(chunks: &[String]) -> String {
    chunks
        .iter()
        .map(|c| format!("\"{}\"", c.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn txt_row(id: &str, name: &str, purpose: &str, chunks: Vec<String>, category: &str) -> RecordRow {
    let expected = chunks.concat();
    RecordRow {
        id: id.to_string(),
        category: category.to_string(),
        rtype: "TXT".to_string(),
        name: name.to_string(),
        purpose: purpose.to_string(),
        expected,
        expected_display: quoted_display(&chunks),
        chunks,
        status: if category == CAT_ADVANCED { ST_OPTIONAL } else { ST_PENDING }.to_string(),
        live: None,
        checked_at: None,
    }
}

/// Human label for an advanced-drawer record (§6.5).
fn advanced_purpose(name: &str, rtype: &str) -> String {
    if name.contains("autoconfig") || name.contains("autodiscover") {
        return "Mail client autoconfig (optional)".to_string();
    }
    if name.contains("mta-sts") {
        return "MTA-STS (optional)".to_string();
    }
    if name.contains("_smtp._tls") {
        return "TLS reporting (TLS-RPT, optional)".to_string();
    }
    match rtype {
        "TLSA" => "DANE (TLSA, optional)".to_string(),
        "SRV" => "Service discovery (SRV, optional)".to_string(),
        "A" | "AAAA" => "Mail host address (optional here — set with the server hostname)".to_string(),
        _ => "Advanced (optional)".to_string(),
    }
}

/// Classify parsed zone records into the §6.2 record table: MX + SPF +
/// DKIM(×selectors) + DMARC as `required`, the synthesized PTR
/// instruction row (pre-mortem #5: rDNS lives at the VPS provider, not
/// the DNS zone), and everything else into the `advanced` drawer.
/// Missing SPF/DMARC lines are synthesized defensively (SPF `mx -all`,
/// DMARC `p=none` per pre-mortem #16 — observation before enforcement)
/// so verification always has its required rows.
pub fn build_rows(domain: &str, zone: &[ZoneRecord], hostname: Option<&str>) -> Vec<RecordRow> {
    let mut mx: Option<RecordRow> = None;
    let mut spf: Option<RecordRow> = None;
    let mut dkim: Vec<RecordRow> = Vec::new();
    let mut dmarc: Option<RecordRow> = None;
    let mut advanced: Vec<RecordRow> = Vec::new();

    let dkim_suffix = format!("._domainkey.{domain}");
    let dmarc_name = format!("_dmarc.{domain}");

    for rec in zone {
        if rec.rtype == "MX" && rec.name == domain {
            mx = Some(RecordRow {
                id: "mx".to_string(),
                category: CAT_REQUIRED.to_string(),
                rtype: "MX".to_string(),
                name: rec.name.clone(),
                purpose: "Mail routing (MX)".to_string(),
                expected: rec.rdata.clone(),
                expected_display: rec.rdata.clone(),
                chunks: Vec::new(),
                status: ST_PENDING.to_string(),
                live: None,
                checked_at: None,
            });
            continue;
        }
        if rec.rtype == "TXT" {
            let joined = rec.chunks.concat();
            let joined_lc = joined.to_ascii_lowercase();
            if rec.name == domain && joined_lc.starts_with("v=spf1") {
                spf = Some(txt_row("spf", &rec.name, "SPF (sender policy)", rec.chunks.clone(), CAT_REQUIRED));
                continue;
            }
            if let Some(selector) = rec.name.strip_suffix(&dkim_suffix) {
                let key_kind = if joined_lc.contains("k=ed25519") {
                    "Ed25519"
                } else if joined_lc.contains("k=rsa") || joined_lc.contains("p=mii") {
                    "RSA"
                } else {
                    "key"
                };
                dkim.push(txt_row(
                    &format!("dkim:{selector}"),
                    &rec.name,
                    &format!("DKIM signing key ({selector}, {key_kind})"),
                    rec.chunks.clone(),
                    CAT_REQUIRED,
                ));
                continue;
            }
            if rec.name == dmarc_name && joined_lc.starts_with("v=dmarc1") {
                dmarc = Some(txt_row(
                    "dmarc",
                    &rec.name,
                    "DMARC policy (strongly recommended — nagged, not blocking)",
                    rec.chunks.clone(),
                    CAT_REQUIRED,
                ));
                continue;
            }
        }
        // Everything else → advanced drawer, straight from the zone
        // file, no verification gating (§6.5).
        let idx = advanced.len();
        let purpose = advanced_purpose(&rec.name, &rec.rtype);
        advanced.push(if rec.rtype == "TXT" {
            txt_row(&format!("adv:{idx}"), &rec.name, &purpose, rec.chunks.clone(), CAT_ADVANCED)
        } else {
            RecordRow {
                id: format!("adv:{idx}"),
                category: CAT_ADVANCED.to_string(),
                rtype: rec.rtype.clone(),
                name: rec.name.clone(),
                purpose,
                expected: rec.rdata.clone(),
                expected_display: rec.rdata.clone(),
                chunks: Vec::new(),
                status: ST_OPTIONAL.to_string(),
                live: None,
                checked_at: None,
            }
        });
    }

    // Defensive synthesis for absent required lines (never for DKIM —
    // the keys only exist server-side in Stalwart).
    let spf = spf.unwrap_or_else(|| {
        txt_row("spf", domain, "SPF (sender policy)", vec!["v=spf1 mx -all".to_string()], CAT_REQUIRED)
    });
    let dmarc = dmarc.unwrap_or_else(|| {
        txt_row(
            "dmarc",
            &dmarc_name,
            "DMARC policy (strongly recommended — nagged, not blocking)",
            vec![format!("v=DMARC1; p=none; rua=mailto:postmaster@{domain}")],
            CAT_REQUIRED,
        )
    });
    let mx = mx.or_else(|| {
        hostname.map(|h| RecordRow {
            id: "mx".to_string(),
            category: CAT_REQUIRED.to_string(),
            rtype: "MX".to_string(),
            name: domain.to_string(),
            purpose: "Mail routing (MX)".to_string(),
            expected: format!("10 {h}."),
            expected_display: format!("10 {h}."),
            chunks: Vec::new(),
            status: ST_PENDING.to_string(),
            live: None,
            checked_at: None,
        })
    });

    // PTR instruction row (§6.2 — "? Unverifiable until set").
    let host_label = hostname.map(String::from).unwrap_or_else(|| "<your mail hostname>".to_string());
    let ptr = RecordRow {
        id: "ptr".to_string(),
        category: CAT_INSTRUCTION.to_string(),
        rtype: "PTR".to_string(),
        name: host_label.clone(),
        purpose: "Reverse DNS (set at your VPS provider, not in this zone)".to_string(),
        expected: format!(
            "Set the reverse DNS (PTR) of your server's IP to {host_label} in your VPS provider's panel (Hetzner/DigitalOcean/…) — it cannot be set at your domain registrar"
        ),
        expected_display: String::new(),
        chunks: Vec::new(),
        status: ST_UNVERIFIABLE.to_string(),
        live: None,
        checked_at: None,
    };

    let mut rows = Vec::new();
    if let Some(mx) = mx {
        rows.push(mx);
    }
    rows.push(spf);
    rows.extend(dkim);
    rows.push(dmarc);
    rows.push(ptr);
    rows.extend(advanced);
    rows
}

// ── SPF split-config (§8.3) ─────────────────────────────────────────────

/// The SPF value the record table shows for a send mode. Relay mode
/// swaps in the include form built from the CONFIRMED include string
/// (from `mail_relay_configs.spf_include`); with no include on file
/// yet, the zone-file SPF stands (S5's relay-config flow sets it).
/// Direct and receive-only render the zone file's SPF untouched.
pub fn spf_expected(send_mode: &str, zone_spf: &str, relay_include: Option<&str>) -> String {
    if send_mode == "relay" {
        if let Some(inc) = relay_include.map(str::trim).filter(|s| !s.is_empty()) {
            return format!("v=spf1 mx include:{inc} ~all");
        }
    }
    zone_spf.to_string()
}

/// Apply the send-mode SPF adjustment to a loaded record table. Runs
/// at READ time (show/check/zone download) so a mode change reflects
/// immediately without a stored rebuild.
pub fn apply_send_mode_spf(rows: &mut [RecordRow], send_mode: &str, relay_include: Option<&str>) {
    for row in rows.iter_mut() {
        if row.id == "spf" {
            let value = spf_expected(send_mode, &row.expected, relay_include);
            if value != row.expected {
                row.expected_display = format!("\"{value}\"");
                row.chunks = vec![value.clone()];
                row.expected = value;
            }
            return;
        }
    }
}

/// Render the record table back into a downloadable zone file
/// (§6.2 header button). Regenerated from the EFFECTIVE rows — not the
/// raw Stalwart text — so the relay-mode SPF adjustment is what the
/// user downloads. TXT values keep their quoted split form.
pub fn render_zone_file(domain: &str, rows: &[RecordRow]) -> String {
    let mut out = format!("; K2 Mail DNS records for {domain}\n; records can take up to 48 h to propagate\n");
    for row in rows {
        if row.category == CAT_INSTRUCTION {
            continue;
        }
        let rdata = if row.rtype == "TXT" { row.expected_display.clone() } else { row.expected.clone() };
        out.push_str(&format!("{}.\t3600\tIN\t{}\t{}\n", row.name, row.rtype, rdata));
    }
    out
}

// ── DB access ───────────────────────────────────────────────────────────

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn map_domain_row(r: &rusqlite::Row) -> rusqlite::Result<MailDomain> {
    Ok(MailDomain {
        id: r.get(0)?,
        domain: r.get(1)?,
        stalwart_domain_id: r.get(2)?,
        send_mode: r.get(3)?,
        relay_config_id: r.get(4)?,
        status: r.get(5)?,
        dns_status_json: r.get(6)?,
        verified_at: r.get(7)?,
        last_checked_at: r.get(8)?,
        created_at: r.get(9)?,
    })
}

const DOMAIN_COLS: &str = "id, domain, stalwart_domain_id, send_mode, relay_config_id, \
                           status, dns_status_json, verified_at, last_checked_at, created_at";

/// Load one domain row by NORMALIZED domain name.
pub fn load_domain(conn: &Connection, domain: &str) -> Option<MailDomain> {
    conn.query_row(
        &format!("SELECT {DOMAIN_COLS} FROM mail_domains WHERE domain = ?1"),
        rusqlite::params![domain],
        |r| map_domain_row(r),
    )
    .ok()
}

/// All domain rows, oldest first (stable default-domain order).
pub fn load_all_domains(conn: &Connection) -> Vec<MailDomain> {
    let mut stmt = match conn
        .prepare(&format!("SELECT {DOMAIN_COLS} FROM mail_domains ORDER BY created_at, domain"))
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |r| map_domain_row(r))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Parse a row's stored `dns_status_json` (empty default when unset —
/// never a panic on legacy/garbage content, just an empty table).
pub fn dns_status_of(row: &MailDomain) -> DnsStatus {
    row.dns_status_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

/// The relay SPF include for a domain, from its relay config row
/// (`mail_relay_configs.spf_include` — the owner-confirmed string, §8.3).
pub fn relay_spf_include(conn: &Connection, relay_config_id: Option<&str>) -> Option<String> {
    let id = relay_config_id?;
    conn.query_row(
        "SELECT spf_include FROM mail_relay_configs WHERE id = ?1",
        rusqlite::params![id],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
    .filter(|s| !s.trim().is_empty())
}

/// A domain's EFFECTIVE record table: stored rows + the read-time
/// send-mode SPF adjustment.
pub fn effective_rows(conn: &Connection, row: &MailDomain) -> Vec<RecordRow> {
    let mut rows = dns_status_of(row).records;
    let include = relay_spf_include(conn, row.relay_config_id.as_deref());
    apply_send_mode_spf(&mut rows, &row.send_mode, include.as_deref());
    rows
}

/// Persist a verification pass: the updated record table +
/// `last_checked_at` + domain status (+ `verified_at` on the flip).
pub fn save_check(
    conn: &Connection,
    domain_id: &str,
    status: &DnsStatus,
    domain_status: &str,
    verified_at: Option<i64>,
    checked_at: i64,
) -> Result<(), String> {
    let json = serde_json::to_string(status).map_err(|e| format!("dns status serialize: {e}"))?;
    conn.execute(
        "UPDATE mail_domains SET dns_status_json = ?1, status = ?2, verified_at = ?3, \
         last_checked_at = ?4 WHERE id = ?5",
        rusqlite::params![json, domain_status, verified_at, checked_at, domain_id],
    )
    .map(|_| ())
    .map_err(|e| format!("dns status update: {e}"))
}

// ── Engine seam (Stalwart behind a trait — testable without network) ────

/// What S2 needs from Stalwart. The production impl is the JMAP
/// client; tests use a fake. Keeping the trait THIS narrow keeps the
/// AGPL boundary honest — everything else is K2-side state.
pub trait DomainEngine {
    fn create_domain(&self, domain: &str) -> Result<CreatedDomain, String>;
    fn delete_domain(&self, stalwart_domain_id: &str) -> Result<(), String>;
    fn dns_zonefile(&self, stalwart_domain_id: &str) -> Result<String, String>;
}

impl DomainEngine for StalwartClient {
    fn create_domain(&self, domain: &str) -> Result<CreatedDomain, String> {
        self.domain_create(domain)
    }
    fn delete_domain(&self, stalwart_domain_id: &str) -> Result<(), String> {
        self.domain_delete(stalwart_domain_id)
    }
    fn dns_zonefile(&self, stalwart_domain_id: &str) -> Result<String, String> {
        self.domain_dns_zonefile(stalwart_domain_id)
    }
}

/// Resolve a `mail_server.*_secret_ref` to its secret value.
///
/// S1 HANDSHAKE SEAM: the supervisor slice owns WHERE secrets live;
/// until it lands, the two spellings below are the contract it must
/// emit refs in (`env:<VAR>` or an absolute file path — 0600, root of
/// trust is the box itself). Anything else fails loudly here rather
/// than guessing.
fn resolve_secret_ref(secret_ref: &str) -> Result<String, String> {
    if let Some(var) = secret_ref.strip_prefix("env:") {
        return std::env::var(var).map_err(|_| format!("secret env var '{var}' is not set"));
    }
    let path = secret_ref.strip_prefix("file:").unwrap_or(secret_ref);
    if path.starts_with('/') {
        return std::fs::read_to_string(path)
            .map(|s| s.trim().to_string())
            .map_err(|e| format!("secret file '{path}': {e}"))
            .and_then(|s| {
                if s.is_empty() {
                    Err(format!("secret file '{path}' is empty"))
                } else {
                    Ok(s)
                }
            });
    }
    Err(format!(
        "unrecognized secret ref '{secret_ref}' (expected env:<VAR> or an absolute file path)"
    ))
}

/// The `mail_server` fields S2 needs (hostname rides along for the
/// PTR row + MX synthesis).
pub struct ServerInfo {
    pub status: String,
    pub hostname: Option<String>,
    pub api_url: Option<String>,
    pub api_key_ref: Option<String>,
}

/// Read the `mail_server` singleton; `None` = not installed.
pub fn server_info(conn: &Connection) -> Option<ServerInfo> {
    conn.query_row(
        "SELECT status, hostname, api_url, api_key_ref FROM mail_server WHERE id = 1",
        [],
        |r| {
            Ok(ServerInfo {
                status: r.get(0)?,
                hostname: r.get(1)?,
                api_url: r.get(2)?,
                api_key_ref: r.get(3)?,
            })
        },
    )
    .ok()
}

/// Build the production Stalwart client from the `mail_server` row.
/// Every failure names the fix (comprehension-gate style).
pub fn engine_from_db() -> Result<(StalwartClient, Option<String>), String> {
    let info = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        server_info(&conn)
    };
    let Some(info) = info else {
        return Err("the mail server is not installed — enable it in Settings → Email first".to_string());
    };
    if info.status != "running" && info.status != "degraded" {
        return Err(format!(
            "the mail server is {} — start it in Settings → Email before managing domains",
            info.status
        ));
    }
    let api_url = info
        .api_url
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "mail server has no api_url — re-run Enable in Settings → Email".to_string())?;
    let key_ref = info
        .api_key_ref
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "mail server has no API key — re-run Enable in Settings → Email".to_string())?;
    let api_key = resolve_secret_ref(&key_ref)?;
    Ok((StalwartClient::new(api_url, api_key), info.hostname))
}

// ── Operations (route bodies live here so they test with fakes) ─────────

/// Operation errors, mapped onto the route layer's stable
/// `{code, hint}` shapes.
#[derive(Debug)]
pub enum OpError {
    /// Bad input → 400 `usage`.
    Usage(String),
    /// Unknown domain → 404 `not_found`.
    NotFound(String),
    /// Domain already hosted → 409 `exists`.
    Exists(String),
    /// Remove without `confirm: true` → 409 `confirm_required`.
    ConfirmRequired(String),
    /// Stalwart said no / transport failed → 502 `engine`.
    Engine(String),
}

/// The §6.2 footer note, everywhere the table renders.
const PROPAGATION_NOTE: &str = "records can take up to 48 h to propagate";

/// Full per-domain JSON (the `show` shape; `add`/`check` return it too).
pub fn domain_json(conn: &Connection, row: &MailDomain) -> serde_json::Value {
    let rows = effective_rows(conn, row);
    let dmarc_valid = rows.iter().any(|r| r.id == "dmarc" && r.status == ST_VALID);
    serde_json::json!({
        "ok": true,
        "domain": row.domain,
        "status": row.status,
        "sendMode": row.send_mode,
        "verifiedAt": row.verified_at,
        "lastCheckedAt": row.last_checked_at,
        "createdAt": row.created_at,
        // The DMARC nag (§6.3): strongly recommended, never blocking.
        "dmarcNag": !dmarc_valid,
        "records": rows,
        "zoneFile": render_zone_file(&row.domain, &rows),
        "note": PROPAGATION_NOTE,
    })
}

/// Add a domain (§6.1-6.2): normalize → Stalwart create (auto-DKIM) →
/// zone file → record table → persist. Returns the record-table JSON.
pub fn add_domain(engine: &dyn DomainEngine, raw_domain: &str) -> Result<serde_json::Value, OpError> {
    let domain = k2_core::mail_domain::normalize_mail_domain(raw_domain).map_err(OpError::Usage)?;

    // Pre-flight duplicate check (the UNIQUE constraint backstops the
    // race below).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if load_domain(&conn, &domain).is_some() {
            return Err(OpError::Exists(format!("domain '{domain}' is already added")));
        }
    }

    let created = engine.create_domain(&domain).map_err(OpError::Engine)?;
    let zone_text = match created.dns_zone_file.clone() {
        Some(z) => z,
        None => match engine.dns_zonefile(&created.id) {
            Ok(z) => z,
            Err(e) => {
                // Don't leave a half-created domain in Stalwart that K2
                // doesn't track — best-effort undo, then fail loudly.
                let _ = engine.delete_domain(&created.id);
                return Err(OpError::Engine(format!("created but could not read dnsZoneFile: {e}")));
            }
        },
    };

    let db = k2_core::db::shared();
    let conn = db.lock();
    let hostname = server_info(&conn).and_then(|i| i.hostname);
    let records = build_rows(&domain, &parse_zone_file(&zone_text), hostname.as_deref());
    let status = DnsStatus { records, zone_file: zone_text };
    let json = serde_json::to_string(&status)
        .map_err(|e| OpError::Engine(format!("dns status serialize: {e}")))?;
    let id = uuid::Uuid::new_v4().to_string();
    let inserted = conn.execute(
        "INSERT INTO mail_domains (id, domain, stalwart_domain_id, send_mode, status, \
         dns_status_json, created_at) VALUES (?1, ?2, ?3, 'receive-only', 'pending', ?4, ?5)",
        rusqlite::params![id, domain, created.id, json, now_secs()],
    );
    if let Err(e) = inserted {
        let _ = engine.delete_domain(&created.id);
        return Err(if e.to_string().contains("UNIQUE") {
            OpError::Exists(format!("domain '{domain}' is already added"))
        } else {
            OpError::Engine(format!("domain insert: {e}"))
        });
    }
    let row = load_domain(&conn, &domain)
        .ok_or_else(|| OpError::Engine("domain row vanished after insert".to_string()))?;
    Ok(domain_json(&conn, &row))
}

/// Remove a domain (§6.6): requires `confirm: true` (the error names
/// the active-address count), retires the domain's addresses (this
/// module's ONE write into `mail_addresses`), destroys the Stalwart
/// domain, deletes the K2 row. `purge` (mail data) is acknowledged and
/// echoed; actual mailbox purge arrives with the S3 addresses slice —
/// before S3 no K2-minted mailbox data exists.
pub fn remove_domain(
    engine: Option<&dyn DomainEngine>,
    raw_domain: &str,
    confirm: bool,
    purge: bool,
) -> Result<serde_json::Value, OpError> {
    let domain = k2_core::mail_domain::normalize_mail_domain(raw_domain).map_err(OpError::Usage)?;

    let (row, active_addresses) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(row) = load_domain(&conn, &domain) else {
            return Err(OpError::NotFound(format!("domain '{domain}' is not hosted here")));
        };
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mail_addresses WHERE domain_id = ?1 AND status = 'active'",
                rusqlite::params![row.id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        (row, count)
    };

    if !confirm {
        return Err(OpError::ConfirmRequired(format!(
            "removing '{domain}' retires {active_addresses} active address(es) and destroys the \
             domain in the mail server — repeat with confirm: true"
        )));
    }

    // Stalwart first: if the engine refuses, nothing local changed.
    let mut stalwart_deleted = false;
    if let Some(stalwart_id) = row.stalwart_domain_id.as_deref() {
        let Some(engine) = engine else {
            return Err(OpError::Engine(
                "the mail server is unavailable — start it in Settings → Email, then retry the remove"
                    .to_string(),
            ));
        };
        engine.delete_domain(stalwart_id).map_err(OpError::Engine)?;
        stalwart_deleted = true;
    }

    let retired = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let retired = conn
            .execute(
                "UPDATE mail_addresses SET status = 'retired', retired_at = ?1 \
                 WHERE domain_id = ?2 AND status = 'active'",
                rusqlite::params![now_secs(), row.id],
            )
            .unwrap_or(0);
        let _ = conn.execute(
            "DELETE FROM mail_domains WHERE id = ?1",
            rusqlite::params![row.id],
        );
        retired
    };

    Ok(serde_json::json!({
        "ok": true,
        "domain": domain,
        "retiredAddresses": retired,
        "stalwartDeleted": stalwart_deleted,
        "purge": purge,
    }))
}

/// Owner `domain/list`: every domain + status chips + a per-record
/// state summary.
pub fn list_owner_json() -> serde_json::Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let domains: Vec<serde_json::Value> = load_all_domains(&conn)
        .iter()
        .map(|row| {
            let rows = effective_rows(&conn, row);
            let count = |st: &str| rows.iter().filter(|r| r.status == st).count();
            serde_json::json!({
                "domain": row.domain,
                "status": row.status,
                "sendMode": row.send_mode,
                "verifiedAt": row.verified_at,
                "lastCheckedAt": row.last_checked_at,
                "dmarcNag": !rows.iter().any(|r| r.id == "dmarc" && r.status == ST_VALID),
                "records": {
                    "valid": count(ST_VALID),
                    "missing": count(ST_MISSING),
                    "wrong": count(ST_WRONG),
                    "pending": count(ST_PENDING),
                    "unknown": count(ST_UNKNOWN),
                },
            })
        })
        .collect();
    serde_json::json!({ "ok": true, "domains": domains })
}

/// Pure default-domain resolution (§11.1.2): the configured
/// `mail_default_domain` setting when it is currently a Verified
/// domain, else the first Verified domain (oldest added). `verified`
/// must be in `load_all_domains` order.
pub fn resolve_default_domain(configured: &str, verified: &[String]) -> Option<String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        // Tolerate an un-normalized stored value.
        let normalized = k2_core::mail_domain::normalize_mail_domain(configured)
            .unwrap_or_else(|_| configured.to_string());
        if verified.iter().any(|d| d == &normalized) {
            return Some(normalized);
        }
    }
    verified.first().cloned()
}

/// Agent `domains` view (comprehension-gate §11.1.2): ONLY Verified
/// domains (what `k2 mail create` can mint on) + the workspace default
/// — resolved from the GLOBAL `mail_default_domain` setting, empty =
/// first-verified.
pub fn list_agent_json() -> serde_json::Value {
    let verified: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        load_all_domains(&conn)
            .into_iter()
            .filter(|d| d.status == "verified")
            .map(|d| d.domain)
            .collect()
    };
    let configured = k2_core::app_settings::load().mail_default_domain;
    let default = resolve_default_domain(&configured, &verified);
    let domains: Vec<serde_json::Value> = verified
        .iter()
        .map(|d| {
            serde_json::json!({
                "domain": d,
                "isDefault": Some(d) == default.as_ref(),
            })
        })
        .collect();
    serde_json::json!({ "ok": true, "domains": domains, "defaultDomain": default })
}

/// `domain/show`: the full record table for one domain.
pub fn show_json(raw_domain: &str) -> Result<serde_json::Value, OpError> {
    let domain = k2_core::mail_domain::normalize_mail_domain(raw_domain).map_err(OpError::Usage)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = load_domain(&conn, &domain)
        .ok_or_else(|| OpError::NotFound(format!("domain '{domain}' is not hosted here")))?;
    Ok(domain_json(&conn, &row))
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A realistic Stalwart-shaped dnsZoneFile for acme.dev — MX, SPF,
    /// two DKIM selectors (the RSA one split into multiple quoted
    /// strings, >255-char rdata), DMARC, and the advanced set
    /// (autoconfig/autodiscover CNAMEs, MTA-STS, TLS-RPT, TLSA, SRV,
    /// host A record). Comments + tabs + a parenthesized continuation
    /// exercise the parser's tolerance.
    ///
    /// LIVE-BOX FLAG: this fixture encodes our best model of Stalwart
    /// v0.16.x output; S2's first live run must diff a real
    /// `dnsZoneFile` against it (esp. whether long DKIM TXT rdata is
    /// pre-split into quoted strings, and TTL/class column presence).
    pub(crate) const ZONE_FIXTURE: &str = r#"; Generated by Stalwart for acme.dev
acme.dev.	3600	IN	MX	10 mail.acme.dev.
202601e._domainkey.acme.dev.	86400	IN	TXT	"v=DKIM1; k=ed25519; p=Fo0barEd25519KeyMaterialAAAA="
202601r._domainkey.acme.dev.	86400	IN	TXT	"v=DKIM1; k=rsa; p=MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA0FirstChunkOfALongRsaKeyThatExceedsTwoHundredAndFiftyFiveCharactersWhenPublishedAsASingleStringSoStalwartSplitsIt" "SecondChunkOfTheKeyMaterialContinuingTheBase64PayloadUntilTheEndIDAQAB"
acme.dev.	3600	IN	TXT	"v=spf1 mx -all"
_dmarc.acme.dev.	86400	IN	TXT	"v=DMARC1; p=none; rua=mailto:postmaster@acme.dev"
autoconfig.acme.dev.	3600	IN	CNAME	mail.acme.dev.
autodiscover.acme.dev.	3600	IN	CNAME	mail.acme.dev.
mta-sts.acme.dev.	3600	IN	CNAME	mail.acme.dev.
_mta-sts.acme.dev.	86400	IN	TXT	"v=STSv1; id=1719000000"
_smtp._tls.acme.dev.	86400	IN	TXT	"v=TLSRPTv1; rua=mailto:postmaster@acme.dev"
_25._tcp.mail.acme.dev.	3600	IN	TLSA	3 1 1 5f4dbe28cbcae1f6e10bcc16b6e7bcd63c364bbdedcbc8ef58b5a8d4f6e0a9c1
_jmap._tcp.acme.dev.	86400	IN	SRV	0 1 443 mail.acme.dev.
mail.acme.dev.	3600	IN	A	203.0.113.7
"#;

    /// A minimal fixture variant: no TTL, no class, lowercase type —
    /// the "format drifted" case the parser must survive.
    const ZONE_FIXTURE_SPARSE: &str = "acme.dev. mx 10 mail.acme.dev.\nacme.dev. txt \"v=spf1 mx -all\" ; inline comment\n";

    /// Build the standard fixture rows (shared with dns_verify tests).
    pub(crate) fn fixture_rows() -> Vec<RecordRow> {
        build_rows("acme.dev", &parse_zone_file(ZONE_FIXTURE), Some("mail.acme.dev"))
    }

    // ── Zone-file parser ──

    #[test]
    fn zone_fixture_parses_all_records() {
        let recs = parse_zone_file(ZONE_FIXTURE);
        assert_eq!(recs.len(), 13, "{recs:#?}");
        let mx = recs.iter().find(|r| r.rtype == "MX").expect("MX parsed");
        assert_eq!(mx.name, "acme.dev", "trailing dot stripped");
        assert_eq!(mx.rdata, "10 mail.acme.dev.");
        // The split RSA DKIM TXT keeps BOTH chunks (pre-mortem #4/#16).
        let rsa = recs
            .iter()
            .find(|r| r.name == "202601r._domainkey.acme.dev")
            .expect("RSA DKIM parsed");
        assert_eq!(rsa.chunks.len(), 2, "split preserved");
        assert!(rsa.rdata.ends_with("IDAQAB"), "chunks joined for comparison");
        assert!(!rsa.rdata.contains("\" \""), "join has no quote residue");
        // TLSA/SRV/A survive as advanced-typed records.
        assert!(recs.iter().any(|r| r.rtype == "TLSA"));
        assert!(recs.iter().any(|r| r.rtype == "SRV"));
        assert!(recs.iter().any(|r| r.rtype == "A"));
    }

    #[test]
    fn zone_parser_tolerates_sparse_and_garbage_lines() {
        let recs = parse_zone_file(ZONE_FIXTURE_SPARSE);
        assert_eq!(recs.len(), 2, "{recs:#?}");
        assert_eq!(recs[0].rtype, "MX");
        assert_eq!(recs[1].chunks, vec!["v=spf1 mx -all"], "comment stripped");

        // Garbage never panics, never fabricates records.
        assert!(parse_zone_file("").is_empty());
        assert!(parse_zone_file("$ORIGIN acme.dev.\n; just a comment\n\n").is_empty());
        // ("a" is a real record TYPE, so the garbage line must not
        // accidentally contain one-letter words.)
        assert!(parse_zone_file("this is definitely zero zone data").is_empty());
    }

    #[test]
    fn zone_parser_joins_parenthesized_continuations() {
        let text = "_dmarc.acme.dev. 3600 IN TXT ( \"v=DMARC1; p=none; \"\n    \"rua=mailto:postmaster@acme.dev\" )\n";
        let recs = parse_zone_file(text);
        assert_eq!(recs.len(), 1, "{recs:#?}");
        assert_eq!(recs[0].chunks.len(), 2);
        assert_eq!(
            recs[0].rdata,
            "v=DMARC1; p=none; rua=mailto:postmaster@acme.dev"
        );
    }

    // ── Row building / classification ──

    #[test]
    fn fixture_rows_classify_into_the_prd_table() {
        let rows = fixture_rows();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        // Order: MX, SPF, DKIM×2, DMARC, PTR, advanced drawer.
        assert_eq!(
            &ids[..6],
            &["mx", "spf", "dkim:202601e", "dkim:202601r", "dmarc", "ptr"]
        );
        assert_eq!(rows.iter().filter(|r| r.category == CAT_ADVANCED).count(), 8);

        let spf = rows.iter().find(|r| r.id == "spf").unwrap();
        assert_eq!(spf.expected, "v=spf1 mx -all");
        assert_eq!(spf.status, ST_PENDING);

        // The split DKIM row: joined copy string, split display form.
        let rsa = rows.iter().find(|r| r.id == "dkim:202601r").unwrap();
        assert_eq!(rsa.chunks.len(), 2);
        assert!(!rsa.expected.contains('"'), "copy string is the JOINED value");
        assert!(
            rsa.expected_display.starts_with('"') && rsa.expected_display.contains("\" \""),
            "display preserves the quoted split: {}",
            rsa.expected_display
        );
        assert!(rsa.purpose.contains("RSA"));
        let ed = rows.iter().find(|r| r.id == "dkim:202601e").unwrap();
        assert!(ed.purpose.contains("Ed25519"));

        // PTR instruction row: unverifiable, names the hostname, and
        // points at the VPS provider (pre-mortem #5).
        let ptr = rows.iter().find(|r| r.id == "ptr").unwrap();
        assert_eq!(ptr.category, CAT_INSTRUCTION);
        assert_eq!(ptr.status, ST_UNVERIFIABLE);
        assert!(ptr.expected.contains("mail.acme.dev"));
        assert!(ptr.expected.contains("VPS provider"));

        // Advanced rows carry the optional status, never pending.
        assert!(rows
            .iter()
            .filter(|r| r.category == CAT_ADVANCED)
            .all(|r| r.status == ST_OPTIONAL));
    }

    #[test]
    fn missing_required_lines_are_synthesized_defensively() {
        // A zone with ONLY DKIM (worst-case drift): SPF + DMARC are
        // synthesized (p=none per pre-mortem #16), MX from hostname.
        let zone = "sel._domainkey.acme.dev. IN TXT \"v=DKIM1; k=ed25519; p=abc\"\n";
        let rows = build_rows("acme.dev", &parse_zone_file(zone), Some("mail.acme.dev"));
        let mx = rows.iter().find(|r| r.id == "mx").expect("MX synthesized");
        assert_eq!(mx.expected, "10 mail.acme.dev.");
        let spf = rows.iter().find(|r| r.id == "spf").expect("SPF synthesized");
        assert_eq!(spf.expected, "v=spf1 mx -all");
        let dmarc = rows.iter().find(|r| r.id == "dmarc").expect("DMARC synthesized");
        assert!(dmarc.expected.contains("p=none"), "{}", dmarc.expected);
        // Without a hostname there is no MX to synthesize — no panic,
        // no fabricated exchange.
        let rows = build_rows("acme.dev", &parse_zone_file(zone), None);
        assert!(rows.iter().all(|r| r.id != "mx"));
    }

    // ── SPF split-config (§8.3) ──

    #[test]
    fn spf_row_respects_send_mode() {
        // Direct + receive-only: the zone file's SPF, untouched.
        assert_eq!(spf_expected("direct", "v=spf1 mx -all", None), "v=spf1 mx -all");
        assert_eq!(
            spf_expected("receive-only", "v=spf1 mx -all", Some("spf.mailgun.org")),
            "v=spf1 mx -all"
        );
        // Relay: the include form, from the COLUMN value.
        assert_eq!(
            spf_expected("relay", "v=spf1 mx -all", Some("spf.mailgun.org")),
            "v=spf1 mx include:spf.mailgun.org ~all"
        );
        assert_eq!(
            spf_expected("relay", "v=spf1 mx -all", Some("amazonses.com")),
            "v=spf1 mx include:amazonses.com ~all"
        );
        // Relay with no include on file yet: zone SPF stands.
        assert_eq!(spf_expected("relay", "v=spf1 mx -all", None), "v=spf1 mx -all");
        assert_eq!(spf_expected("relay", "v=spf1 mx -all", Some("  ")), "v=spf1 mx -all");
    }

    #[test]
    fn apply_send_mode_spf_rewrites_only_the_spf_row() {
        let mut rows = fixture_rows();
        apply_send_mode_spf(&mut rows, "relay", Some("_spf.smtp2go.com"));
        let spf = rows.iter().find(|r| r.id == "spf").unwrap();
        assert_eq!(spf.expected, "v=spf1 mx include:_spf.smtp2go.com ~all");
        assert_eq!(spf.chunks, vec![spf.expected.clone()]);
        assert_eq!(spf.expected_display, format!("\"{}\"", spf.expected));
        // Nothing else moved.
        let rsa = rows.iter().find(|r| r.id == "dkim:202601r").unwrap();
        assert_eq!(rsa.chunks.len(), 2);
    }

    // ── Zone-file rendering (download payload) ──

    #[test]
    fn rendered_zone_file_preserves_txt_splits_and_skips_ptr() {
        let mut rows = fixture_rows();
        apply_send_mode_spf(&mut rows, "relay", Some("spf.provider.example"));
        let zone = render_zone_file("acme.dev", &rows);
        assert!(zone.contains("acme.dev.\t3600\tIN\tMX\t10 mail.acme.dev."));
        // Relay SPF is what downloads.
        assert!(zone.contains("\"v=spf1 mx include:spf.provider.example ~all\""));
        // The split DKIM keeps its quoted-chunks form.
        assert!(zone.contains("\" \""), "split TXT rendered as multiple quoted strings");
        // The PTR instruction row is NOT a zone record.
        assert!(!zone.contains("PTR"));
    }

    // ── Default-domain resolution (§11.1.2) ──

    #[test]
    fn default_domain_prefers_configured_verified_else_first() {
        let verified = vec!["acme.dev".to_string(), "beta.example".to_string()];
        // Empty setting → first verified.
        assert_eq!(resolve_default_domain("", &verified).as_deref(), Some("acme.dev"));
        // Configured + verified → configured.
        assert_eq!(
            resolve_default_domain("beta.example", &verified).as_deref(),
            Some("beta.example")
        );
        // Configured but NOT verified (stale setting) → first verified.
        assert_eq!(
            resolve_default_domain("gone.example", &verified).as_deref(),
            Some("acme.dev")
        );
        // Un-normalized configured value still matches (pre-mortem #14).
        assert_eq!(
            resolve_default_domain("  Beta.Example.  ", &verified).as_deref(),
            Some("beta.example")
        );
        // No verified domains → None.
        assert_eq!(resolve_default_domain("acme.dev", &[]), None);
    }

    // ── Ops against the shared test DB + a fake engine ──

    /// Canned engine: no network, scripted failures.
    pub(crate) struct FakeEngine {
        pub zone: String,
        pub fail_create: bool,
        pub deleted: std::sync::Mutex<Vec<String>>,
    }

    impl FakeEngine {
        pub(crate) fn ok() -> Self {
            Self {
                zone: ZONE_FIXTURE.to_string(),
                fail_create: false,
                deleted: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl DomainEngine for FakeEngine {
        fn create_domain(&self, domain: &str) -> Result<CreatedDomain, String> {
            if self.fail_create {
                return Err("Domain/set create rejected — forbidden".to_string());
            }
            Ok(CreatedDomain {
                id: format!("stw-{domain}"),
                dns_zone_file: Some(self.zone.clone()),
            })
        }
        fn delete_domain(&self, id: &str) -> Result<(), String> {
            self.deleted.lock().unwrap().push(id.to_string());
            Ok(())
        }
        fn dns_zonefile(&self, _id: &str) -> Result<String, String> {
            Ok(self.zone.clone())
        }
    }

    fn cleanup_domain(domain: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE domain_id IN \
             (SELECT id FROM mail_domains WHERE domain = ?1)",
            rusqlite::params![domain],
        );
        let _ = conn.execute("DELETE FROM mail_domains WHERE domain = ?1", rusqlite::params![domain]);
    }

    #[test]
    fn add_domain_normalizes_persists_and_returns_the_table() {
        let engine = FakeEngine::ok();
        // Punycode + case + FQDN dot at the API boundary (pre-mortem
        // #14) — the fixture zone is for acme.dev but the row's domain
        // must be the NORMALIZED input.
        cleanup_domain("acme.dev");
        let v = add_domain(&engine, "  ACME.dev.  ").expect("add ok");
        assert_eq!(v["ok"], true);
        assert_eq!(v["domain"], "acme.dev");
        assert_eq!(v["status"], "pending");
        assert_eq!(v["sendMode"], "receive-only");
        assert_eq!(v["note"], "records can take up to 48 h to propagate");
        assert!(v["dmarcNag"].as_bool().unwrap(), "nothing verified yet");
        let records = v["records"].as_array().unwrap();
        assert!(records.len() >= 6);
        assert_eq!(records[0]["id"], "mx");
        assert_eq!(records[0]["type"], "MX");
        assert!(v["zoneFile"].as_str().unwrap().contains("MX"));

        // Persisted: row exists, stalwart id stored, dup add → Exists.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let row = load_domain(&conn, "acme.dev").expect("row persisted");
            assert_eq!(row.stalwart_domain_id.as_deref(), Some("stw-acme.dev"));
            assert_eq!(row.status, "pending");
            assert_eq!(dns_status_of(&row).records.len(), records.len());
            assert!(dns_status_of(&row).zone_file.contains("Stalwart"));
        }
        match add_domain(&engine, "acme.dev") {
            Err(OpError::Exists(_)) => {}
            other => panic!("dup add must be Exists, got {other:?}"),
        }
        // Invalid domain → Usage, engine untouched.
        match add_domain(&engine, "not a domain") {
            Err(OpError::Usage(_)) => {}
            other => panic!("must be Usage, got {other:?}"),
        }
        cleanup_domain("acme.dev");
    }

    #[test]
    fn add_domain_engine_failure_surfaces_and_stores_nothing() {
        let engine = FakeEngine { fail_create: true, ..FakeEngine::ok() };
        cleanup_domain("failadd.example");
        match add_domain(&engine, "failadd.example") {
            Err(OpError::Engine(e)) => assert!(e.contains("forbidden"), "{e}"),
            other => panic!("must be Engine, got {other:?}"),
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(load_domain(&conn, "failadd.example").is_none(), "nothing persisted");
    }

    #[test]
    fn remove_domain_requires_confirm_then_retires_addresses() {
        let engine = FakeEngine::ok();
        cleanup_domain("rm.example");
        // Seed: a domain row + two active addresses + one already
        // retired (must stay untouched).
        let zone = ZONE_FIXTURE.replace("acme.dev", "rm.example");
        let engine_seed = FakeEngine { zone, ..FakeEngine::ok() };
        add_domain(&engine_seed, "rm.example").expect("seed add");
        let domain_id: String = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let id = load_domain(&conn, "rm.example").unwrap().id;
            for (addr, status) in [
                ("bot@rm.example", "active"),
                ("scout@rm.example", "active"),
                ("old@rm.example", "retired"),
            ] {
                conn.execute(
                    "INSERT INTO mail_addresses (id, address, domain_id, owner_project_id, \
                     status, created_at) VALUES (?1, ?2, ?3, 'proj-1', ?4, 100)",
                    rusqlite::params![format!("addr-{addr}"), addr, id, status],
                )
                .expect("seed address");
            }
            id
        };

        // No confirm → ConfirmRequired naming the ACTIVE count (2).
        match remove_domain(Some(&engine), "rm.example", false, false) {
            Err(OpError::ConfirmRequired(hint)) => {
                assert!(hint.contains("2 active address"), "{hint}");
            }
            other => panic!("must be ConfirmRequired, got {other:?}"),
        }
        // Nothing changed yet.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            assert!(load_domain(&conn, "rm.example").is_some());
        }

        // Confirmed: Stalwart delete + retire + row gone.
        let v = remove_domain(Some(&engine), "RM.example", true, true).expect("remove ok");
        assert_eq!(v["ok"], true);
        assert_eq!(v["retiredAddresses"], 2);
        assert_eq!(v["stalwartDeleted"], true);
        assert_eq!(v["purge"], true);
        assert_eq!(engine.deleted.lock().unwrap().as_slice(), ["stw-rm.example"]);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            assert!(load_domain(&conn, "rm.example").is_none(), "row deleted");
            let retired: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM mail_addresses WHERE domain_id = ?1 AND status = 'retired'",
                    rusqlite::params![domain_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(retired, 3, "both active rows retired; the old one untouched");
            let active: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM mail_addresses WHERE domain_id = ?1 AND status = 'active'",
                    rusqlite::params![domain_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(active, 0);
        }
        // Unknown domain → NotFound.
        match remove_domain(Some(&engine), "rm.example", true, false) {
            Err(OpError::NotFound(_)) => {}
            other => panic!("must be NotFound, got {other:?}"),
        }
        cleanup_domain("rm.example");
    }

    #[test]
    fn remove_with_stalwart_id_but_no_engine_fails_closed() {
        cleanup_domain("noengine.example");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_domains (id, domain, stalwart_domain_id, created_at) \
                 VALUES ('d-noeng', 'noengine.example', 'stw-x', 100)",
                [],
            )
            .expect("seed");
        }
        match remove_domain(None, "noengine.example", true, false) {
            Err(OpError::Engine(e)) => assert!(e.contains("Settings → Email"), "{e}"),
            other => panic!("must fail closed, got {other:?}"),
        }
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            assert!(load_domain(&conn, "noengine.example").is_some(), "row kept");
        }
        cleanup_domain("noengine.example");
    }

    #[test]
    fn effective_rows_apply_relay_include_from_the_column() {
        cleanup_domain("relayspf.example");
        let zone = ZONE_FIXTURE.replace("acme.dev", "relayspf.example");
        let engine = FakeEngine { zone, ..FakeEngine::ok() };
        add_domain(&engine, "relayspf.example").expect("seed add");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_relay_configs (id, kind, spf_include, created_at) \
                 VALUES ('rc-1', 'smtp', 'spf.provider.example', 100)",
                [],
            )
            .expect("seed relay config");
            conn.execute(
                "UPDATE mail_domains SET send_mode = 'relay', relay_config_id = 'rc-1' \
                 WHERE domain = 'relayspf.example'",
                [],
            )
            .expect("flip mode");
            let row = load_domain(&conn, "relayspf.example").unwrap();
            let rows = effective_rows(&conn, &row);
            let spf = rows.iter().find(|r| r.id == "spf").unwrap();
            assert_eq!(spf.expected, "v=spf1 mx include:spf.provider.example ~all");
            let _ = conn.execute("DELETE FROM mail_relay_configs WHERE id = 'rc-1'", []);
        }
        cleanup_domain("relayspf.example");
    }

    #[test]
    fn secret_ref_forms() {
        std::env::set_var("K2_TEST_MAIL_KEY", "sekrit");
        assert_eq!(resolve_secret_ref("env:K2_TEST_MAIL_KEY").unwrap(), "sekrit");
        assert!(resolve_secret_ref("env:K2_TEST_MAIL_KEY_MISSING").is_err());
        assert!(resolve_secret_ref("keychain:whatever").is_err(), "unknown scheme fails loudly");
        let dir = std::env::temp_dir().join(format!("k2-mail-secret-{}", std::process::id()));
        std::fs::write(&dir, "  filekey\n").expect("write");
        assert_eq!(
            resolve_secret_ref(dir.to_str().unwrap()).unwrap(),
            "filekey",
            "file form trims"
        );
        let _ = std::fs::remove_file(&dir);
    }
}
