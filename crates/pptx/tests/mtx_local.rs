//! Local check against real `.fntdata` files (never committed): set `DECKCRAFT_FNTDATA_DIR`
//! to a folder of extracted `ppt/fonts/*.fntdata` parts. Skips when unset.

use skrifa::{MetadataProvider, raw::TableProvider, string::StringId};

#[test]
fn local_fntdata_decode() {
    let Ok(dir) = std::env::var("DECKCRAFT_FNTDATA_DIR") else { return };
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    for p in names {
        let data = std::fs::read(&p).unwrap();
        let (font, ty) = deckcraft_pptx::fonts::decode(&p.to_string_lossy(), &data).unwrap_or_else(|e| panic!("{p:?}: {e:?}"));
        assert_eq!(&font[..4], &[0, 1, 0, 0], "{p:?}");
        let f = skrifa::FontRef::new(&font).unwrap_or_else(|e| panic!("{p:?}: {e}"));
        let name = |id| f.localized_strings(id).english_or_first().map(|s| s.to_string()).unwrap_or_default();
        let glyphs = f.maxp().unwrap().num_glyphs();
        // Every glyph outline must load.
        let outlines = f.outline_glyphs();
        let mut ok = 0;
        for g in 0..glyphs {
            if let Some(o) = outlines.get(skrifa::GlyphId::new(g.into())) {
                let mut pen = Count(0);
                if o.draw(skrifa::instance::Size::unscaled(), &mut pen).is_ok() {
                    ok += 1;
                }
            }
        }
        println!(
            "{}: {ty} {} bytes, family={:?} subfamily={:?}, glyphs={glyphs} outlines={ok}",
            p.display(),
            font.len(),
            name(StringId::FAMILY_NAME),
            name(StringId::SUBFAMILY_NAME)
        );
    }
}

struct Count(usize);
impl skrifa::outline::OutlinePen for Count {
    fn move_to(&mut self, _: f32, _: f32) {
        self.0 += 1
    }
    fn line_to(&mut self, _: f32, _: f32) {
        self.0 += 1
    }
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
        self.0 += 1
    }
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
        self.0 += 1
    }
    fn close(&mut self) {}
}
