//! Streaming protection against pathological XML tokens.
//!
//! `quick_xml` streams documents, but it must still accumulate one complete
//! event before returning it. A single unterminated text node, tag, comment,
//! CDATA section, processing instruction, or DOCTYPE can therefore make its
//! event buffer grow with the input. This reader enforces a raw byte limit per
//! lexical token before bytes reach that event buffer.

use std::io::{self, Read};

/// A streaming reader that rejects an XML lexical token larger than the
/// configured raw-byte limit.
///
/// The limit includes delimiters (`<`, `>`, `<!--`, and so on). Text between
/// markup delimiters is one token. Entity references are additionally counted
/// as nested tokens, so an unterminated reference is bounded even when the
/// surrounding text is otherwise short.
///
/// Once the limit is exceeded, the reader is permanently failed and returns an
/// [`io::ErrorKind::InvalidData`] error on the current or next read. The reader
/// does not allocate in proportion to the input or token size.
pub(crate) struct XmlTokenLimitedReader<R> {
    inner: R,
    scanner: XmlTokenScanner,
    failure: Option<TokenKind>,
}

impl<R> XmlTokenLimitedReader<R> {
    pub(crate) fn new(inner: R, max_token_bytes: usize) -> Self {
        Self {
            inner,
            scanner: XmlTokenScanner::new(max_token_bytes),
            failure: None,
        }
    }

    #[cfg(test)]
    fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for XmlTokenLimitedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(kind) = self.failure {
            return Err(limit_error(kind, self.scanner.max_token_bytes));
        }

        let read = self.inner.read(buffer)?;
        if let Err((index, kind)) = self.scanner.consume_slice(&buffer[..read]) {
            self.failure = Some(kind);

            // Bytes preceding the first offending byte are valid. Report
            // them now, then surface the terminal error on the next read.
            // If the offending byte is first, report the error immediately.
            return if index == 0 {
                Err(limit_error(kind, self.scanner.max_token_bytes))
            } else {
                Ok(index)
            };
        }

        Ok(read)
    }
}

#[inline]
fn find_byte(input: &[u8], first: u8) -> Option<usize> {
    input.iter().position(|byte| *byte == first)
}

#[inline]
fn find_byte2(input: &[u8], first: u8, second: u8) -> Option<usize> {
    input
        .iter()
        .position(|byte| *byte == first || *byte == second)
}

#[inline]
fn find_byte3(input: &[u8], first: u8, second: u8, third: u8) -> Option<usize> {
    input
        .iter()
        .position(|byte| *byte == first || *byte == second || *byte == third)
}

fn limit_error(kind: TokenKind, max_token_bytes: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "XML {} token exceeds the configured limit of {max_token_bytes} bytes",
            kind.description()
        ),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenKind {
    Text,
    Entity,
    Tag,
    Comment,
    Cdata,
    ProcessingInstruction,
    Doctype,
}

impl TokenKind {
    fn description(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Entity => "entity-reference",
            Self::Tag => "tag",
            Self::Comment => "comment",
            Self::Cdata => "CDATA",
            Self::ProcessingInstruction => "processing-instruction",
            Self::Doctype => "DOCTYPE",
        }
    }
}

struct XmlTokenScanner {
    max_token_bytes: usize,
    state: LexicalState,
}

impl XmlTokenScanner {
    fn new(max_token_bytes: usize) -> Self {
        Self {
            max_token_bytes,
            state: LexicalState::Text {
                len: 0,
                entity_len: None,
            },
        }
    }

    fn consume(&mut self, byte: u8) -> Result<(), TokenKind> {
        let max = self.max_token_bytes;
        let mut replacement = None;

        match &mut self.state {
            LexicalState::Text { len, entity_len } => {
                if byte == b'<' {
                    Self::start_markup(max, &mut replacement)?;
                } else {
                    if let Some(current_entity_len) = entity_len.as_mut() {
                        Self::bump(current_entity_len, max, TokenKind::Entity)?;
                    }
                    Self::bump(len, max, TokenKind::Text)?;

                    match entity_len {
                        Some(_) if byte == b';' => *entity_len = None,
                        Some(_) => {}
                        None if byte == b'&' => *entity_len = Some(1),
                        None => {}
                    }
                }
            }
            LexicalState::PendingMarkup {
                len,
                prefix,
                prefix_len,
            } => {
                Self::bump(len, max, TokenKind::Tag)?;
                prefix[*prefix_len] = byte;
                *prefix_len += 1;
                let candidate = &prefix[..*prefix_len];

                replacement = if candidate == PROCESSING_INSTRUCTION_PREFIX {
                    Some(LexicalState::ProcessingInstruction {
                        len: *len,
                        previous_question: false,
                    })
                } else if candidate == COMMENT_PREFIX {
                    Some(LexicalState::Comment {
                        len: *len,
                        trailing_dashes: 0,
                    })
                } else if candidate == CDATA_PREFIX {
                    Some(LexicalState::Cdata {
                        len: *len,
                        trailing_brackets: 0,
                    })
                } else if candidate == DOCTYPE_PREFIX {
                    Some(LexicalState::Doctype {
                        len: *len,
                        quote: None,
                        subset_depth: 0,
                        mode: DoctypeMode::Normal,
                        open_prefix: 0,
                    })
                } else if is_special_markup_prefix(candidate) {
                    None
                } else {
                    Some(generic_markup_state(*len, candidate))
                };
            }
            LexicalState::Tag { len, quote } => {
                Self::bump(len, max, TokenKind::Tag)?;
                match *quote {
                    Some(delimiter) if byte == delimiter => *quote = None,
                    Some(_) => {}
                    None if matches!(byte, b'\'' | b'"') => *quote = Some(byte),
                    None if byte == b'>' => replacement = Some(LexicalState::empty_text()),
                    None => {}
                }
            }
            LexicalState::Comment {
                len,
                trailing_dashes,
            } => {
                Self::bump(len, max, TokenKind::Comment)?;
                if byte == b'>' && *trailing_dashes >= 2 {
                    replacement = Some(LexicalState::empty_text());
                } else if byte == b'-' {
                    *trailing_dashes = trailing_dashes.saturating_add(1).min(2);
                } else {
                    *trailing_dashes = 0;
                }
            }
            LexicalState::Cdata {
                len,
                trailing_brackets,
            } => {
                Self::bump(len, max, TokenKind::Cdata)?;
                if byte == b'>' && *trailing_brackets >= 2 {
                    replacement = Some(LexicalState::empty_text());
                } else if byte == b']' {
                    *trailing_brackets = trailing_brackets.saturating_add(1).min(2);
                } else {
                    *trailing_brackets = 0;
                }
            }
            LexicalState::ProcessingInstruction {
                len,
                previous_question,
            } => {
                Self::bump(len, max, TokenKind::ProcessingInstruction)?;
                if byte == b'>' && *previous_question {
                    replacement = Some(LexicalState::empty_text());
                } else {
                    *previous_question = byte == b'?';
                }
            }
            LexicalState::Doctype {
                len,
                quote,
                subset_depth,
                mode,
                open_prefix,
            } => {
                Self::bump(len, max, TokenKind::Doctype)?;

                match mode {
                    DoctypeMode::Comment { trailing_dashes } => {
                        if byte == b'>' && *trailing_dashes >= 2 {
                            *mode = DoctypeMode::Normal;
                            *open_prefix = 0;
                        } else if byte == b'-' {
                            *trailing_dashes = trailing_dashes.saturating_add(1).min(2);
                        } else {
                            *trailing_dashes = 0;
                        }
                    }
                    DoctypeMode::ProcessingInstruction { previous_question } => {
                        if byte == b'>' && *previous_question {
                            *mode = DoctypeMode::Normal;
                            *open_prefix = 0;
                        } else {
                            *previous_question = byte == b'?';
                        }
                    }
                    DoctypeMode::Normal => match *quote {
                        Some(delimiter) if byte == delimiter => {
                            *quote = None;
                            *open_prefix = 0;
                        }
                        Some(_) => {}
                        None => {
                            if *open_prefix == 1 && byte == b'?' {
                                *mode = DoctypeMode::ProcessingInstruction {
                                    previous_question: false,
                                };
                                *open_prefix = 0;
                            } else if *open_prefix == 3 && byte == b'-' {
                                *mode = DoctypeMode::Comment { trailing_dashes: 0 };
                                *open_prefix = 0;
                            } else {
                                match byte {
                                    b'\'' | b'"' => {
                                        *quote = Some(byte);
                                        *open_prefix = 0;
                                    }
                                    b'[' => {
                                        *subset_depth = subset_depth.saturating_add(1);
                                        *open_prefix = 0;
                                    }
                                    b']' => {
                                        *subset_depth = subset_depth.saturating_sub(1);
                                        *open_prefix = 0;
                                    }
                                    b'>' if *subset_depth == 0 => {
                                        replacement = Some(LexicalState::empty_text());
                                    }
                                    _ => {
                                        *open_prefix = match (*open_prefix, byte) {
                                            (_, b'<') => 1,
                                            (1, b'!') => 2,
                                            (2, b'-') => 3,
                                            _ => 0,
                                        };
                                    }
                                }
                            }
                        }
                    },
                }
            }
        }

        if let Some(replacement) = replacement {
            self.state = replacement;
        }
        Ok(())
    }

    /// Consume an input block while bulk-counting bytes that cannot change the
    /// current lexical state. Large 3MF mesh XML otherwise pays the full state
    /// machine dispatch for every byte, even though almost all bytes are plain
    /// attribute or text content. Delimiter bytes still go through `consume`,
    /// so token boundaries and the exact first over-limit byte are unchanged.
    fn consume_slice(&mut self, input: &[u8]) -> Result<(), (usize, TokenKind)> {
        let mut offset = 0;
        while offset < input.len() {
            let remaining = &input[offset..];
            let plain_len = match &self.state {
                LexicalState::Text {
                    entity_len: None, ..
                } => find_byte2(remaining, b'<', b'&').unwrap_or(remaining.len()),
                LexicalState::Text {
                    entity_len: Some(_),
                    ..
                } => find_byte2(remaining, b'<', b';').unwrap_or(remaining.len()),
                LexicalState::Tag { quote: None, .. } => {
                    find_byte3(remaining, b'\'', b'"', b'>').unwrap_or(remaining.len())
                }
                LexicalState::Tag {
                    quote: Some(delimiter),
                    ..
                } => find_byte(remaining, *delimiter).unwrap_or(remaining.len()),
                LexicalState::Comment { .. } => {
                    find_byte2(remaining, b'-', b'>').unwrap_or(remaining.len())
                }
                LexicalState::Cdata { .. } => {
                    find_byte2(remaining, b']', b'>').unwrap_or(remaining.len())
                }
                LexicalState::ProcessingInstruction { .. } => {
                    find_byte2(remaining, b'?', b'>').unwrap_or(remaining.len())
                }
                LexicalState::PendingMarkup { .. } | LexicalState::Doctype { .. } => 0,
            };

            if plain_len > 0 {
                self.consume_plain_bytes(plain_len)
                    .map_err(|(relative, kind)| (offset + relative, kind))?;
                offset += plain_len;
            }
            if offset < input.len() {
                self.consume(input[offset]).map_err(|kind| (offset, kind))?;
                offset += 1;
            }
        }
        Ok(())
    }

    fn consume_plain_bytes(&mut self, count: usize) -> Result<(), (usize, TokenKind)> {
        let max = self.max_token_bytes;
        match &mut self.state {
            LexicalState::Text { len, entity_len } => {
                if let Some(entity_len) = entity_len {
                    let text_available = max.saturating_sub(*len);
                    let entity_available = max.saturating_sub(*entity_len);
                    let available = text_available.min(entity_available);
                    if count > available {
                        let kind = if entity_available <= text_available {
                            TokenKind::Entity
                        } else {
                            TokenKind::Text
                        };
                        return Err((available, kind));
                    }
                    *entity_len += count;
                } else {
                    Self::bump_plain(len, count, max, TokenKind::Text)?;
                    return Ok(());
                }
                *len += count;
            }
            LexicalState::Tag { len, .. } => {
                Self::bump_plain(len, count, max, TokenKind::Tag)?;
            }
            LexicalState::Comment { len, .. } => {
                Self::bump_plain(len, count, max, TokenKind::Comment)?;
            }
            LexicalState::Cdata { len, .. } => {
                Self::bump_plain(len, count, max, TokenKind::Cdata)?;
            }
            LexicalState::ProcessingInstruction { len, .. } => {
                Self::bump_plain(len, count, max, TokenKind::ProcessingInstruction)?;
            }
            LexicalState::PendingMarkup { .. } | LexicalState::Doctype { .. } => {
                debug_assert_eq!(count, 0);
            }
        }
        Ok(())
    }

    fn bump_plain(
        len: &mut usize,
        count: usize,
        max: usize,
        kind: TokenKind,
    ) -> Result<(), (usize, TokenKind)> {
        let available = max.saturating_sub(*len);
        if count > available {
            return Err((available, kind));
        }
        *len += count;
        Ok(())
    }

    fn start_markup(max: usize, replacement: &mut Option<LexicalState>) -> Result<(), TokenKind> {
        let mut prefix = [0; MAX_SPECIAL_PREFIX_LEN];
        prefix[0] = b'<';
        let mut len = 0;
        Self::bump(&mut len, max, TokenKind::Tag)?;
        *replacement = Some(LexicalState::PendingMarkup {
            len,
            prefix,
            prefix_len: 1,
        });
        Ok(())
    }

    fn bump(len: &mut usize, max: usize, kind: TokenKind) -> Result<(), TokenKind> {
        if *len >= max {
            return Err(kind);
        }
        *len += 1;
        Ok(())
    }
}

const PROCESSING_INSTRUCTION_PREFIX: &[u8] = b"<?";
const COMMENT_PREFIX: &[u8] = b"<!--";
const CDATA_PREFIX: &[u8] = b"<![CDATA[";
const DOCTYPE_PREFIX: &[u8] = b"<!DOCTYPE";
const MAX_SPECIAL_PREFIX_LEN: usize = DOCTYPE_PREFIX.len();

fn is_special_markup_prefix(candidate: &[u8]) -> bool {
    [
        PROCESSING_INSTRUCTION_PREFIX,
        COMMENT_PREFIX,
        CDATA_PREFIX,
        DOCTYPE_PREFIX,
    ]
    .into_iter()
    .any(|prefix| prefix.starts_with(candidate))
}

fn generic_markup_state(len: usize, prefix: &[u8]) -> LexicalState {
    let mut quote = None;
    for byte in prefix.iter().copied().skip(1) {
        match quote {
            Some(delimiter) if byte == delimiter => quote = None,
            Some(_) => {}
            None if matches!(byte, b'\'' | b'"') => quote = Some(byte),
            None if byte == b'>' => return LexicalState::empty_text(),
            None => {}
        }
    }
    LexicalState::Tag { len, quote }
}

enum LexicalState {
    Text {
        len: usize,
        entity_len: Option<usize>,
    },
    PendingMarkup {
        len: usize,
        prefix: [u8; MAX_SPECIAL_PREFIX_LEN],
        prefix_len: usize,
    },
    Tag {
        len: usize,
        quote: Option<u8>,
    },
    Comment {
        len: usize,
        trailing_dashes: u8,
    },
    Cdata {
        len: usize,
        trailing_brackets: u8,
    },
    ProcessingInstruction {
        len: usize,
        previous_question: bool,
    },
    Doctype {
        len: usize,
        quote: Option<u8>,
        subset_depth: usize,
        mode: DoctypeMode,
        open_prefix: u8,
    },
}

impl LexicalState {
    fn empty_text() -> Self {
        Self::Text {
            len: 0,
            entity_len: None,
        }
    }
}

enum DoctypeMode {
    Normal,
    Comment { trailing_dashes: u8 },
    ProcessingInstruction { previous_question: bool },
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor, Read};

    use quick_xml::{Reader, events::Event};

    use super::XmlTokenLimitedReader;

    fn read_in_chunks(
        input: &[u8],
        max_token_bytes: usize,
        chunk_size: usize,
    ) -> std::io::Result<Vec<u8>> {
        let mut reader = XmlTokenLimitedReader::new(Cursor::new(input), max_token_bytes);
        let mut output = Vec::new();
        let mut buffer = vec![0; chunk_size];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => return Ok(output),
                Ok(read) => output.extend_from_slice(&buffer[..read]),
                Err(error) => return Err(error),
            }
        }
    }

    fn quick_xml_error(input: &[u8], max_token_bytes: usize) -> quick_xml::Error {
        let limited = XmlTokenLimitedReader::new(Cursor::new(input), max_token_bytes);
        let mut reader = Reader::from_reader(BufReader::with_capacity(3, limited));
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Eof) => panic!("expected the XML token limit to fail"),
                Ok(_) => buffer.clear(),
                Err(error) => return error,
            }
        }
    }

    fn assert_invalid_data(input: &[u8], max_token_bytes: usize) {
        let error = quick_xml_error(input, max_token_bytes);
        match error {
            quick_xml::Error::Io(error) => {
                assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
                assert!(error.to_string().contains("configured limit"));
            }
            other => panic!("expected an I/O limit error, got {other:?}"),
        }
    }

    #[test]
    fn permits_many_tokens_when_each_is_within_the_limit() {
        let input = b"<r><a>12345</a><b x='>y'>67890</b></r>";
        let output = read_in_chunks(input, 10, 2).expect("bounded XML should pass");
        assert_eq!(output, input);
    }

    #[test]
    fn bulk_and_single_byte_scans_enforce_identical_boundaries() {
        let input = b"<root a='quoted > value' b=\"second\">text&amp;more<!--ok--><![CDATA[data]]><?pi ok?></root>";
        for max_token_bytes in [8, 24, 64, input.len()] {
            let normalize = |result: std::io::Result<Vec<u8>>| {
                result.map_err(|error| (error.kind(), error.to_string()))
            };
            assert_eq!(
                normalize(read_in_chunks(input, max_token_bytes, 1)),
                normalize(read_in_chunks(input, max_token_bytes, input.len())),
            );
        }
    }

    #[test]
    fn permits_a_token_exactly_at_the_limit() {
        let input = b"<r>12345</r>";
        assert_eq!(read_in_chunks(input, 5, 1).unwrap(), input);
    }

    #[test]
    fn rejects_oversized_text() {
        assert_invalid_data(b"<r>123456</r>", 5);
    }

    #[test]
    fn rejects_oversized_tag_and_does_not_treat_quoted_close_as_a_boundary() {
        assert_invalid_data(b"<root attribute='>>>>>>>>>>>>>>>>'>x</root>", 16);
    }

    #[test]
    fn rejects_oversized_comment_when_delimiters_cross_read_boundaries() {
        assert_invalid_data(b"<r><!--0123456789--></r>", 12);
    }

    #[test]
    fn rejects_oversized_cdata_when_delimiters_cross_read_boundaries() {
        assert_invalid_data(b"<r><![CDATA[0123456789]]></r>", 16);
    }

    #[test]
    fn rejects_oversized_processing_instruction() {
        assert_invalid_data(b"<?target 0123456789?><r/>", 12);
    }

    #[test]
    fn rejects_oversized_doctype_as_one_token_despite_internal_markup() {
        let input = b"<!DOCTYPE r [<!-- [ ignored ] --><?inside [ignored]?>\
                      <!ELEMENT r (#PCDATA)><!ENTITY x '0123456789'>]><r/>";
        assert_invalid_data(input, 64);
    }

    #[test]
    fn accepts_a_bounded_doctype_with_quotes_comments_and_processing_instructions() {
        let input = b"<!DOCTYPE r [<!-- [ ignored ] --><?inside [ignored]?>\
                      <!ELEMENT r (#PCDATA)><!ENTITY x 'value > [ ]'>]><r>x</r>";
        assert_eq!(read_in_chunks(input, input.len(), 1).unwrap(), input);
    }

    #[test]
    fn rejects_an_unterminated_entity_reference() {
        let error = quick_xml_error(b"<r>&01234567890123456789</r>", 12);
        match error {
            quick_xml::Error::Io(error) => {
                assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
                assert!(error.to_string().contains("entity-reference"));
            }
            other => panic!("expected an I/O limit error, got {other:?}"),
        }
    }

    #[test]
    fn zero_limit_fails_before_delivering_the_first_byte() {
        let error = read_in_chunks(b"<r/>", 0, 8).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn delivers_only_the_valid_prefix_then_remains_failed() {
        let mut reader = XmlTokenLimitedReader::new(Cursor::new(b"abcd"), 3);
        let mut buffer = [0; 16];

        assert_eq!(reader.read(&mut buffer).unwrap(), 3);
        assert_eq!(&buffer[..3], b"abc");
        let first_error = reader.read(&mut buffer).unwrap_err();
        let second_error = reader.read(&mut buffer).unwrap_err();
        assert_eq!(first_error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(second_error.kind(), std::io::ErrorKind::InvalidData);

        // The wrapper never retains or allocates the unread suffix.
        assert_eq!(reader.into_inner().position(), 4);
    }
}
