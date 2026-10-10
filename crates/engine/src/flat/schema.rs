//! The flat deck schema (version 1): a self-contained, absolutely positioned slide model for web
//! hosts. Documented in `docs/flat-deck.md`. Units are points, colours `#rrggbb`, opacity 0..1,
//! rotation degrees clockwise.

use serde::{Deserialize, Serialize};

pub const FLAT_VERSION: u32 = 1;

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatDeck {
    pub version: u32,
    pub size: FlatSize,
    #[serde(default)]
    pub slides: Vec<FlatSlide>,
    #[serde(default)]
    pub media: Vec<FlatMedia>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FlatSize {
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatMedia {
    pub id: String,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatSlide {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub background: Option<FlatFill>,
    #[serde(default)]
    pub transition: Option<FlatTransition>,
    /// Array order is z-order (back to front).
    #[serde(default)]
    pub elements: Vec<FlatElement>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatTransition {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub duration_ms: f64,
    /// Effect option (direction…), as in the engine's `transition.set`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub option: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum FlatFill {
    Solid {
        color: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opacity: Option<f64>,
    },
    Image {
        media: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatLine {
    #[serde(default)]
    pub color: String,
    #[serde(default = "hairline")]
    pub width: f64,
    /// OOXML preset dash name: `solid`, `dash`, `dot`, `lgDash`, `dashDot`, `sysDash`, `sysDot`…
    #[serde(default = "solid")]
    pub dash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Arrowhead at the start / end: `triangle`, `stealth`, `diamond`, `oval`, `arrow`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_end: Option<String>,
}

fn hairline() -> f64 {
    0.75
}

fn solid() -> String {
    "solid".into()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatElement {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub w: f64,
    #[serde(default)]
    pub h: f64,
    #[serde(default)]
    pub rotation: f64,
    #[serde(default)]
    pub flip_h: bool,
    #[serde(default)]
    pub flip_v: bool,
    #[serde(default)]
    pub alt: String,
    /// A layout/master graphic shown behind the slide's own shapes.
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_master: bool,
    /// Placeholder type the element came from (`title`, `ctrTitle`, `subTitle`, `body`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animations: Vec<FlatAnimation>,
    #[serde(flatten)]
    pub body: FlatBody,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum FlatBody {
    #[serde(rename_all = "camelCase")]
    Shape {
        preset: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        adj: Vec<f64>,
        #[serde(default)]
        fill: Option<FlatFill>,
        #[serde(default)]
        line: Option<FlatLine>,
        #[serde(default)]
        text: Option<FlatText>,
    },
    #[serde(rename_all = "camelCase")]
    Picture {
        media: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crop: Option<FlatCrop>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opacity: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<FlatLine>,
    },
    #[serde(rename_all = "camelCase")]
    Table { col_widths: Vec<f64>, rows: Vec<FlatRow> },
    #[serde(rename_all = "camelCase")]
    Chart {
        /// `bar`, `column`, `line`, `pie` or `other`.
        chart_type: String,
        /// `clustered`, `stacked` or `percent` (bar, column, line).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grouping: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default)]
        categories: Vec<String>,
        #[serde(default)]
        series: Vec<FlatSeries>,
        /// The chart rendered as PNG (always present; the only content for `other`).
        media: String,
    },
    #[serde(rename_all = "camelCase")]
    Media {
        /// `video` or `audio`.
        media_kind: String,
        media: String,
        #[serde(default)]
        poster: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Raster { media: String },
}

impl Default for FlatBody {
    fn default() -> Self {
        FlatBody::Shape { preset: "rect".into(), adj: vec![], fill: None, line: None, text: None }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FlatCrop {
    pub l: f64,
    pub t: f64,
    pub r: f64,
    pub b: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FlatInsets {
    pub l: f64,
    pub t: f64,
    pub r: f64,
    pub b: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FlatText {
    /// `top`, `middle` or `bottom`.
    pub v_align: String,
    /// Measured from the element's box (a preset's own text rectangle is folded in).
    pub insets: FlatInsets,
    /// `none`, `shrink` or `resize`.
    pub autofit: String,
    pub wrap: bool,
    /// Shrink-on-overflow scale the text is drawn at (only when below 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<u32>,
    pub paragraphs: Vec<FlatParagraph>,
}

impl Default for FlatText {
    fn default() -> Self {
        FlatText {
            v_align: "top".into(),
            insets: FlatInsets { l: 7.2, t: 3.6, r: 7.2, b: 3.6 },
            autofit: "none".into(),
            wrap: true,
            font_scale: None,
            columns: None,
            paragraphs: vec![],
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatParagraph {
    /// `left`, `center`, `right` or `justify`.
    #[serde(default)]
    pub align: String,
    #[serde(default)]
    pub level: u8,
    #[serde(default)]
    pub indent: f64,
    #[serde(default)]
    pub margin_left: f64,
    #[serde(default)]
    pub space_before: f64,
    #[serde(default)]
    pub space_after: f64,
    /// Line spacing as a multiple of single spacing; `null` when given in points (`lineSpacingPt`).
    #[serde(default)]
    pub line_spacing: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_spacing_pt: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub rtl: bool,
    #[serde(default)]
    pub bullet: Option<FlatBullet>,
    #[serde(default)]
    pub runs: Vec<FlatRun>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum FlatBullet {
    #[serde(rename_all = "camelCase")]
    Char {
        char: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        font: Option<String>,
        /// Size as a fraction of the text size.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Number {
        scheme: String,
        #[serde(default = "one")]
        start_at: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f64>,
    },
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatRun {
    /// `\n` is a line break inside the paragraph. An empty run only carries the formatting of an
    /// empty paragraph.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub font: String,
    #[serde(default)]
    pub font_ea: String,
    #[serde(default)]
    pub size: f64,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strike: bool,
    #[serde(default)]
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default)]
    pub highlight: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    /// Baseline shift as a fraction of the size (0.3 = superscript).
    #[serde(default)]
    pub baseline: f64,
    /// `all` or `small` capitals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caps: Option<String>,
    /// Character spacing in points.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub spacing: f64,
    /// A field (`slidenum`, `datetime1`…); `text` holds its current value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatRow {
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    pub cells: Vec<FlatCell>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatCell {
    #[serde(default)]
    pub text: FlatText,
    #[serde(default)]
    pub fill: Option<FlatFill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub borders: Option<FlatBorders>,
    #[serde(default = "one")]
    pub grid_span: u32,
    #[serde(default = "one")]
    pub row_span: u32,
    #[serde(default)]
    pub h_merge: bool,
    #[serde(default)]
    pub v_merge: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FlatBorders {
    #[serde(default)]
    pub l: Option<FlatLine>,
    #[serde(default)]
    pub t: Option<FlatLine>,
    #[serde(default)]
    pub r: Option<FlatLine>,
    #[serde(default)]
    pub b: Option<FlatLine>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatSeries {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub values: Vec<Option<f64>>,
    #[serde(default)]
    pub color: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatAnimation {
    /// `entrance`, `emphasis`, `exit` (also `path`, `media`).
    #[serde(rename = "type")]
    pub kind: String,
    pub effect: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub option: String,
    /// `onClick`, `withPrevious` or `afterPrevious`.
    pub trigger: String,
    #[serde(default)]
    pub duration_ms: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delay_ms: f64,
    /// Position in the slide's animation sequence (sorts animations across elements).
    #[serde(default)]
    pub order: u32,
}
