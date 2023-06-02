use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use tempfile::NamedTempFile;

mod replace;
use replace::{Pattern, ReplaceOptions, Replacer};
mod unescape;
use unescape::unescape_bytes;

/// rp: A line-oriented stream replacer
#[derive(Debug, Parser)]
struct Args {
    /// Modify files in-place rather than printing to stdout
    #[arg(short, long, requires = "files")]
    in_place: bool,

    /// PATTERN and REPLACEMENT are literal strings, not regular expressions.
    #[arg(short = 'F', long)]
    fixed_strings: bool,

    /// Case-insensitive search (regex mode only).
    #[arg(short = 'I', long, conflicts_with = "fixed_strings")]
    ignore_case: bool,

    /// Enable escape-sequence interpretation in REPLACEMENT.
    ///
    /// We support the same set of escape sequences as Rust string literals. Additionally non-ASCII
    /// hex escapes (e.g. \xFF) are supported. This works in regex or literal pattern mode.
    ///
    /// \\, \n, \r, \t - backslash, newline, carraige-return, and tab
    /// \xHH - exactly two hex digits (uppercase or lowercase)
    /// \u{UUUU} - between 1 and 6 hex digits of a unicode codepoint, will be encoded as UTF-8
    #[arg(short, long, verbatim_doc_comment)]
    escape: bool,

    /// Replace all occurrences on each line rather than just the first match.
    #[arg(short = 'g', long)]
    replace_all: bool,

    /// Print only matching lines where at least one replacement occurred.
    #[arg(short = 'n', long)]
    only_matches: bool,

    /// The pattern (regex or literal string) to search for
    pattern: String,

    /// The replacement text.
    ///
    /// In regex mode, capture groups are specified using '$', e.g. $0 for the full match, $1 for
    /// the first group, or $name for a named capture group. Curly braces like ${1} or ${name} can
    /// also be used. Use $$ for a literal dollar sign.
    replacement: String,

    /// List of input files. Omit or use '-' for stdin.
    files: Vec<PathBuf>,
}

fn do_replace_stdout<P: Pattern>(replacer: Replacer<P>, files: &[PathBuf]) -> anyhow::Result<()> {
    let mut failed = false;
    for path in files {
        let ret = if let Some("-") = path.to_str() {
            // reading from stdin
            replacer.replace_stream(&mut io::stdin().lock(), &mut io::stdout().lock())
        } else {
            let mut file = BufReader::new(
                File::open(path).with_context(|| format!("unable to open '{}'", path.display()))?,
            );
            replacer.replace_stream(&mut file, &mut io::stdout().lock())
        };

        if let Err(err) = ret {
            eprintln!("Error on '{}': {}", path.display(), err);
            failed = true;
        }
    }

    if failed {
        Err(anyhow::anyhow!("failed processing one or more files"))
    } else {
        Ok(())
    }
}

fn replace_one_inplace<P: Pattern>(replacer: &Replacer<P>, path: &Path) -> anyhow::Result<()> {
    // open input first to make sure that the file exists
    let infile = File::open(path).context("failed to open")?;
    let dir = match path.parent() {
        Some(dir) => {
            if dir.as_os_str().is_empty() {
                Path::new(".")
            } else if !dir.is_dir() {
                // this shouldn't actually happen
                anyhow::bail!("parent '{}' isn't a directory", dir.display())
            } else {
                dir
            }
        }
        None => anyhow::bail!("unable to get parent directory"),
    };

    // get input metadata, we'll need its permissions later
    let infile_meta = infile.metadata().context("failed to get file metadata")?;
    // now we can buffer the input
    let mut infile = BufReader::new(infile);

    let mut outfile =
        BufWriter::new(NamedTempFile::new_in(dir).context("failed to open temporary output file")?);
    replacer.replace_stream(&mut infile, &mut outfile)?;

    // Close the input first before we rename over it
    drop(infile);

    // get the tempfile out of the BufWriter, this will flush the remaining buffer
    let outfile = outfile.into_inner().context("write error")?;
    // atomically rename to replace the file
    let new_outfile = outfile
        .persist(path)
        .context("failed to save updated file")?;

    // set the same permissions as the input
    new_outfile
        .set_permissions(infile_meta.permissions())
        .context("failed to set permissions on udpated file")?;

    Ok(())
}

fn do_replace_inplace<P: Pattern>(replacer: Replacer<P>, files: &[PathBuf]) -> anyhow::Result<()> {
    for file in files {
        replace_one_inplace(&replacer, file).with_context(|| file.display().to_string())?;
    }
    Ok(())
}

fn run() -> anyhow::Result<()> {
    let args = Args::parse();

    let files = if args.files.is_empty() {
        vec![PathBuf::from("-")]
    } else {
        args.files
    };

    // Quick check that stdin isn't specified twice in the files list. We can't be completely sure
    // because of /proc/self/fd/0 or /dev/stdout or other such paths.
    let stdin_arg_count = files
        .iter()
        .filter(|p| matches!(p.to_str(), Some("-")))
        .count();

    if stdin_arg_count > 1 {
        anyhow::bail!("stdin '-' argument specified more than once");
    } else if args.in_place && stdin_arg_count > 0 {
        anyhow::bail!("stdin can't be used with in-place replacement");
    }

    let opts = ReplaceOptions {
        replace_all: args.replace_all,
        only_matches: args.only_matches,
    };

    let replacement = if args.escape {
        unescape_bytes(args.replacement.as_bytes())?
    } else {
        args.replacement.into_bytes()
    };

    if args.fixed_strings {
        let replacer = opts.build_literal(args.pattern, replacement);
        if args.in_place {
            do_replace_inplace(replacer, &files)
        } else {
            do_replace_stdout(replacer, &files)
        }
    } else {
        let replacer = opts
            .build_regex(&args.pattern, replacement, args.ignore_case)
            .context("invalid pattern regex")?;
        if args.in_place {
            do_replace_inplace(replacer, &files)
        } else {
            do_replace_stdout(replacer, &files)
        }
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("Error: {err:#}");
        std::process::exit(1);
    }
}
