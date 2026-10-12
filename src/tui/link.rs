//! Linking a recurring goal entry to an open goal that is a round of it but
//! was never created from it, and unlinking one linked by mistake. Backs `l`
//! on the Recurring Goals screen.
//!
//! A list for the reason [`super::destination`] is one: what is stored is the
//! id under the cursor, never a name, since goal names are not unique. The
//! entry's name only decides which rows are lifted to the top -- a hint about
//! where to look, never an answer.

use super::account_label::account_cell;
use super::cursor::{Cursor, Viewport};
use super::search::{self, Search, SearchBox};
use crate::account_label::Account;
use crate::db::{GoalId, RecurringGoalId};
use chrono::{Datelike, NaiveDate};
use jluszcz_finance_utils::impl_scroll;
use ratatui::text::{Line as TextLine, Span};

/// A goal as the list offers it: what it is called and where it sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offered {
    pub id: GoalId,
    pub name: String,
    pub container: Account,
    /// Drawn beside the name because the name alone does not tell two rounds
    /// of one entry apart: an entry and its goals can share a name with
    /// another entry and its goals, and the month is what differs.
    pub goal_date: Option<NaiveDate>,
    /// Already a round of this entry: `u` unlinks it, `Enter` does nothing.
    pub linked: bool,
}

/// The list, its filter, and where the cursor is in it.
pub struct Linker {
    entry: RecurringGoalId,
    entry_name: String,
    /// 1-12, drawn in the title beside the name for the reason
    /// [`Offered::goal_date`] is drawn beside a goal's.
    entry_month: i64,
    offered: Vec<Offered>,
    /// Indices into `offered`, after the search. Parallel to what is drawn.
    visible: Vec<usize>,
    search: SearchBox,
    cursor: Cursor,
}

impl Linker {
    /// The entry's existing links first, then the goals whose names contain
    /// the entry's and fall in its month, then the rest whose names contain
    /// it, then everything else, each group in the order `offered` arrived
    /// in. It opens at the top: on an existing link where there is one, so a
    /// reflexive `Enter` changes nothing, and otherwise on the likeliest
    /// goal -- which `Enter` links, and `u` takes back.
    pub fn new(
        entry: RecurringGoalId,
        entry_name: String,
        entry_month: i64,
        mut offered: Vec<Offered>,
    ) -> Linker {
        let needle = entry_name.to_lowercase();
        // Stable, so each group keeps the order it arrived in; cached, so each
        // name is lowercased once rather than per comparison.
        offered.sort_by_cached_key(|o| {
            if o.linked {
                return 0;
            }
            let named = o.name.to_lowercase().contains(&needle);
            let in_month = o
                .goal_date
                .is_some_and(|d| i64::from(d.month()) == entry_month);
            match (named, in_month) {
                (true, true) => 1,
                (true, false) => 2,
                (false, _) => 3,
            }
        });
        let mut linker = Linker {
            entry,
            entry_name,
            entry_month,
            offered,
            visible: Vec::new(),
            search: SearchBox::new(),
            cursor: Cursor::new(),
        };
        linker.refilter();
        linker
    }

    pub fn entry(&self) -> RecurringGoalId {
        self.entry
    }

    /// The rows the search left, in list order.
    pub fn offered(&self) -> Vec<&Offered> {
        self.visible.iter().map(|i| &self.offered[*i]).collect()
    }

    pub fn selected(&self) -> Option<&Offered> {
        self.visible
            .get(self.cursor.index())
            .map(|i| &self.offered[*i])
    }

    /// Mark the row for `goal` as no longer linked, after `u` has written
    /// that. The row stays where it is, so the cursor does not jump away from
    /// what the owner just did.
    pub fn mark_unlinked(&mut self, goal: GoalId) {
        if let Some(o) = self.offered.iter_mut().find(|o| o.id == goal) {
            o.linked = false;
        }
    }

    /// The border's title, carrying the `/` box the way
    /// [`super::destination::Chooser::title`] does, and for its reason: this
    /// modal has no footer either.
    pub(super) fn title(&self) -> TextLine<'static> {
        let mut spans = vec![Span::raw(format!(
            "Link {} ({}) — / search · Enter link · u unlink · Esc cancel",
            crate::demo::text(&self.entry_name),
            super::month_name(self.entry_month)
        ))];
        if self.is_searching() || !self.search().is_empty() {
            spans.push(Span::raw(" · /"));
            spans.extend(search::search_spans(self));
        }
        TextLine::from(spans)
    }
}

impl Search for Linker {
    fn search_box(&self) -> &SearchBox {
        &self.search
    }

    fn search_box_mut(&mut self) -> &mut SearchBox {
        &mut self.search
    }

    fn refilter(&mut self) {
        let matcher = self.matcher();
        self.visible = self
            .offered
            .iter()
            .enumerate()
            .filter(|(_, o)| matcher.matches(&o.name, &[]))
            .map(|(i, _)| i)
            .collect();
        self.cursor.clamp(self.visible.len());
    }
}

impl_scroll!(Linker, visible);

use super::widget::centered;
use super::{Chrome, render_table};
use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::widgets::{Block, Cell, Clear, Row};

/// One row per goal, its date and container beside it, and whether it is
/// already linked. Returns the [`Viewport`] it
/// drew.
pub(super) fn render(frame: &mut Frame, linker: &Linker) -> Viewport {
    let area = centered(
        frame.area(),
        76,
        frame.area().height.saturating_sub(4).max(8),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(Block::bordered().title(linker.title()), area);
    let inner = area.inner(ratatui::layout::Margin::new(1, 1));

    let offered = linker.offered();
    let rows: Vec<Row> = offered
        .iter()
        .map(|o| {
            Row::new(vec![
                Cell::from(crate::demo::text(&o.name).into_owned()),
                Cell::from(
                    o.goal_date
                        .map_or_else(String::new, |d| d.format("%b %Y").to_string()),
                ),
                account_cell(&o.container),
                Cell::from(if o.linked { "linked" } else { "" }),
            ])
        })
        .collect();
    let widths = [
        Constraint::Min(24),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(6),
    ];
    render_table(
        frame,
        inner,
        linker,
        Chrome::bare(),
        &widths,
        rows,
        offered.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AccountId;
    use crate::test_support::day;

    fn offered() -> Vec<Offered> {
        [
            (1, "Bill Payments", "Rainy Day", None),
            (2, "Dropbox 2027", "Rainy Day", Some(day(2027, 3, 1))),
            (3, "Vacation", "Nest Egg", Some(day(2027, 9, 1))),
            (4, "dropbox", "Nest Egg", None),
        ]
        .into_iter()
        .map(|(id, name, container, goal_date)| Offered {
            id: GoalId(id),
            name: name.to_string(),
            container: Account::held(AccountId(id), container, None),
            goal_date,
            linked: false,
        })
        .collect()
    }

    /// Named for a month none of [`offered`] falls in, so only the name
    /// lifts a row.
    fn linker() -> Linker {
        Linker::new(RecurringGoalId(9), "Dropbox".to_string(), 1, offered())
    }

    fn names(linker: &Linker) -> Vec<&str> {
        linker.offered().iter().map(|o| o.name.as_str()).collect()
    }

    #[test]
    fn goals_named_like_the_entry_are_listed_first_and_the_list_opens_on_them() {
        let linker = linker();
        assert_eq!(
            names(&linker),
            vec!["Dropbox 2027", "dropbox", "Bill Payments", "Vacation"]
        );
        assert_eq!(linker.selected().unwrap().id, GoalId(2));
    }

    /// The lift is a hint: with no goal named like the entry, every goal is
    /// still offered, in the order it arrived.
    #[test]
    fn an_entry_no_goal_is_named_like_still_offers_every_goal() {
        let linker = Linker::new(RecurringGoalId(9), "Backblaze".to_string(), 1, offered());
        assert_eq!(
            names(&linker),
            vec!["Bill Payments", "Dropbox 2027", "Vacation", "dropbox"]
        );
    }

    /// Two entries sharing a name, each with a goal sharing it too: the
    /// month is what tells the rounds apart, so the one in the entry's month
    /// is the one the list opens on.
    #[test]
    fn a_goal_in_the_entrys_month_is_listed_before_one_merely_named_like_it() {
        let mut goals = offered();
        goals.push(Offered {
            id: GoalId(5),
            name: "Dropbox".to_string(),
            container: Account::held(AccountId(5), "Rainy Day", None),
            goal_date: Some(day(2027, 9, 1)),
            linked: false,
        });
        let linker = Linker::new(RecurringGoalId(9), "Dropbox".to_string(), 9, goals);

        assert_eq!(
            names(&linker),
            vec![
                "Dropbox",
                "Dropbox 2027",
                "dropbox",
                "Bill Payments",
                "Vacation"
            ]
        );
        assert_eq!(linker.selected().unwrap().id, GoalId(5));
    }

    /// Opening on an existing link is what makes a reflexive `Enter` a
    /// no-op rather than a second, unwanted link.
    #[test]
    fn the_entrys_existing_links_are_listed_first_and_the_list_opens_on_them() {
        let mut goals = offered();
        goals[2].linked = true;
        let linker = Linker::new(RecurringGoalId(9), "Dropbox".to_string(), 1, goals);

        assert_eq!(names(&linker)[0], "Vacation");
        assert!(linker.selected().unwrap().linked);
    }

    #[test]
    fn marking_a_row_unlinked_keeps_it_in_place() {
        let mut goals = offered();
        goals[2].linked = true;
        let mut linker = Linker::new(RecurringGoalId(9), "Dropbox".to_string(), 1, goals);

        linker.mark_unlinked(GoalId(3));

        assert_eq!(names(&linker)[0], "Vacation");
        assert!(!linker.selected().unwrap().linked);
    }

    #[test]
    fn each_row_draws_its_goals_month_and_year() {
        use jluszcz_finance_utils::tui::testing::draw_buffer;

        let linker = linker();
        let buffer = draw_buffer(80, 20, |frame| {
            render(frame, &linker);
        });
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("Mar 2027"), "{text}");
        assert!(text.contains("Sep 2027"), "{text}");
    }

    #[test]
    fn searching_narrows_to_the_goals_that_match() {
        let mut linker = linker();
        linker.begin_search();
        for c in "vac".chars() {
            linker.push_search(c);
        }
        assert_eq!(names(&linker), vec!["Vacation"]);
        assert_eq!(linker.selected().unwrap().id, GoalId(3));
    }

    #[test]
    fn a_search_matching_nothing_selects_nothing() {
        let mut linker = linker();
        for c in "zzzz".chars() {
            linker.push_search(c);
        }
        assert!(linker.selected().is_none());
    }

    #[test]
    fn the_title_names_the_entry_being_linked() {
        let title: String = linker()
            .title()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(title.starts_with("Link Dropbox (Jan)"), "{title}");
    }
}
