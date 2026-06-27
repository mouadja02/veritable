use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "vrtb",
    version,
    about = "a local and cross-database result-set comparison engine"
)]
struct CLI {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    // put subcommands here
}

fn main() {
    let _cli = CLI::parse();
}
