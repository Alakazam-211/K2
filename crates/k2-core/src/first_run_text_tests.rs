//! Tests for what `k2 setup` prints (`?format=text`): the CLI's own
//! wording, byte for byte.

use super::*;
use crate::first_run::{KesselDefaults, KesselStatus, KesselView, Row};

fn row(id: &'static str, state: State, opens: Option<&'static str>, note: Option<&'static str>) -> Row {
    Row { id, state, opens, note }
}

fn status(rows: Vec<Row>) -> Status {
    Status { enabled: true, hidden: false, finished: false, rows, kessel: None, newly_finished: false }
}

fn kessel(offer: bool, live: bool) -> Option<KesselStatus> {
    Some(KesselStatus {
        offer: offer.then_some(OFFER_MEET_KESSEL),
        kessel: live.then(|| KesselView { workspace_id: "ws-kessel".into(), name: "Kessel".into(), path: "/Users/me/k2-agents/kessel".into() }),
        kessel_defaults: KesselDefaults { name: "Kessel".into(), handle: "kessel".into(), path: "/Users/me/k2-agents/kessel".into() },
    })
}

#[test]
fn first_run_text_each_state_and_the_count() {
    let s = status(vec![
        row("llm_sign_in", State::Done, Some("llm_sign_in"), None),
        row("first_agent", State::Todo, Some("add_workspace"), None),
        row("tunnel", State::Skipped, Some("tunnel"), None),
    ]);
    assert_eq!(
        render_text(&s),
        "Setup: 1 of 3 done\n  [x] Sign in to an AI tool\n  [ ] Make your first agent\n  [-] Link your subdomain\n"
    );
}

#[test]
fn first_run_text_every_label_and_an_unknown_id() {
    let ids = ["dev_tools", "llm_sign_in", "first_agent", "chief_of_staff", "tunnel", "phone", "own_sign_in", "k2_cli"];
    let s = status(ids.iter().map(|id| row(id, State::Done, None, None)).collect());
    assert_eq!(
        render_text(&s),
        "Setup: 8 of 8 done\n  [x] Install developer tools\n  [x] Sign in to an AI tool\n  [x] Make your first agent\n  [x] Hire your Chief of Staff\n  [x] Link your subdomain\n  [x] Connect your phone\n  [x] Make your own sign-in\n  [x] k2_cli\n"
    );
}

#[test]
fn first_run_text_notes_show_until_done() {
    let s = status(vec![
        row("chief_of_staff", State::Todo, Some("kessel_sheet"), Some(NOTE_NEEDS_KESSEL)),
        row("phone", State::Todo, Some("phone"), Some(NOTE_NEEDS_TUNNEL)),
        row("own_sign_in", State::Skipped, Some("owner_account_prompt"), Some("some_new_note")),
    ]);
    assert_eq!(
        render_text(&s),
        "Setup: 0 of 3 done\n  [ ] Hire your Chief of Staff (You will meet Kessel first.)\n  [ ] Connect your phone (Away from home, your phone needs your subdomain first.)\n  [-] Make your own sign-in\n"
    );
    // A done row prints no note; a skipped one still does.
    let s = status(vec![
        row("phone", State::Done, Some("phone"), Some(NOTE_NEEDS_TUNNEL)),
        row("chief_of_staff", State::Skipped, Some("kessel_sheet"), Some(NOTE_NEEDS_KESSEL)),
    ]);
    assert_eq!(
        render_text(&s),
        "Setup: 1 of 2 done\n  [x] Connect your phone\n  [-] Hire your Chief of Staff (You will meet Kessel first.)\n"
    );
}

#[test]
fn first_run_text_dev_tools_mac_linux_installing_done() {
    let rest = || row("llm_sign_in", State::Todo, Some("llm_sign_in"), None);
    let mac = status(vec![row("dev_tools", State::Todo, Some("dev_tools"), None), rest()]);
    assert_eq!(
        render_text(&mac),
        "Setup: 0 of 2 done\n  [ ] Install developer tools (The Apple tools include git, which workspaces need: k2 git-tools install)\n  [ ] Sign in to an AI tool\n"
    );
    let installing = status(vec![row("dev_tools", State::Todo, Some("dev_tools"), Some(NOTE_INSTALLING)), rest()]);
    assert_eq!(
        render_text(&installing),
        "Setup: 0 of 2 done\n  [ ] Install developer tools (The Apple installer is open. Follow it, and this step finishes by itself.)\n  [ ] Sign in to an AI tool\n"
    );
    let linux = status(vec![row("dev_tools", State::Todo, None, Some("sudo apt install git")), rest()]);
    assert_eq!(
        render_text(&linux),
        "Setup: 0 of 2 done\n  [ ] Install developer tools (run: sudo apt install git)\n  [ ] Sign in to an AI tool\n"
    );
    let done = status(vec![row("dev_tools", State::Done, Some("dev_tools"), None), rest()]);
    assert_eq!(render_text(&done), "Setup: 1 of 2 done\n  [x] Install developer tools\n  [ ] Sign in to an AI tool\n");
    let skipped = status(vec![row("dev_tools", State::Skipped, Some("dev_tools"), None)]);
    assert_eq!(
        render_text(&skipped),
        "Setup: 0 of 1 done\n  [-] Install developer tools (The Apple tools include git, which workspaces need: k2 git-tools install)\n"
    );
}

#[test]
fn first_run_text_finished_hidden_and_the_switch_off() {
    let one = || vec![row("phone", State::Done, Some("phone"), None)];
    let mut s = status(one());
    s.finished = true;
    s.hidden = true;
    assert_eq!(
        render_text(&s),
        "Setup: 1 of 1 done\n  [x] Connect your phone\nFinished: the app no longer shows this list (k2 setup show brings it back).\n"
    );
    let mut s = status(one());
    s.hidden = true;
    assert_eq!(render_text(&s), "Setup: 1 of 1 done\n  [x] Connect your phone\nHidden in the app (k2 setup show brings it back).\n");
    let mut s = status(one());
    s.enabled = false;
    assert_eq!(
        render_text(&s),
        "Setup: 1 of 1 done\n  [x] Connect your phone\nThe app does not show setup lists on this server yet.\n"
    );
}

#[test]
fn first_run_text_no_rows() {
    // An agent never gets a list (403 first_run_person_only, which the CLI
    // prints from the JSON refusal); a person with no rows gets the count.
    assert_eq!(render_text(&status(Vec::new())), "Setup: 0 of 0 done\n");
}

#[test]
fn first_run_text_kessel_offer_and_live_kessel() {
    let mut s = status(vec![row("llm_sign_in", State::Done, Some("llm_sign_in"), None)]);
    s.kessel = kessel(true, false);
    assert_eq!(
        render_text(&s),
        "Setup: 1 of 1 done\n  [x] Sign in to an AI tool\nNext: meet Kessel, the agent that runs K2: k2 setup kessel\n"
    );
    s.kessel = kessel(false, true);
    assert_eq!(
        render_text(&s),
        "Setup: 1 of 1 done\n  [x] Sign in to an AI tool\nKessel: Kessel at /Users/me/k2-agents/kessel (k2 thread <its handle> to talk to it)\n"
    );
    // kesselFirstRun on, no Kessel and no offer: nothing more.
    s.kessel = kessel(false, false);
    assert_eq!(render_text(&s), "Setup: 1 of 1 done\n  [x] Sign in to an AI tool\n");
}

#[test]
fn first_run_text_made_kessel_and_chief_of_staff() {
    let granted = vec!["agents.hire".to_string(), "settings.read".to_string()];
    let left = vec![("organization.manage".to_string(), "owner_floor".to_string())];
    assert_eq!(
        render_made_text(Made::Kessel, "Kessel", "kessel", "/Users/me/k2-agents/kessel", &granted, &left),
        "Made Kessel: Kessel (kessel) at /Users/me/k2-agents/kessel\nAllowed from the start: agents.hire, settings.read\nIt asks on a permission card for: organization.manage (owner_floor)\nKessel is starting and will say hello on its Thread: k2 thread kessel\n"
    );
    assert_eq!(
        render_made_text(Made::ChiefOfStaff, "Ada", "ada", "/Users/me/k2-agents/ada", &[], &[]),
        "Made your Chief of Staff: Ada (ada) at /Users/me/k2-agents/ada\nTell your person on your Thread, and point them to Ada (k2 msg ada).\n"
    );
}
