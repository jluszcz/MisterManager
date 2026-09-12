Integration tests read the real `Money.xlsx` workbook, wherever `MM_WORKBOOK`
says it is.

It is not committed -- it is personal financial data, and neither is its path,
which is why there is no default. Tests that need it are skipped when
`MM_WORKBOOK` is unset or names a file that is not there, so `cargo test`
passes on a clean checkout.

Two separate things keep it quiet, and only one of them is that skip. Every
binary here is `#![cfg(feature = "import")]`, so a run without
`--features import` compiles them to nothing rather than skipping them; a run
with the feature and no workbook is the one that skips loudly. A green
`cargo test` therefore says nothing about the importer on its own -- the
`MM_REQUIRE_WORKBOOK` section of the root `README.md` carries the invocation
that does.

`nport_fund_of_funds.xml` and `nport_direct.xml` are a different kind of
fixture: two small, invented N-PORT filings that `src/mix/sec.rs`'s
`parse_filing` tests read with `include_bytes!`. Unlike the workbook, these
*are* committed -- no `MM_WORKBOOK`-style restriction applies to them,
because nothing in either file is real. The element names and nesting are a
real filing's; every name, id and figure is drawn from `CLAUDE.md`'s fixture
vocabulary rather than downloaded from EDGAR.
