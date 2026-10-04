//! Ticket HTML brief (prd-ticket-html-brief-v1.md, vs-live H29–H44).
//!
//! An agent that opens a ticket attaches a short HTML page, the
//! **brief**: the problem, what it tried, what it needs, the options.
//! The brief is untrusted. The daemon is the only judge (H27): this
//! module checks it (UTF-8, cap, visible text), cleans it against a
//! fixed allowlist (ammonia, H11/H34), extracts a plain-text copy, and
//! stores it in `feedback_briefs` in the same transaction as the ticket
//! row (H6/H8, migration 0126). Clients only render what comes back.
//!
//! Every open product question Rosson has not answered yet is one
//! constant in the "Policy" block below, so changing a default is a
//! one-line edit:
//!
//! - [`BRIEF_POLICY`]: 0.43.2 is the grace release (`Warn`); 0.43.3
//!   flips it to `Require` (H25, S8).
//! - [`FYI_NEEDS_BRIEF`]: `fyi` tickets are exempt (default 2).
//! - [`MAX_BRIEF_BYTES`]: 1 MiB raw input (default 4, H7).
//! - [`WARN_WITHOUT_NEED_SECTION`]: a missing `.k2-need` section only
//!   warns (default 5, H16).
//!
//! The owner-token door is the agent door (default 1, H1); the daemon
//! route decides the door and calls [`brief_required`].
//!
//! The same door also carries the **assignee policy** (0.43.2): an agent
//! ticket should name a user on this server. See the "Assignee policy"
//! block: [`ASSIGNEE_POLICY`] is `Warn` now and flips to `Require` in one
//! line, exactly like [`BRIEF_POLICY`].

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, CharacterTokens, CommentToken, EndTag, StartTag, Tag, TagToken, Token,
    TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use rusqlite::params;
use sha2::{Digest, Sha256};

// ── Policy (Rosson's open questions live here) ──────────────────────────

/// Whether the agent door must attach a brief.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BriefPolicy {
    /// A missing brief still files the ticket; the response carries a
    /// `brief_missing` warning (exit 0 in the CLI).
    Warn,
    /// A missing brief is refused: 400 `brief_required`.
    Require,
}

/// THE switch. 0.43.2 ships `Warn` (one grace release, H25: long-running
/// agents still hold the old skill text). **0.43.3 flips this to
/// `BriefPolicy::Require`** — one line, plus the Require column of the
/// door-matrix test already runs through [`with_policy_override`].
pub const BRIEF_POLICY: BriefPolicy = BriefPolicy::Warn;

/// Rosson default 2: an `fyi` ticket asks nothing of the human, so it
/// never needs a brief (it may still carry one). Flip to `true` to make
/// every agent kind need one (the PRD's H2 recommendation).
pub const FYI_NEEDS_BRIEF: bool = false;

/// Rosson default 5 / H16: the daemon never requires particular
/// sections. When this is true, a brief without a `.k2-need` element
/// gets a `brief_no_need` warning.
pub const WARN_WITHOUT_NEED_SECTION: bool = true;

/// Cap on the raw brief, in bytes (H7, Rosson default 4). The cleaned
/// HTML must fit under it too.
pub const MAX_BRIEF_BYTES: usize = 1024 * 1024;

/// Cap on the whole `POST /cli/feedback/create` body (H30): the 1 MiB
/// brief + JSON escaping + base64 slack. The dispatcher refuses a larger
/// `Content-Length` before reading the body.
pub const MAX_CREATE_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Cap on the stored plain-text extract.
pub const MAX_TEXT_BYTES: usize = 16 * 1024;

/// Allowlist version tag stored with every brief (H34). Changing the
/// allowlist bumps it; stored rows are never re-cleaned.
pub const SANITIZER_TAG: &str = "k2-brief-v1";

/// The live policy. [`BRIEF_POLICY`], unless a test on this thread set
/// an override with [`with_policy_override`] (H36: one test binary runs
/// both columns of the door matrix).
pub fn brief_policy() -> BriefPolicy {
    #[cfg(any(test, feature = "test-util"))]
    {
        if let Some(p) = POLICY_OVERRIDE.with(|c| c.get()) {
            return p;
        }
    }
    BRIEF_POLICY
}

/// Must a caller through the agent door attach a brief for this kind?
/// People (app guests, Connect users) never need one; the route only
/// calls this for the owner-token door.
pub fn brief_required(kind: &str) -> bool {
    FYI_NEEDS_BRIEF || kind != "fyi"
}

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    static POLICY_OVERRIDE: Cell<Option<BriefPolicy>> = const { Cell::new(None) };
    static FAIL_NEXT_INSERT: Cell<bool> = const { Cell::new(false) };
}

/// Test-only: run `f` with the policy forced to `policy` on THIS thread.
#[cfg(any(test, feature = "test-util"))]
pub fn with_policy_override<R>(policy: BriefPolicy, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<BriefPolicy>);
    impl Drop for Reset {
        fn drop(&mut self) {
            POLICY_OVERRIDE.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(POLICY_OVERRIDE.with(|c| c.replace(Some(policy))));
    f()
}

/// Test-only: make the next [`insert_with`] on THIS thread fail, to
/// prove the ticket row rolls back with it (T3).
#[cfg(any(test, feature = "test-util"))]
pub fn fail_next_insert_for_test() {
    FAIL_NEXT_INSERT.with(|c| c.set(true));
}

// ── Errors and warnings ─────────────────────────────────────────────────

/// Why a brief was refused (H3). Each maps to a stable wire code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BriefError {
    /// Raw input (or the cleaned result) is over [`MAX_BRIEF_BYTES`].
    TooLarge { bytes: usize },
    /// No visible text after cleaning.
    Empty,
    /// Not UTF-8.
    Invalid,
}

impl BriefError {
    pub fn code(&self) -> &'static str {
        match self {
            BriefError::TooLarge { .. } => "brief_too_large",
            BriefError::Empty => "brief_empty",
            BriefError::Invalid => "brief_invalid",
        }
    }

    pub fn hint(&self) -> String {
        match self {
            BriefError::TooLarge { bytes } => format!(
                "the brief is {bytes} bytes; the cap is {MAX_BRIEF_BYTES} bytes (1 MiB, \
                 inline images included). Shrink or drop screenshots. See `k2 study ticket-brief`."
            ),
            BriefError::Empty => "the brief has no visible text after cleaning. Fill in \
                 `k2 tickets template` (Problem, What I tried, What I need from you). \
                 See `k2 study ticket-brief`."
                .to_string(),
            BriefError::Invalid => "the brief is not valid UTF-8. Save the file as UTF-8 \
                 and try again. See `k2 study ticket-brief`."
                .to_string(),
        }
    }
}

/// Hint for 400 `brief_required` and the `brief_missing` warning.
pub const BRIEF_REQUIRED_HINT: &str = "agents must attach an HTML brief. Run \
    `k2 tickets template > brief.html`, fill it in, then \
    `k2 tickets ask \"<title>\" --html brief.html`. See `k2 study ticket-brief`.";

/// One `{code, hint}` warning on the create response (H17/H25).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BriefWarning {
    pub code: &'static str,
    pub hint: String,
}

/// The `brief_missing` warning (Warn policy, agent door, no brief).
pub fn missing_warning() -> BriefWarning {
    BriefWarning {
        code: "brief_missing",
        hint: format!(
            "{BRIEF_REQUIRED_HINT} This ticket was filed anyway: K2 0.43.2 only warns. \
             The next release refuses an agent ticket without a brief."
        ),
    }
}

// ── Assignee policy (0.43.2) ────────────────────────────────────────────
//
// An agent's ticket should be assigned to a person on this server, so it
// lands on someone's board (and in their push) instead of nobody's. Same
// shape as the brief: the owner-token door only, `fyi` exempt, `Warn`
// for now. "A user on this server" is the host owner (the literal
// `owner`, or the owner display name) or any stored Connect user: the
// people list `k2 connections list --users` prints
// (`connect_users::list_people_for_agents`). The daemon route builds
// that list and calls [`unknown_assignees`].

/// Whether the agent door must assign the ticket to a user on this server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssigneePolicy {
    /// No assignee (or a name that is not a user here) still files the
    /// ticket; the response carries an `assignee_required` /
    /// `assignee_unknown` warning (exit 0 in the CLI).
    Warn,
    /// Refused: 400 `assignee_required` / `assignee_unknown`.
    Require,
}

/// THE switch. 0.43.2 ships `Warn` (agents still run the old skill text).
/// **A later release flips this to `AssigneePolicy::Require`**: one
/// line; the Require column of the door-matrix test already runs through
/// [`with_assignee_policy_override`].
pub const ASSIGNEE_POLICY: AssigneePolicy = AssigneePolicy::Warn;

/// `fyi` tickets ask nothing of a person, so they need no assignee (they
/// may still carry one), same as [`FYI_NEEDS_BRIEF`].
pub const FYI_NEEDS_ASSIGNEE: bool = false;

/// The live assignee policy: [`ASSIGNEE_POLICY`] unless a test on this
/// thread set an override.
pub fn assignee_policy() -> AssigneePolicy {
    #[cfg(any(test, feature = "test-util"))]
    {
        if let Some(p) = ASSIGNEE_POLICY_OVERRIDE.with(|c| c.get()) {
            return p;
        }
    }
    ASSIGNEE_POLICY
}

/// Must a caller through the agent door assign this kind of ticket?
/// People (app guests, Connect users) never must; the route only calls
/// this for the owner-token door.
pub fn assignee_required(kind: &str) -> bool {
    FYI_NEEDS_ASSIGNEE || kind != "fyi"
}

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    static ASSIGNEE_POLICY_OVERRIDE: Cell<Option<AssigneePolicy>> = const { Cell::new(None) };
}

/// Test-only: run `f` with the assignee policy forced on THIS thread.
#[cfg(any(test, feature = "test-util"))]
pub fn with_assignee_policy_override<R>(policy: AssigneePolicy, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<AssigneePolicy>);
    impl Drop for Reset {
        fn drop(&mut self) {
            ASSIGNEE_POLICY_OVERRIDE.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(ASSIGNEE_POLICY_OVERRIDE.with(|c| c.replace(Some(policy))));
    f()
}

/// The rule, in the words agents see (warning, refusal, skill text).
pub const ASSIGNEE_REQUIRED_MESSAGE: &str = "A ticket must be assigned to a user on this \
    server (`--assign <user>`). This will be required in a future update.";

/// How to fix it: who the users are, and how to assign.
pub const ASSIGNEE_REQUIRED_HINT: &str = "Run `k2 connections list --users` to see the \
    users on this server, then `k2 tickets ask \"<title>\" --assign <user>`. Fix a filed \
    ticket with `k2 tickets assign <id> <user>`. See `k2 study ticket-brief`.";

/// The `assignee_required` warning (Warn policy, agent door, no assignee).
/// `ticket_id` (when filed) makes the fix command copy-pasteable.
pub fn assignee_missing_warning(ticket_id: Option<&str>) -> BriefWarning {
    let fix = match ticket_id {
        Some(id) => format!(
            " This ticket was filed unassigned: assign it now with \
             `k2 tickets assign {} <user>`.",
            &id[..id.len().min(8)]
        ),
        None => String::new(),
    };
    BriefWarning {
        code: "assignee_required",
        hint: format!("{ASSIGNEE_REQUIRED_MESSAGE}{fix} {ASSIGNEE_REQUIRED_HINT}"),
    }
}

/// Text for the `assignee_unknown` warning / refusal.
pub fn assignee_unknown_hint(unknown: &[String]) -> String {
    let who = unknown
        .iter()
        .map(|u| format!("`{u}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let noun = if unknown.len() == 1 { "is not a user" } else { "are not users" };
    format!(
        "{who} {noun} on this server (the owner or a Connect user). \
         {ASSIGNEE_REQUIRED_MESSAGE} {ASSIGNEE_REQUIRED_HINT}"
    )
}

/// The `assignee_unknown` warning.
pub fn assignee_unknown_warning(unknown: &[String]) -> BriefWarning {
    BriefWarning {
        code: "assignee_unknown",
        hint: assignee_unknown_hint(unknown),
    }
}

/// Trim, drop blanks, dedup (first spelling wins): the same cleaning
/// `feedback::set_assignees` applies before it stores the snapshots.
pub fn clean_assignees(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let t = n.trim();
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// The names in `assignees` that are not users on this server. `known`
/// is the people list (owner display name + Connect usernames); the wire
/// literal `owner` always counts. Case-insensitive, like Connect
/// usernames and the owner dedup in `list_people_for_agents`.
pub fn unknown_assignees(assignees: &[String], known: &[String]) -> Vec<String> {
    clean_assignees(assignees)
        .into_iter()
        .filter(|a| {
            !a.eq_ignore_ascii_case("owner")
                && !known.iter().any(|k| k.trim().eq_ignore_ascii_case(a))
        })
        .collect()
}

// ── Allowlist (H11 + H34) ───────────────────────────────────────────────

/// Tags kept by the cleaner.
pub const ALLOWED_TAGS: &[&str] = &[
    "h1", "h2", "h3", "h4", "p", "br", "hr", "ul", "ol", "li", "dl", "dt", "dd", "strong", "em",
    "b", "i", "u", "s", "mark", "small", "sub", "sup", "code", "pre", "kbd", "blockquote", "a",
    "img", "table", "caption", "thead", "tbody", "tfoot", "tr", "th", "td", "section", "header",
    "footer", "div", "span", "details", "summary", "figure", "figcaption",
];

/// Tags removed WITH their contents (everything else outside the
/// allowlist is unwrapped: the tag goes, its text stays).
const CLEAN_CONTENT_TAGS: &[&str] = &[
    "script", "style", "title", "template", "iframe", "object", "embed", "svg", "math",
    "noscript", "textarea", "select", "head",
];

/// The only classes a brief may use (H34): styling hooks K2's brief
/// stylesheet knows. Agents can't invent new ones.
pub const ALLOWED_CLASSES: &[&str] = &["k2-need", "k2-options", "k2-callout", "k2-brief"];

/// `href` schemes kept on `a` (H11).
const HREF_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// Attributes allowed per tag (besides `title` everywhere and `class`
/// through [`ALLOWED_CLASSES`]).
fn tag_attributes() -> HashMap<&'static str, HashSet<&'static str>> {
    HashMap::from([
        ("a", HashSet::from(["href"])),
        ("img", HashSet::from(["src", "alt", "width", "height"])),
        ("td", HashSet::from(["colspan", "rowspan"])),
        ("th", HashSet::from(["colspan", "rowspan"])),
        ("details", HashSet::from(["open"])),
    ])
}

/// `img src` is only an inline raster image.
fn is_allowed_img_src(value: &str) -> bool {
    let v = value.trim_start().to_ascii_lowercase();
    ["png", "jpeg", "gif", "webp"]
        .iter()
        .any(|kind| v.starts_with(&format!("data:image/{kind};base64,")))
}

/// `href` is an absolute http/https/mailto URL.
fn is_allowed_href(value: &str) -> bool {
    match url::Url::parse(value.trim()) {
        Ok(u) => HREF_SCHEMES.contains(&u.scheme()),
        Err(_) => false,
    }
}

fn builder() -> ammonia::Builder<'static> {
    let mut b = ammonia::Builder::empty();
    b.tags(ALLOWED_TAGS.iter().copied().collect())
        .clean_content_tags(CLEAN_CONTENT_TAGS.iter().copied().collect())
        .generic_attributes(HashSet::from(["title"]))
        .tag_attributes(tag_attributes())
        .allowed_classes(
            ALLOWED_TAGS
                .iter()
                .map(|t| (*t, ALLOWED_CLASSES.iter().copied().collect::<HashSet<_>>()))
                .collect(),
        )
        // `url_schemes` is global (H34), so `data` is listed here and the
        // attribute filter narrows it: no `data:` href, and an img src
        // must be an inline png/jpeg/gif/webp.
        .url_schemes(HashSet::from(["http", "https", "mailto", "data"]))
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(Some("noopener noreferrer"))
        .strip_comments(true)
        .attribute_filter(|element, attribute, value| match (element, attribute) {
            ("a", "href") if !is_allowed_href(value) => None,
            ("img", "src") if !is_allowed_img_src(value) => None,
            _ => Some(value.into()),
        });
    b
}

/// Clean HTML against the brief allowlist. A full document is accepted;
/// its body content is kept (the fragment parser drops html/head/body).
pub fn sanitize(raw: &str) -> String {
    builder().clean(raw).to_string()
}

// ── Cleaning entry point ────────────────────────────────────────────────

/// A brief that passed every check, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanBrief {
    pub html: String,
    pub text: String,
    pub bytes: i64,
    pub sha256: String,
    pub sanitizer: String,
    /// The cleaner removed something from the input (`brief_sanitized`).
    pub removed: bool,
    /// The cleaned brief has a `.k2-need` element.
    pub has_need: bool,
}

impl CleanBrief {
    /// Non-fatal notes for the create response.
    pub fn warnings(&self) -> Vec<BriefWarning> {
        let mut out = Vec::new();
        if self.removed {
            out.push(BriefWarning {
                code: "brief_sanitized",
                hint: "K2 removed markup the brief may not use (scripts, styles, forms, \
                       remote images, comments, unknown tags or attributes). Check how it \
                       reads with `k2 tickets show <id> --html`. See `k2 study ticket-brief`."
                    .to_string(),
            });
        }
        if WARN_WITHOUT_NEED_SECTION && !self.has_need {
            out.push(BriefWarning {
                code: "brief_no_need",
                hint: "the brief has no `<section class=\"k2-need\">` (What I need from you). \
                       Say plainly what the human must decide or do. \
                       See `k2 tickets template`."
                    .to_string(),
            });
        }
        out
    }
}

/// Check and clean raw brief bytes (H7, H11, H16): UTF-8, the raw cap,
/// clean, the cleaned cap, visible text. Pure: no DB, no lock (H33c:
/// clean BEFORE taking the shared connection).
pub fn clean_bytes(raw: &[u8]) -> Result<CleanBrief, BriefError> {
    if raw.len() > MAX_BRIEF_BYTES {
        return Err(BriefError::TooLarge { bytes: raw.len() });
    }
    let s = std::str::from_utf8(raw).map_err(|_| BriefError::Invalid)?;
    clean(s)
}

/// [`clean_bytes`] for input that is already a `str` (the JSON body).
pub fn clean(raw: &str) -> Result<CleanBrief, BriefError> {
    if raw.len() > MAX_BRIEF_BYTES {
        return Err(BriefError::TooLarge { bytes: raw.len() });
    }
    let html = sanitize(raw);
    if html.len() > MAX_BRIEF_BYTES {
        return Err(BriefError::TooLarge { bytes: html.len() });
    }
    let (text, has_need) = extract(&html);
    if text.trim().is_empty() {
        return Err(BriefError::Empty);
    }
    let sha256 = sha256_hex(html.as_bytes());
    Ok(CleanBrief {
        bytes: html.len() as i64,
        text,
        sha256,
        sanitizer: SANITIZER_TAG.to_string(),
        removed: removed_anything(raw),
        has_need,
        html,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

// ── Tokenizer helpers ───────────────────────────────────────────────────

/// Run the html5ever tokenizer over `html`, handing each token to `on`.
/// Switches to the raw-text states for script/style/etc. the way the
/// tree builder would, so their contents never read as tags.
fn tokenize(html: &str, on: impl FnMut(Token)) {
    struct Sink<F: FnMut(Token)> {
        on: RefCell<F>,
    }
    impl<F: FnMut(Token)> TokenSink for Sink<F> {
        type Handle = ();
        fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
            let raw = match &token {
                TagToken(Tag { kind: StartTag, name, self_closing: false, .. }) => {
                    match &**name {
                        "script" => Some(TokenSinkResult::RawData(RawKind::ScriptData)),
                        "style" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript" => {
                            Some(TokenSinkResult::RawData(RawKind::Rawtext))
                        }
                        "title" | "textarea" => Some(TokenSinkResult::RawData(RawKind::Rcdata)),
                        "plaintext" => Some(TokenSinkResult::Plaintext),
                        _ => None,
                    }
                }
                _ => None,
            };
            (self.on.borrow_mut())(token);
            raw.unwrap_or(TokenSinkResult::Continue)
        }
    }
    let input = BufferQueue::default();
    input.push_back(StrTendril::from(html));
    let tok = Tokenizer::new(Sink { on: RefCell::new(on) }, TokenizerOpts::default());
    let _ = tok.feed(&input);
    tok.end();
}

/// Did cleaning remove anything from `raw`? A token-level check against
/// the same allowlist the cleaner uses: a comment, a tag outside the
/// allowlist, an attribute outside it, a class outside
/// [`ALLOWED_CLASSES`], or a URL the filter drops. Structural wrappers
/// of a full document (html/head/body/title, `<meta charset>`) and a
/// doctype are expected and don't count.
pub fn removed_anything(raw: &str) -> bool {
    let tags: HashSet<&str> = ALLOWED_TAGS.iter().copied().collect();
    let per_tag = tag_attributes();
    let removed = Cell::new(false);
    tokenize(raw, |token| {
        if removed.get() {
            return;
        }
        match token {
            CommentToken(_) => removed.set(true),
            TagToken(tag) if tag.kind == StartTag => {
                let name = &*tag.name;
                if matches!(name, "html" | "head" | "body" | "title") {
                    return;
                }
                if name == "meta"
                    && tag.attrs.len() == 1
                    && &*tag.attrs[0].name.local == "charset"
                {
                    return;
                }
                if !tags.contains(name) {
                    removed.set(true);
                    return;
                }
                for attr in &tag.attrs {
                    let attr_name = &*attr.name.local;
                    let value = &*attr.value;
                    let ok = match attr_name {
                        "title" => true,
                        "class" => value
                            .split_ascii_whitespace()
                            .all(|c| ALLOWED_CLASSES.contains(&c)),
                        "href" if name == "a" => is_allowed_href(value),
                        "src" if name == "img" => is_allowed_img_src(value),
                        other => per_tag.get(name).is_some_and(|set| set.contains(other)),
                    };
                    if !ok {
                        removed.set(true);
                        return;
                    }
                }
            }
            _ => {}
        }
    });
    removed.get()
}

/// Block-level tags: a line break before and after.
const BLOCK_TAGS: &[&str] = &[
    "p", "div", "section", "header", "footer", "h1", "h2", "h3", "h4", "ul", "ol", "dl", "dt",
    "dd", "table", "caption", "thead", "tbody", "tfoot", "tr", "blockquote", "pre", "figure",
    "figcaption", "details", "summary", "hr",
];

/// Plain-text extract of CLEANED brief HTML, plus whether it has a
/// `.k2-need` element. Paragraphs and list items keep their own lines,
/// ordered lists are numbered, a link's URL follows its text in
/// parentheses, an image shows its alt text. Capped at
/// [`MAX_TEXT_BYTES`].
pub fn extract_text(clean_html: &str) -> String {
    extract(clean_html).0
}

fn extract(clean_html: &str) -> (String, bool) {
    let out = RefCell::new(String::new());
    let pre_depth = Cell::new(0usize);
    let has_need = Cell::new(false);
    // One entry per open list: `Some(n)` = ordered, last number used.
    let lists: RefCell<Vec<Option<usize>>> = RefCell::new(Vec::new());
    let hrefs: RefCell<Vec<Option<String>>> = RefCell::new(Vec::new());

    let newline = |out: &mut String| {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
    };
    tokenize(clean_html, |token| {
        let mut o = out.borrow_mut();
        match token {
            CharacterTokens(t) => {
                if pre_depth.get() > 0 {
                    o.push_str(&t);
                } else {
                    for ch in t.chars() {
                        if ch.is_whitespace() {
                            if !o.is_empty() && !o.ends_with([' ', '\n']) {
                                o.push(' ');
                            }
                        } else {
                            o.push(ch);
                        }
                    }
                }
            }
            TagToken(tag) => {
                let name = &*tag.name;
                if tag.kind == StartTag
                    && tag.attrs.iter().any(|a| {
                        &*a.name.local == "class"
                            && a.value.split_ascii_whitespace().any(|c| c == "k2-need")
                    })
                {
                    has_need.set(true);
                }
                match (tag.kind, name) {
                    (StartTag, "br") => o.push('\n'),
                    (StartTag, "li") => {
                        newline(&mut o);
                        let mut ls = lists.borrow_mut();
                        match ls.last_mut() {
                            Some(Some(n)) => {
                                *n += 1;
                                o.push_str(&format!("{n}. "));
                            }
                            _ => o.push_str("- "),
                        }
                    }
                    (EndTag, "li") => newline(&mut o),
                    (StartTag, "ul") => {
                        newline(&mut o);
                        lists.borrow_mut().push(None);
                    }
                    (StartTag, "ol") => {
                        newline(&mut o);
                        lists.borrow_mut().push(Some(0));
                    }
                    (EndTag, "ul" | "ol") => {
                        newline(&mut o);
                        lists.borrow_mut().pop();
                    }
                    (StartTag, "pre") => {
                        newline(&mut o);
                        pre_depth.set(pre_depth.get() + 1);
                    }
                    (EndTag, "pre") => {
                        pre_depth.set(pre_depth.get().saturating_sub(1));
                        newline(&mut o);
                    }
                    (StartTag, "td" | "th") => {
                        if !o.is_empty() && !o.ends_with('\n') {
                            o.push_str(" | ");
                        }
                    }
                    (StartTag, "a") => hrefs.borrow_mut().push(
                        tag.attrs
                            .iter()
                            .find(|a| &*a.name.local == "href")
                            .map(|a| a.value.to_string()),
                    ),
                    (EndTag, "a") => {
                        if let Some(Some(href)) = hrefs.borrow_mut().pop() {
                            let shown = href.strip_prefix("mailto:").unwrap_or(&href);
                            if !o.ends_with(shown) {
                                if !o.is_empty() && !o.ends_with([' ', '\n']) {
                                    o.push(' ');
                                }
                                o.push_str(&format!("({href})"));
                            }
                        }
                    }
                    (StartTag, "img") => {
                        if let Some(alt) = tag
                            .attrs
                            .iter()
                            .find(|a| &*a.name.local == "alt")
                            .map(|a| a.value.trim().to_string())
                            .filter(|a| !a.is_empty())
                        {
                            o.push_str(&format!("[image: {alt}]"));
                        }
                    }
                    (_, n) if BLOCK_TAGS.contains(&n) => newline(&mut o),
                    _ => {}
                }
            }
            _ => {}
        }
    });

    // Tidy: trim each line's end, keep at most one blank line in a row.
    // Leading spaces stay: collapsed whitespace never starts a line, so
    // any that remain came from a `<pre>` (code, logs) and matter.
    let raw = out.into_inner();
    let mut text = String::new();
    let mut blank_run = 0;
    for line in raw.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        text.push_str(line);
        text.push('\n');
    }
    let text = text.trim().to_string();
    (cap_text(text), has_need.get())
}

/// Cut `text` to [`MAX_TEXT_BYTES`] on a char boundary, with a marker.
fn cap_text(text: String) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    const MARK: &str = "\n… (truncated; open the brief for the rest)";
    let mut cut = MAX_TEXT_BYTES - MARK.len();
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut t = text[..cut].to_string();
    t.push_str(MARK);
    t
}

// ── Storage ─────────────────────────────────────────────────────────────

/// One stored brief, as `show?brief=1` returns it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredBrief {
    pub html: String,
    pub text: String,
    pub bytes: i64,
    pub sha256: String,
    pub sanitizer: String,
    pub created_at: i64,
}

/// Insert a brief for `feedback_id` on the caller's connection (inside
/// the caller's transaction; [`crate::feedback::create_with_brief`]).
pub fn insert_with(
    conn: &rusqlite::Connection,
    feedback_id: &str,
    brief: &CleanBrief,
    now: i64,
) -> Result<(), String> {
    #[cfg(any(test, feature = "test-util"))]
    {
        if FAIL_NEXT_INSERT.with(|c| c.replace(false)) {
            return Err("feedback brief insert failed: forced by test".to_string());
        }
    }
    conn.execute(
        "INSERT INTO feedback_briefs (feedback_id, html, text, bytes, sha256, sanitizer, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            feedback_id,
            brief.html,
            brief.text,
            brief.bytes,
            brief.sha256,
            brief.sanitizer,
            now
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("feedback brief insert failed: {e}"))
}

/// The brief for one ticket (FULL id), if it has one.
pub fn get(feedback_id: &str) -> Option<StoredBrief> {
    let db = crate::db::shared();
    let conn = db.lock();
    get_with(&conn, feedback_id)
}

/// [`get`] on a caller-supplied connection.
pub fn get_with(conn: &rusqlite::Connection, feedback_id: &str) -> Option<StoredBrief> {
    conn.query_row(
        "SELECT html, text, bytes, sha256, sanitizer, created_at FROM feedback_briefs \
         WHERE feedback_id = ?1",
        params![feedback_id],
        |r| {
            Ok(StoredBrief {
                html: r.get(0)?,
                text: r.get(1)?,
                bytes: r.get(2)?,
                sha256: r.get(3)?,
                sanitizer: r.get(4)?,
                created_at: r.get(5)?,
            })
        },
    )
    .ok()
}
