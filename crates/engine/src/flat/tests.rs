use serde_json::{Value, json};

use super::*;
use crate::{Session, cmd, sample};

fn sample_session() -> Session {
    let mut s = Session::new();
    sample::open_sample(&mut s).unwrap();
    s
}

fn flatten_cmd(s: &mut Session) -> FlatDeck {
    let v = s.execute("file.flatten", &json!({})).unwrap();
    serde_json::from_value(v).unwrap()
}

fn tiny_png() -> Vec<u8> {
    let img = deckcraft_render::Image { width: 4, height: 2, pixels: [200u8, 40, 40, 255].repeat(8) };
    img.to_png()
}

/// A deck built in code with one of everything the flattener treats specially.
fn built_session() -> Session {
    let mut s = Session::new();
    s.execute("file.new", &json!({"blank": true})).unwrap();
    s.execute("slide.new", &json!({"layout": "titleOnly", "title": "Shapes"})).unwrap();
    let id = |v: Value| v["id"].as_u64().unwrap();
    let a = id(s.execute("shape.insert", &json!({"preset": "rect", "rect": [36, 100, 200, 100], "text": "Solid"})).unwrap());
    s.execute("shape.fill", &json!({"id": a, "color": "#336699"})).unwrap();
    let g = id(s.execute("shape.insert", &json!({"preset": "ellipse", "rect": [300, 100, 200, 100], "text": "Gradient"})).unwrap());
    s.execute("shape.fill", &json!({"id": g, "gradient": {"stops": [[0, "accent1"], [1, "accent2"]], "angle": 0}})).unwrap();
    let c = id(s.execute("shape.insert", &json!({"preset": "cloud", "rect": [560, 100, 200, 100]})).unwrap());
    s.execute("shape.freeform", &json!({"points": [[40, 300], [120, 260], [200, 320], [60, 380]], "closed": true})).unwrap();
    // A group of two shapes (no rotation): children come out at slide coordinates.
    let g1 = id(s.execute("shape.insert", &json!({"preset": "rect", "rect": [600, 300, 60, 40]})).unwrap());
    let g2 = id(s.execute("shape.insert", &json!({"preset": "ellipse", "rect": [700, 340, 60, 40]})).unwrap());
    s.execute("arrange.group", &json!({"ids": [g1, g2]})).unwrap();
    let pic = s.execute("insert.picture", &json!({"data": cmd::base64_encode(&tiny_png()), "name": "dot.png", "rect": [300, 420, 80, 40]})).unwrap();
    assert!(pic["id"].as_u64().is_some());
    s.execute("edit.select", &json!({"ids": [a]})).unwrap();
    s.execute("animation.add", &json!({"effect": "fade", "class": "entrance"})).unwrap();
    s.execute("slide.new", &json!({"layout": "titleOnly", "title": "Table"})).unwrap();
    s.execute("insert.table", &json!({"rows": 2, "cols": 2, "rect": [100, 150, 400, 80], "data": [["a", "b"], ["c", "d"]]})).unwrap();
    let _ = c;
    s
}

fn elements_of(d: &FlatDeck, slide: usize) -> &[FlatElement] {
    d.slides.get(slide).map(|s| s.elements.as_slice()).unwrap_or(&[])
}

#[test]
fn flatten_classifies_shapes() {
    let mut s = built_session();
    let d = flatten_cmd(&mut s);
    assert_eq!(d.version, FLAT_VERSION);
    assert_eq!(d.slides.len(), 2);
    let els = elements_of(&d, 0);
    let by_text = |t: &str| {
        els.iter()
            .find(|e| match &e.body {
                FlatBody::Shape { text: Some(tx), .. } => tx.paragraphs.iter().any(|p| p.runs.iter().any(|r| r.text == t)),
                _ => false,
            })
            .unwrap()
    };
    // Solid rect: a vector shape with its fill.
    let solid_el = by_text("Solid");
    match &solid_el.body {
        FlatBody::Shape { preset, fill: Some(FlatFill::Solid { color, .. }), .. } => {
            assert_eq!(preset, "rect");
            assert_eq!(color, "#336699");
        }
        b => panic!("solid rect: {b:?}"),
    }
    assert_eq!((solid_el.x, solid_el.y, solid_el.w, solid_el.h), (36.0, 100.0, 200.0, 100.0));
    assert_eq!(solid_el.animations.len(), 1);
    assert_eq!(solid_el.animations[0].kind, "entrance");
    // Gradient ellipse: a raster plus a transparent text box on top with the same box.
    let over = by_text("Gradient");
    assert!(over.id.ends_with("-text"));
    match &over.body {
        FlatBody::Shape { preset, fill: None, line: None, text: Some(t), .. } => {
            assert_eq!(preset, "rect");
            // The ellipse's own text rectangle is folded into the insets.
            assert!(t.insets.l > 7.2 && t.insets.t > 3.6, "{:?}", t.insets);
        }
        b => panic!("overlay: {b:?}"),
    }
    let oi = els.iter().position(|e| e.id == over.id).unwrap();
    assert!(matches!(els[oi - 1].body, FlatBody::Raster { .. }), "raster right below the text");
    assert_eq!((over.x, over.y, over.w, over.h), (300.0, 100.0, 200.0, 100.0));
    // Cloud (not in the allow-list) and the freeform (custom geometry): rasters.
    let rasters = els.iter().filter(|e| matches!(e.body, FlatBody::Raster { .. })).count();
    assert!(rasters >= 3, "{rasters}");
    // Group children at slide coordinates.
    let rects: Vec<_> = els.iter().filter(|e| (e.w, e.h) == (60.0, 40.0)).map(|e| (e.x, e.y)).collect();
    assert!(rects.contains(&(600.0, 300.0)) && rects.contains(&(700.0, 340.0)), "{rects:?}");
    // Picture.
    let pic = els.iter().find(|e| matches!(e.body, FlatBody::Picture { .. })).unwrap();
    let FlatBody::Picture { media, .. } = &pic.body else { panic!() };
    assert!(d.media.iter().any(|m| &m.id == media && m.content_type == "image/png"));
    // Title placeholder: resolved text formatting.
    let title = els.iter().find(|e| e.placeholder.as_deref() == Some("title")).unwrap();
    let FlatBody::Shape { text: Some(t), .. } = &title.body else { panic!() };
    let r = &t.paragraphs[0].runs[0];
    assert_eq!(r.text, "Shapes");
    assert!(r.size > 20.0 && !r.font.is_empty() && !r.font.starts_with('+') && r.color.starts_with('#'));
    // Table: resolved style colours.
    let tbl = elements_of(&d, 1).iter().find(|e| matches!(e.body, FlatBody::Table { .. })).unwrap();
    let FlatBody::Table { col_widths, rows } = &tbl.body else { panic!() };
    assert_eq!(col_widths.len(), 2);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].cells[0].fill.is_some());
    assert_eq!(rows[1].cells[1].text.paragraphs[0].runs[0].text, "d");
}

#[test]
fn every_flat_media_item_is_retrievable() {
    let mut s = built_session();
    let d = flatten_cmd(&mut s);
    assert!(!d.media.is_empty());
    for m in &d.media {
        let v = s.execute("media.bytes", &json!({"id": m.id})).unwrap();
        let data = cmd::base64_decode(v["data"].as_str().unwrap()).unwrap();
        assert!(!data.is_empty(), "{}", m.id);
        if m.content_type == "image/png" {
            assert!(data.starts_with(b"\x89PNG"));
        }
    }
    assert!(s.execute("media.bytes", &json!({"id": "nope"})).is_err());
    assert!(s.execute("media.bytes", &json!({})).is_err());
}

/// Compare two flat decks: same slides, elements, boxes (within 0.5 pt), text and fills. A raster
/// comes back as a picture; ids, media ids and the master flag may differ.
fn assert_equivalent(a: &FlatDeck, b: &FlatDeck) {
    assert_eq!(a.slides.len(), b.slides.len());
    assert_eq!(a.size, b.size);
    for (sa, sb) in a.slides.iter().zip(&b.slides) {
        assert_eq!(sa.notes, sb.notes);
        assert_eq!(sa.hidden, sb.hidden);
        assert_eq!(sa.transition, sb.transition);
        assert_eq!(sa.background.is_some(), sb.background.is_some());
        if let (Some(FlatFill::Solid { color: ca, .. }), Some(FlatFill::Solid { color: cb, .. })) = (&sa.background, &sb.background) {
            assert_eq!(ca, cb);
        }
        assert_eq!(sa.elements.len(), sb.elements.len(), "slide {}", sa.id);
        for (ea, eb) in sa.elements.iter().zip(&sb.elements) {
            let close = |x: f64, y: f64| (x - y).abs() <= 0.5;
            assert!(close(ea.x, eb.x) && close(ea.y, eb.y) && close(ea.w, eb.w) && close(ea.h, eb.h), "{ea:?}\n{eb:?}");
            assert!(close(ea.rotation, eb.rotation));
            assert_eq!(ea.name, eb.name);
            assert_eq!(
                ea.animations.iter().map(|x| (&x.kind, &x.effect, &x.trigger)).collect::<Vec<_>>(),
                eb.animations.iter().map(|x| (&x.kind, &x.effect, &x.trigger)).collect::<Vec<_>>()
            );
            match (&ea.body, &eb.body) {
                (
                    FlatBody::Shape { preset: pa, fill: fa, line: la, text: ta, .. },
                    FlatBody::Shape { preset: pb, fill: fb, line: lb, text: tb, .. },
                ) => {
                    assert_eq!(pa, pb);
                    assert_eq!(fa, fb, "{}", ea.id);
                    assert_eq!(la, lb, "{}", ea.id);
                    assert_eq!(ta, tb, "{}", ea.id);
                }
                (FlatBody::Raster { .. } | FlatBody::Picture { .. }, FlatBody::Picture { .. }) => {}
                (FlatBody::Table { col_widths: ca, rows: ra }, FlatBody::Table { col_widths: cb, rows: rb }) => {
                    assert_eq!(ca, cb);
                    assert_eq!(ra.len(), rb.len());
                    for (x, y) in ra.iter().zip(rb) {
                        assert!((x.height - y.height).abs() <= 0.5);
                        assert_eq!(x.cells, y.cells);
                    }
                }
                (
                    FlatBody::Chart { chart_type: ka, series: sa, categories: ca, .. },
                    FlatBody::Chart { chart_type: kb, series: sb, categories: cb, .. },
                ) => {
                    assert_eq!(ka, kb);
                    assert_eq!(ca, cb);
                    assert_eq!(sa, sb);
                }
                (x, y) => assert_eq!(std::mem::discriminant(x), std::mem::discriminant(y), "{x:?} vs {y:?}"),
            }
        }
    }
}

fn round_trip(s: &mut Session) {
    let a = flatten_cmd(s);
    let deck = serde_json::to_value(&a).unwrap();
    s.execute("file.fromFlat", &json!({"deck": deck})).unwrap();
    assert!(s.doc().unwrap().doc.validate().is_empty());
    let b = flatten_cmd(s);
    assert_equivalent(&a, &b);
    // And again: a flat deck that came from a flat deck is a fixed point.
    s.execute("file.fromFlat", &json!({"deck": serde_json::to_value(&b).unwrap()})).unwrap();
    let c = flatten_cmd(s);
    assert_equivalent(&b, &c);
}

#[test]
fn sample_round_trips() {
    let mut s = sample_session();
    let d = flatten_cmd(&mut s);
    assert!(d.slides.len() >= 8);
    assert!(d.slides.iter().all(|sl| sl.background.is_some()));
    assert!(d.slides.iter().flat_map(|sl| &sl.elements).any(|e| matches!(e.body, FlatBody::Chart { .. })));
    assert!(d.slides.iter().flat_map(|sl| &sl.elements).any(|e| matches!(e.body, FlatBody::Table { .. })));
    assert!(d.slides.iter().any(|sl| sl.transition.is_some()));
    assert_eq!(d.slides[0].notes, "Welcome everyone. This deck was made in DeckCraft.");
    round_trip(&mut s);
}

#[test]
fn built_deck_round_trips_and_exports() {
    let mut s = built_session();
    round_trip(&mut s);
    // The rebuilt deck saves as PPTX and PDF, and the PPTX opens again.
    let pptx = s.execute("file.saveBytes", &json!({"format": "pptx"})).unwrap()["data"].as_str().unwrap().to_string();
    let pdf = s.execute("file.saveBytes", &json!({"format": "pdf"})).unwrap()["data"].as_str().unwrap().to_string();
    assert!(cmd::base64_decode(&pdf).unwrap().starts_with(b"%PDF"));
    s.execute("file.openBytes", &json!({"name": "x.pptx", "data": pptx})).unwrap();
    let st = s.doc().unwrap();
    assert_eq!(st.doc.slides.len(), 2);
    assert!(st.doc.slides[0].title().contains("Shapes"));
}

#[test]
fn from_flat_is_undoable_and_creates_a_document_when_none() {
    let mut s = sample_session();
    let n = s.doc().unwrap().doc.slides.len();
    let deck = json!({"version": 1, "size": {"w": 720, "h": 540}, "slides": [{"id": "a", "elements": [
        {"kind": "shape", "id": "1", "x": 10, "y": 10, "w": 100, "h": 50, "preset": "roundRect",
         "fill": {"type": "solid", "color": "#ff0000", "opacity": 0.5},
         "line": {"color": "#000000", "width": 2, "dash": "dash", "tailEnd": "triangle"},
         "text": {"vAlign": "middle", "insets": {"l": 7.2, "t": 3.6, "r": 7.2, "b": 3.6}, "autofit": "none", "wrap": true,
                  "paragraphs": [{"align": "center", "runs": [{"text": "Hi\nthere", "font": "Inter", "size": 20, "color": "#123456", "bold": true}],
                                  "bullet": {"type": "number", "scheme": "arabicPeriod"}}]}}]}]});
    s.execute("file.fromFlat", &json!({"deck": deck})).unwrap();
    assert_eq!(s.doc().unwrap().doc.slides.len(), 1);
    assert_eq!(s.doc().unwrap().doc.slide_size.width, 720.0);
    let d = flatten_cmd(&mut s);
    let FlatBody::Shape { fill, line, text: Some(t), .. } = &d.slides[0].elements[0].body else { panic!() };
    assert_eq!(fill, &Some(FlatFill::Solid { color: "#ff0000".into(), opacity: Some(128.0 / 255.0) }));
    let l = line.as_ref().unwrap();
    assert_eq!((l.dash.as_str(), l.width, l.tail_end.as_deref()), ("dash", 2.0, Some("triangle")));
    let p = &t.paragraphs[0];
    assert_eq!(p.runs.iter().map(|r| r.text.as_str()).collect::<String>(), "Hi\nthere");
    assert!(matches!(p.bullet, Some(FlatBullet::Number { .. })));
    assert_eq!(p.runs[0].color, "#123456");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().doc.slides.len(), n);
    // No document open: fromFlat opens one.
    let mut s = Session::new();
    s.execute("file.fromFlat", &json!({"deck": deck})).unwrap();
    assert_eq!(s.documents().len(), 1);
}

#[test]
fn media_from_params_and_staging() {
    let png = tiny_png();
    let deck = json!({"version": 1, "size": {"w": 960, "h": 540}, "media": [{"id": "p", "contentType": "image/png", "name": "p.png"}],
        "slides": [{"id": "s", "background": {"type": "image", "media": "p"},
                    "elements": [{"kind": "picture", "media": "p", "x": 0, "y": 0, "w": 40, "h": 20, "crop": {"l": 0.1, "t": 0, "r": 0, "b": 0}},
                                 {"kind": "raster", "media": "q", "x": 50, "y": 0, "w": 40, "h": 20}]}]});
    let mut s = Session::new();
    // Missing media is an error naming it.
    let e = s.execute("file.fromFlat", &json!({"deck": deck, "media": {"p": cmd::base64_encode(&png)}})).unwrap_err();
    assert!(e.to_string().contains("`q`"), "{e}");
    s.staged_media.insert("q".into(), png.clone());
    s.execute("file.fromFlat", &json!({"deck": deck, "media": {"p": cmd::base64_encode(&png)}})).unwrap();
    assert!(s.staged_media.is_empty());
    let doc = &s.doc().unwrap().doc;
    assert_eq!(doc.media.len(), 1, "identical bytes are stored once");
    let d = flatten_cmd(&mut s);
    let FlatBody::Picture { crop: Some(c), .. } = &d.slides[0].elements[0].body else { panic!() };
    assert!((c.l - 0.1).abs() < 1e-9);
    assert!(matches!(d.slides[0].background, Some(FlatFill::Image { .. })));
}

#[test]
fn hostile_flat_decks_are_errors_not_crashes() {
    let mut s = sample_session();
    let text = json!({"vAlign": "?", "insets": {"l": 1e308, "t": -1e308, "r": 0, "b": 0}, "autofit": "shrink", "fontScale": -4, "wrap": false, "columns": 99999,
        "paragraphs": [{"align": "?", "level": 200, "indent": 1e308, "lineSpacing": 1e308, "spaceBefore": -1,
                        "bullet": {"type": "char", "char": ""}, "runs": [{"text": "x", "size": 1e308, "color": "nope", "baseline": 1e9, "link": "#slide-99999999"}]}]});
    let shape = json!({"kind": "shape", "x": 1e308, "y": -1e308, "w": -1e300, "h": 1e300, "rotation": 1e308, "preset": "notAPreset", "adj": [1e308], "text": text});
    let table = json!({"kind": "table", "colWidths": [1e308, -1], "rows": [{"height": 1e308, "cells": [{"gridSpan": 4000000000u64, "rowSpan": 0, "text": {}}]}]});
    let chart = json!({"kind": "chart", "chartType": "pie", "media": "", "series": [{"name": "s", "values": [1e308, null], "color": "#zz"}]});
    let anim = json!({"kind": "shape", "preset": "rect", "animations": [{"type": "weird", "effect": "nonsense", "trigger": "?", "durationMs": 1e308, "order": 4000000000u64}]});
    let big = json!({"version": 1, "size": {"w": 1e308, "h": -5}, "slides": [{"id": "x", "elements": [shape, table, chart, anim]}]});
    s.execute("file.fromFlat", &json!({"deck": big})).unwrap();
    assert!(s.doc().unwrap().doc.validate().is_empty());
    let _ = flatten_cmd(&mut s);
    let _ = deckcraft_render::render_slide(&s.doc().unwrap().doc, 0, &deckcraft_render::RenderOpts { scale: 0.1, ..Default::default() });
    for bad in [
        json!({}),
        json!({"deck": null}),
        json!({"deck": {"version": 2, "size": {"w": 1, "h": 1}}}),
        json!({"deck": {"version": 1}}),
        json!({"deck": {"version": 1, "size": {"w": 1, "h": 1}, "slides": [{"elements": [{"kind": "nope"}]}]}}),
        json!({"deck": {"version": 1, "size": {"w": 1, "h": 1}, "slides": [{"elements": [{"kind": "picture", "media": "missing"}]}]}}),
        json!({"deck": {"version": 1, "size": {"w": 1, "h": 1}, "slides": [{"elements": [{"kind": "media", "mediaKind": "video", "media": "1"}]}]}}),
    ] {
        assert!(s.execute("file.fromFlat", &bad).is_err(), "{bad}");
    }
    assert!(s.doc().unwrap().doc.validate().is_empty());
}

#[test]
fn sniffs_common_types() {
    assert_eq!(sniff_content_type(&tiny_png()), "image/png");
    assert_eq!(sniff_content_type(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
    assert_eq!(sniff_content_type(b"\0\0\0\x18ftypmp42"), "video/mp4");
    assert_eq!(sniff_content_type(b""), "application/octet-stream");
}
