use clap::{Parser, ValueEnum};
use eyre::{eyre, Result, WrapErr};
use globset::Glob;
use rayon::prelude::*;
use size::{Base, Size, Style};
use std::ffi::OsString;
use std::fs::{canonicalize, read_dir, symlink_metadata, DirEntry, FileType, ReadDir};
use std::path::{Path, PathBuf};

// For convenience's sake, define the size of a mebibyte.
const MEBIBYTE: u64 = 1024 * 1024;

/// The ways in which displayed files can be sorted. Derives ValueEnum and Clone so that it can be
/// used as a type for the clap command-line arguments.
#[derive(ValueEnum, Clone)]
enum SortMethod {
    Size,
    Name,
}

/// A representation of a file from within the file system. `OsString`s are used because Rust
/// `String`s are UTF-8 encoded, and not all file names and extensions will be UTF-8 encoded in a
/// file system.
///
/// The file's `formatted_size` refers to how it will be displayed in the output. Some examples
/// include `1024`, `1 KiB`, or `1.02 KB`.
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

/// Returns whether the file at the supplied path is a hidden file, i.e. whether its name starts
/// with a '.' character.
///
/// If a file's name cannot be represented in UTF-8, we assume it's not hidden, since we can't
/// inspect the first character of its name.
///
/// Non-file paths will also return false.
fn path_is_hidden(file_path: &Path) -> bool {
    match file_path.file_name() {
        Some(name) => match name.to_str() {
            Some(str_name) => str_name.starts_with('.'),
            None => false,
        },
        None => false,
    }
}

/// Extract file details from the supplied `PathBuf`, applying the appropriate command-line
/// arguments, and returning the created `LffFile` in success cases.
///
/// # Errors
///
/// - If the absolute flag is passed, and the file's path cannot be canonicalised.
/// - If metadata cannot be retrieved for the file.
fn handle_entry(file_path: PathBuf, args: &LffArgs) -> Result<LffFile> {
    // The OsString representation of PathBufs is actually pretty good, so we can just use that no
    // matter what the absolute flag value is.
    let file_name: OsString = match args.absolute {
        true => canonicalize(&file_path)?.into_os_string(),
        // Yes, cloning isn't good, but it's an extremely minor performance hit in this case.
        false => file_path.clone().into_os_string(),
    };
    let file_extension: Option<OsString> = file_path.extension().map(|ext| ext.to_os_string());
    // We use symlink_metadata() here rather than just metadata() because we don't want to follow
    // all the links around the filesystem - this improves performance somewhat. Some other tools in
    // this area use blocks() and then multiply by the block size to get the true file size, but
    // we're not overly concerned about that.
    let file_size: u64 = symlink_metadata(&file_path)?.len();
    let file_size_rep: String = match args.pretty {
        true => Size::from_bytes(file_size)
            .format()
            .with_base(if args.base_ten {
                Base::Base10
            } else {
                Base::Base2
            })
            // Abbreviate the size so that we don't get the whole word 'bytes' in the output.
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

/// Extract files and their details from the supplied `ReadDir` in parallel, applying the
/// appropriate command-line arguments, and returning a `Vec` of created `LffFile`s in success
/// cases.
///
/// # Errors
///
/// - If the directory entry cannot be retrieved.
/// - If the file type cannot be determined for the retrieved directory entry.
/// - If there is an issue handling the directory entry in [handle_entry].
/// - If the supplied glob pattern to filter on is invalid.
fn handle_directory(directory: ReadDir, args: &LffArgs) -> Result<Vec<LffFile>> {
    // It seems odd at first glance that we would be using a two-dimensional Vec here, but this is
    // due to limitations in the rayon parallelism library with respect to flattening.
    // Fundamentally, this is due to error handling - rayon does not let us collect Results with a
    // single-dimensional Vec.
    let two_d_files: Result<Vec<Vec<LffFile>>> = directory
        .into_iter()
        // We need to enumerate here so that we can exit early if no sort has been applied, and an
        // applied limit has been reached.
        .enumerate()
        // Split and handle each directory entry in parallel.
        .par_bridge()
        // Rayon doesn't play nice with flat_map() and then collecting with Results, so we just use
        // map() and flatten after.
        .map(|(idx, entry_result)| {
            // If a limit argument was supplied, no sort was supplied, and we've reached the limit
            // (or further, since we may have surpassed the limit due to parallelism), exit early.
            if let Some(lim) = args.limit {
                if args.sort_method.is_none() && idx >= lim {
                    // We just return empty vectors when no files are returned - these will be
                    // flattened out later.
                    return Ok(vec![]);
                }
            }
            let entry: DirEntry = entry_result?;
            let file_path: PathBuf = entry.path();
            // For whatever reason, using the FileType here to determine whether the entry is a file
            // or a directory is significantly faster than using the same methods on the PathBuf.
            let entry_type: FileType = entry.file_type()?;
            if entry_type.is_file() {
                let file: LffFile = handle_entry(file_path, args)?;
                let large_enough: bool = file.size as f64 / MEBIBYTE as f64 >= args.min_size_mib;
                let correct_ext: bool = match &args.extension {
                    Some(arg_ext) => match file.extension {
                        // We need to use a ref to the file's extension in order to compare OsString
                        // equality.
                        Some(ref file_ext) => file_ext == arg_ext,
                        None => false,
                    },
                    None => true,
                };
                let correct_name: bool = match &args.name_pattern {
                    Some(arg_np) => Glob::new(arg_np)
                        .wrap_err_with(|| eyre!("Invalid glob from name pattern flag: '{arg_np}'"))?
                        .compile_matcher()
                        .is_match(&file.name),
                    None => true,
                };
                let is_not_hidden: bool = match &args.exclude_hidden {
                    true => !file.hidden,
                    false => true,
                };
                // If all our optional conditions are met, return a Vec with a single file.
                if large_enough && correct_ext && correct_name && is_not_hidden {
                    return Ok(vec![file]);
                }
            } else if entry_type.is_dir() {
                // Just ignore directories we can't read.
                if let Ok(dir) = read_dir(&file_path) {
                    match args.exclude_hidden {
                        // Add a guard so we only need two cases.
                        true if path_is_hidden(&file_path) => (),
                        // This actually returns a Vec with 0 or more files, which will be flattened
                        // out later.
                        _ => return handle_directory(dir, args),
                    };
                }
            }
            // We should never really get here, but just in case, return an empty Vec to be
            // flattened out later.
            Ok(vec![])
        })
        .collect();
    // Now we can flatten out our two-dimensional file Vec - if an error occurred during the
    // processing of the directory, the first to occur will be returned.
    let flat_files: Vec<LffFile> = two_d_files?.into_iter().flatten().collect();
    Ok(flat_files)
}

/// Run `lff` with the supplied arguments.
///
/// # Errors
///
/// - If the supplied start directory does not exist.
/// - If there is an issue handling the directory in [handle_directory].
fn run_finder(args: LffArgs) -> Result<()> {
    let directory: ReadDir = read_dir(&args.directory)
        .wrap_err_with(|| format!("Invalid supplied start directory: '{}'", &args.directory))?;

    let mut files_vec: Vec<LffFile> = handle_directory(directory, &args)?;

    // We need to work out the longest file size string representation in the returned files so that
    // we can appropriately pad the output.
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

/// The main function of `lff`.
///
/// # Errors
///
/// - If there is an issue running `lff` in [run_finder].
fn main() -> Result<()> {
    let args: LffArgs = LffArgs::parse();
    run_finder(args)?;
    Ok(())
}

/*
TODOS
Tests
GitHub actions - lint, test/coverage, build/package
Interactive mode, use ratatui, allow scrolling, deleting maybe, etc.
 */
