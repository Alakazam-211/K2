//! Seed decks for the fuzz targets (scripts/check-fuzz.sh): written with
//! deckcraft's own model and writer, so the fuzzer starts from packages the
//! reader takes. `cargo run -p k2-deckrender --example fuzz_seeds -- <dir>`.

use std::sync::Arc;

use deckcraft_model::style::Fill;
use deckcraft_model::text::{Paragraph, Run, RunProps, TextBody};
use deckcraft_model::{ColorRef, Rgba, Shape, ShapeId, ShapeKind, Xfrm};

fn text(id: u32, s: &str, x: f64, y: f64) -> Shape {
    let props = RunProps { font: Some("Calibri".into()), size: Some(24.0), ..Default::default() };
    Shape {
        id: ShapeId(id),
        name: format!("Text {id}"),
        xfrm: Some(Xfrm::new(x, y, 300.0, 60.0)),
        kind: ShapeKind::Shape,
        text_box: true,
        text: Some(TextBody { paragraphs: vec![Paragraph { runs: vec![Run::with(s, props)], ..Default::default() }], ..Default::default() }),
        ..Default::default()
    }
}

fn boxed(id: u32, x: f64, y: f64, rot: f64, rgb: (u8, u8, u8)) -> Shape {
    let mut xf = Xfrm::new(x, y, 120.0, 80.0);
    xf.rot = rot;
    xf.flip_h = rot > 0.0;
    Shape {
        id: ShapeId(id),
        name: format!("Box {id}"),
        xfrm: Some(xf),
        kind: ShapeKind::Shape,
        fill: Some(Fill::solid(ColorRef::rgb(Rgba::rgb(rgb.0, rgb.1, rgb.2)))),
        ..Default::default()
    }
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "fuzz-seeds".into());
    std::fs::create_dir_all(&dir).expect("seed folder");
    let mut decks = Vec::new();
    // The default new deck.
    decks.push(deckcraft_model::defaults::new_presentation(None));
    // Text, shapes, rotation and a group, over three slides.
    let mut p = deckcraft_model::defaults::new_presentation(None);
    let layout = p.slides[0].layout;
    let mut s = (*p.slides[0]).clone();
    s.shapes.push(text(3000, "Hello K2 – ünïcödé, 日本語, שלום", 40.0, 40.0));
    s.shapes.push(boxed(3001, 400.0, 200.0, 0.0, (255, 0, 0)));
    s.shapes.push(boxed(3002, 600.0, 300.0, 30.0, (0, 128, 255)));
    let child = Xfrm::new(0.0, 0.0, 240.0, 160.0);
    s.shapes.push(Shape {
        id: ShapeId(3003),
        name: "Group".into(),
        xfrm: Some(Xfrm::new(100.0, 300.0, 240.0, 160.0)),
        kind: ShapeKind::Group { children: vec![boxed(3004, 0.0, 0.0, 0.0, (0, 200, 0)), text(3005, "in a group", 20.0, 80.0)], child },
        ..Default::default()
    });
    p.slides[0] = Arc::new(s);
    for _ in 0..2 {
        if let Some(n) = deckcraft_model::defaults::new_slide(&mut p, layout) {
            p.slides.push(Arc::new(n));
        }
    }
    decks.push(p);
    for (i, d) in decks.iter().enumerate() {
        let bytes = deckcraft_pptx::export(d).expect("export");
        std::fs::write(format!("{dir}/seed-{i}.pptx"), bytes).expect("write seed");
    }
    println!("wrote {} seeds to {dir}", decks.len());
}
