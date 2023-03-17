use std::io::{self, BufRead, Write};

use regex::bytes::{Regex, RegexBuilder};

pub trait Pattern {
    /// Make replacements in the given input text and write the result to the provided buffer.
    ///
    /// Returns the total number of replacements that were made.
    ///
    /// Args:
    ///   * `buf`: the string with replacements applied will be saved into this buffer. This method
    ///     will only append to `buf` and will not clear it.
    ///   * `text`: the input text (byte string)
    ///   * `rep`: the replacement to make
    ///   * `all`: if false, replace only the first occurrence
    fn replace_into(&self, buf: &mut Vec<u8>, text: &[u8], rep: &[u8], all: bool) -> usize;
}

impl Pattern for Regex {
    fn replace_into(&self, buf: &mut Vec<u8>, text: &[u8], mut rep: &[u8], all: bool) -> usize {
        // use the regex Replacer trait locally so it doesn't conflict with our own Replacer
        // struct. Also the rep argument must be mut to work with Replacer, but it can still be
        // a shared slice.
        // This implementation is derived from Regex::bytes::Regex::replacen()
        use regex::bytes::Replacer;

        if let Some(rep) = rep.no_expansion() {
            let mut it = self.find_iter(text).peekable();
            if it.peek().is_none() {
                buf.extend_from_slice(text);
                return 0;
            }
            let mut last = 0;
            let mut count = 0;
            for m in it {
                count += 1;
                buf.extend_from_slice(&text[last..m.start()]);
                buf.extend_from_slice(&rep);
                last = m.end();
                if !all {
                    break;
                }
            }
            buf.extend_from_slice(&text[last..]);
            return count;
        }

        // The slower path, which we use if the replacement needs access to capture groups.
        let mut it = self.captures_iter(text).peekable();
        if it.peek().is_none() {
            buf.extend_from_slice(text);
            return 0;
        }
        let mut last_match = 0;
        let mut count = 0;
        for cap in it {
            count += 1;
            // unwrap on 0 is OK because captures only reports matches
            let m = cap.get(0).unwrap();
            buf.extend_from_slice(&text[last_match..m.start()]);
            rep.replace_append(&cap, buf);
            last_match = m.end();
            if !all {
                break;
            }
        }
        buf.extend_from_slice(&text[last_match..]);
        count
    }
}

impl Pattern for &[u8] {
    fn replace_into(&self, buf: &mut Vec<u8>, text: &[u8], rep: &[u8], all: bool) -> usize {
        let mut last = 0;
        let mut count = 0;
        for start in memchr::memmem::find_iter(text, &self) {
            count += 1;
            buf.extend_from_slice(&text[last..start]);
            buf.extend_from_slice(rep);
            last = start + self.len();
            if !all {
                break;
            }
        }
        buf.extend_from_slice(&text[last..]);
        count
    }
}

// can't be generic over AsRef<[u8]> so hard-code an impl for Vec
impl Pattern for Vec<u8> {
    fn replace_into(&self, buf: &mut Vec<u8>, text: &[u8], rep: &[u8], all: bool) -> usize {
        (&**self).replace_into(buf, text, rep, all)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StreamIOError {
    #[error("read error: {0}")]
    Read(#[source] io::Error),
    #[error("write error: {0}")]
    Write(#[source] io::Error),
}

#[derive(Debug, Clone)]
pub struct Replacer<P> {
    pattern: P,
    replacement: Vec<u8>,
    replace_all: bool,
    print_only_matches: bool,
}

// Weird () trait here for constructors that return concrete types. Replacer must have a type
// parameter, but this impl block can't be generic without causing confusion and unnecessary type
// annotations for callers.
impl Replacer<()> {
    pub fn regex<R>(re: &str, replacement: R) -> Result<Replacer<Regex>, regex::Error>
    where
        R: Into<Vec<u8>>,
    {
        Ok(Replacer {
            pattern: RegexBuilder::new(re).multi_line(true).build()?,
            replacement: replacement.into(),
            replace_all: false,
            print_only_matches: false,
        })
    }

    pub fn literal<P, R>(pattern: P, replacement: R) -> Replacer<Vec<u8>>
    where
        P: Into<Vec<u8>>,
        R: Into<Vec<u8>>,
    {
        Replacer {
            pattern: pattern.into(),
            replacement: replacement.into(),
            replace_all: false,
            print_only_matches: false,
        }
    }
}

// builder methods are actually generic
impl<P> Replacer<P> {
    pub fn replace_all(self, replace_all: bool) -> Self {
        Self {
            replace_all,
            ..self
        }
    }

    pub fn print_only_matches(self, print_only_matches: bool) -> Self {
        Self {
            print_only_matches,
            ..self
        }
    }
}

// and pattern related methods are generic over Patterns only
impl<P: Pattern> Replacer<P> {
    #[allow(unused)]
    pub fn new<R>(pattern: P, replacement: R) -> Replacer<P>
    where
        R: Into<Vec<u8>>,
    {
        Replacer {
            pattern,
            replacement: replacement.into(),
            replace_all: false,
            print_only_matches: false,
        }
    }

    pub fn replace_stream<R, W>(&self, input: &mut R, output: &mut W) -> Result<(), StreamIOError>
    where
        R: BufRead,
        W: Write,
    {
        let mut buf = vec![];
        let mut repbuf = vec![];
        loop {
            // read some input
            buf.clear();
            input
                .read_until(b'\n', &mut buf)
                .map_err(StreamIOError::Read)?;
            if buf.is_empty() {
                break;
            }

            // do the replacement
            repbuf.clear();
            let rep_count =
                self.pattern
                    .replace_into(&mut repbuf, &buf, &self.replacement, self.replace_all);

            // write the output (maybe)
            if !self.print_only_matches || rep_count != 0 {
                output.write_all(&repbuf).map_err(StreamIOError::Write)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::bytes::Regex;

    #[test]
    fn test_regex_replace_into() {
        let re = Regex::new(r"(\w+),\s*(\w+)").unwrap();
        let mut buf = vec![];
        let tests = [
            ("Wild, Allen", "$2 $1", false, 1, "Allen Wild"),
            ("foobar", "$2 $1", false, 0, "foobar"),
            (
                "Last, First. Last2, First2.",
                "$2 $1",
                false,
                1,
                "First Last. Last2, First2.",
            ),
            (
                "Last, First. Last2, First2.",
                "$2 $1",
                true,
                2,
                "First Last. First2 Last2.",
            ),
            ("", "asdf", false, 0, ""),
            ("", "asdf", true, 0, ""),
        ];

        for (text, rep, all, excount, expected) in tests {
            buf.clear();
            let count = re.replace_into(&mut buf, text.as_bytes(), rep.as_bytes(), all);
            assert_eq!(count, excount);
            assert_eq!(&buf, expected.as_bytes());
        }
    }

    #[test]
    fn test_literal_replace_into() {
        let pat = b"foo";
        let mut buf = vec![];
        let tests = [
            ("foobar", "FOO", false, 1, "FOObar"),
            ("what foo bar foo", "FOO", false, 1, "what FOO bar foo"),
            ("what foo bar foo", "FOO", true, 2, "what FOO bar FOO"),
            ("asdf", "", true, 0, "asdf"),
            ("", "asdf", false, 0, ""),
            ("", "asdf", true, 0, ""),
        ];

        for (text, rep, all, excount, expected) in tests {
            buf.clear();
            let count = pat
                .as_slice()
                .replace_into(&mut buf, text.as_bytes(), rep.as_bytes(), all);
            assert_eq!(count, excount);
            assert_eq!(&buf, expected.as_bytes());
        }
    }
}
