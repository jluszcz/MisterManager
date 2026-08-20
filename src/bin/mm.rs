use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use clap::{Parser, Subcommand};
use mistermanager::{db, import, tui};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mm", about = "MisterManager")]
struct Cli {
    /// Database file. Defaults to ~/.local/share/mistermanager/money.db
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Treat this date as today. Defaults to the system date.
    #[arg(long, global = true)]
    today: Option<NaiveDate>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Load a Money.xlsx workbook into the database.
    Import {
        workbook: PathBuf,
        /// Overwrite previously imported data instead of refusing to run.
        /// Without this flag, importing into a database that already holds
        /// transactions or goals fails rather than doubling every row.
        #[arg(long)]
        replace: bool,
    },
}

fn default_db() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    let dir = PathBuf::from(home).join(".local/share/mistermanager");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("money.db"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = match cli.db {
        Some(p) => p,
        None => default_db()?,
    };
    let db = db::open(&path)?;
    let today = cli.today.unwrap_or_else(|| Local::now().date_naive());

    match cli.command {
        // No subcommand launches the application. `--db` and `--today` are
        // global, so the TUI honors them exactly as the importer does.
        None => tui::run(db, today)?,
        Some(Command::Import { workbook, replace }) => {
            match import::import_all(&db, &workbook, today, replace)? {
                // The Savings sheet names its two blocks by position and
                // carries no account code, so the first import against an
                // empty database can only get as far as the accounts. Said
                // here rather than left as a healthy exit code over a
                // database with no goals in it.
                import::Report::AccountsOnly { accounts } => {
                    println!("imported {accounts} accounts");
                    println!(
                        "next: open the app, press 9, and set which Savings block each \
                         container account holds -- then re-run this same command, with no flag"
                    );
                }
                import::Report::Full(report) => print_full(&report),
            }
        }
    }
    Ok(())
}

fn print_full(report: &import::Full) {
    println!(
        "imported {} cash rows, {} credit rows",
        report.ledger.cash_rows, report.ledger.credit_rows
    );
    println!(
        "imported {} goals, {} buckets, {} recurring goals",
        report.savings.goals, report.savings.buckets, report.savings.recurring_goals
    );
    println!("imported {} fund rows", report.funds);
    // No birth date on record leaves every fund row a frozen RemainderShare
    // -- including what would otherwise track age -- and the Funds screen's
    // birth-date prompt can never open for that data, so the collapse would
    // otherwise be invisible.
    if report.fund_targets_frozen {
        eprintln!(
            "no birth date on record: every fund row was stored as a fixed share, \
             including what would otherwise track age -- set the birth date and \
             re-import with --replace, or fix the row's kind on the Funds screen with E"
        );
    }
    for line in &report.ledger.skipped {
        eprintln!("skipped: {line}");
    }
}
