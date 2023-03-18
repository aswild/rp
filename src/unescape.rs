#[derive(Debug, thiserror::Error)]
#[error("invalid escape sequence at position {pos}: {kind}")]
pub struct Error {
    pos: usize,
    kind: ErrorKind,
}

#[derive(Debug, thiserror::Error)]
enum ErrorKind {
    #[error("un-paired backslash escape")]
    MissingEscape,

    #[error("invalid escape character '{0}'")]
    InvalidEscape(char),

    #[error("truncated hex escape")]
    TruncatedHex,

    #[error("invalid hex digit '{0}'")]
    InvalidHexDigit(char),

    #[error("truncated unicode escape")]
    TruncatedUnicode,

    #[error("invalid unicode codepoint '{0:X}'")]
    InvalidUnicodeCodepoint(u32),
}

impl ErrorKind {
    fn at(self, pos: usize) -> Error {
        Error { pos, kind: self }
    }
}

/// Expand escape sequences in a string, producing a byte array.
///
/// Supports the same set of escapes as Rust string literals[1], but additionally hex escapes for
/// non-ascii bytes (such as "\xFF") are allowed. The resulting byte array might not be valid
/// UTF-8.
///
/// [1] https://doc.rust-lang.org/reference/tokens.html#character-escapes
#[inline]
pub fn unescape_bytes(bytes: impl AsRef<[u8]>) -> Result<Vec<u8>, Error> {
    unescape_bytes_(bytes.as_ref())
}

#[inline(never)]
fn unescape_bytes_(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut last = 0;

    while let Some(mut pos) = memchr::memchr(b'\\', &bytes[last..]) {
        // mechr gives us the position relative to where we started (last), advance pos to index
        // directly into the input
        pos += last;
        out.extend_from_slice(&bytes[last..pos]);

        let esc = *bytes.get(pos + 1).ok_or(ErrorKind::MissingEscape.at(pos))?;
        match esc {
            b'0' => out.push(b'\0'),
            b'\\' => out.push(b'\\'),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'x' => {
                let b = parse_hex(&bytes[(pos + 2)..]).map_err(|e| e.at(pos))?;
                out.push(b);
                pos += 2; // we consumed 2 extra bytes of input
            }
            b'u' => {
                let (i, count) = parse_unicode(&bytes[(pos + 2)..]).map_err(|e| e.at(pos))?;
                let c =
                    char::try_from(i).map_err(|_| ErrorKind::InvalidUnicodeCodepoint(i).at(pos))?;
                let mut cb = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut cb).as_bytes());
                pos += count; // consumed some extra bytes of input
            }
            _ => return Err(ErrorKind::InvalidEscape(esc.into()).at(pos)),
        }

        // consume the backslash and the escape type byte. Hex and Unicode escapes may also
        // have advanced pos to consume extra bytes
        last = pos + 2;
    }

    out.extend_from_slice(&bytes[last..]);
    Ok(out)
}

/// parse a single hex nibble from an ASCII byte
fn nibble(b: u8) -> Result<u8, ErrorKind> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        other => Err(ErrorKind::InvalidHexDigit(other.into())),
    }
}

/// parse exactly 2 hex digits from the start of bytes
fn parse_hex(bytes: impl AsRef<[u8]>) -> Result<u8, ErrorKind> {
    let bytes = bytes.as_ref();
    let hi = bytes
        .first()
        .copied()
        .ok_or(ErrorKind::TruncatedHex)
        .and_then(nibble)?;
    let lo = bytes
        .get(1)
        .copied()
        .ok_or(ErrorKind::TruncatedHex)
        .and_then(nibble)?;
    Ok((hi << 4) | lo)
}

/// Parse up to six hex digits surrounded by curly braces and collects them into a u32. Returns the
/// parsed u32 and the number of bytes used in the input (including the braces)
fn parse_unicode(bytes: impl AsRef<[u8]>) -> Result<(u32, usize), ErrorKind> {
    let bytes = bytes.as_ref();

    // we always must have at least 3 characters of input, a pair of curly braces and a digit
    if bytes.len() < 3 {
        return Err(ErrorKind::TruncatedUnicode);
    }
    // must start with '{'
    if bytes[0] != b'{' {
        return Err(ErrorKind::TruncatedUnicode);
    }
    // must find a '}' within 8 characters
    let end = bytes
        .iter()
        .take(8)
        .position(|b| *b == b'}')
        .ok_or(ErrorKind::TruncatedUnicode)?;

    let digits = &bytes[1..end];
    let mut val = 0;
    for digit in digits {
        val = (val << 4) | (nibble(*digit)? as u32);
    }

    Ok((val, digits.len() + 2))
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_nibble() {
        use super::nibble;

        assert_eq!(nibble(b'0').unwrap(), 0);
        assert_eq!(nibble(b'1').unwrap(), 1);
        assert_eq!(nibble(b'A').unwrap(), 10);
        assert_eq!(nibble(b'a').unwrap(), 10);
        assert_eq!(nibble(b'F').unwrap(), 15);
        assert_eq!(nibble(b'f').unwrap(), 15);

        assert!(nibble(b'-').is_err());
        assert!(nibble(b'\0').is_err());
        assert!(nibble(b'g').is_err());
        assert!(nibble(b'\n').is_err());
    }

    #[test]
    fn test_hex() {
        use super::parse_hex;

        assert_eq!(parse_hex("00").unwrap(), 0);
        assert_eq!(parse_hex("10").unwrap(), 0x10);
        assert_eq!(parse_hex("FE").unwrap(), 0xfe);
        assert_eq!(parse_hex("ed").unwrap(), 0xed);

        assert!(parse_hex("").is_err());
        assert!(parse_hex("0").is_err());
        assert!(parse_hex("x").is_err());
        assert!(parse_hex("0g").is_err());
    }

    #[test]
    fn test_unicode() {
        use super::parse_unicode;

        assert_eq!(parse_unicode("{0}").unwrap(), (0, 3));
        assert_eq!(parse_unicode("{9}").unwrap(), ('\t' as u32, 3));
        assert_eq!(parse_unicode("{0009}").unwrap(), ('\t' as u32, 6));
        assert_eq!(parse_unicode("{1b}").unwrap(), (0x1b, 4));
        assert_eq!(parse_unicode("{1F600}").unwrap(), ('😀' as u32, 7));
        // invalid codepoint, but still parsable
        assert_eq!(parse_unicode("{D800}").unwrap(), (0xD800, 6));

        assert!(parse_unicode("").is_err());
        assert!(parse_unicode("{}").is_err());
        assert!(parse_unicode("{0   ").is_err());
        assert!(parse_unicode("{123456789}").is_err());
        assert!(parse_unicode("{123X}").is_err());
    }

    #[test]
    fn test_unescape() {
        use super::unescape_bytes;

        #[track_caller]
        fn check(input: &str, expected: impl AsRef<[u8]>) {
            assert_eq!(unescape_bytes(input.as_bytes()).unwrap(), expected.as_ref());
        }

        check(r"hello\x20world", "hello world");
        check(r"hello\0world", "hello\0world");
        check(r"hello\xFFworld", b"hello\xffworld");
        check(r"hello\u{1F600}world", "hello😀world");
        check(r"hello\u{000020}world", "hello world");
        check(r"hello\u{20}world", "hello world");
        check(r"hello\x20world\n", "hello world\n");
        check(r"\n\n\n\xff\u{1234}", b"\n\n\n\xff\xe1\x88\xb4");

        assert!(unescape_bytes(r"hello\").is_err());
        assert!(unescape_bytes(r"hello\xgg").is_err());
        assert!(unescape_bytes(r"hello\x{ff}").is_err());
        assert!(unescape_bytes(r"\u{D800}").is_err()); // not a valid unicode codepoint
        assert!(unescape_bytes(r"bad escape \X10").is_err());
    }
}
