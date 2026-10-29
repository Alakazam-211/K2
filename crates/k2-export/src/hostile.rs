//! TEST ONLY (feature `test-hostile`): exporters that misbehave on purpose,
//! so the host's limits are proven against a real process (tests/limits.rs,
//! the headless export cases). Never in a release build: the helper-app
//! verify refuses a `k2-export` carrying [`MARKER`].

use std::time::Duration;

use k2_mpc_base::export::{ExportCtl, ExportError, ExportReport, ExportTarget, Exporter, Fidelity, Options, Sink, Source, SourceKind};

/// In every hostile build's bytes; release checks refuse it.
pub const MARKER: &str = "K2_EXPORT_TEST_HOSTILE_ENABLED";

const fn t(id: &'static str) -> ExportTarget {
    ExportTarget::new(id, "application/octet-stream", "bin", Fidelity::Lossless, &[])
}

/// Every hostile target.
pub const TARGETS: &[ExportTarget] = &[
    t("hostile-panic"),
    t("hostile-abort"),
    t("hostile-alloc"),
    t("hostile-sleep"),
    t("hostile-spin"),
    t("hostile-flood"),
    t("hostile-exit"),
    t("hostile-decode"),
    t("hostile-slow"),
    t("hostile-malloc"),
    t("hostile-env"),
];

pub struct Hostile;

impl Exporter for Hostile {
    fn id(&self) -> &'static str {
        // The marker rides in the id, so it is in the binary for sure.
        "k2-export-hostile@0 K2_EXPORT_TEST_HOSTILE_ENABLED"
    }

    fn targets(&self, _: &SourceKind) -> Vec<ExportTarget> {
        TARGETS.to_vec()
    }

    fn export(
        &self,
        src: Source<'_>,
        target: &str,
        _: &Options,
        out: &mut dyn Sink,
        ctl: &ExportCtl<'_>,
    ) -> Result<ExportReport, ExportError> {
        let _ = MARKER;
        match target {
            "hostile-panic" => panic!("hostile exporter panicked"),
            "hostile-abort" => std::process::abort(),
            "hostile-alloc" => {
                let mut keep: Vec<Vec<u8>> = Vec::new();
                loop {
                    // Touched, so the pages are really in use.
                    keep.push(vec![7u8; 16 << 20]);
                }
            }
            "hostile-sleep" => loop {
                std::thread::sleep(Duration::from_secs(3600));
            },
            "hostile-spin" => {
                let mut x: u64 = 0;
                loop {
                    x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
                }
            }
            "hostile-flood" => {
                let chunk = vec![b'x'; 64 << 10];
                loop {
                    out.write_all(&chunk)?;
                }
            }
            // Memory outside Rust's allocator (as a native decoder's would be):
            // Linux's RLIMIT_AS makes malloc fail; macOS's footprint watchdog
            // kills the worker.
            "hostile-malloc" => loop {
                // SAFETY: a fresh allocation, written in full, never freed.
                let p = unsafe { libc::malloc(16 << 20) } as *mut u8;
                if p.is_null() {
                    return Err(ExportError::Limit("memory"));
                }
                // SAFETY: p points at 16 MiB just allocated.
                unsafe { std::ptr::write_bytes(p, 7, 16 << 20) };
            },
            "hostile-exit" => std::process::exit(0),
            // The names of every environment variable it got (one a line).
            "hostile-env" => {
                for (k, _) in std::env::vars_os() {
                    out.write_all(k.to_string_lossy().as_bytes())?;
                    out.write_all(b"\n")?;
                }
                Ok(ExportReport::default())
            }
            "hostile-decode" => Err(ExportError::Decode("hostile: not this format".into())),
            // Reads the input slowly, reporting progress: for cancel and progress.
            "hostile-slow" => {
                if let Source::Bytes { read, size, .. } = src {
                    let mut buf = [0u8; 1];
                    let mut done = 0u64;
                    loop {
                        std::thread::sleep(Duration::from_millis(100));
                        let n = read.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        out.write_all(&buf[..n])?;
                        done += n as u64;
                        (ctl.progress)(done, size);
                    }
                }
                Ok(ExportReport::default())
            }
            _ => Err(ExportError::Unsupported { kind: "any".into(), target: target.into() }),
        }
    }
}
