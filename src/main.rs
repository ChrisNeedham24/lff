use clap::{Parser, ValueEnum};
use globset::Glob;
use size::{Base, Size, Style};
use std::ffi::OsString;
use std::fs::{canonicalize, metadata, read_dir, DirEntry, Metadata, ReadDir};
use std::io::Error as IOError;
use std::path::PathBuf;

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

fn io_panic(err: IOError) -> ! {
    panic!("Error was: {}", err);
}

fn path_is_hidden(file_path: PathBuf) -> bool {
    match file_path.file_name() {
        Some(name) => match name.to_str() {
            Some(str_name) => str_name.starts_with('.'),
            None => false,
        },
        None => false,
    }
}

fn handle_entry(file_path: PathBuf, args: &LffArgs) -> LffFile {
    let file_name: OsString = match args.absolute {
        true => match canonicalize(&file_path) {
            Ok(path) => path.into_os_string(),
            Err(error) => io_panic(error),
        },
        false => file_path.clone().into_os_string(),
    };
    let file_extension: Option<OsString> = file_path.extension().map(|ext| ext.to_os_string());
    let file_metadata: Metadata = match metadata(&file_path) {
        Ok(metadata) => metadata,
        Err(error) => io_panic(error),
    };
    let file_size: u64 = file_metadata.len();
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

    LffFile {
        name: file_name,
        extension: file_extension,
        size: file_size,
        formatted_size: file_size_rep,
        hidden: path_is_hidden(file_path),
    }
}

fn handle_directory(directory: ReadDir, files_vec: &mut Vec<LffFile>, args: &LffArgs) {
    for entry_result in directory {
        let entry: DirEntry = match entry_result {
            Ok(entry) => entry,
            Err(error) => io_panic(error),
        };
        let file_path: PathBuf = entry.path();
        if file_path.is_file() {
            let file: LffFile = handle_entry(file_path, args);
            let large_enough: bool = file.size as f64 / MEBIBYTE as f64 >= args.min_size_mib;
            let correct_ext: bool = match &args.extension {
                Some(arg_ext) => match file.extension {
                    Some(ref file_ext) => file_ext == arg_ext,
                    None => false,
                },
                None => true,
            };
            let correct_name: bool = match &args.name_pattern {
                Some(arg_np) => match Glob::new(arg_np) {
                    Ok(glob) => glob.compile_matcher().is_match(&file.name),
                    Err(error) => panic!("Invalid glob: {}", error),
                },
                None => true,
            };
            let is_not_hidden: bool = match &args.exclude_hidden {
                true => !file.hidden,
                false => true,
            };
            if large_enough && correct_ext && correct_name && is_not_hidden {
                files_vec.push(file);
            }
        } else if file_path.is_dir() {
            match read_dir(&file_path) {
                Ok(directory) => match args.exclude_hidden {
                    true if path_is_hidden(file_path) => (),
                    _ => handle_directory(directory, files_vec, args),
                },
                Err(error) => io_panic(error),
            };
        }
    }
}

fn run_finder(args: LffArgs) {
    let mut files_vec: Vec<LffFile> = Vec::new();

    let directory: ReadDir = match read_dir(&args.directory) {
        Ok(directory) => directory,
        Err(error) => io_panic(error),
    };

    handle_directory(directory, &mut files_vec, &args);

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
}

fn main() {
    let args: LffArgs = LffArgs::parse();
    run_finder(args);
}

/*
TODOS
Benchmarking
Efficiency - the path clone is bad
Tests
Remove panic? - probably use anyhow instead
Consider default flag values
Add header using flags - e.g. 5 files larger than 50 MiB, sorted by name
Interactive mode, use ratatui, allow scrolling, deleting maybe, etc.
Add formatting to prints - e.g. bold matches for name or extension
Smart limiting - if there's no sort, return early
README
GitHub actions - lint, test, build/package
 */
