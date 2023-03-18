use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use tempfile::NamedTempFile;

mod replace;
use replace::{Pattern, ReplaceOptions, Replacer};

/// rp: A line-oriented stream replacer
#[derive(Debug, Parser)]
struct Args {
    /// Modify files in-place rather than printing to stdout
    #[arg(short, long, requires = "files")]
    in_place: bool,

    /// PATTERN and REPLACEMENT are literal strings, not regular expressions.
    #[arg(short = 'F', long)]
    fixed_strings: bool,

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
    let mut infile = BufReader::new(File::open(path).context("failed to open")?);
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

    let mut outfile =
        BufWriter::new(NamedTempFile::new_in(dir).context("failed to open output file")?);
    replacer.replace_stream(&mut infile, &mut outfile)?;
    // get the tempfile out of the BufWriter, this will flush the remaining buffer
    let outfile = outfile.into_inner().context("write error")?;
    // atomically rename to replace the file
    outfile
        .persist(path)
        .context("failed to save updated file")?;

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

    if args.fixed_strings {
        let replacer = opts.build_literal(args.pattern, args.replacement);
        if args.in_place {
            do_replace_inplace(replacer, &files)
        } else {
            do_replace_stdout(replacer, &files)
        }
    } else {
        let replacer = opts
            .build_regex(&args.pattern, args.replacement)
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
