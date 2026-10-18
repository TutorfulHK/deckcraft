# The flat deck (version 1)

The flat deck is a presentation with every inherited value resolved — placeholders, layouts,
masters, theme colours and fonts, table styles — into absolutely positioned, self-contained
elements. It is for hosts that keep their own simple slide model, such as a web app driving the
[`deckcraft-wasm`](../packaging/wasm/README.md) library from a Web Worker.

| Command | Does |
|---|---|
| `file.flatten {}` | → the flat deck of the active presentation |
| `file.fromFlat {deck, media?: {id: base64}}` | replaces the active presentation (one undo step) with one built from a flat deck, or opens it when none is open |
| `media.bytes {id}` | → `{data: base64, contentType, name, bytes}` of a media id the flat deck references |

Then `file.saveBytes {format: "pptx" \| "pdf"}` and `file.render {slide, scale}` work as usual.

The Rust types are in `crates/engine/src/flat/schema.rs`.

## Conventions

- Units are **points** (1/72 in), as `f64`. Colours are `"#rrggbb"`; opacity is `0..1` and is
  omitted when 1. Rotation is in degrees clockwise about the element's centre; flips apply before
  rotation.
- Array order is z-order, back to front. Master and layout graphics come first, with
  `"fromMaster": true`.
- Values are resolved: no `null` means "inherit". Where a field may be `null` it says what `null`
  means.
- Hidden shapes are left out (they don't show in the slide show).

## Deck

```
FlatDeck  { version: 1, size: {w, h}, slides: [FlatSlide], media: [{id, contentType, name}] }
FlatSlide { id, hidden, notes (plain text), background: Fill | null (null = transparent),
            transition: {type, durationMs, option?} | null, elements: [Element] }
Fill      = {type: "solid", color, opacity?} | {type: "image", media}
Line      = {color, width, dash, opacity?, headEnd?, tailEnd?}
```

`dash` is an OOXML preset name (`solid`, `dot`, `dash`, `lgDash`, `dashDot`, `lgDashDot`,
`lgDashDotDot`, `sysDash`, `sysDot`, `sysDashDot`, `sysDashDotDot`). `headEnd` / `tailEnd` name an
arrowhead (`triangle`, `stealth`, `diamond`, `oval`, `arrow`). Transition types are the engine's
(`fade`, `push`, `wipe`, `morph`…; see `transition.set`).

A background that isn't a plain colour or picture (gradient, pattern) is rendered to a PNG and sent
as an `image` fill.

## Elements

Every element has

```
{ id, name, x, y, w, h, rotation, flipH, flipV, alt, fromMaster?, placeholder?, animations?, kind, … }
```

`placeholder` is the placeholder type the element came from (`title`, `ctrTitle`, `subTitle`,
`body`, `obj`, `dt`, `ftr`, `sldNum`…). `animations` (when any):

```
{ type: "entrance" | "emphasis" | "exit" | "path" | "media", effect, option?, trigger: "onClick" | "withPrevious" | "afterPrevious",
  durationMs, delayMs?, order }
```

`order` is the animation's place in the slide's sequence; sort by it across elements. An
animation on a group is repeated on each of its elements, the later ones `withPrevious`.

| `kind` | Fields |
|---|---|
| `shape` | `preset`, `adj?` (OOXML adjust values), `fill: Fill \| null`, `line: Line \| null`, `text: Text \| null` |
| `picture` | `media`, `crop?: {l, t, r, b}` (fractions of the image), `opacity?`, `line?` |
| `table` | `colWidths: [pt]`, `rows: [{height, cells: [Cell]}]` |
| `chart` | `chartType: bar \| column \| line \| pie \| other`, `grouping?: clustered \| stacked \| percent`, `title?`, `categories: [string]`, `series: [{name, values: [number \| null], color}]`, `media` (the chart as PNG) |
| `media` | `mediaKind: video \| audio`, `media`, `poster: media \| null` |
| `raster` | `media` (a PNG of what DeckCraft draws) |

### What becomes a raster

A shape is a vector `shape` only when the web renderer can draw it exactly: its geometry is one of
the presets in `VECTOR_PRESETS` (`crates/engine/src/flat/mod.rs`: `rect`, `roundRect`, `ellipse`,
`triangle`, `rtTriangle`, `diamond`, `parallelogram`, `trapezoid`, `pentagon`, `hexagon`,
`octagon`, `star5`, `rightArrow`, `leftArrow`, `upArrow`, `downArrow`, `chevron`, `homePlate`,
`line`, `straightConnector1`, `plus`), its fill and outline are none or one solid colour, and it
has no visible effects (shadow, glow, reflection, soft edges).

Anything else — another preset, custom geometry (freeforms), a gradient, pattern or picture fill, a
gradient outline, effects, ink, OLE objects and SmartArt kept as opaque XML, rotated or flipped
groups, pictures that are cropped beyond their bounds, tiled, recoloured or in formats a browser
can't show (EMF, WMF, TIFF) — is rendered to PNG at 2 px per point and sent as a `raster` element
whose box is the drawing's axis-aligned bounds (outlines and effects included, rotation 0).

When a rasterised shape has text, the PNG is drawn **without** its text and the text follows as a
separate transparent `shape` (preset `rect`, `fill` and `line` null, id `<id>-text`) with the
original box, so it stays editable. Text the flat model can't lay out (WordArt warps, vertical or
rotated text) stays in the PNG instead.

Groups without rotation or flips are flattened: their children come out as elements at slide
coordinates.

### Text

```
Text      { vAlign: top | middle | bottom, insets: {l, t, r, b}, autofit: none | shrink | resize, wrap,
            fontScale?, columns?, paragraphs: [Paragraph] }
Paragraph { align: left | center | right | justify, level, indent, marginLeft, spaceBefore, spaceAfter,
            lineSpacing: number | null, lineSpacingPt?, rtl?, bullet: Bullet | null, runs: [Run] }
Bullet    = {type: "char", char, color?, font?, size?} | {type: "number", scheme, startAt, color?, size?}
Run       { text, font, fontEa, size, bold, italic, underline, strike, color, opacity?, highlight: color | null,
            link: string | null, baseline, caps?, spacing?, field? }
```

- `insets` are measured from the element's box: a preset's own text rectangle (an ellipse's inner
  box, say) is folded in.
- `fontScale` is the shrink-on-overflow scale DeckCraft draws the text at (only when below 1); run
  sizes are the unscaled sizes.
- `lineSpacing` is a multiple of single spacing; it is `null` when the spacing is exact, given in
  points as `lineSpacingPt`. `spaceBefore` / `spaceAfter` are points.
- `\n` in a run's text is a line break inside the paragraph. An empty paragraph has one run with
  empty text that carries its formatting.
- Bullet `color` / `font` are absent when they follow the text. Symbol-font bullets (Wingdings) are
  mapped to Unicode characters. `scheme` is an OOXML auto-number scheme (`arabicPeriod`,
  `alphaLcParenR`, `romanUcPeriod`…). Bullet `size` is a fraction of the text size.
- `baseline` is a fraction of the size (0.3 = superscript). `caps` is `all` or `small`. `field`
  names a field (`slidenum`, `datetime1`…); `text` is its current value.
- `link` is a URL, or `#slide-<slide id>`, `#next`, `#previous`, `#first`, `#last`.

### Tables

```
Cell { text: Text, fill: Fill | null, borders?: {l, t, r, b: Line | null}, gridSpan, rowSpan, hMerge, vMerge }
```

Fills, borders, text colours and bold come resolved from the table style. Row `height` is the
height the row is drawn at (rows grow to fit their text). Merged-over cells are present with
`hMerge` / `vMerge` set. A cell's `insets` are its margins.

## Media

`media` lists every id the deck references. Ids of the presentation's own items are their numeric
ids; PNGs made while flattening have ids starting with `x`. Fetch bytes with `media.bytes {id}` (or
`Engine.mediaBytes(id)` in the wasm library). The PNGs live until the next `file.flatten`.

## Building from a flat deck

`file.fromFlat` makes one slide per flat slide on the blank layout (master graphics off) with every
element placed absolutely and every property explicit. Text placeholders (`title`, `ctrTitle`,
`subTitle`, `body`, `obj`) become placeholders again, so slide titles survive into PPTX. Media are
found, in order, in the command's `media` map (base64), in bytes the host staged
(`Engine.putMedia`), among the PNGs of the last flatten, and in the open presentation (by numeric
id) — so a deck flattened from the open presentation can be sent back without its media. A missing
media id is an error naming it.

Round trip: `flatten → fromFlat → flatten` gives the same slides, boxes, text, runs and fills,
except that `raster` elements come back as `picture`s, `fromMaster` graphics as ordinary elements,
and ids are new. Charts of type `other` come back as pictures. Numbers out of range are clamped and
unknown names fall back to defaults; malformed decks are errors, never crashes.
