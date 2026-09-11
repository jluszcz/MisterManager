//! The Funds tab: one line saying there is nothing to draw.
//!
//! The holdings are in the database, but every fund's composition reads as
//! never fetched until something populates `fund_mix`, and a look-through of
//! nothing is a table of blanks. So the tab says so in a sentence. An empty
//! `<section>` would be a tab a reader taps and gets a blank page from, with
//! no way to tell a page with nothing to say from one that failed to build --
//! which is the same reason the screen draws `—` in the mix column rather than
//! leaving the row's cells empty.

pub(super) fn table() -> String {
    "<p>No fund composition has been fetched, so there is nothing to look \
     through yet.</p>"
        .to_string()
}
