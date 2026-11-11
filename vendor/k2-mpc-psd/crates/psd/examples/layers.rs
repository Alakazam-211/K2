//! Lists a PSD or PSB file's layers as the pack indexes them: `cargo run -p k2-mpc-psd --example
//! layers -- FILE.psd`. The README shows this file.

use std::fs::File;

use k2_mpc_base::index::{IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_base::session::BlobPack;
use k2_mpc_psd::PsdPack;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: layers FILE.psd")?;
    let mut file = File::open(&path)?;
    let size = file.metadata()?.len();
    // the bounds a host's kit config sets: most nodes, deepest nesting, stored bytes, text bytes
    let limits = IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX);
    let draft = PsdPack.index(&mut file, size, &limits)?;
    for node in &draft.nodes {
        let key = node.key.as_deref().unwrap_or("(the kit mints one)");
        println!("{key} {} {}", node.kind, node.name);
    }
    Ok(())
}
