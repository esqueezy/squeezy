'use strict';

// Regression tests for replay.js (the xterm.js VS Code oracle leg).
//
// The headline test (`surfaces a reflow-driven divider stack`) is the
// regression guard for deep-review #51: replay.js used to apply every frame's
// term.resize() synchronously while xterm.js parsed term.write() bytes
// asynchronously, so the whole stream was parsed at the FINAL geometry. That
// collapsed mid-stream reflow and HID divider stacking that a per-frame replay
// would surface. The `reflow-stack-capturelog.json` fixture is constructed so
// that:
//   * parsed entirely at the final (narrow) geometry -> the two ☽ dividers
//     wrap onto a single matched row  -> count 1 -> replay.js exits 0 (the bug
//     masks the stack), and
//   * parsed per-frame (wide then narrow, with the parser draining between)
//     -> the two ☽ dividers land on separate rows -> count 2 -> replay.js
//     exits 1 (FAIL, stack surfaced).
// So this test FAILS against the old synchronous-resize replay.js (it would see
// exit 0) and PASSES against the per-frame-chained fix (exit 1).

const test = require('node:test');
const assert = require('node:assert');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const TOOL_DIR = path.resolve(__dirname, '..');
const REPLAY = path.join(TOOL_DIR, 'replay.js');
const FIXTURES = path.join(__dirname, 'fixtures');

function runReplay(fixture) {
  return spawnSync(process.execPath, [REPLAY, path.join(FIXTURES, fixture)], {
    cwd: TOOL_DIR,
    encoding: 'utf8',
  });
}

test('surfaces a reflow-driven divider stack across a width change', () => {
  const res = runReplay('reflow-stack-capturelog.json');
  // With the per-frame-chained replay, the second (narrow) frame is parsed only
  // after xterm.js has reflowed the first (wide) frame, so both ☽ dividers stay
  // on their own rows and the stack is caught: replay.js exits 1.
  assert.strictEqual(
    res.status,
    1,
    `expected exit 1 (divider STACKED), got ${res.status}.\n` +
      `stdout:\n${res.stdout}\nstderr:\n${res.stderr}`,
  );
  assert.match(res.stderr, /divider STACKED/);
});

test('passes a clean single-divider capture', () => {
  const res = runReplay('single-divider-capturelog.json');
  assert.strictEqual(
    res.status,
    0,
    `expected exit 0 (no stacking), got ${res.status}.\n` +
      `stdout:\n${res.stdout}\nstderr:\n${res.stderr}`,
  );
  assert.match(res.stdout, /no divider stacking/);
});
