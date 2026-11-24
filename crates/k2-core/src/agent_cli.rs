//! Which command agents type now: the one answer every generator of
//! agent-facing text asks (0.46.18).
//!
//! The owner's "New k5 command (Beta)" switch (`k5CliBeta`) puts the Rust
//! `k5` command on the PATH next to `k2`. While it is on **and** the links
//! really run the k5 program (the same check the switch's heal makes,
//! [`crate::cli_stage::current_choice`]), [`name`] is `k5`; otherwise it is
//! the brand's command, [`CLI_NAME`]. A partner build (any brand but the
//! one the Beta is for) always gets its own command, whatever the switch
//! says: a partner build never teaches `k2`, and never `k5`.
//!
//! The text agents read was written with the brand's command in it, typed
//! out or through `cli_name!()` in `concat!` consts, so it can't take a
//! value at run time. [`teach`] is the one place that text follows the
//! answer: it renames the command where it is typed as a command (`k2 msg`,
//! `` `k2` ``) and leaves everything else alone (`.k2/`, `k2-cli`,
//! `k2::host`, `k2.dev`, `cli/k2`, `K2`). With the switch off it returns
//! the same bytes, borrowed.
//!
//! Only text K2 writes is taught. A person's text (a workspace or agent
//! name, a role, a note, a ticket, a file they wrote) goes into a
//! generator through [`Held`] and comes out byte for byte, and the daemon
//! teaches only the `hint` of a refusal, never the rest of an answer.
//!
//! Who asks: the generated AGENTS.md (`workspace::skill_regen`), the live
//! context layers (`workspace::context_layers`), the `k2-cli`,
//! `k2-permissions`, `k2-chat` and `k2-zen` skills, the per-agent skills
//! and wake context (`skills::writer`, `skills::content`), and the
//! daemon's Ask line and refusal hints to agents. A flip regenerates the
//! files (`skill_regen::sync_cli_name_teaching_everywhere`). Text a CLI
//! prints itself is the CLI's: each names itself.

use std::borrow::Cow;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::brand::{BRAND_ID, CLI_NAME};
use k2_client::cli_install::K5_NAME;

/// The brand the k5 Beta is for: its command is the one `k5` replaces.
/// Every other brand's build keeps its own command.
const BETA_BRAND_ID: &str = "k2";

/// How long a "the links run k5" answer is kept: it reads the program's
/// bytes for its version mark, and it only changes with a flip (which
/// [`forget`]s it) or an update.
const RUNS_FRESH: Duration = Duration::from_secs(60);

static RUNS: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

/// The command agents type now. See the module docs.
pub fn name() -> &'static str {
    name_for(BRAND_ID, CLI_NAME, crate::app_settings::k5_cli_beta, k5_runs)
}

/// [`name`] with its inputs passed in: this build's brand id and command,
/// the owner's switch, and whether the links run the k5 program. `runs` is
/// asked only while the switch is on.
pub fn name_for(
    brand_id: &str,
    brand_cli: &'static str,
    switch_on: impl FnOnce() -> bool,
    runs: impl FnOnce() -> bool,
) -> &'static str {
    if brand_id != BETA_BRAND_ID || brand_cli == K5_NAME {
        return brand_cli;
    }
    if switch_on() && runs() {
        K5_NAME
    } else {
        brand_cli
    }
}

/// True when the commands' links run the k5 program now: what the
/// switch's heal links (`cli_stage::current_choice`), kept for a minute.
pub fn k5_runs() -> bool {
    #[cfg(any(test, feature = "test-util"))]
    if let Some(v) = TEST_RUNS.with(|c| c.get()) {
        return v;
    }
    let mut kept = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *kept {
        if at.elapsed() < RUNS_FRESH {
            return v;
        }
    }
    let v = crate::cli_stage::current_choice().binary().is_some();
    *kept = Some((Instant::now(), v));
    v
}

/// Drop the kept answer: the switch flipped or the links were re-pointed.
pub fn forget() {
    *RUNS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    static TEST_RUNS: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Tests: on this thread, answer "the links run k5" with `runs` instead of
/// reading a program (a test build has none). Put back on drop.
#[cfg(any(test, feature = "test-util"))]
pub struct K5RunsForTest(Option<bool>);

#[cfg(any(test, feature = "test-util"))]
impl K5RunsForTest {
    pub fn set(runs: bool) -> Self {
        Self(TEST_RUNS.with(|c| c.replace(Some(runs))))
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Drop for K5RunsForTest {
    fn drop(&mut self) {
        TEST_RUNS.with(|c| c.set(self.0));
    }
}

/// `text` as agents should read it now: [`teach_as`] with [`name`].
pub fn teach(text: &str) -> Cow<'_, str> {
    teach_as(text, name())
}

/// `text` with the brand's command, where it is typed as a command, named
/// `cli` instead. `cli` equal to the brand's command: the same bytes.
pub fn teach_as<'a>(text: &'a str, cli: &str) -> Cow<'a, str> {
    rename_command(text, CLI_NAME, cli)
}

/// The one line a document gets while agents use another command than the
/// brand's: "`k2` still works and does the same." `None` for the brand's
/// own command.
pub fn still_works_line(cli: &str) -> Option<String> {
    (cli != CLI_NAME).then(|| format!("`{CLI_NAME}` still works and does the same."))
}

/// [`teach_as`] for a document that opens with a heading and a first
/// paragraph (the Tooling section, the k2-cli skill): the same, plus
/// [`still_works_line`] as its own paragraph after the first one.
pub fn teach_doc_as<'a>(text: &'a str, cli: &str) -> Cow<'a, str> {
    let taught = teach_as(text, cli);
    let Some(line) = still_works_line(cli) else {
        return taught;
    };
    let mut out = taught.into_owned();
    // After the heading's blank line comes the first paragraph; the line
    // goes after the blank line that ends it (or at the end).
    let at = out.match_indices("\n\n").nth(1).map(|(i, _)| i + 2).unwrap_or(out.len());
    let sep = if out[..at].ends_with('\n') { "" } else { "\n\n" };
    out.insert_str(at, &format!("{sep}{line}\n\n"));
    Cow::Owned(out)
}

/// Opens and closes a [`Held`] stand-in. Private-use characters: never in
/// the text K2 writes, and neither one is a word character, so a stand-in
/// next to a command changes nothing about where [`rename_command`] looks.
const HOLD_OPEN: char = '\u{E000}';
const HOLD_CLOSE: char = '\u{E001}';

/// A person's text on its way into text agents are taught: a workspace,
/// agent or skill name, a note, a role, a file a person wrote. A generator
/// puts each such value in through [`Held::hold`], so [`Held::teach_as`]
/// renames only the text K2 wrote and puts every held value back as it
/// was, byte for byte. [`Held::pass_through`] holds nothing: the same
/// generator then gives its text as written.
pub struct Held {
    values: Option<std::cell::RefCell<Vec<String>>>,
}

impl Held {
    /// Holds each value until [`Held::teach_as`] (or [`Held::put_back`]).
    pub fn new() -> Self {
        Self { values: Some(std::cell::RefCell::new(Vec::new())) }
    }

    /// Holds nothing: [`Held::hold`] gives the value itself.
    pub fn pass_through() -> Self {
        Self { values: None }
    }

    /// `value` as it goes into the text: a stand-in teaching never reads,
    /// or the value itself for [`Held::pass_through`].
    pub fn hold<'a>(&self, value: &'a str) -> Cow<'a, str> {
        let Some(values) = &self.values else {
            return Cow::Borrowed(value);
        };
        let mut values = values.borrow_mut();
        values.push(value.to_string());
        Cow::Owned(format!("{HOLD_OPEN}{}{HOLD_CLOSE}", values.len() - 1))
    }

    /// `text` taught as `cli` ([`teach_as`]), then every held value put
    /// back as it was.
    pub fn teach_as(&self, text: &str, cli: &str) -> String {
        self.put_back(&teach_as(text, cli))
    }

    /// [`Held::teach_as`] with [`name`].
    pub fn teach(&self, text: &str) -> String {
        self.teach_as(text, name())
    }

    /// `text` with every held value put back as it was, nothing taught.
    pub fn put_back(&self, text: &str) -> String {
        let Some(values) = &self.values else {
            return text.to_string();
        };
        let values = values.borrow();
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find(HOLD_OPEN) {
            out.push_str(&rest[..at]);
            let after = &rest[at + HOLD_OPEN.len_utf8()..];
            let held = after
                .find(HOLD_CLOSE)
                .and_then(|end| after[..end].parse::<usize>().ok().map(|i| (i, end)))
                .and_then(|(i, end)| values.get(i).map(|v| (v, end)));
            match held {
                Some((v, end)) => {
                    out.push_str(v);
                    rest = &after[end + HOLD_CLOSE.len_utf8()..];
                }
                None => {
                    out.push(HOLD_OPEN);
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }
}

impl Default for Held {
    fn default() -> Self {
        Self::new()
    }
}

/// `text` with `from`, where it is typed as a command, renamed `to`. A
/// command is `from` with no word, path or host character just before it
/// (letters, digits, `_ . / - ~ $ : @ \ =`), followed by a backtick (`` `k2` ``)
/// or by one space and a lowercase word, a flag or a placeholder
/// (`k2 msg`, `k2 --help`, `k2 <noun>`). The same bytes, borrowed, when
/// nothing changes.
pub fn rename_command<'a>(text: &'a str, from: &str, to: &str) -> Cow<'a, str> {
    if from.is_empty() || from == to || !text.contains(from) {
        return Cow::Borrowed(text);
    }
    let mut out = String::new();
    let mut last = 0;
    for (i, _) in text.match_indices(from) {
        if i < last || !is_command_at(text, i, from.len()) {
            continue;
        }
        if out.is_empty() {
            out.reserve(text.len() + 16);
        }
        out.push_str(&text[last..i]);
        out.push_str(to);
        last = i + from.len();
    }
    if last == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[last..]);
    Cow::Owned(out)
}

fn is_command_at(text: &str, i: usize, len: usize) -> bool {
    let joined = |c: char| c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '-' | '~' | '$' | ':' | '@' | '\\' | '=');
    if text[..i].chars().next_back().is_some_and(joined) {
        return false;
    }
    let mut after = text[i + len..].chars();
    match after.next() {
        Some('`') => true,
        Some(' ') => after.next().is_some_and(|c| c.is_ascii_lowercase() || matches!(c, '-' | '<' | '[')),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The k5 command's name as these tests spell it: this build's next
    /// name, never typed out.
    const NEXT: &str = K5_NAME;

    fn cmd(word: &str) -> String {
        format!("{CLI_NAME} {word}")
    }

    #[test]
    fn the_beta_brand_follows_the_switch_and_the_program() {
        let k2 = CLI_NAME;
        if BRAND_ID != BETA_BRAND_ID {
            return; // a partner build: the partner test below covers it
        }
        assert_eq!(name_for(BRAND_ID, k2, || false, || panic!("not asked while off")), k2);
        assert_eq!(name_for(BRAND_ID, k2, || true, || false), k2, "on, but the links run the script");
        assert_eq!(name_for(BRAND_ID, k2, || true, || true), NEXT);
    }

    #[test]
    fn a_partner_brand_keeps_its_own_command_whatever_the_switch_says() {
        for (id, cli) in [("xy", "xy"), ("acme", "acme")] {
            for on in [false, true] {
                for runs in [false, true] {
                    assert_eq!(name_for(id, cli, || on, || runs), cli, "{id} on={on} runs={runs}");
                }
            }
        }
        // The k5 brand's command is k5 already.
        assert_eq!(name_for(NEXT, NEXT, || false, || false), NEXT);
    }

    #[test]
    fn the_brands_own_command_is_the_same_bytes() {
        let text = format!("Run `{}` or `{CLI_NAME}`.\n", cmd("msg x"));
        assert!(matches!(teach_as(&text, CLI_NAME), Cow::Borrowed(t) if t == text));
        assert!(matches!(teach_doc_as(&text, CLI_NAME), Cow::Borrowed(t) if t == text));
        assert_eq!(still_works_line(CLI_NAME), None);
    }

    #[test]
    fn renames_commands_and_nothing_else() {
        let n = CLI_NAME;
        let renamed = [
            (format!("`{n} msg <ws>`"), format!("`{NEXT} msg <ws>`")),
            (format!("{n} heartbeat list"), format!("{NEXT} heartbeat list")),
            (format!("the `{n}` CLI"), format!("the `{NEXT}` CLI")),
            (format!("({n} app list)"), format!("({NEXT} app list)")),
            (format!("Ask with: {n} permission request a.b"), format!("Ask with: {NEXT} permission request a.b")),
            (format!("    {n} --help"), format!("    {NEXT} --help")),
            (format!("{n} <noun>"), format!("{NEXT} <noun>")),
            (format!("\"hint\":\"{n} study x\""), format!("\"hint\":\"{NEXT} study x\"")),
        ];
        for (from, want) in renamed {
            assert_eq!(rename_command(&from, n, NEXT), want, "{from}");
        }
        let kept = [
            format!("`.{n}/skills/{n}-cli/SKILL.md`"),
            format!("**{n}-cli**"),
            format!("agent::host.{n}.dev"),
            format!("{n}::host"),
            format!("cli/{n} script"),
            format!("~/.{n} folder"),
            format!("{n}so app"),
            format!("{n}skn_ pass"),
            n.to_string(),
            format!("{n} Space"),
            format!("{n}."),
            format!("K2_{n} x"),
            format!("${n} x"),
            format!("x{n} msg"),
            // The home folder, the skill, the crate path, the host, the
            // script's path and the product, each as a person reads it.
            format!(".{n}/"),
            format!("{n}-cli"),
            format!("{n}::host::x"),
            format!("https://{n}.dev/docs"),
            format!("{n}.dev"),
            format!("cli/{n}"),
            format!("\"cli/{n} msg\""),
            "K2 msg".to_string(),
            "the K2 app".to_string(),
        ];
        for text in kept {
            assert!(matches!(rename_command(&text, n, NEXT), Cow::Borrowed(_)), "{text}");
            assert!(matches!(teach_as(&text, NEXT), Cow::Borrowed(_)), "{text}");
        }
    }

    /// A held value comes back byte for byte, whatever it says: a person's
    /// text that types the command, one next to a command K2 wrote, one
    /// with the stand-in's own characters in it. K2's text around it is
    /// taught.
    #[test]
    fn a_held_value_comes_back_as_written_and_only_k2s_text_is_taught() {
        let said = cmd("msg acme \"hi\"");
        let odd = format!("{HOLD_OPEN}0{HOLD_CLOSE} {} {HOLD_OPEN}", cmd("read x"));
        let held = Held::new();
        let text = format!(
            "Run `{}`. Note: {}. Then `{}`{}\n",
            cmd("inbox"),
            held.hold(&said),
            cmd("msg"),
            held.hold(&odd)
        );
        let taught = held.teach_as(&text, NEXT);
        assert_eq!(taught, format!("Run `{NEXT} inbox`. Note: {said}. Then `{NEXT} msg`{odd}\n"));
        assert_eq!(held.put_back(&text), format!("Run `{}`. Note: {said}. Then `{}`{odd}\n", cmd("inbox"), cmd("msg")));
        // The brand's own command: the text as written, the values back.
        assert_eq!(held.teach_as(&text, CLI_NAME), held.put_back(&text));
        // Pass-through holds nothing: the value is the text itself.
        let none = Held::pass_through();
        assert!(matches!(none.hold(&said), Cow::Borrowed(v) if v == said));
        assert_eq!(none.teach_as(&format!("{said}\n"), NEXT), format!("{NEXT} msg acme \"hi\"\n"));
    }

    #[test]
    fn a_document_gets_the_still_works_line_after_its_first_paragraph() {
        let doc = format!("## Tooling\n\nYou have the `{CLI_NAME}` CLI.\n\nReminders: `{}`.\n", cmd("remind"));
        let taught = teach_doc_as(&doc, NEXT);
        let want = format!(
            "## Tooling\n\nYou have the `{NEXT}` CLI.\n\n`{CLI_NAME}` still works and does the same.\n\nReminders: `{NEXT} remind`.\n"
        );
        assert_eq!(taught, want);
        assert_eq!(taught.matches("still works").count(), 1);
    }

    #[test]
    fn the_test_override_is_this_threads_and_put_back() {
        {
            let _on = K5RunsForTest::set(true);
            assert!(k5_runs());
            {
                let _off = K5RunsForTest::set(false);
                assert!(!k5_runs());
            }
            assert!(k5_runs(), "the inner guard put the outer value back");
            std::thread::spawn(|| assert_eq!(TEST_RUNS.with(|c| c.get()), None, "another thread is not affected"))
                .join()
                .expect("thread");
        }
        assert_eq!(TEST_RUNS.with(|c| c.get()), None);
    }
}
