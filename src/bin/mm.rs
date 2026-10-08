use anyhow::{Context, Result};
use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use jluszcz_finance_utils::backup::cli::{self as backup, BackupArgs};
use jluszcz_finance_utils::cli::CommonArgs;
use jluszcz_finance_utils::report::cli::{self as report_cli, ReportArgs};
#[cfg(feature = "import")]
use mistermanager::import;
use mistermanager::{BACKUP, balance_history, config, db, mix, report, tui};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mm", about = "MisterManager")]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
    /// Scramble every dollar figure's digits, for showing the application to
    /// someone.
    ///
    /// Not global, unlike the three above: it changes what the screens draw
    /// and nothing else. `mm report` refuses it rather than ignoring it --
    /// the scramble is the TUI's, installed before its first frame, so it
    /// would never reach a page written without one.
    #[cfg(feature = "demo")]
    #[arg(long)]
    demo: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

impl Cli {
    /// Whether this run is a demo. Always `false` without the `demo`
    /// feature, where there is no flag to read: a build that cannot install
    /// the mask must not have a way to ask for it.
    #[cfg(feature = "demo")]
    fn demo(&self) -> bool {
        self.demo
    }

    #[cfg(not(feature = "demo"))]
    fn demo(&self) -> bool {
        false
    }
}

#[derive(Subcommand)]
enum Command {
    /// Load a Money.xlsx workbook into the database.
    ///
    /// Behind the `import` feature: a build without it has no importer to
    /// call, so it offers no subcommand that would only fail.
    #[cfg(feature = "import")]
    Import {
        workbook: PathBuf,
        /// Overwrite previously imported data instead of refusing to run.
        /// Without this flag, importing into a database that already holds
        /// transactions or goals fails rather than doubling every row.
        #[arg(long)]
        replace: bool,
    },
    /// Write the HTML report without opening the application.
    Report(ReportArgs),
    /// Refresh fund compositions from SEC's latest N-PORT filings.
    ///
    /// Not behind a feature, unlike `import`: fetching a public filing is an
    /// ordinary network read, not the spreadsheet parser a default build
    /// deliberately omits.
    Mixes {
        /// Refresh only this ticker. Without it, every ticker any holding
        /// names is refreshed.
        #[arg(long)]
        ticker: Option<String>,
    },
    /// Back the database up to S3, if the schedule says one is due.
    Backup(BackupArgs),
}

fn main() -> Result<()> {
    let cli: Cli = jluszcz_finance_utils::cli::parse(config::APP, db::FILE_NAME, true);
    let demo = cli.demo();
    let scratch = cli.common.scratch;
    let is_explicit_backup = matches!(cli.command, Some(Command::Backup(_)));
    if is_explicit_backup {
        cli.common.refuse_scratch_backup()?;
    }
    // `db::snapshot` opens nothing through `db::open`, so a scratch copy keeps
    // the schema version the original has and this run is the one that
    // migrates it.
    let path = cli.common.db_path(
        BACKUP.app,
        || jluszcz_finance_utils::config::data_path(config::APP, db::FILE_NAME),
        db::snapshot,
    )?;
    if scratch {
        eprintln!("scratch database: {}", path.display());
    }
    // The copy's own directory: a scratch run's page goes there, beside the
    // database it was rendered from, never over the real one.
    let scratch_dir = cli.common.scratch_dir(&path);
    let today = cli.common.today_or_local();
    let real_today = cli.common.today.is_none().then_some(today);

    let config_path = cli.common.config_path(config::APP)?;
    // Before the TUI opens: a config file that does not parse should say so
    // on a terminal that is still in its normal mode. `mm report --dir` reads
    // nothing from it, so a broken file does not stop that run.
    let cfg = match &cli.command {
        Some(Command::Report(ReportArgs { dir: Some(_) })) => config::Config::default(),
        _ => config::load(&config_path)?,
    };

    match cli.command {
        // No subcommand launches the application. `--db` and `--today` are
        // global, so the TUI honors them exactly as the importer does.
        None => {
            let sec_contact = cfg.sec.as_ref().map(|s| s.contact.clone());
            let db = db::open(&path)?;
            // Before the TUI, so the months the ledger can recover are on
            // record whether or not anything in the session goes on to write.
            take_snapshot(&db, real_today);
            let db = tui::run(db, today, demo, sec_contact)?;
            // Again on the way out, so this month holds what the session left
            // it at rather than what it opened on.
            take_snapshot(&db, real_today);
            // Never fatal: someone who has already quit should not be told the
            // application broke because a synced folder was unmounted. A demo
            // run writes no page anywhere, scratch directory included.
            report_cli::after_quit(report_cli::on_quit(
                demo,
                scratch_dir.as_deref(),
                |dir| report::write(&db, dir, today),
                || report::write_if_enabled(&db, &cfg, today, demo),
            ));
        }
        #[cfg(feature = "import")]
        Some(Command::Import { workbook, replace }) => {
            let db = db::open(&path)?;
            match import::import_all(&db, &workbook, today, replace)? {
                // The Savings sheet names its two blocks by position and
                // carries no account code, so the first import against an
                // empty database can only get as far as the accounts. Said
                // here rather than left as a healthy exit code over a
                // database with no goals in it.
                import::Report::AccountsOnly { accounts } => {
                    println!("imported {accounts} accounts");
                    println!(
                        "next: open the app, press 0, and set which Savings block each \
                         container account holds -- then re-run this same command, with no flag"
                    );
                }
                import::Report::Full(report) => {
                    // A `--replace` clears the ledger months, and they are the
                    // new ledger's to fill rather than the next launch's.
                    take_snapshot(&db, real_today);
                    print_full(&report)
                }
            }
        }
        Some(Command::Report(args)) => {
            // The mask lives in the TUI and nothing here installs it, so the
            // flag would silently write the real figures it exists to mask.
            if demo {
                anyhow::bail!(
                    "--demo cannot be honoured here: no subcommand installs the mask, so \
                     the page would carry the figures the flag exists to mask. Drop the \
                     flag, or quit the app with --demo, which writes no report at all"
                );
            }
            let db = db::open(&path)?;
            // Never the config's "off": an unset `[report]` section means the
            // owner does not want a page written behind every quit, which is
            // a different question from the one `mm report` asks.
            let dir = report_cli::dir(
                &args,
                scratch_dir.as_deref(),
                cfg.report.as_ref(),
                &config_path,
            )?;
            // Asked for outright, so a failure is an error exit rather than a
            // line on stderr, exactly as an explicit `mm backup` is.
            println!(
                "{}",
                report_cli::describe(&report::write(&db, &dir, today)?)
            );
        }
        Some(Command::Mixes { ticker }) => {
            let db = db::open(&path)?;
            // Named rather than defaulted: SEC refuses a request declaring no
            // contact, and the repository may hold no real address, so the
            // contact is configuration a run must supply.
            let contact = cfg
                .sec
                .as_ref()
                .with_context(|| {
                    format!(
                        "no [sec] contact configured in {} -- {}",
                        config_path.display(),
                        config::ADD_SEC_CONTACT
                    )
                })?
                .contact
                .clone();
            // Uppercased and trimmed where it is typed, the rule the Funds
            // form's `commit` already answers to: a ticker is the key
            // `fund_mix` is stored under and none of the three places it is
            // one folds case, so a row written under `usm` would be a second
            // composition no holding typed `USM` could ever read. Every
            // other route into `refresh` reads `holding`, where the form has
            // already normalised it; this is the one that does not.
            let tickers = match ticker {
                Some(ticker) => vec![ticker.trim().to_uppercase()],
                None => db::holding::tickers(&db)?,
            };
            let refreshed = mix::refresh(&db, &contact, &tickers)?;
            print_refreshed(&refreshed);
            // The scriptable route, so a run where nothing succeeded says so
            // in its exit code as well as on stderr. A partial run still
            // exits 0 -- some tickers did update, and their compositions are
            // written.
            if refreshed.updated.is_empty() && !refreshed.failed.is_empty() {
                anyhow::bail!(
                    "no ticker was refreshed: all {} failed",
                    refreshed.failed.len()
                );
            }
        }
        // Never opens the database: opening creates and seeds a missing
        // file, and a mistyped `--db` would then be uploaded as a backup.
        Some(Command::Backup(args)) => {
            backup::command(&BACKUP, &path, cfg.backup.as_ref(), &args, db::snapshot)?
        }
    }

    // Both of the other arms fall through to here, so an `mm import` gets the
    // same scheduled check the TUI does. It skips a `--db` run, which matters
    // here: the importer's documented dry-run points `--db` at a scratch file.
    if !is_explicit_backup {
        cli.common
            .scheduled_backup(&BACKUP, &path, cfg.backup.as_ref(), db::snapshot);
    }
    Ok(())
}

/// A snapshot is a chart's history, not a figure anything spends, so a failed
/// one warns rather than keeping the app closed or failing an import that has
/// already committed. Any ledger month it missed is recovered by the next run.
///
/// `None` under `--today`, which skips it: the snapshot writes the month it
/// is handed as the current one, so a pretended date would rewrite a month
/// that is over -- fund history included, which nothing can rebuild -- or
/// record one that has not happened. `--db` is not a reason to skip, since the
/// snapshot lands in the database it names.
fn take_snapshot(db: &db::Db, real_today: Option<NaiveDate>) {
    let Some(today) = real_today else {
        return;
    };
    if let Err(e) = balance_history::take(db, today) {
        eprintln!("snapshot failed: {e:#}");
    }
}

fn print_refreshed(refreshed: &mix::Refreshed) {
    if !refreshed.updated.is_empty() {
        println!("updated {}", refreshed.updated.join(", "));
    }
    for (ticker, error) in &refreshed.failed {
        eprintln!("failed to refresh {ticker}: {error}");
    }
}

#[cfg(feature = "import")]
fn print_full(report: &import::Full) {
    println!(
        "imported {} cash rows, {} credit rows",
        report.ledger.cash_rows, report.ledger.credit_rows
    );
    println!(
        "imported {} goals, {} buckets, {} recurring goals",
        report.savings.goals, report.savings.buckets, report.savings.recurring_goals
    );
    for line in &report.ledger.skipped {
        eprintln!("skipped: {line}");
    }
}
