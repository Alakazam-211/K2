//! The native self-view's pure parts: when the sink opens and the window
//! shows, the frame rule (mirror and flush), the mirror by facing, where the
//! window goes, and the fallback when the native path fails.

use super::*;

#[derive(Default)]
struct FakeOps {
    acts: Vec<String>,
    /// `open_sink` answers this refusal.
    refuse: Option<&'static str>,
    /// `show` answers this failure.
    no_window: Option<&'static str>,
    told: Vec<SelfViewState>,
}

impl SelfOps for FakeOps {
    fn open_sink(&mut self) -> Result<(), String> {
        self.acts.push("open".into());
        match self.refuse {
            Some(code) => Err(code.into()),
            None => Ok(()),
        }
    }
    fn close_sink(&mut self) {
        self.acts.push("close".into());
    }
    fn show(&mut self, window: &str, _anchor: Option<CssRect>) -> Result<(), String> {
        self.acts.push(format!("show {window}"));
        match self.no_window {
            Some(why) => Err(why.into()),
            None => Ok(()),
        }
    }
    fn hide(&mut self) {
        self.acts.push("hide".into());
    }
    fn tell(&mut self, state: &SelfViewState) {
        self.told.push(state.clone());
    }
    fn tick(&mut self) {
        self.acts.push("tick".into());
    }
}

fn want(window: &str, on: bool) -> ToSelf {
    ToSelf::Want { window: window.into(), want: on, anchor: None }
}

fn acts(ops: &mut FakeOps) -> Vec<String> {
    std::mem::take(&mut ops.acts)
}

fn last_told(ops: &FakeOps) -> SelfViewState {
    ops.told.last().cloned().expect("the webviews were told")
}

#[test]
fn the_sink_opens_when_a_window_wants_it_and_the_window_shows_only_while_the_feed_is_live() {
    let mut plan = SelfPlan::default();
    let mut ops = FakeOps::default();
    plan.handle(want("main", true), &mut ops);
    assert_eq!(acts(&mut ops), vec!["open"]);
    // Open, but no frames yet: the page keeps its own tile.
    assert_eq!(last_told(&ops), SelfViewState { available: true, showing: false, reason: None });

    plan.handle(ToSelf::Feed(Feed::Live), &mut ops);
    assert_eq!(acts(&mut ops), vec!["show main"]);
    assert_eq!(last_told(&ops), SelfViewState { available: true, showing: true, reason: None });

    // The camera pauses (another app took it, the window is hidden): the
    // window hides and the page's tile says paused; the sink stays.
    plan.handle(ToSelf::Feed(Feed::Paused), &mut ops);
    assert_eq!(acts(&mut ops), vec!["hide"]);
    assert!(!last_told(&ops).showing);
    plan.handle(ToSelf::Feed(Feed::Live), &mut ops);
    assert_eq!(acts(&mut ops), vec!["show main"]);

    // The camera goes off (or the call ends): hidden, then closed.
    plan.handle(want("main", false), &mut ops);
    assert_eq!(acts(&mut ops), vec!["hide", "close"]);
    assert_eq!(last_told(&ops), SelfViewState { available: true, showing: false, reason: None });
}

#[test]
fn nothing_opens_without_a_window_that_wants_it_and_late_feed_events_are_ignored() {
    let mut plan = SelfPlan::default();
    let mut ops = FakeOps::default();
    plan.handle(want("main", false), &mut ops);
    plan.handle(ToSelf::Feed(Feed::Live), &mut ops);
    plan.handle(ToSelf::NoBuffer, &mut ops);
    plan.handle(ToSelf::Dismissed, &mut ops);
    assert_eq!(acts(&mut ops), Vec::<String>::new());
    assert!(!plan.is_open());
}

#[test]
fn a_refused_sink_leaves_the_page_its_tile_until_the_camera_goes_off_and_on() {
    let mut plan = SelfPlan::default();
    let mut ops = FakeOps { refuse: Some("video_unsupported"), ..FakeOps::default() };
    plan.handle(want("main", true), &mut ops);
    assert_eq!(acts(&mut ops), vec!["open"]);
    assert_eq!(
        last_told(&ops),
        SelfViewState { available: true, showing: false, reason: Some("open_refused:video_unsupported".into()) }
    );
    // The same want again (a new anchor): no retry loop.
    plan.handle(want("main", true), &mut ops);
    assert_eq!(acts(&mut ops), Vec::<String>::new());
    // Off and on: tried again.
    plan.handle(want("main", false), &mut ops);
    assert_eq!(last_told(&ops).reason, None);
    ops.refuse = None;
    plan.handle(want("main", true), &mut ops);
    assert_eq!(acts(&mut ops), vec!["open"]);
    assert_eq!(last_told(&ops).reason, None);
}

#[test]
fn frames_without_an_os_buffer_close_the_sink_and_the_page_keeps_its_tile() {
    let mut plan = SelfPlan::default();
    let mut ops = FakeOps::default();
    plan.handle(want("main", true), &mut ops);
    plan.handle(ToSelf::Feed(Feed::Live), &mut ops);
    acts(&mut ops);
    plan.handle(ToSelf::NoBuffer, &mut ops);
    assert_eq!(acts(&mut ops), vec!["hide", "close"]);
    assert_eq!(
        last_told(&ops),
        SelfViewState { available: true, showing: false, reason: Some("no_pixel_buffer".into()) }
    );
}

#[test]
fn a_window_that_cannot_show_closes_the_sink_and_says_why() {
    let mut plan = SelfPlan::default();
    let mut ops = FakeOps { no_window: Some("timeout"), ..FakeOps::default() };
    plan.handle(want("main", true), &mut ops);
    plan.handle(ToSelf::Feed(Feed::Live), &mut ops);
    assert_eq!(acts(&mut ops), vec!["open", "show main", "close"]);
    assert_eq!(last_told(&ops), SelfViewState { available: true, showing: false, reason: Some("window:timeout".into()) });
    // Never blank: the next feed event changes nothing.
    plan.handle(ToSelf::Feed(Feed::Live), &mut ops);
    assert_eq!(acts(&mut ops), Vec::<String>::new());
}

#[test]
fn closing_the_window_keeps_it_closed_until_the_camera_next_goes_off() {
    let mut plan = SelfPlan::default();
    let mut ops = FakeOps::default();
    plan.handle(want("main", true), &mut ops);
    plan.handle(ToSelf::Feed(Feed::Live), &mut ops);
    acts(&mut ops);
    plan.handle(ToSelf::Dismissed, &mut ops);
    assert_eq!(acts(&mut ops), vec!["hide", "close"]);
    assert_eq!(last_told(&ops), SelfViewState { available: true, showing: false, reason: Some("closed".into()) });
    // Still wanted (the camera stays on): stays closed.
    plan.handle(want("main", true), &mut ops);
    assert_eq!(acts(&mut ops), Vec::<String>::new());
    assert_eq!(last_told(&ops).reason.as_deref(), Some("closed"));
    // The camera off and on again: back.
    plan.handle(want("main", false), &mut ops);
    plan.handle(want("main", true), &mut ops);
    plan.handle(ToSelf::Feed(Feed::Live), &mut ops);
    assert_eq!(acts(&mut ops), vec!["open", "show main"]);
}

#[test]
fn the_window_follows_the_k2_window_that_shows_the_call() {
    let mut plan = SelfPlan::default();
    let mut ops = FakeOps::default();
    plan.handle(want("main", true), &mut ops);
    plan.handle(ToSelf::Feed(Feed::Live), &mut ops);
    assert_eq!(acts(&mut ops), vec!["open", "show main"]);
    // Pop out: the call's own window wants it too, then the main window lets go.
    plan.handle(want("call", true), &mut ops);
    assert_eq!(acts(&mut ops), vec!["hide", "show call"]);
    plan.handle(want("main", false), &mut ops);
    assert_eq!(acts(&mut ops), Vec::<String>::new());
    // Back to chat: the main window wants it, the call window closes.
    plan.handle(want("main", true), &mut ops);
    plan.handle(want("call", false), &mut ops);
    assert_eq!(acts(&mut ops), vec!["hide", "show main"]);
    assert!(plan.is_open());
    plan.handle(want("main", false), &mut ops);
    assert_eq!(acts(&mut ops), vec!["hide", "close"]);
}

#[test]
fn every_want_is_answered_and_other_changes_only_when_the_state_changes() {
    let mut plan = SelfPlan::default();
    let mut ops = FakeOps::default();
    plan.handle(want("main", true), &mut ops);
    plan.handle(want("main", true), &mut ops);
    assert_eq!(ops.told.len(), 2);
    plan.handle(ToSelf::Feed(Feed::Off), &mut ops);
    assert_eq!(ops.told.len(), 2);
    plan.handle(ToSelf::Feed(Feed::Live), &mut ops);
    assert_eq!(ops.told.len(), 3);
    assert!(ops.told.iter().all(|s| s.available));
}

#[test]
fn the_end_of_the_thread_hides_and_closes_and_an_open_sink_ticks() {
    let mut ops = FakeOps::default();
    let (tx, rx) = mpsc::channel();
    tx.send(want("main", true)).unwrap();
    tx.send(ToSelf::Feed(Feed::Live)).unwrap();
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(90));
        drop(tx);
    });
    self_loop(&mut ops, rx, Duration::from_millis(20));
    sender.join().unwrap();
    let a = ops.acts;
    assert_eq!(&a[..2], &["open".to_string(), "show main".to_string()]);
    assert!(a.iter().filter(|x| *x == "tick").count() >= 1, "{a:?}");
    assert_eq!(&a[a.len() - 2..], &["hide".to_string(), "close".to_string()], "{a:?}");
}

#[test]
fn a_closed_sink_does_not_tick() {
    let mut ops = FakeOps::default();
    let (tx, rx) = mpsc::channel::<ToSelf>();
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        drop(tx);
    });
    self_loop(&mut ops, rx, Duration::from_millis(10));
    sender.join().unwrap();
    assert_eq!(ops.acts, Vec::<String>::new());
}

// ── One frame ───────────────────────────────────────────────────────────

#[test]
fn frames_go_to_the_layer_only_while_the_window_shows() {
    let mut gate = FrameGate::default();
    assert_eq!(gate.frame(false, true), Gate::Drop);
    gate.set_showing(true);
    assert_eq!(gate.frame(false, true), Gate::Push { flush: true });
    assert_eq!(gate.frame(false, false), Gate::Push { flush: false });
    gate.set_showing(false);
    assert_eq!(gate.frame(false, false), Gate::Drop);
}

#[test]
fn a_mirror_change_drops_frames_until_the_view_has_turned() {
    let mut gate = FrameGate::default();
    gate.set_showing(true);
    // The first front camera frame: the view turns to a mirror first.
    assert_eq!(gate.frame(true, true), Gate::Mirror(true));
    assert!(gate.is_mirrored());
    assert_eq!(gate.frame(true, false), Gate::Drop);
    gate.mirrored();
    assert_eq!(gate.frame(true, false), Gate::Push { flush: false });
    // A switch to a camera facing away: back the other way.
    assert_eq!(gate.frame(false, true), Gate::Mirror(false));
    gate.mirrored();
    assert_eq!(gate.frame(false, false), Gate::Push { flush: false });
}

#[test]
fn every_camera_facing_you_is_a_mirror() {
    assert!(desktop_mirror(Facing::Front));
    assert!(desktop_mirror(Facing::External));
    assert!(desktop_mirror(Facing::Unknown));
    assert!(!desktop_mirror(Facing::Back));
}

#[test]
fn the_feed_maps_and_timestamps_go_to_90khz() {
    assert_eq!(Feed::of(&CameraFeed::Live), Feed::Live);
    assert_eq!(Feed::of(&CameraFeed::Paused), Feed::Paused);
    assert_eq!(Feed::of(&CameraFeed::Off), Feed::Off);
    assert_eq!(pts_90k(1_000_000), 90_000);
    assert_eq!(pts_90k(u64::MAX), i64::try_from(u64::MAX / 100).unwrap());
}

// ── Where the window goes ───────────────────────────────────────────────

const CONTENT: ScreenRect = ScreenRect { x: 100.0, y: 50.0, w: 1200.0, h: 800.0 };

#[test]
fn with_no_call_view_it_sits_top_right_below_the_top_bar() {
    let at = panel_frame(CONTENT, None);
    assert_eq!((at.w, at.h), (PANEL_W, PANEL_H));
    assert_eq!(at.x, 100.0 + 1200.0 - PANEL_MARGIN - PANEL_W);
    // Its top is TOP_INSET + MARGIN below the content's top.
    assert_eq!(at.y + at.h, 50.0 + 800.0 - PANEL_TOP_INSET - PANEL_MARGIN);
}

#[test]
fn it_sits_in_the_call_views_top_right_corner() {
    let anchor = CssRect { x: 300.0, y: 100.0, width: 600.0, height: 400.0 };
    let at = panel_frame(CONTENT, Some(anchor));
    assert_eq!(at.x, 100.0 + 300.0 + 600.0 - PANEL_MARGIN - PANEL_W);
    assert_eq!(at.y + at.h, 50.0 + 800.0 - 100.0 - PANEL_MARGIN);
}

#[test]
fn it_stays_inside_the_window_and_ignores_an_empty_anchor() {
    // An anchor narrower than the window would push it off the left edge.
    let narrow = CssRect { x: 0.0, y: 0.0, width: 50.0, height: 50.0 };
    let at = panel_frame(CONTENT, Some(narrow));
    assert_eq!(at.x, CONTENT.x);
    // An anchor past the bottom: kept in.
    let low = CssRect { x: 0.0, y: 790.0, width: 1200.0, height: 10.0 };
    let at = panel_frame(CONTENT, Some(low));
    assert_eq!(at.y, CONTENT.y);
    // Zero-sized: as with none.
    let empty = CssRect { x: 10.0, y: 10.0, width: 0.0, height: 0.0 };
    assert_eq!(panel_frame(CONTENT, Some(empty)), panel_frame(CONTENT, None));
}

// ── The log ─────────────────────────────────────────────────────────────

#[test]
fn the_stats_line_is_counts_only() {
    let mut stats = NativeSinkStats::default();
    stats.frames_in = 300;
    stats.frames_out = 290;
    stats.fps = 29.5;
    stats.dropped_queue_full = 10;
    let counts = SelfCounts::default();
    counts.shown.store(288, Ordering::Relaxed);
    counts.dropped.store(2, Ordering::Relaxed);
    counts.feed.store(1, Ordering::Relaxed);
    let line = stats_line(&stats, &counts);
    assert!(
        line.starts_with("camera self_view feed=live frames_in=300 frames_out=290 fps=29.5 dropped_queue_full=10"),
        "{line}"
    );
    assert!(line.contains("shown=288 dropped=2 no_buffer=0"), "{line}");
    assert!(line.ends_with("mirrored=0 previewed=0"), "{line}");
}

#[test]
fn codes_are_kept_short_and_plain() {
    assert_eq!(code("Video_Unsupported"), "video_unsupported");
    assert_eq!(code("bad code! with spaces/and\"quotes"), "badcodewithspacesandquotes");
    assert_eq!(code(&"x".repeat(100)).len(), 48);
}

#[test]
fn the_state_the_page_reads_is_snake_case_json() {
    let s = SelfViewState { available: true, showing: true, reason: None };
    assert_eq!(serde_json::to_value(&s).unwrap(), serde_json::json!({"available": true, "showing": true, "reason": null}));
}

// ── The zero-hop preview ────────────────────────────────────────────────

/// What the main thread does with the slot, as `reconcile` does: the swap
/// from the layer that is up to the one wanted, then that one is up.
fn swap(slot: &PreviewSlot<&'static str>, installed: &mut Option<u64>) -> PreviewSwap {
    let s = preview_swap(*installed, slot.wanted_generation());
    *installed = slot.wanted_generation();
    s
}

#[test]
fn with_no_preview_offered_the_frames_draw_the_picture() {
    // An older Sew, another OS, or no offer yet: the 0.46.17 path.
    let slot: PreviewSlot<&str> = PreviewSlot::default();
    let mut installed = None;
    assert_eq!(swap(&slot, &mut installed), PreviewSwap::Keep);
    assert_eq!(path_of(installed), SelfPath::Buffers);
    assert_eq!(slot.wanted(), None);
}

#[test]
fn an_offered_preview_goes_up_and_comes_down_when_it_goes() {
    let mut slot = PreviewSlot::default();
    let mut installed = None;
    assert!(slot.ready(1, "cam-a"));
    assert_eq!(slot.wanted(), Some((1, "cam-a")));
    // Until the main thread has put it up, the frames keep the picture.
    assert_eq!(path_of(installed), SelfPath::Buffers);
    assert_eq!(swap(&slot, &mut installed), PreviewSwap::Install(1));
    assert_eq!(path_of(installed), SelfPath::Preview { generation: 1 });
    assert_eq!(swap(&slot, &mut installed), PreviewSwap::Keep);

    assert!(slot.gone(1));
    assert_eq!(swap(&slot, &mut installed), PreviewSwap::Remove(1));
    assert_eq!(path_of(installed), SelfPath::Buffers);
}

#[test]
fn a_new_generation_replaces_the_old_layer_in_one_step() {
    // A flip or a reopen: Sew sends the old one's PreviewGone first, but the
    // main thread may run only once both are in: one replace, no gap.
    let mut slot = PreviewSlot::default();
    let mut installed = None;
    slot.ready(1, "front");
    swap(&slot, &mut installed);
    assert!(slot.gone(1));
    assert!(slot.ready(2, "back"));
    assert_eq!(swap(&slot, &mut installed), PreviewSwap::Replace { old: 1, new: 2 });
    assert_eq!(slot.wanted(), Some((2, "back")));
    // A newer offer with no PreviewGone before it replaces too.
    assert!(slot.ready(3, "external"));
    assert_eq!(swap(&slot, &mut installed), PreviewSwap::Replace { old: 2, new: 3 });
}

#[test]
fn stale_offers_and_goes_change_nothing() {
    let mut slot = PreviewSlot::default();
    slot.ready(5, "now");
    // A PreviewGone for an older generation leaves the current one up.
    assert!(!slot.gone(4));
    assert_eq!(slot.wanted_generation(), Some(5));
    // A repeat, or an older generation offered late, is ignored.
    assert!(!slot.ready(5, "again"));
    assert!(!slot.ready(3, "old"));
    assert_eq!(slot.wanted(), Some((5, "now")));
    // Once gone, it isn't taken again.
    assert!(slot.gone(5));
    assert!(!slot.ready(5, "now"));
    assert_eq!(slot.wanted(), None);
}

#[test]
fn a_layer_that_cannot_be_made_falls_back_to_the_frames_until_the_next_generation() {
    let mut slot = PreviewSlot::default();
    let mut installed = Some(1);
    slot.ready(1, "a");
    slot.ready(2, "b");
    // Making generation 2's layer failed: generation 1's (a stale source)
    // comes down too, and the frames draw the picture.
    assert!(slot.failed(2));
    assert_eq!(swap(&slot, &mut installed), PreviewSwap::Remove(1));
    assert_eq!(path_of(installed), SelfPath::Buffers);
    assert!(!slot.failed(2), "only the offered generation fails");
    assert!(!slot.ready(2, "b"), "no retry loop on the same source");
    assert!(slot.ready(3, "c"), "the next source tries again");
}

#[test]
fn a_closed_sink_takes_the_preview_down_and_the_next_sink_starts_afresh() {
    let mut slot = PreviewSlot::default();
    let mut installed = None;
    slot.ready(7, "a");
    swap(&slot, &mut installed);
    slot.clear();
    assert_eq!(swap(&slot, &mut installed), PreviewSwap::Remove(7));
    // A new sink's numbers may start lower: taken.
    assert!(slot.ready(1, "b"));
    assert_eq!(swap(&slot, &mut installed), PreviewSwap::Install(1));
}
