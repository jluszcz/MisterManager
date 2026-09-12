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
  `refresh_every_mix` and `mm mixes` with no `--ticker` are the same reading, and both take their
  tickers out of `holding`, where `tui::fund::HoldingForm::commit` has already uppercased them.
  `mm mixes --ticker` is the one route that reads no holding, so it uppercases and trims its own
  argument where the argument is read — the root `CLAUDE.md`'s rule that a ticker is normalised
  once, by whichever writer takes it, since neither `fund_mix`'s primary key nor `holding`'s
  duplicate guards fold case.
- **A refresh with no tickers touches neither SEC nor the database.** `resolve_series` is the first
  hop and downloads the whole 1.2 MB ticker file whatever it is handed, so `refresh` answers an
  empty list before it — which is what `G` on a Funds screen holding nothing and `mm mixes` against
  a database with no holdings both ask for. It is also what makes the screen's `nothing to refresh`
  true rather than the report of a round trip that found nothing.
- **A failure reason is prose a screen prints verbatim, so the one that names a ticker masks it
  where it is built.** `fetch_ticker`'s "SEC lists no series for" is that one; every other reason
  here names a URL, a series id or an element, none of them the owner's. The Funds screen draws
  `Refreshed::failed` as the masked ticker and the reason side by side, and a reason carrying the
  real ticker would put a pseudonym beside the thing it stands for. The rule is `db::holding`'s own
  refusals', and `src/demo/mod.rs` is where it is argued.
- **Which classification path a filing takes is decided by its holdings count, not by anything it
  says about itself.** A filing never states an asset class: `assetCat` is a regulatory category,
  and a fund-of-funds' holdings are other funds whose own category says nothing about what *they*
  hold. So a small filing is read as a book of funds and classified by keyword over each holding's
  name and title, and a large one as a book of securities and classified by `assetCat` and
  `invCountry`. `FUND_OF_FUNDS_MAX` is the fork and carries the measurement behind it; the two
  keyword orders and the category list are each argued where they are written. What a holding
  matching nothing becomes is `classify`'s to say, and the root `CLAUDE.md` states the rule it
  answers to.

- **A weight the parser cannot believe is refused at the seam, because `classify` cannot refuse
  it later.** `sec::PCT_VAL_LIMIT` bounds a single position's `pctVal` either side of zero, and
  `PendingHolding::finish` is where it is applied, beside the refusal a missing `pctVal` already
  earns. The reason is downstream and in another module: `classify` scales every figure by 10,000
  and sums the lot into an `i64`, so an infinity — `1e999` is a legal `f64` parse — saturates that
  cast to `i64::MAX` and the next holding of its class overflows the accumulation. That is a
  composition quietly wrong where a missing figure is loudly refused, and by the time `classify`
  sees it there is no filing left to name. The constant is a range rather than an `is_finite`
  check so it refuses `NaN` too, which would otherwise cast to a zero and read as a holding this
  parser dropped.

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

Two facts about the client sit here rather than being re-derived from `query`'s own source. **The
client outlives the runtime that built it**: `query::http_client()` is a process-wide
`OnceLock<Client>`, while every function in `sec.rs` opens a current-thread runtime for the span of
its own call and drops it — so the singleton is created inside the first of those runtimes and
handed to every one after. If this test ever fails with a transport or `Canceled` error rather than
a parse error, that is the cause and not `THROTTLE_MARKER`. **And `gzip` is on end to end**: the
shared client sets it and reqwest asks for and decodes it, so the "3.2–20.6 MB filing"
`parse_filing` is sized against is roughly a 2 MB transfer — which is why that client's 30-second
whole-request timeout is far less tight than the filing figure makes it read.

- **A filing carries the fund's own name, and it is `genInfo/seriesName` rather than `regName`
  beside it.** The registrant is the trust — `Fidelity Concord Street Trust`, `SCHWAB CAPITAL
  TRUST` — which holds dozens of unrelated funds and is shouted in some filings and title-cased in
  others; the series is the fund. It names a *series* and never a share class, two classes of one
  fund sharing a `seriesId`, so the ticker beside it is what tells them apart.
  It is `Option` because it is not what the filing is fetched for: a composition with no name on
  it is a complete answer to what `g` asks, and refusing a refresh over a missing courtesy label
  would make a document SEC accepted unreadable here. An empty element reads as absent, one
  spelling for one absence. `tests/sec_live.rs` is the one place a real filing says how often that
  happens, and asserts structurally there like everything else: non-empty, and not the registrant.
  It is stored on `fund_mix` rather than on `holding`, repeated across a ticker's slice rows
  exactly as `report_date` is — both arrive from one filing and `set_for_ticker` rewrites them
  together, so a row carrying one and not the other is unreachable.
