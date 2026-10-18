//! The flat deck: a presentation with every inherited value resolved (placeholders, layouts,
//! masters, theme) into absolutely positioned, self-contained elements, for web hosts that keep
//! their own simple slide model (`file.flatten` / `file.fromFlat`, the `deckcraft-wasm` library).
//! What the schema can't express faithfully is rendered to PNG and sent as a `raster` element.
//! Schema: [`schema`] and `docs/flat-deck.md`.

mod build;
pub mod schema;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use deckcraft_color::Rgba;
use deckcraft_geom::Xfrm;
use deckcraft_geom::{Affine, Rect};
use deckcraft_model::resolve::{self, Ctx};
use deckcraft_model::style::{Fill, Line, PictureFill, PictureMode};
use deckcraft_model::text::{Action, Anchor, AutoFit, Bullet, Caps, Paragraph, RunKind, Spacing, Strike, TextBody, TextDir};
use deckcraft_model::{AnimClass, AnimStart, Geom, MediaId, Presentation, Shape, ShapeId, ShapeKind, Slide};
use deckcraft_render::Layer;

pub use build::unflatten;
pub use schema::*;

/// Pixels per point of the PNGs made for raster elements, chart pictures and backgrounds.
pub const RASTER_SCALE: f64 = 2.0;

/// Preset geometries emitted as vector `shape` elements: exactly the ones the web host's JS SVG
/// renderer (the consumer of `file.flatten`) can draw. Any other preset, custom geometry, a
/// non-solid fill or outline, or visible effects make the shape a `raster` element.
pub const VECTOR_PRESETS: &[&str] = &[
    "rect",
    "roundRect",
    "ellipse",
    "triangle",
    "rtTriangle",
    "diamond",
    "parallelogram",
    "trapezoid",
    "pentagon",
    "hexagon",
    "octagon",
    "star5",
    "rightArrow",
    "leftArrow",
    "upArrow",
    "downArrow",
    "chevron",
    "homePlate",
    "line",
    "straightConnector1",
    "plus",
];

/// Image types a browser shows directly; others (EMF, WMF, TIFF…) are rendered to PNG.
const WEB_IMAGES: &[&str] = &["image/png", "image/jpeg", "image/jpg", "image/gif", "image/webp", "image/svg+xml", "image/bmp", "image/avif"];

const MAX_DEPTH: usize = 32;

/// Bytes of a media item referenced by a flat deck.
#[derive(Clone, Debug)]
pub struct FlatBlob {
    pub content_type: String,
    pub name: String,
    pub data: Arc<Vec<u8>>,
}

pub fn hex(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

fn opacity(c: Rgba) -> Option<f64> {
    (c.a < 255).then(|| c.a as f64 / 255.0)
}

fn solid(c: Rgba) -> FlatFill {
    FlatFill::Solid { color: hex(c), opacity: opacity(c) }
}

/// Field values as drawn on slide `index`.
struct Fields<'a> {
    pres: &'a Presentation,
    index: usize,
}

impl deckcraft_text::Fields for Fields<'_> {
    fn field(&self, kind: &str) -> Option<String> {
        if kind == "slidenum" {
            return Some(self.pres.slide_number(self.index).to_string());
        }
        if kind.starts_with("datetime") {
            return Some(self.pres.header_footer.date_text.clone());
        }
        if kind == "footer" {
            return Some(self.pres.header_footer.footer_text.clone());
        }
        None
    }
}

/// Collected media: deck items referenced, and PNGs made while flattening.
#[derive(Default)]
struct Out {
    media: Vec<FlatMedia>,
    seen: HashSet<String>,
    blobs: HashMap<String, FlatBlob>,
}

impl Out {
    fn doc_media(&mut self, pres: &Presentation, id: MediaId) -> Option<String> {
        let m = pres.media(id)?;
        let key = id.0.to_string();
        if self.seen.insert(key.clone()) {
            self.media.push(FlatMedia { id: key.clone(), content_type: m.content_type.clone(), name: m.name.clone() });
        }
        Some(key)
    }
    fn png(&mut self, key: String, png: Vec<u8>) -> Option<String> {
        if png.is_empty() {
            return None;
        }
        let name = format!("{key}.png");
        if self.seen.insert(key.clone()) {
            self.media.push(FlatMedia { id: key.clone(), content_type: "image/png".into(), name: name.clone() });
        }
        self.blobs.insert(key.clone(), FlatBlob { content_type: "image/png".into(), name, data: Arc::new(png) });
        Some(key)
    }
}

/// One shape tree of one slide being flattened.
struct Cx<'a> {
    pres: &'a Presentation,
    slide: &'a Slide,
    index: usize,
    layer: Layer,
    ctx: &'a Ctx<'a>,
    fields: Fields<'a>,
    /// Prefix making element ids of layout/master graphics distinct from the slide's.
    prefix: String,
    from_master: bool,
}

impl Cx<'_> {
    fn elem_id(&self, id: ShapeId) -> String {
        format!("{}{}", self.prefix, id)
    }
    fn media_key(&self, id: ShapeId, what: &str) -> String {
        format!("x{}-{}{}{}", self.slide.id, self.prefix, id, what)
    }
}

/// An element plus the slide shape it came from (top-level shape, own shape) for animations.
struct Emitted {
    el: FlatElement,
    origin: Option<(ShapeId, ShapeId)>,
}

/// Flatten `pres`: the flat deck and the bytes of the PNGs made for it.
pub fn flatten(pres: &Presentation) -> (FlatDeck, HashMap<String, FlatBlob>) {
    let mut out = Out::default();
    let mut slides = Vec::with_capacity(pres.slides.len());
    for (index, slide) in pres.slides.iter().enumerate() {
        slides.push(flat_slide(pres, index, slide, &mut out));
    }
    let fonts = pres
        .embedded_fonts
        .iter()
        .filter_map(|f| {
            let media_id = out.doc_media(pres, f.media)?;
            Some(FlatFont { typeface: f.typeface.clone(), style: FlatFontStyle { bold: f.bold, italic: f.italic }, media_id })
        })
        .collect();
    let deck =
        FlatDeck { version: FLAT_VERSION, size: FlatSize { w: pres.slide_size.width, h: pres.slide_size.height }, slides, media: out.media, fonts };
    (deck, out.blobs)
}

fn flat_slide(pres: &Presentation, index: usize, slide: &Slide, out: &mut Out) -> FlatSlide {
    let mut fs = FlatSlide {
        id: slide.id.to_string(),
        hidden: slide.hidden,
        notes: slide.notes_text(),
        background: None,
        transition: slide.transition.as_ref().filter(|t| t.kind != "none").map(|t| FlatTransition {
            kind: t.kind.clone(),
            duration_ms: t.duration_ms as f64,
            option: t.option.clone(),
        }),
        elements: vec![],
    };
    let Some(sctx) = Ctx::for_slide(pres, slide) else { return fs };
    fs.background = flat_background(pres, index, slide, &sctx, out);
    let mut emitted: Vec<Emitted> = vec![];
    let (show_layout, show_master) = resolve::show_master_shapes(slide, sctx.layout);
    if show_master {
        let mctx = Ctx::for_master(pres, sctx.master);
        let cx = Cx {
            pres,
            slide,
            index,
            layer: Layer::Master,
            ctx: &mctx,
            fields: Fields { pres, index },
            prefix: format!("m{}-", sctx.master.id),
            from_master: true,
        };
        for s in sctx.master.shapes.iter().filter(|s| s.ph.is_none()) {
            emit(&cx, out, s, (0.0, 0.0), None, 0, &mut emitted);
        }
    }
    if show_layout && let Some(l) = sctx.layout {
        let lctx = Ctx::for_layout(pres, sctx.master, l);
        let cx = Cx {
            pres,
            slide,
            index,
            layer: Layer::Layout,
            ctx: &lctx,
            fields: Fields { pres, index },
            prefix: format!("l{}-", l.id),
            from_master: true,
        };
        for s in l.shapes.iter().filter(|s| s.ph.is_none()) {
            emit(&cx, out, s, (0.0, 0.0), None, 0, &mut emitted);
        }
    }
    let cx = Cx { pres, slide, index, layer: Layer::Slide, ctx: &sctx, fields: Fields { pres, index }, prefix: String::new(), from_master: false };
    for s in &slide.shapes {
        emit(&cx, out, s, (0.0, 0.0), Some(s.id), 0, &mut emitted);
    }
    attach_animations(slide, &mut emitted);
    fs.elements = emitted.into_iter().map(|e| e.el).collect();
    fs
}

fn attach_animations(slide: &Slide, emitted: &mut [Emitted]) {
    for (order, a) in slide.animations.iter().enumerate() {
        let own: Vec<usize> = emitted.iter().enumerate().filter(|(_, e)| e.origin.is_some_and(|o| o.1 == a.shape)).map(|(i, _)| i).collect();
        let targets = if own.is_empty() {
            emitted.iter().enumerate().filter(|(_, e)| e.origin.is_some_and(|o| o.0 == a.shape)).map(|(i, _)| i).collect()
        } else {
            own
        };
        for (k, i) in targets.into_iter().enumerate() {
            let trigger = if k == 0 { a.start } else { AnimStart::WithPrevious };
            let fa = FlatAnimation {
                kind: match a.class {
                    AnimClass::Entrance => "entrance",
                    AnimClass::Emphasis => "emphasis",
                    AnimClass::Exit => "exit",
                    AnimClass::Path => "path",
                    AnimClass::Media => "media",
                }
                .into(),
                effect: a.effect.clone(),
                option: a.option.clone(),
                trigger: match trigger {
                    AnimStart::OnClick => "onClick",
                    AnimStart::WithPrevious => "withPrevious",
                    AnimStart::AfterPrevious => "afterPrevious",
                }
                .into(),
                duration_ms: a.duration_ms as f64,
                delay_ms: a.delay_ms as f64,
                order: u32::try_from(order).unwrap_or(u32::MAX),
            };
            if let Some(e) = emitted.get_mut(i) {
                e.el.animations.push(fa);
            }
        }
    }
}

fn simple_picture(pres: &Presentation, pf: &PictureFill) -> bool {
    let stretch = match &pf.mode {
        PictureMode::Stretch { fill_rect } => fill_rect.iter().all(|v| v.abs() < 1e-6),
        PictureMode::Tile { .. } => false,
    };
    stretch
        && pf.adjust == Default::default()
        && pf.crop.iter().all(|v| v.is_finite() && (0.0..1.0).contains(v))
        && pres.media(pf.media).is_some_and(|m| !m.data.is_empty() && WEB_IMAGES.contains(&m.content_type.to_ascii_lowercase().as_str()))
}

fn flat_background(pres: &Presentation, index: usize, slide: &Slide, ctx: &Ctx, out: &mut Out) -> Option<FlatFill> {
    let (fill, ph) = resolve::background(ctx, Some(slide));
    match &fill {
        Fill::None => None,
        Fill::Solid { color } => Some(solid(ctx.color(color, ph))),
        Fill::Picture(pf) if simple_picture(pres, pf) => out.doc_media(pres, pf.media).map(|media| FlatFill::Image { media }),
        _ => {
            let img = deckcraft_render::render_background(pres, index, RASTER_SCALE);
            out.png(format!("x{}-bg", slide.id), img.to_png()).map(|media| FlatFill::Image { media })
        }
    }
}

/// The colour of a fill when it is one plain colour.
fn solid_color(ctx: &Ctx, f: &Fill, ph: Option<Rgba>) -> Option<Rgba> {
    match f {
        Fill::Solid { color } => Some(ctx.color(color, ph)),
        _ => None,
    }
}

/// Is the shape's outline something the flat schema draws (none, or one solid colour)?
fn flat_line(ctx: &Ctx, shape: &Shape) -> Result<Option<FlatLine>, ()> {
    let (line, ph) = resolve::line(ctx, shape);
    line_to_flat(ctx, &line, ph)
}

fn line_to_flat(ctx: &Ctx, line: &Line, ph: Option<Rgba>) -> Result<Option<FlatLine>, ()> {
    let c = match &line.fill {
        None | Some(Fill::None) => return Ok(None),
        Some(Fill::Solid { color }) => ctx.color(color, ph),
        Some(_) => return Err(()),
    };
    let dash = match &line.dash {
        Some(deckcraft_model::style::Dash::Preset { name }) => name.clone(),
        Some(deckcraft_model::style::Dash::Custom { .. }) => "dash".into(),
        None => "solid".into(),
    };
    let end = |e: &Option<deckcraft_model::style::LineEnd>| e.as_ref().filter(|e| e.kind != "none").map(|e| e.kind.clone());
    Ok(Some(FlatLine {
        color: hex(c),
        width: line.width.unwrap_or(0.75).max(0.0),
        dash,
        opacity: opacity(c),
        head_end: end(&line.head),
        tail_end: end(&line.tail),
    }))
}

fn common(cx: &Cx, s: &Shape, x: &Xfrm, body: FlatBody) -> FlatElement {
    FlatElement {
        id: cx.elem_id(s.id),
        name: s.name.clone(),
        x: x.x,
        y: x.y,
        w: x.w,
        h: x.h,
        rotation: x.rot,
        flip_h: x.flip_h,
        flip_v: x.flip_v,
        alt: s.descr.clone(),
        from_master: cx.from_master,
        placeholder: s.ph.as_ref().map(|p| p.kind.xml().to_string()),
        animations: vec![],
        body,
    }
}

/// Flatten one shape (recursing into groups) at offset `off` (the enclosing groups' translation).
fn emit(cx: &Cx, out: &mut Out, s: &Shape, off: (f64, f64), top: Option<ShapeId>, depth: usize, acc: &mut Vec<Emitted>) {
    if depth > MAX_DEPTH || s.hidden {
        return;
    }
    let lx = resolve::xfrm(cx.ctx, s);
    if !lx.is_finite() {
        return;
    }
    let x = Xfrm { x: lx.x + off.0, y: lx.y + off.1, ..lx };
    let origin = top.map(|t| (t, s.id));
    let push = |acc: &mut Vec<Emitted>, el: FlatElement| acc.push(Emitted { el, origin });
    match &s.kind {
        ShapeKind::Group { children, child } => {
            let (cw, ch) = (child.w, child.h);
            let unit = |g: f64, c: f64| c.abs() < 1e-9 || (g / c - 1.0).abs() < 1e-3;
            if x.rot.abs() > 1e-6 || x.flip_h || x.flip_v || !unit(x.w, cw) || !unit(x.h, ch) {
                if let Some(el) = raster(cx, out, s, off) {
                    push(acc, el);
                }
                return;
            }
            let goff = (x.x - child.x, x.y - child.y);
            for c in children {
                emit(cx, out, c, goff, top, depth + 1, acc);
            }
        }
        ShapeKind::Shape | ShapeKind::Connector { .. } => emit_shape(cx, out, s, &x, off, &mut |el| push(acc, el)),
        ShapeKind::Picture { fill: pf } => {
            let geo_rect = s.geom.preset_name() == Some("rect");
            let (fx, _) = resolve::effects(cx.ctx, s);
            let line = flat_line(cx.ctx, s);
            let media =
                (geo_rect && fx.is_none_or(|e| e.is_empty()) && simple_picture(cx.pres, pf)).then(|| out.doc_media(cx.pres, pf.media)).flatten();
            match (media, line) {
                (Some(media), Ok(line)) => {
                    let [l, t, r, b] = pf.crop;
                    let crop = pf.crop.iter().any(|v| v.abs() > 1e-9).then_some(FlatCrop { l, t, r, b });
                    let opacity = pf.alpha.filter(|a| a.is_finite() && *a < 0.999).map(|a| a.clamp(0.0, 1.0));
                    push(acc, common(cx, s, &x, FlatBody::Picture { media, crop, opacity, line }));
                }
                _ => {
                    if let Some(el) = raster(cx, out, s, off) {
                        push(acc, el);
                    }
                }
            }
        }
        ShapeKind::Media(mc) => match out.doc_media(cx.pres, mc.media) {
            Some(media) => {
                let poster = mc.poster.and_then(|p| out.doc_media(cx.pres, p));
                push(acc, common(cx, s, &x, FlatBody::Media { media_kind: if mc.video { "video" } else { "audio" }.into(), media, poster }));
            }
            None => {
                if let Some(el) = raster(cx, out, s, off) {
                    push(acc, el);
                }
            }
        },
        ShapeKind::Table(t) => push(acc, flat_table(cx, s, &x, t)),
        ShapeKind::Chart(c) => match flat_chart(cx, out, s, &x, c) {
            Some(el) => push(acc, el),
            None => {
                if let Some(el) = raster(cx, out, s, off) {
                    push(acc, el);
                }
            }
        },
        ShapeKind::Ink { .. } | ShapeKind::Opaque { .. } => {
            if let Some(el) = raster(cx, out, s, off) {
                push(acc, el);
            }
        }
    }
}

fn emit_shape(cx: &Cx, out: &mut Out, s: &Shape, x: &Xfrm, off: (f64, f64), push: &mut dyn FnMut(FlatElement)) {
    let has_text = s.text.as_ref().is_some_and(|t| !t.is_empty());
    let bp = resolve::body(cx.ctx, s);
    // Text the flat schema can't lay out (WordArt warps, vertical or rotated text) stays in the
    // picture.
    let odd_text = has_text && (bp.warp.is_some() || !matches!(bp.vert, None | Some(TextDir::Horizontal)) || bp.rot.is_some_and(|r| r.abs() > 1e-6));
    let preset = match &s.geom {
        Geom::Preset { name, adj } if VECTOR_PRESETS.contains(&name.as_str()) => Some((name.clone(), adj.clone())),
        _ => None,
    };
    let (fill, fill_ph) = resolve::fill(cx.ctx, s);
    let fill = match &fill {
        None | Some(Fill::None) => Ok(None),
        Some(Fill::Group) | Some(Fill::Background) => Err(()),
        Some(f) => solid_color(cx.ctx, f, fill_ph).map(|c| Some(solid(c))).ok_or(()),
    };
    let line = flat_line(cx.ctx, s);
    let (fx, _) = resolve::effects(cx.ctx, s);
    let effects = fx.is_some_and(|e| !e.is_empty());
    match (preset, fill, line) {
        (Some((preset, adj)), Ok(fill), Ok(line)) if !effects && !odd_text => {
            let text = s.text.as_ref().map(|t| flat_text(cx, s, t, &preset, &adj, x.w, x.h));
            push(common(cx, s, x, FlatBody::Shape { preset, adj, fill, line, text }));
        }
        _ => {
            // The picture without its text, the text on top as a transparent box, so it stays
            // editable.
            let overlay = has_text && !odd_text;
            let pic = if overlay { Shape { text: None, ..s.clone() } } else { s.clone() };
            if let Some(el) = raster(cx, out, &pic, off) {
                push(el);
            }
            if overlay && let Some(t) = &s.text {
                let (name, adj) = match &s.geom {
                    Geom::Preset { name, adj } => (name.clone(), adj.clone()),
                    Geom::Custom { .. } => ("rect".to_string(), vec![]),
                };
                let text = flat_text(cx, s, t, &name, &adj, x.w, x.h);
                let mut el = common(cx, s, x, FlatBody::Shape { preset: "rect".into(), adj: vec![], fill: None, line: None, text: Some(text) });
                el.id = format!("{}-text", el.id);
                push(el);
            }
        }
    }
}

/// A `raster` element: the shape drawn to PNG over its slide-space bounds (outlines and effects
/// included).
fn raster(cx: &Cx, out: &mut Out, s: &Shape, off: (f64, f64)) -> Option<FlatElement> {
    let (img, r) = deckcraft_render::render_shape_on(cx.pres, cx.index, cx.layer, s, Affine::translate(off), RASTER_SCALE)?;
    let media = out.png(cx.media_key(s.id, ""), img.to_png())?;
    let x = Xfrm::new(r.x0, r.y0, r.width(), r.height());
    let mut el = common(cx, s, &x, FlatBody::Raster { media });
    el.placeholder = None;
    Some(el)
}

fn v_align(a: Option<Anchor>) -> &'static str {
    match a {
        Some(Anchor::Middle) | Some(Anchor::Distributed) => "middle",
        Some(Anchor::Bottom) => "bottom",
        _ => "top",
    }
}

/// The preset's text rectangle in a `w × h` box.
fn text_rect(preset: &str, adj: &[f64], w: f64, h: f64) -> Rect {
    deckcraft_geom::preset::build(preset, w, h, adj).map(|g| g.text_rect).unwrap_or_else(|| Rect::new(0.0, 0.0, w, h))
}

fn flat_text(cx: &Cx, s: &Shape, body: &TextBody, preset: &str, adj: &[f64], w: f64, h: f64) -> FlatText {
    let tr = text_rect(preset, adj, w, h);
    text_in(cx.ctx, &cx.fields, s, body, tr, w, h)
}

/// A text body laid out in `tr` (shape-local) of a `w × h` box.
fn text_in(ctx: &Ctx, fields: &Fields, s: &Shape, body: &TextBody, tr: Rect, w: f64, h: f64) -> FlatText {
    let bp = resolve::body(ctx, s);
    let insets = FlatInsets {
        l: tr.x0 + bp.inset_l.unwrap_or(7.2),
        t: tr.y0 + bp.inset_t.unwrap_or(3.6),
        r: (w - tr.x1) + bp.inset_r.unwrap_or(7.2),
        b: (h - tr.y1) + bp.inset_b.unwrap_or(3.6),
    };
    let autofit = match bp.autofit {
        Some(AutoFit::Shrink { .. }) => "shrink",
        Some(AutoFit::Shape) => "resize",
        _ => "none",
    };
    // PowerPoint stores the scale it settled on (`<a:normAutofit fontScale lnSpcReduction>`);
    // use it as-is, and only search for one when the file stored none.
    let stored = match bp.autofit {
        Some(AutoFit::Shrink { font_scale, line_reduction }) if font_scale.is_finite() && font_scale < 0.999 => {
            Some((font_scale.clamp(0.01, 1.0), line_reduction))
        }
        _ => None,
    };
    let (font_scale, line_reduction) = if let Some((fs, lr)) = stored {
        (Some(fs), (lr.is_finite() && lr > 0.0).then(|| lr.clamp(0.0, 1.0)))
    } else if autofit == "shrink" && !body.is_empty() {
        let l = deckcraft_text::layout(ctx, s, body, &deckcraft_text::Opts { rect: tr, fields, prompt_color: None, no_shrink: false });
        ((l.font_scale.is_finite() && l.font_scale < 0.999).then_some(l.font_scale), None)
    } else {
        (None, None)
    };
    FlatText {
        v_align: v_align(bp.anchor).into(),
        insets,
        autofit: autofit.into(),
        wrap: bp.wrap.unwrap_or(true),
        font_scale,
        line_reduction,
        columns: bp.columns.filter(|c| *c > 1),
        paragraphs: body.paragraphs.iter().map(|p| flat_para(ctx, fields, s, p)).collect(),
    }
}

fn spacing_pt(s: Option<Spacing>, size: f64) -> f64 {
    match s {
        Some(Spacing::Pct(p)) => p * size * 1.2,
        Some(Spacing::Pts(v)) => v,
        None => 0.0,
    }
}

fn flat_para(ctx: &Ctx, fields: &Fields, s: &Shape, para: &Paragraph) -> FlatParagraph {
    let pp = resolve::para(ctx, s, para);
    let lead = para.runs.first().map(|r| resolve::run(ctx, s, para, &r.props)).unwrap_or_else(|| resolve::run(ctx, s, para, &para.end_props));
    let lead_size = lead.size.unwrap_or(18.0);
    let (line_spacing, line_spacing_pt) = match pp.line_spacing {
        Some(Spacing::Pct(f)) => (Some(f), None),
        Some(Spacing::Pts(v)) => (None, Some(v)),
        None => (Some(1.0), None),
    };
    let bcolor = pp.bullet_color.as_ref().map(|c| hex(ctx.color(c, None)));
    let bullet = match &pp.bullet {
        Some(Bullet::Char { char }) => {
            let font = pp.bullet_font.as_deref().map(|f| ctx.master.theme.font(f));
            let mapped = deckcraft_text::map_symbol_bullet(char, font.as_deref());
            let symbol = mapped != *char || font.as_deref().is_some_and(|f| f.to_ascii_lowercase().contains("wingdings"));
            Some(FlatBullet::Char { char: mapped, color: bcolor, font: if symbol { None } else { font }, size: pp.bullet_size })
        }
        Some(Bullet::AutoNum { scheme, start_at }) => {
            Some(FlatBullet::Number { scheme: scheme.clone(), start_at: *start_at, color: bcolor, size: pp.bullet_size })
        }
        Some(Bullet::Picture { .. }) => Some(FlatBullet::Char { char: "•".into(), color: bcolor, font: None, size: pp.bullet_size }),
        Some(Bullet::None) | None => None,
    };
    let mut runs: Vec<FlatRun> = para
        .runs
        .iter()
        .map(|r| {
            let (text, field) = match &r.kind {
                RunKind::Text | RunKind::Math { .. } => (r.text.replace('\u{b}', "\n"), None),
                RunKind::Break => ("\n".to_string(), None),
                RunKind::Field { field } => (deckcraft_text::Fields::field(fields, field).unwrap_or_else(|| r.text.clone()), Some(field.clone())),
            };
            flat_run(ctx, &resolve::run(ctx, s, para, &r.props), text, field)
        })
        .collect();
    if runs.is_empty() {
        runs.push(flat_run(ctx, &lead, String::new(), None));
    }
    FlatParagraph {
        align: match pp.align {
            Some(deckcraft_model::text::Align::Center) => "center",
            Some(deckcraft_model::text::Align::Right) => "right",
            Some(deckcraft_model::text::Align::Justify | deckcraft_model::text::Align::JustLow | deckcraft_model::text::Align::Distributed) => {
                "justify"
            }
            _ => "left",
        }
        .into(),
        level: para.level.min(8),
        indent: pp.indent.unwrap_or(0.0),
        margin_left: pp.margin_left.unwrap_or(0.0),
        space_before: spacing_pt(pp.space_before, lead_size),
        space_after: spacing_pt(pp.space_after, lead_size),
        line_spacing,
        line_spacing_pt,
        rtl: pp.rtl.unwrap_or(false),
        bullet,
        runs,
    }
}

fn flat_run(ctx: &Ctx, rp: &deckcraft_model::RunProps, text: String, field: Option<String>) -> FlatRun {
    let color = resolve::text_color(ctx, rp);
    let font_ea = rp.font_ea.as_deref().map(|f| ctx.master.theme.font(f)).unwrap_or_default();
    let link = rp.link.as_ref().and_then(|l| match &l.action {
        Action::Url { url } => Some(url.clone()),
        Action::Slide { slide } => Some(format!("#slide-{slide}")),
        Action::NextSlide => Some("#next".into()),
        Action::PreviousSlide => Some("#previous".into()),
        Action::FirstSlide => Some("#first".into()),
        Action::LastSlide => Some("#last".into()),
        _ => None,
    });
    FlatRun {
        text,
        font: resolve::font_family(ctx, rp),
        font_ea,
        size: rp.size.unwrap_or(18.0),
        bold: rp.bold.unwrap_or(false),
        italic: rp.italic.unwrap_or(false),
        underline: rp.underline.as_deref().is_some_and(|u| u != "none"),
        strike: matches!(rp.strike, Some(Strike::Single | Strike::Double)),
        color: hex(color),
        opacity: opacity(color),
        highlight: rp.highlight.as_ref().map(|c| hex(ctx.color(c, None))),
        link,
        baseline: rp.baseline.filter(|b| b.is_finite()).unwrap_or(0.0),
        caps: match rp.caps {
            Some(Caps::All) => Some("all".into()),
            Some(Caps::Small) => Some("small".into()),
            _ => None,
        },
        spacing: rp.spacing.filter(|v| v.is_finite()).unwrap_or(0.0),
        field,
    }
}

fn flat_table(cx: &Cx, s: &Shape, x: &Xfrm, t: &deckcraft_model::Table) -> FlatElement {
    let heights = deckcraft_render::table_row_heights(cx.ctx, t, &cx.fields);
    let ncols = t.cols.len();
    let rows: Vec<FlatRow> = t
        .rows
        .iter()
        .enumerate()
        .map(|(ri, row)| {
            let height = heights.get(ri).copied().unwrap_or(row.height);
            let cells = row
                .cells
                .iter()
                .take(ncols)
                .enumerate()
                .map(|(ci, cell)| {
                    let lk = deckcraft_render::table_cell_look(cx.ctx, t, ri, ci);
                    let fill = match &cell.fill {
                        Some(f) => solid_color(cx.ctx, f, None).map(solid),
                        None => lk.fill.map(solid),
                    };
                    let border = |i: usize| -> Option<FlatLine> {
                        match cell.borders.get(i).and_then(|b| b.as_ref()) {
                            Some(l) => line_to_flat(cx.ctx, l, None).ok().flatten(),
                            None => lk.border.map(|(c, w)| FlatLine {
                                color: hex(c),
                                width: w,
                                dash: "solid".into(),
                                opacity: opacity(c),
                                head_end: None,
                                tail_end: None,
                            }),
                        }
                    };
                    let body = deckcraft_render::table_cell_body(cx.ctx, t, ri, ci, cell);
                    let tmp = Shape { text: Some(body.clone()), ..Default::default() };
                    // Text sits in the cell rectangle; its width matters only for shrink.
                    let w: f64 = t.cols.iter().skip(ci).take(cell.grid_span.max(1) as usize).sum();
                    let text = text_in(cx.ctx, &cx.fields, &tmp, &body, Rect::new(0.0, 0.0, w, height), w, height);
                    FlatCell {
                        text,
                        fill,
                        borders: Some(FlatBorders { l: border(0), r: border(1), t: border(2), b: border(3) }),
                        grid_span: cell.grid_span.max(1),
                        row_span: cell.row_span.max(1),
                        h_merge: cell.h_merge,
                        v_merge: cell.v_merge,
                    }
                })
                .collect();
            FlatRow { height, cells }
        })
        .collect();
    let bx = Xfrm { w: t.cols.iter().sum(), h: rows.iter().map(|r| r.height).sum(), ..*x };
    common(cx, s, &bx, FlatBody::Table { col_widths: t.cols.clone(), rows })
}

fn flat_chart(cx: &Cx, out: &mut Out, s: &Shape, x: &Xfrm, c: &deckcraft_model::Chart) -> Option<FlatElement> {
    use deckcraft_model::chart::ChartType as T;
    let (chart_type, grouping) = match c.kind {
        T::Column => ("column", Some("clustered")),
        T::StackedColumn => ("column", Some("stacked")),
        T::PercentColumn => ("column", Some("percent")),
        T::Bar => ("bar", Some("clustered")),
        T::StackedBar => ("bar", Some("stacked")),
        T::PercentBar => ("bar", Some("percent")),
        T::Line | T::LineMarkers => ("line", Some("clustered")),
        T::StackedLine => ("line", Some("stacked")),
        T::Pie => ("pie", None),
        _ => ("other", None),
    };
    let img = deckcraft_render::render_shape_box(cx.pres, cx.index, cx.layer, s, RASTER_SCALE)?;
    let media = out.png(cx.media_key(s.id, "-chart"), img.to_png())?;
    let series = c
        .series
        .iter()
        .enumerate()
        .map(|(i, se)| FlatSeries {
            name: se.name.clone(),
            values: se.values.iter().map(|v| v.filter(|v| v.is_finite())).collect(),
            color: hex(se
                .fill
                .as_ref()
                .and_then(|f| solid_color(cx.ctx, f, None))
                .unwrap_or_else(|| deckcraft_render::chart_series_color(cx.ctx, c, i))),
        })
        .collect();
    Some(common(
        cx,
        s,
        x,
        FlatBody::Chart {
            chart_type: chart_type.into(),
            grouping: grouping.map(String::from),
            title: c.title.clone(),
            categories: c.categories.clone(),
            series,
            media,
        },
    ))
}

/// Bytes of a media item a flat deck references: a PNG made by the last flatten, or an item of
/// the active presentation (by its numeric id).
pub fn media_bytes(s: &crate::Session, id: &str) -> Option<FlatBlob> {
    if let Some(b) = s.flat_media.get(id) {
        return Some(b.clone());
    }
    let mid = id.parse::<u32>().ok()?;
    let m = s.active()?.doc.media(MediaId(mid))?;
    Some(FlatBlob { content_type: m.content_type.clone(), name: m.name.clone(), data: m.data.clone() })
}

/// Guess a content type from the first bytes.
pub fn sniff_content_type(data: &[u8]) -> &'static str {
    let starts = |p: &[u8]| data.starts_with(p);
    let at = |i: usize, p: &[u8]| data.get(i..i + p.len()) == Some(p);
    if starts(b"\x89PNG") {
        "image/png"
    } else if starts(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if starts(b"GIF8") {
        "image/gif"
    } else if starts(b"RIFF") && at(8, b"WEBP") {
        "image/webp"
    } else if starts(b"RIFF") && at(8, b"WAVE") {
        "audio/wav"
    } else if starts(b"BM") {
        "image/bmp"
    } else if starts(b"<svg") || starts(b"<?xml") {
        "image/svg+xml"
    } else if at(4, b"ftyp") {
        "video/mp4"
    } else if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        "video/webm"
    } else if starts(b"OggS") {
        "audio/ogg"
    } else if starts(b"ID3") || data.first() == Some(&0xFF) && data.get(1).is_some_and(|b| b & 0xE0 == 0xE0) {
        "audio/mpeg"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests;
