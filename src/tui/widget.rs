//! How a form is drawn: the box, the labelled lines inside it, and the caret
//! on the one field that has the focus.
//!
//! Every modal in the app draws through these, whether or not what it is
//! drawing is a form — the key reference panel and the destination chooser
//! want the same centered box — which is why they are a module of their own
//! rather than a half of [`super::form`]. Nothing here holds state: a
//! caller hands over the lines it wants and gets back the area they took.

use super::autocomplete::Autocomplete;
use super::form::Caret;
use super::style::Color;
use super::{Label, label_line};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

/// How wide every form is drawn. One number, because a form that opened at a
/// different width than the one beside it would move its fields under the
/// hand that is already typing into them.
pub(super) const FORM_WIDTH: u16 = 64;

/// A centered rectangle, clamped to `area`.
pub(super) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// One labelled input line; the focused one carries a caret.
pub(super) fn field_line(label: &str, value: Label, caret: Option<Caret>) -> TextLine<'static> {
    field_line_noted(label, value, caret, "")
}

/// One `field_line` per field, in the order the form walks them, with the
/// caret on whichever is focused and a note past the value of whichever
/// carry one.
///
/// Every form built over a field enum is one stack over it, so the shape is
/// stated here and each render says only what its own form has: which
/// fields, how it spells one, and which of them earns a note. That last one
/// is *declared* rather than decided in an `if` inside the map, since which
/// field carries a note is a fact about the form rather than a step in
/// drawing it.
///
/// Three renders stand outside it, for two reasons. The Accounts screen's has
/// the enum and still cannot map over it: its `Color` field draws through
/// [`field_line_tinted`], so it runs a `match` per field instead.
/// [`super::form::ValueForm`] and the Planning screen's transfer confirmation
/// have no field enum to walk at all -- one holds a single `Entry`, and the
/// other is a preview of the ledger rows with one date field under them.
pub(super) fn field_stack<F: Copy + PartialEq>(
    fields: &[F],
    focus: F,
    caret: Caret,
    label: impl Fn(F) -> &'static str,
    display: impl Fn(F) -> Label,
    notes: &[(F, &str)],
) -> Vec<TextLine<'static>> {
    fields
        .iter()
        .map(|f| {
            let note = notes
                .iter()
                .find(|(field, _)| *field == *f)
                .map(|(_, note)| *note)
                .unwrap_or_default();
            field_line_noted(
                label(*f),
                display(*f),
                (focus == *f).then(|| caret.clone()),
                note,
            )
        })
        .collect()
}

/// The same, with a note past the value -- what the field comes to, where its
/// text is an expression rather than the figure itself. An empty note draws
/// nothing, trailing space included.
pub(super) fn field_line_noted(
    label: &str,
    value: Label,
    caret: Option<Caret>,
    note: &str,
) -> TextLine<'static> {
    let mut spans = vec![Span::raw(format!("{label:>12}  "))];
    spans.extend(value_spans(&value, caret));
    spans.push(Span::raw(trailer(note)));
    TextLine::from(spans)
}

/// The same, with the *value* drawn in a color -- the one field whose text is
/// a name for something the form cannot otherwise show. The Accounts screen's
/// `Color` selector cycles eight names, and a name is not a color: drawing
/// `Teal` in teal is what makes the choice answerable without saving it and
/// looking.
///
/// Only the value is tinted. The label and the caret are chrome and belong to
/// the form rather than to the field's content.
pub(super) fn field_line_tinted(
    label: &str,
    value: String,
    focused: bool,
    color: Color,
) -> TextLine<'static> {
    let mut spans = vec![
        Span::raw(format!("{label:>12}  ")),
        Span::styled(value, Style::default().fg(color)),
    ];
    if focused {
        spans.push(Span::styled(PAST_THE_END, caret_style()));
    }
    spans.push(Span::raw(trailer("")));
    TextLine::from(spans)
}

/// The same as [`field_line_noted`], but the label is itself a [`Label`]
/// rather than a plain `&str` -- the Reconcile modal's field label, which
/// carries the same colored account segment its border does two lines above.
///
/// The pad is measured off the label's flattened character count, which is
/// the count `format!("{label:>12}  ")` pads to, so a colored label sits in
/// the same column an uncolored one does and only the color differs.
///
/// Measured off exactly the text that is then drawn, rather than off a
/// trimmed copy of it: a label carrying surrounding space would otherwise be
/// padded for one width and drawn at another, and the two would disagree for
/// whoever wrote that label rather than for whoever wrote this.
pub(super) fn field_line_labeled(
    label: &Label,
    value: Label,
    caret: Option<Caret>,
) -> TextLine<'static> {
    let width = label.plain_text().chars().count();
    let mut spans = vec![Span::raw(" ".repeat(12usize.saturating_sub(width)))];
    spans.extend(label_line(label).spans);
    spans.push(Span::raw("  "));
    spans.extend(value_spans(&value, caret));
    spans.push(Span::raw(trailer("")));
    TextLine::from(spans)
}

/// How the caret is drawn: reverse video over the character it is on, which
/// is the block a terminal's own cursor paints.
///
/// A block *over* a character rather than a bar *between* two of them. A bar
/// costs a column, so every value shifted right of the caret as the caret
/// moved through it, and a field read as though it had a space typed into it.
pub(super) fn caret_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

/// The character the caret sits on at the end of a line. There is nothing
/// typed there to block out, and this is the one place the caret costs a
/// column -- as a terminal's own cursor does, sitting past the last
/// character.
const PAST_THE_END: &str = " ";

/// The value, with the caret drawn onto it at its own offset.
///
/// The value arrives as a [`Label`] because an account is colored wherever it
/// is drawn, so the caret has to be laid over one character of one span
/// without flattening the rest -- and reverse video keeps that span's color,
/// swapping it into the background. `None` is a field that does not have the
/// caret and draws none at all.
pub(super) fn value_spans(value: &Label, caret: Option<Caret>) -> Vec<Span<'static>> {
    let spans = label_line(value).spans;
    let Some(caret) = caret else {
        return spans;
    };
    let at = caret.offset(&value.plain_text());

    let mut out = Vec::with_capacity(spans.len() + 3);
    let mut seen = 0;
    let mut placed = false;
    for span in spans {
        let len = span.content.chars().count();
        if !placed && at < seen + len {
            let (before, on, after) = split_around(&span.content, at - seen);
            if !before.is_empty() {
                out.push(Span::styled(before.to_string(), span.style));
            }
            out.push(Span::styled(
                on.to_string(),
                span.style.patch(caret_style()),
            ));
            if !after.is_empty() {
                out.push(Span::styled(after.to_string(), span.style));
            }
            placed = true;
        } else {
            out.push(span);
        }
        seen += len;
    }
    if !placed {
        out.push(Span::styled(PAST_THE_END, caret_style()));
    }
    out
}

/// `text` split into what precedes the character `at`, that character, and
/// what follows it. Counted in characters, so a multi-byte one is not sliced
/// through the middle.
fn split_around(text: &str, at: usize) -> (&str, &str, &str) {
    let byte = |n: usize| {
        text.char_indices()
            .nth(n)
            .map_or(text.len(), |(index, _)| index)
    };
    let (from, to) = (byte(at), byte(at + 1));
    (&text[..from], &text[from..to], &text[to..])
}

/// The note that follows a field's value, where it has one.
fn trailer(note: &str) -> String {
    if note.is_empty() {
        String::new()
    } else {
        format!("  {note}")
    }
}

/// Draw a form: the centered box, its border and title, and one line per
/// row. Returns the area it took, for the forms that hang an
/// autocomplete popup off the bottom of it.
///
/// The height is the lines themselves plus the border's two rows, which is
/// what lets one function serve a fixed field order, a variable one
/// (`FundForm::fields`), and the forms that add a line of their own past
/// the fields.
pub(super) fn render_fields(
    frame: &mut Frame,
    title: impl Into<Label>,
    lines: Vec<TextLine<'static>>,
) -> Rect {
    let area = centered(frame.area(), FORM_WIDTH, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(label_line(&title.into()))),
        area,
    );
    area
}

/// The suggestion list, drawn under the form. Returns how many suggestion rows
/// it actually drew.
///
/// The form is centered, so on a short terminal this hangs off the bottom and
/// is clipped — at ten rows or fewer only the borders survive. The count is
/// what confines the cursor to the rows the user can see: a suggestion that
/// was not drawn must not be selectable, because applying one writes a
/// description, an amount and an account the user never read.
pub(super) fn render_popup(frame: &mut Frame, form_area: Rect, popup: &Autocomplete) -> usize {
    if !popup.is_open() {
        return 0;
    }
    let area = Rect {
        x: form_area.x,
        y: form_area.y + form_area.height,
        width: form_area.width,
        height: popup.suggestions().len() as u16 + 2,
    }
    .intersection(frame.area());
    // The border spends a row at the top and one at the bottom; whatever is
    // left is how many suggestions the paragraph inside can show.
    let drawn = popup
        .suggestions()
        .len()
        .min(usize::from(area.height.saturating_sub(2)));
    if drawn == 0 {
        // Drawing an empty box titled "Enter or Tab accepts" would advertise
        // keys that, correctly, now do nothing.
        return 0;
    }
    frame.render_widget(Clear, area);
    let lines: Vec<TextLine> = popup
        .suggestions()
        .iter()
        .take(drawn)
        .enumerate()
        .map(|(i, s)| {
            let marker = if i == popup.selected_index() {
                ">"
            } else {
                " "
            };
            TextLine::from(format!(
                "{marker} {}   {}   ×{}",
                crate::demo::text(&s.description),
                crate::demo::figure(s.cents),
                s.uses
            ))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("↑/↓ · Enter or Tab accepts · Esc closes")),
        area,
    );
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::form::Field;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::style::Modifier;
    #[cfg(feature = "demo")]
    use {
        crate::db::AccountId,
        crate::db::txn::Suggestion,
        crate::money::Cents,
        crate::tui::form::{FormFields, ValueForm},
    };

    /// One autocomplete row, for the popup draw below.
    #[cfg(feature = "demo")]
    fn suggestion(description: &str, account_id: AccountId, cents: i64) -> Suggestion {
        Suggestion {
            description: description.to_string(),
            account_id,
            cents: Cents(cents),
            uses: 3,
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// The autocomplete list is a window onto rows already written: each
    /// suggestion carries the amount it would fill in and the description off
    /// the same real transaction, and a demo hides both -- the buffer a `Tab`
    /// accepts one into is untouched either way.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_amounts_and_descriptions_the_autocomplete_list_offers() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        crate::demo::install_with_salt(7);
        let mut popup = Autocomplete::default();
        popup.set(vec![suggestion("Whole Foods", AccountId(1), 12_345)]);

        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal
            .draw(|frame| {
                let area = Rect {
                    x: 0,
                    y: 0,
                    width: 60,
                    height: 3,
                };
                render_popup(frame, area, &popup);
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(!text.contains("123.45"), "the amount survived: {text}");
        assert!(
            text.contains(&crate::demo::figure(Cents(12_345))),
            "no scrambled amount found: {text}"
        );
        assert!(
            !text.contains("Whole Foods"),
            "the description survived: {text}"
        );
        assert!(
            text.contains(&crate::demo::text("Whole Foods").to_string()),
            "no scrambled description found: {text}"
        );
    }

    fn joined(line: &TextLine) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Which characters a line draws in reverse video -- the caret, and
    /// nothing else in the app draws a field that way.
    fn reversed(line: &TextLine) -> String {
        line.spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect()
    }

    /// The caret is a block over the character it is on, not a glyph between
    /// two of them: a bar inserted at the caret costs a column, so the value
    /// shifted right of the caret every time the caret moved through it.
    #[test]
    fn a_focused_field_draws_its_caret_on_the_character_it_is_on() {
        let mut field = Field::given("rent");
        field.edit(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        field.edit(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));

        let line = field_line(
            "Description",
            Label::from("rent"),
            Some(Caret::in_field(&field)),
        );
        assert!(joined(&line).ends_with("rent"), "{}", joined(&line));
        assert_eq!(reversed(&line), "n");
    }

    /// The caret lands on a character rather than a byte. A span sliced
    /// through the middle of a multi-byte one panics the draw.
    #[test]
    fn a_caret_on_a_multi_byte_character_blocks_the_whole_character() {
        let mut field = Field::given("café");
        field.edit(ctrl('b'));

        let line = field_line(
            "Description",
            Label::from("café"),
            Some(Caret::in_field(&field)),
        );
        assert!(joined(&line).ends_with("café"), "{}", joined(&line));
        assert_eq!(reversed(&line), "é");
    }

    /// At the end of the line there is no character to block out, so the
    /// caret sits on the space past the last one -- the only place it costs a
    /// column, and where a terminal's own cursor sits too.
    #[test]
    fn a_caret_at_the_end_of_a_line_blocks_the_space_past_it() {
        let field = Field::given("rent");
        let line = field_line(
            "Description",
            Label::from("rent"),
            Some(Caret::in_field(&field)),
        );

        assert!(joined(&line).ends_with("rent "), "{}", joined(&line));
        assert_eq!(reversed(&line), " ");
    }

    #[test]
    fn an_unfocused_field_draws_no_caret() {
        let line = field_line("Description", Label::from("rent"), None);
        assert!(joined(&line).ends_with("rent"));
        assert_eq!(reversed(&line), "");
    }

    /// A selector is a choice rather than a buffer, so its caret goes past
    /// the choice, where every caret in the app was drawn before there was
    /// one to place.
    #[test]
    fn a_selector_draws_its_caret_past_the_choice() {
        let line = field_line("Account", Label::from("CHK"), Some(Caret::End));
        assert!(joined(&line).ends_with("CHK "), "{}", joined(&line));
        assert_eq!(reversed(&line), " ");
    }

    /// A caret drawn inside a scrambled figure would count the digits back
    /// out, and it stays out even though a scrambled figure is exactly as
    /// wide as the one it replaces: it is a different string, not merely a
    /// shorter one.
    #[cfg(feature = "demo")]
    #[test]
    fn a_caret_in_a_scrambled_figure_goes_to_the_end_of_it() {
        crate::demo::install_with_salt(7);
        let mut form = ValueForm::money("Amount", "123.45");
        form.edit(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));

        let drawn = form.display();
        assert_ne!(drawn, "123.45");
        let line = field_line("Amount", Label::from(drawn.clone()), Some(form.caret()));
        assert!(
            joined(&line).ends_with(&format!("{drawn} ")),
            "{}",
            joined(&line)
        );
        assert_eq!(reversed(&line), " ");
    }

    /// `field_line_labeled` replaced a `format!("{label:>12}  ")` with a
    /// pad span measured off `Label::plain_text()`. For a label with no
    /// account the two must draw the identical characters, whether the label
    /// is short enough to need padding or long enough to overrun it --
    /// otherwise every other `ValueForm` (Planning's constants, Actual
    /// Value, Birth Date) would have shifted along with the Reconcile fix.
    #[test]
    fn a_labeled_field_with_no_account_reads_identically_to_field_line() {
        fn joined(line: &TextLine) -> String {
            line.spans.iter().map(|s| s.content.as_ref()).collect()
        }
        for label in ["Target", "A Very Long Label Indeed"] {
            for caret in [None, Some(Caret::End)] {
                let plain = field_line(label, Label::from("26"), caret.clone());
                let labeled = field_line_labeled(&Label::from(label), Label::from("26"), caret);
                assert_eq!(joined(&plain), joined(&labeled), "{label:?}");
            }
        }
    }

    /// Eleven forms had this arithmetic written out, each with its own `+ 2`,
    /// `+ 3` or `+ 4`. Deriving the height from the lines is what makes those
    /// the same number: a form that adds a line past its fields gets a box a
    /// row taller without saying so.
    #[test]
    fn a_form_is_as_tall_as_its_lines_plus_its_border() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for count in [1usize, 3, 6] {
            let lines: Vec<TextLine> = (0..count)
                .map(|i| field_line("Label", Label::from(i.to_string()), None))
                .collect();
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            let mut drawn = Rect::default();
            terminal
                .draw(|frame| drawn = render_fields(frame, "Title", lines.clone()))
                .unwrap();

            assert_eq!(drawn.height, count as u16 + 2, "{count} lines");
            assert_eq!(drawn.width, FORM_WIDTH);
            // Centered: the margins either side are equal.
            assert_eq!(drawn.x, (80 - FORM_WIDTH) / 2);

            let rendered = terminal.backend().to_string();
            assert!(rendered.contains("Title"), "the border lost its title");
            assert!(
                rendered.contains(&(count - 1).to_string()),
                "the last line was not drawn"
            );
        }
    }
}
