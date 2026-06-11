# xtermcheck — the standalone VS Code (xterm.js) oracle

`xtermcheck` is an **out-of-process VS Code oracle** for the `squeezy-tui`
append-only renderer. It replays a captured ANSI stream through
[`@xterm/headless`](https://www.npmjs.com/package/@xterm/headless), the exact
terminal engine VS Code's integrated terminal (and anything else built on
xterm.js) uses, and asserts the renderer's turn divider does not stack.

> **History:** this used to be one leg of an in-process Rust term-matrix under
> `crates/squeezy-tui/src/termsim/` (vt100 + alacritty_terminal legs feeding the
> same stream). That matrix and its `paint_main` producer were removed during the
> alt-screen migration (Phase 10). `xtermcheck` now stands on its own: it consumes
> the self-describing `CaptureLog` JSON documented below, so it needs no Rust
> producer, and it carries its own Node regression tests under `test/`.

It exists to catch the one regression those well-behaved Rust emulators cannot
see: under xterm.js's cursor/reflow behavior, the append-only renderer can leave
**more than one `╰─☽ … ─────` divider** painted in the live viewport at once. The
moon-crescent (`☽`) divider closing a turn must appear **at most once**; two or
more means stale dividers stacked. `xtermcheck` exits non-zero when that happens.

## Why it is gated

xterm.js is a Node/npm dependency, so this oracle is **opt-in**: it runs only
when `node` is present (and `@xterm/headless` is installed). The default
`cargo test` path does not invoke it. Treat a missing `node` as "oracle not run",
not as a pass; CI that wants the VS Code guarantee must provision Node and install
deps.

## Tests

The oracle carries its own Node regression tests (no Rust producer required):

```sh
cd crates/squeezy-tui/tools/termsim/xtermcheck
npm install        # pulls @xterm/headless
npm test           # runs node --test against test/
```

The headline test feeds a two-frame `CaptureLog` (`test/fixtures/`) whose first
frame is wide and second frame is narrow, so that correct per-frame replay
surfaces a divider stack across the width change.

## Setup

```sh
cd crates/squeezy-tui/tools/termsim/xtermcheck
npm install        # pulls @xterm/headless
```

## Usage

```sh
node replay.js path/to/capturelog.json
```

Exit codes:

| code | meaning                                              |
|------|------------------------------------------------------|
| `0`  | OK — at most one `☽` divider in the final viewport   |
| `1`  | FAIL — divider **stacked** (2+ visible)              |
| `2`  | usage / bad input (missing file, malformed JSON, …) |

Sample run:

```sh
$ node replay.js fixtures/example-capturelog.json
xtermcheck: replayed 3 frame(s), 412 byte(s); final viewport 80x24; 1 ☽ divider line(s) in viewport
xtermcheck: OK — no divider stacking
```

## CaptureLog JSON contract

The input is a self-describing `CaptureLog`: a byte stream plus per-frame marks
(formerly the `CaptureLog` / `FrameMark` Rust types; the producer is gone, but
the JSON shape is stable and self-contained):

```json
{
  "bytes_base64": "G1s/MjAyNmgbWzJK...",
  "frames": [
    { "byte_offset": 137, "w": 80, "h": 24 },
    { "byte_offset": 290, "w": 80, "h": 24 },
    { "byte_offset": 412, "w": 100, "h": 30 }
  ]
}
```

- `bytes_base64` — the whole append-only ANSI byte stream, base64-encoded.
  Alternatively supply `bytes_hex` (a hex string; whitespace ignored). Exactly
  one of the two is required.
- `frames` — one mark per painted frame, in paint order. Frame *i*'s bytes are
  `bytes[frames[i-1].byte_offset .. frames[i].byte_offset]` (frame 0 starts at
  offset 0), so the log is self-slicing. `w`/`h` are the terminal columns/rows
  (the `FixedSize`) in effect for that paint.

The replay seeds the terminal at frame 0's size, then for each frame calls
`term.resize(w, h)` **before** writing that frame's byte slice — reproducing the
per-frame resize the harness drove. After the last frame it reads the live
viewport (`buffer.active`, anchored at `baseY`) and counts rows matching
`/☽[^\n]*?[─╌┈]/u`.

## Producing a CaptureLog

There is no longer a Rust `CaptureLog` producer (the old `src/termsim/` harness
was removed). A capture is just the JSON above, so any process that can capture
squeezy's emitted byte stream plus the `(w, h)` in effect at each paint can write
one. The committed `test/fixtures/*.json` are hand-built examples; produce more
the same way (collect the bytes, record a `byte_offset` + `w`/`h` per frame,
base64- or hex-encode the bytes).

To run the oracle against a capture in CI or a script:

```sh
command -v node >/dev/null 2>&1 \
  && node crates/squeezy-tui/tools/termsim/xtermcheck/replay.js capturelog.json
```

A non-zero exit fails the VS Code oracle.
