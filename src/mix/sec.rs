//! The network client for a fund's SEC filing -- the second seam beside
//! `src/backup/s3.rs` allowed to name `reqwest`, `tokio` and
//! `jluszcz_rust_utils`.
//!
//! Three hops turn a ticker into a filing's holdings. [`resolve_series`]
//! reads `company_tickers_mf.json` for the series a ticker belongs to;
//! [`latest_filing`] walks that series' own Atom feed to its newest NPORT-P
//! and downloads it; [`parse_filing`] -- pure, and where the tests live --
//! turns the XML into a [`Filing`]. Requests run strictly sequentially:
//! SEC's own published limit is 10/second, and there is nothing to gain here
//! by approaching it.
//!
//! Each public function opens a current-thread runtime for the span of its
//! own call and drops it, exactly as `backup::s3::upload` does and for the
//! same reason: nothing else in the crate is async, so there is no reason
//! for a runtime to outlive one call.
//!
//! `jluszcz_rust_utils::query::http_get`/`http_get_json` are not used here --
//! both force `Accept: application/json`, and two of the three hops answer
//! in XML.

use super::RawHolding;
use anyhow::{Context, Result, anyhow, bail};
use chrono::NaiveDate;
use jluszcz_rust_utils::query;
use quick_xml::Reader;
use quick_xml::escape::{resolve_predefined_entity, unescape};
use quick_xml::events::Event;
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// `company_tickers_mf.json`'s own address, unversioned and unparameterized
/// -- SEC republishes it in place rather than dating each edition.
const TICKER_URL: &str = "https://www.sec.gov/files/company_tickers_mf.json";

/// One filing: the date it reports as of, what the fund calls itself, and
/// every holding it lists.
#[derive(Debug)]
pub struct Filing {
    pub report_date: NaiveDate,
    /// `genInfo/seriesName` -- the fund's own name, as it files it.
    ///
    /// The *series* name and not the registrant's: `regName` beside it is the
    /// trust, which holds dozens of unrelated funds ("Fidelity Concord Street
    /// Trust", "SCHWAB CAPITAL TRUST"), and is shouted in some filings and
    /// title-cased in others.
    ///
    /// A series, not a share class. Two classes of one fund share a
    /// `seriesId` and therefore this name, so it says which fund a ticker is
    /// and never which class -- the ticker beside it is what says that.
    ///
    /// `Option` because it is not what the filing is fetched *for*: a
    /// composition with no name on it is a complete answer to the question
    /// `g` asks, and refusing the whole refresh over a missing courtesy label
    /// would make a filing SEC accepted unreadable here. Every filing
    /// measured carries one.
    pub series_name: Option<String>,
    pub holdings: Vec<RawHolding>,
}

/// What every request here declares itself as. SEC refuses to answer a
/// request with no contact in its `User-Agent` at all, which is what makes
/// `contact` a required runtime parameter rather than a constant -- a real
/// address may never be a literal in this repository.
fn user_agent(contact: &str) -> String {
    format!("MisterManager/1 ({contact})")
}

/// Every ticker asked about, resolved to the SEC series it belongs to.
///
/// `company_tickers_mf.json` lists roughly thirty thousand fund share
/// classes; a caller here wants a handful. Nothing about the parsed file is
/// kept once the requested rows are found -- caching the other twenty-nine
/// thousand-odd to look up ten tickers is the wrong trade, and the file is
/// small enough (~1.2 MB) that re-fetching it next time is cheaper than a
/// cache invalidation policy would be.
///
/// Two share classes of one fund carry the same `seriesId`, which is exactly
/// why the map is keyed by ticker rather than the reverse.
pub fn resolve_series(contact: &str, tickers: &[String]) -> Result<HashMap<String, String>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the runtime for the ticker lookup")?;

    runtime.block_on(async {
        let body = fetch_text(contact, TICKER_URL).await?;
        parse_ticker_file(&body, tickers)
    })
}

/// `{"fields": [...], "data": [[...], ...]}` -- rows are positional arrays
/// against `fields`, not objects, so a field's index is looked up rather
/// than assumed: reading position 1 for `seriesId` would quietly read the
/// wrong column the day SEC reorders the file.
#[derive(Deserialize)]
struct TickerMfFile {
    fields: Vec<String>,
    data: Vec<Vec<serde_json::Value>>,
}

fn parse_ticker_file(body: &str, tickers: &[String]) -> Result<HashMap<String, String>> {
    let file: TickerMfFile =
        serde_json::from_str(body).context("parsing company_tickers_mf.json")?;

    let field_index = |name: &str| -> Result<usize> {
        file.fields
            .iter()
            .position(|f| f == name)
            .ok_or_else(|| anyhow!("company_tickers_mf.json has no {name:?} field"))
    };
    let series_idx = field_index("seriesId")?;
    let symbol_idx = field_index("symbol")?;

    let mut resolved = HashMap::new();
    for row in &file.data {
        let Some(symbol) = row.get(symbol_idx).and_then(|v| v.as_str()) else {
            continue;
        };
        if !tickers.iter().any(|t| t == symbol) {
            continue;
        }
        let Some(series_id) = row.get(series_idx).and_then(|v| v.as_str()) else {
            continue;
        };
        resolved.insert(symbol.to_string(), series_id.to_string());
    }
    Ok(resolved)
}

/// A series' newest NPORT-P filing.
///
/// The series id goes in the `CIK` slot of `browse-edgar`'s query string --
/// see [`browse_edgar_url`] for why -- and the feed's one `<filing-href>`
/// names an `-index.htm` page whose directory holds `primary_doc.xml`
/// alongside it.
pub fn latest_filing(contact: &str, series_id: &str) -> Result<Filing> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the runtime for the filing fetch")?;

    runtime.block_on(async {
        let atom = fetch_text(contact, &browse_edgar_url(series_id)).await?;
        let index_href = extract_filing_href(&atom)?;
        let dir = index_href
            .rsplit_once('/')
            .map(|(dir, _)| dir)
            .ok_or_else(|| anyhow!("filing-href {index_href:?} carries no directory"))?;
        let doc_url = format!("{dir}/primary_doc.xml");
        let xml = fetch_bytes(contact, &doc_url).await?;
        parse_filing(&xml)
    })
}

/// The series id, not the fund's own CIK: `browse-edgar` reads the `CIK`
/// slot as whichever entity id it is given, and a trust's CIK there would
/// return every series that trust ever filed for rather than this one's
/// alone.
fn browse_edgar_url(series_id: &str) -> String {
    format!(
        "https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK={series_id}&type=NPORT-P&count=1&output=atom"
    )
}

/// The feed's one `<filing-href>`. `count=1` in the query string already
/// narrowed the feed to a single entry, so nothing else in it is read.
///
/// Trimmed before it is handed back: an untrimmed trailing newline would
/// ride into [`latest_filing`]'s `rsplit_once('/')` and turn a clean
/// directory into a malformed second request rather than an error naming
/// what was wrong.
fn extract_filing_href(atom: &str) -> Result<String> {
    let mut reader = Reader::from_reader(atom.as_bytes());
    let mut pending = false;
    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(e) if e.local_name().as_ref() == b"filing-href" => pending = true,
            Event::End(e) if e.local_name().as_ref() == b"filing-href" => pending = false,
            Event::Text(e) if pending => {
                let href = unescape(&e.decode()?)?.into_owned();
                return Ok(href.trim().to_string());
            }
            _ => {}
        }
    }
    bail!("the filing feed carries no filing-href")
}

/// How many extra attempts a throttled request gets, beyond `query::send`'s
/// own three. SEC's own limit is 10/second, so a request still throttled
/// after this many whole-second backoffs is not going to clear on a fourth.
const MAX_THROTTLE_RETRIES: u32 = 3;

/// The text SEC's automated-tool firewall answers with when it decides to
/// throttle a request: a 403 that looks like an ordinary permission failure
/// to `query::send`, which retries 5xx and 429 but leaves every other
/// non-2xx alone. A plain 403 -- a bad URL, or a contact SEC still refuses --
/// carries no such text and is left to fail on the first try.
const THROTTLE_MARKER: &str = "Undeclared Automated Tool";

fn is_sec_throttle(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("HTTP 403") && message.contains(THROTTLE_MARKER)
}

async fn send_with_retry(client: &Client, url: &str, contact: &str) -> Result<reqwest::Response> {
    let mut attempt = 0u32;
    loop {
        let request = client
            .get(url)
            .header(reqwest::header::USER_AGENT, user_agent(contact));
        match query::send(request).await {
            Ok(response) => return Ok(response),
            Err(error) if attempt < MAX_THROTTLE_RETRIES && is_sec_throttle(&error) => {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(attempt as u64)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn fetch_text(contact: &str, url: &str) -> Result<String> {
    let client = query::http_client()?;
    let response = send_with_retry(client, url, contact).await?;
    response
        .text()
        .await
        .with_context(|| format!("reading response body from {url}"))
}

async fn fetch_bytes(contact: &str, url: &str) -> Result<Vec<u8>> {
    let client = query::http_client()?;
    let response = send_with_retry(client, url, contact).await?;
    let body = response
        .bytes()
        .await
        .with_context(|| format!("reading response body from {url}"))?;
    Ok(body.to_vec())
}

/// Which of a holding's fields the next `Text` event fills in -- set on the
/// matching `Start`, consumed by the following `Text`, and never carried
/// past the `End` that closes it.
enum Field {
    ReportDate,
    SeriesName,
    Name,
    Title,
    Cusip,
    PctVal,
    AssetCat,
    InvCountry,
}

/// A holding as it is read out of one `<invstOrSec>`.
///
/// `pct_val` is the one field with no sensible default: every other scalar
/// stands in for the filing having simply not said something, but a missing
/// weight is not "zero" the way a missing title is "no title". A holding read
/// as `0.0` here is a holding this parser dropped, and `classify` has no way
/// to tell it from a filing that genuinely under-reports -- so the weight
/// goes to `Unclassified`, and a fund reads as unplaceable when what is
/// actually wrong is that a row was lost on the way in. `Option` is what
/// keeps that distinguishable from an honestly-reported zero for as long as
/// it can be.
struct PendingHolding {
    name: String,
    title: String,
    cusip: String,
    pct_val: Option<f64>,
    asset_cat: String,
    inv_country: String,
}

impl PendingHolding {
    fn new() -> Self {
        Self {
            name: String::new(),
            title: String::new(),
            cusip: String::new(),
            pct_val: None,
            asset_cat: String::new(),
            inv_country: String::new(),
        }
    }

    /// `ordinal` is this holding's 1-based position among the filing's
    /// holdings, read so far -- a filing can list thousands, and "some
    /// holding somewhere has no pctVal" is not an error a reader can act on.
    fn finish(self, ordinal: usize) -> Result<RawHolding> {
        let pct_val = self.pct_val.ok_or_else(|| {
            anyhow!(
                "invstOrSec #{ordinal} ({:?}, cusip {:?}) carries no pctVal",
                self.name,
                self.cusip
            )
        })?;
        Ok(RawHolding {
            name: self.name,
            title: self.title,
            cusip: self.cusip,
            pct_val,
            asset_cat: self.asset_cat,
            inv_country: self.inv_country,
        })
    }
}

/// Turns a filing's raw XML into a [`Filing`]. Streamed with
/// [`quick_xml::Reader`] rather than parsed into a tree: a 3.2-20.6 MB
/// filing is normal, and materializing one as a DOM before reading it back
/// out would hold the whole document twice over for nothing.
///
/// Only the elements [`RawHolding`] and [`Filing::report_date`] need are
/// tracked; everything else in the filing -- `fundInfo`, `derivativeInfo`,
/// identifiers other than `cusip` -- is walked over rather than read.
///
/// **A holding's fields are read as its direct children and at no other
/// depth**, which is what `holding_depth` is counted for. N-PORT nests
/// subtrees inside `<invstOrSec>` that carry the very element names this
/// reads: a repurchase agreement's `repurchaseCollaterals/
/// repurchaseCollateral` has an `<assetCat>` of its own, and the derivative
/// subtrees carry name-, title- and cusip-shaped leaves. Matched at any
/// depth, the last of those to be walked wins, and a repo position takes its
/// collateral's category into `classify` -- a holding reclassified out of
/// the class the filing actually put it in, with nothing anywhere to say so.
pub fn parse_filing(xml: &[u8]) -> Result<Filing> {
    let mut reader = Reader::from_reader(xml);

    let mut report_date: Option<String> = None;
    let mut series_name: Option<String> = None;
    let mut in_gen_info = false;
    let mut current: Option<PendingHolding> = None;
    let mut holdings: Vec<RawHolding> = Vec::new();
    let mut pending: Option<Field> = None;
    // What the field being read has said so far. A leaf's content arrives in
    // as many events as it has entity references in it -- quick-xml reports
    // `&amp;` as a `GeneralRef` of its own, between the two `Text` events
    // either side of it -- so a field taken from the first of them would read
    // `S&P 500 Index Fund` as `S`. Accumulated here and applied whole at the
    // closing tag instead.
    let mut buffer = String::new();
    // How many elements are open, and which of those depths the holding
    // being read was opened at -- so `depth == holding_depth + 1` is "a
    // direct child of this `<invstOrSec>`" and nothing deeper qualifies.
    let mut depth: usize = 0;
    let mut holding_depth: Option<usize> = None;

    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(e) => {
                depth += 1;
                let child_of_holding = current.is_some() && holding_depth == Some(depth - 1);
                match e.local_name().as_ref() {
                    b"genInfo" => in_gen_info = true,
                    b"repPdDate" if in_gen_info => pending = Some(Field::ReportDate),
                    b"seriesName" if in_gen_info => pending = Some(Field::SeriesName),
                    b"invstOrSec" => {
                        current = Some(PendingHolding::new());
                        holding_depth = Some(depth);
                    }
                    b"name" if child_of_holding => pending = Some(Field::Name),
                    b"title" if child_of_holding => pending = Some(Field::Title),
                    b"cusip" if child_of_holding => pending = Some(Field::Cusip),
                    b"pctVal" if child_of_holding => pending = Some(Field::PctVal),
                    b"assetCat" if child_of_holding => pending = Some(Field::AssetCat),
                    b"invCountry" if child_of_holding => pending = Some(Field::InvCountry),
                    _ => {}
                }
                // Whatever the last field left behind, so a leaf starts empty.
                // A field is opened and closed before any other element is,
                // every one of them being a leaf.
                if pending.is_some() {
                    buffer.clear();
                }
            }
            // A self-closing element carries no text, so it is opened and
            // closed in this one step -- there is no `Text` event coming to
            // fill `pending`, and setting it anyway would let the next
            // sibling's inter-element whitespace fill this field instead.
            // `<invstOrSec/>` is routed through the same `finish` a normal
            // close uses rather than materialized directly: it carries no
            // `pctVal` either, so it is the same error rather than a
            // zero-weight row that quietly inflates `holdings.len()` --
            // which is the fund-of-funds/direct-fund fork's only input.
            Event::Empty(e) => {
                if e.local_name().as_ref() == b"invstOrSec" {
                    holdings.push(PendingHolding::new().finish(holdings.len() + 1)?);
                }
            }
            Event::Text(e) => {
                if pending.is_some() {
                    buffer.push_str(&unescape(&e.decode()?)?);
                }
            }
            // One entity reference, reported on its own between the text
            // either side of it. Resolved here rather than left to
            // `unescape`, which never sees it: by the time a reference
            // reaches this loop quick-xml has already cut it out of the text.
            // A reference nothing here can resolve is carried through as it
            // was written -- a name drawn with `&nbsp;` in it says more than
            // a name with a hole where a character was.
            Event::GeneralRef(e) => {
                if pending.is_some() {
                    let name = e.decode()?;
                    match e.resolve_char_ref()? {
                        Some(resolved) => buffer.push(resolved),
                        None => match resolve_predefined_entity(&name) {
                            Some(resolved) => buffer.push_str(resolved),
                            None => buffer.push_str(&format!("&{name};")),
                        },
                    }
                }
            }
            Event::End(e) => {
                // The whole of what the leaf said, at the tag that closes it.
                //
                // Trimmed once, here, for every field alike: a filer whose
                // generator pretty-prints leaf content --
                // `<invCountry>\n  US\n</invCountry>` -- must not send a
                // holding to the wrong class (or the wrong number format)
                // with no error anywhere. An empty leaf writes nothing at
                // all, so `<pctVal></pctVal>` is the missing figure
                // `PendingHolding::finish` names rather than a number that
                // would not parse.
                if let Some(field) = pending.take() {
                    let text = buffer.trim();
                    // `current` is set on every `Start` that also sets
                    // `pending` to a holding field, so it is always present
                    // here; `ReportDate` and `SeriesName` are the two
                    // variants that do not touch it.
                    if !text.is_empty() {
                        match field {
                            Field::ReportDate => report_date = Some(text.to_string()),
                            Field::SeriesName => series_name = Some(text.to_string()),
                            Field::Name => current.as_mut().unwrap().name = text.to_string(),
                            Field::Title => current.as_mut().unwrap().title = text.to_string(),
                            Field::Cusip => current.as_mut().unwrap().cusip = text.to_string(),
                            Field::PctVal => {
                                current.as_mut().unwrap().pct_val =
                                    Some(text.parse().with_context(|| {
                                        format!("pctVal {text:?} is not a number")
                                    })?);
                            }
                            Field::AssetCat => {
                                current.as_mut().unwrap().asset_cat = text.to_string();
                            }
                            Field::InvCountry => {
                                current.as_mut().unwrap().inv_country = text.to_string();
                            }
                        }
                    }
                }
                match e.local_name().as_ref() {
                    b"genInfo" => in_gen_info = false,
                    b"invstOrSec" => {
                        if let Some(holding) = current.take() {
                            holdings.push(holding.finish(holdings.len() + 1)?);
                        }
                        holding_depth = None;
                    }
                    _ => {}
                }
                // The field was taken above, whether or not the leaf said
                // anything, so nothing survives this tag to catch the
                // whitespace before the next sibling.
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }

    let report_date =
        report_date.ok_or_else(|| anyhow!("the filing carries no genInfo/repPdDate"))?;
    let report_date = NaiveDate::parse_from_str(&report_date, "%Y-%m-%d")
        .with_context(|| format!("repPdDate {report_date:?} is not a date"))?;

    // An empty `<seriesName/>` is no name rather than a name of nothing --
    // the same reading `RawHolding` gives an empty `<title>`, one absence
    // spelled one way.
    let series_name = series_name.filter(|name| !name.trim().is_empty());

    Ok(Filing {
        report_date,
        series_name,
        holdings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::day;

    #[test]
    fn a_filings_series_name_is_read_and_the_registrants_is_not() {
        let filing = parse_filing(
            br#"<edgarSubmission>
                  <formData><genInfo>
                    <regName>SOME CAPITAL TRUST</regName>
                    <seriesName>Total Market Index Fund</seriesName>
                    <repPdDate>2026-06-30</repPdDate>
                  </genInfo></formData>
                </edgarSubmission>"#,
        )
        .unwrap();
        assert_eq!(
            filing.series_name.as_deref(),
            Some("Total Market Index Fund")
        );
    }

    /// A leaf's content arrives in as many events as it has entity
    /// references in it, so a field read off the first of them stops at the
    /// first `&` -- which is the character an index fund's name is most
    /// likely to carry. The holding fields are read the same way and are
    /// what `classify` matches on, so the same cut would send `AT&T Inc` to
    /// a class chosen on the strength of `AT`.
    #[test]
    fn an_entity_reference_inside_a_field_is_read_as_part_of_it() {
        let filing = parse_filing(
            br#"<edgarSubmission>
                  <formData><genInfo>
                    <seriesName>Acme S&amp;P 500 Index Fund</seriesName>
                    <repPdDate>2026-06-30</repPdDate>
                  </genInfo>
                  <invstOrSecs><invstOrSec>
                    <name>AT&#38;T Inc</name>
                    <title>AT&amp;T Inc</title>
                    <cusip>000000000</cusip>
                    <pctVal>100.0</pctVal>
                    <assetCat>EC</assetCat>
                    <invCountry>US</invCountry>
                  </invstOrSec></invstOrSecs></formData>
                </edgarSubmission>"#,
        )
        .unwrap();

        assert_eq!(
            filing.series_name.as_deref(),
            Some("Acme S&P 500 Index Fund")
        );
        assert_eq!(filing.holdings[0].name, "AT&T Inc");
        assert_eq!(filing.holdings[0].title, "AT&T Inc");
    }

    /// A filing that carries no name is still a filing: the composition is
    /// what `g` asked for, and refusing the whole refresh over a missing
    /// label would make a document SEC accepted unreadable here. An empty
    /// element reads the same way a missing one does -- one absence, one
    /// spelling.
    #[test]
    fn a_filing_with_no_series_name_still_parses_and_an_empty_one_is_no_name() {
        for gen_info in [
            "<repPdDate>2026-06-30</repPdDate>",
            "<seriesName></seriesName><repPdDate>2026-06-30</repPdDate>",
            "<seriesName>   </seriesName><repPdDate>2026-06-30</repPdDate>",
        ] {
            let xml = format!(
                "<edgarSubmission><formData><genInfo>{gen_info}</genInfo></formData></edgarSubmission>"
            );
            let filing = parse_filing(xml.as_bytes()).unwrap();
            assert_eq!(filing.series_name, None, "{gen_info}");
            assert_eq!(filing.report_date, day(2026, 6, 30));
        }
    }

    #[test]
    fn a_filings_report_date_is_read_from_its_general_information() {
        let filing = parse_filing(include_bytes!(
            "../../tests/fixtures/nport_fund_of_funds.xml"
        ))
        .unwrap();
        assert_eq!(filing.report_date, day(2026, 6, 30));
    }

    #[test]
    fn every_holding_in_a_filing_is_read_with_its_percentage() {
        let filing = parse_filing(include_bytes!(
            "../../tests/fixtures/nport_fund_of_funds.xml"
        ))
        .unwrap();

        assert_eq!(filing.holdings.len(), 4);
        let total: f64 = filing.holdings.iter().map(|h| h.pct_val).sum();
        assert!(
            (total - 100.0).abs() < 0.01,
            "the fixture's holdings do not foot"
        );
    }

    #[test]
    fn a_direct_funds_securities_are_read_with_their_categories_and_countries() {
        let filing = parse_filing(include_bytes!("../../tests/fixtures/nport_direct.xml")).unwrap();

        assert_eq!(filing.holdings.len(), 30);
        assert!(filing.holdings.iter().any(|h| h.inv_country != "US"));
        // The fixture's `<title></title>` carries no separate title, same
        // as a real security with none. This is what a real filing looks
        // like, not a regression guard -- see
        // `stray_text_between_elements_is_never_assigned_to_the_wrong_leaf`
        // below for the test that actually distinguishes the fixed parser
        // from the broken one: real filings carry no stray text, so
        // whitespace here trims to the same empty string either way.
        assert!(filing.holdings[0].title.is_empty());
    }

    /// The two shapes of "character data belongs to no leaf" that the
    /// self-review fix in `parse_filing` closed, pinned with non-whitespace
    /// text so `trim()` cannot make a broken parser and a correct one agree
    /// by accident -- which is exactly what happened the first time this
    /// was pinned only with inter-element indentation.
    ///
    /// `<title></title>ZZZ` is an empty leaf (no `Text` event of its own)
    /// followed by stray text before the next element: if `Event::End`
    /// does not clear `pending`, "ZZZ" is read as `title`'s content instead
    /// of being ignored. `<cusip/>YYY` is a self-closing leaf followed by
    /// stray text: if `Event::Empty` sets `pending` the way `Event::Start`
    /// does, "YYY" is read as `cusip`'s content instead of being ignored.
    /// The two assertions are independent -- each catches only its own half
    /// reverted, which is what proves the test pins both rather than one
    /// masking the other.
    #[test]
    fn stray_text_between_elements_is_never_assigned_to_the_wrong_leaf() {
        let xml = br#"<edgarSubmission><formData>
            <genInfo><repPdDate>2026-06-30</repPdDate></genInfo>
            <invstOrSecs>
                <invstOrSec>
                    <name>Alpha</name><title></title>ZZZ<cusip/>YYY<pctVal>100.0</pctVal>
                    <assetCat>EC</assetCat><invCountry>US</invCountry>
                </invstOrSec>
            </invstOrSecs>
        </formData></edgarSubmission>"#;

        let filing = parse_filing(xml).unwrap();

        assert_eq!(filing.holdings.len(), 1);
        let holding = &filing.holdings[0];
        assert_eq!(holding.name, "Alpha");
        assert_eq!(
            holding.title, "",
            "an empty leaf's End did not clear pending"
        );
        assert_eq!(holding.cusip, "", "a self-closing leaf's Empty set pending");
        assert_eq!(holding.pct_val, 100.0);
        assert_eq!(holding.asset_cat, "EC");
        assert_eq!(holding.inv_country, "US");
    }

    /// N-PORT nests subtrees inside `<invstOrSec>` that carry the same
    /// element names a holding's own fields do: a repurchase agreement's
    /// collateral has an `<assetCat>`, and the derivative subtrees carry
    /// name-, title- and cusip-shaped leaves. Read at any depth, the nested
    /// ones are walked last and win, so a repo position is classified as its
    /// collateral -- money moved to another class with nothing on the row
    /// saying it was. Neither committed fixture nests anything, which is why
    /// this is written out here.
    #[test]
    fn a_nested_subtrees_fields_never_overwrite_the_holdings_own() {
        let xml = br#"<edgarSubmission><formData>
            <genInfo><repPdDate>2026-06-30</repPdDate></genInfo>
            <invstOrSecs>
                <invstOrSec>
                    <name>A Held Company</name>
                    <title>A Held Company</title>
                    <cusip>000000001</cusip>
                    <pctVal>100.0</pctVal>
                    <assetCat>DBT</assetCat>
                    <invCountry>US</invCountry>
                    <repurchaseAgrmt>
                        <repurchaseCollaterals>
                            <repurchaseCollateral>
                                <name>Some Collateral</name>
                                <title>Some Collateral</title>
                                <cusip>000000009</cusip>
                                <pctVal>12.5</pctVal>
                                <assetCat>EC</assetCat>
                                <invCountry>DE</invCountry>
                            </repurchaseCollateral>
                        </repurchaseCollaterals>
                    </repurchaseAgrmt>
                </invstOrSec>
            </invstOrSecs>
        </formData></edgarSubmission>"#;

        let filing = parse_filing(xml).unwrap();

        assert_eq!(filing.holdings.len(), 1);
        let holding = &filing.holdings[0];
        assert_eq!(holding.asset_cat, "DBT", "the collateral's category won");
        assert_eq!(holding.inv_country, "US", "the collateral's country won");
        assert_eq!(holding.name, "A Held Company");
        assert_eq!(holding.title, "A Held Company");
        assert_eq!(holding.cusip, "000000001");
        assert_eq!(holding.pct_val, 100.0);
    }

    #[test]
    fn a_document_that_is_not_a_filing_is_an_error_naming_what_was_missing() {
        let err = parse_filing(b"<nonsense/>").unwrap_err();
        assert!(
            err.to_string().contains("repPdDate"),
            "the error does not name the missing element: {err}"
        );
    }

    /// A holding read as `0.0` is one this parser lost, and `classify`
    /// cannot tell that from a filing that under-reports -- it would send
    /// the weight to `Unclassified` and call the fund unplaceable. This is
    /// the "absent entirely" shape: no `<pctVal>` at all.
    #[test]
    fn a_holding_with_no_percentage_element_is_an_error_naming_the_holding() {
        let xml = br#"<edgarSubmission><formData>
            <genInfo><repPdDate>2026-06-30</repPdDate></genInfo>
            <invstOrSecs>
                <invstOrSec>
                    <name>Some Fund</name>
                    <cusip>000000005</cusip>
                </invstOrSec>
            </invstOrSecs>
        </formData></edgarSubmission>"#;

        let err = parse_filing(xml).unwrap_err();
        assert!(err.to_string().contains("pctVal"), "{err}");
        assert!(err.to_string().contains("Some Fund"), "{err}");
    }

    /// The "open and closed with no text" shape -- `<pctVal></pctVal>` --
    /// which emits no `Text` event at all and so must not read as `0.0`
    /// either.
    #[test]
    fn a_holding_with_an_empty_percentage_element_is_an_error() {
        let xml = br#"<edgarSubmission><formData>
            <genInfo><repPdDate>2026-06-30</repPdDate></genInfo>
            <invstOrSecs>
                <invstOrSec>
                    <name>Some Fund</name>
                    <pctVal></pctVal>
                </invstOrSec>
            </invstOrSecs>
        </formData></edgarSubmission>"#;

        let err = parse_filing(xml).unwrap_err();
        assert!(err.to_string().contains("pctVal"), "{err}");
    }

    /// The third shape a missing weight can take: the holding element
    /// itself is self-closing, which must not silently materialize a
    /// zero-weight row that inflates `holdings.len()` -- the fund-of-funds
    /// versus direct-fund fork's only input.
    #[test]
    fn a_self_closing_holding_element_is_an_error_rather_than_a_phantom_row() {
        let xml = br#"<edgarSubmission><formData>
            <genInfo><repPdDate>2026-06-30</repPdDate></genInfo>
            <invstOrSecs>
                <invstOrSec/>
            </invstOrSecs>
        </formData></edgarSubmission>"#;

        let err = parse_filing(xml).unwrap_err();
        assert!(err.to_string().contains("pctVal"), "{err}");
    }

    /// A filer whose generator pretty-prints leaf content --
    /// `<invCountry>\n  US\n</invCountry>` -- must not send a holding to
    /// the wrong class because of surrounding whitespace `classify`'s exact
    /// string matches never tolerate.
    #[test]
    fn a_holdings_category_and_country_are_trimmed_before_they_are_stored() {
        let xml = br#"<edgarSubmission><formData>
            <genInfo><repPdDate>2026-06-30</repPdDate></genInfo>
            <invstOrSecs>
                <invstOrSec>
                    <name>Some Fund</name>
                    <pctVal>100.0</pctVal>
                    <assetCat>
                        EC
                    </assetCat>
                    <invCountry>
                        US
                    </invCountry>
                </invstOrSec>
            </invstOrSecs>
        </formData></edgarSubmission>"#;

        let filing = parse_filing(xml).unwrap();
        assert_eq!(filing.holdings[0].asset_cat, "EC");
        assert_eq!(filing.holdings[0].inv_country, "US");
    }

    /// The marker text is what tells SEC's own throttle apart from a 403
    /// that means something else entirely -- a wrong URL, or a contact SEC
    /// still refuses. Getting this wrong in either direction either retries
    /// a request that will never succeed or gives up on one three attempts
    /// from clearing.
    #[test]
    fn a_403_carrying_secs_throttle_text_is_recognized_as_a_throttle() {
        let error = anyhow!(
            "HTTP 403 from https://www.sec.gov/foo: Your Request Originates from \
             an Undeclared Automated Tool"
        );
        assert!(is_sec_throttle(&error));
    }

    #[test]
    fn a_plain_403_is_not_mistaken_for_a_throttle() {
        let error = anyhow!("HTTP 403 from https://www.sec.gov/foo: Forbidden");
        assert!(!is_sec_throttle(&error));
    }

    /// `company_tickers_mf.json`'s rows are positional against `fields`,
    /// not fixed -- this fixture deliberately spells them in a different
    /// order than the brief's own example, which is what a test pinning
    /// "resolved by name" has to do to actually distinguish it from
    /// "resolved by position and it happened to still work."
    #[test]
    fn parse_ticker_file_resolves_columns_by_field_name_rather_than_position() {
        let body = r#"{
            "fields": ["symbol", "cik", "classId", "seriesId"],
            "data": [["USM", 1, "C000000001", "S000000002"]]
        }"#;

        let resolved = parse_ticker_file(body, &["USM".to_string()]).unwrap();

        assert_eq!(resolved.get("USM"), Some(&"S000000002".to_string()));
    }

    /// Thirty thousand-odd rows and a caller wanting a handful: only the
    /// requested tickers may survive into the map.
    #[test]
    fn parse_ticker_file_keeps_only_rows_matching_a_requested_ticker() {
        let body = r#"{
            "fields": ["cik", "seriesId", "classId", "symbol"],
            "data": [
                [1, "S000000001", "C000000001", "TDF45"],
                [2, "S000000002", "C000000002", "USM"]
            ]
        }"#;

        let resolved = parse_ticker_file(body, &["USM".to_string()]).unwrap();

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved.get("USM"), Some(&"S000000002".to_string()));
        assert!(!resolved.contains_key("TDF45"));
    }

    /// The one fact this whole hop exists to get right: the series id, not
    /// the fund's own CIK, is what goes in the `CIK` slot.
    #[test]
    fn browse_edgar_url_puts_the_series_id_in_the_cik_slot() {
        let url = browse_edgar_url("S000000001");

        assert_eq!(
            url,
            "https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK=S000000001\
             &type=NPORT-P&count=1&output=atom"
        );
    }

    #[test]
    fn extract_filing_href_reads_the_feeds_one_entry() {
        let atom = "<feed><entry><filing-href>\
             https://www.sec.gov/Archives/edgar/data/1/000000000001-index.htm\
             </filing-href></entry></feed>";

        let href = extract_filing_href(atom).unwrap();

        assert_eq!(
            href,
            "https://www.sec.gov/Archives/edgar/data/1/000000000001-index.htm"
        );
    }

    /// An untrimmed href would ride its surrounding whitespace into
    /// `latest_filing`'s `rsplit_once('/')` and turn a clean directory into
    /// a malformed second request.
    #[test]
    fn extract_filing_href_trims_surrounding_whitespace() {
        let atom = "<feed><entry><filing-href>\n  \
             https://www.sec.gov/Archives/edgar/data/1/x-index.htm\n  \
             </filing-href></entry></feed>";

        let href = extract_filing_href(atom).unwrap();

        assert_eq!(
            href,
            "https://www.sec.gov/Archives/edgar/data/1/x-index.htm"
        );
    }

    #[test]
    fn extract_filing_href_errors_when_the_feed_carries_no_filing_href() {
        let err = extract_filing_href("<feed><entry></entry></feed>").unwrap_err();
        assert!(err.to_string().contains("filing-href"));
    }
}
