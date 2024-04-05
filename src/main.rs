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

/// The ways in which displayed files can be sorted. Derives `ValueEnum` and `Clone` so that it can
/// be used as a type for the clap command-line arguments.
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
    #[cfg(not(tarpaulin_include))]
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
    #[cfg(not(tarpaulin_include))]
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

/// Runs the [run_finder] function with the supplied `LffArgs` and an optionally-supplied
/// `LffPrinter`. If one is not supplied, an `LffStdoutPrinter` is used - in effect providing a
/// default argument for the [run_finder] function.
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
#[cfg(not(tarpaulin_include))]
fn main() -> Result<()> {
    // Set the eyre handler to be our custom one before running the finder.
    eyre::set_hook(Box::new(|_| Box::new(LffEyreHandler)))?;
    let args: LffArgs = LffArgs::parse();
    run_finder!(args)
}

/// A few functions are excluded from coverage collection:
/// - [LffEyreHandler::debug]: This is actually tested in [test_lff_eyre_handler], but is excluded
///   due to the fact that the test must run in isolation. This is because if other tests run before
///   it, eyre installs its standard handler, not our custom one, resulting in an error when the
///   test runs.
/// - [LffStdoutPrinter::println]: We cannot test values being printed to standard out, so this
///   function is excluded.
/// - [main]: Since the main function only consists of setting up eyre - which is tested elsewhere -
///   and parsing command-line arguments before running the finder, there is no need to test this.
///   Indeed, running the main function in a test results in errors because clap attempts to parse
///   the command-line arguments that are passed to `cargo test`.
#[cfg(test)]
mod tests {
    use crate::{
        handle_directory, handle_entry, path_is_hidden, run_finder, LffArgs, LffEyreHandler,
        LffFile, LffPrinter, LffStdoutPrinter, SortMethod, NO_FILES_FOUND_STR,
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

    /// A test printer that records 'printed' output in a `Vec`. Derives `Default` for convenience's
    /// sake when instantiating test instances.
    #[derive(Default)]
    struct LffTestPrinter(Vec<String>);

    /// The implementation of our printer trait for the test printer.
    impl LffPrinter for LffTestPrinter {
        /// Record the value in the printer's `Vec`, rather than printing it, so we can assert on it
        /// later.
        fn println(&mut self, value: String) {
            self.0.push(value);
        }
    }

    /// Ensure that our custom eyre handler correctly formats returned errors.
    ///
    /// This test is ignored by default because it needs to run in isolation - in cases where it is
    /// run after other tests, eyre will have already installed its default handler, resulting in an
    /// error when this test attempts to install our custom one.
    #[test]
    #[ignore]
    fn test_lff_eyre_handler() {
        // Install our custom handler in the same way as the main function.
        eyre::set_hook(Box::new(|_| Box::new(LffEyreHandler))).unwrap();

        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        // We pass an invalid glob as an argument so that we can get a consistent error that will
        // not vary based on operating system - unlike a file not found error, for example.
        let test_args: &LffArgs = &LffArgs {
            name_pattern: Some(String::from("[")),
            ..BASE_ARGS
        };

        let test_error: Report = handle_directory(test_dir, test_args).unwrap_err();
        // By formatting the Report like this, we directly call the debug function of our handler.
        let formatted_error: String = format!("{:?}", test_error);
        assert_eq!(
            "Invalid glob from name pattern flag: '['\n\n\
            Caused by:\n    error parsing glob '[': unclosed character class; missing ']'",
            formatted_error
        );
    }

    /// Ensure that the hidden status of paths is correctly determined.
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

        // In order to create a situation in which the to_str() call on the file name fails the
        // UTF-8 validity check, we need to enter unsafe mode and create a Path from an invalid
        // sequence of bytes. These bytes are taken directly from the documentation of the
        // from_utf8() function, in the part documenting incorrect bytes.
        unsafe {
            let invalid_bytes: Vec<u8> = vec![0, 159, 145, 160];
            let non_utf8_path: &Path = Path::new(from_utf8_unchecked(&invalid_bytes));
            assert!(!path_is_hidden(non_utf8_path));
        }
        // Since this is an invalid file name altogether, we expect this to not be hidden.
        let invalid_path: &Path = Path::new("test_resources/..");
        assert!(!path_is_hidden(invalid_path));
    }

    /// Ensure that a file has the correct details extracted.
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

    /// Ensure that when handling an entry with the absolute flag, the correct file name is
    /// extracted.
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
            // Obviously the full absolute path will differ on different machines, but as long as
            // the 'lff/' part of this path is there, we at least know that the path extends further
            // back than the root directory of this repository.
            .ends_with("lff/test_resources/snow.txt"));
    }

    /// Ensure that the correct error message is generated when an entry with an invalid path is
    /// supplied, and the absolute flag is on.
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

    /// Ensure that files with no extension and hidden files are both correctly determined to have
    /// no extension.
    #[test]
    fn test_handle_entry_none_extension() {
        let test_file_no_ext: PathBuf = Path::new("test_resources/LICENCE").to_path_buf();
        let no_ext_file: LffFile = handle_entry(test_file_no_ext, &BASE_ARGS).unwrap();
        assert_eq!(None, no_ext_file.extension);

        let test_file_hidden: PathBuf = Path::new("test_resources/.hidden").to_path_buf();
        let hidden_file: LffFile = handle_entry(test_file_hidden, &BASE_ARGS).unwrap();
        assert_eq!(None, hidden_file.extension);
    }

    /// Ensure that the correct error message is generated when an entry with an invalid path is
    /// supplied.
    #[test]
    fn test_handle_entry_metadata_invalid_path() {
        let test_file: PathBuf = Path::new("test_resources/snow2.txt").to_path_buf();
        let metadata_error: Report = handle_entry(test_file, &BASE_ARGS).unwrap_err();
        assert_eq!(
            "Could not retrieve metadata for \"test_resources/snow2.txt\"",
            metadata_error.to_string()
        );
    }

    /// Ensure that an entry's file size is of base 2 by default when the pretty flag is passed.
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

    /// Ensure that an entry's file size is of base 10 when both the pretty and base ten flags are
    /// passed.
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

    /// Ensure that an entry's file size is of the abbreviated style when the pretty flag is passed.
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

    /// Ensure that hidden entries are correctly identified as such.
    #[test]
    fn test_handle_entry_hidden() {
        let test_file: PathBuf = Path::new("test_resources/.hidden").to_path_buf();
        let file: LffFile = handle_entry(test_file, &BASE_ARGS).unwrap();
        assert!(file.hidden);
    }

    /// Ensure that all of the files in the test directory have their details correctly extracted.
    #[test]
    fn test_handle_directory() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let mut files: Vec<LffFile> = handle_directory(test_dir, &BASE_ARGS).unwrap();
        // Since handle_directory() does no sorting in of itself, we need to manually sort the
        // returned files in order for the test to be repeatable - the files are read in parallel,
        // after all.
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

    /// Ensure that 'smart limiting' (early exit) is applied when handling a directory and the
    /// limit flag is passed and no sort flag is passed.
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

    /// Ensure that the limit flag is ignored when handling a directory and the sort flag is also
    /// passed.
    #[test]
    fn test_handle_directory_limit_with_sort() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let test_args: &LffArgs = &LffArgs {
            limit: Some(1),
            sort_method: Some(SortMethod::Size),
            ..BASE_ARGS
        };
        let files: Vec<LffFile> = handle_directory(test_dir, test_args).unwrap();
        // Despite passing a limit of 1, we still get 5 files.
        assert_eq!(5, files.len());
    }

    /// Ensure that the minimum size flag functions as expected.
    #[test]
    fn test_handle_directory_min_size() {
        let test_dir: ReadDir = read_dir("test_resources").unwrap();
        let test_args: &LffArgs = &LffArgs {
            // 1 MiB / 1024 = 1 KiB.
            min_size_mib: 1.0 / 1024.0,
            ..BASE_ARGS
        };

        let files: Vec<LffFile> = handle_directory(test_dir, test_args).unwrap();
        assert_eq!(1, files.len());
        let spider_file: &LffFile = &files[0];
        assert_eq!("test_resources/.hidden_dir/spider.txt", spider_file.name);
        // We expect the one file returned to reach the size threshold.
        assert_eq!(1183, spider_file.size);
    }

    /// Ensure that the extension filter flag functions as expected.
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
        // We expect the one file returned to have the md extension.
        assert_eq!(Some(OsString::from("md")), mud_file.extension);
    }

    /// Ensure that the name pattern filter flag functions as expected.
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
        // We expect the one file returned to match the *no* glob.
        assert_eq!("test_resources/snow.txt", snow_file.name);
    }

    /// Ensure that the correct error message is generated when an invalid glob pattern is supplied
    /// as the name pattern filter flag.
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

    /// Ensure that the exclude hidden flag functions as expected, excluding both hidden files and
    /// hidden directories.
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
        // We expect the one file returned to not be hidden.
        assert_eq!("test_resources/visible/mud.md", mud_file.name);
        assert!(!mud_file.hidden);
    }

    /// Ensure that when the finder is run, the expected formatted text is output.
    #[test]
    fn test_run_finder() {
        let test_args: LffArgs = LffArgs {
            directory: String::from("test_resources"),
            // Sort by size for a repeatable test.
            sort_method: Some(SortMethod::Size),
            ..BASE_ARGS
        };
        let mut test_printer: LffTestPrinter = LffTestPrinter::default();

        run_finder!(test_args, &mut test_printer).unwrap();
        // Check that the correct output has been 'printed'.
        assert_eq!(5, test_printer.0.len());
        assert_eq!(
            "1183  \"test_resources/.hidden_dir/spider.txt\"",
            test_printer.0[0]
        );
        assert_eq!("544   \"test_resources/snow.txt\"", test_printer.0[1]);
        assert_eq!("329   \"test_resources/visible/mud.md\"", test_printer.0[2]);
        assert_eq!("27    \"test_resources/LICENCE\"", test_printer.0[3]);
        assert_eq!("0     \"test_resources/.hidden\"", test_printer.0[4]);
    }

    /// Ensure that when the finder is run and sorted by name, the expected formatted text is
    /// output.
    #[test]
    fn test_run_finder_sort_by_name() {
        let test_args: LffArgs = LffArgs {
            directory: String::from("test_resources"),
            sort_method: Some(SortMethod::Name),
            ..BASE_ARGS
        };
        let mut test_printer: LffTestPrinter = LffTestPrinter::default();

        run_finder!(test_args, &mut test_printer).unwrap();
        // Check that the correct output has been 'printed'.
        assert_eq!(5, test_printer.0.len());
        assert_eq!("0     \"test_resources/.hidden\"", test_printer.0[0]);
        assert_eq!(
            "1183  \"test_resources/.hidden_dir/spider.txt\"",
            test_printer.0[1]
        );
        assert_eq!("27    \"test_resources/LICENCE\"", test_printer.0[2]);
        assert_eq!("544   \"test_resources/snow.txt\"", test_printer.0[3]);
        assert_eq!("329   \"test_resources/visible/mud.md\"", test_printer.0[4]);
    }

    /// Ensure that the limit flag functions correctly when running the finder in combination with
    /// the sort flag.
    #[test]
    fn test_run_finder_limit() {
        let test_args: LffArgs = LffArgs {
            directory: String::from("test_resources"),
            sort_method: Some(SortMethod::Size),
            limit: Some(3),
            ..BASE_ARGS
        };
        let mut test_printer: LffTestPrinter = LffTestPrinter::default();

        run_finder!(test_args, &mut test_printer).unwrap();
        // We expect only the three largest of the test files to have been output.
        assert_eq!(3, test_printer.0.len());
        assert_eq!(
            "1183  \"test_resources/.hidden_dir/spider.txt\"",
            test_printer.0[0]
        );
        assert_eq!("544   \"test_resources/snow.txt\"", test_printer.0[1]);
        assert_eq!("329   \"test_resources/visible/mud.md\"", test_printer.0[2]);
    }

    /// Ensure that the correct message is output when no matching files are found.
    #[test]
    fn test_run_finder_no_files() {
        let test_args: LffArgs = LffArgs {
            directory: String::from("test_resources"),
            // Naturally we don't have any test files at 100 MiB or more.
            min_size_mib: 100.0,
            ..BASE_ARGS
        };
        let mut test_printer: LffTestPrinter = LffTestPrinter::default();
        run_finder!(test_args, &mut test_printer).unwrap();
        // Check that the correct output has been 'printed'.
        assert_eq!(NO_FILES_FOUND_STR, test_printer.0[0]);
    }

    /// Ensure that the correct error message is generated when the finder is run against a
    /// non-existent directory.
    #[test]
    fn test_run_finder_invalid_dir() {
        let test_args: LffArgs = LffArgs {
            directory: String::from("this is not real"),
            ..BASE_ARGS
        };
        let dir_err: Report = run_finder!(test_args).unwrap_err();
        assert_eq!(
            "Invalid supplied start directory: 'this is not real'",
            dir_err.to_string()
        );
    }
}

/*
TODOS
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
