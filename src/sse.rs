//! Incremental Server-Sent Events parser for the replication stream.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SseEvent {
    Comment(String),
    Data(String),
}

#[derive(Debug, Default)]
pub(crate) struct SseParser {
    pending_bytes: Vec<u8>,
    buffer: String,
}

impl SseParser {
    /// Feeds a chunk of the response body and returns the events it completes.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.decode(chunk);

        let mut events = Vec::new();
        while let Some((end, separator_len)) = find_frame_end(&self.buffer) {
            if let Some(event) = parse_frame(&self.buffer[..end]) {
                events.push(event);
            }
            self.buffer.drain(..end + separator_len);
        }
        events
    }

    /// Appends `chunk` to the text buffer, holding back a UTF-8 sequence split across chunks.
    fn decode(&mut self, chunk: &[u8]) {
        self.pending_bytes.extend_from_slice(chunk);
        loop {
            match std::str::from_utf8(&self.pending_bytes) {
                Ok(text) => {
                    self.buffer.push_str(text);
                    self.pending_bytes.clear();
                    return;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    self.buffer.push_str(
                        std::str::from_utf8(&self.pending_bytes[..valid])
                            .expect("validated prefix"),
                    );
                    match e.error_len() {
                        // Incomplete sequence at the end: wait for the next chunk.
                        None => {
                            self.pending_bytes.drain(..valid);
                            return;
                        }
                        Some(len) => {
                            self.buffer.push(char::REPLACEMENT_CHARACTER);
                            self.pending_bytes.drain(..valid + len);
                        }
                    }
                }
            }
        }
    }
}

/// Finds the first blank line (`\n\n`, `\r\n\r\n` or mixed) and returns its start and length.
fn find_frame_end(buffer: &str) -> Option<(usize, usize)> {
    let bytes = buffer.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == b'\r' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'\n' {
                let start = if i > 0 && bytes[i - 1] == b'\r' {
                    i - 1
                } else {
                    i
                };
                return Some((start, j + 1 - start));
            }
        }
        i += 1;
    }
    None
}

fn parse_frame(frame: &str) -> Option<SseEvent> {
    let mut data_lines = Vec::new();
    let mut comment = None;
    for line in frame.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)) {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix(':') {
            comment = Some(rest.to_owned());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(char::is_whitespace).unwrap_or(rest));
        }
    }
    if !data_lines.is_empty() {
        Some(SseEvent::Data(data_lines.join("\n")))
    } else {
        comment.map(SseEvent::Comment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_frames() {
        let mut p = SseParser::default();
        let events = p.push(b": connected\n\ndata: {\"type\":\"init\"}\n\n: ping\n\n");
        assert_eq!(
            events,
            vec![
                SseEvent::Comment(" connected".into()),
                SseEvent::Data("{\"type\":\"init\"}".into()),
                SseEvent::Comment(" ping".into()),
            ]
        );
    }

    #[test]
    fn handles_frames_split_across_chunks() {
        let mut p = SseParser::default();
        assert!(p.push(b"data: hel").is_empty());
        assert!(p.push(b"lo\n").is_empty());
        assert_eq!(p.push(b"\ndata: x"), vec![SseEvent::Data("hello".into())]);
        assert_eq!(p.push(b"\n\n"), vec![SseEvent::Data("x".into())]);
    }

    #[test]
    fn handles_crlf_and_multiline_data() {
        let mut p = SseParser::default();
        let events = p.push(b"data: a\r\ndata:b\r\nid: 1\r\n\r\ndata: c\n\r\n");
        assert_eq!(
            events,
            vec![SseEvent::Data("a\nb".into()), SseEvent::Data("c".into())]
        );
    }

    #[test]
    fn handles_utf8_split_across_chunks() {
        let mut p = SseParser::default();
        let bytes = "data: 日本\n\n".as_bytes();
        let split = 8; // inside the first multi-byte character
        assert!(p.push(&bytes[..split]).is_empty());
        assert_eq!(p.push(&bytes[split..]), vec![SseEvent::Data("日本".into())]);
    }

    #[test]
    fn replaces_invalid_utf8() {
        let mut p = SseParser::default();
        assert_eq!(
            p.push(b"data: a\xffb\n\n"),
            vec![SseEvent::Data("a\u{FFFD}b".into())]
        );
    }
}
