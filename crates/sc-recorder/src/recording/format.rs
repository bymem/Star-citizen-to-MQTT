//! `frames.bin`: an 8-byte header, then length-prefixed records.
//!
//! All integers are little-endian so the file is trivial to parse from any
//! language. Record layout: `u64 t_us`, `u16 segment`, `u32 len`, payload.

use std::io::{self, BufWriter, Write};

use bytes::Bytes;

/// Magic plus format version (last byte).
pub const MAGIC: &[u8; 8] = b"SCREC\0\0\x01";
const RECORD_HEADER_LEN: usize = 8 + 2 + 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRecord {
    /// Microseconds since recording start (monotonic clock).
    pub t_us: u64,
    /// Connection index, starting at 0.
    pub segment: u16,
    /// Raw `TelemetryStreamResponse` bytes exactly as received.
    pub payload: Bytes,
}

/// Appends records to `frames.bin`. Buffered; the caller flushes on a timer
/// so a crash loses at most about one second of frames.
pub struct FrameWriter<W: Write> {
    out: BufWriter<W>,
}

impl<W: Write> FrameWriter<W> {
    pub fn new(inner: W) -> io::Result<Self> {
        let mut out = BufWriter::with_capacity(256 * 1024, inner);
        out.write_all(MAGIC)?;
        Ok(Self { out })
    }

    pub fn write(&mut self, t_us: u64, segment: u16, payload: &[u8]) -> io::Result<()> {
        let len = u32::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame over 4 GiB"))?;
        self.out.write_all(&t_us.to_le_bytes())?;
        self.out.write_all(&segment.to_le_bytes())?;
        self.out.write_all(&len.to_le_bytes())?;
        self.out.write_all(payload)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    /// Flushes and returns the underlying writer.
    pub fn into_inner(self) -> io::Result<W> {
        self.out.into_inner().map_err(|e| e.into_error())
    }
}

/// Iterates the records of an in-memory `frames.bin`.
///
/// A truncated last record (crash or kill mid-write) ends iteration cleanly;
/// `truncated_bytes()` tells how much was left over.
pub struct FrameReader {
    data: Bytes,
    pos: usize,
}

impl FrameReader {
    pub fn new(data: Bytes) -> io::Result<Self> {
        if data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a frames.bin file (bad magic or unsupported version)",
            ));
        }
        Ok(Self {
            data,
            pos: MAGIC.len(),
        })
    }

    /// Bytes after the last complete record.
    pub fn truncated_bytes(&self) -> usize {
        self.data.len() - self.pos
    }
}

impl Iterator for FrameReader {
    type Item = FrameRecord;

    fn next(&mut self) -> Option<FrameRecord> {
        let rest = &self.data[self.pos..];
        if rest.len() < RECORD_HEADER_LEN {
            return None;
        }
        let t_us = u64::from_le_bytes(rest[0..8].try_into().unwrap());
        let segment = u16::from_le_bytes(rest[8..10].try_into().unwrap());
        let len = u32::from_le_bytes(rest[10..14].try_into().unwrap()) as usize;
        let start = self.pos + RECORD_HEADER_LEN;
        let end = start.checked_add(len)?;
        if end > self.data.len() {
            return None;
        }
        self.pos = end;
        Some(FrameRecord {
            t_us,
            segment,
            // Zero-copy slice into the shared buffer.
            payload: self.data.slice(start..end),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut w = FrameWriter::new(Vec::new()).unwrap();
        w.write(0, 0, b"abc").unwrap();
        w.write(58_000, 0, b"").unwrap();
        w.write(120_000, 1, &[7u8; 300]).unwrap();
        w.into_inner().unwrap()
    }

    #[test]
    fn round_trip() {
        let records: Vec<_> = FrameReader::new(sample().into()).unwrap().collect();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].payload.as_ref(), b"abc");
        assert_eq!(records[1].t_us, 58_000);
        assert!(records[1].payload.is_empty());
        assert_eq!(records[2].segment, 1);
        assert_eq!(records[2].payload.len(), 300);
    }

    #[test]
    fn tolerates_truncated_last_record() {
        let full = sample();
        // Cut into the middle of the last payload, then into its header.
        for cut in [full.len() - 1, full.len() - 300 - 5] {
            let mut reader = FrameReader::new(Bytes::copy_from_slice(&full[..cut])).unwrap();
            assert_eq!(reader.by_ref().count(), 2);
            assert!(reader.truncated_bytes() > 0);
        }
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(FrameReader::new(Bytes::from_static(b"NOTSCREC")).is_err());
    }
}
