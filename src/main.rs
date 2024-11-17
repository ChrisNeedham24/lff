use clap::Parser;
use eyre::Result;
use lff::{self, LffArgs, LffEyreHandler, LffStdoutPrinter};

/// The main function of `lff`.
///
/// # Errors
/// - If there is an issue setting our custom eyre handler.
/// - If there is an issue running the finder in [lff::run_finder].
#[cfg(not(tarpaulin_include))]
fn main() -> Result<()> {
    // Set the eyre handler to be our custom one before running the finder.
    eyre::set_hook(Box::new(|_| Box::new(LffEyreHandler)))?;
    let args: LffArgs = LffArgs::parse();
    lff::run_finder(args, &mut LffStdoutPrinter)
}
