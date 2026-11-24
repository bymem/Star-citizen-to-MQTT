//! Cheap, schema-free scan of a frame's top-level fields.
//!
//! Used for the live status line and coverage stats. Walks protobuf
//! wire-format keys without decoding values, so it costs next to nothing at
//! ~17 frames per second.

/// Top-level sections of `TelemetryStreamResponse` (field number, name),
/// in the order they are shown to the player.
pub const SECTIONS: [(u32, &str); 6] = [
    (6, "actor"),
    (3, "vehicle"),
    (4, "weapons"),
    (5, "events"),
    (7, "force_feedback"),
    (8, "turret"),
];

/// Bit set of sections present in one frame; bit N = field number N.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SectionSet(pub u32);

impl SectionSet {
    pub fn contains(self, field: u32) -> bool {
        field < 32 && self.0 & (1 << field) != 0
    }
}

/// Returns which top-level fields appear in `frame`. Stops quietly at the
/// first malformed key, since this is only for statistics.
pub fn scan(frame: &[u8]) -> SectionSet {
    let mut set = 0u32;
    let mut pos = 0;
    while pos < frame.len() {
        let Some(key) = read_varint(frame, &mut pos) else {
            break;
        };
        let field = (key >> 3) as u32;
        if field < 32 {
            set |= 1 << field;
        }
        let ok = match key & 7 {
            0 => read_varint(frame, &mut pos).is_some(),
            1 => skip(frame, &mut pos, 8),
            2 => match read_varint(frame, &mut pos) {
                Some(len) => skip(frame, &mut pos, len as usize),
                None => false,
            },
            5 => skip(frame, &mut pos, 4),
            _ => false,
        };
        if !ok {
            break;
        }
    }
    SectionSet(set)
}

/// Reads a protobuf varint, advancing `pos`. `None` on truncation or overflow.
pub fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn skip(buf: &[u8], pos: &mut usize, n: usize) -> bool {
    match pos.checked_add(n) {
        Some(end) if end <= buf.len() => {
            *pos = end;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_top_level_sections() {
        // field 3 (len 2: 08 01), field 6 (len 0), field 5 varint-free len 1.
        let frame = [0x1A, 0x02, 0x08, 0x01, 0x32, 0x00, 0x2A, 0x01, 0x00];
        let set = scan(&frame);
        assert!(set.contains(3));
        assert!(set.contains(5));
        assert!(set.contains(6));
        assert!(!set.contains(4));
    }

    #[test]
    fn truncated_frame_does_not_panic() {
        let set = scan(&[0x1A, 0x10, 0x00]);
        assert!(set.contains(3));
    }
}
