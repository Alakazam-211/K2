//! `cargo run -p k2-core --bin contract-gen [-- --check]`
//!
//! Writes every file generated from `crates/k2-core/src/contract/catalog.json`
//! (prd-zen-user-widgets-v2 UWA11): the renderer's verb and cap tables, the
//! frame's `k2.d.ts` and `k2-frame.js`, the freshness snapshot, the fenced
//! `k2 zen guide api` and `k2 study app-api` blocks in `cli/k2`, the App
//! route table `app_surface.rs` and the server SDK's `verbs.ts` (S23). `--check` writes nothing and
//! exits 1 when any output is stale. See `k2_core::contract::gen`.

use std::path::PathBuf;
use std::process::ExitCode;

use k2_core::contract::{catalog, gen};

/// The part of `cli/k2` that holds the line `begin`, when `cli/k2` is built
/// from parts: the files `cli/src/ORDER` lists, one name per line.
/// `Ok(None)` when there is no `cli/src/ORDER` (cli/k2 is edited directly).
fn cli_part_with(root: &std::path::Path, begin: &str) -> Result<Option<PathBuf>, String> {
    let src = root.join("cli/src");
    let Ok(order) = std::fs::read_to_string(src.join("ORDER")) else {
        return Ok(None);
    };
    for name in order.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let part = src.join(name);
        let text = std::fs::read_to_string(&part).map_err(|e| format!("read cli/src/{name}: {e}"))?;
        if text.contains(begin) {
            return Ok(Some(part));
        }
    }
    Err(format!("no part listed in cli/src/ORDER has the line '{begin}'"))
}

fn main() -> ExitCode {
    let check = std::env::args().skip(1).any(|a| a == "--check");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime_path = root.join(gen::RUNTIME_JS);
    let runtime = if runtime_path.exists() {
        Some(std::fs::read_to_string(&runtime_path).unwrap_or_else(|e| panic!("read {}: {e}", gen::RUNTIME_JS)))
    } else {
        None
    };
    let outs = match gen::render(catalog(), runtime.as_deref()) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("contract-gen: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut stale = Vec::new();
    for o in outs {
        let (file, fence) = gen::file_of(o.path);
        // A block inside cli/k2 is written to cli/k2 and, when cli/k2 is
        // built from parts (scripts/build-cli.sh), to the part that holds
        // the block, so the join of the parts stays equal to cli/k2.
        let mut targets = vec![root.join(file)];
        if let (Some(begin), true) = (fence, file == gen::CLI) {
            match cli_part_with(&root, begin) {
                Ok(Some(part)) => targets.push(part),
                Ok(None) => {}
                Err(e) => {
                    eprintln!("contract-gen: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        let mut changed = false;
        for path in targets {
            let shown = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
            // Two blocks can share one file (cli/k2): read what the last
            // write left there.
            let current = std::fs::read_to_string(&path).ok();
            let next = if let Some(begin) = fence {
                let cli = current.clone().unwrap_or_else(|| panic!("{shown} is missing"));
                match gen::splice_fenced(&cli, begin, &o.text) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("contract-gen: {e} (in {shown}). Add the two marker lines where the page belongs.");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                o.text.clone()
            };
            if current.as_deref() == Some(next.as_str()) {
                continue;
            }
            changed = true;
            if !check {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("mkdir {}: {e}", dir.display()));
                }
                std::fs::write(&path, next).unwrap_or_else(|e| panic!("write {shown}: {e}"));
                println!("wrote {} ({shown})", o.path);
            }
        }
        if changed {
            stale.push(o.path);
        }
    }
    if check && !stale.is_empty() {
        eprintln!("contract-gen --check: stale {stale:?}; {}", gen::REGEN);
        return ExitCode::FAILURE;
    }
    if stale.is_empty() {
        println!("contract-gen: every output is fresh");
    }
    ExitCode::SUCCESS
}
