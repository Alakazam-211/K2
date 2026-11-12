//! The worker, run as the host runs it: a good deck renders (pixels, shape
//! boxes, sldId, fonts); garbage, truncated and other non-packages fail typed;
//! a zip bomb is stopped at the memory cap and a huge slide refused, both as
//! `too_large`; the compiled-in Liberation Sans is the pinned 2.1.5.

use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use deckcraft_model::style::Fill;
use deckcraft_model::text::{Paragraph, Run, RunKind, RunProps, TextBody};
use deckcraft_model::{ColorRef, Rgba, Shape, ShapeId, ShapeKind, Xfrm};
use k2_deckrender::host::{self, Limits, Request};
use k2_deckrender::proto::{FailureCode, ImageFormat, SlideRef};

fn bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_k2-deckrender"))
}

const RECT_ID: u32 = 4000;
const RECT_PT: (f64, f64, f64, f64) = (480.0, 270.0, 200.0, 100.0);

/// A two-slide deck made with deckcraft's own model and writer: slide 0 has a
/// red rectangle (id 4000) and a text box in Calibri, slide 1 is empty.
fn deck() -> Vec<u8> {
    let mut p = deckcraft_model::defaults::new_presentation(None);
    let layout = p.slides[0].layout;
    let mut s = (*p.slides[0]).clone();
    s.shapes.push(Shape {
        id: ShapeId(RECT_ID),
        name: "Red box".into(),
        xfrm: Some(Xfrm::new(RECT_PT.0, RECT_PT.1, RECT_PT.2, RECT_PT.3)),
        kind: ShapeKind::Shape,
        fill: Some(Fill::solid(ColorRef::rgb(Rgba::rgb(255, 0, 0)))),
        ..Default::default()
    });
    let props = RunProps { font: Some("Calibri".into()), size: Some(28.0), ..Default::default() };
    s.shapes.push(Shape {
        id: ShapeId(4001),
        name: "Words".into(),
        xfrm: Some(Xfrm::new(60.0, 60.0, 400.0, 80.0)),
        kind: ShapeKind::Shape,
        text_box: true,
        text: Some(TextBody { paragraphs: vec![Paragraph { runs: vec![Run::with("Hello K2", props)], ..Default::default() }], ..Default::default() }),
        ..Default::default()
    });
    p.slides[0] = Arc::new(s);
    let second = deckcraft_model::defaults::new_slide(&mut p, layout).expect("slide 2");
    p.slides.push(Arc::new(second));
    deckcraft_pptx::export(&p).expect("export")
}

/// The package's entries, in order.
fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).expect("entry");
            let name = f.name().expect("entry name").to_string();
            let mut d = Vec::new();
            f.read_to_end(&mut d).expect("read");
            (name, d)
        })
        .collect()
}

fn rezip(es: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (n, d) in es {
        z.start_file(n.as_str(), o).expect("start");
        z.write_all(d).expect("write");
    }
    z.finish().expect("finish").into_inner()
}

fn limits() -> Limits {
    Limits { wall: Duration::from_secs(60), memory_bytes: 1 << 30, ..Limits::default() }
}

fn code_of(pptx: &[u8], slide: SlideRef, width: u32, l: &Limits) -> FailureCode {
    match host::render(bin(), &Request::new(pptx, slide, width), l) {
        Ok(r) => panic!("rendered {} x {}", r.meta.width, r.meta.height),
        Err(f) => {
            eprintln!("[deckrender] {}: {} | stderr: {}", f.code, f.detail, f.stderr.trim());
            f.code
        }
    }
}

#[test]
fn a_good_deck_renders_with_its_boxes_and_fonts() {
    let bytes = deck();
    let started = std::time::Instant::now();
    let r = host::render(bin(), &Request::new(&bytes, SlideRef::Index(0), 960), &limits()).expect("render");
    eprintln!("[deckrender] slide 0 at 960 px: {} ms, {} WebP bytes, timings {:?}", started.elapsed().as_millis(), r.image.len(), r.meta.timings);
    let m = &r.meta;
    assert_eq!((m.width, m.height, m.slide_index, m.slide_count), (960, 540, 0, 2));
    assert_eq!(m.slide_size_emu, [960 * 12_700, 540 * 12_700]);
    assert_eq!(r.format, ImageFormat::Webp);
    let img = image::load_from_memory_with_format(&r.image, image::ImageFormat::WebP).expect("webp").to_rgba8();
    assert_eq!((img.width(), img.height()), (960, 540));
    let c = img.get_pixel((RECT_PT.0 + RECT_PT.2 / 2.0) as u32, (RECT_PT.1 + RECT_PT.3 / 2.0) as u32).0;
    assert!(c[0] > 200 && c[1] < 60 && c[2] < 60, "the rectangle's middle is red: {c:?}");
    let corner = img.get_pixel(950, 530).0;
    assert!(corner[0] > 200 && corner[1] > 200 && corner[2] > 200, "the background is white: {corner:?}");

    let b = m.shapes.iter().find(|s| s.id == RECT_ID).expect("the rectangle's box");
    let emu = |v: f64| (v * 12_700.0).round() as i64;
    assert_eq!([b.x, b.y, b.w, b.h], [emu(RECT_PT.0), emu(RECT_PT.1), emu(RECT_PT.2), emu(RECT_PT.3)]);
    assert_eq!(b.bbox, [b.x, b.y, b.w, b.h]);
    assert_eq!((b.rot, b.flip_h, b.flip_v, b.approx_box, b.kind.as_str(), b.name.as_str()), (0.0, false, false, false, "shape", "Red box"));
    assert!(m.shapes.iter().any(|s| s.id == 4001), "the text box is listed");
    assert!(!m.fonts.used.is_empty(), "some family drew the text: {:?}", m.fonts);
    // Only the bundled fonts are loaded, and Calibri isn't one of them.
    assert!(m.fonts.substituted.iter().any(|s| s.requested == "Calibri"), "Calibri is reported as substituted: {:?}", m.fonts);
    assert!(m.timings.families >= 1 && m.timings.faces >= 4, "{:?}", m.timings);

    // PNG on request: the same pixels (both encodings are lossless).
    let png = host::render(bin(), &Request { format: ImageFormat::Png, ..Request::new(&bytes, SlideRef::Index(0), 960) }, &limits()).expect("png");
    assert_eq!(png.format, ImageFormat::Png);
    let png_img = image::load_from_memory_with_format(&png.image, image::ImageFormat::Png).expect("png").to_rgba8();
    assert_eq!(png_img.as_raw(), img.as_raw(), "WebP and PNG carry the same pixels");

    // The same slide by its sldId, and the second slide by both.
    let id0 = m.sld_id.expect("the slide list matches the package's");
    let again = host::render(bin(), &Request::new(&bytes, SlideRef::SldId(id0), 480), &limits()).expect("by sldId");
    assert_eq!((again.meta.slide_index, again.meta.width, again.meta.height), (0, 480, 270));
    let second = host::render(bin(), &Request::new(&bytes, SlideRef::Index(1), 320), &limits()).expect("slide 1");
    let id1 = second.meta.sld_id.expect("sldId of slide 1");
    assert_ne!(id0, id1);
    let by_id = host::render(bin(), &Request::new(&bytes, SlideRef::SldId(id1), 320), &limits()).expect("slide 1 by id");
    assert_eq!(by_id.meta.slide_index, 1);
    assert_eq!(by_id.image, second.image, "the same slide renders to the same bytes");

    assert_eq!(code_of(&bytes, SlideRef::Index(2), 320, &limits()), FailureCode::NoSuchSlide);
    assert_eq!(code_of(&bytes, SlideRef::SldId(id0.wrapping_add(id1) + 7), 320, &limits()), FailureCode::NoSuchSlide);
}

#[test]
fn garbage_truncated_and_other_files_fail_typed_not_hang() {
    let l = Limits { wall: Duration::from_secs(30), ..limits() };
    let good = deck();
    let started = std::time::Instant::now();
    assert_eq!(code_of(b"", SlideRef::Index(0), 320, &l), FailureCode::InvalidInput);
    assert_eq!(code_of(b"this is not a presentation", SlideRef::Index(0), 320, &l), FailureCode::InvalidInput);
    let noise: Vec<u8> = (0..65_536u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    assert_eq!(code_of(&noise, SlideRef::Index(0), 320, &l), FailureCode::InvalidInput);
    for cut in [4, 64, good.len() / 3, good.len() / 2, good.len() - 30] {
        assert_eq!(code_of(&good[..cut], SlideRef::Index(0), 320, &l), FailureCode::InvalidInput, "cut at {cut}");
    }
    // A zip that isn't a presentation.
    let other = rezip(&[("hello.txt".to_string(), b"hi".to_vec())]);
    assert_eq!(code_of(&other, SlideRef::Index(0), 320, &l), FailureCode::InvalidInput);
    // A compound file (a binary .ppt or an encrypted package).
    let mut cfb = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    cfb.resize(4096, 0);
    assert_eq!(code_of(&cfb, SlideRef::Index(0), 320, &l), FailureCode::UnsupportedPart);
    assert!(started.elapsed() < Duration::from_secs(60), "typed answers, not timeouts: {:?}", started.elapsed());
}

#[test]
fn a_zip_bomb_is_stopped_at_the_memory_cap() {
    // A real deck plus one part that inflates to 400 MiB of zeros (well under
    // the reader's own 512 MiB part limit, so only the memory cap stops it).
    let mut es = entries(&deck());
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(false);
    for (n, d) in &es {
        z.start_file(n.as_str(), o).expect("start");
        z.write_all(d).expect("write");
    }
    z.start_file("ppt/media/image999.png", o).expect("bomb");
    let zeros = vec![0u8; 1 << 20];
    for _ in 0..400 {
        z.write_all(&zeros).expect("zeros");
    }
    let bomb = z.finish().expect("finish").into_inner();
    es.clear();
    eprintln!("[deckrender] zip bomb: {} bytes compressed", bomb.len());
    assert!(bomb.len() < 4 << 20);
    let l = Limits { memory_bytes: 256 << 20, wall: Duration::from_secs(60), ..Limits::default() };
    let started = std::time::Instant::now();
    assert_eq!(code_of(&bomb, SlideRef::Index(0), 320, &l), FailureCode::TooLarge);
    eprintln!("[deckrender] zip bomb stopped after {} ms", started.elapsed().as_millis());
}

#[test]
fn huge_slides_and_images_are_refused_as_too_large() {
    // sldSz 1 pt x 20000 pt: at 320 px wide the image would be 6.4 M px tall.
    let es: Vec<(String, Vec<u8>)> = entries(&deck())
        .into_iter()
        .map(|(n, d)| {
            if n == "ppt/presentation.xml" {
                let x = String::from_utf8(d).expect("utf-8");
                let start = x.find("<p:sldSz").expect("sldSz");
                let end = start + x[start..].find("/>").expect("end") + 2;
                let x = format!("{}<p:sldSz cx=\"12700\" cy=\"254000000\"/>{}", &x[..start], &x[end..]);
                (n, x.into_bytes())
            } else {
                (n, d)
            }
        })
        .collect();
    let tall = rezip(&es);
    assert_eq!(code_of(&tall, SlideRef::Index(0), 320, &limits()), FailureCode::TooLarge);
    // A 16:9 slide 8000 px wide is 36 M px: over the 32 M px default.
    assert_eq!(code_of(&deck(), SlideRef::Index(0), 8000, &limits()), FailureCode::TooLarge);
    // Wider than the host allows: refused before any worker starts.
    assert_eq!(code_of(&deck(), SlideRef::Index(0), 9000, &limits()), FailureCode::TooLarge);
}

/// A deck whose slide has a date field saved as "1/2/2003".
fn dated_deck() -> Vec<u8> {
    let mut p = deckcraft_model::defaults::new_presentation(None);
    let mut s = (*p.slides[0]).clone();
    let mut run = Run::with("1/2/2003", RunProps { size: Some(40.0), ..Default::default() });
    run.kind = RunKind::Field { field: "datetime1".into() };
    s.shapes.push(Shape {
        id: ShapeId(4100),
        name: "Date".into(),
        xfrm: Some(Xfrm::new(60.0, 300.0, 600.0, 80.0)),
        kind: ShapeKind::Shape,
        text_box: true,
        text: Some(TextBody { paragraphs: vec![Paragraph { runs: vec![run], ..Default::default() }], ..Default::default() }),
        ..Default::default()
    });
    p.slides[0] = Arc::new(s);
    deckcraft_pptx::export(&p).expect("export")
}

#[test]
fn date_fields_draw_the_saved_or_requested_text_never_the_clock() {
    let bytes = dated_deck();
    let a = host::render(bin(), &Request::new(&bytes, SlideRef::Index(0), 480), &limits()).expect("render");
    assert_eq!((a.meta.date_text.as_str(), a.meta.date_text_from.as_str()), ("1/2/2003", "file"));
    let b = host::render(bin(), &Request::new(&bytes, SlideRef::Index(0), 480), &limits()).expect("again");
    assert_eq!(a.image, b.image, "the same bytes every time");
    let c = host::render(bin(), &Request { date_text: Some("Q4 review"), ..Request::new(&bytes, SlideRef::Index(0), 480) }, &limits()).expect("asked");
    assert_eq!((c.meta.date_text.as_str(), c.meta.date_text_from.as_str()), ("Q4 review", "request"));
    assert_ne!(a.image, c.image, "the requested text is drawn");
    let plain = host::render(bin(), &Request::new(&deck(), SlideRef::Index(0), 320), &limits()).expect("no field");
    assert_eq!(plain.meta.date_text_from, "none");
}

/// A slide drawn first in a fresh process is the same, byte for byte, as when the process drew
/// other slides before it (deckcraft picks fallback faces among the faces already loaded; the
/// bundled fonts are all loaded before the first draw).
#[test]
fn a_slide_renders_the_same_first_or_after_other_slides() {
    let bytes = deck();
    let dated = dated_deck();
    for (deck, slide) in [(&bytes, 1u32), (&bytes, 0), (&dated, 0)] {
        let fresh = host::render(bin(), &Request::new(deck, SlideRef::Index(slide), 640), &limits()).expect("fresh process");
        // In this process, after drawing the other decks' slides first.
        let opts = k2_deckrender::worker::Opts::default();
        for warm in [(&dated, 0u32), (&bytes, 0), (&bytes, 1)] {
            let _ = k2_deckrender::worker::render(warm.0, SlideRef::Index(warm.1), 640, &opts).expect("warm-up");
        }
        let after = k2_deckrender::worker::render(deck, SlideRef::Index(slide), 640, &opts).expect("after others");
        assert_eq!(fresh.image, after.image, "slide {slide}: first in a process and after others give the same bytes");
    }
}
