use clap::{Parser, ValueEnum};
use eyre::{eyre, Result, WrapErr};
use globset::Glob;
use rayon::prelude::*;
use size::{Base, Size, Style};
use std::ffi::OsString;
use std::fs::{canonicalize, read_dir, symlink_metadata, DirEntry, FileType, ReadDir};
use std::path::{Path, PathBuf};

const MEBIBYTE: u64 = 1024 * 1024;

#[derive(ValueEnum, Clone)]
enum SortMethod {
    Size,
    Name,
}

struct LffFile {
    name: OsString,
    extension: Option<OsString>,
    size: u64,
    formatted_size: String,
    hidden: bool,
}

/// Recursively finds large files.
#[derive(Parser)]
#[command(version, about)]
struct LffArgs {
    /// The directory to begin searching in.
    directory: String,
    /// Display absolute paths for files.
    /// Automatically true if the supplied directory isn't relative.
    #[arg(short, long)]
    absolute: bool,
    /// Whether to display file sizes in KB/MB/GB over KiB/MiB/GiB.
    #[arg(long)]
    base_ten: bool,
    /// Exclude hidden files and directories.
    #[arg(long)]
    exclude_hidden: bool,
    /// Filter files by extension.
    #[arg(short, long)]
    extension: Option<OsString>,
    /// Return a maximum of this many files.
    #[arg(short, long)]
    limit: Option<usize>,
    /// The minimum size in MiB for displayed files, e.g. 10 = 10 MiB, 0.1 = 100 KiB.
    #[arg(short, long, default_value_t = 50.0)]
    min_size_mib: f64,
    /// Filter file names by glob patterns, e.g. *abc* will yield 1abc2.txt.
    #[arg(short, long)]
    name_pattern: Option<String>,
    /// Pretty-prints file sizes.
    #[arg(short, long)]
    pretty: bool,
    /// How to sort found files.
    #[arg(short, long, value_enum)]
    sort_method: Option<SortMethod>,
}

fn path_is_hidden(file_path: &Path) -> bool {
    match file_path.file_name() {
        Some(name) => match name.to_str() {
            Some(str_name) => str_name.starts_with('.'),
            None => false,
        },
        None => false,
    }
}

fn handle_entry(file_path: PathBuf, args: &LffArgs) -> Result<LffFile> {
    let file_name: OsString = match args.absolute {
        true => canonicalize(&file_path)?.into_os_string(),
        false => file_path.clone().into_os_string(),
    };
    let file_extension: Option<OsString> = file_path.extension().map(|ext| ext.to_os_string());
    let file_size: u64 = symlink_metadata(&file_path)?.len();
    let file_size_rep: String = match args.pretty {
        true => Size::from_bytes(file_size)
            .format()
            .with_base(if args.base_ten {
                Base::Base10
            } else {
                Base::Base2
            })
            .with_style(Style::Abbreviated)
            .to_string(),
        false => file_size.to_string(),
    };

    Ok(LffFile {
        name: file_name,
        extension: file_extension,
        size: file_size,
        formatted_size: file_size_rep,
        hidden: path_is_hidden(&file_path),
    })
}

fn handle_directory(directory: ReadDir, args: &LffArgs) -> Result<Vec<LffFile>> {
    let files = directory
        .into_iter()
        .enumerate()
        .par_bridge()
        .flat_map(|(idx, entry_result)| {
            if let Some(lim) = args.limit {
                if args.sort_method.is_none() && idx >= lim {
                    return vec![];
                }
            }
            let entry: DirEntry = entry_result.unwrap();
            let file_path: PathBuf = entry.path();
            let entry_type: FileType = entry.file_type().unwrap();
            if entry_type.is_file() {
                let file: LffFile = handle_entry(file_path, args).unwrap();
                let large_enough: bool = file.size as f64 / MEBIBYTE as f64 >= args.min_size_mib;
                let correct_ext: bool = match &args.extension {
                    Some(arg_ext) => match file.extension {
                        Some(ref file_ext) => file_ext == arg_ext,
                        None => false,
                    },
                    None => true,
                };
                let correct_name: bool = match &args.name_pattern {
                    Some(arg_np) => Glob::new(arg_np)
                        .wrap_err_with(|| eyre!("Invalid glob from name pattern flag: '{arg_np}'"))
                        .unwrap()
                        .compile_matcher()
                        .is_match(&file.name),
                    None => true,
                };
                let is_not_hidden: bool = match &args.exclude_hidden {
                    true => !file.hidden,
                    false => true,
                };
                if large_enough && correct_ext && correct_name && is_not_hidden {
                    return vec![file];
                }
            } else if entry_type.is_dir() {
                match args.exclude_hidden {
                    true if path_is_hidden(&file_path) => (),
                    _ => return handle_directory(read_dir(&file_path).unwrap(), args).unwrap(),
                };
            }
            vec![]
        })
        .collect();
    Ok(files)
}

fn run_finder(args: LffArgs) -> Result<()> {
    let directory: ReadDir = read_dir(&args.directory)
        .wrap_err_with(|| format!("Invalid supplied start directory: '{}'", &args.directory))?;

    let mut files_vec: Vec<LffFile> = handle_directory(directory, &args)?;

    let longest_size_rep: usize = match files_vec
        .iter()
        .max_by(|x, y| x.formatted_size.len().cmp(&y.formatted_size.len()))
    {
        Some(file) => file.formatted_size.len(),
        None => 0,
    };

    match args.sort_method {
        Some(SortMethod::Size) => files_vec.sort_by(|a, b| b.size.cmp(&a.size)),
        Some(SortMethod::Name) => files_vec.sort_by(|a, b| a.name.cmp(&b.name)),
        _ => (),
    };
    if let Some(lim) = args.limit {
        files_vec.truncate(lim);
    }

    if !files_vec.is_empty() {
        for file in &files_vec {
            println!(
                "{:<width$}  {:?}",
                file.formatted_size,
                file.name,
                width = longest_size_rep
            );
        }
    } else {
        println!("No files found for the specified arguments!");
    }

    Ok(())
}

fn main() -> Result<()> {
    let args: LffArgs = LffArgs::parse();
    run_finder(args)?;
    Ok(())
}

/*
TODOS
Remove unwraps - look at implementing the collect trait or even a custom iterator
Test glob before running
Benchmarking - use hyperfine
Interactive mode, use ratatui, allow scrolling, deleting maybe, etc.
Tests
Comments
README
GitHub actions - lint, test/coverage, build/package
 */

// BENCHES
// RECURSIVE
// hyperfine --warmup 10 --runs 20 './target/release/lff -m 0 -s size -l 20 ~/Downloads' 'du -a ~/Downloads | sort -r -n | head -n 20' 'dust --skip-total -R -F -n 20 -r ~/Downloads/'
// NOT RECURSIVE
// hyperfine --warmup 10 --runs 20 './target/release/lff -m 0 -s size -l 20 ~/Downloads' 'du -s ~/Downloads/* | sort -r -n | head -n 20' 'dust --skip-total -R -F -n 20 -r -d 1 ~/Downloads/'
