//! E6 tests for `session::archive` — per-session NDJSON writer.
//!
//! Covers:
//!   - Spawn → publish N frames → exit → N lines in archive.ndjson
//!     each deserializes back to the original frame
//!   - File path shape: `<project>/.k2/sessions/<id>/archive.ndjson`
//!   - Writer exits naturally when SessionEntry's sender drops
//!   - File appended, not truncated (two runs on the same path
//!     coexist)

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use k2_core::session::{
    self, archive, Frame, SemanticKind, SessionEntry, SessionId,
};

fn tmp_project(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "k2so-archive-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn text_frame(s: &str) -> Frame {
    Frame::Text {
        bytes: s.as_bytes().to_vec(),
        style: None,
    }
}

fn read_archive_lines(path: &PathBuf) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().map(str::to_string).collect()
}

/// Decode a Frame::Text line and return its UTF-8 body. `None`
/// for other frame types. Frame::Text serializes `bytes` as a
/// JSON array of integers, so the substring `"first-run-1"`
/// doesn't appear literally in the archive line — we have to
/// deserialize and decode to check the content.
fn text_frame_body(line: &str) -> Option<String> {
    let frame: Frame = serde_json::from_str(line).ok()?;
    match frame {
        Frame::Text { bytes, .. } => String::from_utf8(bytes).ok(),
        _ => None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_and_publish_creates_ndjson_with_one_line_per_frame() {
    let session_id = SessionId::new();
    let project = tmp_project("basic");
    let entry = Arc::new(SessionEntry::new());

    let handle = archive::spawn(session_id, entry.clone(), project.clone());
    // Yield to let the writer open its file before we publish.
    tokio::time::sleep(Duration::from_millis(50)).await;

    for i in 0..5 {
        entry.publish(text_frame(&format!("line-{i}")));
    }

    // Close the sender by dropping all SessionEntry Arcs.
    drop(entry);

    // Writer exits on RecvError::Closed. Give it a deadline.
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    let archive_path = project
        .join(".k2/sessions")
        .join(session_id.to_string())
        .join("archive.ndjson");
    assert!(archive_path.exists(), "archive not created: {archive_path:?}");

    let lines = read_archive_lines(&archive_path);
    assert_eq!(lines.len(), 5, "expected 5 lines, got {}: {lines:?}", lines.len());

    // Each line round-trips back to its Frame.
    for (i, line) in lines.iter().enumerate() {
        let frame: Frame = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("line {i} bad JSON: {e}\n{line}"));
        match frame {
            Frame::Text { bytes, .. } => {
                let s = String::from_utf8(bytes).unwrap();
                assert_eq!(s, format!("line-{i}"));
            }
            other => panic!("line {i} wrong variant: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn archive_survives_across_multiple_writer_lifetimes() {
    // Two separate (session_id, entry) pairs writing to the SAME
    // project_root should each produce their own archive file
    // under their own session_id dir — no cross-contamination.
    let project = tmp_project("multi");
    let id_a = SessionId::new();
    let id_b = SessionId::new();
    let entry_a = Arc::new(SessionEntry::new());
    let entry_b = Arc::new(SessionEntry::new());

    let ha = archive::spawn(id_a, entry_a.clone(), project.clone());
    let hb = archive::spawn(id_b, entry_b.clone(), project.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;

    entry_a.publish(text_frame("from-a-1"));
    entry_a.publish(text_frame("from-a-2"));
    entry_b.publish(text_frame("from-b-1"));

    drop(entry_a);
    drop(entry_b);

    let _ = tokio::time::timeout(Duration::from_secs(2), ha).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), hb).await;

    let a_lines = read_archive_lines(
        &project.join(".k2/sessions").join(id_a.to_string()).join("archive.ndjson"),
    );
    let b_lines = read_archive_lines(
        &project.join(".k2/sessions").join(id_b.to_string()).join("archive.ndjson"),
    );
    assert_eq!(a_lines.len(), 2);
    assert_eq!(b_lines.len(), 1);
    assert_eq!(text_frame_body(&a_lines[0]).as_deref(), Some("from-a-1"));
    assert_eq!(text_frame_body(&a_lines[1]).as_deref(), Some("from-a-2"));
    assert_eq!(text_frame_body(&b_lines[0]).as_deref(), Some("from-b-1"));
}

#[tokio::test(flavor = "current_thread")]
async fn archive_appends_across_two_runs_on_same_session_id() {
    // If the daemon restarts mid-session (signal-kill + respawn),
    // the second writer should APPEND to the existing file, not
    // clobber it.
    let project = tmp_project("append");
    let session_id = SessionId::new();

    // First writer's lifetime.
    {
        let entry = Arc::new(SessionEntry::new());
        let handle = archive::spawn(session_id, entry.clone(), project.clone());
        tokio::time::sleep(Duration::from_millis(50)).await;
        entry.publish(text_frame("first-run-1"));
        entry.publish(text_frame("first-run-2"));
        drop(entry);
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }

    // Second writer, same session_id.
    {
        let entry = Arc::new(SessionEntry::new());
        let handle = archive::spawn(session_id, entry.clone(), project.clone());
        tokio::time::sleep(Duration::from_millis(50)).await;
        entry.publish(text_frame("second-run-1"));
        drop(entry);
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }

    let archive_path = project
        .join(".k2/sessions")
        .join(session_id.to_string())
        .join("archive.ndjson");
    let lines = read_archive_lines(&archive_path);
    assert_eq!(lines.len(), 3, "expected 3 total lines across 2 runs: {lines:?}");
    assert_eq!(text_frame_body(&lines[0]).as_deref(), Some("first-run-1"));
    assert_eq!(text_frame_body(&lines[1]).as_deref(), Some("first-run-2"));
    assert_eq!(text_frame_body(&lines[2]).as_deref(), Some("second-run-1"));
}

#[tokio::test(flavor = "current_thread")]
async fn semantic_event_frames_round_trip_through_archive() {
    let project = tmp_project("semantic");
    let session_id = SessionId::new();
    let entry = Arc::new(SessionEntry::new());

    let handle = archive::spawn(session_id, entry.clone(), project.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let semantic = Frame::SemanticEvent {
        kind: SemanticKind::ToolCall,
        payload: serde_json::json!({ "name": "bash", "id": "t_1" }),
    };
    entry.publish(semantic.clone());

    let cursor_op = Frame::CursorOp(k2_core::session::CursorOp::ClearScreen);
    entry.publish(cursor_op.clone());

    drop(entry);
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    let path = project
        .join(".k2/sessions")
        .join(session_id.to_string())
        .join("archive.ndjson");
    let lines = read_archive_lines(&path);
    assert_eq!(lines.len(), 2);

    let decoded_1: Frame = serde_json::from_str(&lines[0]).unwrap();
    let decoded_2: Frame = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(decoded_1, semantic);
    assert_eq!(decoded_2, cursor_op);
}

#[tokio::test(flavor = "current_thread")]
async fn empty_session_produces_empty_archive_file() {
    // Edge case: a session that registers and immediately drops
    // should produce an empty archive file (the writer ran, opened
    // the file, saw no frames, observed the sender drop, exited).
    let project = tmp_project("empty");
    let session_id = SessionId::new();
    let entry = Arc::new(SessionEntry::new());

    let handle = archive::spawn(session_id, entry.clone(), project.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;

    drop(entry);
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    let path = project
        .join(".k2/sessions")
        .join(session_id.to_string())
        .join("archive.ndjson");
    assert!(path.exists(), "even empty sessions should create the file");
    let bytes = std::fs::metadata(&path).unwrap().len();
    assert_eq!(bytes, 0, "empty session should produce zero bytes");
}

#[test]
fn byte_thresholds_match_expected_values() {
    // Post-G2 thresholds: per-segment rotation boundary is small
    // enough to keep individual files tailable/greppable; aggregate
    // cap is far larger than the MVP 500MB because rotation makes
    // the aggregate-use story sustainable. Locked so future phases
    // have to bump these deliberately.
    assert_eq!(session::ROTATE_BYTES, 50 * 1024 * 1024);
    assert_eq!(session::WARN_BYTES, 1_024 * 1024 * 1024);
    assert_eq!(session::HARD_LIMIT_BYTES, 5 * 1_024 * 1_024 * 1_024);
}

// ─────────────────────────────────────────────────────────────────────
// G2 — rotation + compact helpers
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn active_segment_rotates_past_boundary() {
    // Small ROTATE_BYTES would require writing 50MB+ of frames in a
    // test — too slow. Instead, set the active segment up with a
    // single ROTATE_BYTES+ pre-existing file on disk, then publish
    // one more frame and assert that the NEXT write triggers
    // rotation: archive.ndjson shrinks back to just the new frame,
    // archive.000.ndjson has the old bytes.
    let project = tmp_project("rotate");
    let session_id = SessionId::new();
    let archive_dir = project
        .join(".k2/sessions")
        .join(session_id.to_string());
    std::fs::create_dir_all(&archive_dir).unwrap();
    let active = archive_dir.join("archive.ndjson");

    // Seed the active file with a single fake frame line that's
    // already bigger than ROTATE_BYTES. Use a tiny padding bytes
    // field so the JSON parses as a real Frame::Text.
    let padding_size = (archive::ROTATE_BYTES as usize) + 1_024;
    let pad_bytes: Vec<u8> = vec![b'a'; padding_size];
    let frame = text_frame(std::str::from_utf8(&pad_bytes).unwrap());
    let mut fat_line = serde_json::to_vec(&frame).unwrap();
    fat_line.push(b'\n');
    std::fs::write(&active, &fat_line).unwrap();

    // Now spin up the writer; it sees bytes_in_current > ROTATE_BYTES
    // already, so the very next write triggers rotation.
    let entry = Arc::new(SessionEntry::new());
    let handle = archive::spawn(session_id, entry.clone(), project.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;

    entry.publish(text_frame("post-rotation"));
    drop(entry);
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    // archive.ndjson now holds only the "post-rotation" frame.
    let active_lines = read_archive_lines(&active);
    assert_eq!(
        active_lines.len(),
        1,
        "active segment should have exactly one line after rotation: {active_lines:?}"
    );
    assert_eq!(
        text_frame_body(&active_lines[0]).as_deref(),
        Some("post-rotation")
    );

    // archive.000.ndjson exists and holds the pre-rotation bytes.
    let rotated = archive_dir.join("archive.000.ndjson");
    assert!(rotated.exists(), "rotated segment should exist: {rotated:?}");
    let rotated_size = std::fs::metadata(&rotated).unwrap().len();
    assert!(
        rotated_size > archive::ROTATE_BYTES,
        "rotated segment carries the pre-rotation bytes ({rotated_size} bytes)"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rotation_index_increments_across_multiple_rotations() {
    // Seed TWO pre-rotated segments; the writer should skip those
    // indices and use 002 for its own next rotation.
    let project = tmp_project("rotate-multi");
    let session_id = SessionId::new();
    let archive_dir = project
        .join(".k2/sessions")
        .join(session_id.to_string());
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(archive_dir.join("archive.000.ndjson"), b"old-0\n").unwrap();
    std::fs::write(archive_dir.join("archive.001.ndjson"), b"old-1\n").unwrap();

    // Seed active segment over ROTATE_BYTES so next publish rotates.
    let fat = vec![b'b'; (archive::ROTATE_BYTES as usize) + 1_024];
    let frame = text_frame(std::str::from_utf8(&fat).unwrap());
    let mut fat_line = serde_json::to_vec(&frame).unwrap();
    fat_line.push(b'\n');
    std::fs::write(archive_dir.join("archive.ndjson"), &fat_line).unwrap();

    let entry = Arc::new(SessionEntry::new());
    let handle = archive::spawn(session_id, entry.clone(), project.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;
    entry.publish(text_frame("bump"));
    drop(entry);
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    // 000 + 001 preserved, 002 newly rotated (the pre-seeded fat
    // active segment moves to index 002).
    assert!(archive_dir.join("archive.000.ndjson").exists());
    assert!(archive_dir.join("archive.001.ndjson").exists());
    assert!(archive_dir.join("archive.002.ndjson").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn aggregate_hard_limit_freezes_writes_but_keeps_session_alive() {
    // Simulate an archive dir whose aggregate bytes already exceed
    // HARD_LIMIT_BYTES. The writer should open successfully but
    // refuse to write any further frames — the `frozen` state is
    // set on startup from the aggregate size read off disk.
    //
    // Using a real 5GB seed file would wreck CI; instead we monkey-
    // patch by creating a file whose reported size is artificially
    // large via sparse-file truncation.
    let project = tmp_project("aggregate-freeze");
    let session_id = SessionId::new();
    let archive_dir = project
        .join(".k2/sessions")
        .join(session_id.to_string());
    std::fs::create_dir_all(&archive_dir).unwrap();

    // Sparse file: set_len grows the logical size without actually
    // allocating blocks. read_dir → metadata().len() reports the
    // logical size, which is what compute_aggregate_bytes sums.
    let fake_old = archive_dir.join("archive.999.ndjson");
    let f = std::fs::File::create(&fake_old).unwrap();
    f.set_len(archive::HARD_LIMIT_BYTES + 1).unwrap();
    drop(f);

    let entry = Arc::new(SessionEntry::new());
    let handle = archive::spawn(session_id, entry.clone(), project.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;
    entry.publish(text_frame("should-not-appear"));
    drop(entry);
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    // Active segment is empty — writer was frozen from startup.
    let active = archive_dir.join("archive.ndjson");
    let bytes = std::fs::metadata(&active).unwrap().len();
    assert_eq!(
        bytes, 0,
        "active segment must stay empty when aggregate cap is already exceeded"
    );
}

#[test]
fn parse_rotation_index_matches_expected_patterns() {
    assert_eq!(archive::parse_rotation_index("archive.000.ndjson"), Some(0));
    assert_eq!(archive::parse_rotation_index("archive.007.ndjson"), Some(7));
    assert_eq!(
        archive::parse_rotation_index("archive.042.ndjson.gz"),
        Some(42)
    );
    assert_eq!(archive::parse_rotation_index("archive.999.ndjson"), Some(999));

    // Non-matches.
    assert_eq!(archive::parse_rotation_index("archive.ndjson"), None);
    assert_eq!(archive::parse_rotation_index("archive.foo.ndjson"), None);
    assert_eq!(archive::parse_rotation_index("not-an-archive.000.ndjson"), None);
    assert_eq!(archive::parse_rotation_index("archive.000.txt"), None);
    assert_eq!(
        archive::parse_rotation_index("archive.001.ndjson.zstd"),
        None
    );
}

#[test]
fn rotated_uncompressed_segments_skips_active_and_gzipped() {
    let project = tmp_project("compact-enumeration");
    let session_id = SessionId::new();
    let dir = project
        .join(".k2/sessions")
        .join(session_id.to_string());
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(dir.join("archive.ndjson"), b"active\n").unwrap();
    std::fs::write(dir.join("archive.000.ndjson.gz"), b"gzipped").unwrap();
    std::fs::write(dir.join("archive.001.ndjson"), b"old\n").unwrap();
    std::fs::write(dir.join("archive.002.ndjson"), b"old\n").unwrap();

    let segments = archive::rotated_uncompressed_segments(&dir).unwrap();
    let names: Vec<_> = segments
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["archive.001.ndjson".to_string(), "archive.002.ndjson".to_string()]
    );
}
