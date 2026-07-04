pub mod format;
pub mod spec;

use clap::{Parser, Subcommand};

use vrtb_core::build_plan;
use vrtb_core::conformance::{Verdict, conformance_check};
use vrtb_core::error::{Result, VeritableError};

use format::Format;
use spec::{build_engine, parse_target};

#[derive(Parser)]
#[command(
    name = "vrtb",
    version,
    about = "a local and cross-database result-set comparison engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Row-level diff (not yet implemented).
    Diff {
        #[arg(short, long)]
        src: String,
        #[arg(short, long)]
        dst: String,
        #[arg(short, long)]
        key: String,
        #[arg(short, long)]
        columns: Option<Vec<String>>,
        #[arg(short, long, default_value_t = Format::Human)]
        format: Format,
    },

    /// Whole-table checksum + count comparison of two tables.
    Check {
        #[arg(short, long)]
        src: String,
        #[arg(short, long)]
        dst: String,
        #[arg(short, long)]
        key: String,
        #[arg(short, long)]
        columns: Option<Vec<String>>,
        #[arg(short, long, default_value_t = Format::Human)]
        format: Format,
    },

    /// Alias of `check`, framed as a cross-engine conformance assertion.
    Conformance {
        #[arg(short, long)]
        src: String,
        #[arg(short, long)]
        dst: String,
        #[arg(short, long)]
        key: String,
        #[arg(short, long)]
        columns: Option<Vec<String>>,
        #[arg(short, long, default_value_t = Format::Human)]
        format: Format,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => std::process::ExitCode::from(code),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::ExitCode::from(e.exit_code())
        }
    }
}

fn run(cli: Cli) -> Result<u8> {
    match cli.command {
        Commands::Check {
            src,
            dst,
            key,
            columns,
            format,
        }
        | Commands::Conformance {
            src,
            dst,
            key,
            columns,
            format,
        } => run_checksum(&src, &dst, &key, columns.as_deref(), format),
        Commands::Diff { .. } => Err(VeritableError::Engine(
            "diff is not implemented yet (joindiff/hashdiff are stubs)".into(),
        )),
    }
}

// Build both engines, introspect, plan the shared columns, run the whole-table
// checksum comparison, and emit the verdict. Exit code: 0 = match, 1 = differ.
fn run_checksum(
    src: &str,
    dst: &str,
    key: &str,
    columns: Option<&[String]>,
    format: Format,
) -> Result<u8> {
    let src_target = parse_target(src)?;
    let dst_target = parse_target(dst)?;

    let src_engine = build_engine(&src_target.spec)?;
    let dst_engine = build_engine(&dst_target.spec)?;

    let src_schema = src_engine.introspect(&src_target.table)?;
    let dst_schema = dst_engine.introspect(&dst_target.table)?;
    let plan = build_plan(&src_schema, &dst_schema, columns, key)?;

    let verdict = conformance_check(
        src_engine.as_ref(),
        &src_target.table,
        dst_engine.as_ref(),
        &dst_target.table,
        &plan,
        format,
    )?;

    emit(&verdict, format);
    Ok(if verdict.is_match() { 0 } else { 1 })
}

fn emit(verdict: &Verdict, format: Format) {
    match format {
        Format::Human => match verdict {
            Verdict::Match => println!("MATCH — count and checksum agree on both sides"),
            Verdict::Differ { src, dst } => {
                println!("DIFFER");
                println!(
                    "  src: count={} h1={} h2={}",
                    src.count, src.sum_h1, src.sum_h2
                );
                println!(
                    "  dst: count={} h1={} h2={}",
                    dst.count, dst.sum_h1, dst.sum_h2
                );
            }
        },
        Format::Summary => println!(
            "{}",
            if verdict.is_match() {
                "match"
            } else {
                "differ"
            }
        ),
        Format::Json | Format::Jsonl => println!("{}", verdict_json(verdict)),
    }
}

fn verdict_json(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Match => r#"{"result":"match"}"#.to_string(),
        Verdict::Differ { src, dst } => format!(
            r#"{{"result":"differ","src":{{"count":{},"h1":"{}","h2":"{}"}},"dst":{{"count":{},"h1":"{}","h2":"{}"}}}}"#,
            src.count, src.sum_h1, src.sum_h2, dst.count, dst.sum_h1, dst.sum_h2
        ),
    }
}
