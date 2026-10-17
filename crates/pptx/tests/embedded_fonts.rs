//! Embedded fonts (`p:embeddedFontLst`) are decoded to plain fonts on import. Synthetic package
//! built in code: a default export with font parts and relationships spliced in.

use std::io::{Cursor, Write};

use deckcraft_model::defaults;
use deckcraft_pptx::fonts;

fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).expect("entry");
            let mut d = vec![];
            std::io::Read::read_to_end(&mut f, &mut d).expect("read");
            (f.name().to_string(), d)
        })
        .collect()
}

fn rezip(es: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for (n, d) in es {
        z.start_file(n.as_str(), o).expect("start");
        z.write_all(d).expect("write");
    }
    z.finish().expect("finish").into_inner()
}

fn fake_ttf(seed: u8) -> Vec<u8> {
    let mut v = vec![0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    v.extend((0u8..80).map(|i| i.wrapping_mul(seed)));
    v
}

#[test]
fn embedded_fonts_are_decoded_on_import() {
    let mut es = entries(&deckcraft_pptx::export(&defaults::new_presentation(None)).expect("export"));
    let guid_part = "ppt/fonts/{6C4A1E2F-0B3D-4A5C-9E8F-112233445566}.odttf";
    let regular = fake_ttf(3);
    let mut obf = regular.clone();
    fonts::xor_obfuscation(&mut obf, &fonts::guid_key(guid_part).expect("key"));
    let bold = fake_ttf(5);
    // An MTX-compressed EOT (flags & 4): skipped, not crashed on.
    let mut mtx = vec![0u8; 36];
    mtx[0..4].copy_from_slice(&36u32.to_le_bytes());
    mtx[4..8].copy_from_slice(&4u32.to_le_bytes());
    mtx[8..12].copy_from_slice(&0x0002_0002u32.to_le_bytes());
    mtx[12..16].copy_from_slice(&4u32.to_le_bytes());
    es.push((guid_part.into(), obf));
    es.push(("ppt/fonts/font2.fntdata".into(), bold.clone()));
    es.push(("ppt/fonts/font3.fntdata".into(), mtx));
    let rel = |id: &str, t: &str| {
        format!(r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/font" Target="{t}"/>"#)
    };
    for (name, data) in &mut es {
        if name == "ppt/_rels/presentation.xml.rels" {
            let s = String::from_utf8(data.clone()).expect("utf8");
            let add =
                rel("rIdF1", guid_part.trim_start_matches("ppt/")) + &rel("rIdF2", "fonts/font2.fntdata") + &rel("rIdF3", "fonts/font3.fntdata");
            *data = s.replace("</Relationships>", &(add + "</Relationships>")).into_bytes();
        } else if name == "ppt/presentation.xml" {
            let s = String::from_utf8(data.clone()).expect("utf8");
            let lst = r#"<p:embeddedFontLst><p:embeddedFont><p:font typeface="Synth Sans"/><p:regular r:id="rIdF1"/><p:bold r:id="rIdF2"/><p:italic r:id="rIdF3"/></p:embeddedFont><p:embeddedFont><p:font typeface=""/><p:regular r:id="rIdF2"/></p:embeddedFont></p:embeddedFontLst>"#;
            *data = s.replace("</p:presentation>", &format!("{lst}</p:presentation>")).into_bytes();
        }
    }
    let p = deckcraft_pptx::import(&rezip(&es)).expect("import");
    assert_eq!(p.embedded_fonts.len(), 2, "{:?}", p.embedded_fonts);
    let face = |b: bool| p.embedded_fonts.iter().find(|f| f.bold == b && !f.italic).expect("face");
    assert_eq!(face(false).typeface, "Synth Sans");
    let bytes = |f: &deckcraft_model::EmbeddedFont| {
        p.media.iter().find(|m| m.id == f.media).map(|m| (m.data.to_vec(), m.content_type.clone())).expect("media")
    };
    assert_eq!(bytes(face(false)), (regular, "font/ttf".to_string()));
    assert_eq!(bytes(face(true)).0, bold);
}
