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
use quick_xml::escape::unescape;
use quick_xml::events::Event;
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// `company_tickers_mf.json`'s own address, unversioned and unparameterized
/// -- SEC republishes it in place rather than dating each edition.
const TICKER_URL: &str = "https://www.sec.gov/files/company_tickers_mf.json";

/// One filing: the date it reports as of, and every holding it lists.
#[derive(Debug)]
pub struct Filing {
    pub report_date: NaiveDate,
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
fn extract_filing_href(atom: &str) -> Result<String> {
    let mut reader = Reader::from_reader(atom.as_bytes());
    let mut pending = false;
    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(e) if e.local_name().as_ref() == b"filing-href" => pending = true,
            Event::End(e) if e.local_name().as_ref() == b"filing-href" => pending = false,
            Event::Text(e) if pending => {
                return Ok(unescape(&e.decode()?)?.into_owned());
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
    Name,
    Title,
    Cusip,
    PctVal,
    AssetCat,
    InvCountry,
}

/// Turns a filing's raw XML into a [`Filing`]. Streamed with
/// [`quick_xml::Reader`] rather than parsed into a tree: a 3.2-20.6 MB
/// filing is normal, and materializing one as a DOM before reading it back
/// out would hold the whole document twice over for nothing.
///
/// Only the elements [`RawHolding`] and [`Filing::report_date`] need are
/// tracked; everything else in the filing -- `fundInfo`, `derivativeInfo`,
/// identifiers other than `cusip` -- is walked over rather than read.
pub fn parse_filing(xml: &[u8]) -> Result<Filing> {
    let mut reader = Reader::from_reader(xml);

    let mut report_date: Option<String> = None;
    let mut in_gen_info = false;
    let mut current: Option<RawHolding> = None;
    let mut holdings = Vec::new();
    let mut pending: Option<Field> = None;

    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(e) => match e.local_name().as_ref() {
                b"genInfo" => in_gen_info = true,
                b"repPdDate" if in_gen_info => pending = Some(Field::ReportDate),
                b"invstOrSec" => {
                    current = Some(RawHolding {
                        name: String::new(),
                        title: String::new(),
                        cusip: String::new(),
                        pct_val: 0.0,
                        asset_cat: String::new(),
                        inv_country: String::new(),
                    });
                }
                b"name" if current.is_some() => pending = Some(Field::Name),
                b"title" if current.is_some() => pending = Some(Field::Title),
                b"cusip" if current.is_some() => pending = Some(Field::Cusip),
                b"pctVal" if current.is_some() => pending = Some(Field::PctVal),
                b"assetCat" if current.is_some() => pending = Some(Field::AssetCat),
                b"invCountry" if current.is_some() => pending = Some(Field::InvCountry),
                _ => {}
            },
            // A self-closing element carries no text, so it is opened and
            // closed in this one step -- there is no `Text` event coming to
            // fill `pending`, and setting it anyway would let the next
            // sibling's inter-element whitespace fill this field instead.
            Event::Empty(e) => {
                if e.local_name().as_ref() == b"invstOrSec" {
                    holdings.push(RawHolding {
                        name: String::new(),
                        title: String::new(),
                        cusip: String::new(),
                        pct_val: 0.0,
                        asset_cat: String::new(),
                        inv_country: String::new(),
                    });
                }
            }
            Event::Text(e) => {
                if let Some(field) = pending.take() {
                    let text = unescape(&e.decode()?)?.into_owned();
                    // `current` is set on every `Start` that also sets
                    // `pending` to a holding field, so it is always present
                    // here; `ReportDate` is the one variant that does not
                    // touch it.
                    match field {
                        Field::ReportDate => report_date = Some(text),
                        Field::Name => current.as_mut().unwrap().name = text,
                        Field::Title => current.as_mut().unwrap().title = text,
                        Field::Cusip => current.as_mut().unwrap().cusip = text,
                        Field::PctVal => {
                            current.as_mut().unwrap().pct_val = text
                                .trim()
                                .parse()
                                .with_context(|| format!("pctVal {text:?} is not a number"))?;
                        }
                        Field::AssetCat => current.as_mut().unwrap().asset_cat = text,
                        Field::InvCountry => current.as_mut().unwrap().inv_country = text,
                    }
                }
            }
            Event::End(e) => {
                match e.local_name().as_ref() {
                    b"genInfo" => in_gen_info = false,
                    b"invstOrSec" => {
                        if let Some(holding) = current.take() {
                            holdings.push(holding);
                        }
                    }
                    _ => {}
                }
                // A leaf's own closing tag is the next event after its
                // `Text` (if it had any), so this is always safe: an empty
                // leaf's `pending` is cleared here instead of surviving to
                // catch the whitespace before its next sibling.
                pending = None;
            }
            _ => {}
        }
    }

    let report_date =
        report_date.ok_or_else(|| anyhow!("the filing carries no genInfo/repPdDate"))?;
    let report_date = NaiveDate::parse_from_str(&report_date, "%Y-%m-%d")
        .with_context(|| format!("repPdDate {report_date:?} is not a date"))?;

    Ok(Filing {
        report_date,
        holdings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::day;

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
    }

    #[test]
    fn a_document_that_is_not_a_filing_is_an_error_naming_what_was_missing() {
        let err = parse_filing(b"<nonsense/>").unwrap_err();
        assert!(
            err.to_string().contains("repPdDate"),
            "the error does not name the missing element: {err}"
        );
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
}
