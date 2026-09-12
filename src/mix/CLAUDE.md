# mix — a fund's composition, from SEC

`classify` is the whole of the feature's logic and has neither a network nor a database in it;
`sec` is the network client, the only place `reqwest` and `jluszcz_rust_utils` are named and one of
two places `tokio` is, `src/backup/s3.rs` being the other; `mod.rs` is the policy joining the two
and the only thing here that writes `db::fund_mix`. Nothing in this directory draws anything — what
the composition is *for* is `src/allocation.rs`, which both the Funds screen and the report's Funds
tab read.

Everything below is either a fact about SEC's own service, which no test in a clean checkout can
confirm, or a rule that spans this directory and the places a refresh is triggered from.

## Three hops, and the one that is easy to get wrong

A ticker becomes a filing's holdings in three requests, in this order and no other:
`company_tickers_mf.json` for the series the ticker belongs to, that series' own Atom feed for its
newest NPORT-P, and `primary_doc.xml` in the directory the feed's `filing-href` names.
`resolve_series`, `latest_filing` and `parse_filing` are the three, and each says at its own
definition what it reads and what it ignores.

**The series id goes in `browse-edgar`'s `CIK` slot**, which reads as a mistake and is the fact the
middle hop turns on: that parameter takes whichever entity id it is given, and a trust's own CIK
there answers with every series that trust has ever filed for rather than this one's.
`browse_edgar_url` is where that is stated and
`browse_edgar_url_puts_the_series_id_in_the_cik_slot` is what holds it up.

**Nothing about the three is enforced by a type.** A URL shape, a field name, a feed element: each
is a string this crate spells and SEC's service answers to, and a fixture can only ever confirm
that the parser reads the document it was handed. That is what `tests/sec_live.rs` is for, and it
is the only test in the crate that can notice the hops coming apart.

## Invariants

- **Every request declares a contact, and it is configuration rather than a constant.** SEC refuses
  a request whose `User-Agent` carries none, and a real address may not be a literal in any tracked
  file — so `[sec] contact` in the config file is the only place it comes from, and a run without
  one refuses rather than asking anonymously. Two callers reach the fetcher and each spells that
  refusal in its own medium: `mm mixes` names the config file's path, and the Funds screen's
  `g`/`G` put a status line where it has no path to name. Both say what to do in the one
  `config::ADD_SEC_CONTACT` rather than twice over. `user_agent` is what the contact becomes.
- **Nothing refreshes a mix on its own.** There is no schedule here and no cache — a composition is
  as old as the last time someone pressed `g`/`G` or ran `mm mixes`, which is why `fund_mix` stores
  a `report_date` per ticker and why both sinks draw it beside the holding. There is nothing a
  schedule could be a proxy for either: a filing is published on the fund family's own calendar,
  and nothing here can tell a stale composition from a current one without fetching it.
- **A fund's composition is keyed on the ticker and nothing else.** One fetch of `USM` prices every
  account holding it, and a refresh asked for "everything" reads `holding::tickers` rather than the
  rows a screen is showing — the account filter and the search narrow a list, not a portfolio.
  `refresh_every_mix` and `mm mixes` with no `--ticker` are the same reading. Both take their
  tickers out of `holding`, where the form has already uppercased them — `mm mixes --ticker` is the
  one route that does not, and it fails at the first hop rather than writing a row nothing reads,
  since `parse_ticker_file` matches SEC's own symbols exactly.
- **Which classification path a filing takes is decided by its holdings count, not by anything it
  says about itself.** A filing never states an asset class: `assetCat` is a regulatory category,
  and a fund-of-funds' holdings are other funds whose own category says nothing about what *they*
  hold. So a small filing is read as a book of funds and classified by keyword over each holding's
  name and title, and a large one as a book of securities and classified by `assetCat` and
  `invCountry`. `FUND_OF_FUNDS_MAX` is the fork and carries the measurement behind it; the two
  keyword orders and the category list are each argued where they are written. What a holding
  matching nothing becomes is `classify`'s to say, and the root `CLAUDE.md` states the rule it
  answers to.

## Checking it against the live service

`tests/sec_live.rs` is the only test that reaches SEC, and it is the only thing that would notice
EDGAR changing a URL shape or a field name — without it the feature fails silently one quarter
after the last time anyone looked. It asserts structurally and never a figure, so a fund's holdings
turning over does not turn it red. It skips loudly unless `MM_SEC_TICKER` names a fund, and
`MM_REQUIRE_SEC=1` makes the skip a failure:

```sh
MM_REQUIRE_SEC=1 MM_SEC_TICKER=<ticker> cargo test --test sec_live
```

There is deliberately no default ticker. Which funds the owner holds is the same kind of fact as an
account code, and the contact comes from the config file at run time for the same reason
`MM_WORKBOOK` is not written down.

**`THROTTLE_MARKER` is a guess, and this run is what confirms it.** SEC answers a request it decides
to throttle with a 403 — which `query::send` treats as permanent and does not retry — carrying
prose that says so, and `is_sec_throttle` tells that 403 from an ordinary one by looking for that
prose. The string was written without a throttled response to read, so two things are worth knowing
before changing it. First, **the marker has to appear in the first kilobyte of the body**:
`query::send` truncates a response body to that before it ever reaches an `anyhow::Error`, so prose
further down the page is prose `is_sec_throttle` never sees. Second, it degrades safely in both
directions, which is why it stays a guess rather than a blocker — too narrow, a real throttle fails
on the first attempt with an error that names the 403; too broad, a permanent 403 costs three extra
requests over six seconds. Neither reading can hammer SEC, whose own published limit is 10 requests
a second and which this module stays orders of magnitude under by running every request
sequentially.
