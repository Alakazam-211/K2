//! Exports: the contract between a host and the code that turns a doc's version into another
//! format (a PNG, a PDF, an XLSX, …). The kit holds only the contract, a registry and the
//! identity exporter ([`AsIs`]); format exporters live outside it, and a host registers them.
//!
//! - **An export never changes the doc.** It reads one version (the host gets the bytes from
//!   [`crate::session::Session::file_at`] / [`crate::session::Session::file_now`] for a live
//!   doc, or [`crate::session::BlobSession::read_version`] for a tier-0 file) and writes to a
//!   [`Sink`]; it takes no lease, makes no version and ignores claims.
//! - **Reproducible:** a pure exporter gives the same bytes for the same (version, target,
//!   options, exporter id).
//! - **Honest:** every export returns an [`ExportReport`] saying what was lost.
//! - **Bounded:** [`ExportCtl`] carries the limits, a cancel flag and a progress callback;
//!   [`ExportRegistry::export`] checks the options against the target's [`OptionSpec`]s,
//!   honours a cancel set before the call, and turns a panicking exporter into
//!   [`ExportError::Internal`] (no panic escapes).
//!
//! K2-agnostic: nothing here runs processes, opens files or knows a host. The host runs an
//! export where it chooses (a worker process for hostile decoders), names the output and
//! places it.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// How faithful a target is to its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Fidelity {
    /// Everything the source holds survives (a re-encoding, or the bytes themselves).
    Lossless,
    /// Content survives with some loss (a JPEG, a dialect without the source's quoting).
    Lossy,
    /// Structure is flattened (layers merged, a table to text).
    Flatten,
    /// Rendered to pixels or pages (a document as PNG, a sheet as PDF).
    Render,
}

/// What an option takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OptionKind {
    /// `true` or `false`.
    Bool,
    /// A whole number in `min..=max`.
    Int { min: i64, max: i64 },
    /// One of these words.
    Choice(&'static [&'static str]),
    /// Any text (at most [`Options::MAX_VALUE`] bytes).
    Text,
}

/// One documented option of a target (`quality`, `dpi`, `pages`, `sheet`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct OptionSpec {
    pub name: &'static str,
    pub kind: OptionKind,
    /// The value used when the caller gives none (`None`: the option is optional and unset).
    pub default: Option<&'static str>,
    /// One line for `--list` and help.
    pub doc: &'static str,
}

impl OptionSpec {
    pub const fn new(
        name: &'static str,
        kind: OptionKind,
        default: Option<&'static str>,
        doc: &'static str,
    ) -> Self {
        OptionSpec {
            name,
            kind,
            default,
            doc,
        }
    }

    /// Whether `v` is a value of this option.
    pub fn accepts(&self, v: &str) -> bool {
        match self.kind {
            OptionKind::Bool => v == "true" || v == "false",
            OptionKind::Int { min, max } => {
                v.parse::<i64>().is_ok_and(|n| (min..=max).contains(&n))
            }
            OptionKind::Choice(words) => words.contains(&v),
            OptionKind::Text => v.len() <= Options::MAX_VALUE,
        }
    }
}

/// A target an exporter can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExportTarget {
    /// Stable id: `png`, `pdf`, `docx`, `csv`, `as-is`, …
    pub id: &'static str,
    /// `image/png`; `application/octet-stream` where it follows the source.
    pub media_type: &'static str,
    /// Without the dot; empty: the source's own.
    pub extension: &'static str,
    pub fidelity: Fidelity,
    pub options: &'static [OptionSpec],
}

impl ExportTarget {
    pub const fn new(
        id: &'static str,
        media_type: &'static str,
        extension: &'static str,
        fidelity: Fidelity,
        options: &'static [OptionSpec],
    ) -> Self {
        ExportTarget {
            id,
            media_type,
            extension,
            fidelity,
            options,
        }
    }
}

/// What is being exported, as an exporter matches it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SourceKind {
    /// A live doc of this pack (its schema name: `markdown`, `text`, `csv`, …).
    Pack(String),
    /// A tier-0 file, by its sniffed or declared format (`png`, `pdf`, …; lower case).
    Format(String),
}

/// The version being exported.
#[non_exhaustive]
pub enum Source<'a> {
    /// The file's bytes at that version (a live doc's [`crate::session::Session::file_at`], or
    /// a tier-0 version's verified reader). `name` is the file's name, for messages and for
    /// formats that sniff by extension.
    Bytes {
        read: &'a mut dyn Read,
        size: u64,
        name: &'a str,
    },
    /// A live text doc's roots at that version ([`crate::session::Session::show`]): `pack` is
    /// its schema name.
    Text {
        pack: &'a str,
        roots: &'a [(String, String)],
    },
}

/// The caller's options, `name → value`, checked against the target's [`OptionSpec`]s by
/// [`ExportRegistry::export`] (unknown names and bad values are refused before any work).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    values: BTreeMap<String, String>,
}

impl Options {
    /// The longest value accepted, in bytes.
    pub const MAX_VALUE: usize = 4096;

    pub fn new() -> Self {
        Options::default()
    }

    /// Parse `k=v` pairs (`--option k=v` on a CLI). A pair without `=` or with an empty name
    /// is an error naming it; a repeated name keeps the last value.
    pub fn parse<'s>(pairs: impl IntoIterator<Item = &'s str>) -> Result<Options, ExportError> {
        let mut o = Options::new();
        for p in pairs {
            match p.split_once('=') {
                Some((k, v)) if !k.is_empty() => o.set(k, v),
                _ => {
                    return Err(ExportError::Declined {
                        code: "bad_option",
                        message: format!("not NAME=VALUE: {p:?}"),
                    })
                }
            }
        }
        Ok(o)
    }

    pub fn set(&mut self, name: &str, value: &str) {
        self.values.insert(name.to_owned(), value.to_owned());
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// The value of `spec`: the caller's, else its default.
    pub fn value<'s>(&'s self, spec: &OptionSpec) -> Option<&'s str> {
        self.get(spec.name).or(spec.default)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Bounds on one export, enforced by the exporter before and while it works (a decoder
/// checks dimensions against `max_pixels` before allocating).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExportLimits {
    pub max_input_bytes: u64,
    pub max_output_bytes: u64,
    pub max_pixels: u64,
    pub max_pages: u32,
    /// Wall time, for the host to enforce (the kit has no clock here).
    pub max_wall_ms: u64,
}

impl Default for ExportLimits {
    /// 1 GiB in, 1 GiB out, 256 megapixels, 10,000 pages, 10 minutes.
    fn default() -> Self {
        ExportLimits {
            max_input_bytes: 1 << 30,
            max_output_bytes: 1 << 30,
            max_pixels: 256 * 1024 * 1024,
            max_pages: 10_000,
            max_wall_ms: 600_000,
        }
    }
}

/// Read-only resources the host provides (fonts, linked images, colour profiles). An
/// exporter never scans the system for them (determinism).
pub trait Resources: Send + Sync {
    /// A font by family name.
    fn font(&self, family: &str) -> Option<Vec<u8>> {
        let _ = family;
        None
    }
    /// A file the source links to, by the name the source uses.
    fn linked(&self, name: &str) -> Option<Vec<u8>> {
        let _ = name;
        None
    }
}

/// No resources.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoResources;

impl Resources for NoResources {}

/// One export's controls.
pub struct ExportCtl<'a> {
    /// Set by the host to stop; the exporter checks it at least once per unit of work and
    /// returns [`ExportError::Cancelled`].
    pub cancel: &'a AtomicBool,
    /// `(done, total)` in the exporter's units (bytes, pages).
    pub progress: &'a dyn Fn(u64, u64),
    pub limits: ExportLimits,
    pub resources: &'a dyn Resources,
}

impl ExportCtl<'_> {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// [`ExportError::Cancelled`] if the host asked to stop.
    pub fn check(&self) -> Result<(), ExportError> {
        if self.is_cancelled() {
            Err(ExportError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Where an export writes. The host calls [`Sink::finish`] only after the export returned
/// `Ok` (a temp file renamed into place); after an error it drops the sink and nothing is
/// placed.
pub trait Sink: Write {
    fn finish(self: Box<Self>) -> io::Result<()>;
}

/// A sink into memory (tests, small exports), with the bytes written so far.
#[derive(Debug, Default)]
pub struct VecSink {
    pub bytes: Vec<u8>,
}

impl Write for VecSink {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Sink for VecSink {
    fn finish(self: Box<Self>) -> io::Result<()> {
        Ok(())
    }
}

/// Something the export could not keep.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Loss {
    /// Stable, short: `layers-flattened`, `font-substituted`, `colour-converted`, …
    pub kind: &'static str,
    /// For people: what, where.
    pub detail: String,
}

impl Loss {
    pub fn new(kind: &'static str, detail: impl Into<String>) -> Self {
        Loss {
            kind,
            detail: detail.into(),
        }
    }
}

/// What an export did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExportReport {
    pub losses: Vec<Loss>,
    pub notes: Vec<String>,
    /// Pages written, for paged targets.
    pub pages: Option<u32>,
    /// Bytes written to the sink.
    pub bytes: u64,
}

/// Why an export failed. Nothing was placed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ExportError {
    /// No registered exporter writes this target for this source.
    #[error("no exporter writes {target} from {kind}")]
    Unsupported { kind: String, target: String },
    /// The exporter refused this input or these options, with a stable code.
    #[error("{message} ({code})")]
    Declined { code: &'static str, message: String },
    /// A limit of [`ExportLimits`] would be passed (its name).
    #[error("export limit: {0}")]
    Limit(&'static str),
    #[error("export cancelled")]
    Cancelled,
    /// The source could not be read in its format.
    #[error("could not read the source: {0}")]
    Decode(String),
    #[error("export i/o: {0}")]
    Io(#[from] io::Error),
    /// The exporter failed in a way it shouldn't (a panic, caught): a bug to report.
    #[error("exporter {exporter} failed: {message}")]
    Internal { exporter: String, message: String },
}

impl ExportError {
    /// The exit code (the kit's table, [`crate::tools::exit`]): 2 for what the caller can
    /// change (target, options, limits), 1 otherwise.
    pub fn exit(&self) -> u8 {
        match self {
            ExportError::Unsupported { .. }
            | ExportError::Declined { .. }
            | ExportError::Limit(_) => 2,
            ExportError::Cancelled
            | ExportError::Decode(_)
            | ExportError::Io(_)
            | ExportError::Internal { .. } => 1,
        }
    }
}

impl crate::errors::Refusal for ExportError {
    fn report(&self) -> crate::errors::ErrorReport {
        use crate::errors::ids as r;
        use crate::errors::ErrorReport as R;
        let msg = self.to_string();
        match self {
            ExportError::Unsupported { kind, target } => R::new(&r::EXPORT_UNSUPPORTED, msg)
                .with("kind", kind.as_str())
                .with("target", target.as_str()),
            ExportError::Declined { code, .. } => {
                R::new(&r::EXPORT_DECLINED, msg).with("code", *code)
            }
            ExportError::Limit(l) => R::new(&r::EXPORT_LIMIT, msg).with("limit", *l),
            ExportError::Cancelled => R::new(&r::EXPORT_CANCELLED, msg),
            ExportError::Decode(_) => R::new(&r::EXPORT_DECODE, msg),
            ExportError::Io(_) => R::new(&r::EXPORT_IO, msg),
            ExportError::Internal { exporter, .. } => {
                R::new(&r::EXPORT_INTERNAL, msg).with("exporter", exporter.as_str())
            }
        }
    }
}

/// A format exporter (outside the kit; [`AsIs`] is the kit's only one). `Send + Sync`: the
/// host may run exports on worker threads.
pub trait Exporter: Send + Sync {
    /// Stable id with the exporter's version: `k2-export-image@0.1.0`.
    fn id(&self) -> &'static str;
    /// The targets it writes from `kind` (empty: none).
    fn targets(&self, kind: &SourceKind) -> Vec<ExportTarget>;
    /// Write `src` as `target` into `out`. Options were checked against the target's specs.
    fn export(
        &self,
        src: Source<'_>,
        target: &str,
        opts: &Options,
        out: &mut dyn Sink,
        ctl: &ExportCtl<'_>,
    ) -> Result<ExportReport, ExportError>;
}

/// Why a registration was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    #[error("an exporter with id {0} is already registered")]
    DuplicateId(String),
}

/// The exporters a host offers, in registration order. For a (source, target) the first
/// exporter that lists the target wins.
#[derive(Clone, Default)]
pub struct ExportRegistry {
    exporters: Vec<Arc<dyn Exporter>>,
}

impl fmt::Debug for ExportRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.exporters.iter().map(|e| e.id()))
            .finish()
    }
}

impl ExportRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        ExportRegistry::default()
    }

    /// A registry with the kit's [`AsIs`] exporter.
    pub fn with_as_is() -> Self {
        let mut r = ExportRegistry::new();
        r.register(Arc::new(AsIs)).expect("an empty registry");
        r
    }

    pub fn register(&mut self, e: Arc<dyn Exporter>) -> Result<(), RegistryError> {
        if self.exporters.iter().any(|x| x.id() == e.id()) {
            return Err(RegistryError::DuplicateId(e.id().to_owned()));
        }
        self.exporters.push(e);
        Ok(())
    }

    /// Every target offered for `kind`, each once (the first exporter's), with the id of the
    /// exporter that writes it.
    pub fn targets_for(&self, kind: &SourceKind) -> Vec<(&'static str, ExportTarget)> {
        let mut out: Vec<(&'static str, ExportTarget)> = Vec::new();
        for e in &self.exporters {
            for t in e.targets(kind) {
                if !out.iter().any(|(_, o)| o.id == t.id) {
                    out.push((e.id(), t));
                }
            }
        }
        out
    }

    /// Export `src` (of `kind`) as `target`: the first exporter offering it runs, after the
    /// options are checked (an unknown name or a bad value: [`ExportError::Declined`]
    /// `bad_option`, nothing written) and a cancel set before the call is honoured. Output
    /// past [`ExportLimits::max_output_bytes`] is refused at the sink and the export fails
    /// with [`ExportError::Limit`] `output bytes`. A panic in the exporter is caught and
    /// returned as [`ExportError::Internal`]. The report's `bytes` is what reached `out`.
    pub fn export(
        &self,
        kind: &SourceKind,
        src: Source<'_>,
        target: &str,
        opts: &Options,
        out: &mut dyn Sink,
        ctl: &ExportCtl<'_>,
    ) -> Result<ExportReport, ExportError> {
        let found = self.exporters.iter().find_map(|e| {
            e.targets(kind)
                .into_iter()
                .find(|t| t.id == target)
                .map(|t| (e, t))
        });
        let Some((e, t)) = found else {
            return Err(ExportError::Unsupported {
                kind: kind_text(kind),
                target: target.to_owned(),
            });
        };
        for (name, value) in opts.iter() {
            match t.options.iter().find(|s| s.name == name) {
                None => {
                    return Err(ExportError::Declined {
                        code: "bad_option",
                        message: format!("{target} has no option {name:?}"),
                    })
                }
                Some(s) if !s.accepts(value) => {
                    return Err(ExportError::Declined {
                        code: "bad_option",
                        message: format!("{target}: {value:?} is not a value of {name}"),
                    })
                }
                Some(_) => {}
            }
        }
        ctl.check()?;
        let mut counted = Counting {
            inner: out,
            n: 0,
            max: ctl.limits.max_output_bytes,
            over: false,
        };
        let res = catch_unwind(AssertUnwindSafe(|| {
            e.export(src, target, opts, &mut counted, ctl)
        }));
        let (n, over) = (counted.n, counted.over);
        match res {
            Ok(Ok(mut report)) => {
                report.bytes = n;
                Ok(report)
            }
            // the output limit, however the exporter passed the sink's refusal on
            Ok(Err(_)) if over => Err(ExportError::Limit("output bytes")),
            Ok(Err(err)) => Err(err),
            Err(panic) => Err(ExportError::Internal {
                exporter: e.id().to_owned(),
                message: panic_text(&panic),
            }),
        }
    }
}

fn kind_text(k: &SourceKind) -> String {
    match k {
        SourceKind::Pack(p) => format!("a {p} doc"),
        SourceKind::Format(f) => format!("a {f} file"),
    }
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_owned()
    }
}

/// A sink wrapper that counts bytes and refuses to pass `max` (the output limit).
struct Counting<'a> {
    inner: &'a mut dyn Sink,
    n: u64,
    max: u64,
    over: bool,
}

impl Write for Counting<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.n.saturating_add(b.len() as u64) > self.max {
            self.over = true;
            return Err(io::Error::other("export limit: output bytes"));
        }
        let k = self.inner.write(b)?;
        self.n += k as u64;
        Ok(k)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Sink for Counting<'_> {
    fn finish(self: Box<Self>) -> io::Result<()> {
        // the registry never finishes a sink: the host does, after `Ok`
        Ok(())
    }
}

/// The kit's identity exporter: target `as-is` for any source, the file's bytes unchanged
/// (a live doc's [`crate::session::Session::file_at`]; a tier-0 version's stored bytes).
/// A `Text` source with one root writes that root's text; with several it is declined
/// (`not_one_file`: export the file's bytes instead).
#[derive(Debug, Clone, Copy, Default)]
pub struct AsIs;

/// The `as-is` target.
pub const AS_IS: ExportTarget = ExportTarget::new(
    "as-is",
    "application/octet-stream",
    "",
    Fidelity::Lossless,
    &[],
);

impl Exporter for AsIs {
    fn id(&self) -> &'static str {
        concat!("k2-mpc-base-as-is@", env!("CARGO_PKG_VERSION"))
    }

    fn targets(&self, _: &SourceKind) -> Vec<ExportTarget> {
        vec![AS_IS]
    }

    fn export(
        &self,
        src: Source<'_>,
        target: &str,
        _: &Options,
        out: &mut dyn Sink,
        ctl: &ExportCtl<'_>,
    ) -> Result<ExportReport, ExportError> {
        if target != AS_IS.id {
            return Err(ExportError::Unsupported {
                kind: "any source".into(),
                target: target.to_owned(),
            });
        }
        match src {
            Source::Bytes { read, size, .. } => {
                if size > ctl.limits.max_input_bytes {
                    return Err(ExportError::Limit("input bytes"));
                }
                let mut buf = [0u8; 64 * 1024];
                let mut done = 0u64;
                loop {
                    ctl.check()?;
                    let k = read.read(&mut buf)?;
                    if k == 0 {
                        break;
                    }
                    out.write_all(&buf[..k])?;
                    done += k as u64;
                    (ctl.progress)(done, size);
                }
                Ok(ExportReport::default())
            }
            Source::Text { roots, .. } => match roots {
                [(_, text)] => {
                    ctl.check()?;
                    if text.len() as u64 > ctl.limits.max_input_bytes {
                        return Err(ExportError::Limit("input bytes"));
                    }
                    out.write_all(text.as_bytes())?;
                    (ctl.progress)(text.len() as u64, text.len() as u64);
                    Ok(ExportReport::default())
                }
                _ => Err(ExportError::Declined {
                    code: "not_one_file",
                    message: format!(
                        "the doc has {} roots: export its file's bytes (Source::Bytes)",
                        roots.len()
                    ),
                }),
            },
        }
    }
}
