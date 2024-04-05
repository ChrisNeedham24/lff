use clap::{Parser, ValueEnum};
use eyre::{eyre, EyreHandler, Result, WrapErr};
use globset::Glob;
use rayon::prelude::*;
use size::{Base, Size, Style};
use std::error::Error as StdError;
use std::ffi::OsString;
use std::fmt::{Formatter, Result as FmtResult};
use std::fs::{canonicalize, read_dir, symlink_metadata, DirEntry, FileType, ReadDir};
use std::path::{Path, PathBuf};

// For convenience's sake, define the size of a mebibyte.
const MEBIBYTE: u64 = 1024 * 1024;

// The message to return when no files are found matching the supplied arguments.
const NO_FILES_FOUND_STR: &str = "No files found for the specified arguments!";

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
#[derive(Debug)]
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
    /// Whether to display file sizes in KB/MB/GB over KiB/MiB/GiB when pretty-printing is enabled.
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
    /// Filter file names by quoted glob patterns, e.g. '*abc*' will yield 1abc2.txt.
    #[arg(short, long)]
    name_pattern: Option<String>,
    /// Pretty-prints file sizes.
    #[arg(short, long)]
    pretty: bool,
    /// How to sort found files.
    #[arg(short, long, value_enum)]
    sort_method: Option<SortMethod>,
}

/// A custom handler for eyre - we want to omit the location from returned errors.
struct LffEyreHandler;

/// The implementation of the EyreHandler trait for our custom eyre handler.
impl EyreHandler for LffEyreHandler {
    /// Defines the format for our custom handler - exactly the same as the standard format except
    /// without the location.
    ///
    /// # Errors
    /// - If there is an issue writing to the supplied formatter.
    fn debug(&self, error: &(dyn StdError + 'static), f: &mut Formatter<'_>) -> FmtResult {
        writeln!(f, "{}\n", error)?;
        if let Some(src) = error.source() {
            write!(f, "Caused by:\n    {}", src)?;
        }
        Ok(())
    }
}

/// A custom printer trait - we define this in order to inject a printer dependency into our tests
/// in order to test standard output.
trait LffPrinter {
    /// Prints the given `String` value - we maintain a reference to `self` so that the test
    /// implementations of this trait can supply data structures to keep track of passed values.
    fn println(&mut self, value: String);
}

/// The standard printer, printing straight to standard out.
struct LffStdoutPrinter;

/// The implementation of our printer trait for the standard printer used in the business logic.
impl LffPrinter for LffStdoutPrinter {
    /// Prints the given `String` value using the `println!` macro.
    fn println(&mut self, value: String) {
        println!("{}", value);
    }
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
        true => canonicalize(&file_path)
            .wrap_err_with(|| format!("Could not generate absolute path for {:?}", &file_path))?
            .into_os_string(),
        // Yes, cloning isn't good, but it's an extremely minor performance hit in this case.
        false => file_path.clone().into_os_string(),
    };
    let file_extension: Option<OsString> = file_path.extension().map(|ext| ext.to_os_string());
    // We use symlink_metadata() here rather than just metadata() because we don't want to follow
    // all the links around the filesystem - this improves performance somewhat. Some other tools in
    // this area use blocks() and then multiply by the block size to get the true file size, but
    // we're not overly concerned about that.
    let file_size: u64 = symlink_metadata(&file_path)
        .wrap_err_with(|| format!("Could not retrieve metadata for {:?}", &file_path))?
        .len();
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
fn run_finder(args: LffArgs, printer: &mut dyn LffPrinter) -> Result<()> {
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
        // Print each of the given files to the supplied printer, padding the file size so that
        // all of the file names are horizontally aligned.
        for file in &files_vec {
            printer.println(format!(
                "{:<width$}  {:?}",
                file.formatted_size,
                file.name,
                width = longest_size_rep
            ));
        }
    } else {
        printer.println(String::from(NO_FILES_FOUND_STR));
    }

    Ok(())
}

macro_rules! run_finder {
    ($args: expr, $printer: expr) => {
        run_finder($args, $printer)
    };
    ($args: expr) => {
        run_finder($args, &mut LffStdoutPrinter)
    };
}

/// The main function of `lff`.
///
/// # Errors
/// - If there is an issue setting our custom eyre handler.
/// - If there is an issue running the finder in [run_finder].
fn main() -> Result<()> {
    // Set the eyre handler to be our custom one before running the finder.
    eyre::set_hook(Box::new(|_| Box::new(LffEyreHandler)))?;
    let args: LffArgs = LffArgs::parse();
    run_finder!(args)
}

#[cfg(test)]
mod tests {
    use crate::{
        handle_directory, handle_entry, path_is_hidden, run_finder, LffArgs, LffFile, LffPrinter,
        SortMethod, NO_FILES_FOUND_STR,
    };
    use eyre::Report;
    use std::ffi::OsString;
    use std::fs::{read_dir, ReadDir};
    use std::path::{Path, PathBuf};
    use std::str::from_utf8_unchecked;

    const BASE_ARGS: LffArgs = LffArgs {
        directory: String::new(),
        absolute: false,
        base_ten: false,
        exclude_hidden: false,
        extension: None,
        limit: None,
        min_size_mib: 0.0,
        name_pattern: None,
        pretty: false,
        sort_method: None,
    };

    #[derive(Default)]
    struct LffTestPrinter(Vec<String>);

    impl LffPrinter for LffTestPrinter {
        fn println(&mut self, value: String) {
            self.0.push(value);
        }
    }

    #[test]
    fn test_hidden_paths() {
        let visible_file: &Path = Path::new("test_resources/snow.txt");
        let visible_dir: &Path = Path::new("test_resources/visible");
        assert!(!path_is_hidden(visible_file));
        assert!(!path_is_hidden(visible_dir));

        let hidden_file: &Path = Path::new("test_resources/.hidden");
        let hidden_dir: &Path = Path::new("test_resources/.hidden_dir");
        assert!(path_is_hidden(hidden_file));
        assert!(path_is_hidden(hidden_dir));

        unsafe {
            let invalid_bytes: Vec<u8> = vec![0, 159, 145, 160];
            let non_utf8_path = Path::new(from_utf8_unchecked(&invalid_bytes));
            assert!(!path_is_hidden(non_utf8_path));
        }
        let invalid_path: &Path = Path::new("test_resources/..");
        assert!(!path_is_hidden(invalid_path));
    }

    #[test]
    fn test_handle_entry() {
        let test_file: PathBuf = Path::new("test_resources/snow.txt").to_path_buf();
        let file: LffFile = handle_entry(test_file, &BASE_ARGS).unwrap();
        assert_eq!("test_resources/snow.txt", file.name);
        assert_eq!(Some(OsString::from("txt")), file.extension);
        assert_eq!(544, file.size);
        assert_eq!("544", file.formatted_size);
        assert!(!file.hidden);
    }

    #[test]
    fn test_handle_entry_absolute() {
        let test_file: PathBuf = Path::new("test_resources/snow.txt").to_path_buf();
        let test_args: &LffArgs = &LffArgs {
            absolute: true,
            ..BASE_ARGS
        };

        let file: LffFile = handle_entry(test_file, test_args).unwrap();
        assert!(file
            .name
            .to_str()
            .unwrap()
            .ends_with("lff/test_resources/snow.txt"));
    }

    #[test]
    fn test_handle_entry_absolute_invalid_path() {
        let test_file: PathBuf = Path::new("test_resources/snow2.txt").to_path_buf();
        let test_args: &LffArgs = &LffArgs {
            absolute: true,
            ..BASE_ARGS
        };
        let canonicalize_error: Report = handle_entry(test_file, test_args).unwrap_err();
        assert_eq!(
            "Could not generate absolute path for \"test_resources/snow2.txt\"",
            canonicalize_error.to_string()
        );
    }

    #[test]
    fn test_handle_entry_none_extension() {
        let test_file_no_ext: PathBuf = Path::new("test_resources/LICENCE").to_path_buf();
        let no_ext_file: LffFile = handle_entry(test_file_no_ext, &BASE_ARGS).unwrap();
        assert_eq!(None, no_ext_file.extension);

        let test_file_hidden: PathBuf = Path::new("test_resources/.hidden").to_path_buf();
        let hidden_file: LffFile = handle_entry(test_file_hidden, &BASE_ARGS).unwrap();
        assert_eq!(None, hidden_file.extension);
    }

    #[test]
    fn test_handle_entry_metadata_invalid_path() {
        let test_file: PathBuf = Path::new("test_resources/snow2.txt").to_path_buf();
        let metadata_error: Report = handle_entry(test_file, &BASE_ARGS).unwrap_err();
        assert_eq!(
            "Could not retrieve metadata for \"test_resources/snow2.txt\"",
            metadata_error.to_string()
        );
    }

    #[test]
    fn test_handle_entry_pretty() {
        let test_file: PathBuf = Path::new("test_resources/.hidden_dir/spider.txt").to_path_buf();
        let test_args: &LffArgs = &LffArgs {
            pretty: true,
            ..BASE_ARGS
        };

        let file: LffFile = handle_entry(test_file, test_args).unwrap();
        assert_eq!("1.16 KiB", file.formatted_size);
    }

    #[test]
    fn test_handle_entry_pretty_base_ten() {
        let test_file: PathBuf = Path::new("test_resources/.hidden_dir/spider.txt").to_path_buf();
        let test_args: &LffArgs = &LffArgs {
            pretty: true,
            base_ten: true,
            ..BASE_ARGS
        };

        let file: LffFile = handle_entry(test_file, test_args).unwrap();
        assert_eq!("1.18 KB", file.formatted_size);
    }

    #[test]
    fn test_handle_entry_pretty_under_kilo() {
        let test_file: PathBuf = Path::new("test_resources/snow.txt").to_path_buf();
        let test_args: &LffArgs = &LffArgs {
            pretty: true,
            ..BASE_ARGS
        };

        let file: LffFile = handle_entry(test_file, test_args).unwrap();
        assert_eq!("544 B", file.formatted_size);
    }

    #[test]
    fn test_handle_entry_hidden() {
        let test_file: PathBuf = Path::new("test_resources/.hidden").to_path_buf();
        let file: LffFile = handle_entry(test_file, &BASE_ARGS).unwrap();
        assert!(file.hidden);
    }

    #[test]
    fn test_handle_directory() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let mut files: Vec<LffFile> = handle_directory(test_dir, &BASE_ARGS).unwrap();
        files.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(5, files.len());

        let hidden_file: &LffFile = &files[0];
        assert_eq!("test_resources/.hidden", hidden_file.name);
        assert_eq!(None, hidden_file.extension);
        assert_eq!(0, hidden_file.size);
        assert_eq!("0", hidden_file.formatted_size);
        assert!(hidden_file.hidden);

        let spider_file: &LffFile = &files[1];
        assert_eq!("test_resources/.hidden_dir/spider.txt", spider_file.name);
        assert_eq!(Some(OsString::from("txt")), spider_file.extension);
        assert_eq!(1183, spider_file.size);
        assert_eq!("1183", spider_file.formatted_size);
        assert!(!spider_file.hidden);

        let licence_file: &LffFile = &files[2];
        assert_eq!("test_resources/LICENCE", licence_file.name);
        assert_eq!(None, licence_file.extension);
        assert_eq!(27, licence_file.size);
        assert_eq!("27", licence_file.formatted_size);
        assert!(!licence_file.hidden);

        let snow_file: &LffFile = &files[3];
        assert_eq!("test_resources/snow.txt", snow_file.name);
        assert_eq!(Some(OsString::from("txt")), snow_file.extension);
        assert_eq!(544, snow_file.size);
        assert_eq!("544", snow_file.formatted_size);
        assert!(!snow_file.hidden);

        let mud_file: &LffFile = &files[4];
        assert_eq!("test_resources/visible/mud.md", mud_file.name);
        assert_eq!(Some(OsString::from("md")), mud_file.extension);
        assert_eq!(329, mud_file.size);
        assert_eq!("329", mud_file.formatted_size);
        assert!(!mud_file.hidden);
    }

    #[test]
    fn test_handle_directory_limit_no_sort() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let test_args: &LffArgs = &LffArgs {
            limit: Some(1),
            ..BASE_ARGS
        };
        let files: Vec<LffFile> = handle_directory(test_dir, test_args).unwrap();
        assert_eq!(1, files.len());
    }

    #[test]
    fn test_handle_directory_limit_with_sort() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let test_args: &LffArgs = &LffArgs {
            limit: Some(1),
            sort_method: Some(SortMethod::Size),
            ..BASE_ARGS
        };
        let files: Vec<LffFile> = handle_directory(test_dir, test_args).unwrap();
        assert_eq!(5, files.len());
    }

    #[test]
    fn test_handle_directory_min_size() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let test_args: &LffArgs = &LffArgs {
            min_size_mib: 0.001,
            ..BASE_ARGS
        };

        let files: Vec<LffFile> = handle_directory(test_dir, test_args).unwrap();
        assert_eq!(1, files.len());
        let spider_file: &LffFile = &files[0];
        assert_eq!("test_resources/.hidden_dir/spider.txt", spider_file.name);
        assert_eq!(1183, spider_file.size);
    }

    #[test]
    fn test_handle_directory_extension() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let test_args: &LffArgs = &LffArgs {
            extension: Some(OsString::from("md")),
            ..BASE_ARGS
        };

        let files: Vec<LffFile> = handle_directory(test_dir, test_args).unwrap();
        assert_eq!(1, files.len());
        let mud_file: &LffFile = &files[0];
        assert_eq!("test_resources/visible/mud.md", mud_file.name);
        assert_eq!(Some(OsString::from("md")), mud_file.extension);
    }

    #[test]
    fn test_handle_directory_name_pattern() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let test_args: &LffArgs = &LffArgs {
            name_pattern: Some(String::from("*no*")),
            ..BASE_ARGS
        };

        let files: Vec<LffFile> = handle_directory(test_dir, test_args).unwrap();
        assert_eq!(1, files.len());
        let snow_file: &LffFile = &files[0];
        assert_eq!("test_resources/snow.txt", snow_file.name);
    }

    #[test]
    fn test_handle_directory_invalid_name_pattern() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let test_args: &LffArgs = &LffArgs {
            name_pattern: Some(String::from("[")),
            ..BASE_ARGS
        };
        let new_glob_error: Report = handle_directory(test_dir, test_args).unwrap_err();
        assert_eq!(
            "Invalid glob from name pattern flag: '['",
            new_glob_error.to_string()
        );
    }

    #[test]
    fn test_handle_directory_exclude_hidden() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let test_args: &LffArgs = &LffArgs {
            exclude_hidden: true,
            // This pattern would match .hidden_dir/spider.txt, visible/mud.md, and .hidden, but
            // since we're excluding hidden files and directories, we only expect mud.md to be
            // yielded.
            name_pattern: Some(String::from("*d*")),
            ..BASE_ARGS
        };

        let files: Vec<LffFile> = handle_directory(test_dir, test_args).unwrap();
        assert_eq!(1, files.len());
        let mud_file: &LffFile = &files[0];
        assert_eq!("test_resources/visible/mud.md", mud_file.name);
        assert_eq!(Some(OsString::from("md")), mud_file.extension);
    }
    //
    // #[test]
    // fn test_print_output_no_files() {
    //     let mut test_printer: LffTestPrinter = LffTestPrinter::default();
    //     print_output(vec![], 10, &mut test_printer);
    //     assert_eq!(NO_FILES_FOUND_STR, test_printer.0[0]);
    // }

    #[test]
    fn test_run_finder() {
        let test_args: LffArgs = LffArgs {
            directory: String::from("test_resources"),
            sort_method: Some(SortMethod::Size),
            ..BASE_ARGS
        };
        let mut test_printer: LffTestPrinter = LffTestPrinter::default();

        run_finder!(test_args, &mut test_printer).unwrap();
        assert_eq!("1183  \"test_resources/.hidden_dir/spider.txt\"", test_printer.0[0]);
        assert_eq!("544   \"test_resources/snow.txt\"", test_printer.0[1]);
        assert_eq!("329   \"test_resources/visible/mud.md\"", test_printer.0[2]);
        assert_eq!("27    \"test_resources/LICENCE\"", test_printer.0[3]);
        assert_eq!("0     \"test_resources/.hidden\"", test_printer.0[4]);
    }

    #[test]
    fn test_run_finder_invalid_dir() {
        let test_args: LffArgs = LffArgs {
            directory: String::from("this is not real"),
            ..BASE_ARGS
        };
        
    }
}

/*
TODOS
Tests
GitHub actions - lint, test/coverage, build/package
Interactive mode, use ratatui, allow scrolling, deleting maybe, etc.
Create GitHub issues for missing stuff:
- highlighting of filters
- glob performance
- package manager bundling
- stuff removed from TODOs in other commits
- regex support
- etc
 */
