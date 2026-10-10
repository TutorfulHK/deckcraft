# @tutorfulhk/deckcraft-wasm

DeckCraft as a headless WebAssembly library, for web apps that keep their own slide model: open
and save `.pptx`, convert to and from the **flat deck** (a resolved, absolutely positioned slide
model, see [`docs/flat-deck.md`](../../docs/flat-deck.md)), render slide thumbnails to PNG, and
export PDF. It is the DeckCraft engine (`crates/engine`) behind a thin wasm-bindgen binding
(`crates/wasm`); it is not the egui app.

Run it in a **Web Worker**: calls are synchronous and a large deck takes a while. The module never
panics on purpose, but a WebAssembly trap can't be caught inside the module: if a call throws a
`WebAssembly.RuntimeError`, restart the worker.

## API

```ts
import init, { Engine, addFont, version } from "@tutorfulhk/deckcraft-wasm";

await init();                                   // fetches deckcraft_wasm_bg.wasm next to the module
addFont(new Uint8Array(await (await fetch("/fonts/Inter.ttf")).arrayBuffer())); // see Fonts

const engine = new Engine();
engine.openBytes("talk.pptx", pptxBytes);       // .pptx (or .deckcraft); replaces the open deck
const deck = JSON.parse(engine.flatten());      // the flat deck
for (const m of deck.media) {
  const bytes = engine.mediaBytes(m.id);        // Uint8Array: pictures, raster PNGs, chart PNGs, video
}
const thumb = engine.renderPng(0, 0.25);        // slide 0 as PNG at 0.25 px per point

// Back again: stage new media (raw bytes, no base64), then build from the flat deck.
engine.putMedia("my-image-1", imageBytes);
engine.fromFlat(JSON.stringify(deck));          // media ids from the last flatten need no re-upload
const pptx = engine.saveBytes("pptx");          // Uint8Array; also "pdf" and "deckcraft"

engine.execute("slide.new", JSON.stringify({ layout: "blank" })); // any engine command (JSON in/out)
version();                                      // "0.4.0"
```

| Call | Does |
|---|---|
| `new Engine()` | an engine with no presentation open |
| `execute(id, paramsJson) → string` | runs any engine command (`deckcraft-cli commands` lists them) |
| `openBytes(name, bytes)` | opens `.pptx` / `.deckcraft` bytes, replacing the open presentation |
| `saveBytes(format) → Uint8Array` | `"pptx"`, `"pdf"` (one page per slide) or `"deckcraft"` |
| `flatten() → string` | the flat deck as JSON |
| `putMedia(id, bytes)` | stages media for the next `fromFlat` |
| `fromFlat(deckJson)` | replaces the open presentation (or opens one) with one built from a flat deck |
| `mediaBytes(id) → Uint8Array` | bytes of a media id the flat deck references |
| `renderPng(slide, scale) → Uint8Array` | slide (0-based) as PNG; `scale` is pixels per point, capped at 4 |
| `addFont(bytes) → number` | adds a TTF/OTF/TTC font; returns the faces added |
| `version() → string` | library version (also `Engine.version()`) |

Errors are thrown as `Error` with the engine's message (unknown command, bad parameters, a file
that isn't a presentation, missing media…).

`fromFlat` finds media in this order: bytes staged with `putMedia`, PNGs made by the last
`flatten`, and the open presentation's own media (by the numeric ids `flatten` gave them). A
missing id is an error naming it.

## Fonts

Text is laid out and rendered with real font metrics, so load the fonts your decks use with
`addFont` before opening, rendering or flattening them (font files are not part of this package —
see the craft-fonts repository for openly licensed ones). Without them the engine falls back to
its one built-in face.

## Building

```sh
cargo install wasm-bindgen-cli --locked --version 0.2.129   # the version in Cargo.lock
npm install -g binaryen                                     # wasm-opt (optional, about halves the module)
packaging/wasm/build.sh target/wasm-pkg                     # FEATURES=none: without media probing
```

`build.sh` builds with the `wasm-release` profile (`opt-level = "z"`, fat LTO), runs
`wasm-bindgen --target web` and `wasm-opt -Oz`, writes `package.json` from the template here, and
prints the module size. The `media` feature (on by default) adds audio/video probing and video
poster frames (the codecs); without it, media is still embedded, flattened and saved.

The GitHub workflow `.github/workflows/wasm-package.yml` does the same on a tag `wasm-v<version>`
and publishes the package to GitHub Packages (`https://npm.pkg.github.com`). To install it, point
the scope at that registry in `.npmrc`:

```
@tutorfulhk:registry=https://npm.pkg.github.com
```

## Size

`deckcraft_wasm_bg.wasm` (no fonts embedded) from `build.sh`, measured 2026-10-10 with
wasm-bindgen 0.2.129 and wasm-opt (binaryen) 132:

| Build | After wasm-bindgen | + `wasm-opt -Oz` | gzip -9 | brotli -q 11 |
|---|---|---|---|---|
| `FEATURES=none` | 15.2 MB | 8.37 MB | 2.90 MB | 2.03 MB |
| with `media` | 19.0 MB | 10.6 MB | 3.85 MB | 2.75 MB |

The compressed columns are of the `wasm-opt` output. Without `wasm-opt` (wasm-bindgen output
only) the transfer sizes are 3.45 MB gzip / 2.24 MB brotli without `media`, and 4.58 MB /
3.01 MB with it. The JS glue is about 13 KB (3 KB gzipped).

## Licence

MIT OR Apache-2.0, as DeckCraft. Third-party notices: `NOTICE`.
