//! C4 line-mux tests: `LineMux::feed()` on synthetic PTY byte chunks.
//!
//! Recorded Claude Code fixtures land alongside C6 when the recognizer
//! needs real data; Phase 1 sticks to hand-crafted byte sequences so
//! the tests can be read end-to-end without inspecting a `.bin` blob.

use k2_core::session::{CursorOp, CursorShape, EraseMode, Frame, ModeKind, Style};
use k2_core::term::LineMux;

/// Helper: extract Style from a Text frame (None = default style).
fn text_frame_style(frame: &Frame) -> Option<&Option<Style>> {
    match frame {
        Frame::Text { style, .. } => Some(style),
        _ => None,
    }
}

/// Helper: collect all (bytes, style) tuples from Text frames.
fn text_frames_with_styles(frames: &[Frame]) -> Vec<(&[u8], Option<Style>)> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::Text { bytes, style } => Some((bytes.as_slice(), style.clone())),
            _ => None,
        })
        .collect()
}

fn text_frame_bytes(frame: &Frame) -> Option<&[u8]> {
    match frame {
        Frame::Text { bytes, .. } => Some(bytes),
        _ => None,
    }
}

fn cursor_op(frame: &Frame) -> Option<&CursorOp> {
    match frame {
        Frame::CursorOp(op) => Some(op),
        _ => None,
    }
}

#[test]
fn two_lines_committed_and_two_text_frames_emitted() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"hello\nworld\n");

    // Scrollback has 2 committed lines.
    let lines: Vec<_> = mux.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].text, "hello");
    assert_eq!(lines[1].text, "world");
    assert!(lines[0].seqno < lines[1].seqno);

    // Two Text frames — one per run of printable chars + the
    // committing LF. Phase 4.5 preserves the `\n` in Frame::Text
    // bytes so TerminalGrid consumers can reconstruct line breaks.
    let text_frames: Vec<_> = frames.iter().filter_map(text_frame_bytes).collect();
    assert_eq!(text_frames.len(), 2);
    assert_eq!(text_frames[0], b"hello\n");
    assert_eq!(text_frames[1], b"world\n");

    // The current (unfinished) line is empty since the chunk ended on LF.
    assert!(mux.current_line_text().is_none());
}

#[test]
fn clear_screen_emits_one_cursor_op() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[2J");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::ClearScreen)
    ));
}

#[test]
fn cup_goto_parses_row_col() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[10;20H");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::Goto { row: 10, col: 20 })
    ));
}

#[test]
fn cup_goto_defaults_when_params_omitted() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[H");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::Goto { row: 1, col: 1 })
    ));
}

#[test]
fn cursor_movement_variants() {
    let cases: [(&[u8], fn(&CursorOp) -> bool); 4] = [
        (b"\x1b[3A", |op| matches!(op, CursorOp::Up(3))),
        (b"\x1b[5B", |op| matches!(op, CursorOp::Down(5))),
        (b"\x1b[7C", |op| matches!(op, CursorOp::Forward(7))),
        (b"\x1b[9D", |op| matches!(op, CursorOp::Back(9))),
    ];
    for (input, check) in cases {
        let mut mux = LineMux::new();
        let frames = mux.feed(input);
        assert_eq!(frames.len(), 1);
        assert!(check(cursor_op(&frames[0]).unwrap()));
    }
}

#[test]
fn erase_in_line_modes() {
    let mut mux = LineMux::new();
    // EL 0 (to end), EL 1 (from start), EL 2 (all).
    let frames = mux.feed(b"\x1b[0K\x1b[1K\x1b[2K");
    let ops: Vec<_> = frames.iter().filter_map(cursor_op).collect();
    assert_eq!(ops.len(), 3);
    assert!(matches!(
        ops[0],
        CursorOp::EraseInLine(EraseMode::ToEnd)
    ));
    assert!(matches!(
        ops[1],
        CursorOp::EraseInLine(EraseMode::FromStart)
    ));
    assert!(matches!(
        ops[2],
        CursorOp::EraseInLine(EraseMode::All)
    ));
}

#[test]
fn backspace_strips_last_char_from_current_line() {
    let mut mux = LineMux::new();
    let _ = mux.feed(b"hello\x08\x08\x08");
    assert_eq!(mux.current_line_text(), Some("he"));
}

#[test]
fn unterminated_line_visible_via_current_line_text() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"partial");
    // No line committed yet — chunk didn't end with LF.
    assert_eq!(mux.line_count(), 0);
    assert_eq!(mux.current_line_text(), Some("partial"));
    // One Text frame was flushed at chunk end.
    let text_frames: Vec<_> = frames.iter().filter_map(text_frame_bytes).collect();
    assert_eq!(text_frames.len(), 1);
    assert_eq!(text_frames[0], b"partial");
}

#[test]
fn mixed_text_and_control_preserves_ordering() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"pre\x1b[2Jpost\n");
    // Frames should be: Text("pre"), CursorOp(ClearScreen), Text("post")
    // (terminal line committed + Text frame flushed before LF).
    assert!(frames.len() >= 3);
    assert_eq!(text_frame_bytes(&frames[0]), Some(&b"pre"[..]));
    assert!(matches!(
        cursor_op(&frames[1]),
        Some(CursorOp::ClearScreen)
    ));
    // Phase 4.5: the committing LF is preserved in Frame::Text.
    assert_eq!(text_frame_bytes(&frames[2]), Some(&b"post\n"[..]));

    let lines: Vec<_> = mux.lines().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "prepost");
}

#[test]
fn split_feed_does_not_lose_bytes() {
    // Two `feed()` calls, each a partial CSI sequence — vte's Parser
    // is stateful, so the mux should still emit the one expected
    // CursorOp once the sequence completes.
    let mut mux = LineMux::new();
    let first = mux.feed(b"\x1b[");
    assert_eq!(first.len(), 0, "incomplete CSI should emit nothing");
    let second = mux.feed(b"2J");
    assert_eq!(second.len(), 1);
    assert!(matches!(
        cursor_op(&second[0]),
        Some(CursorOp::ClearScreen)
    ));
}

#[test]
fn scrollback_cap_trims_oldest_lines() {
    let mut mux = LineMux::with_cap(3);
    let _ = mux.feed(b"a\nb\nc\nd\ne\n");
    // Cap is 3 — only last 3 lines remain.
    let lines: Vec<_> = mux.lines().map(|l| l.text.clone()).collect();
    assert_eq!(lines, vec!["c".to_string(), "d".to_string(), "e".to_string()]);
    // Seqnos keep growing; current_seqno reflects how many lines
    // have been committed + the one being built.
    assert!(mux.current_seqno() >= 5);
}

#[test]
fn seqno_is_monotonic_across_line_commits() {
    let mut mux = LineMux::new();
    let _ = mux.feed(b"alpha\nbeta\ngamma\n");
    let lines: Vec<_> = mux.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].seqno < lines[1].seqno);
    assert!(lines[1].seqno < lines[2].seqno);
}

#[test]
fn empty_feed_is_a_noop() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"");
    assert!(frames.is_empty());
    assert_eq!(mux.line_count(), 0);
    assert!(mux.current_line_text().is_none());
}

#[test]
#[should_panic(expected = "LineMux cap must be >= 1")]
fn zero_cap_panics() {
    let _ = LineMux::with_cap(0);
}

// ── Phase 4.5 SGR tests ────────────────────────────────────────────

#[test]
fn sgr_red_fg_colors_subsequent_text() {
    // ESC[31m sets fg to red (palette index 1).
    let mut mux = LineMux::new();
    let frames = mux.feed(b"plain\x1b[31mred\n");
    let triples = text_frames_with_styles(&frames);
    assert_eq!(triples.len(), 2, "got {:#?}", triples);
    // First frame "plain" has no style.
    assert_eq!(triples[0].0, b"plain");
    assert!(triples[0].1.is_none());
    // Second frame "red\n" carries fg=0xcd0000.
    assert_eq!(triples[1].0, b"red\n");
    let style = triples[1].1.as_ref().expect("red style");
    assert_eq!(style.fg, Some(0xcd0000));
    assert_eq!(style.bg, None);
}

#[test]
fn sgr_reset_clears_current_style() {
    // After red fg + reset, following text is unstyled.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[31mred\x1b[0mplain\n");
    let triples = text_frames_with_styles(&frames);
    assert_eq!(triples.len(), 2);
    assert_eq!(triples[0].0, b"red");
    assert_eq!(triples[0].1.as_ref().unwrap().fg, Some(0xcd0000));
    assert_eq!(triples[1].0, b"plain\n");
    assert!(triples[1].1.is_none(), "expected default style after reset");
}

#[test]
fn sgr_empty_params_equals_reset() {
    // ESC[m (no params) == ESC[0m.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[31mred\x1b[mplain\n");
    let triples = text_frames_with_styles(&frames);
    assert_eq!(triples.len(), 2);
    assert!(triples[1].1.is_none());
}

#[test]
fn sgr_bold_italic_underline_set_attrs() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[1;3;4mbiu\n");
    let triples = text_frames_with_styles(&frames);
    let style = triples[0].1.as_ref().expect("style");
    assert!(style.bold);
    assert!(style.italic);
    assert!(style.underline);
}

#[test]
fn sgr_22_turns_off_bold() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[1mb\x1b[22mnot\n");
    let triples = text_frames_with_styles(&frames);
    assert_eq!(triples.len(), 2);
    assert!(triples[0].1.as_ref().unwrap().bold);
    // After 22, bold off — and no other attrs → collapse to None.
    assert!(triples[1].1.is_none());
}

#[test]
fn sgr_256_color_fg() {
    // ESC[38;5;208m should set fg to orange (palette index 208 in the
    // 6x6x6 cube: i=192 → r=LEVELS[5]=255, g=LEVELS[2]=135, b=LEVELS[0]=0).
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[38;5;208morange\n");
    let triples = text_frames_with_styles(&frames);
    let style = triples[0].1.as_ref().expect("style");
    assert_eq!(style.fg, Some(0xff8700), "got {:06x}", style.fg.unwrap());
}

#[test]
fn sgr_truecolor_fg() {
    // ESC[38;2;255;100;50m
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[38;2;255;100;50mtc\n");
    let triples = text_frames_with_styles(&frames);
    let style = triples[0].1.as_ref().expect("style");
    assert_eq!(style.fg, Some(0xff6432));
}

#[test]
fn sgr_bg_color_and_default_bg() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[44mbluebg\x1b[49mclear\n");
    let triples = text_frames_with_styles(&frames);
    assert_eq!(triples.len(), 2);
    let first = triples[0].1.as_ref().unwrap();
    assert_eq!(first.bg, Some(0x0000ee));
    // ESC[49m clears bg → style collapses to None.
    assert!(triples[1].1.is_none());
}

#[test]
fn sgr_bright_colors_30_90_range() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[91mbrightred\n");
    let triples = text_frames_with_styles(&frames);
    let style = triples[0].1.as_ref().expect("style");
    assert_eq!(style.fg, Some(0xff0000));
}

#[test]
fn save_cursor_emits_save_cursor_op() {
    // DECSC (ESC[s) — save cursor. Claude Code uses this before
    // painting spinners to avoid visibly moving the "real" cursor.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[s");
    assert_eq!(frames.len(), 1);
    assert!(matches!(cursor_op(&frames[0]), Some(CursorOp::SaveCursor)));
}

#[test]
fn restore_cursor_emits_restore_cursor_op() {
    // DECRC (ESC[u) — restore cursor to saved position.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[u");
    assert_eq!(frames.len(), 1);
    assert!(matches!(cursor_op(&frames[0]), Some(CursorOp::RestoreCursor)));
}

#[test]
fn legacy_esc7_emits_save_cursor_op() {
    // ESC 7 — legacy (non-CSI) save cursor. Pre-dates CSI s/u;
    // still widely used by tmux, vim, and likely Claude Code's
    // input-line repaint.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b7");
    assert_eq!(frames.len(), 1);
    assert!(matches!(cursor_op(&frames[0]), Some(CursorOp::SaveCursor)));
}

#[test]
fn legacy_esc8_emits_restore_cursor_op() {
    // ESC 8 — legacy restore cursor.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b8");
    assert_eq!(frames.len(), 1);
    assert!(matches!(cursor_op(&frames[0]), Some(CursorOp::RestoreCursor)));
}

#[test]
fn dectcem_hide_emits_set_cursor_visible_false() {
    // CSI ? 25 l — DECTCEM cursor hide. TUIs emit this before a
    // multi-step repaint so the caret doesn't flicker through
    // intermediate positions.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[?25l");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::SetCursorVisible(false))
    ));
}

#[test]
fn dectcem_show_emits_set_cursor_visible_true() {
    // CSI ? 25 h — DECTCEM cursor show. Paired with hide above;
    // emitted after the repaint settles.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[?25h");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::SetCursorVisible(true))
    ));
}

#[test]
fn bracketed_paste_mode_set_emits_mode_change_on() {
    // DECSET ?2004 h — bracketed paste mode on. The TUI is
    // announcing it wants pastes wrapped in ESC[200~ / ESC[201~.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[?2004h");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        &frames[0],
        Frame::ModeChange { mode: ModeKind::BracketedPaste, on: true }
    ));
}

#[test]
fn bracketed_paste_mode_reset_emits_mode_change_off() {
    // DECRST ?2004 l — bracketed paste off. Paste should stop
    // being wrapped; TUI is back in line-oriented / raw mode.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[?2004l");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        &frames[0],
        Frame::ModeChange { mode: ModeKind::BracketedPaste, on: false }
    ));
}

#[test]
fn alt_screen_mode_1049_emits_mode_change() {
    // DECSET ?1049 h — modern alt-screen enter (xterm+). TUIs use
    // this so their full-screen UI doesn't overwrite the user's
    // shell output; on exit (?1049 l) the prior buffer is restored.
    let mut mux = LineMux::new();
    let on_frames = mux.feed(b"\x1b[?1049h");
    assert_eq!(on_frames.len(), 1);
    assert!(matches!(
        &on_frames[0],
        Frame::ModeChange { mode: ModeKind::AltScreen, on: true }
    ));
    let off_frames = mux.feed(b"\x1b[?1049l");
    assert_eq!(off_frames.len(), 1);
    assert!(matches!(
        &off_frames[0],
        Frame::ModeChange { mode: ModeKind::AltScreen, on: false }
    ));
}

#[test]
fn synchronized_output_mode_2026_emits_mode_change() {
    // DECSET ?2026 is xterm+kitty+wezterm+alacritty's "synchronized
    // output" op. TUIs wrap multi-step repaints in ?2026 h / ?2026 l
    // so the renderer can apply the whole burst atomically.
    let mut mux = LineMux::new();
    let on = mux.feed(b"\x1b[?2026h");
    assert_eq!(on.len(), 1);
    assert!(matches!(
        &on[0],
        Frame::ModeChange { mode: ModeKind::SynchronizedOutput, on: true }
    ));
    let off = mux.feed(b"\x1b[?2026l");
    assert_eq!(off.len(), 1);
    assert!(matches!(
        &off[0],
        Frame::ModeChange { mode: ModeKind::SynchronizedOutput, on: false }
    ));
}

#[test]
fn alt_screen_mode_47_is_aliased_to_alt_screen() {
    // DECSET ?47 — the original xterm alt-screen op. Less capable
    // than ?1049 (no cursor save/restore), but some TUIs still
    // emit it; we surface it as the same ModeKind so consumers
    // don't need to branch on the variant.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[?47h");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        &frames[0],
        Frame::ModeChange { mode: ModeKind::AltScreen, on: true }
    ));
}

#[test]
fn application_cursor_mode_1_emits_mode_change() {
    // DECSET ?1 — zsh / vim use this to get SS3-format arrow keys.
    let mut mux = LineMux::new();
    let on = mux.feed(b"\x1b[?1h");
    assert_eq!(on.len(), 1);
    assert!(matches!(
        &on[0],
        Frame::ModeChange { mode: ModeKind::ApplicationCursor, on: true }
    ));
    let off = mux.feed(b"\x1b[?1l");
    assert_eq!(off.len(), 1);
    assert!(matches!(
        &off[0],
        Frame::ModeChange { mode: ModeKind::ApplicationCursor, on: false }
    ));
}

#[test]
fn autowrap_mode_7_emits_mode_change() {
    // DECSET ?7 — autowrap off makes writes past EOL clamp at the
    // last column.
    let mut mux = LineMux::new();
    let off = mux.feed(b"\x1b[?7l");
    assert_eq!(off.len(), 1);
    assert!(matches!(
        &off[0],
        Frame::ModeChange { mode: ModeKind::Autowrap, on: false }
    ));
}

#[test]
fn backspace_byte_passes_through_to_text_frame() {
    // Regression test for the 2026-04-21 "dead characters" bug:
    // LineMux used to pop \x08 from pending_text, so the BS byte never
    // reached the consumer's Text frame. That ate the BS-SP-BS echo
    // that shells emit for backspace + the BS-l-s-SP-BS sequence zsh
    // readline uses for up-arrow history replace — the grid's cursor
    // never moved back, and old characters stayed visible.
    //
    // After the fix, the BS byte is included in the emitted Text
    // frame's bytes array so TerminalGrid.writeChar sees it and
    // decrements the cursor column.
    let mut mux = LineMux::new();
    // 'a' + BS + 'b' should emit a Text frame containing [a, \x08, b].
    let frames = mux.feed(b"a\x08b");
    let bytes = text_frame_bytes(&frames[0]).expect("text frame");
    assert_eq!(bytes, b"a\x08b");
}

#[test]
fn shell_replace_sequence_emits_bs_bytes() {
    // Realistic zsh readline replacement: erase one typed char and
    // draw a two-char command. Sequence: \x08 l s \x20 \x08
    // (BS-'l'-'s'-SP-BS). All five bytes must reach the consumer.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x08ls \x08");
    let concat: Vec<u8> = frames
        .iter()
        .filter_map(text_frame_bytes)
        .flatten()
        .copied()
        .collect();
    assert_eq!(concat, b"\x08ls \x08");
}

#[test]
fn bel_byte_emits_bell_frame() {
    // `\x07` BEL — bash Ctrl-R on empty history, readline
    // ambiguity, explicit `echo -e '\a'`. LineMux surfaces it as
    // Frame::Bell so the renderer can flash the pane.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x07");
    assert!(frames.iter().any(|f| matches!(f, Frame::Bell)));
}

#[test]
fn bel_inside_text_preserves_ordering() {
    // BEL embedded between prints: the Bell frame must appear
    // AFTER the preceding Text frame, not before — the Text flush
    // happens before the Bell push.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"hi\x07there");
    let mut saw_text_hi = false;
    let mut saw_bell = false;
    let mut saw_text_there = false;
    for f in &frames {
        match f {
            Frame::Text { bytes, .. } if bytes == b"hi" => {
                saw_text_hi = true;
                assert!(!saw_bell && !saw_text_there);
            }
            Frame::Bell => {
                assert!(saw_text_hi);
                saw_bell = true;
            }
            Frame::Text { bytes, .. } if bytes == b"there" => {
                assert!(saw_bell);
                saw_text_there = true;
            }
            _ => {}
        }
    }
    assert!(saw_text_hi && saw_bell && saw_text_there);
}

#[test]
fn decscusr_steady_block_emits_set_cursor_style() {
    // CSI 2 SP q — vim normal mode requests a solid block.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[2 q");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::SetCursorStyle(CursorShape::SteadyBlock))
    ));
}

#[test]
fn decscusr_blinking_bar_emits_set_cursor_style() {
    // CSI 5 SP q — vim insert mode (and most readline-mode editors).
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[5 q");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::SetCursorStyle(CursorShape::BlinkingBar))
    ));
}

#[test]
fn decscusr_zero_and_one_map_to_blinking_block_default() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[0 q");
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::SetCursorStyle(CursorShape::BlinkingBlock))
    ));
    let mut mux2 = LineMux::new();
    let frames2 = mux2.feed(b"\x1b[1 q");
    assert!(matches!(
        cursor_op(&frames2[0]),
        Some(CursorOp::SetCursorStyle(CursorShape::BlinkingBlock))
    ));
}

#[test]
fn focus_reporting_mode_1004_emits_mode_change() {
    // DECSET ?1004 — TUIs request focus-in / focus-out events.
    let mut mux = LineMux::new();
    let on = mux.feed(b"\x1b[?1004h");
    assert_eq!(on.len(), 1);
    assert!(matches!(
        &on[0],
        Frame::ModeChange { mode: ModeKind::FocusReporting, on: true }
    ));
}

#[test]
fn dectcem_ignores_unknown_private_mode() {
    // CSI ? 12 l — we don't handle this yet (cursor blink). It
    // should be silently dropped, NOT misinterpreted as cursor hide.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[?12l");
    assert_eq!(frames.len(), 0);
}

#[test]
fn save_paint_restore_sequence_emits_ordered_ops() {
    // End-to-end Claude-style spinner paint:
    //   save → go to row 5 col 1 → emit char → restore
    // Grid consumer sees cursor end up where it started.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[s\x1b[5;1Hx\x1b[u");
    // Expected frames: SaveCursor, Goto(5,1), Text("x"),
    // RestoreCursor.
    assert!(frames.len() >= 4);
    assert!(matches!(cursor_op(&frames[0]), Some(CursorOp::SaveCursor)));
    assert!(matches!(
        cursor_op(&frames[1]),
        Some(CursorOp::Goto { row: 5, col: 1 })
    ));
    assert_eq!(text_frame_bytes(&frames[2]), Some(&b"x"[..]));
    assert!(matches!(
        cursor_op(&frames[3]),
        Some(CursorOp::RestoreCursor)
    ));
}

#[test]
fn sgr_flushes_before_style_change() {
    // Pending "pre" carries the OLD style, even mid-line.
    let mut mux = LineMux::new();
    let frames = mux.feed(b"pre\x1b[32mpost\n");
    let triples = text_frames_with_styles(&frames);
    assert_eq!(triples.len(), 2);
    assert_eq!(triples[0].0, b"pre");
    assert!(triples[0].1.is_none());
    assert_eq!(triples[1].0, b"post\n");
    let post_style = triples[1].1.as_ref().unwrap();
    assert_eq!(post_style.fg, Some(0x00cd00));
}
