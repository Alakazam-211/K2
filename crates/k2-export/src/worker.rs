//! The worker process (`k2-export`): hello, read one header, run one
//! `describe` or `export` through the kit's `ExportRegistry`, send exactly
//! one final frame, exit. It never opens a file, a socket or a database:
//! its input is stdin, its output stdout, its errors stderr (the host keeps
//! a ring of them for its log, never for callers).
//!
//! A cancel is a kill (the host's), so the kit's cancel flag stays false.

use std::cell::RefCell;
use std::io::{self, BufRead, Read, Write};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_mpc_base::errors::Refusal;
use k2_mpc_base::export::{ExportCtl, ExportError, ExportRegistry, Exporter, NoResources, Options, Sink, Source};

use crate::proto::{self, Header};

/// Most `D` payload bytes buffered before a frame is sent.
const DATA_CHUNK: usize = 64 << 10;

/// At most 4 progress frames a second.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// The exporters this worker offers, in registration order: the kit's
/// as-is, then (test builds only) the hostile ones.
pub fn exporters() -> Vec<Arc<dyn Exporter>> {
    #[allow(unused_mut)]
    let mut v: Vec<Arc<dyn Exporter>> = vec![Arc::new(k2_mpc_base::export::AsIs)];
    #[cfg(feature = "test-hostile")]
    v.push(Arc::new(crate::hostile::Hostile));
    v
}

fn registry() -> ExportRegistry {
    let mut r = ExportRegistry::new();
    for e in exporters() {
        r.register(e).expect("exporter ids are distinct");
    }
    r
}

/// The hello payload.
pub fn hello() -> serde_json::Value {
    serde_json::json!({
        "proto": proto::PROTO,
        "version": proto::VERSION,
        "exporters": exporters().iter().map(|e| e.id()).collect::<Vec<_>>(),
        "hostile": cfg!(feature = "test-hostile"),
    })
}

/// The `E` frame bytes for an error report (the kit's JSON).
pub fn error_frame(e: &ExportError) -> Vec<u8> {
    proto::frame(proto::TAG_ERROR, e.report().to_json().as_bytes())
}

/// Shared stdout for the sink and the progress callback (one thread).
struct Out<W: Write> {
    w: W,
    buf: Vec<u8>,
    last_progress: Option<Instant>,
}

impl<W: Write> Out<W> {
    fn flush_data(&mut self) -> io::Result<()> {
        while !self.buf.is_empty() {
            let n = self.buf.len().min(proto::MAX_FRAME);
            proto::write_frame(&mut self.w, proto::TAG_DATA, &self.buf[..n])?;
            self.buf.drain(..n);
        }
        Ok(())
    }
}

struct FrameSink<'a, W: Write> {
    out: &'a RefCell<Out<W>>,
}

impl<W: Write> Write for FrameSink<'_, W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let mut o = self.out.borrow_mut();
        o.buf.extend_from_slice(b);
        if o.buf.len() >= DATA_CHUNK {
            o.flush_data()?;
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.out.borrow_mut().flush_data()
    }
}

impl<W: Write> Sink for FrameSink<'_, W> {
    fn finish(self: Box<Self>) -> io::Result<()> {
        // The host finishes: after the `R` frame and a clean exit.
        Ok(())
    }
}

/// Run the worker on `input` / `output`. `set_memory_cap` arms the
/// binary's counting allocator (cap in bytes, and the prebuilt `E` frame it
/// writes when an allocation would pass it). Returns the exit code.
pub fn run(input: &mut dyn BufRead, output: &mut dyn Write, set_memory_cap: &dyn Fn(u64, Vec<u8>)) -> i32 {
    if proto::write_frame(output, proto::TAG_HELLO, hello().to_string().as_bytes()).is_err() {
        return proto::EXIT_ERROR;
    }
    let mut line = Vec::new();
    let read = (&mut *input).take(proto::MAX_HEADER as u64 + 1).read_until(b'\n', &mut line);
    let header = match read {
        Ok(_) if line.len() > proto::MAX_HEADER => Err("header line too long".to_string()),
        Ok(0) => Err("no header".to_string()),
        Ok(_) => Header::parse(line.strip_suffix(b"\n").unwrap_or(&line)),
        Err(e) => Err(format!("reading the header: {e}")),
    };
    let header = match header {
        Ok(h) => h,
        Err(m) => return fail(output, &ExportError::Internal { exporter: "k2-export".into(), message: m }),
    };
    match header {
        Header::Describe { kind } => {
            let targets: Vec<serde_json::Value> =
                registry().targets_for(&kind.kit()).iter().map(|(id, t)| proto::target_json(id, t)).collect();
            let body = serde_json::json!({ "targets": targets }).to_string();
            match proto::write_frame(output, proto::TAG_REPORT, body.as_bytes()) {
                Ok(()) => proto::EXIT_OK,
                Err(_) => proto::EXIT_ERROR,
            }
        }
        Header::Export { kind, target, options, limits, name, size, .. } => {
            let limit_err = ExportError::Limit("memory");
            set_memory_cap(limits.memory_bytes, error_frame(&limit_err));
            let mut opts = Options::new();
            for (k, v) in &options {
                opts.set(k, v);
            }
            let cancel = AtomicBool::new(false);
            let out = RefCell::new(Out { w: &mut *output, buf: Vec::new(), last_progress: None });
            let res = {
                let progress = |done: u64, total: u64| {
                    let mut o = out.borrow_mut();
                    let now = Instant::now();
                    if o.last_progress.is_some_and(|t| now.duration_since(t) < PROGRESS_EVERY) {
                        return;
                    }
                    o.last_progress = Some(now);
                    // Data written so far goes first, so progress never runs ahead of it.
                    let _ = o.flush_data();
                    let _ = proto::write_frame(&mut o.w, proto::TAG_PROGRESS, &proto::progress_payload(done, total));
                };
                let ctl = ExportCtl { cancel: &cancel, progress: &progress, limits: limits.kit(), resources: &NoResources };
                let mut stdin = (&mut *input).take(size);
                let src = Source::Bytes { read: &mut stdin, size, name: &name };
                let mut sink = FrameSink { out: &out };
                registry().export(&kind.kit(), src, &target, &opts, &mut sink, &ctl)
            };
            let mut o = out.into_inner();
            match res {
                Ok(report) => {
                    if o.flush_data().is_err() {
                        return proto::EXIT_ERROR;
                    }
                    let body = report_json(&report).to_string();
                    match proto::write_frame(&mut o.w, proto::TAG_REPORT, body.as_bytes()) {
                        Ok(()) => proto::EXIT_OK,
                        Err(_) => proto::EXIT_ERROR,
                    }
                }
                // Whatever data went out is discarded by the host.
                Err(e) => fail(&mut o.w, &e),
            }
        }
    }
}

fn fail(output: &mut dyn Write, e: &ExportError) -> i32 {
    let _ = output.write_all(&error_frame(e));
    let _ = output.flush();
    proto::EXIT_ERROR
}

/// An `ExportReport` on the wire.
pub fn report_json(r: &k2_mpc_base::export::ExportReport) -> serde_json::Value {
    serde_json::json!({
        "losses": r.losses.iter().map(|l| serde_json::json!({ "kind": l.kind, "detail": l.detail })).collect::<Vec<_>>(),
        "notes": r.notes,
        "pages": r.pages,
        "bytes": r.bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{read_frame, Frame, Kind, Limits};

    fn limits() -> Limits {
        Limits {
            max_input_bytes: 1 << 20,
            max_output_bytes: 1 << 20,
            max_pixels: 1,
            max_pages: 1,
            max_wall_ms: 1000,
            memory_bytes: 1 << 30,
        }
    }

    fn frames(mut out: &[u8]) -> Vec<Frame> {
        let mut v = Vec::new();
        loop {
            match read_frame(&mut out) {
                Ok(f) => v.push(f),
                Err(crate::proto::FrameError::Eof) => return v,
                Err(e) => panic!("bad frame: {e:?}"),
            }
        }
    }

    #[test]
    fn as_is_passes_the_bytes_through_and_reports() {
        let body = b"# hello\nworld\n".to_vec();
        let h = Header::Export {
            job: "j".into(),
            kind: Kind::Pack("markdown".into()),
            target: "as-is".into(),
            options: vec![],
            limits: limits(),
            name: "notes.md".into(),
            size: body.len() as u64,
        };
        let mut input = h.json().to_string().into_bytes();
        input.push(b'\n');
        input.extend_from_slice(&body);
        let mut out = Vec::new();
        let rc = run(&mut &input[..], &mut out, &|_, _| {});
        assert_eq!(rc, proto::EXIT_OK);
        let f = frames(&out);
        assert!(matches!(&f[0], Frame::Hello(_)));
        let data: Vec<u8> = f.iter().filter_map(|x| if let Frame::Data(d) = x { Some(d.clone()) } else { None }).flatten().collect();
        assert_eq!(data, body);
        let Some(Frame::Report(r)) = f.last() else { panic!("no report: {f:?}") };
        let r: serde_json::Value = serde_json::from_slice(r).expect("report json");
        assert_eq!(r["bytes"], serde_json::json!(body.len()));
    }

    #[test]
    fn an_unknown_target_is_the_kits_e001_and_a_bad_option_its_e002() {
        for (target, opts, id) in [("nope", vec![], "MPC-E001"), ("as-is", vec![("dpi".to_string(), "9".to_string())], "MPC-E002")] {
            let h = Header::Export {
                job: "j".into(),
                kind: Kind::Format("png".into()),
                target: target.into(),
                options: opts,
                limits: limits(),
                name: "a.png".into(),
                size: 0,
            };
            let mut input = h.json().to_string().into_bytes();
            input.push(b'\n');
            let mut out = Vec::new();
            assert_eq!(run(&mut &input[..], &mut out, &|_, _| {}), proto::EXIT_ERROR);
            let f = frames(&out);
            let Some(Frame::Error(e)) = f.last() else { panic!("no error frame: {f:?}") };
            let e: serde_json::Value = serde_json::from_slice(e).expect("error json");
            assert_eq!(e["id"], serde_json::json!(id), "{e}");
        }
    }

    #[test]
    fn describe_lists_as_is() {
        let h = Header::Describe { kind: Kind::Format("png".into()) };
        let mut input = h.json().to_string().into_bytes();
        input.push(b'\n');
        let mut out = Vec::new();
        assert_eq!(run(&mut &input[..], &mut out, &|_, _| {}), proto::EXIT_OK);
        let f = frames(&out);
        let Some(Frame::Report(r)) = f.last() else { panic!("no report") };
        let r: serde_json::Value = serde_json::from_slice(r).expect("json");
        assert_eq!(r["targets"][0]["id"], serde_json::json!("as-is"));
        assert_eq!(r["targets"][0]["fidelity"], serde_json::json!("lossless"));
    }

    #[test]
    fn a_garbled_header_is_an_internal_error_frame() {
        let mut out = Vec::new();
        assert_eq!(run(&mut &b"{nope\n"[..], &mut out, &|_, _| {}), proto::EXIT_ERROR);
        let f = frames(&out);
        let Some(Frame::Error(e)) = f.last() else { panic!("no error frame") };
        let e: serde_json::Value = serde_json::from_slice(e).expect("json");
        assert_eq!(e["id"], serde_json::json!("MPC-E007"));
    }
}
