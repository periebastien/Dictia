//! Parseur incrémental de Server-Sent Events (`text/event-stream`).
//!
//! Les fragments réseau coupent arbitrairement les lignes, voire les
//! caractères UTF-8 : on accumule des octets et on ne décode que des lignes
//! complètes.

#[derive(Debug, Default)]
pub struct SseParser {
    pending: Vec<u8>,
    data: Option<String>,
}

impl SseParser {
    /// Ajoute un fragment et renvoie les `data` des événements complets.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=pos).collect();
            self.handle_line(&line[..line.len() - 1], &mut events);
        }
        events
    }

    /// Fin du flux : traite une éventuelle dernière ligne sans saut de ligne.
    pub fn finish(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.handle_line(&line, &mut events);
        }
        events.extend(self.data.take());
        events
    }

    fn handle_line(&mut self, line: &[u8], events: &mut Vec<String>) {
        let line = String::from_utf8_lossy(line);
        let line = line.strip_suffix('\r').unwrap_or(&line);
        if line.is_empty() {
            events.extend(self.data.take());
            return;
        }
        if line.starts_with(':') {
            return; // commentaire / keep-alive
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        if field == "data" {
            match &mut self.data {
                Some(d) => {
                    d.push('\n');
                    d.push_str(value);
                }
                None => self.data = Some(value.to_owned()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut p = SseParser::default();
        assert!(p.push(b"data: {\"a\"").is_empty());
        assert!(p.push(b":1}\n").is_empty());
        assert_eq!(p.push(b"\ndata: x\n\n"), ["{\"a\":1}", "x"]);
        assert!(p.finish().is_empty());
    }

    #[test]
    fn handles_crlf_comments_and_other_fields() {
        let mut p = SseParser::default();
        let events = p.push(b": ping\r\nevent: delta\r\nid: 3\r\ndata:brut\r\n\r\n");
        assert_eq!(events, ["brut"]);
    }

    #[test]
    fn joins_multiline_data() {
        let mut p = SseParser::default();
        assert_eq!(p.push(b"data: a\ndata: b\n\n"), ["a\nb"]);
    }

    #[test]
    fn keeps_utf8_split_inside_a_character() {
        let bytes = "data: déjà\n\n".as_bytes();
        let split = bytes.iter().position(|&b| b == 0xC3).unwrap() + 1;
        let mut p = SseParser::default();
        assert!(p.push(&bytes[..split]).is_empty());
        assert_eq!(p.push(&bytes[split..]), ["déjà"]);
    }

    #[test]
    fn finish_flushes_unterminated_event() {
        let mut p = SseParser::default();
        assert!(p.push(b"data: [DONE]").is_empty());
        assert_eq!(p.finish(), ["[DONE]"]);
    }
}
