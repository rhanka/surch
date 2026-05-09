use std::{
    env, fmt,
    process::ExitCode,
    time::{Duration, Instant},
};

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use surch_api::app_router;
use tower::ServiceExt;

const BAN_TINY_NDJSON: &str =
    include_str!("../../../tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson");
const BAN_TINY_ORACLE: &str = "tests/opensearch_compat/oracle/replays/ban_tiny_search.json";
const BAN_TINY_DOCUMENTS: u64 = 3;
const SINGLE_HIT_COUNT: u64 = 1;
const DEFAULT_BENCH_ITERATIONS: usize = 1_000;
const DEFAULT_HTTP_BENCH_SURCH_URL: &str = "http://127.0.0.1:7700";
const DEFAULT_HTTP_BENCH_OPENSEARCH_URL: &str = "http://127.0.0.1:9200";
const DEFAULT_HTTP_BENCH_INDEX: &str = "ban_tiny";
const DEFAULT_HTTP_BENCH_DATASET: &str =
    "tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson";

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
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("ban-poc") => run_ban_poc().await,
        Some("ban-bench") => run_ban_bench(parse_bench_iterations(args)?).await,
        Some("ban-compare-plan") => {
            print_ban_compare_plan();
            Ok(())
        }
        Some("ban-http-bench") => match parse_ban_http_bench_args(args)? {
            BanHttpBenchCommand::Help => {
                print_ban_http_bench_help();
                Ok(())
            }
            BanHttpBenchCommand::Plan(plan) => {
                print_ban_http_bench_plan(&plan);
                Ok(())
            }
        },
        Some("--help") | Some("-h") | None => {
            print_help();
            Ok(())
        }
        Some(command) => Err(CliError::Usage(format!("unknown command `{command}`"))),
    }
}

async fn run_ban_poc() -> Result<(), CliError> {
    let router = load_ban_tiny().await?;
    let count = count_documents(router.clone()).await?;
    let match_label = first_hit_id(
        router.clone(),
        "match label",
        json!({
            "query": {
                "match": {
                    "label": "Rue de Rivoli"
                }
            }
        }),
    )
    .await?;
    let bool_address = first_hit_id(
        router.clone(),
        "bool address",
        json!({
            "query": {
                "bool": {
                    "must": [
                        {
                            "match": {
                                "street_name": "Cours de l'Intendance"
                            }
                        },
                        {
                            "match": {
                                "postcode": "33000"
                            }
                        }
                    ]
                }
            }
        }),
    )
    .await?;
    let fuzzy_label = first_hit_id(
        router,
        "fuzzy label",
        json!({
            "query": {
                "fuzzy": {
                    "label": {
                        "value": "Ale des Erables",
                        "fuzziness": 2
                    }
                }
            }
        }),
    )
    .await?;

    println!("Surch BAN PoC");
    println!("dataset: ban_tiny");
    println!("documents: {BAN_TINY_DOCUMENTS}");
    println!("count: {count}");
    println!("match label: {match_label}");
    println!("bool address: {bool_address}");
    println!("fuzzy label: {fuzzy_label}");
    println!("oracle: {BAN_TINY_ORACLE}");

    Ok(())
}

async fn run_ban_bench(iterations: usize) -> Result<(), CliError> {
    let load_metric = bench_load(iterations).await?;
    let router = load_ban_tiny().await?;
    let count_metric = bench_count(router.clone(), iterations).await?;
    let match_metric = bench_search(
        router.clone(),
        "search_match_label",
        iterations,
        json!({
            "query": {
                "match": {
                    "label": "Rue de Rivoli"
                }
            }
        }),
        "75101_0001_00001",
    )
    .await?;
    let bool_metric = bench_search(
        router.clone(),
        "search_bool_address",
        iterations,
        json!({
            "query": {
                "bool": {
                    "must": [
                        {
                            "match": {
                                "street_name": "Cours de l'Intendance"
                            }
                        },
                        {
                            "match": {
                                "postcode": "33000"
                            }
                        }
                    ]
                }
            }
        }),
        "33063_0002_00010B",
    )
    .await?;
    let fuzzy_metric = bench_search(
        router,
        "search_fuzzy_label",
        iterations,
        json!({
            "query": {
                "fuzzy": {
                    "label": {
                        "value": "Ale des Erables",
                        "fuzziness": 2
                    }
                }
            }
        }),
        "67482_0003_00007",
    )
    .await?;

    println!("Surch BAN bench");
    println!("dataset: ban_tiny");
    println!("documents: {BAN_TINY_DOCUMENTS}");
    println!("iterations: {iterations}");
    println!("oracle: {BAN_TINY_ORACLE}");
    println!("runtime: Surch in-process axum router");
    print_metric(&load_metric);
    print_metric(&count_metric);
    print_metric(&match_metric);
    print_metric(&bool_metric);
    print_metric(&fuzzy_metric);
    println!("guardrails:");
    println!("  - Surch in-process axum router only; no HTTP server is started");
    println!("  - no OpenSearch ratio; this harness does not run OpenSearch");
    println!("  - compare only on the same host/build only");
    println!("  - responses are validated against BAN tiny oracle counts and top hit ids");

    Ok(())
}

async fn load_ban_tiny() -> Result<Router, CliError> {
    let router = app_router();
    execute_json(router.clone(), Method::PUT, "/ban_tiny", None).await?;
    execute_json(
        router.clone(),
        Method::POST,
        "/_bulk",
        Some(BAN_TINY_NDJSON.to_owned()),
    )
    .await?;
    execute_json(router.clone(), Method::POST, "/ban_tiny/_refresh", None).await?;

    Ok(router)
}

async fn count_documents(router: Router) -> Result<u64, CliError> {
    let response = execute_json(router, Method::GET, "/ban_tiny/_count", None).await?;
    response
        .get("count")
        .and_then(Value::as_u64)
        .ok_or(CliError::UnexpectedResponse(
            "count response misses `count`",
        ))
}

async fn first_hit_id(
    router: Router,
    label: &'static str,
    body: Value,
) -> Result<String, CliError> {
    let response = execute_json(
        router,
        Method::POST,
        "/ban_tiny/_search",
        Some(body.to_string()),
    )
    .await?;
    response
        .pointer("/hits/hits/0/_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(CliError::MissingHit(label))
}

async fn execute_json(
    router: Router,
    method: Method,
    path: &str,
    body: Option<String>,
) -> Result<Value, CliError> {
    let response = router
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.unwrap_or_default()))
                .map_err(|error| CliError::Http(error.to_string()))?,
        )
        .await
        .map_err(|error| CliError::Http(error.to_string()))?;

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|error| CliError::Http(error.to_string()))?;
    let value: Value = serde_json::from_slice(&body)?;

    if status != StatusCode::OK && status != StatusCode::CREATED {
        return Err(CliError::UnexpectedStatus {
            status,
            body: value,
        });
    }

    Ok(value)
}

#[derive(Debug)]
struct BenchMetric {
    operation: &'static str,
    iterations: usize,
    count: u64,
    latencies: Vec<Duration>,
}

impl BenchMetric {
    fn new(
        operation: &'static str,
        iterations: usize,
        count: u64,
        latencies: Vec<Duration>,
    ) -> Result<Self, CliError> {
        if latencies.len() != iterations {
            return Err(CliError::UnexpectedResponse(
                "benchmark latency sample count mismatch",
            ));
        }

        Ok(Self {
            operation,
            iterations,
            count,
            latencies,
        })
    }

    fn latency_summary(&self) -> LatencySummary {
        LatencySummary::from_samples(&self.latencies)
    }
}

#[derive(Debug)]
struct LatencySummary {
    min: Duration,
    p50: Duration,
    p95: Duration,
    max: Duration,
    total: Duration,
}

impl LatencySummary {
    fn from_samples(samples: &[Duration]) -> Self {
        debug_assert!(!samples.is_empty());
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let total = samples
            .iter()
            .copied()
            .fold(Duration::ZERO, |sum, sample| sum + sample);

        Self {
            min: sorted[0],
            p50: percentile(&sorted, 50),
            p95: percentile(&sorted, 95),
            max: sorted[sorted.len() - 1],
            total,
        }
    }
}

async fn bench_load(iterations: usize) -> Result<BenchMetric, CliError> {
    let mut latencies = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let _router = load_ban_tiny().await?;
        latencies.push(start.elapsed());
    }

    BenchMetric::new("load_ban_tiny", iterations, BAN_TINY_DOCUMENTS, latencies)
}

async fn bench_count(router: Router, iterations: usize) -> Result<BenchMetric, CliError> {
    let mut latencies = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let count = count_documents(router.clone()).await?;
        latencies.push(start.elapsed());
        if count != BAN_TINY_DOCUMENTS {
            return Err(CliError::UnexpectedResponse(
                "count benchmark expected 3 docs",
            ));
        }
    }

    BenchMetric::new("count_match_all", iterations, BAN_TINY_DOCUMENTS, latencies)
}

async fn bench_search(
    router: Router,
    operation: &'static str,
    iterations: usize,
    body: Value,
    expected_top_hit_id: &'static str,
) -> Result<BenchMetric, CliError> {
    let body = body.to_string();
    let mut latencies = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let response = execute_json(
            router.clone(),
            Method::POST,
            "/ban_tiny/_search",
            Some(body.clone()),
        )
        .await?;
        latencies.push(start.elapsed());
        let total = response
            .pointer("/hits/total/value")
            .and_then(Value::as_u64)
            .ok_or(CliError::UnexpectedResponse(
                "search response misses total hit count",
            ))?;
        if total != 1 {
            return Err(CliError::UnexpectedResponse(
                "search benchmark expected one hit",
            ));
        }
        let top_hit_id = response
            .pointer("/hits/hits/0/_id")
            .and_then(Value::as_str)
            .ok_or(CliError::UnexpectedResponse(
                "search benchmark response misses top hit id",
            ))?;
        if top_hit_id != expected_top_hit_id {
            return Err(CliError::UnexpectedResponse(
                "search benchmark top hit id mismatched oracle",
            ));
        }
    }

    BenchMetric::new(operation, iterations, SINGLE_HIT_COUNT, latencies)
}

fn parse_bench_iterations(mut args: impl Iterator<Item = String>) -> Result<usize, CliError> {
    let mut iterations = DEFAULT_BENCH_ITERATIONS;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iterations" => {
                let value = args.next().ok_or_else(|| {
                    CliError::Usage("missing value after --iterations".to_owned())
                })?;
                iterations = value.parse::<usize>().map_err(|_| {
                    CliError::Usage("--iterations must be a positive integer".to_owned())
                })?;
                if iterations == 0 {
                    return Err(CliError::Usage(
                        "--iterations must be greater than zero".to_owned(),
                    ));
                }
            }
            "--help" | "-h" => {
                print_help();
            }
            unknown => return Err(CliError::Usage(format!("unknown option `{unknown}`"))),
        }
    }

    Ok(iterations)
}

#[derive(Debug)]
struct BanHttpBenchPlan {
    surch_url: String,
    opensearch_url: String,
    index: String,
    iterations: usize,
    dataset: String,
    oracle: String,
    warmup: usize,
    report: Option<String>,
}

impl Default for BanHttpBenchPlan {
    fn default() -> Self {
        Self {
            surch_url: DEFAULT_HTTP_BENCH_SURCH_URL.to_owned(),
            opensearch_url: DEFAULT_HTTP_BENCH_OPENSEARCH_URL.to_owned(),
            index: DEFAULT_HTTP_BENCH_INDEX.to_owned(),
            iterations: DEFAULT_BENCH_ITERATIONS,
            dataset: DEFAULT_HTTP_BENCH_DATASET.to_owned(),
            oracle: BAN_TINY_ORACLE.to_owned(),
            warmup: 0,
            report: None,
        }
    }
}

#[derive(Debug)]
enum BanHttpBenchCommand {
    Help,
    Plan(BanHttpBenchPlan),
}

fn parse_ban_http_bench_args(
    mut args: impl Iterator<Item = String>,
) -> Result<BanHttpBenchCommand, CliError> {
    let mut plan = BanHttpBenchPlan::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(BanHttpBenchCommand::Help),
            "--surch-url" => {
                let value = option_value(&mut args, "--surch-url")?;
                validate_http_url("--surch-url", &value)?;
                plan.surch_url = value;
            }
            "--opensearch-url" => {
                let value = option_value(&mut args, "--opensearch-url")?;
                validate_http_url("--opensearch-url", &value)?;
                plan.opensearch_url = value;
            }
            "--index" => {
                let value = option_value(&mut args, "--index")?;
                validate_index_name(&value)?;
                plan.index = value;
            }
            "--iterations" => {
                let value = option_value(&mut args, "--iterations")?;
                plan.iterations = parse_iterations(&value)?;
            }
            "--dataset" => {
                let value = option_value(&mut args, "--dataset")?;
                validate_dataset_path(&value)?;
                plan.dataset = value;
            }
            "--oracle" => {
                let value = option_value(&mut args, "--oracle")?;
                validate_non_empty_path("--oracle", &value)?;
                plan.oracle = value;
            }
            "--warmup" => {
                let value = option_value(&mut args, "--warmup")?;
                plan.warmup = parse_warmup(&value)?;
            }
            "--report" => {
                let value = option_value(&mut args, "--report")?;
                validate_non_empty_path("--report", &value)?;
                plan.report = Some(value);
            }
            unknown => return Err(CliError::Usage(format!("unknown option `{unknown}`"))),
        }
    }

    Ok(BanHttpBenchCommand::Plan(plan))
}

fn option_value(
    args: &mut impl Iterator<Item = String>,
    option: &'static str,
) -> Result<String, CliError> {
    args.next()
        .ok_or_else(|| CliError::Usage(format!("missing value after {option}")))
}

fn parse_iterations(value: &str) -> Result<usize, CliError> {
    let iterations = value
        .parse::<usize>()
        .map_err(|_| CliError::Usage("--iterations must be a positive integer".to_owned()))?;
    if iterations == 0 {
        return Err(CliError::Usage(
            "--iterations must be greater than zero".to_owned(),
        ));
    }

    Ok(iterations)
}

fn validate_http_url(option: &'static str, value: &str) -> Result<(), CliError> {
    if value.starts_with("http://") || value.starts_with("https://") {
        return Ok(());
    }

    Err(CliError::Usage(format!(
        "{option} must start with http:// or https://"
    )))
}

fn validate_index_name(value: &str) -> Result<(), CliError> {
    if value.is_empty() {
        return Err(CliError::Usage("--index must not be empty".to_owned()));
    }
    if value.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(CliError::Usage(
            "--index must not contain whitespace".to_owned(),
        ));
    }

    Ok(())
}

fn parse_warmup(value: &str) -> Result<usize, CliError> {
    value
        .parse::<usize>()
        .map_err(|_| CliError::Usage("--warmup must be a non-negative integer".to_owned()))
}

fn validate_dataset_path(value: &str) -> Result<(), CliError> {
    validate_non_empty_path("--dataset", value)?;
    if value
        .chars()
        .any(|character| character.is_whitespace() && character.is_control())
    {
        return Err(CliError::Usage(
            "--dataset must not contain control whitespace".to_owned(),
        ));
    }

    Ok(())
}

fn validate_non_empty_path(option: &'static str, value: &str) -> Result<(), CliError> {
    if value.is_empty() {
        return Err(CliError::Usage(format!("{option} must not be empty")));
    }

    Ok(())
}

fn print_metric(metric: &BenchMetric) {
    let latency = metric.latency_summary();
    let total_seconds = latency.total.as_secs_f64().max(f64::EPSILON);
    let ops_per_second = metric.iterations as f64 / total_seconds;

    println!(
        "{}: operation={} count={} iterations={} p50_us={:.2} p95_us={:.2} min_us={:.2} max_us={:.2} ops_per_second={:.2}",
        metric.operation,
        metric.operation,
        metric.count,
        metric.iterations,
        micros(latency.p50),
        micros(latency.p95),
        micros(latency.min),
        micros(latency.max),
        ops_per_second,
    );
}

fn percentile(sorted: &[Duration], percentile: usize) -> Duration {
    debug_assert!(!sorted.is_empty());
    let rank = (percentile as f64 / 100.0 * sorted.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[index]
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000.0
}

fn print_help() {
    println!("Surch demo commands");
    println!("  ban-poc");
    println!("  ban-bench [--iterations N]");
    println!("  ban-compare-plan");
    println!("  ban-http-bench [OPTIONS]");
}

fn print_ban_compare_plan() {
    println!("Surch BAN compare plan");
    println!("dataset: ban_tiny");
    println!("documents: {BAN_TINY_DOCUMENTS}");
    println!("oracle required: {BAN_TINY_ORACLE}");
    println!("OpenSearch over HTTP: measure only after healthcheck and index reset");
    println!("Surch in-process: current smoke path; label separately from HTTP runtimes");
    println!("operations: create index, bulk ingest, refresh, count, match, bool, fuzzy");
    println!("metrics: ingest duration, docs/s, min, p50, p95, p99, max, errors, top hit id");
    println!(
        "guardrail: no global ratio until Surch and OpenSearch use symmetric HTTP engine paths"
    );
    println!("guardrail: does not start OpenSearch, Docker, or a Surch server");
}

fn print_ban_http_bench_help() {
    println!("Surch BAN HTTP bench");
    println!("usage: surch-demo ban-http-bench [OPTIONS]");
    println!("prints a dry-run plan; sends no HTTP requests");
    println!("options:");
    println!(
        "  --surch-url URL        Surch HTTP base URL (default: {DEFAULT_HTTP_BENCH_SURCH_URL})"
    );
    println!(
        "  --opensearch-url URL   OpenSearch HTTP base URL (default: {DEFAULT_HTTP_BENCH_OPENSEARCH_URL})"
    );
    println!("  --index NAME           target index name (default: {DEFAULT_HTTP_BENCH_INDEX})");
    println!("  --iterations N         requests per measured operation (default: {DEFAULT_BENCH_ITERATIONS})");
    println!(
        "  --dataset PATH         NDJSON dataset path (default: {DEFAULT_HTTP_BENCH_DATASET})"
    );
    println!("  --oracle PATH          oracle replay path (default: {BAN_TINY_ORACLE})");
    println!("  --warmup N             warmup requests per operation (default: 0)");
    println!("  --report PATH          optional report output path");
    println!("  -h, --help             print this help");
}

fn print_ban_http_bench_plan(plan: &BanHttpBenchPlan) {
    println!("Surch BAN HTTP bench plan");
    println!("mode: dry-run");
    println!("dataset: {}", plan.dataset);
    println!("documents: {BAN_TINY_DOCUMENTS}");
    println!("index: {}", plan.index);
    println!("iterations: {}", plan.iterations);
    println!("warmup: {}", plan.warmup);
    println!("surch_url: {}", plan.surch_url);
    println!("opensearch_url: {}", plan.opensearch_url);
    println!("oracle: {}", plan.oracle);
    match &plan.report {
        Some(report) => println!("report: {report}"),
        None => println!("report: <none>"),
    }
    println!("operations:");
    println!("  - create_index");
    println!("  - bulk_ingest");
    println!("  - refresh");
    println!("  - count");
    println!("  - search_match_label");
    println!("  - search_bool_address");
    println!("  - search_fuzzy_label");
    println!("metrics: status, elapsed_us, docs, hits_total, top_hit_id, error");
    println!("guardrail: no HTTP requests are sent by this command yet");
    println!("guardrail: does not start OpenSearch, Docker, or a Surch server");
    println!("guardrail: compare only on the same host/build only");
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Http(String),
    Json(serde_json::Error),
    MissingHit(&'static str),
    UnexpectedResponse(&'static str),
    UnexpectedStatus { status: StatusCode, body: Value },
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Http(message) => formatter.write_str(message),
            Self::Json(error) => write!(formatter, "{error}"),
            Self::MissingHit(label) => write!(formatter, "{label} returned no hit"),
            Self::UnexpectedResponse(message) => formatter.write_str(message),
            Self::UnexpectedStatus { status, body } => {
                write!(formatter, "unexpected HTTP status {status}: {body}")
            }
        }
    }
}

impl std::error::Error for CliError {}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
