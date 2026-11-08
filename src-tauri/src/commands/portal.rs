//! Public "quantms portal" example source.
//!
//! The quantms portal (https://portal.quantms.org) publishes reanalysed QPX
//! datasets as public parquet files, browsable through a small HTTP service at
//! `https://browse.quantms.org`. The layout is:
//!
//! ```text
//! /quantms/datasets/<PXD-code>/<run-hash>/qpx/<PXD-code>.<view>.parquet
//! ```
//!
//! These files are freely downloadable over HTTPS (no AWS credentials needed),
//! so this module provides the app with a ready-made example source to open.

use crate::commands::portal_http;
use crate::commands::progress;
use crate::commands::transfers as xfers;
use crate::qpx::ViewInfo;
use reqwest::Client;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use tauri::AppHandle;

const BASE: &str = "https://browse.quantms.org";
const DATASETS_ROOT: &str = "/quantms/datasets/";

/// File and folder names taken from URLs must stay inside the cache folder.
fn safe_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && !s.contains("..")
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Cache folder for one run of a dataset (`<PXD>/<run-hash>`), so runs of the
/// same accession that share file names do not overwrite each other.
fn run_cache_dir(root: &std::path::Path, url: &str) -> PathBuf {
    let rest = url
        .strip_prefix(BASE)
        .and_then(|r| r.strip_prefix(DATASETS_ROOT))
        .unwrap_or("");
    let mut dir = root.to_path_buf();
    for seg in rest.split('/').take(2).filter(|s| safe_component(s)) {
        dir.push(seg);
    }
    dir
}

/// Size of a remote file. The portal answers HEAD with 405, so fall back to a
/// one-byte ranged GET and read the total from `Content-Range`.
async fn remote_size(client: &Client, url: &str, token: &AtomicBool) -> Result<u64, String> {
    // reqwest's `.content_length()` returns the *body* length, which is 0 for
    // a HEAD reply, so size HEADs by the Content-Length header directly.
    if let Ok(r) = client.head(url).send().await {
        if r.status().is_success() {
            let cl = r
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|n| *n > 0);
            if let Some(n) = cl {
                return Ok(n);
            }
        }
    }
    let total = xfers::cancellable(token, async {
        let r = client
            .get(url)
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("HTTP {}", r.status()));
        }
        let total = r
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|v| v.trim().parse::<u64>().ok());
        // A server that ignores Range sends the whole (200) body: use its
        // length. On a 206 without a size the probe read is one byte, which
        // is not a real total, so don't trust it.
        let status = r.status();
        Ok(total.or_else(|| {
            if status == reqwest::StatusCode::OK {
                r.content_length()
            } else {
                None
            }
        })
        .unwrap_or(0))
    })
    .await?;
    Ok(total)
}

fn cache_root() -> Result<PathBuf, String> {
    let base = dirs::cache_dir()
        .or_else(dirs::data_local_dir)
        .ok_or_else(|| "no cache directory available".to_string())?;
    Ok(base.join("qpxui").join("portal"))
}

async fn get_html(url: &str) -> Result<String, String> {
    let client = portal_http::client_builder()?.build().map_err(|e| format!("portal HTTP client: {e:?}"))?;
    let resp = client
        .get(url)
        .header("Accept", "text/html")
        .send()
        .await
        .map_err(|e| format!("request {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("request {url}: HTTP {}", resp.status()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("read {url}: {e}"))?;
    Ok(body)
}

/// Very small HTML anchor extractor: returns `(href, link_text)` pairs.
fn extract_links(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(rel) = rest.find("<a href=\"") {
        let after = &rest[rel + 9..];
        let Some(end_href) = after.find('"') else { break };
        let href = &after[..end_href];
        let rest2 = &after[end_href + 1..];
        let Some(gt) = rest2.find('>') else { break };
        let between = &rest2[gt + 1..];
        let Some(close) = between.find("</a>") else { break };
        let text = between[..close].trim();
        out.push((href.to_string(), text.to_string()));
        rest = &between[close + 4..];
    }
    out
}

/// Parse a human-friendly size string like `5,794,378 B`, `4.5 MB`, `1.2 GB`.
fn parse_size(raw: &str) -> u64 {
    // Strip any HTML tags (e.g. `<small>` around the number) and `&nbsp;`.
    let mut clean = String::new();
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => clean.push(ch),
            _ => {}
        }
    }
    let clean = clean
        .replace("&nbsp;", " ")
        .replace('\u{a0}', " ")
        .replace(',', "")
        .trim()
        .to_string();
    let lower = clean.to_lowercase();
    let (num_part, mult) = if lower.ends_with("gb") {
        (clean[..clean.len() - 2].trim(), 1024u64.pow(3))
    } else if lower.ends_with("mb") {
        (clean[..clean.len() - 2].trim(), 1024u64.pow(2))
    } else if lower.ends_with("kb") {
        (clean[..clean.len() - 2].trim(), 1024u64)
    } else if lower.ends_with('b') {
        (clean[..clean.len() - 1].trim(), 1u64)
    } else {
        (clean.trim(), 1u64)
    };
    let n: f64 = num_part
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    (n * mult as f64) as u64
}

/// `(<link text>, <size bytes>)` for the parquet links on a listing page.
fn parquet_entries(html: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(rel) = rest.find("<a href=\"") {
        let after = &rest[rel + 9..];
        let Some(end_href) = after.find('"') else { break };
        let href = &after[..end_href];
        if !href.ends_with(".parquet") {
            rest = &after[end_href..];
            continue;
        }
        let rest2 = &after[end_href + 1..];
        let Some(gt) = rest2.find('>') else { break };
        let between = &rest2[gt + 1..];
        let Some(close) = between.find("</a>") else { break };
        let name = between[..close].trim();
        // After `</a>` comes whitespace, `&nbsp;`, then the size, then `<br>`.
        let after_close = &between[close + 4..];
        let size_raw = after_close.split("<br").next().unwrap_or("");
        let size = parse_size(size_raw);
        out.push((name.to_string(), size));
        rest = &between[close + 4..];
    }
    out
}

/// List all dataset codes at the portal root (PXD…, IPX…, MSV…, PDC…, PMID…).
#[tauri::command]
pub async fn portal_datasets(limit: Option<usize>) -> Result<Vec<String>, String> {
    let url = format!("{BASE}{DATASETS_ROOT}");
    let html = get_html(&url).await?;
    let mut codes: Vec<String> = extract_links(&html)
        .into_iter()
        .map(|(href, _)| href.trim_end_matches('/').to_string())
        .filter_map(|h| h.rsplit('/').next().map(|s| s.to_string()))
        .filter(|name| {
            !name.is_empty()
                && name != ".."
                && name
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_uppercase())
                    .unwrap_or(false)
        })
        .collect();
    codes.sort();
    codes.dedup();
    if let Some(n) = limit {
        codes.truncate(n);
    }
    Ok(codes)
}

fn view_base() -> String {
    format!("{BASE}{DATASETS_ROOT}")
}

/// Registry metadata for a reanalysed dataset on the quantms portal.
///
/// The source API uses snake_case keys; the app's frontend uses camelCase, so
/// deserialization reads the snake_case fields and serialization emits camelCase.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PortalDataset {
    pub accession: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub organism: Option<String>,
    #[serde(default)]
    pub instrument: Option<String>,
    #[serde(default)]
    pub samples: Option<u64>,
    #[serde(default)]
    pub proteins: Option<u64>,
    #[serde(default)]
    pub peptides: Option<u64>,
    #[serde(default, rename(deserialize = "psm_count", serialize = "psmCount"))]
    pub psm_count: Option<u64>,
    #[serde(default, rename(deserialize = "total_size", serialize = "totalSize"))]
    pub total_size: Option<u64>,
    /// `accession/run-hash`, used to resolve the view URLs directly.
    #[serde(default, rename(deserialize = "dataset_ref", serialize = "datasetRef"))]
    pub dataset_ref: Option<String>,
    #[serde(default, rename(deserialize = "px_url", serialize = "pxUrl"))]
    pub px_url: Option<String>,
    #[serde(default, rename(deserialize = "collection_title", serialize = "collectionTitle"))]
    pub collection_title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub keywords: Option<Vec<String>>,
    #[serde(default)]
    pub diseases: Option<Vec<String>>,
    #[serde(default)]
    pub tissues: Option<Vec<String>>,
    #[serde(default)]
    pub publication: Option<Publication>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub collection: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Publication {
    #[serde(default)]
    pub doi: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct DatasetSearchPage {
    #[serde(default)]
    total_pages: usize,
    #[serde(default)]
    datasets: Vec<PortalDataset>,
}

/// List the searchable registry of reanalysed datasets from the quantms portal
/// (fetched from `api.quantms.org/dataset-search/datasets`, paging through all
/// results). This gives a rich, searchable catalogue rather than a bare list.
#[tauri::command]
pub async fn portal_registry(page_size: Option<usize>) -> Result<Vec<PortalDataset>, String> {
    let page_size = page_size.unwrap_or(50).clamp(1, 100);
    let client = portal_http::client_builder()?.build().map_err(|e| format!("portal HTTP client: {e:?}"))?;

    async fn get_page(
        client: &Client,
        url: &str,
    ) -> Result<DatasetSearchPage, String> {
        let mut last = "no attempt".to_string();
        for _ in 0..3 {
            match client.get(url).send().await {
                Ok(resp) => match resp.error_for_status() {
                    Ok(resp) => match resp.json::<DatasetSearchPage>().await {
                        Ok(page) => return Ok(page),
                        Err(e) => last = format!("parse: {e}"),
                    },
                    Err(e) => last = format!("http: {e}"),
                },
                Err(e) => last = format!("request: {e:?}"),
            }
        }
        Err(format!("{url}: {last}"))
    }

    let first = get_page(
        &client,
        &format!("https://api.quantms.org/dataset-search/datasets?page_size={page_size}&page=1"),
    )
    .await?;
    let total_pages = first.total_pages.max(1);
    let mut all = first.datasets;
    for p in 2..=total_pages {
        let page = get_page(
            &client,
            &format!("https://api.quantms.org/dataset-search/datasets?page_size={page_size}&page={p}"),
        )
        .await?;
        all.extend(page.datasets);
    }
    // One accession can hold several runs (e.g. PXD014877 has one per
    // organism), so the run ref, not the accession, identifies an entry.
    let key = |d: &PortalDataset| d.dataset_ref.clone().unwrap_or_else(|| d.accession.clone());
    all.sort_by(|a, b| a.accession.cmp(&b.accession).then_with(|| key(a).cmp(&key(b))));
    all.dedup_by(|a, b| key(a) == key(b));
    Ok(all)
}

/// One page of search results from the quantms portal dataset catalogue.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortalSearchResult {
    pub datasets: Vec<PortalDataset>,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub page: usize,
    #[serde(default, rename(serialize = "totalPages"))]
    pub total_pages: usize,
    #[serde(default)]
    pub sort: String,
}

#[derive(Debug, serde::Deserialize)]
struct SearchPageRaw {
    #[serde(default)]
    datasets: Vec<PortalDataset>,
    #[serde(default)]
    total: usize,
    #[serde(default)]
    page: usize,
    #[serde(default, rename = "total_pages")]
    total_pages: usize,
    #[serde(default)]
    sort: String,
}

/// Facet options shown in the discover UI. Organisms and instruments come from
/// `/facets`, collections from `/collections` (name + title + dataset count).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortalFacets {
    pub organisms: Vec<String>,
    pub instruments: Vec<String>,
    pub collections: Vec<CollectionOption>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionOption {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename(deserialize = "dataset_count", serialize = "datasetCount"))]
    pub dataset_count: usize,
}

/// Server-side search over the quantms portal dataset catalogue. Only the
/// filters the registry API actually supports are forwarded (`q`, collection,
/// organism, instrument, sort); count/lower-bound filters are not available
/// server-side, so the caller combines this with client-side sorting if needed.
#[tauri::command]
pub async fn portal_search(
    query: Option<String>,
    collection: Option<String>,
    organism: Option<String>,
    instrument: Option<String>,
    sort: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
) -> Result<PortalSearchResult, String> {
    let client = portal_http::client_builder()?.build().map_err(|e| format!("portal HTTP client: {e:?}"))?;

    let mut url = reqwest::Url::parse("https://api.quantms.org/dataset-search/datasets")
        .map_err(|e| format!("bad search URL: {e}"))?;
    {
        let mut qp = url.query_pairs_mut();
        let mut add = |param: &str, value: Option<&str>| {
            if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
                qp.append_pair(param, v);
            }
        };
        add("q", query.as_deref());
        add("collection", collection.as_deref());
        add("organism", organism.as_deref());
        add("instrument", instrument.as_deref());
        add("sort", sort.as_deref());
        qp.append_pair("page", &page.unwrap_or(1).max(1).to_string());
        qp.append_pair("page_size", &page_size.unwrap_or(50).clamp(1, 100).to_string());
    }

    let raw = client
        .get(url.clone())
        .send()
        .await
        .map_err(|e| format!("request: {e:?}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .json::<SearchPageRaw>()
        .await
        .map_err(|e| format!("parse: {e}"))?;

    Ok(PortalSearchResult {
        datasets: raw.datasets,
        total: raw.total,
        page: raw.page,
        total_pages: raw.total_pages.max(1),
        sort: raw.sort,
    })
}

/// Facet options (organisms, instruments, collections) for the discover UI.
#[tauri::command]
pub async fn portal_facets() -> Result<PortalFacets, String> {
    let client = portal_http::client_builder()?.build().map_err(|e| format!("portal HTTP client: {e:?}"))?;

    async fn get_json(client: &Client, url: &str) -> Result<serde_json::Value, String> {
        let mut last = "no attempt".to_string();
        for _ in 0..3 {
            match client.get(url).send().await {
                Ok(resp) => match resp.error_for_status() {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(v) => return Ok(v),
                        Err(e) => last = format!("parse: {e}"),
                    },
                    Err(e) => last = format!("http: {e}"),
                },
                Err(e) => last = format!("request: {e:?}"),
            }
        }
        Err(format!("{url}: {last}"))
    }

    let values = |raw: &serde_json::Value, bucket: &str| -> Vec<String> {
        raw.get(bucket)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|f| f.get("value").and_then(|v| v.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default()
    };

    let fac = get_json(&client, "https://api.quantms.org/dataset-search/facets").await?;
    let mut organisms = values(&fac, "organism");
    let mut instruments = values(&fac, "instrument");
    organisms.sort();
    instruments.sort();

    #[derive(serde::Deserialize)]
    struct CollectionsRaw {
        #[serde(default)]
        collections: Vec<CollectionOption>,
    }
    let colls: CollectionsRaw = serde_json::from_value(
        get_json(&client, "https://api.quantms.org/dataset-search/collections").await?,
    )
    .map_err(|e| format!("collections: {e}"))?;

    Ok(PortalFacets {
        organisms,
        instruments,
        collections: colls.collections,
    })
}

/// Resolve the `qpx/` folder URL (or the run folder directly) for a dataset,
/// then list its parquet views as `ViewInfo` with `path` = the public HTTPS
/// URL and `size_bytes` from the directory listing.
///
/// `dataset` may be a bare accession (e.g. `PXD014414` — resolved by scanning
/// the run sub-folders) or a full run ref returned by the registry API
/// (e.g. `PXD014414/9688c6886714`), which is used verbatim.
#[tauri::command]
pub async fn portal_list_views(
    dataset: String,
    organism: Option<String>,
    psm_count: Option<u64>,
) -> Result<Vec<ViewInfo>, String> {
    let (accession, wanted) = match dataset.split_once('/') {
        Some((acc, run)) => (acc.to_string(), Some(run.to_string())),
        None => (dataset.clone(), None),
    };
    let ds_url = format!("{}{accession}/", view_base());
    let direct = match &wanted {
        Some(run) => {
            let run_url = format!("{ds_url}{run}/");
            get_html(&run_url).await.ok().map(|html| (run_url, html))
        }
        None => None,
    };
    let (run_url, run_html) = match direct {
        Some(found) => found,
        None => {
            // The registry can name a run folder that was since replaced, so
            // pick one of the runs the server holds now.
            let ds_html = get_html(&ds_url)
                .await
                .map_err(|e| format!("dataset {accession} not found on the portal ({e})"))?;
            let run = pick_run(&ds_url, &ds_html, organism.as_deref(), psm_count)
                .await
                .map_err(|e| match &wanted {
                    Some(run) => format!("the registry's run {accession}/{run} is gone from the portal; {e}"),
                    None => e,
                })?;
            let run_url = format!("{ds_url}{run}/");
            let run_html = get_html(&run_url).await?;
            (run_url, run_html)
        }
    };

    // Prefer the `qpx/` sub-folder, else fall back to the run folder directly.
    let qpx_url = if run_html.contains(">qpx/</a>") || run_html.contains(">qpx/<") {
        format!("{run_url}qpx/")
    } else {
        run_url
    };
    let q_html = get_html(&qpx_url).await?;

    let mut views = Vec::new();
    for (name, size) in parquet_entries(&q_html) {
        if let Some((_p, v, fmt)) = crate::qpx::parse_filename(&name) {
            views.push(ViewInfo {
                name: name.clone(),
                view: v,
                format: fmt,
                path: format!("{qpx_url}{name}"),
                size_bytes: size,
            });
        }
    }
    if views.is_empty() {
        return Err(format!("no QPX parquet files in {qpx_url}"));
    }
    views.sort_by(|a, b| a.view.cmp(&b.view));
    Ok(views)
}

/// Run folders (`<run-hash>`) listed on an accession page.
fn run_subdirs(html: &str) -> Vec<String> {
    let mut runs: Vec<String> = extract_links(html)
        .into_iter()
        .filter_map(|(href, _)| href.trim_end_matches('/').rsplit('/').next().map(str::to_string))
        .filter(|last| last.len() >= 8 && last.chars().all(|c| c.is_ascii_hexdigit()))
        .collect();
    runs.dedup();
    runs
}

#[derive(Debug, serde::Deserialize)]
struct RunProvenance {
    #[serde(default)]
    committed_at: i64,
    #[serde(default)]
    files: Vec<serde_json::Value>,
    #[serde(default)]
    stats: serde_json::Value,
}

struct RunInfo {
    run: String,
    committed_at: i64,
    organism: String,
    prefix: String,
    psms: Option<u64>,
}

fn run_info(run: String, p: RunProvenance) -> RunInfo {
    let prefix = p
        .files
        .iter()
        .filter_map(|f| f.get("path").and_then(|v| v.as_str()))
        .filter_map(crate::qpx::parse_filename)
        .map(|(prefix, _, _)| prefix)
        .next()
        .unwrap_or_default();
    let organism = p.stats.get("organism").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let psms = p.stats.get("psms").and_then(|v| v.as_u64());
    RunInfo { run, committed_at: p.committed_at, organism, prefix, psms }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase()
}

/// Choose the run of an accession to open. Runs without QPX parquet files are
/// skipped. One accession can hold separate datasets (one per organism in
/// PXD014877, one per enzyme in PXD009737), so the registry's PSM count, then
/// its organism, narrow the runs to one prefix; runs sharing a prefix are
/// versions of it, and the newest wins.
fn choose_run(runs: Vec<RunInfo>, organism: Option<&str>, psm_count: Option<u64>) -> Result<String, String> {
    let mut runs: Vec<RunInfo> = runs.into_iter().filter(|r| !r.prefix.is_empty()).collect();
    if runs.is_empty() {
        return Err("it has no run with QPX parquet files".to_string());
    }
    runs.sort_by(|a, b| b.committed_at.cmp(&a.committed_at));
    let narrow = |runs: &mut Vec<RunInfo>, keep: &dyn Fn(&RunInfo) -> bool| {
        if runs.iter().any(keep) {
            runs.retain(|r| keep(r));
        }
    };
    if let Some(n) = psm_count.filter(|n| *n > 0) {
        narrow(&mut runs, &|r| r.psms == Some(n));
    }
    if let Some(want) = organism.map(norm).filter(|w| !w.is_empty()) {
        narrow(&mut runs, &|r| norm(&r.organism) == want);
    }
    let prefix = &runs[0].prefix;
    if runs.iter().all(|r| &r.prefix == prefix) {
        return Ok(runs[0].run.clone());
    }
    let mut names: Vec<&str> = runs.iter().map(|r| r.prefix.as_str()).collect();
    names.sort();
    names.dedup();
    Err(format!("it holds separate runs ({}) and none matches this entry", names.join(", ")))
}

async fn pick_run(
    ds_url: &str,
    ds_html: &str,
    organism: Option<&str>,
    psm_count: Option<u64>,
) -> Result<String, String> {
    let runs = run_subdirs(ds_html);
    if runs.is_empty() {
        return Err("it has no run folders".to_string());
    }
    if runs.len() == 1 {
        return Ok(runs[0].clone());
    }
    let client = portal_http::client_builder()?.build().map_err(|e| format!("portal HTTP client: {e:?}"))?;
    let fetches = runs.into_iter().map(|run| {
        let client = client.clone();
        let url = format!("{ds_url}{run}/provenance.json");
        async move {
            let p = client.get(&url).send().await.ok()?.error_for_status().ok()?;
            let p: RunProvenance = p.json().await.ok()?;
            Some(run_info(run, p))
        }
    });
    let infos: Vec<RunInfo> = futures::future::join_all(fetches).await.into_iter().flatten().collect();
    choose_run(infos, organism, psm_count)
}

/// Download the given public parquet URLs into the local cache and return
/// their `ViewInfo` with `path` pointing at the local file. Reports live
/// progress over the `download-progress` event for each file.
#[tauri::command]
pub async fn portal_cache_views(
    app: AppHandle,
    cancel_id: String,
    urls: Vec<String>,
) -> Result<Vec<ViewInfo>, String> {
    portal_cache_views_impl(Some(&app), cancel_id, urls).await
}

/// Testable core of [`portal_cache_views`]; `app` may be `None` in tests.
async fn portal_cache_views_impl(
    app: Option<&AppHandle>,
    cancel_id: String,
    urls: Vec<String>,
) -> Result<Vec<ViewInfo>, String> {
    let client = portal_http::client_builder()?.build().map_err(|e| format!("portal HTTP client: {e:?}"))?;
    let root = cache_root()?;
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;

    // Register the cancel token only after the setup that can fail, so a
    // failed setup never leaks a registry entry (it is cleaned up by `end`
    // below once registered).
    let token = xfers::begin(&cancel_id);

    let result: Result<Vec<ViewInfo>, String> = (async {
        // Resolve names and cache paths, then size everything still to fetch
        // so a download that cannot fit is refused before it starts.
        let mut plan = Vec::new();
        for url in urls {
            let name = url
                .rsplit('/')
                .next()
                .ok_or_else(|| format!("bad url {url}"))?
                .to_string();
            if !safe_component(&name) {
                return Err(format!("unsafe file name in {url}"));
            }
            let dir = run_cache_dir(&root, &url);
            let local = dir.join(&name);
            plan.push((url, name, dir, local));
        }
        let mut sizes = Vec::with_capacity(plan.len());
        let mut need = 0u64;
        for (url, name, _, local) in &plan {
            if xfers::is_cancelled(&token) {
                return Err("cancelled".to_string());
            }
            let size = if local.exists() {
                0
            } else {
                remote_size(&client, url, &token).await.map_err(|e| format!("size of {name}: {e}"))?
            };
            need = need.saturating_add(size);
            sizes.push(size);
        }
        xfers::ensure_space(&root, need)?;

        let mut out = Vec::new();
        for ((url, name, dir, local), head_size) in plan.into_iter().zip(sizes) {
            if xfers::is_cancelled(&token) {
                return Err("cancelled".to_string());
            }
            let (_, view, format) = crate::qpx::parse_filename(&name).unwrap_or_else(|| {
                (String::new(), String::new(), "parquet".to_string())
            });
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let size = if local.exists() {
                std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0)
            } else {
                let resp = xfers::cancellable(&token, async {
                    let resp = client
                        .get(&url)
                        .send()
                        .await
                        .map_err(|e| format!("download {name}: {e}"))?;
                    if !resp.status().is_success() {
                        return Err(format!("download {name}: HTTP {}", resp.status()));
                    }
                    Ok(resp)
                })
                .await?;
                let total = resp.content_length().unwrap_or(head_size);
                if total > head_size {
                    xfers::ensure_space(&dir, total)?;
                }

                // Stream chunk-by-chunk to `<name>.part`: constant memory,
                // progress, and cancellation (also while the server stalls).
                let mut part = xfers::PartFile::create(&local).await?;
                let mut stream = resp.bytes_stream();
                use futures::StreamExt;
                loop {
                    let next = match tokio::time::timeout(xfers::POLL, stream.next()).await {
                        Ok(n) => n,
                        Err(_) if xfers::is_cancelled(&token) => return Err("cancelled".to_string()),
                        Err(_) => continue,
                    };
                    if xfers::is_cancelled(&token) {
                        return Err("cancelled".to_string());
                    }
                    let Some(chunk) = next else { break };
                    let chunk = chunk.map_err(|e| format!("read {name}: {e}"))?;
                    part.write(&chunk).await?;
                    if let Some(app) = app {
                        progress::chunk(app, &name, part.written, total);
                    }
                }
                if total > 0 && part.written != total {
                    return Err(format!("download {name}: got {} of {} bytes", part.written, total));
                }
                let done = part.finish().await?;
                if let Some(app) = app {
                    progress::done(app, &name, done);
                }
                done
            };
            out.push(ViewInfo {
                name,
                view,
                format,
                path: local.to_string_lossy().to_string(),
                size_bytes: size,
            });
        }
        Ok(out)
    })
    .await;

    // Clean up the token whether we succeeded, failed, or were cancelled.
    xfers::end(&cancel_id);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_parser_strips_tags_and_nbsp() {
        assert_eq!(parse_size(" &nbsp;<small>5,794,378 B</small>"), 5_794_378);
        assert_eq!(parse_size("<small>4.5 MB</small>"), 4_718_592);
        assert_eq!(parse_size("<small>1.2 GB</small>"), 1288490188);
        assert_eq!(parse_size(" 1,032 B "), 1032);
    }

    #[tokio::test]
    async fn portal_lists_and_downloads_pxd014414() {
        let ds = "PXD014414".to_string();
        let views = portal_list_views(ds.clone(), None, None).await.expect("list views");
        assert!(!views.is_empty(), "expected views for {ds}");
        assert!(views.iter().any(|v| v.size_bytes > 0), "sizes should be > 0");

        let url = views
            .iter()
            .find(|v| v.view == "dataset")
            .map(|v| v.path.clone())
            .expect("dataset view present");
        let cached = portal_cache_views_impl(None, "test-cancel".to_string(), vec![url])
            .await
            .expect("cache");
        assert_eq!(cached.len(), 1);
        assert!(cached[0].size_bytes > 0);
        assert!(std::path::Path::new(&cached[0].path).exists());
    }

    /// Tiny HTTP server: HEAD/GET advertise `len` bytes; GET streams 64 KB
    /// chunks slowly so a test can cancel mid-file.
    async fn slow_server(len: u64) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let head = std::str::from_utf8(&buf[..n]).unwrap_or("").starts_with("HEAD");
                    let hdr = format!("HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n");
                    if sock.write_all(hdr.as_bytes()).await.is_err() || head {
                        return;
                    }
                    let chunk = vec![0u8; 64 * 1024];
                    loop {
                        if sock.write_all(&chunk).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                });
            }
        });
        format!("http://{addr}")
    }

    /// HTTP server like the portal's: HEAD is 405, GET honours `Range`.
    async fn no_head_server(body: Vec<u8>) -> String {        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = std::sync::Arc::new(body);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                    let resp = if req.starts_with("head") {
                        b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                    } else if req.contains("range: bytes=0-0") {
                        let mut r = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/{}\r\nContent-Length: 1\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        r.push(body[0]);
                        r
                    } else {
                        let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                        r.extend_from_slice(&body);
                        r
                    };
                    let _ = sock.write_all(&resp).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn downloads_when_server_rejects_head() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let base = no_head_server(body.clone()).await;
        let client = Client::new();
        assert_eq!(remote_size(&client, &format!("{base}/x"), &AtomicBool::new(false)).await.unwrap(), body.len() as u64);

        let name = format!("nohead-test-{}.feature.parquet", std::process::id());
        let url = format!("{base}/{name}");
        let out = portal_cache_views_impl(None, "nohead".into(), vec![url]).await.expect("download");
        let got = std::fs::read(&out[0].path).unwrap();
        std::fs::remove_file(&out[0].path).ok();
        assert_eq!(got, body);
    }

    /// HTTP server where HEAD answers 200 with a Content-Length; the size
    /// lookup must use that header (not reqwest's body length, which is 0).
    async fn head_server(body: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = std::sync::Arc::new(body);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                    let resp = if req.starts_with("head") {
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes()
                    } else {
                        let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                        r.extend_from_slice(&body);
                        r
                    };
                    let _ = sock.write_all(&resp).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn sizes_by_head_content_length_when_head_supported() {
        let body: Vec<u8> = (0..10_000u32).map(|i| (i % 253) as u8).collect();
        let base = head_server(body.clone()).await;
        let client = Client::new();
        assert_eq!(remote_size(&client, &format!("{base}/x"), &AtomicBool::new(false)).await.unwrap(), body.len() as u64);
    }

    #[tokio::test]
    async fn new_download_starts_after_a_cancel() {
        let slow = slow_server(1 << 30).await;
        let big = format!("{slow}/recancel-{}.feature.parquet", std::process::id());
        let job = tokio::spawn(portal_cache_views_impl(None, "first".into(), vec![big]));
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        assert!(xfers::cancel("first"));
        assert_eq!(job.await.unwrap().unwrap_err(), "cancelled");

        let body: Vec<u8> = (0..100_000u32).map(|i| (i % 7) as u8).collect();
        let base = no_head_server(body.clone()).await;
        let url = format!("{base}/after-cancel-{}.pg.parquet", std::process::id());
        let out = portal_cache_views_impl(None, "second".into(), vec![url]).await.expect("second download");
        let got = std::fs::read(&out[0].path).unwrap();
        std::fs::remove_file(&out[0].path).ok();
        assert_eq!(got, body);
    }

    #[tokio::test]
    async fn cancel_mid_file_leaves_no_partial_file() {
        let base = slow_server(1 << 30).await;
        let name = format!("cancel-test-{}.feature.parquet", std::process::id());
        let local = cache_root().unwrap().join(&name);
        let id = format!("test-cancel-{}", std::process::id());
        let task = tokio::spawn(portal_cache_views_impl(None, id.clone(), vec![format!("{base}/{name}")]));
        let part = local.with_file_name(format!("{name}.part"));
        for _ in 0..100 {
            if part.exists() && std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(part.exists(), "download should have started");
        assert!(xfers::cancel(&id));
        let r = tokio::time::timeout(std::time::Duration::from_secs(5), task).await.expect("cancel is prompt").unwrap();
        assert_eq!(r.unwrap_err(), "cancelled");
        assert!(!part.exists() && !local.exists(), "partial download must be removed");
    }

    #[tokio::test]
    async fn refuses_download_that_does_not_fit() {
        let base = slow_server(1 << 50).await; // 1 PB
        let name = format!("huge-test-{}.feature.parquet", std::process::id());
        let r = portal_cache_views_impl(None, "test-huge".into(), vec![format!("{base}/{name}")]).await;
        let e = r.unwrap_err();
        assert!(e.starts_with("Not enough disk space"), "{e}");
        let local = cache_root().unwrap().join(&name);
        assert!(!local.exists() && !local.with_file_name(format!("{name}.part")).exists());
    }

    fn info(run: &str, at: i64, organism: &str, prefix: &str, psms: u64) -> RunInfo {
        RunInfo { run: run.into(), committed_at: at, organism: organism.into(), prefix: prefix.into(), psms: Some(psms) }
    }

    #[test]
    fn choose_run_takes_newest_version_of_the_matching_entry() {
        let versions = vec![
            info("old", 1, "homo sapiens", "IPX1", 5),
            info("new", 2, "homo sapiens", "IPX1", 5),
            info("expr", 3, "homo sapiens", "", 0),
        ];
        assert_eq!(choose_run(versions, None, Some(5)).unwrap(), "new");
        let enzymes = || {
            vec![
                info("trypsin", 3, "Homo sapiens", "PXD009737-Trypsin", 573439),
                info("lysc-old", 1, "Homo sapiens", "PXD009737-LysC", 428263),
                info("lysc", 2, "Homo sapiens", "PXD009737-LysC", 428263),
            ]
        };
        assert_eq!(choose_run(enzymes(), Some("homo sapiens"), Some(428263)).unwrap(), "lysc");
        assert!(choose_run(enzymes(), Some("homo sapiens"), None).is_err());
        let organisms = || {
            vec![
                info("vp", 1, "Veillonella parvula", "PXD014877-Veillonella-parvula", 10),
                info("dr", 1, "Danio rerio", "PXD014877-Danio-rerio", 20),
            ]
        };
        assert_eq!(choose_run(organisms(), Some("danio rerio"), Some(99)).unwrap(), "dr");
        assert!(choose_run(organisms(), Some("gallus gallus"), None).is_err());
    }

    #[tokio::test]
    async fn stale_registry_ref_falls_back_to_a_live_run() {
        let views = portal_list_views("IPX0001804002/0e8d80a52200".into(), None, None)
            .await
            .expect("fallback run");
        assert!(views.iter().any(|v| v.view == "psm"));
        assert!(!views[0].path.contains("0e8d80a52200"));
    }

    #[tokio::test]
    async fn registry_lists_and_ref_resolves() {
        let rows = portal_registry(Some(100)).await.expect("registry");
        assert!(rows.len() >= 80, "expected ~83 unique datasets, got {}", rows.len());
        let with_ref = rows
            .iter()
            .find(|r| r.dataset_ref.is_some())
            .expect("some dataset has a run ref");
        let views = portal_list_views(with_ref.dataset_ref.clone().unwrap(), None, None)
            .await
            .expect("list views from ref");
        assert!(!views.is_empty());
        assert!(views.iter().any(|v| v.size_bytes > 0));
    }
}
