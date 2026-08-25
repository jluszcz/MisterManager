# Demo mode: scrambled figures, pseudonymous names, behind a feature

`mm --demo` today replaces every absolute dollar figure with six blocks and leaves every name
alone. It hides what it is asked to hide, and what it costs is the demonstration: a screen of
`██████` shows the app's layout and nothing about the app's *work*, and the owner's account and
goal names — the things a screenshot is least able to explain away — are drawn in full.

This replaces the mask with an obfuscation. Every digit of every absolute figure becomes another
digit, and every word of every owner-entered string becomes a pronounceable pseudoword of the same
length. The screens read as a real ledger belonging to nobody. The whole of it compiles only under
a new `demo` Cargo feature, off by default, so a shipped build carries neither the flag nor the
code behind it.

## The two decisions this rests on

**Magnitude and length are published on purpose.** The fixed-width mask exists because the number
of digits is itself a figure: a mask keeping the shape of `1,234.56` hides the balance and
publishes its order of magnitude. Scrambling per digit gives that up, and so does a
length-preserving pseudoword — a four-letter account name stays four letters. That is the price of
a demonstration that looks like the application working, and it is paid deliberately rather than
overlooked. It also retires a whole invariant: the mask was six columns wide because that is the
narrowest money column any screen lays out, and a demo now draws in exactly the widths an ordinary
run does.

**This is obfuscation for a demonstration, not a security control.** The salt is per run and never
leaves the process, so a screenshot carries no way back to the figures. It is not a cryptographic
guarantee and nothing should be built on it as one: the mapping is length-preserving, deterministic
within a run, and derived from a hash the standard library is free to change. What it defends
against is the person across the table and the screenshot in a bug report — which is what
`--demo` was ever for.

Derived figures stop reconciling, and that is accepted: a scrambled Net is not the sum of the
scrambled bands above it. Nothing on any screen claims to be arithmetic performed in front of the
viewer, and the alternative — scrambling one figure and re-deriving the rest — would leak every
relationship in the waterfall.

## The feature gate

```toml
[features]
demo = []
```

`src/demo.rs` becomes `src/demo/mod.rs`, holding the same public surface it holds today plus
`text`, with two bodies per function. `src/demo/mask.rs` — the salt, the digit rule, the pseudoword
generator — is `#[cfg(feature = "demo")]` and is not compiled otherwise.

```rust
pub fn figure(cents: Cents) -> String {
    #[cfg(feature = "demo")]
    if let Some(salt) = salt() {
        return mask::figure(salt, cents);
    }
    cents.to_string()
}
```

An attribute on the statement rather than a `cfg`-split function: without the feature the body *is*
the real rendering, the branch is not there to fold away, and every call site across `tui` and
`transfer` is untouched. `tui::run(db, today, demo)` keeps its signature and
`report::write_if_enabled` keeps its parameter, so no layer below the binary learns that the
feature exists.

The flag is the one place the binary does:

```rust
/// Obfuscate every figure and name, for showing the application to someone.
#[cfg(feature = "demo")]
#[arg(long)]
demo: bool,
```

with a `Cli::demo(&self) -> bool` that is `self.demo` under the feature and `false` without it. A
default build rejects `--demo` as an unknown argument, which is the honest answer: the mask is not
in the binary. The `mm report` refusal — the subcommand that must not silently ignore the flag —
gates the same way, since without the feature there is no flag to refuse.

`ci.yml` calls the shared `rust-ci.yml` twice: once as it does now, and once with
`all-features: true`. Both builds are then built, tested and linted — the shipped one, and the one
where the obfuscation exists. Testing only `--all-features` would leave a call site that compiles
solely with the feature on; testing only the default would leave the feature's own tests unrun.

## The salt

One `u64`, drawn once in `install` and held in the thread-local the flag already lives in:

```rust
thread_local! {
    static SALT: Cell<Option<u64>> = const { Cell::new(None) };
}
```

`None` *is* "not a demo", so one cell carries both facts and there is no state where the mask is on
without a salt behind it. It stays a thread-local for the reason the flag already is: `cargo test`
runs each test on its own thread, so a test that turns the mask on cannot be seen by the hundreds
that assert real figures.

The salt comes from `std::hash::RandomState::new().build_hasher().finish()` — the standard
library's own randomly seeded hasher, which is entropy the crate already links against. No new
dependency: `rand` would be a whole crate for one `u64` drawn once per run, and this is
obfuscation rather than key material.

`#[cfg(test)] pub fn install_with_salt(salt: u64)` is what lets a unit test assert an exact output.
It is test-only for the same reason `Account::text` is: a fixed salt in a shipped build is a
reproducible mapping across runs, which is the one property the per-run salt exists to deny.

## The figures

A digit is `hash(salt, key, position) % 10`, where `key` is the `Cents` value the figure came from
and **position is counted from the decimal point** — cents at −1 and −2, dollars at 0, 1, 2 and up.

That keying is what makes a demo coherent rather than noisy:

- **The same amount draws the same everywhere.** Two screens quoting one balance quote one
  scrambled balance.
- **`figure` and `whole_figure` agree.** The Planning screen's whole dollars are the ledger's
  figure with its cents dropped, exactly as they are in an ordinary run, because both scramble the
  dollar digits of the same value at the same positions.
- **Different amounts are unrelated.** The key is the whole value, so this is not a digit-for-digit
  substitution cipher: one figure learned tells nothing about the next.

Everything that is not a digit survives in place — grouping commas, the decimal point, and the
sign. The sign has to: `tui::style::amount_color` still sees the real `Cents` and still paints a
negative red, so dropping the `-` would hide the glyph and leave the fact. The leading digit of a
multi-digit integer part is drawn from `1..=9`, because `0,834` reads as a rendering fault rather
than as money. An exact zero scrambles like anything else — nothing about a real figure survives,
including that it was nothing.

`typed(raw)`, the amount a form field shows, keys on `raw.parse::<Cents>()` when the text parses
and on the raw text when it does not. A form prefilled from a row then shows that row's own
scrambled figure rather than a second unrelated one — under the old mask both drew `██████` and
agreed for free, and a form that disagreed with the row it opened on would read as a bug. An empty
field stays empty: there is no figure there to hide, and obfuscating it would say there was.

Nothing here touches `Percent` or `BasisPoints`. A percentage is a shape rather than a sum — the
fund allocation and the Planning splits are what make those screens worth demonstrating, and they
say nothing about how much there is. Nothing here touches a date (a scrambled date is not a date),
a count (`5 goals` over two rows reads as a fault), a match key, or a parser.

## The strings

```rust
pub fn text(s: &str) -> Cow<'_, str>
```

Borrowed when this is not a demo, which is what keeps it callable in a draw path.

The rule walks the string in runs. A run of alphanumerics is a *word* and is replaced; everything
else — spaces, `—`, `/`, `&`, punctuation — passes through in place. `CHK — Everyday` keeps its em
dash, and the `—` that `description::render` returns for a row with no description has no word in
it and so survives as itself.

Each word maps from `(salt, word.to_lowercase())` to a pseudoword of the same length: consonants
and vowels alternating, drawn from a hash-derived byte stream, so the result is pronounceable.
Case is copied character by character from the original, so `CHK` draws as `MEP` and `Everyday` as
`Kolatabe`. A digit inside a word scrambles as a digit rather than becoming a letter, so `CC1`
still reads as a code and `Lego 2026` as a name with a year in it.

Keying on the lowercased word rather than on the whole string is what makes the screens hang
together: the same word maps the same way everywhere in the run, so `Rainy Day` and `Rainy Fund`
still share a word, and an account named in a title and in a ledger row is recognisably the same
account. The cost is that repetition is visible — two goals sharing a word still share one — which
is a property the workbook's own goal names already have and nothing downstream reads.

## Where the rule reaches

Seven owner-entered text columns, through three kinds of site.

**Two seams cover four of them, in one line each.**

- `account_label::Account::render_with` is already the only reader of an account's text, and it is
  already the type that exists because a `format!` flattening an account is how every uncolored
  title on this app got that way. Obfuscating there covers every account name and code on every
  screen — the tables, the titles, the three form selectors — with no draw site to remember.
- `description::render` is already the one rule for what a description reads as in any medium, so
  transaction and recurring-transaction descriptions are covered where the em dash already is: the
  ledger column, the status line a write reports on, and the delete confirmation. The report calls
  it too and is unaffected, because a `--demo` run never writes one.

**The remaining three take `demo::text` at their draw sites**: goal name, recurring-goal name, bill
label, and fund name, on Savings, the worksheet, the Planning bills block, Funds, the pickers and
the autocomplete popup — and in the prose `transfer::diagnose` builds, which names goals *and*
accounts. That prose is why the module stays at the crate root rather than moving under `tui`, and
it is the same reason the module doc already gives for the figures.

**What is deliberately not a caller:**

- The app's own vocabulary — screen titles, column headers, the Planning line labels (`Bills`,
  `Future Housing`), the cadence words, `Kind::label` and `Group::label`. These are the app's
  words, not the owner's, and a demo with them obfuscated demonstrates nothing.
- The `/` needle and every match key. Search goes on matching the *stored* text, exactly as it
  matches the real amount today: obfuscating a match key turns every row's key into noise and
  leaves the owner unable to find anything mid-demonstration, and the needle is a query being typed
  rather than text off a row.
- Anything a parser or a write can see. Every buffer keeps its real text, `demo::text` is applied
  where a field becomes a `Label` and never to what `Field::given` was handed, so a demo can be
  driven rather than only watched and a form opened on a row commits what was already there.

## Testing

Unit tests in `src/demo/` run against `install_with_salt`, so exact outputs can be asserted, and
cover: a figure's digits all change while its commas, point and sign do not; `figure` and
`whole_figure` of one value agreeing on the dollars; a prefilled `typed` agreeing with its row;
no leading zero on a multi-digit integer part; a zero scrambling; case pattern, length and
digit-vs-letter shape preserved through `text`; the same word mapping the same way twice; two salts
disagreeing.

The two existing sweeps extend from figures to names — `a_demo_leaves_no_figure_on_any_screen`
walks all nine screens and `a_demo_leaves_no_figure_on_any_form_a_row_opens` opens every form over
a row that carries one — and both must go on asserting that something *arrived*, since an absence
check over an empty table passes for free. A third sweep covers the prose paths the two do not
reach: `transfer::diagnose`, the status line, and the autocomplete popup.

The roughly thirty existing tests asserting `██████` become assertions that the real figure is
absent, which is what they were always checking. Every test that installs a demo gates on
`all(test, feature = "demo")`.

## Documentation

Four files, and the honest part goes in writing.

- `README.md`'s Demo mode section: what the run now looks like, and that magnitude and name length
  are published on purpose. It also gains the build instruction, since `--demo` is no longer in a
  default build.
- The root `CLAUDE.md` demo bullet, whose "blocks the figures and nothing else" no longer holds.
- `src/tui/CLAUDE.md`, whose "percentages, dates, counts and names are never masked" invariant
  inverts for names and holds for the other three, and whose fixed-width-mask reasoning about
  column widths is replaced by the simpler fact that a demo now draws in an ordinary run's widths.
- `src/report/CLAUDE.md`, where the wording about the mask being the TUI's needs to say the same
  thing about the feature.

## Out of scope

- **No newtypes for the remaining text columns.** `GoalName`, `BillLabel` and the rest, without
  `Display`, would let the compiler enforce what the sweeps assert. It is the right escalation if a
  sweep ever catches a leak, and it is not worth a diff through `db`, search, import, the report
  and every fixture before one does.
- **No fixed salt in a shipped build.** Reproducible screenshots across runs is a real wish and it
  is exactly the property the per-run salt denies; if it is ever wanted it is a flag of its own,
  argued on its own.
- **The report is unchanged.** A `--demo` run still writes none, for the reason it never did: the
  page it would overwrite is the one nothing can regenerate without quitting an ordinary session.
