use serde_json::Value;

use super::*;

/// PPTX bytes of the engine's sample deck (built in code).
fn sample_pptx() -> Vec<u8> {
    let mut s = Session::new();
    deckcraft_engine::sample::open_sample(&mut s).unwrap();
    cmd::file::save_bytes(&s.doc().unwrap().doc, "pptx").unwrap()
}

#[test]
fn pptx_flat_pptx_pdf_png() {
    let mut d = Deck::new();
    d.open_bytes("sample.pptx", &sample_pptx()).unwrap();
    let flat: Value = serde_json::from_str(&d.flatten().unwrap()).unwrap();
    let slides = flat["slides"].as_array().unwrap().len();
    assert!(slides >= 8);
    // Every referenced media item is retrievable.
    for m in flat["media"].as_array().unwrap() {
        assert!(!d.media_bytes(m["id"].as_str().unwrap()).unwrap().is_empty());
    }
    // Back from the flat deck (media found in the open presentation and the last flatten).
    d.from_flat(&flat.to_string()).unwrap();
    assert_eq!(d.session().documents().len(), 1);
    let pptx = d.save_bytes("pptx").unwrap();
    assert!(pptx.starts_with(b"PK"));
    assert!(d.save_bytes("pdf").unwrap().starts_with(b"%PDF"));
    let png = d.render_png(0, 0.25).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    // Reopening replaces the presentation.
    d.open_bytes("again.pptx", &pptx).unwrap();
    assert_eq!(d.session().documents().len(), 1);
    assert_eq!(d.session().doc().unwrap().doc.slides.len(), slides);
}

#[test]
fn staged_media_feeds_from_flat() {
    let mut d = Deck::new();
    let png = deckcraft_engine::render::Image { width: 2, height: 2, pixels: vec![255; 16] }.to_png();
    d.put_media("img", &png);
    let deck = r#"{"version":1,"size":{"w":960,"h":540},"media":[{"id":"img","contentType":"image/png","name":"a.png"}],
        "slides":[{"id":"s1","elements":[{"kind":"picture","media":"img","x":10,"y":10,"w":100,"h":100}]}]}"#;
    d.from_flat(deck).unwrap();
    let flat: Value = serde_json::from_str(&d.flatten().unwrap()).unwrap();
    let id = flat["slides"][0]["elements"][0]["media"].as_str().unwrap().to_string();
    assert_eq!(d.media_bytes(&id).unwrap(), png);
}

#[test]
fn junk_is_an_error_not_a_crash() {
    let mut d = Deck::new();
    assert!(d.flatten().is_err(), "nothing open");
    assert!(d.save_bytes("pptx").is_err());
    assert!(d.render_png(0, 1.0).is_err());
    assert!(d.open_bytes("x.pptx", b"not a zip").is_err());
    assert!(d.from_flat("{").is_err());
    assert!(d.from_flat("[]").is_err());
    assert!(d.execute("no.such.command", "{}").is_err());
    assert!(d.execute("file.new", "{oops").is_err());
    d.execute("file.new", "").unwrap();
    assert!(d.save_bytes("docx").is_err());
    assert!(d.render_png(99, f64::NAN).is_err());
    assert!(d.render_png(0, f64::INFINITY).is_ok());
    assert!(d.media_bytes("nope").is_err());
    assert_eq!(add_font_bytes(b"not a font"), 0);
    assert!(!version().is_empty());
}
