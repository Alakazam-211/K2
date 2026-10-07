//! Fixture replay (§11): synthetic, scrubbed hook JSONL driven through the
//! owner check and the pure row with a fake clock (T-S2a, T-S2c, T-S2d,
//! T-S2j, T-S2k).
//!
//! A fixture is `fixtures/<cli>-<version>/<scenario>.jsonl` plus
//! `<scenario>.meta.json`. Each line is `{t_ms, kind, payload}`:
//! - `hook` (with `pid`): a raw hook body, posted by that process;
//! - `keys`: `"esc"` | `"ctrl-c"`;
//! - `title`: `"working"` | `"idle"` | `"permission"`;
//! - `transcript`: `"interrupt"` | `"turn_end"` (the S3 seam);
//! - `exit`: `"owner"` | `"pty"`;
//! - `tick`: nothing but time passing;
//! - `expect`: assertions on the row at that time (`display`, `reason`,
//!   `lead`, `outcome`, child counts, `lock` = the last status written or
//!   `"untouched"`, `foreign` = owner-check rejections so far,
//!   `turnEnded` = the last turn end's reason).
//!
//! Harness rule (Orca's `/compact` lesson): a hook event that is not in
//! the meta's `registeredEvents` fails the test.

use std::collections::HashMap;

use serde_json::Value;

use crate::agent_hooks::envelope::{self, HookHeaders, HookSource};
use crate::agent_hooks::owner::{
    self, OwnerCheckMode, PaneFacts, PaneOwner, ProcInfo, ProcessTable, Sender, Verdict,
};

use super::{Evidence, KeyInput, Row, RowFacts, TitleSignal};

struct Procs(HashMap<i32, ProcInfo>);

impl ProcessTable for Procs {
    fn info(&self, pid: i32) -> Option<ProcInfo> {
        self.0.get(&pid).copied()
    }
}

pub(crate) struct Fixture {
    pub name: &'static str,
    pub jsonl: &'static str,
    pub meta: &'static str,
}

macro_rules! fixture {
    ($dir:literal, $name:literal) => {
        Fixture {
            name: $name,
            jsonl: include_str!(concat!("fixtures/", $dir, "/", $name, ".jsonl")),
            meta: include_str!(concat!("fixtures/", $dir, "/", $name, ".meta.json")),
        }
    };
}

/// Every Claude 2.1.292 scenario (synthetic; see each meta's `notes`).
pub(crate) fn claude_fixtures() -> Vec<Fixture> {
    vec![
        fixture!("claude-2.1.292", "plain-turn"),
        fixture!("claude-2.1.292", "tool-turn"),
        fixture!("claude-2.1.292", "fg-subagent"),
        fixture!("claude-2.1.292", "bg-subagent"),
        fixture!("claude-2.1.292", "bg-shell"),
        fixture!("claude-2.1.292", "owed-lease"),
        fixture!("claude-2.1.292", "ctrl-c-no-stop"),
        fixture!("claude-2.1.292", "esc-mid-tool"),
        fixture!("claude-2.1.292", "esc-dismiss-permission"),
        fixture!("claude-2.1.292", "permission-denied"),
        fixture!("claude-2.1.292", "permission-late-pretool"),
        fixture!("claude-2.1.292", "nested-claude-p"),
        fixture!("claude-2.1.292", "compact-manual"),
        fixture!("claude-2.1.292", "clear"),
        fixture!("claude-2.1.292", "missed-stop-idle-prompt"),
        fixture!("claude-2.1.292", "stop-failure"),
        fixture!("claude-2.1.292", "late-tool-after-stop"),
        fixture!("claude-2.1.292", "crons"),
        fixture!("claude-2.1.292", "subagent-waiting"),
        fixture!("claude-2.1.292", "owner-exit"),
        fixture!("claude-2.1.292", "pty-exit"),
        fixture!("claude-2.1.292", "ctrl-c-idle-with-subagent"),
        fixture!("claude-2.1.292", "inventory-omission"),
    ]
}

/// The outcome of a replay, for tests that check more than the inline
/// `expect` lines.
pub(crate) struct Replayed {
    pub row: Row,
    pub lock: Option<&'static str>,
    pub compat: Vec<&'static str>,
}

pub(crate) fn replay(f: &Fixture) -> Replayed {
    let meta: Value = serde_json::from_str(f.meta).unwrap_or_else(|e| panic!("{}: meta: {e}", f.name));
    let registered: Vec<&str> = meta["registeredEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("{}: meta.registeredEvents", f.name))
        .iter()
        .map(|v| v.as_str().expect("event name"))
        .collect();
    let child_pid = meta["childPid"].as_i64().unwrap_or_else(|| panic!("{}: meta.childPid", f.name)) as i32;
    let procs = Procs(
        meta["procs"]
            .as_array()
            .unwrap_or_else(|| panic!("{}: meta.procs", f.name))
            .iter()
            .map(|p| {
                let pid = p[0].as_i64().expect("pid") as i32;
                (pid, ProcInfo { pid, ppid: p[1].as_i64().expect("ppid") as i32, start_time: p[2].as_u64().expect("start") })
            })
            .collect(),
    );
    let facts = PaneFacts { child_pid: Some(child_pid), known_conversation_id: None };
    let mut owner_state = PaneOwner::default();
    let mut row = Row::new(
        RowFacts {
            session_id: "11111111-1111-4111-8111-111111111111".into(),
            agent_name: "tab-fixture".into(),
            harness: meta["cli"].as_str().expect("cli").to_string(),
            ..Default::default()
        },
        0,
    );
    let mut lock: Option<&'static str> = None;
    let mut compat = Vec::new();
    let mut foreign = 0u64;
    let mut last_turn_end: Option<&'static str> = None;

    for (n, line) in f.jsonl.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let at = format!("{}:{}", f.name, n + 1);
        let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{at}: {e}"));
        let t = v["t_ms"].as_i64().unwrap_or_else(|| panic!("{at}: t_ms"));
        let kind = v["kind"].as_str().unwrap_or_else(|| panic!("{at}: kind"));
        let payload = &v["payload"];
        let mut changes = Vec::new();
        match kind {
            "hook" => {
                let event = payload["hook_event_name"].as_str().unwrap_or_else(|| panic!("{at}: hook_event_name"));
                assert!(registered.contains(&event), "{at}: {event} is not a registered event");
                let pid = v["pid"].as_i64().unwrap_or_else(|| panic!("{at}: pid")) as i32;
                let headers = HookHeaders {
                    pane: row.facts.session_id.clone(),
                    agent_pid: Some(pid),
                    source: HookSource::Claude,
                    hook_version: Some(2),
                    cli_version: meta["version"].as_str().map(str::to_string),
                    truncated: false,
                    event_hint: None,
                };
                let env = envelope::parse(&headers, payload.to_string().as_bytes()).unwrap_or_else(|e| panic!("{at}: {e:?}"));
                let out = owner::check(&mut owner_state, Some(&facts), &Sender::of(&env), &procs, OwnerCheckMode::Ancestry, t);
                if out.released_previous {
                    changes.push(row.apply(Evidence::OwnerReleased, t));
                }
                match out.verdict {
                    Verdict::Owner => changes.push(row.apply(Evidence::Hook(&env), t)),
                    _ => foreign += 1,
                }
            }
            "keys" => {
                let key = match payload.as_str() {
                    Some("esc") => KeyInput::Esc,
                    Some("ctrl-c") => KeyInput::CtrlC,
                    other => panic!("{at}: keys {other:?}"),
                };
                changes.push(row.apply(Evidence::Key(key), t));
            }
            "title" => {
                let s = match payload.as_str() {
                    Some("working") => TitleSignal::Working,
                    Some("idle") => TitleSignal::Idle,
                    Some("permission") => TitleSignal::Permission,
                    other => panic!("{at}: title {other:?}"),
                };
                changes.push(row.apply(Evidence::Title(s), t));
            }
            "transcript" => match payload.as_str() {
                Some("interrupt") => changes.push(row.apply(Evidence::TranscriptInterrupt, t)),
                Some("turn_end") => changes.push(row.apply(Evidence::TranscriptTurnEnd, t)),
                other => panic!("{at}: transcript {other:?}"),
            },
            "exit" => match payload.as_str() {
                Some("owner") => changes.push(row.apply(Evidence::OwnerReleased, t)),
                Some("pty") => changes.push(row.apply(Evidence::PtyExited, t)),
                other => panic!("{at}: exit {other:?}"),
            },
            "tick" => changes.push(row.tick(t)),
            "expect" => {
                changes.push(row.tick(t));
                for c in &changes {
                    note(c, &mut lock, &mut compat, &mut last_turn_end);
                }
                check(&at, &row, payload, lock, foreign, last_turn_end);
                continue;
            }
            other => panic!("{at}: unknown kind {other}"),
        }
        for c in &changes {
            note(c, &mut lock, &mut compat, &mut last_turn_end);
        }
    }
    Replayed { row, lock, compat }
}

fn note(
    c: &super::Change,
    lock: &mut Option<&'static str>,
    compat: &mut Vec<&'static str>,
    last_turn_end: &mut Option<&'static str>,
) {
    if let Some(s) = c.status_write {
        *lock = Some(s);
    }
    if let Some(w) = c.compat {
        compat.push(w);
    }
    if let Some(t) = c.turn_ended {
        *last_turn_end = Some(t.reason.as_str());
    }
}

fn check(at: &str, row: &Row, want: &Value, lock: Option<&str>, foreign: u64, turn_end: Option<&str>) {
    let obj = want.as_object().unwrap_or_else(|| panic!("{at}: expect payload must be an object"));
    let counts = row.counts();
    for (key, val) in obj {
        let got: Value = match key.as_str() {
            "display" => row.display.as_str().into(),
            "reason" => row.reason.as_str().into(),
            "lead" => row.lead.state.as_str().into(),
            "outcome" => row.lead.outcome.as_str().into(),
            "confirmed" => row.confirmed.into(),
            "subagents" => counts.subagents.into(),
            "shells" => counts.shells.into(),
            "monitors" => counts.monitors.into(),
            "crons" => counts.crons.into(),
            "unknown" => counts.unknown.into(),
            "owed" => counts.owed.into(),
            "waiting" => counts.waiting.into(),
            "lock" => lock.unwrap_or("untouched").into(),
            "foreign" => foreign.into(),
            "turnEnded" => turn_end.map_or(Value::Null, Value::from),
            other => panic!("{at}: unknown expect key {other}"),
        };
        assert_eq!(&got, val, "{at}: {key}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::row::{Display, LeadState, Reason, STALE_AFTER_MS};
    use crate::agent_hooks::install::CLAUDE_FULL_SET;

    /// T-S2a: every §5.4 row is replayed by at least one fixture, and
    /// every fixture's inline expectations hold.
    #[test]
    fn every_claude_fixture_replays_to_its_expectations() {
        crate::test_isolation::assert_no_prod_env();
        for f in claude_fixtures() {
            replay(&f);
        }
    }

    /// The fixtures name each end of §5.4 they cover; together they must
    /// cover every end reason the contract table lists.
    #[test]
    fn fixtures_cover_every_explicit_end() {
        let all: String = claude_fixtures().iter().map(|f| f.jsonl).collect();
        for reason in [
            Reason::TurnDone,
            Reason::TurnFailed,
            Reason::Interrupted,
            Reason::Compacted,
            Reason::SessionBoundary,
            Reason::IdlePrompt,
            Reason::PromptDismissed,
            Reason::ChildDone,
            Reason::OwedExpired,
            Reason::CronsCleared,
            Reason::AgentExited,
            Reason::PtyExited,
        ] {
            let needle = format!("\"reason\":\"{}\"", reason.as_str());
            assert!(all.contains(&needle), "no fixture expects reason {}", reason.as_str());
        }
    }

    /// §11: the repo is public. Fixtures carry no real home paths, no
    /// tokens, and every meta lists the 2.1.292 full set.
    #[test]
    fn fixtures_are_scrubbed_and_registered() {
        let secret_marks = [
            "/Users/", "k2skn_", "k2sk_", "k2rs_", "ghp_", "gho_", "github_pat_", "xoxb-",
            "xoxp-", "xoxa-", "AKIA", "Bearer ", "-----BEGIN", "@",
        ];
        for f in claude_fixtures() {
            for text in [f.jsonl, f.meta] {
                for mark in secret_marks {
                    assert!(!text.contains(mark), "{}: fixture contains {mark:?}", f.name);
                }
                // `sk-` as a key prefix, not inside a word ("task-id").
                let bytes = text.as_bytes();
                for (i, _) in text.match_indices("sk-") {
                    let word_char = i > 0 && bytes[i - 1].is_ascii_alphanumeric();
                    assert!(word_char, "{}: fixture contains an sk- key", f.name);
                }
            }
            let meta: Value = serde_json::from_str(f.meta).expect("meta");
            assert_eq!(meta["synthetic"], true, "{}: synthetic fixtures say so", f.name);
            let events: Vec<&str> = meta["registeredEvents"].as_array().expect("events").iter().map(|v| v.as_str().expect("s")).collect();
            assert_eq!(events, CLAUDE_FULL_SET, "{}: registeredEvents", f.name);
        }
    }

    /// The harness itself refuses an unregistered event.
    #[test]
    #[should_panic(expected = "is not a registered event")]
    fn an_unregistered_event_fails_the_replay() {
        let f = Fixture {
            name: "unregistered",
            jsonl: r#"{"t_ms":0,"kind":"hook","pid":100,"payload":{"hook_event_name":"PreCompact"}}"#,
            meta: include_str!("fixtures/claude-2.1.292/plain-turn.meta.json"),
        };
        replay(&f);
    }

    /// T-S2c: `Stop`, then an async `PostToolUse` with the same
    /// `promptId` stays idle; with a new `promptId` it is working.
    #[test]
    fn late_tool_events_respect_the_turn_latch() {
        let f = claude_fixtures().into_iter().find(|f| f.name == "late-tool-after-stop").expect("fixture");
        let r = replay(&f);
        assert_eq!(r.row.lead.state, LeadState::Working);
    }

    /// T-S2e: a snapshot read (a `to_json`) at 31 m doesn't move
    /// `evidenceAt`; only evidence does.
    #[test]
    fn decay_is_read_only_and_new_evidence_revives() {
        let f = claude_fixtures().into_iter().find(|f| f.name == "tool-turn").expect("fixture");
        let mut r = replay(&f).row;
        // Put the row back to working with one hook, then go silent.
        let env = envelope::parse(
            &HookHeaders {
                pane: r.facts.session_id.clone(),
                agent_pid: Some(100),
                source: HookSource::Claude,
                hook_version: Some(2),
                cli_version: None,
                truncated: false,
                event_hint: None,
            },
            br#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p-9"}"#,
        )
        .expect("parse");
        let t0 = 100_000;
        r.apply(Evidence::Hook(&env), t0);
        assert_eq!(r.display, Display::Working);
        assert_eq!(r.next_deadline(), Some(t0 + STALE_AFTER_MS));
        r.tick(t0 + STALE_AFTER_MS - 1_000);
        assert_eq!(r.display, Display::Working, "29 m 59 s is still working");
        let c = r.tick(t0 + STALE_AFTER_MS);
        assert_eq!(r.display, Display::Unverifiable);
        assert_eq!(r.reason, Reason::StaleNoEvidence);
        assert_eq!(c.status_write, Some("sleeping"), "decay releases the lock");
        let snap = r.to_json();
        r.tick(t0 + STALE_AFTER_MS + 60_000);
        assert_eq!(snap["evidenceAt"], t0);
        assert_eq!(r.to_json()["evidenceAt"], t0, "a read never restamps evidenceAt");
        assert_eq!(r.to_json()["staleSince"], t0 + STALE_AFTER_MS);
        // New evidence returns it to the folded state at once.
        let post = envelope::parse(
            &HookHeaders {
                pane: r.facts.session_id.clone(),
                agent_pid: Some(100),
                source: HookSource::Claude,
                hook_version: Some(2),
                cli_version: None,
                truncated: false,
                event_hint: None,
            },
            br#"{"hook_event_name":"PostToolUse","prompt_id":"p-9","tool_name":"Bash","tool_use_id":"t"}"#,
        )
        .expect("parse");
        let c = r.apply(Evidence::Hook(&post), t0 + STALE_AFTER_MS + 120_000);
        assert_eq!(r.display, Display::Working);
        assert_eq!(c.status_write, Some("running"));
    }

    /// DA30: a row registered after a restart is unconfirmed and writes
    /// nothing, whatever time passes, until its first evidence.
    #[test]
    fn an_unconfirmed_row_never_touches_the_lock() {
        let mut r = Row::new(RowFacts { session_id: "s".into(), harness: "claude".into(), ..Default::default() }, 0);
        for t in [1, 60_000, STALE_AFTER_MS * 3] {
            let c = r.tick(t);
            assert_eq!(c.status_write, None);
            assert_eq!(c.compat, None);
            assert_eq!(r.reason, Reason::Unconfirmed);
        }
        // A key on an unconfirmed row does nothing either.
        let c = r.apply(Evidence::Key(KeyInput::CtrlC), 10);
        assert_eq!(c.status_write, None);
        assert_eq!(r.display, Display::Idle);
    }

    /// DA28: once a hook has spoken, the title is ignored.
    #[test]
    fn title_counts_only_without_hook_evidence() {
        let mut r = Row::new(RowFacts { harness: "codex".into(), ..Default::default() }, 0);
        r.apply(Evidence::Title(TitleSignal::Working), 1);
        assert_eq!(r.display, Display::Working);
        assert_eq!(r.evidence_source, Some(crate::activity::EvidenceSource::Title));
        let c = r.apply(Evidence::Title(TitleSignal::Idle), 2);
        assert_eq!(r.display, Display::Idle);
        assert_eq!(c.status_write, Some("sleeping"));

        let f = claude_fixtures().into_iter().find(|f| f.name == "plain-turn").expect("fixture");
        let mut hooked = replay(&f).row;
        hooked.apply(Evidence::Title(TitleSignal::Working), 1_000_000);
        assert_eq!(hooked.display, Display::Idle, "a hooked row ignores the title");
    }

    /// RL5: the compat words come from the display, and the first word is
    /// never a bare `stop`.
    #[test]
    fn compat_words_follow_the_display() {
        let f = claude_fixtures().into_iter().find(|f| f.name == "bg-subagent").expect("fixture");
        let r = replay(&f);
        assert_eq!(r.compat, vec!["start", "stop"], "no stop while the subagent runs");
        assert_eq!(r.lock, Some("sleeping"));
    }
}
