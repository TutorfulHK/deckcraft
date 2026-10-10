//! DeckCraft as a headless WebAssembly library for web hosts (run it in a Web Worker): open and
//! save PPTX, flatten a presentation to the flat deck (`docs/flat-deck.md`) and build one from
//! it, render slide PNGs, export PDF.
//!
//! A thin binding over engine commands: [`Deck`] holds the logic (plain Rust, tested natively)
//! and [`Engine`] exports it with wasm-bindgen. A panic aborts on wasm (there is no unwinding), so
//! nothing here may panic; the host restarts the worker if the module traps.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use deckcraft_engine::{Session, cmd, flat};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

/// The library's state and logic: one open presentation.
pub struct Deck {
    session: Session,
}

impl Default for Deck {
    fn default() -> Self {
        Self::new()
    }
}

impl Deck {
    pub fn new() -> Self {
        Deck { session: Session::new() }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Run any engine command; params and result are JSON text.
    pub fn execute(&mut self, id: &str, params_json: &str) -> Result<String, String> {
        let params: Value =
            if params_json.trim().is_empty() { json!({}) } else { serde_json::from_str(params_json).map_err(|e| format!("params: {e}"))? };
        let v = self.session.execute(id, &params).map_err(|e| e.to_string())?;
        serde_json::to_string(&v).map_err(|e| e.to_string())
    }

    fn close_all(&mut self) {
        while let Some(i) = self.session.active_index() {
            self.session.close_document(i);
        }
    }

    /// Open presentation bytes (PPTX or native), replacing the open one.
    pub fn open_bytes(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let doc = cmd::file::open_presentation(name, bytes).map_err(|e| e.to_string())?;
        self.close_all();
        self.session.flat_media.clear();
        cmd::file::add_opened(&mut self.session, name, None, doc);
        Ok(())
    }

    /// The open presentation as `pptx`, `pdf` or `deckcraft` bytes.
    pub fn save_bytes(&self, format: &str) -> Result<Vec<u8>, String> {
        let st = self.session.doc().map_err(|e| e.to_string())?;
        match format {
            "pptx" | "pdf" | "deckcraft" => cmd::file::save_bytes(&st.doc, format).map_err(|e| e.to_string()),
            other => Err(format!("unknown format `{other}` (pptx, pdf or deckcraft)")),
        }
    }

    /// The flat deck of the open presentation, as JSON text.
    pub fn flatten(&mut self) -> Result<String, String> {
        self.execute("file.flatten", "{}")
    }

    /// Stage media bytes for the next [`Deck::from_flat`] (avoids base64 for big blobs).
    pub fn put_media(&mut self, id: &str, bytes: &[u8]) {
        self.session.staged_media.insert(id.to_string(), bytes.to_vec());
    }

    /// Replace the open presentation (or open one) with one built from a flat deck (JSON text).
    pub fn from_flat(&mut self, deck_json: &str) -> Result<(), String> {
        let deck: Value = serde_json::from_str(deck_json).map_err(|e| format!("deck: {e}"))?;
        self.session.execute("file.fromFlat", &json!({"deck": deck})).map(|_| ()).map_err(|e| e.to_string())
    }

    /// Bytes of a media id the flat deck references.
    pub fn media_bytes(&self, id: &str) -> Result<Vec<u8>, String> {
        flat::media_bytes(&self.session, id).map(|b| b.data.to_vec()).ok_or_else(|| format!("no media `{id}`"))
    }

    /// Slide `slide` (0-based) as PNG at `scale` pixels per point.
    pub fn render_png(&self, slide: u32, scale: f64) -> Result<Vec<u8>, String> {
        let st = self.session.doc().map_err(|e| e.to_string())?;
        let i = usize::try_from(slide).map_err(|e| e.to_string())?;
        if i >= st.doc.slides.len() {
            return Err(format!("no slide {i}"));
        }
        // Capped so a slide stays a sane allocation (4 px/pt: 3840 × 2160 for 16:9).
        let scale = if scale.is_finite() && scale > 0.0 { scale.clamp(0.01, 4.0) } else { 1.0 };
        let (png, _, _) = cmd::file::render_png(&st.doc, i, scale, false);
        if png.is_empty() { Err("the slide could not be encoded".into()) } else { Ok(png) }
    }
}

/// Add a font (TTF/OTF/TTC bytes) for text layout and rendering; returns the faces added.
pub fn add_font_bytes(bytes: &[u8]) -> u32 {
    u32::try_from(deckcraft_fonts::FontDb::global().add_font(bytes.to_vec())).unwrap_or(u32::MAX)
}

fn js(e: String) -> JsError {
    JsError::new(&e)
}

/// The JS-facing engine: one open presentation.
#[wasm_bindgen]
pub struct Engine {
    deck: Deck,
}

#[wasm_bindgen]
impl Engine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Engine {
        Engine { deck: Deck::new() }
    }

    /// Run any engine command by id with JSON params; returns the JSON result.
    pub fn execute(&mut self, id: &str, params_json: &str) -> Result<String, JsError> {
        self.deck.execute(id, params_json).map_err(js)
    }

    #[wasm_bindgen(js_name = openBytes)]
    pub fn open_bytes(&mut self, name: &str, bytes: &[u8]) -> Result<(), JsError> {
        self.deck.open_bytes(name, bytes).map_err(js)
    }

    /// `pptx`, `pdf` or `deckcraft`.
    #[wasm_bindgen(js_name = saveBytes)]
    pub fn save_bytes(&self, format: &str) -> Result<Vec<u8>, JsError> {
        self.deck.save_bytes(format).map_err(js)
    }

    pub fn flatten(&mut self) -> Result<String, JsError> {
        self.deck.flatten().map_err(js)
    }

    #[wasm_bindgen(js_name = fromFlat)]
    pub fn from_flat(&mut self, deck_json: &str) -> Result<(), JsError> {
        self.deck.from_flat(deck_json).map_err(js)
    }

    #[wasm_bindgen(js_name = putMedia)]
    pub fn put_media(&mut self, id: &str, bytes: &[u8]) {
        self.deck.put_media(id, bytes)
    }

    #[wasm_bindgen(js_name = mediaBytes)]
    pub fn media_bytes(&self, id: &str) -> Result<Vec<u8>, JsError> {
        self.deck.media_bytes(id).map_err(js)
    }

    #[wasm_bindgen(js_name = renderPng)]
    pub fn render_png(&self, slide: u32, scale: f64) -> Result<Vec<u8>, JsError> {
        self.deck.render_png(slide, scale).map_err(js)
    }

    /// The library version (also the free function `version()`).
    #[wasm_bindgen(js_name = version)]
    pub fn engine_version() -> String {
        version()
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// Add a font (TTF/OTF/TTC bytes); returns the number of faces added.
#[wasm_bindgen(js_name = addFont)]
pub fn add_font(bytes: &[u8]) -> u32 {
    add_font_bytes(bytes)
}

/// The library version.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests;
