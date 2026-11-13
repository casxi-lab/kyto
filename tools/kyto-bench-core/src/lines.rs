//! Lines of a console transcript.
//!
//! A transcript is the bytes received from the console, exactly as they
//! arrived, in chunks that each carry the time at which they were received.
//! A line is a run of bytes ended by LF; one CR immediately before the LF is
//! not part of the line's content. A line's offset is the offset of its first
//! byte in the transcript, and its time is the time of the chunk that holds
//! its LF. Bytes left after the last LF when the capture ends form one more
//! line that is not terminated; its time is that of the last chunk.
//!
//! The same [`LineSplitter`] splits a transcript while it is received and
//! when it is read back from a record, so both see the same lines.

use std::fmt;

/// One line of a transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The offset of the line's first byte in the transcript.
    pub offset: u64,
    /// The bytes of the line, without its LF and without one CR immediately
    /// before the LF.
    pub content: Vec<u8>,
    /// Whether an LF ended the line; only the last line of a capture can be
    /// unterminated.
    pub terminated: bool,
    /// The time of the chunk that holds the line's LF, or of the last chunk
    /// for an unterminated line, in nanoseconds of the run's clock.
    pub at_ns: u64,
}

/// Splits chunks of a transcript into lines as they arrive.
#[derive(Clone, Debug, Default)]
pub struct LineSplitter {
    /// The offset of the first byte of `pending` in the transcript.
    start: u64,
    /// The bytes received after the last LF.
    pending: Vec<u8>,
    /// The time of the last non-empty chunk.
    last_at_ns: Option<u64>,
}

impl LineSplitter {
    /// A splitter at the start of a transcript.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of bytes received so far.
    pub fn received(&self) -> u64 {
        self.start + self.pending.len() as u64
    }

    /// The bytes received after the last LF: the start of a line that is not
    /// ended yet.
    pub fn pending(&self) -> &[u8] {
        &self.pending
    }

    /// Takes the next chunk, received at `at_ns`, and gives `each` the lines
    /// it ends, in order, one at a time, each with the offset after its LF.
    /// An empty chunk ends no line and leaves the time of the last chunk
    /// unchanged.
    pub fn push(&mut self, at_ns: u64, bytes: &[u8], mut each: impl FnMut(Line, u64)) {
        if bytes.is_empty() {
            return;
        }
        self.last_at_ns = Some(at_ns);
        let mut rest = bytes;
        while let Some(at) = rest.iter().position(|byte| *byte == b'\n') {
            self.pending.extend_from_slice(&rest[..at]);
            let length = self.pending.len() as u64 + 1;
            let mut content = std::mem::take(&mut self.pending);
            if content.last() == Some(&b'\r') {
                content.pop();
            }
            let line = Line {
                offset: self.start,
                content,
                terminated: true,
                at_ns,
            };
            self.start += length;
            each(line, self.start);
            rest = &rest[at + 1..];
        }
        self.pending.extend_from_slice(rest);
    }

    /// Ends the transcript and returns the unterminated last line, if bytes
    /// are left after the last LF. A CR at its end is content.
    pub fn finish(self) -> Option<Line> {
        if self.pending.is_empty() {
            return None;
        }
        Some(Line {
            offset: self.start,
            content: self.pending,
            terminated: false,
            at_ns: self.last_at_ns.unwrap_or(0),
        })
    }
}

/// Splits a whole transcript, given as its chunks with their times, into
/// lines.
pub fn split<'a>(chunks: impl IntoIterator<Item = (u64, &'a [u8])>) -> Vec<Line> {
    let mut splitter = LineSplitter::new();
    let mut lines = Vec::new();
    for (at_ns, bytes) in chunks {
        splitter.push(at_ns, bytes, |line, _| lines.push(line));
    }
    lines.extend(splitter.finish());
    lines
}

/// Why an index does not describe the chunks of a transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexError {
    /// The transcript has bytes but the index has no entry, or the first
    /// entry's offset is not 0.
    NotFromStart,
    /// An entry's offset is not greater than the one before, so a chunk
    /// would be empty.
    OffsetNotIncreasing { entry: usize },
    /// An entry's offset is at or past the end of the transcript.
    OffsetPastEnd { entry: usize },
    /// An entry's time is earlier than the one before.
    TimeDecreasing { entry: usize },
}

/// The chunks of `transcript` that `index` describes. Each index entry is
/// `[offset, at_ns]`: the offset of the chunk's first byte and the time it
/// was received. Offsets start at 0 and increase, every chunk has at least
/// one byte, and times do not decrease. An empty transcript has an empty
/// index.
pub fn chunks<'a>(
    transcript: &'a [u8],
    index: &[[u64; 2]],
) -> Result<Vec<(u64, &'a [u8])>, IndexError> {
    let len = transcript.len() as u64;
    match index.first() {
        None if len == 0 => return Ok(Vec::new()),
        Some([0, _]) => {}
        _ => return Err(IndexError::NotFromStart),
    }
    for (entry, pair) in index.windows(2).enumerate() {
        let ([offset, at_ns], [next_offset, next_at_ns]) = (pair[0], pair[1]);
        if next_offset <= offset {
            return Err(IndexError::OffsetNotIncreasing { entry: entry + 1 });
        }
        if next_at_ns < at_ns {
            return Err(IndexError::TimeDecreasing { entry: entry + 1 });
        }
    }
    let mut result = Vec::with_capacity(index.len());
    for (entry, [offset, at_ns]) in index.iter().copied().enumerate() {
        if offset >= len {
            return Err(IndexError::OffsetPastEnd { entry });
        }
        let end = index.get(entry + 1).map_or(len, |next| next[0]).min(len);
        // Offsets are below `len`, which came from a `usize`.
        result.push((at_ns, &transcript[offset as usize..end as usize]));
    }
    Ok(result)
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFromStart => write!(f, "the index does not start at offset 0"),
            Self::OffsetNotIncreasing { entry } => {
                write!(f, "index entry {entry} starts too early")
            }
            Self::OffsetPastEnd { entry } => write!(f, "index entry {entry} starts past the end"),
            Self::TimeDecreasing { entry } => write!(f, "index entry {entry} is earlier"),
        }
    }
}

impl std::error::Error for IndexError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(offset: u64, content: &[u8], terminated: bool, at_ns: u64) -> Line {
        Line {
            offset,
            content: content.to_vec(),
            terminated,
            at_ns,
        }
    }

    #[test]
    fn lines_end_at_lf_and_one_cr_before_it_is_not_content() {
        let lines = split([(7, &b"one\r\ntwo\nthree\r\r\n"[..])]);
        assert_eq!(
            lines,
            [
                line(0, b"one", true, 7),
                line(5, b"two", true, 7),
                line(9, b"three\r", true, 7),
            ]
        );
    }

    #[test]
    fn lf_cr_lf_is_a_line_followed_by_an_empty_line() {
        let lines = split([(1, &b"TFTP_GET x\n\r\nnext\r\n"[..])]);
        assert_eq!(
            lines,
            [
                line(0, b"TFTP_GET x", true, 1),
                line(11, b"", true, 1),
                line(13, b"next", true, 1),
            ]
        );
    }

    #[test]
    fn a_line_takes_the_time_of_the_chunk_that_holds_its_lf() {
        let lines = split([
            (10, &b"par"[..]),
            (20, &b"tial\r"[..]),
            (30, &b"\nnext"[..]),
            (40, &b" more\n"[..]),
        ]);
        assert_eq!(
            lines,
            [
                line(0, b"partial", true, 30),
                line(9, b"next more", true, 40)
            ]
        );
    }

    #[test]
    fn an_unterminated_last_line_takes_the_time_of_the_last_chunk() {
        let lines = split([(5, &b"done\nrest"[..]), (9, &b" end\r"[..]), (12, &b""[..])]);
        assert_eq!(
            lines,
            [line(0, b"done", true, 5), line(5, b"rest end\r", false, 9)]
        );
        assert!(split([]).is_empty());
        assert_eq!(split([(3, &b"\n"[..])]), [line(0, b"", true, 3)]);
    }

    #[test]
    fn bytes_are_kept_as_received() {
        let lines = split([(1, &b"\xff\x00\xc3(\r\n"[..])]);
        assert_eq!(lines, [line(0, b"\xff\x00\xc3(", true, 1)]);
    }

    #[test]
    fn the_splitter_counts_received_bytes_and_holds_the_open_line() {
        let mut splitter = LineSplitter::new();
        let pushed = |splitter: &mut LineSplitter, at_ns, bytes: &[u8]| {
            let mut lines = Vec::new();
            splitter.push(at_ns, bytes, |line, after| lines.push((line, after)));
            lines
        };
        let lines = pushed(&mut splitter, 1, b"ab\r\nc\ncd");
        assert_eq!(
            lines,
            [(line(0, b"ab", true, 1), 4), (line(4, b"c", true, 1), 6)]
        );
        assert_eq!(splitter.received(), 8);
        assert_eq!(splitter.pending(), b"cd");
        assert!(pushed(&mut splitter, 2, b"").is_empty());
        assert_eq!(splitter.finish(), Some(line(6, b"cd", false, 1)));
    }

    #[test]
    fn splitting_does_not_depend_on_chunk_boundaries() {
        let transcript = b"a\r\nbb\n\r\n\rc\r\r\nd";
        let whole = split([(0, &transcript[..])]);
        for cut in 0..=transcript.len() {
            let (first, second) = transcript.split_at(cut);
            let parts = split([(0, first), (0, second)]);
            assert_eq!(parts, whole, "cut at {cut}");
        }
    }

    #[test]
    fn an_index_gives_the_chunks_of_the_transcript() {
        let transcript = b"abcdef";
        let found = chunks(transcript, &[[0, 5], [2, 5], [5, 9]]).unwrap();
        assert_eq!(found, [(5, &b"ab"[..]), (5, &b"cde"[..]), (9, &b"f"[..])]);
        assert!(chunks(b"", &[]).unwrap().is_empty());
    }

    #[test]
    fn an_index_that_does_not_describe_the_transcript_is_refused() {
        let transcript = b"abcdef";
        let cases: [(&[[u64; 2]], IndexError); 6] = [
            (&[], IndexError::NotFromStart),
            (&[[1, 0]], IndexError::NotFromStart),
            (
                &[[0, 0], [0, 1]],
                IndexError::OffsetNotIncreasing { entry: 1 },
            ),
            (
                &[[0, 0], [3, 1], [2, 2]],
                IndexError::OffsetNotIncreasing { entry: 2 },
            ),
            (&[[0, 0], [6, 1]], IndexError::OffsetPastEnd { entry: 1 }),
            (&[[0, 4], [3, 3]], IndexError::TimeDecreasing { entry: 1 }),
        ];
        for (index, error) in cases {
            assert_eq!(chunks(transcript, index), Err(error), "{index:?}");
        }
        assert_eq!(
            chunks(b"", &[[0, 0]]),
            Err(IndexError::OffsetPastEnd { entry: 0 })
        );
    }
}
