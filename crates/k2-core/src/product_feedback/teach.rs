//! What agents read about reporting problems to Alakazam Labs (the agent
//! opt-in, 0.46.17, slice 2): one "Reporting … problems" section in every
//! workspace's AGENTS.md, composed **only while a person allowed agents to
//! report** (`agents` on, after the migration rule in [`super::settings`])
//! and sending is on. It says exactly what is allowed: CLI problems; the
//! rest of the platform only if `agentPlatform` is on; and which parts may
//! be attached. Off (not asked, a "no", or sending off) = no section, so
//! agents read nothing about it.
//!
//! `workspace::skill_regen::compose_agents_md` appends it; the settings
//! route recomposes every workspace when it changes
//! (`skill_regen::sync_product_feedback_teaching_everywhere`).

use super::settings::{Settings, ATTACH_PARTS};
use crate::{cli_name, product_name};

/// The section's heading.
pub const HEADING: &str = concat!("## Reporting ", product_name!(), " problems");

/// The AGENTS.md section for these switches, or `None` while agents may
/// not report.
pub fn agents_md_section(s: &Settings) -> Option<String> {
    let policy = s.agent_policy();
    if !s.enabled || !policy.cli {
        return None;
    }
    let (p, c) = (product_name!(), cli_name!());
    let platform = if policy.platform {
        format!(
            "- **Anything else about {p}.** Also allowed: `--scope platform` (no `--command`) for anything else in {p} that gets in your way or looks wrong.\n"
        )
    } else {
        format!(
            "- **Anything else about {p}.** Not on this server: report only CLI problems. A `--scope platform` report is refused (`scope_off`).\n"
        )
    };
    let attach = if policy.attach.is_empty() {
        "- **What you may attach.** Nothing: send text only (`--diagnostics` is refused).\n".to_string()
    } else {
        let labels: Vec<String> = ATTACH_PARTS
            .iter()
            .filter(|(id, _)| policy.attach.iter().any(|a| a == id))
            .map(|(_, label)| lower_first(label))
            .collect();
        format!(
            "- **What you may attach.** With `--diagnostics --yes`, only these details (the versions always go): {}. Nothing else.\n",
            labels.join("; ")
        )
    };
    Some(format!(
        "{HEADING}\n\
\n\
A person on this server lets agents report problems with {p} to Alakazam Labs, who make {p}, with \
`{c} send-feedback`. Reports are private (only Alakazam Labs reads them, never the public), and the \
person sees each one in the Sent tab. It isn't `{c} tickets ask`, which asks your person.\n\
\n\
- **{p} CLI problems.** When a `{c}` command fails or misleads you (a wrong or unclear error, help \
that doesn't match what the command does, or a command that should exist and doesn't), send one \
report: `{c} send-feedback --scope cli --command \"<the command you ran>\" \"<what you expected, and \
what happened>\"`.\n\
{platform}\
{attach}\
- **In your own words.** Say what you ran, what you expected and what happened. Never paste anyone's \
content: no files, chat or mail, terminal output, names, keys or tokens.\n\
- **Once per problem.** Check `{c} send-feedback list` for one you already sent. At most 1 a minute \
and 3 a day from this workspace.\n"
    ))
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(cli: bool, platform: bool, attach: &[&str]) -> Settings {
        let mut s = Settings::default();
        let attach: Vec<String> = attach.iter().map(|x| x.to_string()).collect();
        s.answer(cli, platform, &attach, "owner", 1);
        s
    }

    #[test]
    fn off_unless_a_person_allowed_it() {
        assert_eq!(agents_md_section(&Settings::default()), None, "a new install");
        let old = Settings { agents: true, agent_diagnostics: true, ..Settings::default() };
        assert_eq!(agents_md_section(&old), None, "an old file: not asked = off");
        assert_eq!(agents_md_section(&answered(false, false, &[])), None, "a no");
        let mut off = answered(true, true, &["log-tail"]);
        off.enabled = false;
        assert_eq!(agents_md_section(&off), None, "sending off");
    }

    #[test]
    fn says_exactly_what_is_allowed() {
        let cli = agents_md_section(&answered(true, false, &[])).expect("on");
        assert!(cli.starts_with(HEADING), "{cli}");
        assert!(cli.contains("--scope cli --command"), "{cli}");
        assert!(cli.contains("Not on this server: report only CLI problems"), "{cli}");
        assert!(cli.contains("Nothing: send text only"), "{cli}");
        assert!(!cli.contains("Also allowed: `--scope platform`"), "{cli}");

        let all = agents_md_section(&answered(true, true, &["log-tail", "computer"])).expect("on");
        assert!(all.contains("Also allowed: `--scope platform`"), "{all}");
        assert!(all.contains("computer size and free space; the end of"), "parts in order: {all}");
        assert!(!all.contains("installed"), "only the allowed parts: {all}");
        assert!(!all.contains("Nothing: send text only"), "{all}");
    }

    /// The regen: the section appears in the written `.k2/AGENTS.md` once a
    /// person says yes, changes with the switches, and goes on a "no";
    /// off is the bytes without it.
    #[test]
    fn agents_md_regen_follows_the_switches() {
        let home = crate::test_env::TempHome::new();
        let ws = home.path().join("work/reporting-ws");
        std::fs::create_dir_all(&ws).unwrap();
        let path = ws.to_string_lossy().to_string();
        let written = || std::fs::read_to_string(crate::workspace_dot_dir(&path).join("AGENTS.md")).expect("AGENTS.md written");
        let strip = |s: &str| s.lines().filter(|l| !l.contains("<!-- GENERATED ")).collect::<Vec<_>>().join("\n");

        // An old file (agents on by default before 0.46.17): no section.
        std::fs::write(super::super::settings::path(), r#"{"enabled": true, "agents": true, "agentDiagnostics": true}"#).unwrap();
        crate::workspace::skill_regen::recompose_agents_md(&path);
        let off = written();
        assert!(!off.contains(HEADING), "{off}");
        assert!(!off.contains("send-feedback"), "nothing about it while off: {off}");

        // A person says yes to CLI problems with the log tail.
        let s = answered(true, false, &["log-tail"]);
        super::super::settings::save(&s).unwrap();
        crate::workspace::skill_regen::recompose_agents_md(&path);
        let on = written();
        let section = agents_md_section(&s).unwrap();
        assert!(on.contains(&section), "{on}");
        assert_eq!(strip(&on), strip(&format!("{off}\n{section}")), "on = off + the section, nothing else");

        // Platform too: the section says so.
        super::super::settings::save(&answered(true, true, &["log-tail"])).unwrap();
        crate::workspace::skill_regen::recompose_agents_md(&path);
        assert!(written().contains("Also allowed: `--scope platform`"));

        // A no: gone again.
        super::super::settings::save(&answered(false, false, &[])).unwrap();
        crate::workspace::skill_regen::recompose_agents_md(&path);
        let again = written();
        assert!(!again.contains(HEADING), "{again}");
        assert_eq!(strip(&again), strip(&off));
    }
}
