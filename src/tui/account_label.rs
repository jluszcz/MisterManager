//! An account on its way to a terminal.
//!
//! [`crate::account_label::Account`] carries an account's text and its color together
//! and will not hand over one without the other. This module is where that
//! pair becomes ratatui: [`account_cell`] for a table cell and [`label_line`]
//! for a title, and nothing else in `tui` may draw an account.

use crate::account_label::{Account, Label, Segment};
use ratatui::text::Line as TextLine;
use ratatui::widgets::Cell;

use super::style;

/// A cell naming an account, colored by [`style::palette`].
///
/// One of this module's two exits, and it offers no way to skip the color.
pub(super) fn account_cell(account: &Account) -> Cell<'static> {
    account.render_with(|text, color| {
        super::tinted(
            TextLine::from(text.to_string()),
            Some(style::palette(color)),
        )
    })
}

/// A label as a line of spans, with every account segment in its own color
/// and nothing else colored at all.
///
/// The second of this module's two exits, beside [`account_cell`], and the
/// same guarantee: an account that reaches a screen through here is colored.
pub(super) fn label_line(label: &Label) -> TextLine<'static> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    TextLine::from(
        label
            .segments()
            .iter()
            .map(|segment| match segment {
                Segment::Plain(text) => Span::raw(text.clone()),
                Segment::Account(account) => account.render_with(|text, color| {
                    Span::styled(text.to_string(), Style::default().fg(style::palette(color)))
                }),
            })
            .collect::<Vec<Span<'static>>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AccountId;
    use crate::db::account::{self, AccountColor, Group};
    use crate::test_support::cash;

    fn accounts() -> Vec<account::Account> {
        vec![
            account::Account {
                group: Group::Checking,
                ..cash(1, "CHK")
            },
            account::Account {
                color: Some(AccountColor::Teal),
                ..cash(2, "NST")
            },
        ]
    }

    /// The whole guarantee, at the one place it is discharged: the cell an
    /// account draws into is colored, always, with no way for a caller to ask
    /// for an uncolored one.
    #[test]
    fn every_account_cell_is_colored() {
        let all = accounts();
        for id in [AccountId(1), AccountId(2), AccountId(99)] {
            assert!(cell_color(&Account::named(&all, id)).is_some(), "{id}");
        }
    }

    /// The owner's choice outranks the derived shade here exactly as it does
    /// in `style::account_color` -- this is the plumbing, not a second
    /// decision about what an account looks like.
    #[test]
    fn a_chosen_color_reaches_the_cell() {
        assert_eq!(
            cell_color(&Account::named(&accounts(), AccountId(2))),
            Some(style::account_color(AccountId(2), Some(AccountColor::Teal)))
        );
    }

    /// Reads the foreground the cell's first glyph draws in.
    ///
    /// Through a rendered buffer rather than off the `Cell`, because a
    /// `Cell`'s spans are private -- and because the buffer is what the
    /// terminal actually shows, which is the thing under test. The same trick
    /// `savings.rs` and `recurring_txn.rs` already use to read a column's
    /// color back.
    fn cell_color(account: &Account) -> Option<style::Color> {
        use ratatui::layout::{Constraint, Rect};
        use ratatui::widgets::{Row, Table, Widget};
        let area = Rect::new(0, 0, 20, 1);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        Table::new(
            vec![Row::new(vec![account_cell(account)])],
            [Constraint::Length(20)],
        )
        .render(area, &mut buffer);
        buffer[(0, 0)].style().fg
    }

    /// Exactly the account segments are colored. A title is mostly chrome, and
    /// chrome in an account's color would say the whole line is about that
    /// account rather than the two words that are.
    #[test]
    fn only_the_account_segments_of_a_label_are_colored() {
        let all = accounts();
        let label = Label::plain("Savings · ").account(Account::named(&all, AccountId(2)));
        let colors: Vec<Option<style::Color>> = label_line(&label)
            .spans
            .iter()
            .map(|s| s.style.fg)
            .collect();
        assert_eq!(
            colors,
            vec![
                None,
                Some(style::account_color(AccountId(2), Some(AccountColor::Teal)))
            ]
        );
    }

    /// A title naming nothing is still a Label, so one function draws every
    /// title rather than one for the plain ones and one for the rest.
    #[test]
    fn a_label_with_no_account_draws_as_plain_text() {
        let line = label_line(&Label::from("Edit goal"));
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].style.fg, None);
    }

    /// `AccountName::as_str` is the one way past `Account`, and it exists for the
    /// uses that are not displays of an account -- a description prefill, a
    /// search filter folding case, a form seeding its editable field. Every one
    /// of those is listed here, so a new one has to be added deliberately and is
    /// visible in the diff that adds it. [`Label::plain_text`] is the second
    /// escape, gated the same way: legitimate where the flattened text is
    /// never drawn, and listed for the same reason.
    ///
    /// A source scan rather than a type, because the property is "nobody reached
    /// for the escape hatch", which no signature can state -- and it is purely
    /// textual, so a reflow that split `a.name.as_str()` onto its own line, or
    /// hid it behind `let n = &a.name; n.as_str()`, would pass both clauses with
    /// nothing rewritten. It is the weakest link in the guarantee and says so.
    #[test]
    fn nothing_that_draws_an_account_reads_its_name_as_bare_text() {
        let sanctioned = [
            // `crate::account_label`'s own constructors. They are the one place the
            // text is meant to be read: every `Account` in the crate is built
            // here, and each of these reads is immediately paired with a
            // color. A bare read anywhere else in that file is still a leak,
            // which is why the file is scanned rather than skipped.
            ("account_label.rs", "|a| a.name.as_str().to_string()"),
            ("account_label.rs", "|a| a.code.as_str().to_string()"),
            ("account_label.rs", "let code = a.code.as_str();"),
            ("account_label.rs", "let name = a.name.as_str();"),
            ("account_label.rs", "format!(\"{} — {}\", row.name.as_str()"),
            // `container_names`, which names the plug's containers in
            // `diagnose`'s text report, and the `diagnose` header above it
            // that counts a container's goals. Both are prose the Planning
            // screen prints as prose.
            ("transfer.rs", "account::get(db, *id)?.name.as_str()"),
            ("transfer.rs", "account::get(db, id)?.name.as_str()"),
            // Names the goals behind that count -- a goal, not an account,
            // and outside this guarantee entirely. The scan matches text
            // rather than types, so a goal row reads exactly like an account
            // row and has to be sanctioned by hand.
            ("transfer.rs", "|g| g.name.as_str()"),
            // `Wiring`'s `Container` and the `Row::Transfer` beside it. Both
            // are real displays, and both keep the color in a field of their
            // own for the Planning screen to tint through `planning::Tint` --
            // see `src/tui/CLAUDE.md`'s account-color section. They are the
            // one pair of sanctioned sites where the name and the color are
            // carried apart rather than never separated.
            ("transfer.rs", "name: a.name.as_str().to_string()"),
            ("transfer.rs", "name: account.name.as_str().to_string()"),
            // Seeds the Accounts form's editable Name field -- the owner then
            // owns the text.
            ("tui/accounts.rs", "Field::given(account.name.as_str()"),
            // The destination picker's `Offered.container` -- the first entry
            // in the residual list in `src/tui/CLAUDE.md`'s account-color
            // section.
            ("tui/app/planning.rs", "map_or(\"?\", |a| a.name.as_str())"),
            // The `accounts::Row` `Code` column, deliberately uncolored: the
            // next cell along that row already names the account in color, so
            // tinting the code too would say it twice. The last entry in the
            // residual list in `src/tui/CLAUDE.md`'s account-color section.
            ("tui/app/accounts.rs", "code: account.code.as_str()"),
            // The two status lines reporting a write, transient prose like
            // every other status message sanctioned above -- masked through
            // `crate::demo::text` rather than colored, since the status line
            // is uncolored regardless of a demo.
            ("tui/app/accounts.rs", "new.name.as_str()"),
            ("tui/app/accounts.rs", "edit.name.as_str()"),
            // Prefills a description with the card's code. Not a display of an
            // account: it seeds an editable field the owner then owns.
            ("tui/form.rs", "{} Payment"),
            // Names the source in an error about a transfer to itself. Errors
            // are prose, and the status line is uncolored.
            ("tui/form.rs", "from.code.as_str()"),
            // `Ledger::account_name`, which feeds an `App::status` message --
            // a status message is transient prose.
            ("tui/ledger.rs", "map_or(\"?\", |a| a.name.as_str())"),
            // `Savings::account_name` -- the second entry in the residual
            // list. Its two callers are `open_allocate`'s prefill for the
            // Allocation modal's body and `open_history`, which carries the
            // name into `History` for that modal's `e` to build the same
            // form from -- one display, reached two ways, that stays
            // uncolored only because `AllocationForm` is outside this
            // guarantee. The Unallocated footer used to read through here
            // too and now draws its containers through
            // `Savings::container_account`.
            ("tui/savings.rs", "map_or(\"?\", |a| a.name.as_str())"),
            // `account::checking`'s ambiguity error, which the Planning
            // screen draws in place of the plan. Prose rather than a cell, so
            // it carries no color anywhere -- but it is read by a viewer, so
            // it masks through `crate::demo::text` on the same line.
            (
                "db/account.rs",
                "crate::demo::text(a.name.as_str()).into_owned()",
            ),
            // `insert`'s duplicate-code refusal, which `App::on_key` puts on
            // the status line verbatim. Prose for the same reason, masked the
            // same way.
            ("db/account.rs", "crate::demo::text(existing.code.as_str())"),
        ];

        let mut found: Vec<String> = Vec::new();
        for (file, source) in sink_sources() {
            for (number, line) in source.lines().enumerate() {
                if !line.contains(".name.as_str()") && !line.contains(".code.as_str()") {
                    continue;
                }
                if sanctioned
                    .iter()
                    .any(|(f, needle)| *f == file && line.contains(needle))
                {
                    continue;
                }
                found.push(format!("{file}:{}: {}", number + 1, line.trim()));
            }
        }

        assert!(
            found.is_empty(),
            "an account's text is read as a bare string here, which is how one \
             reaches a screen with no color on it. Draw it through `Account` \
             instead, or add the site to `sanctioned` with a comment saying why \
             it is not a display:\n{}",
            found.join("\n")
        );

        // The second escape: `plain_text()` flattens whatever accounts a
        // `Label` carries, and is meant for the assertions that check
        // wording, not for a draw. This is exactly how `render_value` lost
        // the Reconcile modal field label's tint -- the fix belongs in
        // `field_line_labeled`, not in a longer sanctioned list here.
        let plain_text_sanctioned = [
            // The status line, transient prose like every other status
            // message sanctioned above.
            ("tui/app/planning.rs", "form.label().plain_text().trim()"),
            // The Accounts screen's `Color` field draws its own value through
            // `field_line_tinted`, whose tint names the chosen color rather
            // than an account -- this flattening is that value argument, not
            // a draw of an account.
            ("tui/accounts.rs", "value.plain_text()"),
            // Only the char count is read, to pad a label that may carry an
            // account to the same twelve columns `format!("{label:>12}  ")`
            // pads a plain `&str` label to. `label_line(label)` draws the
            // segments themselves; this flattened copy never reaches the
            // screen, and is taken of exactly the text that is drawn so the
            // measurement and the draw cannot disagree.
            ("tui/form.rs", "label.plain_text().chars().count()"),
            // Where the caret goes is decided against the text the value
            // draws as, and this flattened copy is only ever compared and
            // counted. The spans themselves come from `label_line`, and the
            // caret is laid over one character of one of them with that
            // span's own style kept underneath, so an account keeps its color
            // with the caret sitting in the middle of it.
            ("tui/form.rs", "caret.offset(&value.plain_text())"),
        ];

        let mut leaked: Vec<String> = Vec::new();
        for (file, source) in sink_sources() {
            for (number, line) in source.lines().enumerate() {
                if !line.contains(".plain_text()") {
                    continue;
                }
                if plain_text_sanctioned
                    .iter()
                    .any(|(f, needle)| *f == file && line.contains(needle))
                {
                    continue;
                }
                leaked.push(format!("{file}:{}: {}", number + 1, line.trim()));
            }
        }

        assert!(
            leaked.is_empty(),
            "`Label::plain_text()` flattens an account's color away and is meant \
             for the assertions that check wording, not for a draw. Draw \
             through `label_line` instead, or add the site to \
             `plain_text_sanctioned` with a comment saying why it is not a \
             draw:\n{}",
            leaked.join("\n")
        );
    }

    /// Every file the scan reads, as (path below `src/`, the file's production
    /// code).
    ///
    /// Five roots, because the guarantee has five parts:
    /// `src/account_label.rs`, which owns [`crate::account_label::Account`]
    /// and is the one place its text is meant to be read; `src/tui/`, the
    /// terminal sink; `src/report/`, the html one; `src/transfer.rs`,
    /// which is neither sink but builds display rows both of them draw; and
    /// `src/db/`, where a query module's own refusals name accounts in prose
    /// a screen prints verbatim. A scan of the screens alone would have gone
    /// on passing while a `format!("{}", a.name.as_str())` in `report::html`
    /// flattened an account's color away -- the exact bug this exists to make
    /// unwritable. `transfer.rs` is the same argument one layer further out:
    /// a policy module that hands a name downstream is as able to drop the
    /// color as the screen that draws it, and `Wiring` and `Row::Transfer`
    /// both carry the name and the color as separate fields. `src/db/` is
    /// that argument at its limit: `account::checking`'s ambiguity error is
    /// drawn by the Planning screen in place of the plan, so a name read
    /// there reaches a viewer with no color and, until it was masked, with no
    /// pseudonym either.
    ///
    /// The key is the path below `src/` rather than the file name, since
    /// `tui/ledger.rs` and `report/html/ledger.rs` are two different files and
    /// a sanctioned line in one must not excuse the same text in the other.
    ///
    /// Every file is truncated just before its `#[cfg(test)] mod tests`
    /// declaration, so the scan never sees a test module. The cut is the
    /// module, not the first `#[cfg(test)]` line: `mod.rs` gates two helper
    /// functions, `ends_in_order` and `column_of`, the same way ahead of its
    /// own test module, and cutting at the first would have left `run` and
    /// `event_loop` -- the event loop itself -- unscanned along with them.
    /// Without the cut landing at the module at all, this test fails on its
    /// own fixtures: `app/`, `savings.rs`, `worksheet.rs`, `picker.rs` and
    /// `recurring_goal.rs` all have test helpers that read `.name.as_str()` on
    /// goal rows, worksheet lines or picker entries -- none of them an
    /// account. This file's own fixture above is the same shape, and is read
    /// like any other: it constructs `db::account::Account` rows directly
    /// rather than going through `.name.as_str()`/`.code.as_str()`, so it
    /// carries nothing for the scan to flag.
    fn sink_sources() -> Vec<(String, String)> {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        let mut pending = vec![
            src.join("account_label.rs"),
            src.join("transfer.rs"),
            src.join("tui"),
            src.join("report"),
            src.join("db"),
        ];
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                for entry in std::fs::read_dir(&path).expect("a readable dir") {
                    pending.push(entry.expect("a readable dir entry").path());
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let key = path
                .strip_prefix(&src)
                .expect("a path below src")
                .to_str()
                .expect("a nameable path")
                .to_string();
            let contents = std::fs::read_to_string(&path).expect("a readable file");
            let production = match contents.find("#[cfg(test)]\nmod tests {") {
                Some(index) => &contents[..index],
                None => contents.as_str(),
            };
            out.push((key, production.to_string()));
        }
        out
    }
}
