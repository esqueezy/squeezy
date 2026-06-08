use super::*;
use crate::termsim::types::{CaptureLog, FrameMark};

/// Helper: a `FrameMark` at `byte_offset` with a placeholder size.
fn mark(byte_offset: usize) -> FrameMark {
    FrameMark {
        byte_offset,
        w: 80,
        h: 24,
    }
}

#[test]
fn split_frames_slices_consecutive_ranges() {
    // Three marks carve the buffer into [0..2), [2..5), [5..7).
    let log = CaptureLog {
        bytes: b"ABCDEFG".to_vec(),
        frames: vec![mark(2), mark(5), mark(7)],
    };
    let frames = split_frames(&log);
    let slices: Vec<&[u8]> = frames.iter().map(|f| f.bytes).collect();
    assert_eq!(slices, vec![&b"AB"[..], &b"CDE"[..], &b"FG"[..]]);
}

#[test]
fn split_frames_clamps_offset_beyond_buffer_len() {
    // A truncated/corrupt capture: the second mark points past the end of the
    // byte buffer. The end is clamped to `bytes.len()` so the slice covers the
    // remaining bytes rather than panicking on an out-of-range index.
    let log = CaptureLog {
        bytes: b"ABCD".to_vec(),
        frames: vec![mark(2), mark(999)],
    };
    let frames = split_frames(&log);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].bytes, b"AB");
    assert_eq!(
        frames[1].bytes, b"CD",
        "an offset past the buffer end is clamped to bytes.len()",
    );
}

#[test]
fn split_frames_clamps_out_of_order_marks_to_empty() {
    // Marks must be non-decreasing; an out-of-order (backwards) offset would
    // produce a reversed range. The cursor is held monotonic, so the offending
    // frame clamps to an EMPTY slice instead of panicking on `start > end`.
    let log = CaptureLog {
        bytes: b"ABCDEF".to_vec(),
        // 4, then a backwards 1 (< 4), then 6.
        frames: vec![mark(4), mark(1), mark(6)],
    };
    let frames = split_frames(&log);
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].bytes, b"ABCD");
    assert!(
        frames[1].bytes.is_empty(),
        "a backwards offset clamps to an empty slice, not a reversed range",
    );
    assert_eq!(
        frames[2].bytes, b"EF",
        "the cursor resumes from the monotonic high-water mark (4)",
    );
}

#[test]
fn split_frames_empty_marks_yield_no_slices() {
    // No frame marks at all: there is nothing to slice, so the splitter returns
    // an empty vec (the backends handle the frameless case themselves).
    let log = CaptureLog {
        bytes: b"some bytes".to_vec(),
        frames: vec![],
    };
    assert!(split_frames(&log).is_empty());
}

#[test]
fn split_frames_handles_empty_byte_buffer() {
    // Marks on an empty buffer: every offset clamps to 0, so each frame is an
    // empty slice and nothing indexes out of range.
    let log = CaptureLog {
        bytes: Vec::new(),
        frames: vec![mark(3), mark(7)],
    };
    let frames = split_frames(&log);
    assert_eq!(frames.len(), 2);
    assert!(frames.iter().all(|f| f.bytes.is_empty()));
}
