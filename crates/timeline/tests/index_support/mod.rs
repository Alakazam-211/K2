//! Rows on the structure index, each returning `Err("<row>: ...")` so a planted bug can say which
//! row caught it: shared by `tests/pack.rs` and the planted bugs (`tests/mutants.rs`).
#![allow(dead_code)]

use k2_mpc_base::index::{IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_base::session::BlobPack;
use k2_mpc_timeline::pack::FcpxmlPack;

const FCPXML: &str = include_str!("../data/seed.fcpxml");

/// An FCPXML sequence at 25 fps that says `tcFormat="DF"` is labelled non-drop: 25 fps has no
/// drop-frame form (drop-frame counts only at `30000/1001` and `60000/1001`), so the flag
/// changes nothing. The project's first clip, `x`, starts at its sequence's start timecode, ten
/// seconds: `start_tc` `00:00:10:00`; and the sequence's `drop_frame` is `false`, as the
/// timecodes are.
pub fn row_df_at_25_fps_is_non_drop() -> Result<(), String> {
    const ROW: &str = "row_df_at_25_fps_is_non_drop";
    let df = FCPXML.replace(
        r#"tcStart="10s" duration="9s""#,
        r#"tcStart="10s" tcFormat="DF" duration="9s""#,
    );
    if df == FCPXML {
        return Err(format!(
            "{ROW}: control: the project doesn't say tcFormat=\"DF\""
        ));
    }
    let limits = IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX);
    let d = FcpxmlPack
        .index(&mut df.as_bytes(), df.len() as u64, &limits)
        .map_err(|e| format!("{ROW}: control: the index: {e:?}"))?;
    let clip = d
        .nodes
        .iter()
        .find(|n| n.kind == "clip" && n.name == "x")
        .ok_or_else(|| format!("{ROW}: control: no clip x"))?;
    let tc = clip.attrs.get("start_tc").map(String::as_str);
    if tc != Some("00:00:10:00") {
        return Err(format!(
            "{ROW}: at 25 fps, DF: clip x's start_tc is {tc:?}, want \"00:00:10:00\""
        ));
    }
    let seq = d
        .nodes
        .iter()
        .find(|n| n.kind == "sequence" && n.key.as_deref() == Some("@project:P1"))
        .ok_or_else(|| format!("{ROW}: control: no sequence @project:P1"))?;
    let flag = seq.attrs.get("drop_frame").map(String::as_str);
    if flag != Some("false") {
        return Err(format!(
            "{ROW}: at 25 fps, DF: the sequence's drop_frame is {flag:?}, want \"false\""
        ));
    }
    Ok(())
}
