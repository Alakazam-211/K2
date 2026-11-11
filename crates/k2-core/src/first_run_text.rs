//! What `k2 setup` prints (`/cli/first-run/*?format=text`): plain lines,
//! so the CLI needs no python3. On a Mac without Apple's Command Line
//! Tools `/usr/bin/python3` is one of Apple's shims that opens the
//! installer instead of running, and that is the Mac the `dev_tools` row
//! is for. The wording is the CLI's (byte for byte); both renderers are
//! pure.

use crate::first_run::{State, Status, NOTE_INSTALLING, NOTE_NEEDS_K5, NOTE_NEEDS_TUNNEL, OFFER_MEET_K5};

/// A row's label; an id with none is printed as it is.
pub fn label(id: &str) -> &str {
    match id {
        "dev_tools" => "Install developer tools",
        "llm_sign_in" => "Sign in to an AI tool",
        "first_agent" => "Make your first agent",
        "chief_of_staff" => "Hire your Chief of Staff",
        "tunnel" => "Link your subdomain",
        "phone" => "Connect your phone",
        "own_sign_in" => "Make your own sign-in",
        other => other,
    }
}

fn note_text(note: &str) -> Option<&'static str> {
    match note {
        NOTE_NEEDS_TUNNEL => Some("Away from home, your phone needs your subdomain first."),
        NOTE_NEEDS_K5 => Some("You will meet K5 first."),
        NOTE_INSTALLING => Some("The Apple installer is open. Follow it, and this step finishes by itself."),
        _ => None,
    }
}

fn mark(state: State) -> &'static str {
    match state {
        State::Done => "[x]",
        State::Todo => "[ ]",
        State::Skipped => "[-]",
    }
}

/// The list as `k2 setup` (and `hide`, `show`, `skip`, `dismiss-offer`)
/// prints it.
pub fn render_text(s: &Status) -> String {
    let mut out: Vec<String> = Vec::new();
    let done = s.rows.iter().filter(|r| r.state == State::Done).count();
    out.push(format!("Setup: {done} of {} done", s.rows.len()));
    for r in &s.rows {
        let mut line = format!("  {} {}", mark(r.state), label(r.id));
        let mut note: Option<String> = r.note.and_then(note_text).map(str::to_string);
        if r.id == "dev_tools" && r.opens.is_none() && r.note.is_some_and(|n| !n.is_empty()) {
            // Linux: the note is the install line to run.
            note = r.note.map(|n| format!("run: {n}"));
        } else if r.id == "dev_tools" && r.opens == Some("dev_tools") && note.is_none() {
            note = Some("The Apple tools include git, which workspaces need: k2 git-tools install".to_string());
        }
        if let Some(n) = note {
            if r.state != State::Done {
                line.push_str(&format!(" ({n})"));
            }
        }
        out.push(line);
    }
    if s.finished {
        out.push("Finished: the app no longer shows this list (k2 setup show brings it back).".into());
    } else if s.hidden {
        out.push("Hidden in the app (k2 setup show brings it back).".into());
    }
    if !s.enabled {
        out.push("The app does not show setup lists on this server yet.".into());
    }
    if let Some(k5) = &s.k5 {
        if let Some(live) = &k5.k5 {
            out.push(format!("K5: {} at {} (k2 thread <its handle> to talk to it)", live.name, live.path));
        }
        if k5.offer == Some(OFFER_MEET_K5) {
            out.push("Next: meet K5, the agent that runs K2: k2 setup k5".into());
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Which agent `k2 setup k5` or `k2 setup chief-of-staff` made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Made {
    K5,
    ChiefOfStaff,
}

/// The agent just made, as the two making verbs print it. `left` is each
/// permission left for a card, with why: `(action, reason)`.
pub fn render_made_text(made: Made, name: &str, handle: &str, path: &str, granted: &[String], left: &[(String, String)]) -> String {
    let what = match made {
        Made::K5 => "K5",
        Made::ChiefOfStaff => "your Chief of Staff",
    };
    let mut out = vec![format!("Made {what}: {name} ({handle}) at {path}")];
    if !granted.is_empty() {
        out.push(format!("Allowed from the start: {}", granted.join(", ")));
    }
    if !left.is_empty() {
        let asks: Vec<String> = left.iter().map(|(a, r)| format!("{a} ({r})")).collect();
        out.push(format!("It asks on a permission card for: {}", asks.join(", ")));
    }
    out.push(match made {
        Made::K5 => format!("K5 is starting and will say hello on its Thread: k2 thread {handle}"),
        Made::ChiefOfStaff => format!("Tell your person on your Thread, and point them to {name} (k2 msg {handle})."),
    });
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
#[path = "first_run_text_tests.rs"]
mod tests;
