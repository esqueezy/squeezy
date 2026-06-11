#!/usr/bin/env node
'use strict';

// xtermcheck — the standalone, gated VS Code (xterm.js) oracle.
//
// It loads a CaptureLog (the append-only ANSI stream squeezy's renderer emits,
// serialized to the self-describing JSON shape below), replays it through
// @xterm/headless — the exact terminal engine VS Code's integrated terminal
// uses — and asserts that the moon-crescent (`☽`) divider does NOT stack.
//
// (Historically this was one leg of an in-process Rust term-matrix whose
// `paint_main` producer lived under crates/squeezy-tui/src/termsim/; that
// matrix was removed in the alt-screen migration, so this oracle now stands
// alone and consumes the JSON contract directly.)
//
// "Stacking" is the migration regression this oracle exists to catch: under
// xterm.js's cursor/reflow behavior the append-only renderer could leave more
// than one `╰─☽ … ─────` divider visible in the live viewport at once. Exactly
// one (or zero, before the first turn closes) is correct; two or more means the
// renderer left stale dividers painted, so we exit non-zero.
//
// CaptureLog JSON shape (self-describing; no Rust producer required):
//   {
//     "bytes_base64": "...",          // OR "bytes_hex": "..."
//     "frames": [ { "byte_offset": N, "w": COLS, "h": ROWS }, ... ]
//   }
// Frame i's bytes are bytes[frames[i-1].byte_offset .. frames[i].byte_offset]
// (frame 0 starts at offset 0), so the log is self-slicing per frame. Each
// frame carries the (w, h) in effect when it was painted, so we
// term.resize(w, h) before writing that frame's slice — reproducing the
// per-frame resize that drove the original capture.

const fs = require('fs');
const path = require('path');

// The viewport divider painted by the closing turn line looks like:
//   "   ╰─☽ Cancelled after 5s ───────────────"
// i.e. the moon glyph, optional spacing/text, then a run of box-drawing dashes.
// We treat any viewport row whose moon glyph is (eventually) followed by a
// dash-class character as one rendered divider. The character classes here are
// U+2500 (─), U+254C (╌), U+2508 (┈) — the dashes the renderer can emit.
const DIVIDER_RE = /☽[^\n]*?[─╌┈]/u;

function usageAndExit(code) {
  const msg =
    'Usage: node replay.js <capturelog.json>\n' +
    '\n' +
    'Replays an exported CaptureLog through @xterm/headless (the VS Code\n' +
    'terminal engine) and exits non-zero if more than one ☽ divider is\n' +
    'visible in the final viewport (divider stacking).\n' +
    '\n' +
    'CaptureLog JSON: { "bytes_base64" | "bytes_hex", "frames": [ { "byte_offset", "w", "h" } ] }\n';
  process.stderr.write(msg);
  process.exit(code);
}

function decodeBytes(log, file) {
  if (typeof log.bytes_base64 === 'string') {
    return Buffer.from(log.bytes_base64, 'base64');
  }
  if (typeof log.bytes_hex === 'string') {
    const hex = log.bytes_hex.replace(/\s+/g, '');
    if (hex.length % 2 !== 0) {
      fail(`${file}: bytes_hex has an odd number of hex digits`);
    }
    return Buffer.from(hex, 'hex');
  }
  fail(`${file}: CaptureLog must contain "bytes_base64" or "bytes_hex"`);
}

function fail(message) {
  process.stderr.write(`xtermcheck: ${message}\n`);
  process.exit(2);
}

function loadTerminalCtor() {
  // @xterm/headless ships a CommonJS build whose default export is the
  // Terminal class. Resolve it relative to this file so the oracle works no
  // matter what the caller's cwd is.
  let mod;
  try {
    mod = require('@xterm/headless');
  } catch (err) {
    fail(
      '@xterm/headless is not installed. Run `npm install` in ' +
        path.dirname(__filename) +
        ' first.\nUnderlying error: ' +
        (err && err.message ? err.message : String(err)),
    );
  }
  const Terminal = mod.Terminal || mod.default || mod;
  if (typeof Terminal !== 'function') {
    fail('@xterm/headless did not export a Terminal constructor');
  }
  return Terminal;
}

function readViewportLines(term) {
  const buffer = term.buffer.active;
  const rows = term.rows;
  const base = buffer.baseY; // first row of the live viewport in buffer space
  const lines = [];
  for (let y = 0; y < rows; y++) {
    const line = buffer.getLine(base + y);
    // translateToString(trimRight=true) drops trailing blank cells so rows
    // compare cleanly across reflows (trailing spaces are not significant for
    // divider detection).
    lines.push(line ? line.translateToString(true) : '');
  }
  return lines;
}

function main() {
  const file = process.argv[2];
  if (!file || file === '-h' || file === '--help') {
    usageAndExit(file ? 0 : 1);
  }

  let raw;
  try {
    raw = fs.readFileSync(file, 'utf8');
  } catch (err) {
    fail(`cannot read ${file}: ${err && err.message ? err.message : err}`);
  }

  let log;
  try {
    log = JSON.parse(raw);
  } catch (err) {
    fail(`${file} is not valid JSON: ${err && err.message ? err.message : err}`);
  }

  const bytes = decodeBytes(log, file);
  const frames = Array.isArray(log.frames) ? log.frames : [];
  if (frames.length === 0) {
    fail(`${file}: CaptureLog has no frames to replay`);
  }

  const Terminal = loadTerminalCtor();

  // Seed with the first frame's size; resize per frame below. allowProposedApi
  // is required to read `buffer.active.baseY` / iterate buffer lines on current
  // @xterm/headless. scrollback is generous so committed dividers stay
  // countable if they ever escape the viewport.
  const first = frames[0];
  const term = new Terminal({
    cols: first.w,
    rows: first.h,
    allowProposedApi: true,
    scrollback: 10000,
  });

  // Replay each frame at its own geometry. xterm.js parses term.write() bytes
  // ASYNCHRONOUSLY, so a synchronous for-loop that calls term.resize() between
  // writes would apply every resize up front and then parse the whole queued
  // stream at the FINAL geometry — collapsing any mid-stream reflow and hiding
  // exactly the resize-tearing / divider-stacking regression this oracle
  // exists to catch. To make per-frame reflow actually happen we chain each
  // frame's resize+write inside the PRIOR frame's write callback, so the
  // parser has fully consumed frame i at its (w, h) before we resize for frame
  // i+1. The divider drain runs only in the final frame's callback.
  function finish() {
    const lines = readViewportLines(term);
    let dividerCount = 0;
    const matched = [];
    for (const line of lines) {
      if (DIVIDER_RE.test(line)) {
        dividerCount++;
        matched.push(line);
      }
    }

    const finalFrame = frames[frames.length - 1];
    process.stdout.write(
      `xtermcheck: replayed ${frames.length} frame(s), ` +
        `${bytes.length} byte(s); final viewport ${finalFrame.w}x${finalFrame.h}; ` +
        `${dividerCount} ☽ divider line(s) in viewport\n`,
    );

    if (dividerCount > 1) {
      process.stderr.write(
        `xtermcheck: FAIL — ☽ divider STACKED (${dividerCount} visible, expected <= 1):\n`,
      );
      for (const line of matched) {
        process.stderr.write(`  | ${line}\n`);
      }
      process.exit(1);
    }

    process.stdout.write('xtermcheck: OK — no divider stacking\n');
    process.exit(0);
  }

  function step(i, start) {
    if (i >= frames.length) {
      // Final drain: an empty write whose callback fires once the parser has
      // consumed everything queued above.
      term.write(new Uint8Array(0), finish);
      return;
    }
    const frame = frames[i];
    const end = frame.byte_offset;
    if (end < start || end > bytes.length) {
      fail(
        `${file}: frame ${i} byte_offset ${end} is out of range ` +
          `(prev=${start}, len=${bytes.length})`,
      );
    }
    // Per-frame resize reproduces the FixedSize the harness drove for this
    // paint. We resize, then write this frame's slice, and only advance to the
    // next frame once xterm.js has parsed the slice at this geometry.
    if (term.cols !== frame.w || term.rows !== frame.h) {
      term.resize(frame.w, frame.h);
    }
    const slice = bytes.subarray(start, end);
    if (slice.length > 0) {
      // Uint8Array write path keeps raw bytes intact (no utf-8 re-encode).
      term.write(Uint8Array.from(slice), () => step(i + 1, end));
    } else {
      // Empty slice: nothing to parse, but still drain to preserve ordering
      // before moving on to the next frame's resize.
      term.write(new Uint8Array(0), () => step(i + 1, end));
    }
  }

  step(0, 0);
}

try {
  main();
} catch (err) {
  // Surface internal emulator throws as a clean message instead of dumping the
  // minified @xterm/headless bundle as a stack trace.
  fail(err && err.message ? err.message : String(err));
}
