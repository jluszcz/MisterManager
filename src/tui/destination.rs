//! Pointing one Planning line at a goal, or at nothing. Backs `e` on a
//! destination row.
//!
//! A list rather than a field: the thing being chosen is a goal that already
//! exists, and typing its name would put name matching back where the whole
//! design keeps it out of -- what gets stored is the id under the cursor.

use super::cursor::{Cursor, Viewport, impl_scroll};
use super::search::{Search, SearchBox};
use crate::db::GoalId;
use crate::plan_line::Line;
use ratatui::text::{Line as TextLine, Span};

/// A goal as the list offers it: what it is called and where it sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offered {
    pub id: GoalId,
    pub name: String,
    pub container: String,
}

/// Why one goal was lifted to the top of the list.
///
/// At most one is, and it is the row the list opens on -- which is what keeps
/// the withdrawal beneath it on screen. Eighty-three goals in this database
/// sort before "Home Down Payment", so a list that opened where that goal
/// falls naturally would put the one row that clears the key off the top of
/// the screen, with nothing to say it was ever there.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Lifted {
    /// This line's import substring names it and nothing else claims it:
    /// one keystroke for the case that brought the owner here.
    Suggested,
    /// This line already points at it.
    Current,
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Clear the key. The money leaves the tracked system -- a destination in
    /// its own right, and how the account-backed lines are meant to stand, so it
    /// is a row to choose rather than an absence to back out into.
    Unset,
    Goal {
        id: GoalId,
        name: String,
        container: String,
        lifted: Option<Lifted>,
    },
}

impl Choice {
    fn name(&self) -> &str {
        match self {
            Choice::Unset => "",
            Choice::Goal { name, .. } => name,
        }
    }
}

/// The list, its filter, and where the cursor is in it.
pub struct Chooser {
    line: Line,
    choices: Vec<Choice>,
    /// Indices into `choices`, after the search. Parallel to what is drawn.
    visible: Vec<usize>,
    search: SearchBox,
    cursor: Cursor,
}

impl Chooser {
    /// The list in the order it opens: the one lifted goal if there is one,
    /// then the withdrawal, then every open goal in the order the Savings
    /// screen lists them.
    ///
    /// It always opens at the top, which is the whole reason anything is
    /// lifted: the row the owner most likely wants is under the cursor, and
    /// the withdrawal is beside it rather than eighty-three goals above.
    /// `Enter` straight away is therefore agreement or a no-op, never a
    /// silent re-pointing.
    ///
    /// The suggestion wins the lift when both could claim it, though both
    /// never do: a suggestion is only offered for a line pointing at no live
    /// goal, which is a line with no current goal to lift.
    /// `current` and `suggestion` are goals rather than ids because the goal
    /// a line names need not be among those on offer: a **closed** goal is
    /// still a real row the key still points at, and `offered` holds open
    /// goals only. Lifting it by id alone would find nothing, drop the
    /// cursor onto the withdrawal, and let a stray `Enter` clear a
    /// destination that was never in question.
    pub fn new(
        line: Line,
        offered: Vec<Offered>,
        current: Option<Offered>,
        suggestion: Option<Offered>,
    ) -> Chooser {
        let lifted = match (suggestion, current) {
            (Some(o), _) => Some((o, Lifted::Suggested)),
            (None, Some(o)) => Some((o, Lifted::Current)),
            (None, None) => None,
        };

        let mut choices = Vec::with_capacity(offered.len() + 1);
        if let Some((o, why)) = &lifted {
            choices.push(Choice::Goal {
                id: o.id,
                name: o.name.clone(),
                container: o.container.clone(),
                lifted: Some(*why),
            });
        }
        choices.push(Choice::Unset);
        let promoted = lifted.map(|(o, _)| o.id);
        choices.extend(
            offered
                .into_iter()
                .filter(|o| Some(o.id) != promoted)
                .map(|o| Choice::Goal {
                    id: o.id,
                    name: o.name,
                    container: o.container,
                    lifted: None,
                }),
        );

        let mut chooser = Chooser {
            line,
            choices,
            visible: Vec::new(),
            search: SearchBox::new(),
            cursor: Cursor::new(),
        };
        chooser.refilter();
        chooser
    }

    pub fn line(&self) -> Line {
        self.line
    }

    /// The rows the search left, in list order.
    pub fn choices(&self) -> Vec<&Choice> {
        self.visible.iter().map(|i| &self.choices[*i]).collect()
    }

    pub fn selected(&self) -> Option<&Choice> {
        self.visible
            .get(self.cursor.index())
            .map(|i| &self.choices[*i])
    }

    /// The border's title. A `Line` rather than a `String` because this modal
    /// has no footer: its title is where the `/` box is drawn, caret and all.
    ///
    /// An open box is drawn even while it is empty, which is the state every
    /// `/` begins in: with nowhere else to show it, a title that waited for
    /// the first character would leave `/` looking like a key that does
    /// nothing, and the `Esc` clearing the box after it like a second one. A
    /// shut box with a needle still in it is drawn too -- that is a filter
    /// narrowing the list, and it has to be visible to be undone.
    pub(super) fn title(&self) -> TextLine<'static> {
        let mut spans = vec![Span::raw(format!(
            "{} — / search · Enter choose · Esc cancel",
            self.line.label()
        ))];
        if self.is_searching() || !self.search().is_empty() {
            spans.push(Span::raw(" · /"));
            spans.extend(self.search_spans());
        }
        TextLine::from(spans)
    }
}

impl Search for Chooser {
    fn search_box(&self) -> &SearchBox {
        &self.search
    }

    fn search_box_mut(&mut self) -> &mut SearchBox {
        &mut self.search
    }

    /// The withdrawal row survives every search.
    ///
    /// It has no name to match on, and a filter that hid it would make
    /// clearing a key reachable only by clearing the filter first -- for the
    /// one choice that is always available whatever the goals are called.
    ///
    /// The rows offer no figures: what is being chosen is a goal by identity,
    /// and the amount that will land on it is the waterfall's, not the goal's.
    fn refilter(&mut self) {
        let matcher = self.matcher();
        self.visible = self
            .choices
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c, Choice::Unset) || matcher.matches(c.name(), &[]))
            .map(|(i, _)| i)
            .collect();
        self.cursor.clamp(self.visible.len());
    }
}

impl_scroll!(Chooser, visible);

use super::form::centered;
use super::{Chrome, render_table};
use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::style::Style;
use ratatui::widgets::{Block, Cell, Clear, Row};

/// One row per goal, its container beside it, and the withdrawal among them.
/// Returns the [`Viewport`] it drew: the height `PageUp`/`PageDown` move by,
/// and the row the next draw starts from.
pub(super) fn render(frame: &mut Frame, chooser: &Chooser) -> Viewport {
    let area = centered(
        frame.area(),
        76,
        frame.area().height.saturating_sub(4).max(8),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(Block::bordered().title(chooser.title()), area);
    let inner = area.inner(ratatui::layout::Margin::new(1, 1));

    let suggested_style = Style::default().fg(super::style::WARNING);
    let choices = chooser.choices();
    let rows: Vec<Row> = choices
        .iter()
        .map(|choice| match choice {
            Choice::Unset => Row::new(vec![
                Cell::from("— withdrawal: leaves the tracked system —"),
                Cell::from(""),
                Cell::from(""),
            ]),
            Choice::Goal {
                name,
                container,
                lifted,
                ..
            } => {
                let row = Row::new(vec![
                    Cell::from(crate::demo::text(name).into_owned()),
                    Cell::from(crate::demo::text(container).into_owned()),
                    Cell::from(match lifted {
                        Some(Lifted::Suggested) => "suggested",
                        Some(Lifted::Current) => "current",
                        None => "",
                    }),
                ]);
                // Amber for the suggestion only: it is a prompt. Where the
                // line already points is a statement of fact, and drawing it
                // like a prompt would ask a question nobody asked.
                match lifted {
                    Some(Lifted::Suggested) => row.style(suggested_style),
                    _ => row,
                }
            }
        })
        .collect();
    let widths = [
        Constraint::Min(24),
        Constraint::Length(12),
        Constraint::Length(10),
    ];
    render_table(
        frame,
        inner,
        chooser,
        Chrome::bare(),
        &widths,
        rows,
        choices.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::cursor::Scroll;
    use crate::tui::form::backspace_key;

    fn offered() -> Vec<Offered> {
        [
            (1, "Bill Payments", "Rainy Day"),
            (2, "Housing", "Rainy Day"),
            (3, "Vacation 2026", "Rainy Day"),
            (4, "Vacation 2027", "Rainy Day"),
            (5, "Mom & Dad", "Brokerage"),
        ]
        .into_iter()
        .map(|(id, name, container)| Offered {
            id: GoalId(id),
            name: name.to_string(),
            container: container.to_string(),
        })
        .collect()
    }

    /// The goals the tests name by id, as the app would hand them over.
    fn one(id: Option<i64>) -> Option<Offered> {
        let id = GoalId(id?);
        offered().into_iter().find(|o| o.id == id).or(Some(Offered {
            id,
            name: format!("goal {}", id.0),
            container: "Rainy Day".to_string(),
        }))
    }

    fn chooser(current: Option<i64>, suggestion: Option<i64>) -> Chooser {
        Chooser::new(Line::MomAndDad, offered(), one(current), one(suggestion))
    }

    fn selected_name(chooser: &Chooser) -> String {
        match chooser.selected() {
            None => "<nothing>".to_string(),
            Some(Choice::Unset) => "<unset>".to_string(),
            Some(Choice::Goal { name, .. }) => name.clone(),
        }
    }

    /// A goal's name is the owner's own word for it, drawn beside its
    /// container, which already masks -- the same rule every other list in
    /// the app follows, and the one this list had missed.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_a_goals_name_in_the_destination_list() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        crate::demo::install_with_salt(7);
        let chooser = chooser(None, None);
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &chooser);
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            !text.contains("Bill Payments"),
            "the goal name survived: {text}"
        );
        assert!(
            text.contains(&crate::demo::text("Bill Payments").to_string()),
            "no scrambled goal name found: {text}"
        );
        assert!(
            !text.contains("Rainy Day"),
            "the container survived: {text}"
        );
        assert!(
            text.contains(&crate::demo::text("Rainy Day").to_string()),
            "no scrambled container found: {text}"
        );
    }

    /// The whole point of the suggestion: the line that brought the owner
    /// here is one keystroke from being pointed at the goal it names.
    #[test]
    fn the_suggestion_opens_at_the_top_and_under_the_cursor() {
        let chooser = chooser(None, Some(5));
        assert_eq!(selected_name(&chooser), "Mom & Dad");
        assert!(matches!(
            chooser.choices().first(),
            Some(Choice::Goal {
                lifted: Some(Lifted::Suggested),
                ..
            })
        ));
    }

    /// A goal promoted to the top would otherwise appear again further down,
    /// and choosing between two rows that write the same id is a choice with
    /// no meaning.
    #[test]
    fn a_suggested_goal_is_not_also_listed_in_its_own_place() {
        let chooser = chooser(None, Some(5));
        let times = chooser
            .choices()
            .iter()
            .filter(|c| matches!(c, Choice::Goal { id, .. } if *id == GoalId(5)))
            .count();
        assert_eq!(times, 1);
    }

    /// Eighty-three goals below the fold is where the withdrawal was: the
    /// list opened on a goal near the bottom, and the one row that clears
    /// the key sat off the top of the screen with nothing to say it existed.
    #[test]
    fn the_withdrawal_stays_on_screen_whatever_the_line_already_names() {
        // Mom & Dad is the last goal offered -- the worst case, and the one
        // Future Housing hits against the real database.
        let chooser = chooser(Some(5), None);

        assert_eq!(chooser.selected_index(), 0, "the list opened scrolled away");
        assert!(
            chooser.choices()[..2].contains(&&Choice::Unset),
            "the withdrawal is not within reach of the cursor: {:?}",
            chooser.choices()[..2].to_vec()
        );
    }

    /// The same reason the suggestion is not listed twice: two rows writing
    /// the same id is a choice with no meaning.
    #[test]
    fn the_goal_a_line_already_names_is_not_also_listed_in_its_own_place() {
        let chooser = chooser(Some(5), None);
        let times = chooser
            .choices()
            .iter()
            .filter(|c| matches!(c, Choice::Goal { id, .. } if *id == GoalId(5)))
            .count();
        assert_eq!(times, 1);
    }

    /// Opening on the goal already stored makes `Enter` a no-op rather than a
    /// silent re-pointing at whatever happened to be first.
    #[test]
    fn a_line_already_pointed_somewhere_opens_on_the_goal_it_names() {
        let chooser = chooser(Some(3), None);
        assert_eq!(selected_name(&chooser), "Vacation 2026");
    }

    /// Nothing stored and nothing to suggest: the cursor sits on the state
    /// the line is already in, so `Enter` changes nothing by accident.
    #[test]
    fn an_unset_line_with_no_suggestion_opens_on_the_withdrawal() {
        let chooser = chooser(None, None);
        assert_eq!(selected_name(&chooser), "<unset>");
    }

    /// A goal that has been closed is still a real row the key still names,
    /// and it is not among the goals on offer. Falling back to the top would
    /// put the cursor on the withdrawal, where a stray `Enter` clears the
    /// destination -- the exact silent re-pointing the lift exists to
    /// prevent.
    #[test]
    fn a_line_pointing_at_a_closed_goal_still_opens_on_that_goal() {
        let closed = Offered {
            id: GoalId(42),
            name: "Peak Design".to_string(),
            container: "Rainy Day".to_string(),
        };
        let chooser = Chooser::new(Line::MomAndDad, offered(), Some(closed), None);
        assert_eq!(selected_name(&chooser), "Peak Design");
        assert_eq!(chooser.selected_index(), 0);
    }

    /// A key naming a goal that is gone: the id matches nothing offered, and
    /// the list still has to open somewhere.
    #[test]
    fn a_dangling_current_goal_opens_at_the_top_rather_than_nowhere() {
        let chooser = chooser(Some(9_999), None);
        assert!(chooser.selected().is_some());
    }

    #[test]
    fn searching_narrows_to_the_goals_that_match() {
        let mut chooser = chooser(None, None);
        chooser.begin_search();
        for c in "vac".chars() {
            chooser.push_search(c);
        }

        let names: Vec<String> = chooser
            .choices()
            .iter()
            .filter(|c| !matches!(c, Choice::Unset))
            .map(|c| c.name().to_string())
            .collect();
        assert_eq!(names, vec!["Vacation 2026", "Vacation 2027"]);
    }

    /// Eighty-three goals is a long way to scroll for a name three keystrokes
    /// would find, and the same `/` does the same thing on three other lists.
    #[test]
    fn searching_matches_case_insensitively() {
        let mut chooser = chooser(None, None);
        for c in "MOM".chars() {
            chooser.push_search(c);
        }
        assert_eq!(
            chooser
                .choices()
                .iter()
                .filter(|c| !matches!(c, Choice::Unset))
                .count(),
            1
        );
    }

    /// Clearing a key must not depend on what the goals are called.
    #[test]
    fn the_withdrawal_survives_a_search_that_matches_nothing() {
        let mut chooser = chooser(None, None);
        for c in "zzzz".chars() {
            chooser.push_search(c);
        }
        assert_eq!(chooser.choices(), vec![&Choice::Unset]);
    }

    #[test]
    fn clearing_the_search_shows_every_goal_again() {
        let mut chooser = chooser(None, None);
        chooser.begin_search();
        chooser.push_search('z');
        chooser.clear_search();

        assert_eq!(chooser.choices().len(), offered().len() + 1);
        assert!(!chooser.is_searching());
    }

    /// The cursor is an index into the filtered list, so a search that
    /// shortens it under a cursor sitting past the new end must move rather
    /// than select nothing.
    #[test]
    fn a_search_that_shortens_the_list_moves_the_cursor_into_it() {
        let mut chooser = chooser(None, None);
        chooser.select_last();
        for c in "vac".chars() {
            chooser.push_search(c);
        }
        assert!(chooser.selected().is_some());
    }

    #[test]
    fn backspace_widens_the_search_again() {
        let mut chooser = chooser(None, None);
        for c in "vacation 2026".chars() {
            chooser.push_search(c);
        }
        assert_eq!(chooser.choices().len(), 2);

        for _ in 0.."ation 2026".len() {
            chooser.edit_search(backspace_key());
        }
        assert_eq!(chooser.choices().len(), 3);
    }

    /// The title carries what the list is for: without the line's name,
    /// nothing on screen says which of the six destinations is being pointed.
    #[test]
    fn the_title_names_the_line_and_the_live_search() {
        let mut chooser = chooser(None, None);
        assert!(title(&chooser).contains("Mom & Dad"));

        chooser.push_search('v');
        assert!(title(&chooser).contains("/v"), "{}", title(&chooser));
    }

    /// This modal has no footer, so its title is where the box itself is
    /// drawn -- and the caret has to say the box is taking keystrokes. A
    /// filter kept by `Enter` takes none, and draws none.
    #[test]
    fn the_title_carries_a_caret_only_while_the_box_is_open() {
        use ratatui::style::Modifier;

        let mut chooser = chooser(None, None);
        chooser.begin_search();
        chooser.push_search('v');
        chooser.push_search('a');
        chooser.step_search_caret(crate::tui::form::Step::PREVIOUS);

        let caret = |chooser: &Chooser| -> String {
            chooser
                .title()
                .spans
                .iter()
                .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
                .map(|s| s.content.as_ref())
                .collect()
        };
        assert!(title(&chooser).ends_with("/va"), "{}", title(&chooser));
        assert_eq!(caret(&chooser), "a");

        chooser.end_search();
        assert!(title(&chooser).ends_with("/va"), "{}", title(&chooser));
        assert_eq!(caret(&chooser), "");
    }

    /// An open box is drawn before anything is typed into it, which is the
    /// state every `/` begins in. With no footer to hold it, a title that
    /// waited for the first character would leave `/` looking like a key that
    /// does nothing -- and the `Esc` that closes the empty box like a second
    /// one.
    #[test]
    fn the_title_shows_the_box_as_soon_as_it_opens() {
        use ratatui::style::Modifier;

        let mut chooser = chooser(None, None);
        assert!(!title(&chooser).contains(" · /"), "{}", title(&chooser));

        chooser.begin_search();

        assert!(title(&chooser).contains(" · /"), "{}", title(&chooser));
        let carets = chooser
            .title()
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .count();
        assert_eq!(carets, 1, "the open box draws no caret to type into");
    }

    /// The title's words, for the assertions that are about wording.
    fn title(chooser: &Chooser) -> String {
        chooser
            .title()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }
}
