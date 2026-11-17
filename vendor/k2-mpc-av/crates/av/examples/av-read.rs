//! `av-read CONFIG DIR [--full]`: reads every DXF drawing under `DIR` (its subfolders too) as one
//! set, with the configuration file `CONFIG`, and prints the set's reading: by default in counts
//! and finding codes only, never a value ([`k2_mpc_av::set::summary`]); with `--full`, every row
//! (identifiers, labels and sheet numbers included: for the machine the set is on only). Each
//! drawing is named by its path under `DIR`. A drawing that doesn't read is named with its
//! refusal's id and left out. Test tooling, run by hand; never shipped.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn dxfs(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            dxfs(&p, out)?;
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dxf")) {
            out.push(p);
        }
    }
    Ok(())
}

fn run(config: &Path, dir: &Path, full: bool) -> Result<(), String> {
    let text = std::fs::read_to_string(config).map_err(|e| format!("{}: {e}", config.display()))?;
    let cfg = k2_mpc_av::Config::parse(&text).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    dxfs(dir, &mut files).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut sheets = Vec::new();
    let mut unreadable = 0;
    for p in &files {
        let name = p
            .strip_prefix(dir)
            .unwrap_or(p)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = std::fs::read(p).map_err(|e| format!("{name}: {e}"))?;
        match k2_mpc_dxf::read_doc(&bytes, &k2_mpc_dxf::Limits::default()) {
            Ok(doc) => sheets.push(k2_mpc_av::Sheet { name, doc }),
            Err(e) => {
                unreadable += 1;
                if full {
                    println!("unreadable {name} {}", e.code.id);
                } else {
                    println!("unreadable {}", e.code.id);
                }
            }
        }
    }
    println!(
        "drawings {} read {} unreadable {unreadable}",
        files.len(),
        sheets.len()
    );
    let out = k2_mpc_av::read_set(&cfg, &sheets);
    if full {
        print!("{}", out.stdout);
        println!("{}", out.data.to_json());
    } else {
        print!("{}", k2_mpc_av::set::summary(&out.data));
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [config, dir] => run(Path::new(config), Path::new(dir), false),
        [config, dir, "--full"] => run(Path::new(config), Path::new(dir), true),
        _ => Err("usage: av-read CONFIG DIR [--full]".into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("av-read: {e}");
            ExitCode::FAILURE
        }
    }
}
