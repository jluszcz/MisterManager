use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, Utc};
use clap::{Parser, Subcommand};
use mistermanager::{backup, config, db, import, tui};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "mm", about = "MisterManager")]
struct Cli {
    /// Database file. Defaults to ~/.local/share/mistermanager/money.db
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Treat this date as today. Defaults to the system date.
    #[arg(long, global = true)]
    today: Option<NaiveDate>,
    /// Config file. Defaults to ~/.config/mistermanager/config.toml
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Block every dollar figure out, for showing the application to someone.
    ///
    /// Not global, unlike the three above: it changes what the screens draw
    /// and nothing else, and no subcommand prints a figure for it to block.
    #[arg(long)]
    demo: bool,
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
    /// Back the database up to S3, if the schedule says one is due.
    Backup {
        /// Upload even if the last backup is recent enough.
        #[arg(long)]
        force: bool,
        /// Print when the last backup ran and when the next is due, then exit.
        /// Refuses `--force`: this arm prints and exits without uploading, so
        /// accepting the flag would be accepting an instruction it drops.
        #[arg(long, conflicts_with = "force")]
        status: bool,
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
    // Whether the schedule applies is a question about *which* database this
    // is, so it is asked before the option is collapsed into a path.
    let is_default_db = cli.db.is_none();
    let path = match cli.db {
        Some(p) => p,
        None => default_db()?,
    };
    let db = db::open(&path)?;
    let today = cli.today.unwrap_or_else(|| Local::now().date_naive());

    let config_path = match cli.config {
        Some(p) => p,
        None => config::default_path()?,
    };
    // Before the TUI opens: a config file that does not parse should say so
    // on a terminal that is still in its normal mode.
    let cfg = config::load(&config_path)?;
    let state_path = backup::state::default_path()?;

    let is_explicit_backup = matches!(cli.command, Some(Command::Backup { .. }));
    match cli.command {
        // No subcommand launches the application. `--db` and `--today` are
        // global, so the TUI honors them exactly as the importer does.
        None => tui::run(db, today, cli.demo)?,
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
        Some(Command::Backup { force, status }) => {
            if status {
                print_backup_status(&cfg, &state_path)?;
            } else {
                // Explicitly asked for, so a failure is an error exit rather
                // than a line on stderr.
                let outcome = backup::run_if_due(&path, &cfg, &state_path, Utc::now(), force)?;
                print_backup(&outcome);
            }
        }
    }

    // Both of the other arms fall through to here. An `mm import` gets the
    // same scheduled check the TUI does, for free.
    //
    // Only ever the default database, though. `--db` names a copy -- the
    // importer's own documented dry-run points it at a scratch file -- and one
    // of those reaching S3 would land under a key indistinguishable from a real
    // backup *and* stamp the one state file, suppressing the real database's
    // next upload for a whole interval. An explicit `mm backup` still uploads
    // whatever it was pointed at, because it was asked to.
    if !is_explicit_backup && is_default_db {
        scheduled_backup(&path, &cfg, &state_path);
    }
    Ok(())
}

/// Never fatal. Someone who has already quit the application should not be
/// told it broke because the wifi did -- and the schedule stays due, so the
/// next run tries again.
fn scheduled_backup(db_path: &Path, cfg: &config::Config, state_path: &Path) {
    match backup::run_if_due(db_path, cfg, state_path, Utc::now(), false) {
        Ok(backup::Outcome::BackedUp { key, bytes }) => {
            println!("backed up {} to {key}", backup::human_bytes(bytes));
        }
        // Silent: nothing happened, and this runs after every single quit.
        Ok(_) => {}
        Err(e) => eprintln!("backup failed: {e:#}"),
    }
}

fn print_backup(outcome: &backup::Outcome) {
    match outcome {
        backup::Outcome::Disabled => {
            println!("backups are not configured: no [backup] section in the config file");
        }
        backup::Outcome::NotDue { next } => {
            println!("not due until {}", next.format("%Y-%m-%d %H:%M UTC"));
        }
        backup::Outcome::BackedUp { key, bytes } => {
            println!("backed up {} to {key}", backup::human_bytes(*bytes));
        }
    }
}

fn print_backup_status(cfg: &config::Config, state_path: &Path) -> Result<()> {
    let Some(backup_cfg) = cfg.backup.as_ref() else {
        println!("backups are not configured: no [backup] section in the config file");
        return Ok(());
    };
    println!(
        "bucket {}, prefix {}, profile {}, every {} days",
        backup_cfg.bucket,
        backup::PREFIX,
        backup_cfg.profile,
        backup_cfg.interval_days
    );
    // Matches `run_if_due`: the state file is advisory where a `setting` key is
    // binding, so an unreadable one is a warning and "never backed up" rather than
    // an error exit -- `--status` must not fail more strictly than the scheduled
    // check it is reporting on.
    let state = match backup::state::read(state_path) {
        Ok(state) => state,
        Err(e) => {
            eprintln!("ignoring unreadable backup state: {e:#}");
            None
        }
    };
    match state {
        None => println!("never backed up"),
        Some(state) => {
            println!(
                "last {} ({})",
                state.last_backup_at.format("%Y-%m-%d %H:%M UTC"),
                state.last_key
            );
            println!(
                "next {}",
                backup::next_due(state.last_backup_at, backup_cfg.interval_days)
                    .format("%Y-%m-%d %H:%M UTC")
            );
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
