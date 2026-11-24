//! Reads one brand file and writes the Rust that `lib.rs` includes.
//!
//! The build script uses this module (`#[path]` in `build.rs`), and so do
//! this crate's tests, on small brand files of their own. Every refusal
//! names the file and the field.
//!
//! What it reads: `id`, `schema`, `[names]`, the `[cli]` table, and the
//! tables a hosted box of the brand needs (`[zones]`, `[control]`,
//! `[feeds]`, `[machine]`, `[look]`, `[links]`, `[mail]`,
//! `[phone]`, `[sew]`). `[package]` and `[update]` are read by their own
//! readers (`scripts/brand-values.mjs`, `crates/k2-release-keys`); a table
//! or key it does not know stops the build.
//!
//! A field the brand leaves out takes the default brand's value
//! (`brand/k2`, which spells every field: [`load`]), or is made from the
//! brand's own names where the field says so (`names.short` is the
//! product's name). So brand/k2's file is today's values, and no value is
//! spelled in this code. The machine names (`[machine]`) must be the
//! default brand's for now: the code still uses those names everywhere.
//!
//! The tokens an install writes (the PATH block's two marker lines, the
//! fish file's name, the system-wide path) are made here, by
//! [`path_markers`], [`install_tokens`] and [`system_cli_path`], and nowhere
//! else: other crates take them as generated constants.
//!
//! The product's name and the command's name are two values a brand sets
//! apart; neither is made from the other. The PATH block's markers carry
//! the product's name (`# >>> <product> CLI >>>`, K2's ruling); the file
//! names and paths carry the command's. Builds before that ruling wrote the
//! markers with the command in upper case; when that form differs from
//! the product's, it is listed as an earlier form ([`earlier_markers`]),
//! so an install or an undo still finds and replaces such a block.

use std::path::Path;

/// The one schema this reader knows.
const SCHEMA: i64 = 1;

/// The keys `[cli]` may hold.
const CLI_KEYS: &[&str] = &["name", "aliases"];

/// The keys one alias may hold.
const ALIAS_KEYS: &[&str] = &["name", "mode"];

/// The top-level keys and tables a brand file may hold.
const TOP_KEYS: &[&str] = &[
    "id", "schema", "names", "cli", "package", "update", "zones", "control", "feeds", "machine",
    "look", "links", "mail", "phone", "sew",
];

/// The keys each table this reader checks may hold (`[cli]` has its own).
const TABLE_KEYS: &[(&str, &[&str])] = &[
    (
        "names",
        &["product", "short", "company", "lockup", "server", "login"],
    ),
    ("zones", &["label"]),
    ("control", CONTROL_KEYS),
    ("feeds", &["daemon", "downloads"]),
    ("update", &["keys"]),
    ("machine", &["home", "binary_prefix", "unit_prefix"]),
    ("look", &["icons", "accent"]),
    (
        "links",
        &[
            "site",
            "support_email",
            "support_url",
            "terms",
            "privacy",
            "dashboard",
            "feedback",
        ],
    ),
    ("mail", &["system_from", "system_from_name", "postal"]),
    ("phone", &["bundle_id", "team_id", "push_topic"]),
    ("sew", &["call_name"]),
];

/// Why a brand file may name no root-helper key (K2's ruling): every
/// brand's build ships our signed root helper.
const ROOT_HELPER_REFUSED: &str = "the root helper runs as root and is signed only by K5's release keys; partner builds ship ours (a new owner decision is needed if a partner runs its own servers)";

/// A table or key about the root helper's keys: `root_helper`, `root_helper_keys`, ...
fn names_root_helper(key: &str) -> bool {
    key.starts_with("root_helper")
}

/// `[control]`: all six or none (K2's ruling: a brand never mixes our
/// control endpoints with its own).
const CONTROL_KEYS: &[&str] = &["relay", "relay_list", "connect", "cert", "push", "web_api"];

/// The keys one signing key (`[update] keys`) may hold.
const KEY_KEYS: &[&str] = &["role", "key_id", "pubkey"];

/// A brand file, checked.
#[derive(Debug)]
pub struct Brand {
    pub id: String,
    pub product: String,
    pub cli_name: String,
    pub aliases: Vec<Alias>,
    /// Every other field, each the brand's own, the default brand's, or
    /// made from the brand's names.
    pub fields: Fields,
}

/// The fields a hosted box of the brand needs, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fields {
    /// `names.short`: the name where space is short. Default: the product's.
    pub short: String,
    /// `names.company`: who makes it.
    pub company: String,
    /// `names.lockup`: the product with its maker. Default: `<product> by <company>`.
    pub lockup: String,
    /// `names.server`: what a box is called. Default: `<product> Server`.
    pub server: String,
    /// `names.login`: the account people sign in with.
    pub login: String,
    /// `zones.label`: the zone boxes are named under, `<label>.<zone>`.
    pub label_zone: String,
    /// `control.connect`: the Connect control plane.
    pub connect: String,
    /// `control.cert`: the certificate broker's endpoint.
    pub cert: String,
    /// `control.push`: the push gateway.
    pub push: String,
    /// `control.relay`: the default relay's host.
    pub relay: String,
    /// `control.relay_list`: the list of relays.
    pub relay_list: String,
    /// `control.web_api`: the website's API base.
    pub web_api: String,
    /// `feeds.daemon`: the headless daemon's update manifest.
    pub daemon_feed: String,
    /// `feeds.downloads`: where a release's files are downloaded from.
    pub downloads: String,
    /// `machine.home`: the home folder's name.
    pub home: String,
    /// `machine.binary_prefix`: what the programs' file names start with.
    pub binary_prefix: String,
    /// `machine.unit_prefix`: what the systemd units' names start with.
    pub unit_prefix: String,
    /// `look.icons`: the icons folder, from the brand folder. None: the
    /// app's own icons.
    pub icons: Option<String>,
    /// `look.accent`: the accent colour. None: the style's own.
    pub accent: Option<String>,
    /// `links.site`: the public website.
    pub site: String,
    /// `links.support_email`: where people write for help.
    pub support_email: String,
    /// `links.support_url`: the support page. None: no link.
    pub support_url: Option<String>,
    /// `links.terms`: the terms of service. None: no link.
    pub terms: Option<String>,
    /// `links.privacy`: the privacy policy. None: no link.
    pub privacy: Option<String>,
    /// `links.dashboard`: where a person manages their account. None: no link.
    pub dashboard: Option<String>,
    /// `links.feedback`: where Send feedback goes. None: the button is off.
    pub feedback: Option<String>,
    /// `mail.system_from`: the address system mail comes from, with
    /// [`SERVER_SLOT`] for the box's own server name (`noreply@{server}`).
    pub system_from: String,
    /// `mail.system_from_name`: the name on it. Default: the product's.
    pub system_from_name: String,
    /// `mail.postal`: the brand's boxes may join Postal.
    pub postal: bool,
    /// `phone.bundle_id`: the phone app's bundle id.
    pub phone_bundle_id: String,
    /// `phone.team_id`: the phone app's Apple team id.
    pub phone_team_id: String,
    /// `phone.push_topic`: the phone app's push topic.
    pub push_topic: String,
    /// `sew.call_name`: what calls and chat name the service. Default: the
    /// product's.
    pub sew_call_name: String,
}

/// Another name the command answers to.
#[derive(Debug)]
pub struct Alias {
    pub name: String,
    pub mode: Mode,
}

/// What an alias does. The same three as `AliasMode` in `lib.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Silent,
    Notice,
    Tombstone,
}

impl Mode {
    fn from_word(word: &str) -> Option<Mode> {
        match word {
            "silent" => Some(Mode::Silent),
            "notice" => Some(Mode::Notice),
            "tombstone" => Some(Mode::Tombstone),
            _ => None,
        }
    }

    /// The brand is retiring this name: its installs left a PATH block and
    /// a fish file, which this brand's build takes over. A `silent` alias
    /// is a second name for the command and never had an install of its
    /// own.
    pub fn retires(self) -> bool {
        matches!(self, Mode::Notice | Mode::Tombstone)
    }

    fn variant(self) -> &'static str {
        match self {
            Mode::Silent => "Silent",
            Mode::Notice => "Notice",
            Mode::Tombstone => "Tombstone",
        }
    }
}

/// A command name or an alias: `^[a-z][a-z0-9]{1,11}$`.
pub fn is_command_name(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=12).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The folder the command is installed in system-wide.
pub const SYSTEM_BIN: &str = "/usr/local/bin";

/// What an install by one command name wrote before the PATH markers
/// carried the product's name: its PATH block's first and last lines, and
/// its fish startup file's name. A retired alias's old install, and this
/// command's own earlier form, are found by these.
#[derive(Debug, PartialEq, Eq)]
pub struct InstallTokens {
    pub block_begin: String,
    pub block_end: String,
    pub fish_conf: String,
}

/// The PATH block's first and last lines for the product called
/// `product`: `# >>> <product> CLI >>>` and `# <<< <product> CLI <<<`,
/// the product's name as the brand file spells it.
pub fn path_markers(product: &str) -> (String, String) {
    (
        format!("# >>> {product} CLI >>>"),
        format!("# <<< {product} CLI <<<"),
    )
}

/// The earlier marker pairs of a brand with product `product` and command
/// `name`: the command-form pair ([`install_tokens`]) when it is not the
/// product-form pair, else none. For K2 and K5 the two forms are the same
/// text, so there is none.
pub fn earlier_markers(product: &str, name: &str) -> Vec<(String, String)> {
    let old = install_tokens(name);
    let (begin, end) = path_markers(product);
    if (old.block_begin.as_str(), old.block_end.as_str()) == (begin.as_str(), end.as_str()) {
        Vec::new()
    } else {
        vec![(old.block_begin, old.block_end)]
    }
}

/// The install tokens a build by the command `name` wrote before the
/// markers carried the product's name: `# >>> <NAME> CLI >>>`,
/// `# <<< <NAME> CLI <<<` (the name in upper case) and `<name>-cli.fish`
/// (the fish file's name is still this). A name is ASCII
/// (`is_command_name`), so upper case is one byte each.
pub fn install_tokens(name: &str) -> InstallTokens {
    let upper = name.to_ascii_uppercase();
    InstallTokens {
        block_begin: format!("# >>> {upper} CLI >>>"),
        block_end: format!("# <<< {upper} CLI <<<"),
        fish_conf: format!("{name}-cli.fish"),
    }
}

/// `/usr/local/bin/<name>`.
pub fn system_cli_path(name: &str) -> String {
    format!("{SYSTEM_BIN}/{name}")
}

/// Reads and checks the brand file at `path`. `default` is the default
/// brand's file (`brand/k2/brand.toml`): a field `path` leaves out takes
/// its value, so it must spell every field. When `path` is the default
/// brand's file itself, it is read once and must be whole.
pub fn load(path: &Path, default: &Path) -> Result<Brand, String> {
    let same = match (path.canonicalize(), default.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if same {
        return read_one(path, None);
    }
    let base = read_one(default, None).map_err(|e| format!("{e} (the default brand file)"))?;
    read_one(path, Some(&base))
}

fn read_one(path: &Path, base: Option<&Brand>) -> Result<Brand, String> {
    let shown = path.display();
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!("brand file {shown}: can't read it ({e}); K2_BRAND_DIR names the folder that holds brand.toml")
    })?;
    parse(&text, base.map(|b| &b.fields)).map_err(|e| format!("brand file {shown}: {e}"))
}

fn parse(text: &str, base: Option<&Fields>) -> Result<Brand, String> {
    let doc: toml::Table = toml::from_str(text).map_err(|e| format!("not valid TOML: {e}"))?;
    match doc.get("schema") {
        Some(toml::Value::Integer(SCHEMA)) => {}
        Some(v) => return Err(format!("schema must be {SCHEMA}, not {v}")),
        None => return Err(format!("schema is missing; it must be {SCHEMA}")),
    }
    for (key, value) in &doc {
        if names_root_helper(key) {
            return Err(format!("{key}: {ROOT_HELPER_REFUSED}"));
        }
        if let toml::Value::Table(t) = value {
            if let Some(inner) = t.keys().find(|k| names_root_helper(k)) {
                return Err(format!("{key}.{inner}: {ROOT_HELPER_REFUSED}"));
            }
        }
    }
    if let Some(key) = doc.keys().find(|k| !TOP_KEYS.contains(&k.as_str())) {
        return Err(format!(
            "a top-level key or table this build does not know: {key} (it knows {})",
            TOP_KEYS.join(", ")
        ));
    }
    for (name, known) in TABLE_KEYS {
        if let Some(t) = opt_table(&doc, name)? {
            if let Some(key) = t.keys().find(|k| !known.contains(&k.as_str())) {
                return Err(format!(
                    "[{name}] holds a key it does not know: {key} (it knows {})",
                    known.join(", ")
                ));
            }
        }
    }
    let id = text_field(&doc, "id", "id")?;
    let product = text_field(table(&doc, "names")?, "product", "names.product")?;
    if product.chars().any(char::is_control) {
        return Err(format!(
            "names.product {product:?} must be one line with no control characters (it is in the PATH block's marker lines)"
        ));
    }
    let cli = table(&doc, "cli")?;
    if let Some(key) = cli.keys().find(|k| !CLI_KEYS.contains(&k.as_str())) {
        return Err(format!(
            "[cli] holds a key it does not know: {key} (it knows name and aliases)"
        ));
    }
    let cli_name = text_field(cli, "name", "cli.name")?;
    if !is_command_name(&cli_name) {
        return Err(format!(
            "cli.name {cli_name:?} must match ^[a-z][a-z0-9]{{1,11}}$"
        ));
    }
    let items = match cli.get("aliases") {
        None => &[][..],
        Some(toml::Value::Array(items)) => &items[..],
        Some(_) => return Err("cli.aliases must be a list of { name, mode } tables".into()),
    };
    let mut aliases: Vec<Alias> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let field = format!("cli.aliases[{i}]");
        let toml::Value::Table(t) = item else {
            return Err(format!(
                "{field} must be a table: {{ name = \"...\", mode = \"silent\" }}"
            ));
        };
        if let Some(key) = t.keys().find(|k| !ALIAS_KEYS.contains(&k.as_str())) {
            return Err(format!(
                "{field} holds a key it does not know: {key} (it knows name and mode)"
            ));
        }
        let name = text_field(t, "name", &format!("{field}.name"))?;
        if !is_command_name(&name) {
            return Err(format!(
                "{field}.name {name:?} must match ^[a-z][a-z0-9]{{1,11}}$"
            ));
        }
        if name == cli_name {
            return Err(format!("{field}.name {name:?} is cli.name itself"));
        }
        if aliases.iter().any(|a| a.name == name) {
            return Err(format!("{field}.name {name:?} is listed twice"));
        }
        let word = text_field(t, "mode", &format!("{field}.mode"))?;
        let mode = Mode::from_word(&word)
            .ok_or_else(|| format!("{field}.mode {word:?} must be silent, notice or tombstone"))?;
        aliases.push(Alias { name, mode });
    }
    let fields = parse_fields(&doc, &product, base)?;
    Ok(Brand {
        id,
        product,
        cli_name,
        aliases,
        fields,
    })
}

/// One field's value: the brand's own when it sets it, else the default
/// brand's (`base`), else (reading the default brand itself) it is missing.
fn inherit<T: Clone>(own: Option<T>, base: Option<&T>, field: &str) -> Result<T, String> {
    match (own, base) {
        (Some(v), _) => Ok(v),
        (None, Some(b)) => Ok(b.clone()),
        (None, None) => Err(format!(
            "{field} is missing; the default brand file spells every field"
        )),
    }
}

/// The value of `[table] key`, checked by `check` when it is set.
fn get_str(
    doc: &toml::Table,
    table: &str,
    key: &str,
    check: fn(&str, &str) -> Result<(), String>,
) -> Result<Option<String>, String> {
    let field = format!("{table}.{key}");
    let Some(t) = opt_table(doc, table)? else {
        return Ok(None);
    };
    if t.get(key).is_none() {
        return Ok(None);
    }
    let v = text_field(t, key, &field)?;
    check(&v, &field)?;
    Ok(Some(v))
}

/// Like [`get_str`] for a field that may be `""`: none (no link, off).
fn get_opt(
    doc: &toml::Table,
    table: &str,
    key: &str,
    check: fn(&str, &str) -> Result<(), String>,
) -> Result<Option<Option<String>>, String> {
    let field = format!("{table}.{key}");
    let Some(t) = opt_table(doc, table)? else {
        return Ok(None);
    };
    match t.get(key) {
        None => Ok(None),
        Some(toml::Value::String(s)) if s.is_empty() => Ok(Some(None)),
        Some(toml::Value::String(s)) => {
            check(s, &field)?;
            Ok(Some(Some(s.clone())))
        }
        Some(_) => Err(format!("{field} must be a string (\"\" for none)")),
    }
}

fn get_bool(doc: &toml::Table, table: &str, key: &str) -> Result<Option<bool>, String> {
    let Some(t) = opt_table(doc, table)? else {
        return Ok(None);
    };
    match t.get(key) {
        None => Ok(None),
        Some(toml::Value::Boolean(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("{table}.{key} must be true or false")),
    }
}

/// Every field but the names and the command, resolved against `base`.
fn parse_fields(doc: &toml::Table, product: &str, base: Option<&Fields>) -> Result<Fields, String> {
    let b = |f: fn(&Fields) -> &String| base.map(f);
    let o = |f: fn(&Fields) -> &Option<String>| base.map(f);
    let company = inherit(
        get_str(doc, "names", "company", check_name)?,
        b(|f| &f.company),
        "names.company",
    )?;
    let short = get_str(doc, "names", "short", check_name)?.unwrap_or_else(|| product.to_string());
    let lockup = get_str(doc, "names", "lockup", check_name)?
        .unwrap_or_else(|| format!("{product} by {company}"));
    let server =
        get_str(doc, "names", "server", check_name)?.unwrap_or_else(|| format!("{product} Server"));
    let login = inherit(
        get_str(doc, "names", "login", check_name)?,
        b(|f| &f.login),
        "names.login",
    )?;
    let label_zone = inherit(
        get_str(doc, "zones", "label", check_domain)?,
        b(|f| &f.label_zone),
        "zones.label",
    )?;
    if let Some(t) = opt_table(doc, "control")? {
        let missing: Vec<&str> = CONTROL_KEYS
            .iter()
            .copied()
            .filter(|k| !t.contains_key(*k))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "[control] leaves out {}: a brand that sets [control] sets all six ({}), or leaves the table out for ours",
                missing.join(", "),
                CONTROL_KEYS.join(", ")
            ));
        }
    }
    let url = |key: &str, f: fn(&Fields) -> &String| -> Result<String, String> {
        inherit(
            get_str(doc, "control", key, check_https)?,
            b(f),
            &format!("control.{key}"),
        )
    };
    let connect = url("connect", |f| &f.connect)?;
    let cert = url("cert", |f| &f.cert)?;
    let push = url("push", |f| &f.push)?;
    let relay = inherit(
        get_str(doc, "control", "relay", check_domain)?,
        b(|f| &f.relay),
        "control.relay",
    )?;
    let relay_list = url("relay_list", |f| &f.relay_list)?;
    let web_api = url("web_api", |f| &f.web_api)?;
    let daemon_feed = inherit(
        get_str(doc, "feeds", "daemon", check_https)?,
        b(|f| &f.daemon_feed),
        "feeds.daemon",
    )?;
    let downloads = inherit(
        get_str(doc, "feeds", "downloads", check_https)?,
        b(|f| &f.downloads),
        "feeds.downloads",
    )?;
    // [update] is read by its own reader (crates/k2-release-keys); its
    // keys are also the trust root of the Sew pins a build carries. Here
    // they are only checked for their shape.
    check_key_list(doc, "update", "keys")?;
    let machine = |key: &str, f: fn(&Fields) -> &String| -> Result<String, String> {
        let field = format!("machine.{key}");
        let check: fn(&str, &str) -> Result<(), String> = if key == "home" {
            check_home
        } else {
            check_prefix
        };
        let v = inherit(get_str(doc, "machine", key, check)?, b(f), &field)?;
        match b(f) {
            Some(want) if *want != v => Err(format!(
                "{field} {v:?} is not {want:?}: this build still names it {want:?} everywhere, so a brand may not name it otherwise yet"
            )),
            _ => Ok(v),
        }
    };
    let home = machine("home", |f| &f.home)?;
    let binary_prefix = machine("binary_prefix", |f| &f.binary_prefix)?;
    let unit_prefix = machine("unit_prefix", |f| &f.unit_prefix)?;
    let icons = get_str(doc, "look", "icons", check_rel_path)?;
    let accent = get_str(doc, "look", "accent", check_colour)?;
    let site = inherit(
        get_str(doc, "links", "site", check_https)?,
        b(|f| &f.site),
        "links.site",
    )?;
    let support_email = inherit(
        get_str(doc, "links", "support_email", check_email)?,
        b(|f| &f.support_email),
        "links.support_email",
    )?;
    let link = |key: &str, f: fn(&Fields) -> &Option<String>| -> Result<Option<String>, String> {
        inherit(
            get_opt(doc, "links", key, check_https)?,
            o(f),
            &format!("links.{key}"),
        )
    };
    let support_url = link("support_url", |f| &f.support_url)?;
    let terms = link("terms", |f| &f.terms)?;
    let privacy = link("privacy", |f| &f.privacy)?;
    let dashboard = link("dashboard", |f| &f.dashboard)?;
    let feedback = link("feedback", |f| &f.feedback)?;
    let system_from = inherit(
        get_str(doc, "mail", "system_from", check_system_from)?,
        b(|f| &f.system_from),
        "mail.system_from",
    )?;
    let system_from_name = get_str(doc, "mail", "system_from_name", check_name)?
        .unwrap_or_else(|| product.to_string());
    let postal = inherit(
        get_bool(doc, "mail", "postal")?,
        base.map(|f| &f.postal),
        "mail.postal",
    )?;
    let phone_bundle_id = inherit(
        get_str(doc, "phone", "bundle_id", check_reverse_dns)?,
        b(|f| &f.phone_bundle_id),
        "phone.bundle_id",
    )?;
    let phone_team_id = inherit(
        get_str(doc, "phone", "team_id", check_team_id)?,
        b(|f| &f.phone_team_id),
        "phone.team_id",
    )?;
    let push_topic = inherit(
        get_str(doc, "phone", "push_topic", check_reverse_dns)?,
        b(|f| &f.push_topic),
        "phone.push_topic",
    )?;
    let sew_call_name =
        get_str(doc, "sew", "call_name", check_name)?.unwrap_or_else(|| product.to_string());
    Ok(Fields {
        short,
        company,
        lockup,
        server,
        login,
        label_zone,
        connect,
        cert,
        push,
        relay,
        relay_list,
        web_api,
        daemon_feed,
        downloads,
        home,
        binary_prefix,
        unit_prefix,
        icons,
        accent,
        site,
        support_email,
        support_url,
        terms,
        privacy,
        dashboard,
        feedback,
        system_from,
        system_from_name,
        postal,
        phone_bundle_id,
        phone_team_id,
        push_topic,
        sew_call_name,
    })
}

/// `[<table>] <key>`, when set: a list of `{ role, key_id, pubkey }`,
/// each a minisign public key line whose id is its `key_id`, a primary
/// first, no key twice.
fn check_key_list(doc: &toml::Table, table: &str, key: &str) -> Result<(), String> {
    let list = format!("{table}.{key}");
    let Some(t) = opt_table(doc, table)? else {
        return Ok(());
    };
    let items = match t.get(key) {
        None => return Ok(()),
        Some(toml::Value::Array(a)) if !a.is_empty() => a,
        Some(_) => {
            return Err(format!(
                "{list} must be a list of {{ role, key_id, pubkey }} tables, at least one"
            ))
        }
    };
    let mut seen: Vec<String> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let f = format!("{list}[{i}]");
        let toml::Value::Table(k) = item else {
            return Err(format!("{f} must be a table: {{ role, key_id, pubkey }}"));
        };
        if let Some(bad) = k.keys().find(|x| !KEY_KEYS.contains(&x.as_str())) {
            return Err(format!(
                "{f} holds a key it does not know: {bad} (it knows role, key_id and pubkey)"
            ));
        }
        let role = text_field(k, "role", &format!("{f}.role"))?;
        let key_id = text_field(k, "key_id", &format!("{f}.key_id"))?;
        let pubkey = text_field(k, "pubkey", &format!("{f}.pubkey"))?;
        if role != "primary" && role != "backup" {
            return Err(format!("{f}.role {role:?} must be primary or backup"));
        }
        if i == 0 && role != "primary" {
            return Err(format!("{list} must start with a primary key"));
        }
        let Some(id) = minisign_key_id(&pubkey) else {
            return Err(format!("{f}.pubkey is not a minisign public key line"));
        };
        if id != key_id {
            return Err(format!(
                "{f}.key_id {key_id:?} is not the pubkey's key id ({id})"
            ));
        }
        if seen.contains(&key_id) {
            return Err(format!("{list} lists {key_id} twice"));
        }
        seen.push(key_id);
    }
    Ok(())
}

/// minisign's key id of a public key line ("Ed", the 8 id bytes as a
/// little-endian u64 in 16 upper hex, the 32 key bytes). None when the
/// line is not one.
pub fn minisign_key_id(line: &str) -> Option<String> {
    let raw = base64_decode(line)?;
    if raw.len() != 42 || &raw[0..2] != b"Ed" {
        return None;
    }
    let mut id = [0u8; 8];
    id.copy_from_slice(&raw[2..10]);
    Some(format!("{:016X}", u64::from_le_bytes(id)))
}

/// Standard base64 with padding (no dependency at build time).
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.is_empty() || !b.len().is_multiple_of(4) {
        return None;
    }
    let pad = b.iter().rev().take_while(|c| **c == b'=').count();
    if pad > 2 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in &b[..b.len() - pad] {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ── The shapes ──────────────────────────────────────────────────────
// Each check takes the value and the field's name, and says what is
// wrong with it. scripts/brand/lint.py checks the same shapes.

/// A name people read: one line, 1 to 40 characters, no space at an end.
fn check_name(v: &str, field: &str) -> Result<(), String> {
    if v.chars().count() > 40 || v.trim() != v || v.chars().any(char::is_control) {
        return Err(format!(
            "{field} {v:?} must be one line of 1 to 40 characters with no space at either end"
        ));
    }
    Ok(())
}

/// A domain name in lower case: two or more labels of letters, digits and
/// inner dashes, the last one letters only.
pub fn is_domain(v: &str) -> bool {
    let labels: Vec<&str> = v.split('.').collect();
    v.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            (1..=63).contains(&l.len())
                && l.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
        && labels
            .last()
            .is_some_and(|l| l.bytes().all(|c| c.is_ascii_lowercase()))
}

fn check_domain(v: &str, field: &str) -> Result<(), String> {
    if !is_domain(v) {
        return Err(format!(
            "{field} {v:?} must be a domain name in lower case (example.invalid)"
        ));
    }
    Ok(())
}

/// `https://<domain>`, then nothing or a path: printable ASCII, no space,
/// quote, backslash, angle bracket or backtick.
pub fn is_https_url(v: &str) -> bool {
    let Some(rest) = v.strip_prefix("https://") else {
        return false;
    };
    let (host, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    is_domain(host)
        && path.bytes().all(|c| {
            c.is_ascii_graphic() && !matches!(c, b'"' | b'\'' | b'\\' | b'<' | b'>' | b'`')
        })
}

fn check_https(v: &str, field: &str) -> Result<(), String> {
    if !is_https_url(v) {
        return Err(format!(
            "{field} {v:?} must be an https URL: https://<domain>[/<path>]"
        ));
    }
    Ok(())
}

fn check_email(v: &str, field: &str) -> Result<(), String> {
    let ok = v.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && local
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'+' | b'-'))
            && is_domain(domain)
    });
    if !ok {
        return Err(format!(
            "{field} {v:?} must be an email address: <name>@<domain>"
        ));
    }
    Ok(())
}

/// Where `mail.system_from` puts the box's own server name
/// (`<label>.<zone>`): each box sends from its own name, with SPF, DKIM
/// and DMARC on it.
pub const SERVER_SLOT: &str = "{server}";

/// An email address whose domain is a domain or [`SERVER_SLOT`].
fn check_system_from(v: &str, field: &str) -> Result<(), String> {
    match v.split_once('@') {
        Some((local, SERVER_SLOT)) => check_email(&format!("{local}@server.invalid"), field)
            .map_err(|_| format!("{field} {v:?} must be an email address: <name>@{SERVER_SLOT} or <name>@<domain>")),
        _ => check_email(v, field),
    }
}

fn check_colour(v: &str, field: &str) -> Result<(), String> {
    let ok = v.len() == 7 && v.starts_with('#') && v[1..].bytes().all(|c| c.is_ascii_hexdigit());
    if !ok {
        return Err(format!("{field} {v:?} must be a colour in hex: #RRGGBB"));
    }
    Ok(())
}

/// A folder from the brand folder: relative, `/` between parts, no empty part.
fn check_rel_path(v: &str, field: &str) -> Result<(), String> {
    let ok = !v.starts_with('/')
        && !v.contains('\\')
        && !v.chars().any(char::is_control)
        && v.split('/').all(|p| !p.is_empty());
    if !ok {
        return Err(format!(
            "{field} {v:?} must be a folder from the brand folder: relative, parts split by /"
        ));
    }
    Ok(())
}

/// A reverse-DNS id: `com.example.app`.
fn check_reverse_dns(v: &str, field: &str) -> Result<(), String> {
    let parts: Vec<&str> = v.split('.').collect();
    let ok = parts.len() >= 2
        && parts.iter().all(|p| {
            p.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
                && p.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        });
    if !ok {
        return Err(format!(
            "{field} {v:?} must be a reverse-DNS id in lower case (com.example.app)"
        ));
    }
    Ok(())
}

/// An Apple team id: 10 capital letters and digits.
fn check_team_id(v: &str, field: &str) -> Result<(), String> {
    if v.len() != 10
        || !v
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return Err(format!(
            "{field} {v:?} must be an Apple team id: 10 capital letters and digits"
        ));
    }
    Ok(())
}

/// The home folder's name: a dot, then a command name's shape.
fn check_home(v: &str, field: &str) -> Result<(), String> {
    if !v.strip_prefix('.').is_some_and(is_command_name) {
        return Err(format!(
            "{field} {v:?} must be a dot and a name: ^\\.[a-z][a-z0-9]{{1,11}}$"
        ));
    }
    Ok(())
}

fn check_prefix(v: &str, field: &str) -> Result<(), String> {
    if !is_command_name(v) {
        return Err(format!("{field} {v:?} must match ^[a-z][a-z0-9]{{1,11}}$"));
    }
    Ok(())
}

fn opt_table<'a>(doc: &'a toml::Table, key: &str) -> Result<Option<&'a toml::Table>, String> {
    match doc.get(key) {
        Some(toml::Value::Table(t)) => Ok(Some(t)),
        Some(_) => Err(format!("[{key}] must be a table")),
        None => Ok(None),
    }
}

fn table<'a>(doc: &'a toml::Table, key: &str) -> Result<&'a toml::Table, String> {
    match doc.get(key) {
        Some(toml::Value::Table(t)) => Ok(t),
        Some(_) => Err(format!("[{key}] must be a table")),
        None => Err(format!("[{key}] is missing")),
    }
}

fn text_field(t: &toml::Table, key: &str, field: &str) -> Result<String, String> {
    match t.get(key) {
        Some(toml::Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(toml::Value::String(_)) => Err(format!("{field} is empty")),
        Some(_) => Err(format!("{field} must be a string")),
        None => Err(format!("{field} is missing")),
    }
}

/// The Rust `lib.rs` includes: the names, then the other fields. Strings
/// are written as Rust literals (`{:?}` escapes quotes, backslashes and
/// control characters).
pub fn render(b: &Brand) -> String {
    format!("{}\n{}", render_names(b), render_fields(&b.fields))
}

/// The other fields, one module per table.
fn render_fields(f: &Fields) -> String {
    let opt = |v: &Option<String>| match v {
        Some(s) => format!("Some({s:?})"),
        None => "None".to_string(),
    };
    format!(
        "/// `[names]`: the brand's other names.
pub mod names {{
    /// The name where space is short.
    pub const SHORT: &str = {short:?};
    /// Who makes it.
    pub const COMPANY: &str = {company:?};
    /// The product with its maker.
    pub const LOCKUP: &str = {lockup:?};
    /// What a box is called.
    pub const SERVER: &str = {server:?};
    /// The account people sign in with.
    pub const LOGIN: &str = {login:?};
}}

/// `[zones]`: where boxes are named.
pub mod zones {{
    /// The zone a box's name is under, `<label>.<zone>`, until the
    /// enrolment answer names it.
    pub const LABEL: &str = {label:?};
}}

/// `[control]`: our control endpoints, unless the brand sets its own.
pub mod control {{
    /// The Connect control plane.
    pub const CONNECT: &str = {connect:?};
    /// The certificate broker's endpoint.
    pub const CERT: &str = {cert:?};
    /// The push gateway.
    pub const PUSH: &str = {push:?};
    /// The default relay's host.
    pub const RELAY_HOST: &str = {relay:?};
    /// The list of relays.
    pub const RELAY_LIST: &str = {relay_list:?};
    /// The website's API base.
    pub const WEB_API: &str = {web_api:?};
}}

/// `[feeds]`: where updates come from (the app's feed is
/// `[package] update_feed`, read by scripts/brand-values.mjs).
pub mod feeds {{
    /// The headless daemon's update manifest.
    pub const DAEMON: &str = {daemon:?};
    /// Where a release's files are downloaded from.
    pub const DOWNLOADS: &str = {downloads:?};
}}

/// `[machine]`: the names ops scripts find on a box.
pub mod machine {{
    /// The home folder's name.
    pub const HOME_DIR_NAME: &str = {home:?};
    /// What the programs' file names start with (`<prefix>-daemon`).
    pub const BINARY_PREFIX: &str = {binary_prefix:?};
    /// What the systemd units' names start with (`<prefix>-daemon.service`).
    pub const UNIT_PREFIX: &str = {unit_prefix:?};
}}

/// `[look]`: icons and colours.
pub mod look {{
    /// The icons folder, from the brand folder; None: the app's own.
    pub const ICONS: Option<&str> = {icons};
    /// The accent colour, #RRGGBB; None: the style's own.
    pub const ACCENT: Option<&str> = {accent};
}}

/// `[links]`: where people are sent. None: no link (feedback: off).
pub mod links {{
    /// The public website.
    pub const SITE: &str = {site:?};
    /// Where people write for help.
    pub const SUPPORT_EMAIL: &str = {support_email:?};
    /// The support page.
    pub const SUPPORT_URL: Option<&str> = {support_url};
    /// The terms of service.
    pub const TERMS: Option<&str> = {terms};
    /// The privacy policy.
    pub const PRIVACY: Option<&str> = {privacy};
    /// Where a person manages their account.
    pub const DASHBOARD: Option<&str> = {dashboard};
    /// Where Send feedback goes.
    pub const FEEDBACK: Option<&str> = {feedback};
}}

/// `[mail]`: system mail and Postal.
pub mod mail {{
    /// The address system mail comes from; `{{server}}` is the box's own
    /// server name (SPF, DKIM and DMARC go on that name).
    pub const SYSTEM_FROM: &str = {system_from:?};
    /// The name on it.
    pub const SYSTEM_FROM_NAME: &str = {system_from_name:?};
    /// The brand's boxes may join Postal.
    pub const POSTAL: bool = {postal};
}}

/// `[phone]`: the phone app the brand's boxes pair with.
pub mod phone {{
    /// Its bundle id.
    pub const BUNDLE_ID: &str = {bundle_id:?};
    /// Its Apple team id.
    pub const TEAM_ID: &str = {team_id:?};
    /// Its push topic.
    pub const PUSH_TOPIC: &str = {push_topic:?};
}}

/// `[sew]`: the brand in calls and chat.
pub mod sew {{
    /// What calls and chat name the service.
    pub const CALL_NAME: &str = {call_name:?};
}}
",
        short = f.short,
        company = f.company,
        lockup = f.lockup,
        server = f.server,
        login = f.login,
        label = f.label_zone,
        connect = f.connect,
        cert = f.cert,
        push = f.push,
        relay = f.relay,
        relay_list = f.relay_list,
        web_api = f.web_api,
        daemon = f.daemon_feed,
        downloads = f.downloads,
        home = f.home,
        binary_prefix = f.binary_prefix,
        unit_prefix = f.unit_prefix,
        icons = opt(&f.icons),
        accent = opt(&f.accent),
        site = f.site,
        support_email = f.support_email,
        support_url = opt(&f.support_url),
        terms = opt(&f.terms),
        privacy = opt(&f.privacy),
        dashboard = opt(&f.dashboard),
        feedback = opt(&f.feedback),
        system_from = f.system_from,
        system_from_name = f.system_from_name,
        postal = f.postal,
        bundle_id = f.phone_bundle_id,
        team_id = f.phone_team_id,
        push_topic = f.push_topic,
        call_name = f.sew_call_name,
    )
}

/// The names part of [`render`].
fn render_names(b: &Brand) -> String {
    let names: Vec<String> = b.aliases.iter().map(|a| format!("{:?}", a.name)).collect();
    let own = install_tokens(&b.cli_name);
    let (begin, end) = path_markers(&b.product);
    let earlier: Vec<String> = earlier_markers(&b.product, &b.cli_name)
        .iter()
        .map(|(begin, end)| format!("({begin:?}, {end:?})"))
        .collect();
    let retired: Vec<String> = b
        .aliases
        .iter()
        .filter(|a| a.mode.retires())
        .map(|a| {
            let t = install_tokens(&a.name);
            format!(
                "RetiredName {{ name: {:?}, block_begin: {:?}, block_end: {:?}, fish_conf: {:?} }}",
                a.name, t.block_begin, t.block_end, t.fish_conf
            )
        })
        .collect();
    let modes: Vec<String> = b
        .aliases
        .iter()
        .map(|a| format!("({:?}, AliasMode::{})", a.name, a.mode.variant()))
        .collect();
    format!(
        "// Written by k2-brand's build script from the brand file. Do not edit.

/// The product's name as a literal, for `concat!` in a `const`.
#[macro_export]
macro_rules! product_name {{
    () => {{
        {product:?}
    }};
}}

/// The command's name as a literal, for `concat!` in a `const`.
#[macro_export]
macro_rules! cli_name {{
    () => {{
        {cli:?}
    }};
}}

/// A helper program's file name, `<command>-<part>`, as a literal, for
/// `concat!` in a `const`: `helper_name!(\"agent\")`.
#[macro_export]
macro_rules! helper_name {{
    ($part:literal) => {{
        concat!({cli:?}, \"-\", $part)
    }};
}}

/// The product's name, as people read it.
pub const PRODUCT_NAME: &str = product_name!();

/// The command people and agents type.
pub const CLI_NAME: &str = cli_name!();

/// The brand's id: its folder under `brand/`.
pub const BRAND_ID: &str = {id:?};

/// Other names the command answers to.
pub const CLI_ALIASES: &[&str] = &[{names}];

/// The same names, in the same order, each with what it does.
pub const CLI_ALIAS_MODES: &[(&str, AliasMode)] = &[{modes}];

/// First line of the PATH block an install writes into a shell startup
/// file: the product's name.
pub const PATH_BLOCK_BEGIN: &str = {begin:?};

/// Last line of the PATH block.
pub const PATH_BLOCK_END: &str = {end:?};

/// Earlier forms of this build's own PATH block markers (begin, end): the
/// command in upper case, when that is not the product's form. An install
/// replaces such a block in place, and the undo removes it.
pub const PATH_BLOCK_EARLIER: &[(&str, &str)] = &[{earlier}];

/// The fish startup file an install owns (the whole file is its block).
pub const FISH_CONF: &str = {fish:?};

/// The command's system-wide path: the Linux copy and the root helper's.
pub const SYSTEM_CLI_PATH: &str = {system:?};

/// The aliases this brand is retiring (mode notice or tombstone), with the
/// PATH block and fish file their installs left.
pub const RETIRED: &[RetiredName] = &[{retired}];
",
        product = b.product,
        cli = b.cli_name,
        id = b.id,
        names = names.join(", "),
        modes = modes.join(", "),
        begin = begin,
        end = end,
        earlier = earlier.join(", "),
        fish = own.fish_conf,
        system = system_cli_path(&b.cli_name),
        retired = retired.join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A file of the repo, from its root.
    fn repo(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel)
    }

    /// The default brand's file, which every other brand's left-out
    /// fields come from.
    fn default_file() -> PathBuf {
        repo("brand/k2/brand.toml")
    }

    /// A brand file in a folder of its own under the temp dir, removed
    /// when dropped.
    struct TempBrand {
        dir: PathBuf,
    }

    impl TempBrand {
        fn new(tag: &str, text: Option<&str>) -> TempBrand {
            let dir =
                std::env::temp_dir().join(format!("k2-brand-test-{}-{tag}", std::process::id()));
            if dir.exists() {
                std::fs::remove_dir_all(&dir).expect("clear an old test folder");
            }
            std::fs::create_dir_all(&dir).expect("make the test folder");
            if let Some(text) = text {
                std::fs::write(dir.join("brand.toml"), text).expect("write the test brand file");
            }
            TempBrand { dir }
        }

        fn file(&self) -> PathBuf {
            self.dir.join("brand.toml")
        }
    }

    impl Drop for TempBrand {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const GOOD: &str = r#"
id = "ab"
schema = 1
[names]
product = "AB"
[cli]
name = "ab"
aliases = [{ name = "abc", mode = "notice" }, { name = "ab1", mode = "silent" }, { name = "ab2", mode = "tombstone" }]
"#;

    /// `GOOD` with `from` replaced by `to` (once; it must be there).
    fn good_with(from: &str, to: &str) -> String {
        assert!(GOOD.contains(from), "the good file holds {from:?}");
        GOOD.replacen(from, to, 1)
    }

    /// Loading `text` fails, and the message names the file and holds `want`.
    fn refused(tag: &str, text: &str, want: &str) {
        let t = TempBrand::new(tag, Some(text));
        let err = load(&t.file(), &default_file()).expect_err("the brand file is refused");
        assert!(
            err.starts_with(&format!("brand file {}: ", t.file().display())),
            "names the file: {err}"
        );
        assert!(err.contains(want), "{want:?} in: {err}");
    }

    #[test]
    fn a_good_file_gives_the_expected_text() {
        let t = TempBrand::new("good", Some(GOOD));
        let text = render_names(&load(&t.file(), &default_file()).expect("the good file loads"));
        let want = r##"// Written by k2-brand's build script from the brand file. Do not edit.

/// The product's name as a literal, for `concat!` in a `const`.
#[macro_export]
macro_rules! product_name {
    () => {
        "AB"
    };
}

/// The command's name as a literal, for `concat!` in a `const`.
#[macro_export]
macro_rules! cli_name {
    () => {
        "ab"
    };
}

/// A helper program's file name, `<command>-<part>`, as a literal, for
/// `concat!` in a `const`: `helper_name!("agent")`.
#[macro_export]
macro_rules! helper_name {
    ($part:literal) => {
        concat!("ab", "-", $part)
    };
}

/// The product's name, as people read it.
pub const PRODUCT_NAME: &str = product_name!();

/// The command people and agents type.
pub const CLI_NAME: &str = cli_name!();

/// The brand's id: its folder under `brand/`.
pub const BRAND_ID: &str = "ab";

/// Other names the command answers to.
pub const CLI_ALIASES: &[&str] = &["abc", "ab1", "ab2"];

/// The same names, in the same order, each with what it does.
pub const CLI_ALIAS_MODES: &[(&str, AliasMode)] = &[("abc", AliasMode::Notice), ("ab1", AliasMode::Silent), ("ab2", AliasMode::Tombstone)];

/// First line of the PATH block an install writes into a shell startup
/// file: the product's name.
pub const PATH_BLOCK_BEGIN: &str = "# >>> AB CLI >>>";

/// Last line of the PATH block.
pub const PATH_BLOCK_END: &str = "# <<< AB CLI <<<";

/// Earlier forms of this build's own PATH block markers (begin, end): the
/// command in upper case, when that is not the product's form. An install
/// replaces such a block in place, and the undo removes it.
pub const PATH_BLOCK_EARLIER: &[(&str, &str)] = &[];

/// The fish startup file an install owns (the whole file is its block).
pub const FISH_CONF: &str = "ab-cli.fish";

/// The command's system-wide path: the Linux copy and the root helper's.
pub const SYSTEM_CLI_PATH: &str = "/usr/local/bin/ab";

/// The aliases this brand is retiring (mode notice or tombstone), with the
/// PATH block and fish file their installs left.
pub const RETIRED: &[RetiredName] = &[RetiredName { name: "abc", block_begin: "# >>> ABC CLI >>>", block_end: "# <<< ABC CLI <<<", fish_conf: "abc-cli.fish" }, RetiredName { name: "ab2", block_begin: "# >>> AB2 CLI >>>", block_end: "# <<< AB2 CLI <<<", fish_conf: "ab2-cli.fish" }];
"##;
        assert_eq!(text, want);
    }

    #[test]
    fn no_aliases_gives_empty_lists() {
        for (tag, aliases) in [("empty", "aliases = []\n"), ("absent", "")] {
            let text = format!("id = \"ab\"\nschema = 1\n[names]\nproduct = \"AB\"\n[cli]\nname = \"ab\"\n{aliases}");
            let t = TempBrand::new(tag, Some(&text));
            let out = render(&load(&t.file(), &default_file()).expect("loads"));
            assert!(
                out.contains("pub const CLI_ALIASES: &[&str] = &[];\n"),
                "{tag}: {out}"
            );
            assert!(
                out.contains("pub const CLI_ALIAS_MODES: &[(&str, AliasMode)] = &[];\n"),
                "{tag}: {out}"
            );
            assert!(
                out.contains("pub const RETIRED: &[RetiredName] = &[];\n"),
                "{tag}: {out}"
            );
        }
    }

    /// The default brand's install tokens are today's bytes: the lines in
    /// people's shell startup files, the fish file's name, the system path
    /// and the helper programs' names. A change to the generator that moves
    /// one fails here.
    #[test]
    fn the_default_brand_gives_todays_install_tokens() {
        let file = default_file();
        let out = render(&load(&file, &file).expect("the default brand file loads"));
        for line in [
            "pub const PATH_BLOCK_BEGIN: &str = \"# >>> K2 CLI >>>\";\n",
            "pub const PATH_BLOCK_END: &str = \"# <<< K2 CLI <<<\";\n",
            "pub const FISH_CONF: &str = \"k2-cli.fish\";\n",
            "pub const SYSTEM_CLI_PATH: &str = \"/usr/local/bin/k2\";\n",
            // Its one alias is silent: nothing to take over.
            "pub const RETIRED: &[RetiredName] = &[];\n",
            // The product's form and the command's form are one text.
            "pub const PATH_BLOCK_EARLIER: &[(&str, &str)] = &[];\n",
            // Helper programs keep today's names (k2-ptyd, k2-daemon, ...).
            "        concat!(\"k2\", \"-\", $part)\n",
        ] {
            assert!(out.contains(line), "{line:?} in:\n{out}");
        }
        assert_eq!(
            install_tokens("k2"),
            InstallTokens {
                block_begin: "# >>> K2 CLI >>>".into(),
                block_end: "# <<< K2 CLI <<<".into(),
                fish_conf: "k2-cli.fish".into(),
            }
        );
        assert_eq!(system_cli_path("k2"), "/usr/local/bin/k2");
    }

    /// A `notice` or `tombstone` alias is listed under RETIRED with its
    /// own tokens; a `silent` one is not.
    #[test]
    fn a_retiring_alias_is_listed_and_a_silent_one_is_not() {
        let text = "id = \"xy\"\nschema = 1\n[names]\nproduct = \"XY\"\n[cli]\nname = \"xy\"\n\
            aliases = [{ name = \"xyold\", mode = \"notice\" }, { name = \"xy2\", mode = \"silent\" }]\n";
        let t = TempBrand::new("retired", Some(text));
        let out = render(&load(&t.file(), &default_file()).expect("loads"));
        let want = "pub const RETIRED: &[RetiredName] = &[RetiredName { name: \"xyold\", \
            block_begin: \"# >>> XYOLD CLI >>>\", block_end: \"# <<< XYOLD CLI <<<\", \
            fish_conf: \"xyold-cli.fish\" }];\n";
        assert!(out.contains(want), "{want:?} in:\n{out}");
        assert!(
            out.contains("pub const PATH_BLOCK_BEGIN: &str = \"# >>> XY CLI >>>\";\n"),
            "{out}"
        );
        assert!(
            out.contains("pub const FISH_CONF: &str = \"xy-cli.fish\";\n"),
            "{out}"
        );
        assert!(
            out.contains("pub const SYSTEM_CLI_PATH: &str = \"/usr/local/bin/xy\";\n"),
            "{out}"
        );
        let retired = out
            .lines()
            .find(|l| l.starts_with("pub const RETIRED"))
            .expect("a RETIRED line");
        assert!(
            !retired.contains("xy2"),
            "a silent alias is not retired: {retired}"
        );

        let tomb = TempBrand::new(
            "tombstone",
            Some(&text.replace("\"notice\"", "\"tombstone\"")),
        );
        let out = render(&load(&tomb.file(), &default_file()).expect("loads"));
        assert!(
            out.contains(want),
            "a tombstone alias is retired too:\n{out}"
        );
    }

    /// With a product and a command that are unrelated words, the PATH
    /// markers carry the product, the files and paths the command, and the
    /// command's upper-case form is listed as an earlier marker. A retired
    /// alias keeps the form its old install wrote.
    #[test]
    fn the_markers_carry_the_product_and_the_old_form_is_earlier() {
        let text =
            "id = \"gd\"\nschema = 1\n[names]\nproduct = \"Globex Desk\"\n[cli]\nname = \"gd\"\n\
            aliases = [{ name = \"gdold\", mode = \"notice\" }]\n";
        let t = TempBrand::new("apart", Some(text));
        let out = render(&load(&t.file(), &default_file()).expect("loads"));
        for line in [
            "pub const PATH_BLOCK_BEGIN: &str = \"# >>> Globex Desk CLI >>>\";\n",
            "pub const PATH_BLOCK_END: &str = \"# <<< Globex Desk CLI <<<\";\n",
            "pub const PATH_BLOCK_EARLIER: &[(&str, &str)] = &[(\"# >>> GD CLI >>>\", \"# <<< GD CLI <<<\")];\n",
            "pub const FISH_CONF: &str = \"gd-cli.fish\";\n",
            "pub const SYSTEM_CLI_PATH: &str = \"/usr/local/bin/gd\";\n",
            "block_begin: \"# >>> GDOLD CLI >>>\", block_end: \"# <<< GDOLD CLI <<<\"",
        ] {
            assert!(out.contains(line), "{line:?} in:\n{out}");
        }
    }

    /// The forms, one by one: the product's markers as spelled; an earlier
    /// form only when the command's upper case is other text.
    #[test]
    fn markers_and_earlier_forms() {
        assert_eq!(
            path_markers("Globex Desk"),
            (
                "# >>> Globex Desk CLI >>>".to_string(),
                "# <<< Globex Desk CLI <<<".to_string()
            )
        );
        assert_eq!(
            earlier_markers("Globex Desk", "gd"),
            vec![(
                "# >>> GD CLI >>>".to_string(),
                "# <<< GD CLI <<<".to_string()
            )]
        );
        for (product, name) in [("K2", "k2"), ("K5", "k5"), ("XY", "xy")] {
            assert_eq!(earlier_markers(product, name), Vec::new(), "{product}");
            assert_eq!(
                path_markers(product).0,
                install_tokens(name).block_begin,
                "{product}"
            );
        }
        // Case matters: a product spelled other than the upper-case
        // command is another marker.
        assert_eq!(earlier_markers("Xy", "xy").len(), 1);
    }

    /// The k5 brand's markers are the text a K5 install writes today.
    #[test]
    fn the_k5_brand_keeps_its_markers() {
        let file = repo("brand/k5/brand.toml");
        let out = render(&load(&file, &default_file()).expect("the k5 brand file loads"));
        for line in [
            "pub const PATH_BLOCK_BEGIN: &str = \"# >>> K5 CLI >>>\";\n",
            "pub const PATH_BLOCK_END: &str = \"# <<< K5 CLI <<<\";\n",
            "pub const PATH_BLOCK_EARLIER: &[(&str, &str)] = &[];\n",
        ] {
            assert!(out.contains(line), "{line:?} in:\n{out}");
        }
    }

    /// The product is in the marker lines, so it must be one line.
    #[test]
    fn a_product_with_a_control_character_is_refused() {
        for (tag, product) in [("nl", "A\\nB"), ("tab", "A\\tB")] {
            refused(
                &format!("product-{tag}"),
                &good_with(r#"product = "AB""#, &format!("product = \"{product}\"")),
                "must be one line with no control characters",
            );
        }
    }

    /// The software's name and the command's name are two values a brand
    /// sets apart: neither is made from the other. A helper's name comes
    /// from the command, never from the product.
    #[test]
    fn the_product_and_the_command_stay_apart() {
        let text =
            "id = \"gd\"\nschema = 1\n[names]\nproduct = \"Globex Desk\"\n[cli]\nname = \"gd\"\n";
        let t = TempBrand::new("apart", Some(text));
        let out = render(&load(&t.file(), &default_file()).expect("loads"));
        for line in [
            "        \"Globex Desk\"\n",
            "        \"gd\"\n",
            "        concat!(\"gd\", \"-\", $part)\n",
        ] {
            assert!(out.contains(line), "{line:?} in:\n{out}");
        }
        let helper = out
            .lines()
            .find(|l| l.contains("concat!("))
            .expect("the helper_name line");
        assert!(!helper.contains("Globex"), "{helper}");
    }

    #[test]
    fn strings_are_written_escaped_for_rust() {
        let t = TempBrand::new(
            "escape",
            Some(&good_with(
                r#"product = "AB""#,
                r#"product = "A \"B\" \\ C""#,
            )),
        );
        let out = render(&load(&t.file(), &default_file()).expect("loads"));
        assert!(
            out.contains(
                r#"        "A \"B\" \\ C"
"#
            ),
            "{out}"
        );
    }

    #[test]
    fn a_missing_file_is_refused() {
        let t = TempBrand::new("missing", None);
        let err = load(&t.file(), &default_file()).expect_err("no file");
        assert!(
            err.starts_with(&format!("brand file {}: can't read it", t.file().display())),
            "{err}"
        );
        assert!(err.contains("K2_BRAND_DIR"), "{err}");
    }

    #[test]
    fn a_schema_other_than_1_is_refused() {
        refused(
            "schema2",
            &good_with("schema = 1", "schema = 2"),
            "schema must be 1, not 2",
        );
        refused(
            "schema-text",
            &good_with("schema = 1", "schema = \"1\""),
            "schema must be 1, not \"1\"",
        );
        refused(
            "schema-none",
            &good_with("schema = 1", ""),
            "schema is missing; it must be 1",
        );
    }

    #[test]
    fn a_name_of_the_wrong_shape_is_refused() {
        for (i, bad) in ["a", "Ab", "1ab", "a-b", "a_b", "abcdefghijklm", "ab "]
            .iter()
            .enumerate()
        {
            let want = format!("cli.name {bad:?} must match ^[a-z][a-z0-9]{{1,11}}$");
            refused(
                &format!("name-{i}"),
                &good_with(r#"name = "ab""#, &format!("name = {bad:?}")),
                &want,
            );
        }
        refused(
            "name-empty",
            &good_with(r#"name = "ab""#, r#"name = """#),
            "cli.name is empty",
        );
        refused(
            "name-int",
            &good_with(r#"name = "ab""#, "name = 7"),
            "cli.name must be a string",
        );
        refused(
            "name-none",
            &good_with("name = \"ab\"\n", ""),
            "cli.name is missing",
        );
    }

    #[test]
    fn an_alias_of_the_wrong_shape_is_refused() {
        refused(
            "alias-shape",
            &good_with(r#"name = "abc""#, r#"name = "A-B""#),
            "cli.aliases[0].name \"A-B\" must match",
        );
        refused(
            "alias-long",
            &good_with(r#"name = "ab2""#, r#"name = "abcdefghijklm""#),
            "cli.aliases[2].name",
        );
        refused(
            "alias-none",
            &good_with(r#"name = "abc", "#, ""),
            "cli.aliases[0].name is missing",
        );
        refused(
            "alias-text",
            &good_with(r#"{ name = "ab1", mode = "silent" }"#, r#""ab1""#),
            "cli.aliases[1] must be a table",
        );
    }

    #[test]
    fn an_alias_that_is_the_name_is_refused() {
        refused(
            "alias-is-name",
            &good_with(r#"name = "ab1""#, r#"name = "ab""#),
            "cli.aliases[1].name \"ab\" is cli.name itself",
        );
    }

    #[test]
    fn an_alias_listed_twice_is_refused() {
        refused(
            "alias-twice",
            &good_with(r#"name = "ab2""#, r#"name = "abc""#),
            "cli.aliases[2].name \"abc\" is listed twice",
        );
    }

    #[test]
    fn a_mode_other_than_the_three_is_refused() {
        refused(
            "mode-loud",
            &good_with(r#"mode = "silent""#, r#"mode = "loud""#),
            "cli.aliases[1].mode \"loud\" must be silent, notice or tombstone",
        );
        refused(
            "mode-case",
            &good_with(r#"mode = "notice""#, r#"mode = "Notice""#),
            "cli.aliases[0].mode \"Notice\"",
        );
        refused(
            "mode-none",
            &good_with(r#", mode = "tombstone""#, ""),
            "cli.aliases[2].mode is missing",
        );
    }

    #[test]
    fn an_unknown_key_is_refused() {
        refused(
            "cli-key",
            &good_with("[cli]\n", "[cli]\nnmae = \"ab\"\n"),
            "[cli] holds a key it does not know: nmae",
        );
        refused(
            "alias-key",
            &good_with(r#"mode = "notice""#, r#"mode = "notice", why = "x""#),
            "cli.aliases[0] holds a key it does not know: why",
        );
    }

    #[test]
    fn missing_tables_and_fields_are_refused() {
        refused(
            "no-cli",
            "id = \"ab\"\nschema = 1\n[names]\nproduct = \"AB\"\n",
            "[cli] is missing",
        );
        refused(
            "no-names",
            &good_with("[names]\nproduct = \"AB\"\n", ""),
            "[names] is missing",
        );
        refused(
            "no-product",
            &good_with("product = \"AB\"\n", ""),
            "names.product is missing",
        );
        refused("no-id", &good_with("id = \"ab\"\n", ""), "id is missing");
        refused("aliases-text", "id = \"ab\"\nschema = 1\n[names]\nproduct = \"AB\"\n[cli]\nname = \"ab\"\naliases = \"abc\"\n", "cli.aliases must be a list");
        refused("not-toml", "id = ", "not valid TOML");
    }

    #[test]
    fn command_name_shape() {
        for good in ["ab", "k9", "a1b2", "abcdefghijkl"] {
            assert!(is_command_name(good), "{good}");
        }
        for bad in [
            "",
            "a",
            "1a",
            "Ab",
            "a-b",
            "a_b",
            "a b",
            "abcdefghijklm",
            "\u{e9}a",
        ] {
            assert!(!is_command_name(bad), "{bad}");
        }
    }

    /// Every brand folder in the repo holds a file this build can use, and
    /// its id is its folder's name.
    #[test]
    fn every_brand_folder_in_the_repo_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../brand");
        let mut seen = 0;
        for entry in std::fs::read_dir(&root).expect("the repo has a brand folder") {
            let dir = entry.expect("read a brand folder entry").path();
            if !dir.is_dir() {
                continue;
            }
            let brand =
                load(&dir.join("brand.toml"), &default_file()).unwrap_or_else(|e| panic!("{e}"));
            let folder = dir
                .file_name()
                .and_then(|n| n.to_str())
                .expect("a folder name");
            assert_eq!(brand.id, folder, "the id in {}", dir.display());
            seen += 1;
        }
        assert!(seen >= 1, "no brand folder under {}", root.display());
    }

    /// A brand file that sets every field, with neutral values.
    const FULL: &str = r##"
id = "ab"
schema = 1
[names]
product = "Initech Hub"
short = "Hub"
company = "Initech"
lockup = "Hub, by Initech"
server = "Hub Box"
login = "Initech Account"
[cli]
name = "ab"
[zones]
label = "boxes.example.invalid"
[control]
connect = "https://connect.example.invalid"
cert = "https://cert.example.invalid/cert"
push = "https://push.example.invalid"
relay = "relay.example.invalid"
relay_list = "https://example.invalid/api/tunnel/relays"
web_api = "https://api.example.invalid"
[feeds]
daemon = "https://updates.example.invalid/daemon-latest.json"
downloads = "https://updates.example.invalid/download"
[update]
keys = [
    { role = "primary", key_id = "25B77D84DA3C24B8", pubkey = "RWS4JDzahH23JaLG/sJZ6ECPc58g2hi5jkdAVvOH/XPJXUPnFf/6A99m" },
    { role = "backup", key_id = "437A6680B65F314B", pubkey = "RWRLMV+2gGZ6Q8yHeWW6I/DeKsDpzmn/FdSN6+8WfGpqDP+vMI0Slieq" },
]
[look]
icons = "assets/icons"
accent = "#3A6FF7"
[links]
site = "https://example.invalid"
support_email = "help@example.invalid"
support_url = "https://example.invalid/support"
terms = "https://example.invalid/terms"
privacy = "https://example.invalid/privacy"
dashboard = ""
feedback = ""
[mail]
system_from = "noreply@example.invalid"
system_from_name = "Hub mail"
postal = false
[phone]
bundle_id = "com.example.hub"
team_id = "ABCDE12345"
push_topic = "com.example.hub.push"
[sew]
call_name = "Hub Calls"
"##;

    /// `FULL` with `from` replaced by `to` (once; it must be there).
    fn full_with(from: &str, to: &str) -> String {
        assert!(FULL.contains(from), "the full file holds {from:?}");
        FULL.replacen(from, to, 1)
    }

    /// The default brand's fields, read alone.
    fn default_fields() -> Fields {
        let file = default_file();
        load(&file, &file)
            .expect("the default brand file loads")
            .fields
    }

    /// Every field a brand may set is read as set; the machine names are
    /// the default brand's (a brand may not name them otherwise yet).
    #[test]
    fn the_reader_takes_every_field() {
        let base = default_fields();
        let text = format!(
            "{FULL}[machine]\nhome = {:?}\nbinary_prefix = {:?}\nunit_prefix = {:?}\n",
            base.home, base.binary_prefix, base.unit_prefix
        );
        let t = TempBrand::new("full", Some(&text));
        let b = load(&t.file(), &default_file()).expect("the full file loads");
        let s = |v: &str| v.to_string();
        let want = Fields {
            short: s("Hub"),
            company: s("Initech"),
            lockup: s("Hub, by Initech"),
            server: s("Hub Box"),
            login: s("Initech Account"),
            label_zone: s("boxes.example.invalid"),
            connect: s("https://connect.example.invalid"),
            cert: s("https://cert.example.invalid/cert"),
            push: s("https://push.example.invalid"),
            relay: s("relay.example.invalid"),
            relay_list: s("https://example.invalid/api/tunnel/relays"),
            web_api: s("https://api.example.invalid"),
            daemon_feed: s("https://updates.example.invalid/daemon-latest.json"),
            downloads: s("https://updates.example.invalid/download"),
            home: base.home.clone(),
            binary_prefix: base.binary_prefix.clone(),
            unit_prefix: base.unit_prefix.clone(),
            icons: Some(s("assets/icons")),
            accent: Some(s("#3A6FF7")),
            site: s("https://example.invalid"),
            support_email: s("help@example.invalid"),
            support_url: Some(s("https://example.invalid/support")),
            terms: Some(s("https://example.invalid/terms")),
            privacy: Some(s("https://example.invalid/privacy")),
            dashboard: None,
            feedback: None,
            system_from: s("noreply@example.invalid"),
            system_from_name: s("Hub mail"),
            postal: false,
            phone_bundle_id: s("com.example.hub"),
            phone_team_id: s("ABCDE12345"),
            push_topic: s("com.example.hub.push"),
            sew_call_name: s("Hub Calls"),
        };
        assert_eq!(b.fields, want);
        let out = render(&b);
        for line in [
            "    pub const SHORT: &str = \"Hub\";\n",
            "    pub const LABEL: &str = \"boxes.example.invalid\";\n",
            "    pub const CONNECT: &str = \"https://connect.example.invalid\";\n",
            "    pub const RELAY_HOST: &str = \"relay.example.invalid\";\n",
            "    pub const DAEMON: &str = \"https://updates.example.invalid/daemon-latest.json\";\n",
            "    pub const ICONS: Option<&str> = Some(\"assets/icons\");\n",
            "    pub const ACCENT: Option<&str> = Some(\"#3A6FF7\");\n",
            "    pub const SUPPORT_URL: Option<&str> = Some(\"https://example.invalid/support\");\n",
            "    pub const DASHBOARD: Option<&str> = None;\n",
            "    pub const FEEDBACK: Option<&str> = None;\n",
            "    pub const POSTAL: bool = false;\n",
            "    pub const PUSH_TOPIC: &str = \"com.example.hub.push\";\n",
            "    pub const CALL_NAME: &str = \"Hub Calls\";\n",
        ] {
            assert!(out.contains(line), "{line:?} in:\n{out}");
        }
        // The names part is still the first thing written.
        assert!(out.starts_with(&render_names(&b)), "{out}");
    }

    /// A field a brand leaves out is the default brand's, except the ones
    /// made from the brand's own names.
    #[test]
    fn a_left_out_field_is_the_default_brands_or_made_from_the_names() {
        let base = default_fields();
        let t = TempBrand::new("left-out", Some(GOOD));
        let got = load(&t.file(), &default_file()).expect("loads").fields;
        let want = Fields {
            short: "AB".into(),
            lockup: format!("AB by {}", base.company),
            server: "AB Server".into(),
            system_from_name: "AB".into(),
            sew_call_name: "AB".into(),
            ..base.clone()
        };
        assert_eq!(got, want);
        // System mail comes from each box's own server name by default.
        assert_eq!(base.system_from, format!("noreply@{SERVER_SLOT}"));
        let t = TempBrand::new(
            "own-from",
            Some(&format!(
                "{GOOD}[mail]\nsystem_from = \"hello@{{server}}\"\n"
            )),
        );
        assert_eq!(
            load(&t.file(), &default_file())
                .expect("loads")
                .fields
                .system_from,
            "hello@{server}"
        );
        // An empty link is none, and stays none: it is not the default's.
        let t = TempBrand::new(
            "no-site-link",
            Some(&format!("{GOOD}[links]\ndashboard = \"\"\n")),
        );
        let got = load(&t.file(), &default_file()).expect("loads").fields;
        assert_eq!(got.dashboard, None);
        assert_eq!(got.site, base.site, "the rest of [links] is the default's");
    }

    /// The default brand's file is where every left-out field comes from,
    /// so it must spell each one; a gap in it stops every brand's build
    /// and says it was the default brand's file. A gap in its [control]
    /// is refused by the all-or-none rule, naming the key; a gap anywhere
    /// else by the field's own missing line.
    #[test]
    fn the_default_brand_file_spells_every_field() {
        let text = std::fs::read_to_string(default_file()).expect("read brand/k2");
        let mut gaps = Vec::new();
        for k in CONTROL_KEYS {
            let want =
                format!("[control] leaves out {k}: a brand that sets [control] sets all six");
            gaps.push((k.to_string(), want));
        }
        let field = "feeds.daemon is missing; the default brand file spells every field";
        gaps.push(("daemon".to_string(), field.to_string()));
        for (key, want) in gaps {
            let gap: String = text
                .lines()
                .filter(|l| !l.starts_with(&format!("{key} = ")))
                .map(|l| format!("{l}\n"))
                .collect();
            assert_eq!(
                gap.lines().count() + 1,
                text.lines().count(),
                "brand/k2 has one {key} line to take out"
            );
            let t = TempBrand::new(&format!("default-gap-{key}"), Some(&gap));
            let err = load(&t.file(), &t.file()).expect_err("a default with a gap");
            assert!(err.contains(&want), "{key}: {err}");
            let other = TempBrand::new(&format!("on-a-gap-{key}"), Some(GOOD));
            let err = load(&other.file(), &t.file()).expect_err("a brand on a default with a gap");
            assert!(err.ends_with("(the default brand file)"), "{key}: {err}");
            assert!(err.contains(&want), "{key}: {err}");
        }
    }

    /// Each shape refused, naming the field.
    #[test]
    fn a_field_of_the_wrong_shape_is_refused() {
        for (i, (from, to, want)) in [
            ("label = \"boxes.example.invalid\"", "label = \"Boxes.Example\"", "zones.label \"Boxes.Example\" must be a domain name"),
            ("label = \"boxes.example.invalid\"", "label = \"example\"", "zones.label \"example\" must be a domain name"),
            ("label = \"boxes.example.invalid\"", "label = \"boxes.example.invalid\"\ninternal = true", "[zones] holds a key it does not know: internal"),
            ("connect = \"https://connect.example.invalid\"", "connect = \"http://connect.example.invalid\"", "control.connect \"http://connect.example.invalid\" must be an https URL"),
            ("connect = \"https://connect.example.invalid\"", "connect = \"https://connect.example.invalid/a b\"", "control.connect \"https://connect.example.invalid/a b\" must be an https URL"),
            ("connect = \"https://connect.example.invalid\"", "connect = \"https://connect.example.invalid:8443\"", "must be an https URL"),
            ("relay = \"relay.example.invalid\"", "relay = \"relay\"", "control.relay \"relay\" must be a domain name"),
            ("daemon = \"https://updates.example.invalid/daemon-latest.json\"", "daemon = \"\"", "feeds.daemon is empty"),
            ("key_id = \"25B77D84DA3C24B8\"", "key_id = \"0000000000000000\"", "update.keys[0].key_id \"0000000000000000\" is not the pubkey's key id (25B77D84DA3C24B8)"),
            ("pubkey = \"RWS4JDzahH23JaLG/sJZ6ECPc58g2hi5jkdAVvOH/XPJXUPnFf/6A99m\"", "pubkey = \"RWnotakey\"", "update.keys[0].pubkey is not a minisign public key line"),
            ("role = \"primary\"", "role = \"spare\"", "update.keys[0].role \"spare\" must be primary or backup"),
            ("role = \"primary\"", "role = \"backup\"", "update.keys must start with a primary key"),
            ("key_id = \"437A6680B65F314B\", pubkey = \"RWRLMV+2gGZ6Q8yHeWW6I/DeKsDpzmn/FdSN6+8WfGpqDP+vMI0Slieq\"", "key_id = \"25B77D84DA3C24B8\", pubkey = \"RWS4JDzahH23JaLG/sJZ6ECPc58g2hi5jkdAVvOH/XPJXUPnFf/6A99m\"", "update.keys lists 25B77D84DA3C24B8 twice"),
            ("role = \"primary\", key_id", "role = \"primary\", why = \"x\", key_id", "update.keys[0] holds a key it does not know: why"),
            ("accent = \"#3A6FF7\"", "accent = \"blue\"", "look.accent \"blue\" must be a colour in hex"),
            ("accent = \"#3A6FF7\"", "accent = \"#3A6FF\"", "look.accent \"#3A6FF\" must be a colour in hex"),
            ("icons = \"assets/icons\"", "icons = \"/abs/icons\"", "look.icons \"/abs/icons\" must be a folder from the brand folder"),
            ("icons = \"assets/icons\"", "icons = \"assets//icons\"", "look.icons \"assets//icons\" must be a folder"),
            ("support_email = \"help@example.invalid\"", "support_email = \"help\"", "links.support_email \"help\" must be an email address"),
            ("terms = \"https://example.invalid/terms\"", "terms = \"http://example.invalid/terms\"", "links.terms \"http://example.invalid/terms\" must be an https URL"),
            ("dashboard = \"\"", "dashboard = 7", "links.dashboard must be a string (\"\" for none)"),
            ("site = \"https://example.invalid\"", "site = \"\"", "links.site is empty"),
            ("system_from = \"noreply@example.invalid\"", "system_from = \"noreply\"", "mail.system_from \"noreply\" must be an email address"),
            ("system_from = \"noreply@example.invalid\"", "system_from = \"noreply@{box}\"", "mail.system_from \"noreply@{box}\" must be an email address"),
            ("system_from = \"noreply@example.invalid\"", "system_from = \"no reply@{server}\"", "mail.system_from \"no reply@{server}\" must be an email address: <name>@{server} or <name>@<domain>"),
            ("postal = false", "postal = 0", "mail.postal must be true or false"),
            ("bundle_id = \"com.example.hub\"", "bundle_id = \"Com.Example\"", "phone.bundle_id \"Com.Example\" must be a reverse-DNS id"),
            ("team_id = \"ABCDE12345\"", "team_id = \"abc\"", "phone.team_id \"abc\" must be an Apple team id"),
            ("short = \"Hub\"", "short = \" Hub\"", "names.short \" Hub\" must be one line of 1 to 40 characters"),
            ("short = \"Hub\"", "short = \"Hub\\nTwo\"", "names.short \"Hub\\nTwo\" must be one line"),
            ("call_name = \"Hub Calls\"", "call_name = \"\"", "sew.call_name is empty"),
            ("[sew]", "[sounds]", "a top-level key or table this build does not know: sounds"),
            ("site = ", "website = ", "[links] holds a key it does not know: website"),
        ]
        .iter()
        .enumerate()
        {
            refused(&format!("shape-{i}"), &full_with(from, to), want);
        }
        for (tag, text, want) in [
            (
                "rh-table",
                format!("{GOOD}[root_helper]\nkeys = []\n"),
                "root_helper: ",
            ),
            (
                "rh-key",
                format!("{GOOD}[update]\nroot_helper = []\n"),
                "update.root_helper: ",
            ),
            (
                "rh-top",
                format!("root_helper_keys = []\n{GOOD}"),
                "root_helper_keys: ",
            ),
        ] {
            refused(tag, &text, &format!("{want}{ROOT_HELPER_REFUSED}"));
        }
        assert!(ROOT_HELPER_REFUSED.contains("signed only by K5's release keys"));
        refused(
            "look-text",
            &format!("look = \"blue\"\n{GOOD}"),
            "[look] must be a table",
        );
    }

    /// The machine names are what the code still uses everywhere: a brand
    /// may not name them otherwise until the code reads them.
    #[test]
    fn a_machine_name_other_than_the_defaults_is_refused() {
        let base = default_fields();
        for (i, (key, other, want)) in [
            (
                "home",
                "\".gy\"",
                format!("machine.home \".gy\" is not {:?}", base.home),
            ),
            (
                "home",
                "\"gy\"",
                "machine.home \"gy\" must be a dot and a name".to_string(),
            ),
            (
                "binary_prefix",
                "\"gy\"",
                format!(
                    "machine.binary_prefix \"gy\" is not {:?}",
                    base.binary_prefix
                ),
            ),
            (
                "unit_prefix",
                "\"Gy\"",
                "machine.unit_prefix \"Gy\" must match".to_string(),
            ),
            (
                "tag",
                "\"gy\"",
                "[machine] holds a key it does not know: tag".to_string(),
            ),
        ]
        .iter()
        .enumerate()
        {
            refused(
                &format!("machine-{i}"),
                &format!("{GOOD}[machine]\n{key} = {other}\n"),
                &want,
            );
        }
        // The default's own values pass.
        let t = TempBrand::new(
            "machine-same",
            Some(&format!("{GOOD}[machine]\nhome = {:?}\n", base.home)),
        );
        assert_eq!(
            load(&t.file(), &default_file()).expect("loads").fields.home,
            base.home
        );
    }

    #[test]
    fn domain_and_url_shapes() {
        for good in [
            "example.invalid",
            "a.b.example.invalid",
            "x-1.example.invalid",
            "k9.dev",
        ] {
            assert!(is_domain(good), "{good}");
        }
        for bad in [
            "",
            "example",
            "Example.invalid",
            "-a.example.invalid",
            "a-.example.invalid",
            "a..invalid",
            "a.invalid.",
            "a.in1",
            "a b.invalid",
            "\u{e9}.invalid",
        ] {
            assert!(!is_domain(bad), "{bad}");
        }
        for good in [
            "https://example.invalid",
            "https://example.invalid/",
            "https://a.example.invalid/x/y.json",
            "https://example.invalid/a?b=c",
        ] {
            assert!(is_https_url(good), "{good}");
        }
        for bad in [
            "http://example.invalid",
            "https://",
            "https://example",
            "https://example.invalid:443",
            "https://example.invalid/a b",
            "https://example.invalid/\"",
            "ftp://example.invalid",
            " https://example.invalid",
        ] {
            assert!(!is_https_url(bad), "{bad}");
        }
    }

    /// The key id reader agrees with the ids B9's readers checked for the
    /// default brand's update keys, and refuses what is not a key line.
    #[test]
    fn minisign_key_ids_are_read_as_minisign_prints_them() {
        let doc: toml::Table =
            toml::from_str(&std::fs::read_to_string(default_file()).expect("read")).expect("toml");
        let keys = doc["update"]["keys"].as_array().expect("update.keys");
        assert!(!keys.is_empty());
        for k in keys {
            let pubkey = k["pubkey"].as_str().expect("pubkey");
            let key_id = k["key_id"].as_str().expect("key_id");
            assert_eq!(minisign_key_id(pubkey).as_deref(), Some(key_id), "{pubkey}");
        }
        for bad in [
            "",
            "RWS",
            "RWS4JDzahH23JaLG/sJZ6ECPc58g2hi5jkdAVvOH/XPJXUPnFf/6A99",
            "!WS4JDzahH23JaLG/sJZ6ECPc58g2hi5jkdAVvOH/XPJXUPnFf/6A99m",
            "AAAAJDzahH23JaLG/sJZ6ECPc58g2hi5jkdAVvOH/XPJXUPnFf/6A99m",
        ] {
            assert_eq!(minisign_key_id(bad), None, "{bad}");
        }
    }

    /// `pub const NAME: &str = "...";` in a repo file, as written.
    fn rust_const(file: &str, name: &str) -> String {
        let text =
            std::fs::read_to_string(repo(file)).unwrap_or_else(|e| panic!("read {file}: {e}"));
        let head = format!("pub const {name}: &str = \"");
        let line = text
            .lines()
            .find(|l| l.starts_with(&head))
            .unwrap_or_else(|| panic!("no one-line {head}...\"; in {file}"));
        line[head.len()..]
            .strip_suffix("\";")
            .unwrap_or_else(|| panic!("{file}: {line}"))
            .to_string()
    }

    /// Drift: the default brand's phone app is the one the daemon's pair
    /// page and the approver name today (`<team>.<bundle id>`), and its
    /// push topic is the bundle id (an APNs topic is the app's bundle id).
    #[test]
    fn the_default_brands_phone_app_is_todays() {
        let f = default_fields();
        let app = format!("{}.{}", f.phone_team_id, f.phone_bundle_id);
        assert_eq!(
            rust_const("crates/k2-daemon/src/pair_page.rs", "COMPANION_APP_ID"),
            app
        );
        assert_eq!(
            rust_const("crates/k2-approve/src/attest.rs", "K2_PHONE_APP_ID"),
            app
        );
        assert_eq!(f.push_topic, f.phone_bundle_id);
    }

    /// The distributor template's example brand is a file this build uses.
    #[test]
    fn the_distributor_templates_brand_loads() {
        let b = load(
            &repo("templates/distributor/brand/brand.toml"),
            &default_file(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let base = default_fields();
        for (what, mine, ours) in [
            ("names.company", &b.fields.company, &base.company),
            ("names.login", &b.fields.login, &base.login),
            ("zones.label", &b.fields.label_zone, &base.label_zone),
            ("links.site", &b.fields.site, &base.site),
            (
                "links.support_email",
                &b.fields.support_email,
                &base.support_email,
            ),
            ("feeds.daemon", &b.fields.daemon_feed, &base.daemon_feed),
        ] {
            assert_ne!(mine, ours, "the template sets its own {what}");
        }
    }

    /// `[control]` is all six or none: a brand that sets one control
    /// endpoint sets them all (K2's ruling), so ours and a brand's are
    /// never mixed. Each one left out is named.
    #[test]
    fn a_partial_control_table_is_refused() {
        for (i, key) in CONTROL_KEYS.iter().enumerate() {
            let line = FULL
                .lines()
                .find(|l| l.starts_with(&format!("{key} = ")))
                .unwrap_or_else(|| panic!("FULL sets control.{key}"));
            refused(
                &format!("control-partial-{i}"),
                &full_with(&format!("{line}\n"), ""),
                &format!("[control] leaves out {key}: a brand that sets [control] sets all six"),
            );
        }
        refused(
            "control-empty",
            &format!("{GOOD}[control]\n"),
            "[control] leaves out relay, relay_list, connect, cert, push, web_api",
        );
    }

    /// brand/xy has no [control]: it resolves to exactly our six control
    /// addresses, the ones `the_service_addresses_are_what_they_were`
    /// (k2-client's hosts module) spells out.
    #[test]
    fn a_brand_with_no_control_table_has_our_six_addresses() {
        let xy = std::fs::read_to_string(repo("brand/xy/brand.toml")).expect("read brand/xy");
        let doc: toml::Table = toml::from_str(&xy).expect("brand/xy is TOML");
        assert!(
            doc.get("control").is_none(),
            "brand/xy leaves [control] out"
        );
        let f = load(&repo("brand/xy/brand.toml"), &default_file())
            .expect("brand/xy loads")
            .fields;
        let hosts =
            std::fs::read_to_string(repo("crates/k2-client/src/hosts.rs")).expect("read hosts.rs");
        let spelled = |name: &str| -> String {
            let head = format!("assert_eq!({name}, \"");
            let line = hosts
                .lines()
                .map(str::trim)
                .find(|l| l.starts_with(&head))
                .unwrap_or_else(|| panic!("hosts.rs spells out {name}"));
            line[head.len()..]
                .strip_suffix("\");")
                .unwrap_or_else(|| panic!("{line}"))
                .to_string()
        };
        assert_eq!(
            [
                &f.relay,
                &f.relay_list,
                &f.connect,
                &f.cert,
                &f.push,
                &f.web_api
            ]
            .map(String::as_str),
            [
                spelled("RELAY_HOST"),
                spelled("RELAY_LIST_URL"),
                spelled("CONNECT_BASE"),
                spelled("CERT_BROKER_URL"),
                spelled("PUSH_GATEWAY_URL"),
                spelled("WEB_API_BASE"),
            ]
            .each_ref()
            .map(String::as_str)
        );
        let base = default_fields();
        assert_eq!(
            (
                &f.relay,
                &f.relay_list,
                &f.connect,
                &f.cert,
                &f.push,
                &f.web_api
            ),
            (
                &base.relay,
                &base.relay_list,
                &base.connect,
                &base.cert,
                &base.push,
                &base.web_api
            )
        );
    }
}
