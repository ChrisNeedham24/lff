use std::ffi::{OsString};
use std::fs::{canonicalize, DirEntry, metadata, Metadata, read_dir, ReadDir};
use std::io::Error as IOError;
use std::path::PathBuf;
use clap::{Parser, ValueEnum};
use size::{Size, Style};

const MEBIBYTE: u64 = 1024 * 1024;

#[derive(ValueEnum, Clone)]
enum SortMethod {
    Size,
    Name
}

struct LffFile {
    name: OsString,
    size: u64,
    formatted_size: String
}

/// Recursively finds large files.
#[derive(Parser)]
#[command(version, about)]
struct LffArgs {
    /// The directory to begin searching in.
    directory: String,
    /// Display absolute paths for files.
    #[arg(short, long)]
    absolute: bool,
    /// The minimum size in MiB for displayed files, e.g. 10 = 10 MiB, 0.1 = 100 KiB.
    #[arg(short, long, default_value_t = 50.0)]
    min_size_mib: f64,
    /// Pretty-prints file sizes.
    #[arg(short, long)]
    pretty: bool,
    /// How to sort found files.
    #[arg(short, long, value_enum)]
    sort_method: Option<SortMethod>,
}

fn standard_panic(err: Option<IOError>) -> ! {
    match err {
        Some(err) => panic!("Error was: {}", err),
        None => panic!("Error was unspecified")
    };
}

fn handle_entry(file_path: PathBuf, args: &LffArgs) -> LffFile {
    let file_name: OsString = match args.absolute {
        true => match canonicalize(&file_path) {
            Ok(path) => path.into_os_string(),
            Err(error) => standard_panic(Some(error)),
        },
        false => file_path.clone().into_os_string(),
    };
    let file_metadata: Metadata = match metadata(file_path) {
        Ok(metadata) => metadata,
        Err(error) => standard_panic(Some(error)),
    };
    let file_size: u64 = file_metadata.len();
    let file_size_rep: String = match args.pretty {
        true => Size::from_bytes(file_size)
            .format()
            .with_style(Style::Abbreviated)
            .to_string(),
        false => file_size.to_string(),
    };

    LffFile {
        name: file_name,
        size: file_size,
        formatted_size: file_size_rep
    }
}

fn handle_directory(directory: ReadDir, files_vec: &mut Vec<LffFile>, args: &LffArgs) {
    for entry_result in directory {
        let entry: DirEntry = match entry_result {
            Ok(entry) => entry,
            Err(error) => standard_panic(Some(error)),
        };
        let file_path: PathBuf = entry.path();
        if file_path.is_file() {
            let file: LffFile = handle_entry(file_path, args);
            if file.size as f64 / MEBIBYTE as f64 >= args.min_size_mib {
                files_vec.push(file);
            }
        } else if file_path.is_dir() {
            match read_dir(file_path) {
                Ok(directory) => handle_directory(directory, files_vec, args),
                Err(error) => standard_panic(Some(error)),
            };
        }
    }
}

fn run_finder(args: LffArgs) {
    let mut files_vec: Vec<LffFile> = Vec::new();

    let directory: ReadDir = match read_dir(&args.directory) {
        Ok(directory) => directory,
        Err(error) => standard_panic(Some(error)),
    };

    handle_directory(directory, &mut files_vec, &args);

    let mut longest_size_rep: usize = 0;
    for file in &files_vec {
        if file.formatted_size.len() > longest_size_rep {
            longest_size_rep = file.formatted_size.len()
        }
    }

    match args.sort_method {
        Some(SortMethod::Size) => {
            files_vec.sort_by(|a, b| b.size.cmp(&a.size))
        },
        Some(SortMethod::Name) => {
            files_vec.sort_by(|a, b| a.name.cmp(&b.name))
        },
        _ => ()
    };

    for file in &files_vec {
        println!("{:<width$}  {:?}", file.formatted_size, file.name, width = longest_size_rep);
    }
}

fn main() {
    let args: LffArgs = LffArgs::parse();
    run_finder(args);
}

/*
TODOS
Filter by extension
Filter by name
Limit
Benchmarking
Efficiency - the path clone is bad
Flag for KB not KiB - change min size flag to use that instead too if specified
Flag for exclude hidden
Tests
Remove panic?
Consider default flag values
Add print when there are no files big enough
 */
