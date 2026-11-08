//! Lazy remote access for DuckDB-wasm (S3 buckets and the quantms portal).
//!
//! A tiny HTTP server bound to 127.0.0.1 serves registered remote files with
//! HTTP Range support. DuckDB (running in the webview) treats each object as a
//! remote parquet file and only requests the byte ranges a query needs (footer,
//! then the column chunks / row groups it touches). Each range is fetched from
//! S3 with the backend's AWS credentials (or over HTTPS for public portal
//! files) and kept in an in-memory block cache,
//! so:
//!   * nothing is downloaded up front, however large the object is;
//!   * the bucket needs no CORS configuration and credentials never reach the
//!     webview;
//!   * re-reading the same pages (paging, re-sorting) is served from memory.
//!
//! URLs carry a random per-session token so other local processes can't use
//! the proxy to read the user's buckets.

use crate::commands::portal_http;
use crate::commands::s3::client_for_bucket;
use aws_sdk_s3::Client;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{header, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lru::LruCache;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::OnceCell;

/// Cache granularity. Requests are widened to whole blocks so neighbouring
/// small reads (parquet footers, page headers) share one S3 round-trip.
const BLOCK: u64 = 256 * 1024;
/// In-memory cache budget (in blocks) shared by all streamed objects.
const CACHE_BLOCKS: usize = 2048;
/// Longest single range request (in blocks): 16 x 256 KB = 4 MB.
const CHUNK_BLOCKS: u64 = 16;
/// Range requests issued concurrently for one large read.
const PARALLEL_FETCHES: usize = 8;

/// Where a streamed object's bytes come from.
enum Source {
    /// An S3 object read with the user's AWS credentials.
    S3 { client: Client, bucket: String, key: String },
    /// A public HTTPS URL (e.g. the quantms portal). The portal answers with a
    /// redirect to a short-lived signed URL on S3-compatible storage; we keep
    /// the resolved URL and re-resolve when it expires.
    Http { client: reqwest::Client, url: String, resolved: Mutex<Option<String>> },
    /// A local parquet file (or one already downloaded to the cache). Served
    /// straight from disk, so DuckDB reads only the ranges it needs instead of
    /// the whole file being copied into the webview.
    Local { path: std::path::PathBuf },
}

struct Object {
    /// The app-facing path (`s3://bucket/key` or the portal https URL).
    path: String,
    size: u64,
    last_modified: Option<String>,
    source: Source,
    fetched: AtomicU64,
    requests: AtomicU64,
}

struct Proxy {
    port: u16,
    token: String,
    objects: Mutex<HashMap<String, Arc<Object>>>,
    cache: Mutex<LruCache<(String, u64), Bytes>>,
}

static PROXY: OnceCell<Arc<Proxy>> = OnceCell::const_new();

fn random_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

async fn proxy() -> Result<Arc<Proxy>, String> {
    PROXY
        .get_or_try_init(|| async {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|e| format!("stream proxy bind: {e}"))?;
            let port = listener.local_addr().map_err(|e| e.to_string())?.port();
            let p = Arc::new(Proxy {
                port,
                token: random_token(),
                objects: Mutex::new(HashMap::new()),
                cache: Mutex::new(LruCache::new(NonZeroUsize::new(CACHE_BLOCKS).unwrap())),
            });
            let served = p.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else { continue };
                    let p = served.clone();
                    tokio::spawn(async move {
                        let svc = service_fn(move |req| handle(p.clone(), req));
                        let _ = http1::Builder::new()
                            .keep_alive(true)
                            .serve_connection(TokioIo::new(stream), svc)
                            .await;
                    });
                }
            });
            log::info!("S3 stream proxy listening on 127.0.0.1:{port}");
            Ok(p)
        })
        .await
        .cloned()
}

fn cors(b: hyper::http::response::Builder) -> hyper::http::response::Builder {
    b.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
        .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "Range, Content-Type")
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "Content-Length, Content-Range, Accept-Ranges, Last-Modified",
        )
        .header("Cross-Origin-Resource-Policy", "cross-origin")
        .header(header::CACHE_CONTROL, "no-store")
}

fn plain(status: StatusCode, msg: impl Into<String>) -> Response<Full<Bytes>> {
    cors(Response::builder().status(status))
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Full::new(Bytes::from(msg.into())))
        .unwrap()
}

/// Parse a single `bytes=` range into an inclusive (start, end).
fn parse_range(h: &str, size: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    if size == 0 {
        return None;
    }
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (size.saturating_sub(n), size - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() { size - 1 } else { b.parse::<u64>().ok()?.min(size - 1) };
        (s, e)
    };
    (start <= end && start < size).then_some((start, end))
}

async fn handle(p: Arc<Proxy>, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    if req.method() == Method::OPTIONS {
        return Ok(cors(Response::builder().status(StatusCode::NO_CONTENT))
            .body(Full::new(Bytes::new()))
            .unwrap());
    }
    let mut parts = req.uri().path().trim_start_matches('/').splitn(2, '/');
    let (token, id) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if token != p.token {
        return Ok(plain(StatusCode::FORBIDDEN, "bad token"));
    }
    let id = id.split('/').next().unwrap_or("").to_string();
    let Some(obj) = p.objects.lock().unwrap().get(&id).cloned() else {
        return Ok(plain(StatusCode::NOT_FOUND, "unknown object"));
    };

    let range = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|r| parse_range(r, obj.size));
    if let Some(None) = range {
        return Ok(cors(Response::builder().status(StatusCode::RANGE_NOT_SATISFIABLE))
            .header(header::CONTENT_RANGE, format!("bytes */{}", obj.size))
            .body(Full::new(Bytes::new()))
            .unwrap());
    }
    let range = range.flatten();
    log::debug!("{} {} range={range:?} size={}", req.method(), obj.path, obj.size);
    let (start, end) = range.unwrap_or((0, obj.size.saturating_sub(1)));
    let len = if obj.size == 0 { 0 } else { end - start + 1 };

    let mut b = cors(Response::builder())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, len.to_string());
    if let Some(lm) = &obj.last_modified {
        b = b.header(header::LAST_MODIFIED, lm.as_str());
    }
    b = if range.is_some() {
        b.status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{}", obj.size))
    } else {
        b.status(StatusCode::OK)
    };

    if req.method() == Method::HEAD {
        return Ok(b.body(Full::new(Bytes::new())).unwrap());
    }
    if req.method() != Method::GET {
        return Ok(plain(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"));
    }
    if len == 0 {
        return Ok(b.body(Full::new(Bytes::new())).unwrap());
    }
    match read_range(&p, &id, &obj, start, end).await {
        Ok(data) => Ok(b.body(Full::new(data)).unwrap()),
        Err(e) => {
            log::warn!("stream {}: {e}", obj.path);
            Ok(plain(StatusCode::BAD_GATEWAY, e))
        }
    }
}

/// Follow the portal's redirect once to get the signed storage URL.
async fn resolve(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let resp = client
        .get(url)
        .header(header::RANGE.as_str(), "bytes=0-0")
        .send()
        .await
        .map_err(|e| format!("resolve {url}: {e}"))?;
    if resp.status().is_redirection() {
        if let Some(loc) = resp.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()) {
            // Location may be relative to the requested URL.
            return reqwest::Url::parse(url)
                .and_then(|base| base.join(loc))
                .map(|u| u.to_string())
                .map_err(|e| format!("resolve {url}: bad Location {loc}: {e}"));
        }
    }
    if resp.status().is_success() {
        return Ok(url.to_string());
    }
    Err(format!("resolve {url}: HTTP {}", resp.status()))
}

/// Transient HTTP statuses worth retrying a range fetch for. 5xx are server
/// hiccups, 429/408/425 are throttling / timeout — all likely to clear on retry.
fn is_transient_status(st: u16) -> bool {
    matches!(st, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

/// Short backoff between retries: ~250 ms then ~500 ms. Long enough for the
/// upstream to clear a transient 502/503, short enough not to stall the query.
fn backoff_delay(attempt: usize) -> std::time::Duration {
    std::time::Duration::from_millis(match attempt {
        0 => 250,
        _ => 500,
    })
}

/// One ranged GET against an HTTP source, with bounded retries.
///
/// Two failure modes are handled separately:
///   * the signed URL was rejected/expired (400/401/403) -> re-resolve it and
///     try again;
///   * a transient failure (network/send error, or 408/425/429/5xx such as a
///     remote gateway 502) -> retry with a short backoff, up to a couple of
///     attempts, before giving up.
#[allow(clippy::too_many_arguments)]
async fn http_range(
    client: &reqwest::Client,
    url: &str,
    resolved: &Mutex<Option<String>>,
    range: &str,
) -> Result<reqwest::Response, String> {
    const MAX_ATTEMPTS: usize = 3; // initial + 2 retries
    let mut last_err: Option<String> = None;
    for attempt in 0..MAX_ATTEMPTS {
        // Use the cached signed URL on the first try, otherwise resolve fresh.
        let cached = resolved.lock().unwrap().clone();
        let target = match cached {
            Some(u) if attempt == 0 => u,
            _ => {
                let u = resolve(client, url).await?;
                *resolved.lock().unwrap() = Some(u.clone());
                u
            }
        };

        let send = client
            .get(&target)
            .header(header::RANGE.as_str(), range)
            .send()
            .await;

        let resp = match send {
            Ok(r) => r,
            Err(e) => {
                // Transport-level failure (timeout / connection reset): transient.
                if attempt + 1 < MAX_ATTEMPTS {
                    last_err = Some(format!("range get {url}: {e}"));
                    tokio::time::sleep(backoff_delay(attempt)).await;
                    continue;
                }
                return Err(format!("range get {url}: {e} after {MAX_ATTEMPTS} attempts"));
            }
        };

        let st = resp.status().as_u16();
        // Expired / rejected signed URL: drop the cached URL and resolve fresh.
        if (st == 403 || st == 400 || st == 401) && attempt == 0 {
            *resolved.lock().unwrap() = None;
            continue;
        }
        if resp.status().is_success() {
            return Ok(resp);
        }
        // Not successful: retry transient statuses, but fail fast on permanent
        // client errors (e.g. 404) that retrying will never fix.
        if is_transient_status(st) && attempt + 1 < MAX_ATTEMPTS {
            last_err = Some(format!("range get {url}: HTTP {st}"));
            tokio::time::sleep(backoff_delay(attempt)).await;
            continue;
        }
        return Err(format!("range get {url}: HTTP {st}"));
    }
    Err(last_err.unwrap_or_else(|| format!("range get {url}: signed URL rejected")))
}

/// Fetch one inclusive byte range from the object's source.
async fn fetch(obj: &Object, start: u64, end: u64) -> Result<Bytes, String> {
    let range = format!("bytes={start}-{end}");
    let data = match &obj.source {
        Source::S3 { client, bucket, key } => {
            let out = client
                .get_object()
                .bucket(bucket)
                .key(key)
                .range(range)
                .send()
                .await
                .map_err(|e| format!("S3 range get {key}: {}", aws_sdk_s3::error::DisplayErrorContext(e)))?;
            out.body.collect().await.map_err(|e| e.to_string())?.into_bytes()
        }
        Source::Local { path } => {
            let path = path.clone();
            tokio::task::spawn_blocking(move || read_local(&path, start, end))
                .await
                .map_err(|e| e.to_string())??
        }
        Source::Http { client, url, resolved } => {
            let resp = http_range(client, url, resolved, &range).await?;
            let full = resp.status().as_u16() == 200;
            let body = resp.bytes().await.map_err(|e| format!("read {url}: {e}"))?;
            if full {
                // Server ignored Range; keep only the requested slice.
                let s = (start as usize).min(body.len());
                let e = ((end + 1) as usize).min(body.len());
                body.slice(s..e)
            } else {
                body
            }
        }
    };
    obj.fetched.fetch_add(data.len() as u64, Ordering::Relaxed);
    obj.requests.fetch_add(1, Ordering::Relaxed);
    Ok(data)
}

fn read_local(path: &std::path::Path, start: u64, end: u64) -> Result<Bytes, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; (end - start + 1) as usize];
    f.read_exact(&mut buf).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(Bytes::from(buf))
}

/// Serve [start, end] from the block cache, fetching missing blocks from S3 in
/// as few contiguous range requests as possible.
async fn read_range(p: &Proxy, id: &str, obj: &Object, start: u64, end: u64) -> Result<Bytes, String> {
    // Local files are already one seek away (and in the OS page cache); keep
    // the block cache for remote objects.
    if matches!(obj.source, Source::Local { .. }) {
        return fetch(obj, start, end).await;
    }
    let (first, last) = (start / BLOCK, end / BLOCK);
    let mut blocks: BTreeMap<u64, Bytes> = BTreeMap::new();
    {
        let mut cache = p.cache.lock().unwrap();
        for b in first..=last {
            if let Some(d) = cache.get(&(id.to_string(), b)) {
                blocks.insert(b, d.clone());
            }
        }
    }

    // Group missing blocks into contiguous runs.
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for b in first..=last {
        if blocks.contains_key(&b) {
            continue;
        }
        match runs.last_mut() {
            Some((s, e)) if *e + 1 == b && *e + 1 - *s < CHUNK_BLOCKS => *e = b,
            _ => runs.push((b, b)),
        }
    }
    // Large reads (e.g. `SELECT *` touching many column chunks) become several
    // chunk-sized requests in flight at once instead of one long serial GET.
    use futures::stream::{self, StreamExt, TryStreamExt};
    let fetched: Vec<(u64, u64, Bytes)> = stream::iter(runs)
        .map(|(rb, re)| async move {
            let s = rb * BLOCK;
            let e = ((re + 1) * BLOCK).min(obj.size) - 1;
            fetch(obj, s, e).await.map(|d| (rb, re, d))
        })
        .buffered(PARALLEL_FETCHES)
        .try_collect()
        .await?;
    for (rb, re, data) in fetched {
        let mut cache = p.cache.lock().unwrap();
        for b in rb..=re {
            let off = ((b - rb) * BLOCK) as usize;
            if off >= data.len() {
                break;
            }
            let chunk = data.slice(off..(off + BLOCK as usize).min(data.len()));
            cache.put((id.to_string(), b), chunk.clone());
            blocks.insert(b, chunk);
        }
    }

    let mut out = Vec::with_capacity((end - start + 1) as usize);
    for b in first..=last {
        let d = blocks
            .get(&b)
            .ok_or_else(|| format!("short read for {} (block {b})", obj.path))?;
        let base = b * BLOCK;
        let lo = start.saturating_sub(base) as usize;
        let hi = ((end + 1 - base) as usize).min(d.len());
        if lo < hi {
            out.extend_from_slice(&d[lo..hi]);
        }
    }
    Ok(Bytes::from(out))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamHandle {
    pub url: String,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamStat {
    pub size_bytes: u64,
    pub fetched_bytes: u64,
    pub requests: u64,
}

fn object_id(path: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn handle_for(p: &Proxy, id: &str, path: &str, size: u64) -> StreamHandle {
    let file = path.rsplit('/').next().unwrap_or("file.parquet");
    StreamHandle {
        url: format!("http://127.0.0.1:{}/{}/{id}/{file}", p.port, p.token),
        size_bytes: size,
    }
}

/// Register `s3://bucket/key` views with the local stream proxy and return a
/// 127.0.0.1 URL per view that DuckDB can range-read. Only a HEAD request is
/// made here; the data itself is fetched lazily as queries need it.
pub async fn register(bucket: &str, paths: &[String]) -> Result<BTreeMap<String, StreamHandle>, String> {
    let p = proxy().await?;
    let client = client_for_bucket(bucket).await?;
    let mut out = BTreeMap::new();
    for path in paths {
        let key = path.trim_start_matches(&format!("s3://{bucket}/")).to_string();
        let id = object_id(path);
        let existing = p.objects.lock().unwrap().get(&id).cloned();
        let obj = match existing {
            Some(o) => o,
            None => {
                let head = client
                    .head_object()
                    .bucket(bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|e| format!("S3 head {key}: {}", aws_sdk_s3::error::DisplayErrorContext(e)))?;
                let o = Arc::new(Object {
                    path: path.clone(),
                    size: head.content_length().unwrap_or(0).max(0) as u64,
                    last_modified: head
                        .last_modified()
                        .and_then(|d| d.fmt(aws_sdk_s3::primitives::DateTimeFormat::HttpDate).ok()),
                    source: Source::S3 { client: client.clone(), bucket: bucket.to_string(), key: key.clone() },
                    fetched: AtomicU64::new(0),
                    requests: AtomicU64::new(0),
                });
                p.objects.lock().unwrap().insert(id.clone(), o.clone());
                o
            }
        };
        out.insert(path.clone(), handle_for(&p, &id, path, obj.size));
    }
    Ok(out)
}

/// Register public HTTPS files (quantms portal) with the stream proxy. Only a
/// 1-byte range GET is made per file, to learn its size; data is fetched lazily.
pub async fn register_http(urls: &[String]) -> Result<BTreeMap<String, StreamHandle>, String> {
    let p = proxy().await?;
    let client = portal_http::client_builder()?
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("portal HTTP client: {e:?}"))?;
    let mut out = BTreeMap::new();
    for url in urls {
        // Only the quantms portal: the proxy must not fetch arbitrary hosts.
        if !url.starts_with("https://browse.quantms.org/") {
            return Err(format!("{url}: only https://browse.quantms.org files can be streamed"));
        }
        let id = object_id(url);
        let existing = p.objects.lock().unwrap().get(&id).cloned();
        let obj = match existing {
            Some(o) => o,
            None => {
                let resolved = Mutex::new(None);
                let resp = http_range(&client, url, &resolved, "bytes=0-0").await?;
                let h = resp.headers();
                let total = h
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.rsplit('/').next())
                    .and_then(|v| v.parse::<u64>().ok())
                    .or_else(|| (resp.status().as_u16() == 200).then(|| resp.content_length()).flatten())
                    .ok_or_else(|| format!("{url}: server does not support range requests"))?;
                let last_modified = h
                    .get(reqwest::header::LAST_MODIFIED)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let o = Arc::new(Object {
                    path: url.clone(),
                    size: total,
                    last_modified,
                    source: Source::Http { client: client.clone(), url: url.clone(), resolved },
                    fetched: AtomicU64::new(0),
                    requests: AtomicU64::new(0),
                });
                p.objects.lock().unwrap().insert(id.clone(), o.clone());
                o
            }
        };
        out.insert(url.clone(), handle_for(&p, &id, url, obj.size));
    }
    Ok(out)
}

/// Register local parquet files with the proxy so DuckDB can range-read them
/// lazily. Only regular `.parquet` files are accepted. A file that changed on
/// disk since it was registered (e.g. re-downloaded) gets a fresh entry.
pub async fn register_local(paths: &[String]) -> Result<BTreeMap<String, StreamHandle>, String> {
    let p = proxy().await?;
    let mut out = BTreeMap::new();
    for path in paths {
        let pb = std::path::PathBuf::from(path);
        let is_parquet = pb
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("parquet"));
        if !is_parquet {
            return Err(format!("{path}: only .parquet files can be opened lazily"));
        }
        let meta = std::fs::metadata(&pb).map_err(|e| format!("{path}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{path}: not a file"));
        }
        let last_modified = meta.modified().ok().map(httpdate::fmt_http_date);
        let id = object_id(&format!("file://{path}"));
        let fresh = p
            .objects
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|o| o.size == meta.len() && o.last_modified == last_modified);
        if !fresh {
            let o = Arc::new(Object {
                path: path.clone(),
                size: meta.len(),
                last_modified,
                source: Source::Local { path: pb.clone() },
                fetched: AtomicU64::new(0),
                requests: AtomicU64::new(0),
            });
            p.objects.lock().unwrap().insert(id.clone(), o);
        }
        let size = meta.len();
        // The URL's last segment is cosmetic; keep it URL-safe.
        let name: String = pb
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file.parquet")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' })
            .collect();
        out.insert(path.clone(), handle_for(&p, &id, &name, size));
    }
    Ok(out)
}

/// Bytes actually pulled per streamed view (keyed by its `s3://` / https path).
pub fn stats() -> BTreeMap<String, StreamStat> {
    let Some(p) = PROXY.get() else { return BTreeMap::new() };
    let objects = p.objects.lock().unwrap();
    objects
        .values()
        .filter(|o| !matches!(o.source, Source::Local { .. }))
        .map(|o| {
            (
                o.path.clone(),
                StreamStat {
                    size_bytes: o.size,
                    fetched_bytes: o.fetched.load(Ordering::Relaxed),
                    requests: o.requests.load(Ordering::Relaxed),
                },
            )
        })
        .collect()
}

#[tauri::command]
pub async fn stream_s3_urls(
    bucket: String,
    paths: Vec<String>,
) -> Result<BTreeMap<String, StreamHandle>, String> {
    register(&bucket, &paths).await
}

#[tauri::command]
pub async fn stream_http_urls(urls: Vec<String>) -> Result<BTreeMap<String, StreamHandle>, String> {
    register_http(&urls).await
}

#[tauri::command]
pub async fn stream_local_files(paths: Vec<String>) -> Result<BTreeMap<String, StreamHandle>, String> {
    register_local(&paths).await
}

#[tauri::command]
pub fn stream_s3_stats() -> BTreeMap<String, StreamStat> {
    stats()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ranges_and_header_parsing() {
        let dir = std::env::temp_dir().join(format!("qpxui-range-{}", random_token()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.parquet");
        std::fs::write(&f, b"0123456789").unwrap();
        assert_eq!(&read_local(&f, 2, 5).unwrap()[..], b"2345");
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=4-", 10), Some((4, 9)));
        assert_eq!(parse_range("bytes=20-30", 10), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn register_local_rejects_non_parquet() {
        let err = register_local(&["/etc/hosts".to_string()]).await.unwrap_err();
        assert!(err.contains("only .parquet"));
    }
}
