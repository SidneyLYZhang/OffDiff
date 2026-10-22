use clap::Parser;

use offdiff::cli::Cli;

fn main() {
    let cli = Cli::parse();
    if let Err(error) = offdiff::run(cli) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
