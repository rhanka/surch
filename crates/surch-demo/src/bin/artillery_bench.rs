//! Rust keep-alive replacement for `scripts/bench/artillery-replay.sh`.
//!
//! Spawns a pool of tokio workers that share a hyper-util legacy HTTP
//! client (persistent HTTP/1.1 keep-alive). Each phase issues exactly
//! `rps * duration_s` requests, paced via `tokio::time::sleep_until`.
//! Latency samples are aggregated into per-phase and global percentile
//! summaries, then emitted as a `surch.bench.artillery.v1` JSON report.
//!
//! Query mix mirrors `artillery-replay.sh`: alternating `multi_match`
//! and `bool.must` shapes against the matchID-style INSEE index, with
//! random `(PRENOMS, NOM)` pairs drawn from the `--names` TSV file.
//!
//! A second query mode (`--query-mode trec`) targets a free-text corpus
//! (BEIR TREC-COVID): each line of the `--names` file is one full query
//! string, issued as a `multi_match` over `title`/`text`. This drives
//! the long-posting-list / skip-list regime that the INSEE 10k corpus
//! cannot reach. The default mode (`insee`) is unchanged.

use std::{
    env, fmt, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use chrono::{SecondsFormat, Utc};
use http::{Method, Request, Uri};
use http_body_util::{BodyExt, Full};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use serde_json::{json, Value};
use tokio::{
    sync::mpsc,
    task::JoinSet,
    time::{sleep_until, Instant as TokioInstant},
};

const DEFAULT_URL: &str = "http://127.0.0.1:7700";
const DEFAULT_INDEX: &str = "deces_25k";
const DEFAULT_NAMES: &str = "/home/antoinefa/src/surch/target/insee/artillery_names.txt";
const DEFAULT_WORKERS: usize = 8;
const DEFAULT_PHASES: &str = "2:30,2:30,5:30,10:30,20:30,50:60";
const REPORT_SCHEMA: &str = "surch.bench.artillery.v1";
const DEFAULT_QUERY_MODE: QueryMode = QueryMode::Insee;

/// Query shape produced for each request. `Insee` (default) keeps the
/// historical matchID `(PRENOMS, NOM)` behavior; `Trec` issues a single
/// `multi_match` over `title`/`text` from one free-text query per line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryMode {
    Insee,
    Trec,
}

impl QueryMode {
    fn parse(raw: &str) -> Result<Self, CliError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "insee" => Ok(Self::Insee),
            "trec" => Ok(Self::Trec),
            other => Err(CliError::Usage(format!(
                "--query-mode must be `insee` or `trec`, got `{other}`"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Insee => "insee",
            Self::Trec => "trec",
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), CliError> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    let plan = parse_args(args)?;
    execute_plan(plan).await
}

#[derive(Debug, Clone)]
struct Plan {
    url: String,
    index: String,
    names_path: String,
    workers: usize,
    phases: Vec<Phase>,
    report_path: Option<String>,
    query_mode: QueryMode,
}

#[derive(Debug, Clone, Copy)]
struct Phase {
    rps: u32,
    duration_s: u32,
}

fn parse_args(args: Vec<String>) -> Result<Plan, CliError> {
    let mut url = DEFAULT_URL.to_owned();
    let mut index = DEFAULT_INDEX.to_owned();
    let mut names_path = DEFAULT_NAMES.to_owned();
    let mut workers = DEFAULT_WORKERS;
    let mut phases_raw = DEFAULT_PHASES.to_owned();
    let mut report_path: Option<String> = None;
    let mut query_mode = DEFAULT_QUERY_MODE;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--url" => url = required_value(&mut iter, "--url")?,
            "--index" => index = required_value(&mut iter, "--index")?,
            "--names" => names_path = required_value(&mut iter, "--names")?,
            "--query-mode" => {
                query_mode = QueryMode::parse(&required_value(&mut iter, "--query-mode")?)?;
            }
            "--workers" => {
                let raw = required_value(&mut iter, "--workers")?;
                workers = raw.parse::<usize>().map_err(|_| {
                    CliError::Usage("--workers must be a positive integer".to_owned())
                })?;
                if workers == 0 {
                    return Err(CliError::Usage(
                        "--workers must be greater than zero".to_owned(),
                    ));
                }
            }
            "--phases" => phases_raw = required_value(&mut iter, "--phases")?,
            "--report" => report_path = Some(required_value(&mut iter, "--report")?),
            other => {
                return Err(CliError::Usage(format!("unknown option `{other}`")));
            }
        }
    }

    if !url.starts_with("http://") {
        return Err(CliError::Usage("--url must start with http://".to_owned()));
    }
    if index.is_empty() {
        return Err(CliError::Usage("--index must not be empty".to_owned()));
    }
    if names_path.is_empty() {
        return Err(CliError::Usage("--names must not be empty".to_owned()));
    }

    let phases = parse_phases(&phases_raw)?;

    Ok(Plan {
        url,
        index,
        names_path,
        workers,
        phases,
        report_path,
        query_mode,
    })
}

fn required_value<I: Iterator<Item = String>>(
    iter: &mut I,
    option: &'static str,
) -> Result<String, CliError> {
    iter.next()
        .ok_or_else(|| CliError::Usage(format!("missing value after {option}")))
}

/// Parses a comma-separated `rps:duration_s` list. Rejects empty input,
/// non-integer fields, zero values, and malformed segments.
fn parse_phases(raw: &str) -> Result<Vec<Phase>, CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::Usage("--phases must not be empty".to_owned()));
    }

    let mut phases = Vec::new();
    for segment in trimmed.split(',') {
        let segment = segment.trim();
        if segment.is_empty() {
            return Err(CliError::Usage(format!(
                "--phases segment is empty in `{raw}`"
            )));
        }
        let (rps_raw, duration_raw) = segment.split_once(':').ok_or_else(|| {
            CliError::Usage(format!(
                "--phases segment `{segment}` must be `rps:duration_s`"
            ))
        })?;
        let rps = rps_raw.trim().parse::<u32>().map_err(|_| {
            CliError::Usage(format!("--phases segment `{segment}` has non-integer rps"))
        })?;
        let duration_s = duration_raw.trim().parse::<u32>().map_err(|_| {
            CliError::Usage(format!(
                "--phases segment `{segment}` has non-integer duration_s"
            ))
        })?;
        if rps == 0 {
            return Err(CliError::Usage(format!(
                "--phases segment `{segment}` rps must be > 0"
            )));
        }
        if duration_s == 0 {
            return Err(CliError::Usage(format!(
                "--phases segment `{segment}` duration_s must be > 0"
            )));
        }
        phases.push(Phase { rps, duration_s });
    }

    Ok(phases)
}

/// One unit of work drawn at random per request. In `insee` mode it
/// carries a `(PRENOMS, NOM)` pair; in `trec` mode `nom` is empty and
/// `prenoms` holds the full free-text query string.
#[derive(Debug, Clone)]
struct NameRow {
    prenoms: String,
    nom: String,
}

fn load_names(path: &str, query_mode: QueryMode) -> Result<Vec<NameRow>, CliError> {
    let content = fs::read_to_string(path)
        .map_err(|error| CliError::Io(format!("failed to read --names `{path}`: {error}")))?;
    let mut rows = Vec::new();
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        match query_mode {
            QueryMode::Insee => {
                let mut fields = line.split('\t');
                let prenoms = fields.next().unwrap_or("").trim().to_owned();
                let nom = fields.next().unwrap_or("").trim().to_owned();
                if prenoms.is_empty() || nom.is_empty() {
                    continue;
                }
                rows.push(NameRow { prenoms, nom });
            }
            QueryMode::Trec => {
                // One full free-text query per line; tabs (if any) are
                // folded to spaces so the whole line is one query string.
                let query = line.replace('\t', " ");
                let query = query.trim();
                if query.is_empty() {
                    continue;
                }
                rows.push(NameRow {
                    prenoms: query.to_owned(),
                    nom: String::new(),
                });
            }
        }
    }
    if rows.is_empty() {
        let shape = match query_mode {
            QueryMode::Insee => "(PRENOMS, NOM)",
            QueryMode::Trec => "free-text query",
        };
        return Err(CliError::Io(format!(
            "--names `{path}` produced zero usable {shape} rows"
        )));
    }
    Ok(rows)
}

async fn execute_plan(plan: Plan) -> Result<(), CliError> {
    let names = Arc::new(load_names(&plan.names_path, plan.query_mode)?);
    let base_uri: Uri = format!("{}/{}/_search", plan.url.trim_end_matches('/'), plan.index)
        .parse()
        .map_err(|error| CliError::Usage(format!("invalid composed URL: {error}")))?;

    let mut connector = HttpConnector::new();
    connector.set_nodelay(true);
    connector.set_keepalive(Some(Duration::from_secs(60)));
    let client: Client<HttpConnector, Full<Bytes>> = Client::builder(TokioExecutor::new())
        .pool_idle_timeout(Duration::from_secs(60))
        .pool_max_idle_per_host(plan.workers.max(8))
        .build(connector);
    let client = Arc::new(client);
    let base_uri = Arc::new(base_uri);

    let started_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    println!(
        "artillery_bench: url={} index={} mode={} workers={} phases={} names={}",
        plan.url,
        plan.index,
        plan.query_mode.as_str(),
        plan.workers,
        plan.phases.len(),
        plan.names_path
    );

    let mut phase_reports: Vec<PhaseReport> = Vec::with_capacity(plan.phases.len());
    let mut global_samples_ms: Vec<f64> = Vec::new();
    let mut global_issued: u64 = 0;
    let mut global_errors: u64 = 0;
    let mut global_request_seq: u64 = 0;

    for (phase_index, phase) in plan.phases.iter().enumerate() {
        let report = run_phase(
            phase_index,
            *phase,
            plan.workers,
            plan.query_mode,
            Arc::clone(&client),
            Arc::clone(&base_uri),
            Arc::clone(&names),
            &mut global_request_seq,
        )
        .await?;
        println!(
            "phase {}: rps={} duration_s={} issued={} errors={} p50={:.1}ms p95={:.1}ms p99={:.1}ms max={:.1}ms",
            phase_index + 1,
            phase.rps,
            phase.duration_s,
            report.issued,
            report.errors,
            report.summary.p50_ms,
            report.summary.p95_ms,
            report.summary.p99_ms,
            report.summary.max_ms,
        );
        global_issued += report.issued;
        global_errors += report.errors;
        global_samples_ms.extend_from_slice(&report.samples_ms);
        phase_reports.push(report);
    }

    let global_summary = LatencySummary::from_samples_ms(&global_samples_ms);
    println!(
        "global: issued={} errors={} p50={:.1}ms p95={:.1}ms p99={:.1}ms max={:.1}ms",
        global_issued,
        global_errors,
        global_summary.p50_ms,
        global_summary.p95_ms,
        global_summary.p99_ms,
        global_summary.max_ms,
    );

    if let Some(path) = &plan.report_path {
        let report_json = build_report_json(
            &plan,
            &started_at,
            &phase_reports,
            global_issued,
            global_errors,
            &global_summary,
        );
        write_report(path, &report_json)?;
        println!("report: {path}");
    }

    Ok(())
}

struct PhaseReport {
    rps: u32,
    duration_s: u32,
    issued: u64,
    errors: u64,
    samples_ms: Vec<f64>,
    summary: LatencySummary,
}

#[derive(Debug, Clone)]
struct LatencySummary {
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

impl LatencySummary {
    fn from_samples_ms(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self {
                p50_ms: 0.0,
                p95_ms: 0.0,
                p99_ms: 0.0,
                max_ms: 0.0,
            };
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        Self {
            p50_ms: percentile(&sorted, 50.0),
            p95_ms: percentile(&sorted, 95.0),
            p99_ms: percentile(&sorted, 99.0),
            max_ms: *sorted.last().expect("non-empty checked above"),
        }
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[index]
}

#[allow(clippy::too_many_arguments)]
async fn run_phase(
    phase_index: usize,
    phase: Phase,
    workers: usize,
    query_mode: QueryMode,
    client: Arc<Client<HttpConnector, Full<Bytes>>>,
    base_uri: Arc<Uri>,
    names: Arc<Vec<NameRow>>,
    global_request_seq: &mut u64,
) -> Result<PhaseReport, CliError> {
    let total_requests: u64 = phase.rps as u64 * phase.duration_s as u64;
    if total_requests == 0 {
        return Ok(PhaseReport {
            rps: phase.rps,
            duration_s: phase.duration_s,
            issued: 0,
            errors: 0,
            samples_ms: Vec::new(),
            summary: LatencySummary::from_samples_ms(&[]),
        });
    }

    let workers_eff = workers.min(total_requests as usize).max(1);
    let interval = Duration::from_secs_f64(1.0_f64 / f64::from(phase.rps));
    let start_at = TokioInstant::now() + Duration::from_millis(10);

    // Pre-compute the per-request schedule, query shape, and name index.
    // We seed the PRNG with the system clock + phase index so each phase
    // gets a fresh sequence.
    let nanos_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0xC0FFEE);
    let mut rng = XorShift64::new(nanos_seed ^ (phase_index as u64).wrapping_mul(0x9E37_79B9));

    let mut schedule: Vec<RequestPlan> = Vec::with_capacity(total_requests as usize);
    for i in 0..total_requests {
        let seq = *global_request_seq + i;
        let due = start_at + interval.mul_f64(i as f64);
        let name_index = (rng.next_u64() as usize) % names.len();
        let use_bool_must = seq % 2 == 1;
        schedule.push(RequestPlan {
            due,
            seq,
            name_index,
            use_bool_must,
        });
    }
    *global_request_seq += total_requests;

    let (tx, mut rx) = mpsc::channel::<RequestSample>(workers_eff.saturating_mul(4).max(16));
    let chunks = partition(schedule, workers_eff);

    let mut joinset = JoinSet::new();
    for chunk in chunks {
        let client = Arc::clone(&client);
        let base_uri = Arc::clone(&base_uri);
        let names = Arc::clone(&names);
        let tx = tx.clone();
        joinset.spawn(async move {
            run_worker(client, base_uri, names, query_mode, chunk, tx).await;
        });
    }
    drop(tx);

    // Collect samples in real-time so the channel does not back up.
    let mut samples_ms: Vec<f64> = Vec::with_capacity(total_requests as usize);
    let mut errors: u64 = 0;
    while let Some(sample) = rx.recv().await {
        if sample.ok {
            samples_ms.push(sample.elapsed_ms);
        } else {
            errors += 1;
        }
    }

    while joinset.join_next().await.is_some() {}

    let issued = samples_ms.len() as u64 + errors;
    let summary = LatencySummary::from_samples_ms(&samples_ms);
    Ok(PhaseReport {
        rps: phase.rps,
        duration_s: phase.duration_s,
        issued,
        errors,
        samples_ms,
        summary,
    })
}

#[derive(Debug, Clone, Copy)]
struct RequestPlan {
    due: TokioInstant,
    seq: u64,
    name_index: usize,
    use_bool_must: bool,
}

struct RequestSample {
    ok: bool,
    elapsed_ms: f64,
}

fn partition<T>(mut items: Vec<T>, workers: usize) -> Vec<Vec<T>> {
    let workers = workers.max(1);
    let mut chunks: Vec<Vec<T>> = (0..workers).map(|_| Vec::new()).collect();
    for (i, item) in items.drain(..).enumerate() {
        chunks[i % workers].push(item);
    }
    chunks
}

async fn run_worker(
    client: Arc<Client<HttpConnector, Full<Bytes>>>,
    base_uri: Arc<Uri>,
    names: Arc<Vec<NameRow>>,
    query_mode: QueryMode,
    schedule: Vec<RequestPlan>,
    tx: mpsc::Sender<RequestSample>,
) {
    for plan in schedule {
        sleep_until(plan.due).await;
        let body = build_query_body(&names[plan.name_index], query_mode, plan.use_bool_must);
        let sample = issue_request(&client, &base_uri, body).await;
        if tx.send(sample).await.is_err() {
            break;
        }
        // give other workers / the scheduler a chance to advance
        let _ = plan.seq;
    }
}

fn build_query_body(name: &NameRow, query_mode: QueryMode, use_bool_must: bool) -> Bytes {
    let value = match query_mode {
        QueryMode::Insee => {
            if use_bool_must {
                json!({
                    "query": {
                        "bool": {
                            "must": [
                                { "match": { "PRENOMS": name.prenoms } },
                                { "match": { "NOM": name.nom } }
                            ]
                        }
                    },
                    "size": 10,
                    "track_total_hits": true
                })
            } else {
                json!({
                    "query": {
                        "multi_match": {
                            "query": format!("{} {}", name.prenoms, name.nom),
                            "fields": ["PRENOMS", "NOM"]
                        }
                    },
                    "size": 10,
                    "track_total_hits": true
                })
            }
        }
        QueryMode::Trec => {
            // `name.prenoms` holds the full free-text query. Both branches
            // issue a `multi_match` over title/text — the regime that
            // exercises long posting lists / skip-lists. Odd requests add
            // `operator:"and"` to vary the conjunction load (which is the
            // block-leapfrog hot path), even requests use default OR.
            if use_bool_must {
                json!({
                    "query": {
                        "multi_match": {
                            "query": name.prenoms,
                            "fields": ["title", "text"],
                            "operator": "and"
                        }
                    },
                    "size": 10,
                    "track_total_hits": true
                })
            } else {
                json!({
                    "query": {
                        "multi_match": {
                            "query": name.prenoms,
                            "fields": ["title", "text"]
                        }
                    },
                    "size": 10,
                    "track_total_hits": true
                })
            }
        }
    };
    Bytes::from(serde_json::to_vec(&value).expect("serde_json::to_vec on owned Value cannot fail"))
}

async fn issue_request(
    client: &Client<HttpConnector, Full<Bytes>>,
    base_uri: &Uri,
    body: Bytes,
) -> RequestSample {
    let start = Instant::now();
    let request = Request::builder()
        .method(Method::POST)
        .uri(base_uri.clone())
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::ACCEPT, "application/json")
        .body(Full::new(body));
    let request = match request {
        Ok(req) => req,
        Err(_) => {
            return RequestSample {
                ok: false,
                elapsed_ms: 0.0,
            };
        }
    };

    let response = match client.request(request).await {
        Ok(resp) => resp,
        Err(_) => {
            return RequestSample {
                ok: false,
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            };
        }
    };
    let status = response.status();
    // Drain the body so the connection can be returned to the pool.
    let collected = response.into_body().collect().await;
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    let ok = status.is_success() && collected.is_ok();
    RequestSample { ok, elapsed_ms }
}

fn build_report_json(
    plan: &Plan,
    started_at: &str,
    phase_reports: &[PhaseReport],
    global_issued: u64,
    global_errors: u64,
    global_summary: &LatencySummary,
) -> Value {
    let phases: Vec<Value> = phase_reports
        .iter()
        .map(|report| {
            json!({
                "rps": report.rps,
                "duration_s": report.duration_s,
                "issued": report.issued,
                "errors": report.errors,
                "p50_ms": round1(report.summary.p50_ms),
                "p95_ms": round1(report.summary.p95_ms),
                "p99_ms": round1(report.summary.p99_ms),
                "max_ms": round1(report.summary.max_ms)
            })
        })
        .collect();
    json!({
        "schema": REPORT_SCHEMA,
        "url": plan.url,
        "index": plan.index,
        "query_mode": plan.query_mode.as_str(),
        "workers": plan.workers,
        "names_path": plan.names_path,
        "started_at": started_at,
        "phases": phases,
        "global": {
            "issued": global_issued,
            "errors": global_errors,
            "p50_ms": round1(global_summary.p50_ms),
            "p95_ms": round1(global_summary.p95_ms),
            "p99_ms": round1(global_summary.p99_ms),
            "max_ms": round1(global_summary.max_ms)
        }
    })
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn write_report(path: &str, value: &Value) -> Result<(), CliError> {
    let path_buf = PathBuf::from(path);
    if let Some(parent) = path_buf.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| {
                CliError::Io(format!(
                    "failed to create report directory `{}`: {error}",
                    parent.display()
                ))
            })?;
        }
    }
    let body = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::Io(format!("failed to serialise report: {error}")))?;
    fs::write(&path_buf, format!("{body}\n"))
        .map_err(|error| CliError::Io(format!("failed to write report `{path}`: {error}")))?;
    let _ = Path::new(path);
    Ok(())
}

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        let seed = if seed == 0 {
            0xDEAD_BEEF_CAFE_BABE
        } else {
            seed
        };
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
}

fn print_help() {
    println!("surch-demo artillery_bench");
    println!("Rust keep-alive replacement for scripts/bench/artillery-replay.sh");
    println!();
    println!("USAGE:");
    println!("  artillery_bench [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("  --url URL          HTTP base URL of the target engine (default: {DEFAULT_URL})");
    println!("  --index NAME       target index name (default: {DEFAULT_INDEX})");
    println!("  --names PATH       insee: TSV of `PRENOMS\\tNOM` rows; trec: one free-text query per line (default: {DEFAULT_NAMES})");
    println!(
        "  --query-mode MODE  `insee` (PRENOMS/NOM) or `trec` (multi_match title/text) (default: {})",
        DEFAULT_QUERY_MODE.as_str()
    );
    println!("  --workers N        concurrent tokio workers sharing the keep-alive pool (default: {DEFAULT_WORKERS})");
    println!("  --phases LIST      comma-separated `rps:duration_s` segments (default: {DEFAULT_PHASES})");
    println!("  --report PATH      optional JSON report output path (schema: {REPORT_SCHEMA})");
    println!("  -h, --help         print this help");
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Io(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Io(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CliError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_phases_accepts_default_artillery_scenario() {
        let phases = parse_phases(DEFAULT_PHASES).expect("default phases must parse");
        assert_eq!(phases.len(), 6);
        assert_eq!(phases[0].rps, 2);
        assert_eq!(phases[0].duration_s, 30);
        assert_eq!(phases[5].rps, 50);
        assert_eq!(phases[5].duration_s, 60);
    }

    #[test]
    fn parse_phases_rejects_empty_input() {
        let error = parse_phases("").unwrap_err();
        assert!(matches!(error, CliError::Usage(_)));
    }

    #[test]
    fn parse_phases_rejects_missing_colon() {
        let error = parse_phases("2:30,10").unwrap_err();
        assert!(matches!(error, CliError::Usage(_)));
    }

    #[test]
    fn parse_phases_rejects_zero_rps() {
        let error = parse_phases("0:10").unwrap_err();
        assert!(matches!(error, CliError::Usage(_)));
    }

    #[test]
    fn parse_phases_rejects_zero_duration() {
        let error = parse_phases("5:0").unwrap_err();
        assert!(matches!(error, CliError::Usage(_)));
    }

    #[test]
    fn parse_phases_rejects_non_integer() {
        let error = parse_phases("a:10").unwrap_err();
        assert!(matches!(error, CliError::Usage(_)));
    }

    #[test]
    fn parse_phases_rejects_trailing_comma() {
        let error = parse_phases("2:30,").unwrap_err();
        assert!(matches!(error, CliError::Usage(_)));
    }

    #[test]
    fn latency_summary_from_samples_orders_percentiles() {
        let samples = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let summary = LatencySummary::from_samples_ms(&samples);
        assert_eq!(summary.p50_ms, 5.0);
        assert_eq!(summary.p95_ms, 10.0);
        assert_eq!(summary.p99_ms, 10.0);
        assert_eq!(summary.max_ms, 10.0);
    }

    #[test]
    fn build_query_body_alternates_between_shapes() {
        let name = NameRow {
            prenoms: "Marie".to_owned(),
            nom: "Dupont".to_owned(),
        };
        let multi = build_query_body(&name, QueryMode::Insee, false);
        let bool_must = build_query_body(&name, QueryMode::Insee, true);
        let multi_v: serde_json::Value = serde_json::from_slice(&multi).unwrap();
        let bool_v: serde_json::Value = serde_json::from_slice(&bool_must).unwrap();
        assert!(multi_v["query"]["multi_match"].is_object());
        assert!(bool_v["query"]["bool"]["must"].is_array());
        assert_eq!(multi_v["track_total_hits"], serde_json::Value::Bool(true));
        assert_eq!(bool_v["size"], serde_json::Value::from(10));
    }

    #[test]
    fn build_query_body_trec_uses_multi_match_over_title_text() {
        let row = NameRow {
            prenoms: "coronavirus origin bats".to_owned(),
            nom: String::new(),
        };
        let or_body = build_query_body(&row, QueryMode::Trec, false);
        let and_body = build_query_body(&row, QueryMode::Trec, true);
        let or_v: serde_json::Value = serde_json::from_slice(&or_body).unwrap();
        let and_v: serde_json::Value = serde_json::from_slice(&and_body).unwrap();
        assert_eq!(
            or_v["query"]["multi_match"]["query"],
            json!("coronavirus origin bats")
        );
        assert_eq!(
            or_v["query"]["multi_match"]["fields"],
            json!(["title", "text"])
        );
        assert!(or_v["query"]["multi_match"].get("operator").is_none());
        assert_eq!(and_v["query"]["multi_match"]["operator"], json!("and"));
        assert_eq!(and_v["track_total_hits"], serde_json::Value::Bool(true));
    }

    #[test]
    fn query_mode_parse_accepts_known_modes() {
        assert_eq!(QueryMode::parse("insee").unwrap(), QueryMode::Insee);
        assert_eq!(QueryMode::parse("TREC").unwrap(), QueryMode::Trec);
        assert!(matches!(
            QueryMode::parse("bogus").unwrap_err(),
            CliError::Usage(_)
        ));
    }

    #[test]
    fn build_report_json_has_expected_schema_and_phase_shape() {
        let plan = Plan {
            url: "http://127.0.0.1:7700".to_owned(),
            index: "deces_25k".to_owned(),
            names_path: "/tmp/names.tsv".to_owned(),
            workers: 4,
            phases: vec![Phase {
                rps: 5,
                duration_s: 10,
            }],
            report_path: None,
            query_mode: QueryMode::Insee,
        };
        let phase_reports = vec![PhaseReport {
            rps: 5,
            duration_s: 10,
            issued: 50,
            errors: 0,
            samples_ms: vec![1.0; 50],
            summary: LatencySummary::from_samples_ms(&vec![1.0; 50]),
        }];
        let global_summary = LatencySummary::from_samples_ms(&vec![1.0; 50]);
        let value = build_report_json(
            &plan,
            "2026-05-15T10:00:00.000Z",
            &phase_reports,
            50,
            0,
            &global_summary,
        );
        assert_eq!(value["schema"], json!(REPORT_SCHEMA));
        assert_eq!(value["url"], json!("http://127.0.0.1:7700"));
        assert_eq!(value["index"], json!("deces_25k"));
        assert_eq!(value["phases"].as_array().unwrap().len(), 1);
        assert_eq!(value["phases"][0]["rps"], json!(5));
        assert_eq!(value["phases"][0]["issued"], json!(50));
        assert_eq!(value["global"]["issued"], json!(50));
        assert_eq!(value["global"]["errors"], json!(0));
    }
}
