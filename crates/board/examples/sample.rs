//! Writes a small sample file, for checking that stock zip tools read what we write.
use k2_mpc_board::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: sample <path>")?;
    let mut parts = MemParts::new();
    let h = parts.add_blob("image/png", b"image bytes".to_vec());
    parts.set_preview(b"a preview".to_vec());
    parts.add(
        "extra/data.bin",
        "future",
        "application/x-y",
        b"kept".to_vec(),
        Fields::new(),
    );
    let text = format!(
        "{{\"frames\":[],\"objects\":[{{\"id\":\"i\",\"src\":{{\"blob\":\"{h}\"}},\"type\":\"image\"}}],\"people\":{{}},\"settings\":{{\"title\":\"{}\"}}}}",
        "sample ".repeat(20)
    );
    let board = Board::from_json(text.as_bytes(), &Limits::V1)?;
    let mut out = Vec::new();
    write(
        &mut out,
        &WriteMeta::new(Lineage::from_bytes([1; 16])),
        &board,
        &mut parts,
        &Limits::V1,
    )?;
    std::fs::write(path, out)?;
    Ok(())
}
