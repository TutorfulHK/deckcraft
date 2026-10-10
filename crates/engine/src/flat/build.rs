//! Flat deck → presentation (`file.fromFlat`): one slide per flat slide on the blank layout,
//! every element an absolutely placed shape with all its formatting explicit.

use std::collections::HashMap;
use std::sync::Arc;

use deckcraft_color::{ColorTransform, Rgba};
use deckcraft_geom::{Size, Xfrm};
use deckcraft_model::anim::{ANIMATIONS, TRANSITIONS, animation_info};
use deckcraft_model::style::{ColorRef, Dash, Effects, Fill, Line, LineEnd, PictureFill};
use deckcraft_model::table::{Cell, Row, Table};
use deckcraft_model::text::{
    Action, Align, Anchor, AutoFit, BodyProps, Bullet, Caps, Hyperlink, ParaProps, Run, RunKind, RunProps, Spacing, Strike, TextBody, TextDir,
};
use deckcraft_model::{
    AnimClass, AnimStart, Animation, Background, Chart, Geom, LayoutType, MediaClip, MediaId, PhType, Placeholder, Presentation, Shape, ShapeId,
    ShapeKind, Slide, SlideId, Transition, defaults,
};

use super::schema::*;
use super::{FlatBlob, sniff_content_type};
use crate::{EngineError, Result};

const MAX_SLIDES: usize = 5_000;
const MAX_ELEMENTS: usize = 5_000;
const MAX_PARAS: usize = 5_000;
const MAX_RUNS: usize = 5_000;
const MAX_GRID: usize = 1_000;
const MAX_COORD: f64 = 1.0e6;

fn err(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: "file.fromFlat".into(), msg: msg.into() }
}

fn num(v: f64, lo: f64, hi: f64, default: f64) -> f64 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

fn coord(v: f64) -> f64 {
    num(v, -MAX_COORD, MAX_COORD, 0.0)
}

fn color(hex: &str, opacity: Option<f64>) -> ColorRef {
    let c = Rgba::from_hex(hex).unwrap_or(Rgba::BLACK);
    let mut r = ColorRef::rgb(Rgba { a: 255, ..c });
    if let Some(o) = opacity.filter(|o| o.is_finite() && *o < 0.999) {
        r.mods.push(ColorTransform::Alpha((o.clamp(0.0, 1.0) * 100_000.0).round() as i32));
    }
    r
}

/// Media referenced by the deck, added to the presentation once each.
struct Media<'a> {
    deck: &'a FlatDeck,
    lookup: &'a dyn Fn(&str) -> Option<FlatBlob>,
    added: HashMap<String, MediaId>,
}

impl Media<'_> {
    fn get(&mut self, p: &mut Presentation, id: &str) -> Result<MediaId> {
        if let Some(m) = self.added.get(id) {
            return Ok(*m);
        }
        let blob = (self.lookup)(id).ok_or_else(|| err(format!("media `{id}` was not supplied")))?;
        let entry = self.deck.media.iter().find(|m| m.id == id);
        let ct = entry
            .map(|e| e.content_type.clone())
            .filter(|c| !c.is_empty())
            .or_else(|| Some(blob.content_type.clone()).filter(|c| !c.is_empty() && c != "application/octet-stream"))
            .unwrap_or_else(|| sniff_content_type(&blob.data).to_string());
        let ext = ct.rsplit('/').next().unwrap_or("bin").split('+').next().unwrap_or("bin").replace("jpeg", "jpg");
        let name = entry.map(|e| e.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("media.{ext}"));
        let mid = p.add_media(&name, &ct, Arc::unwrap_or_clone(blob.data));
        self.added.insert(id.to_string(), mid);
        Ok(mid)
    }
}

/// Build a presentation from a flat deck. `lookup` finds the bytes of a media id.
pub fn unflatten(deck: &FlatDeck, lookup: &dyn Fn(&str) -> Option<FlatBlob>) -> Result<Presentation> {
    if deck.version != FLAT_VERSION {
        return Err(err(format!("unsupported flat deck version {} (expected {FLAT_VERSION})", deck.version)));
    }
    if deck.slides.len() > MAX_SLIDES {
        return Err(err(format!("too many slides ({})", deck.slides.len())));
    }
    let size = Size::new(num(deck.size.w, 1.0, 4999.0, 960.0), num(deck.size.h, 1.0, 4999.0, 540.0));
    let mut p = defaults::blank_presentation(size, Default::default(), false);
    let layout = defaults::layout_of_kind(&p, LayoutType::Blank).or_else(|| p.masters.first().and_then(|m| m.layouts.first()).map(|l| l.id));
    let layout = layout.ok_or_else(|| err("no layout"))?;
    let mut media = Media { deck, lookup, added: HashMap::new() };
    for fs in &deck.slides {
        if fs.elements.len() > MAX_ELEMENTS {
            return Err(err(format!("too many elements on slide `{}`", fs.id)));
        }
        let id = SlideId(p.alloc_id());
        let mut slide = Slide { id, layout, show_master_shapes: false, hidden: fs.hidden, ..Default::default() };
        if !fs.notes.is_empty() {
            slide.notes = TextBody::from_text(&fs.notes);
        }
        slide.background = match &fs.background {
            Some(f) => Some(Background::Fill { fill: fill(&mut p, &mut media, f)? }),
            None => None,
        };
        slide.transition = fs.transition.as_ref().filter(|t| t.kind != "none").map(|t| Transition {
            kind: if TRANSITIONS.iter().any(|x| x.0 == t.kind) { t.kind.clone() } else { "fade".into() },
            option: t.option.chars().take(64).collect(),
            duration_ms: num(t.duration_ms, 0.0, 60_000.0, 1000.0) as u32,
            ..Default::default()
        });
        let mut anims: Vec<(u32, usize, Animation)> = vec![];
        let mut body_idx = 0u32;
        for el in &fs.elements {
            let sid = ShapeId(p.alloc_id());
            let shape = element(&mut p, &mut media, el, sid, &mut body_idx)?;
            for a in el.animations.iter().take(64) {
                anims.push((a.order, anims.len(), animation(a, sid)));
            }
            slide.shapes.push(shape);
        }
        anims.sort_by_key(|(order, i, _)| (*order, *i));
        slide.animations = anims.into_iter().map(|(_, _, a)| a).collect();
        p.slides.push(Arc::new(slide));
    }
    p.fix_next_id();
    Ok(p)
}

fn animation(a: &FlatAnimation, shape: ShapeId) -> Animation {
    let class = match a.kind.as_str() {
        "emphasis" => AnimClass::Emphasis,
        "exit" => AnimClass::Exit,
        "path" => AnimClass::Path,
        "media" => AnimClass::Media,
        _ => AnimClass::Entrance,
    };
    let effect = match animation_info(&a.effect, class) {
        Some(i) if i.2 == class => i.0.to_string(),
        _ => ANIMATIONS.iter().find(|i| i.2 == class).map(|i| i.0.to_string()).unwrap_or_else(|| "fade".into()),
    };
    Animation {
        shape,
        class,
        effect,
        option: a.option.chars().take(64).collect(),
        start: match a.trigger.as_str() {
            "withPrevious" => AnimStart::WithPrevious,
            "afterPrevious" => AnimStart::AfterPrevious,
            _ => AnimStart::OnClick,
        },
        duration_ms: num(a.duration_ms, 0.0, 600_000.0, 500.0) as u32,
        delay_ms: num(a.delay_ms, 0.0, 600_000.0, 0.0) as u32,
        ..Default::default()
    }
}

fn fill(p: &mut Presentation, media: &mut Media, f: &FlatFill) -> Result<Fill> {
    Ok(match f {
        FlatFill::Solid { color: c, opacity } => Fill::solid(color(c, *opacity)),
        FlatFill::Image { media: m } => Fill::Picture(PictureFill { media: media.get(p, m)?, ..Default::default() }),
    })
}

fn line(l: &FlatLine) -> Line {
    let end = |k: &Option<String>| {
        k.as_ref().filter(|k| matches!(k.as_str(), "triangle" | "stealth" | "diamond" | "oval" | "arrow")).map(|k| LineEnd {
            kind: k.clone(),
            w: "med".into(),
            len: "med".into(),
        })
    };
    let dash = if Dash::PRESETS.contains(&l.dash.as_str()) { l.dash.clone() } else { "solid".into() };
    Line {
        fill: Some(Fill::solid(color(&l.color, l.opacity))),
        width: Some(num(l.width, 0.0, 1584.0, 0.75)),
        dash: Some(Dash::Preset { name: dash }),
        head: end(&l.head_end),
        tail: end(&l.tail_end),
        ..Default::default()
    }
}

fn opt_line(l: &Option<FlatLine>) -> Line {
    l.as_ref().map(line).unwrap_or_else(Line::none)
}

fn xfrm(el: &FlatElement) -> Xfrm {
    Xfrm {
        x: coord(el.x),
        y: coord(el.y),
        w: num(el.w.abs(), 0.0, MAX_COORD, 0.0),
        h: num(el.h.abs(), 0.0, MAX_COORD, 0.0),
        rot: if el.rotation.is_finite() { el.rotation.rem_euclid(360.0) } else { 0.0 },
        flip_h: el.flip_h,
        flip_v: el.flip_v,
    }
}

fn element(p: &mut Presentation, media: &mut Media, el: &FlatElement, id: ShapeId, body_idx: &mut u32) -> Result<Shape> {
    let x = xfrm(el);
    let mut s =
        Shape { id, name: el.name.chars().take(256).collect(), xfrm: Some(x), descr: el.alt.chars().take(4096).collect(), ..Default::default() };
    match &el.body {
        FlatBody::Shape { preset, adj, fill: f, line: l, text } => {
            let preset = if deckcraft_geom::preset::build(preset, 10.0, 10.0, &[]).is_some() { preset.clone() } else { "rect".to_string() };
            let adj: Vec<f64> = adj.iter().copied().filter(|v| v.is_finite()).take(16).collect();
            s.fill = Some(match f {
                Some(f) => fill(p, media, f)?,
                None => Fill::None,
            });
            s.line = Some(opt_line(l));
            if let Some(t) = text {
                let tr = super::text_rect(&preset, &adj, x.w, x.h);
                s.text = Some(text_body(t, Some((tr, x.w, x.h))));
                let kind = el
                    .placeholder
                    .as_deref()
                    .map(PhType::from_xml)
                    .filter(|_| matches!(el.placeholder.as_deref(), Some("title" | "ctrTitle" | "subTitle" | "body" | "obj")));
                if let Some(kind) = kind {
                    let idx = if kind.is_title() {
                        0
                    } else {
                        *body_idx += 1;
                        *body_idx
                    };
                    s.ph = Some(Placeholder { kind, idx, ..Default::default() });
                    s.effects = Some(Effects::default());
                } else {
                    s.text_box = preset == "rect" && f.is_none() && l.is_none();
                }
            }
            s.geom = Geom::Preset { name: preset, adj };
        }
        FlatBody::Picture { media: m, crop, opacity, line: l } => {
            let crop = crop.map(|c| [c.l, c.t, c.r, c.b].map(|v| num(v, -10.0, 0.999, 0.0))).unwrap_or([0.0; 4]);
            let alpha = opacity.filter(|o| o.is_finite()).map(|o| o.clamp(0.0, 1.0));
            s.kind = ShapeKind::Picture { fill: PictureFill { media: media.get(p, m)?, crop, alpha, ..Default::default() } };
            s.line = Some(opt_line(l));
        }
        FlatBody::Raster { media: m } => {
            s.kind = ShapeKind::Picture { fill: PictureFill { media: media.get(p, m)?, ..Default::default() } };
            s.line = Some(Line::none());
        }
        FlatBody::Table { col_widths, rows } => {
            s.kind = ShapeKind::Table(table(p, media, col_widths, rows)?);
        }
        FlatBody::Chart { chart_type, grouping, title, categories, series, media: m } => {
            use deckcraft_model::chart::{ChartType as T, Series};
            let g = grouping.as_deref().unwrap_or("clustered");
            let kind = match (chart_type.as_str(), g) {
                ("column", "stacked") => Some(T::StackedColumn),
                ("column", "percent") => Some(T::PercentColumn),
                ("column", _) => Some(T::Column),
                ("bar", "stacked") => Some(T::StackedBar),
                ("bar", "percent") => Some(T::PercentBar),
                ("bar", _) => Some(T::Bar),
                ("line", "stacked") => Some(T::StackedLine),
                ("line", _) => Some(T::Line),
                ("pie", _) => Some(T::Pie),
                _ => None,
            };
            match kind {
                Some(kind) => {
                    let pie = kind == T::Pie;
                    let chart = Chart {
                        kind,
                        title: title.as_ref().map(|t| t.chars().take(1024).collect()),
                        categories: categories.iter().take(MAX_GRID).map(|c| c.chars().take(256).collect()).collect(),
                        series: series
                            .iter()
                            .take(256)
                            .map(|se| Series {
                                name: se.name.chars().take(256).collect(),
                                values: se.values.iter().take(MAX_GRID).map(|v| v.filter(|v| v.is_finite())).collect(),
                                fill: (!pie).then(|| Fill::solid(color(&se.color, None))),
                                ..Default::default()
                            })
                            .collect(),
                        vary_colors: pie,
                        ..Default::default()
                    };
                    s.kind = ShapeKind::Chart(Box::new(chart));
                }
                // A chart we can't rebuild keeps its picture.
                None => s.kind = ShapeKind::Picture { fill: PictureFill { media: media.get(p, m)?, ..Default::default() } },
            }
        }
        FlatBody::Media { media_kind, media: m, poster } => {
            let poster = match poster {
                Some(pm) => Some(media.get(p, pm)?),
                None => None,
            };
            s.kind = ShapeKind::Media(MediaClip { media: media.get(p, m)?, video: media_kind != "audio", poster, volume: 1.0, ..Default::default() });
        }
    }
    Ok(s)
}

fn table(p: &mut Presentation, media: &mut Media, cols: &[f64], rows: &[FlatRow]) -> Result<Table> {
    if cols.len() > MAX_GRID || rows.len() > MAX_GRID {
        return Err(err("table too large"));
    }
    let cols: Vec<f64> = cols.iter().map(|w| num(*w, 0.0, MAX_COORD, 72.0)).collect();
    let n = cols.len().max(1);
    let cols = if cols.is_empty() { vec![72.0] } else { cols };
    let mut out_rows = Vec::with_capacity(rows.len());
    for r in rows {
        let mut cells = Vec::with_capacity(n);
        for i in 0..n {
            let Some(c) = r.cells.get(i) else {
                cells.push(Cell::new(""));
                continue;
            };
            let b = c.borders.clone().unwrap_or_default();
            let ins = c.text.insets;
            let pad = |v: f64| num(v, 0.0, 1000.0, 0.0);
            cells.push(Cell {
                text: text_body(&c.text, None),
                fill: Some(match &c.fill {
                    Some(f) => fill(p, media, f)?,
                    None => Fill::None,
                }),
                borders: [Some(opt_line(&b.l)), Some(opt_line(&b.r)), Some(opt_line(&b.t)), Some(opt_line(&b.b))],
                margins: Some([pad(ins.l), pad(ins.r), pad(ins.t), pad(ins.b)]),
                anchor: Some(anchor(&c.text.v_align)),
                grid_span: c.grid_span.clamp(1, n as u32),
                row_span: c.row_span.clamp(1, rows.len().max(1) as u32),
                h_merge: c.h_merge,
                v_merge: c.v_merge,
                ..Default::default()
            });
        }
        out_rows.push(Row { height: num(r.height, 0.0, MAX_COORD, 20.0), cells });
    }
    Ok(Table {
        cols,
        rows: out_rows,
        style: "none".into(),
        first_row: false,
        first_col: false,
        last_row: false,
        last_col: false,
        band_row: false,
        band_col: false,
    })
}

fn anchor(v: &str) -> Anchor {
    match v {
        "middle" => Anchor::Middle,
        "bottom" => Anchor::Bottom,
        _ => Anchor::Top,
    }
}

/// A text body with every property explicit. `rect`: the preset's text rectangle and the box
/// size, to turn box-relative insets back into body insets (`None` for table cells).
fn text_body(t: &FlatText, rect: Option<(deckcraft_geom::Rect, f64, f64)>) -> TextBody {
    let i = t.insets;
    let (l, tp, r, b) = match rect {
        Some((tr, w, h)) => (i.l - tr.x0, i.t - tr.y0, i.r - (w - tr.x1), i.b - (h - tr.y1)),
        None => (i.l, i.t, i.r, i.b),
    };
    let ins = |v: f64| num(v, -MAX_COORD, MAX_COORD, 0.0);
    let body = BodyProps {
        inset_l: Some(ins(l)),
        inset_t: Some(ins(tp)),
        inset_r: Some(ins(r)),
        inset_b: Some(ins(b)),
        anchor: Some(anchor(&t.v_align)),
        anchor_ctr: Some(false),
        wrap: Some(t.wrap),
        autofit: Some(match t.autofit.as_str() {
            "shrink" => {
                let fs = t.font_scale.filter(|v| v.is_finite()).unwrap_or(1.0).clamp(0.1, 1.0);
                AutoFit::Shrink { font_scale: fs, line_reduction: ((1.0 - fs) * 0.8).min(0.2) }
            }
            "resize" => AutoFit::Shape,
            _ => AutoFit::None,
        }),
        vert: Some(TextDir::Horizontal),
        rot: Some(0.0),
        columns: Some(t.columns.unwrap_or(1).clamp(1, 16)),
        col_spacing: Some(0.0),
        warp: None,
        rtl_col: Some(false),
        upright: Some(false),
    };
    TextBody { body, list_style: Default::default(), paragraphs: t.paragraphs.iter().take(MAX_PARAS).map(paragraph).collect() }
}

fn paragraph(fp: &FlatParagraph) -> deckcraft_model::Paragraph {
    let pt = |v: f64| num(v, -MAX_COORD, MAX_COORD, 0.0);
    let (bullet, bullet_font, bullet_color, bullet_size) = match &fp.bullet {
        None => (Bullet::None, None, None, None),
        Some(FlatBullet::Char { char, color: c, font, size }) => (
            Bullet::Char { char: char.chars().take(4).collect::<String>().chars().next().map(String::from).unwrap_or_else(|| "•".into()) },
            font.clone(),
            c.as_ref().map(|c| color(c, None)),
            *size,
        ),
        Some(FlatBullet::Number { scheme, start_at, color: c, size }) => (
            Bullet::AutoNum { scheme: scheme.chars().take(64).collect(), start_at: (*start_at).clamp(1, 32_767) },
            None,
            c.as_ref().map(|c| color(c, None)),
            *size,
        ),
    };
    let props = ParaProps {
        align: Some(match fp.align.as_str() {
            "center" => Align::Center,
            "right" => Align::Right,
            "justify" => Align::Justify,
            _ => Align::Left,
        }),
        margin_left: Some(pt(fp.margin_left)),
        margin_right: Some(0.0),
        indent: Some(pt(fp.indent)),
        line_spacing: Some(match (fp.line_spacing, fp.line_spacing_pt) {
            (Some(m), _) => Spacing::Pct(num(m, 0.0, 100.0, 1.0)),
            (None, Some(v)) => Spacing::Pts(num(v, 0.0, 10_000.0, 12.0)),
            (None, None) => Spacing::Pct(1.0),
        }),
        space_before: Some(Spacing::Pts(num(fp.space_before, 0.0, 10_000.0, 0.0))),
        space_after: Some(Spacing::Pts(num(fp.space_after, 0.0, 10_000.0, 0.0))),
        bullet: Some(bullet),
        bullet_font,
        bullet_color,
        bullet_size: bullet_size.filter(|v| v.is_finite()).map(|v| v.clamp(0.25, 4.0)),
        rtl: Some(fp.rtl),
        ..Default::default()
    };
    let mut para = deckcraft_model::Paragraph { level: fp.level.min(8), props, runs: vec![], end_props: RunProps::default() };
    for fr in fp.runs.iter().take(MAX_RUNS) {
        let rp = run_props(fr);
        if fr.text.is_empty() {
            para.end_props = rp;
            continue;
        }
        if let Some(f) = &fr.field {
            para.runs.push(Run { text: fr.text.clone(), props: rp, kind: RunKind::Field { field: f.chars().take(64).collect() } });
            continue;
        }
        for (k, part) in fr.text.split(['\n', '\u{b}']).enumerate() {
            if k > 0 {
                para.runs.push(Run { text: String::new(), props: rp.clone(), kind: RunKind::Break });
            }
            if !part.is_empty() {
                para.runs.push(Run::with(part, rp.clone()));
            }
        }
    }
    if para.end_props == RunProps::default()
        && let Some(r) = para.runs.last()
    {
        para.end_props = r.props.clone();
    }
    para
}

fn run_props(r: &FlatRun) -> RunProps {
    let link = r.link.as_ref().map(|l| {
        let action = if let Some(id) = l.strip_prefix("#slide-").and_then(|v| v.parse::<u32>().ok()) {
            Action::Slide { slide: SlideId(id) }
        } else {
            match l.as_str() {
                "#next" => Action::NextSlide,
                "#previous" => Action::PreviousSlide,
                "#first" => Action::FirstSlide,
                "#last" => Action::LastSlide,
                _ => Action::Url { url: l.clone() },
            }
        };
        Hyperlink { action, tooltip: String::new(), highlight_click: false }
    });
    RunProps {
        font: Some(if r.font.is_empty() { "+mn-lt".into() } else { r.font.chars().take(128).collect() }),
        font_ea: (!r.font_ea.is_empty()).then(|| r.font_ea.chars().take(128).collect()),
        size: Some(if r.size > 0.0 { num(r.size, 1.0, 4000.0, 18.0) } else { 18.0 }),
        bold: Some(r.bold),
        italic: Some(r.italic),
        underline: Some(if r.underline { "sng" } else { "none" }.into()),
        strike: Some(if r.strike { Strike::Single } else { Strike::None }),
        baseline: Some(num(r.baseline, -1.0, 1.0, 0.0)),
        caps: Some(match r.caps.as_deref() {
            Some("all") => Caps::All,
            Some("small") => Caps::Small,
            _ => Caps::None,
        }),
        spacing: Some(num(r.spacing, -1000.0, 1000.0, 0.0)),
        fill: Some(Fill::solid(color(&r.color, r.opacity))),
        highlight: r.highlight.as_ref().map(|h| color(h, None)),
        link,
        ..Default::default()
    }
}
