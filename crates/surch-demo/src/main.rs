use std::{
    collections::BTreeMap,
    env, fmt, fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::Path,
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
const DEFAULT_HTTP_BENCH_ELASTICSEARCH_URL: &str = "http://127.0.0.1:9200";
const DEFAULT_HTTP_BENCH_INDEX: &str = "ban_tiny";
const DEFAULT_HTTP_BENCH_DATASET: &str =
    "tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson";
const DEFAULT_HTTP_TIMEOUT_SECONDS: u64 = 30;
const BAN_HTTP_BENCH_SCHEMA: &str = "surch.bench.ban_http.v1";

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
                if plan.dry_run {
                    print_ban_http_bench_plan(&plan)
                } else {
                    run_ban_http_bench(plan).await
                }
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
                    "label": {
                        "query": "Rue de Rivoli",
                        "operator": "AND"
                    }
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
                                "street_name": {
                                    "query": "Cours de l'Intendance",
                                    "operator": "AND"
                                }
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
    println!("  - no Elasticsearch ratio; this harness does not run Elasticsearch");
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
    p99: Duration,
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
            p99: percentile(&sorted, 99),
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
    elasticsearch_url: String,
    index: String,
    iterations: usize,
    dataset: String,
    oracle: String,
    warmup: usize,
    report: Option<String>,
    timeout_seconds: u64,
    dry_run: bool,
}

impl Default for BanHttpBenchPlan {
    fn default() -> Self {
        Self {
            surch_url: DEFAULT_HTTP_BENCH_SURCH_URL.to_owned(),
            elasticsearch_url: DEFAULT_HTTP_BENCH_ELASTICSEARCH_URL.to_owned(),
            index: DEFAULT_HTTP_BENCH_INDEX.to_owned(),
            iterations: DEFAULT_BENCH_ITERATIONS,
            dataset: DEFAULT_HTTP_BENCH_DATASET.to_owned(),
            oracle: BAN_TINY_ORACLE.to_owned(),
            warmup: 0,
            report: None,
            timeout_seconds: DEFAULT_HTTP_TIMEOUT_SECONDS,
            dry_run: false,
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
            "--elasticsearch-url" => {
                let value = option_value(&mut args, "--elasticsearch-url")?;
                validate_http_url("--elasticsearch-url", &value)?;
                plan.elasticsearch_url = value;
            }
            "--opensearch-url" => {
                let value = option_value(&mut args, "--opensearch-url")?;
                validate_http_url("--opensearch-url", &value)?;
                plan.elasticsearch_url = value;
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
            "--timeout-seconds" => {
                let value = option_value(&mut args, "--timeout-seconds")?;
                plan.timeout_seconds = parse_positive_u64("--timeout-seconds", &value)?;
            }
            "--dry-run" => {
                plan.dry_run = true;
            }
            unknown => return Err(CliError::Usage(format!("unknown option `{unknown}`"))),
        }
    }

    Ok(BanHttpBenchCommand::Plan(plan))
}

async fn run_ban_http_bench(plan: BanHttpBenchPlan) -> Result<(), CliError> {
    let dataset = BulkDataset::load(&plan.dataset, &plan.index)?;
    let oracle = load_oracle_requests(&plan.oracle, &plan.index)?;
    let surch = HttpEndpoint::parse(&plan.surch_url)?;
    let elasticsearch = HttpEndpoint::parse(&plan.elasticsearch_url)?;

    let surch_report = run_engine_http_bench("surch", &surch, &plan, &dataset, &oracle)?;
    let elasticsearch_report =
        run_engine_http_bench("elasticsearch", &elasticsearch, &plan, &dataset, &oracle)?;
    let reports = vec![surch_report, elasticsearch_report];

    print_ban_http_bench_result(&plan, &dataset, &reports);
    if let Some(report) = &plan.report {
        write_json_report(
            report,
            ban_http_bench_report_json(&plan, &dataset, &reports),
        )?;
    }

    Ok(())
}

fn ban_http_bench_report_json(
    plan: &BanHttpBenchPlan,
    dataset: &BulkDataset,
    reports: &[EngineBenchReport],
) -> Value {
    json!({
        "schema": BAN_HTTP_BENCH_SCHEMA,
        "benchmark": "ban-http-bench",
        "dataset": &plan.dataset,
        "dataset_bytes": dataset.bytes,
        "documents": dataset.documents,
        "index": &plan.index,
        "oracle": &plan.oracle,
        "iterations": plan.iterations,
        "warmup": plan.warmup,
        "timeout_seconds": plan.timeout_seconds,
        "runtime": "symmetric HTTP",
        "http_client": "persistent HTTP/1.1 keep-alive per engine",
        "engines": reports.iter().map(EngineBenchReport::to_json).collect::<Vec<_>>(),
        "guardrails": [
            "same Rust HTTP client path for both engines",
            "persistent HTTP/1.1 connections are reused across warmup and measured samples",
            "oracle validation is required before measured query iterations",
            "no global Surch/Elasticsearch ratio is emitted"
        ]
    })
}

fn run_engine_http_bench(
    name: &'static str,
    endpoint: &HttpEndpoint,
    plan: &BanHttpBenchPlan,
    dataset: &BulkDataset,
    oracle: &[OracleRequest],
) -> Result<EngineBenchReport, CliError> {
    let mut client = HttpClient::new(endpoint.clone(), Duration::from_secs(plan.timeout_seconds));

    reset_index(&mut client, &plan.index)?;
    let mut metrics = Vec::new();
    metrics.push(measure_setup_request(
        &mut client,
        "create_index",
        "PUT",
        &format!("/{}", plan.index),
        Some("{}"),
        "application/json",
        SetupMeasurement::None,
    )?);
    metrics.push(measure_setup_request(
        &mut client,
        "bulk_ingest",
        "POST",
        "/_bulk",
        Some(&dataset.body),
        "application/x-ndjson",
        SetupMeasurement::Bulk {
            docs: dataset.documents,
            bytes: dataset.bytes,
        },
    )?);
    metrics.push(measure_setup_request(
        &mut client,
        "refresh",
        "POST",
        &format!("/{}/_refresh", plan.index),
        None,
        "application/json",
        SetupMeasurement::None,
    )?);

    for request in oracle {
        validate_oracle_request(&mut client, request)?;
    }
    for _ in 0..plan.warmup {
        for request in oracle {
            validate_oracle_request(&mut client, request)?;
        }
    }
    for request in oracle {
        metrics.push(measure_oracle_request(
            &mut client,
            request,
            plan.iterations,
        )?);
    }

    Ok(EngineBenchReport {
        name: name.to_owned(),
        url: endpoint.base_url.clone(),
        metrics,
    })
}

fn reset_index(client: &mut HttpClient, index: &str) -> Result<(), CliError> {
    let response = client.request_json("DELETE", &format!("/{index}"), None, "application/json")?;
    if matches!(response.status, 200 | 202 | 404) {
        return Ok(());
    }

    Err(CliError::UnexpectedStatus {
        status: StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        body: response.body,
    })
}

enum SetupMeasurement {
    None,
    Bulk { docs: u64, bytes: u64 },
}

fn measure_setup_request(
    client: &mut HttpClient,
    operation: &'static str,
    method: &'static str,
    path: &str,
    body: Option<&str>,
    content_type: &'static str,
    measurement: SetupMeasurement,
) -> Result<OperationMetric, CliError> {
    let response = client.request_json(method, path, body, content_type)?;
    require_success(&response)?;
    if operation == "bulk_ingest"
        && response
            .body
            .get("errors")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    {
        return Err(CliError::UnexpectedResponse(
            "bulk response reported item errors",
        ));
    }

    let (docs, bytes) = match measurement {
        SetupMeasurement::None => (None, None),
        SetupMeasurement::Bulk { docs, bytes } => (Some(docs), Some(bytes)),
    };

    Ok(OperationMetric {
        operation: operation.to_owned(),
        status: response.status,
        iterations: 1,
        samples: vec![response.elapsed],
        docs,
        bytes,
        error_count: 0,
        hits: None,
        top_hit_id: None,
        server_took_ms: response.body.get("took").and_then(Value::as_u64),
    })
}

fn measure_oracle_request(
    client: &mut HttpClient,
    request: &OracleRequest,
    iterations: usize,
) -> Result<OperationMetric, CliError> {
    let mut samples = Vec::with_capacity(iterations);
    let mut hits = None;
    let mut top_hit_id = None;
    let mut server_took_ms = None;
    let mut status = request.expected_status;

    for _ in 0..iterations {
        let response = client.request_json(
            &request.method,
            &request.path,
            request.body.as_deref(),
            "application/json",
        )?;
        validate_oracle_response(request, &response)?;
        status = response.status;
        samples.push(response.elapsed);
        if let Some(total) = response
            .body
            .pointer("/hits/total/value")
            .and_then(Value::as_u64)
        {
            hits = Some(total);
        }
        if let Some(id) = response
            .body
            .pointer("/hits/hits/0/_id")
            .and_then(Value::as_str)
        {
            top_hit_id = Some(id.to_owned());
        }
        if let Some(count) = response.body.get("count").and_then(Value::as_u64) {
            hits = Some(count);
        }
        if let Some(took) = response.body.get("took").and_then(Value::as_u64) {
            server_took_ms = Some(took);
        }
    }

    Ok(OperationMetric {
        operation: request.name.clone(),
        status,
        iterations,
        samples,
        docs: None,
        bytes: None,
        error_count: 0,
        hits,
        top_hit_id,
        server_took_ms,
    })
}

fn validate_oracle_request(
    client: &mut HttpClient,
    request: &OracleRequest,
) -> Result<(), CliError> {
    let response = client.request_json(
        &request.method,
        &request.path,
        request.body.as_deref(),
        "application/json",
    )?;
    validate_oracle_response(request, &response)
}

fn validate_oracle_response(
    request: &OracleRequest,
    response: &HttpJsonResponse,
) -> Result<(), CliError> {
    if response.status != request.expected_status {
        return Err(CliError::UnexpectedStatus {
            status: StatusCode::from_u16(response.status)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body: response.body.clone(),
        });
    }
    if let Some(expected) = &request.expected_response {
        compare_json_response(
            expected,
            &response.body,
            &mut Vec::new(),
            &request.comparison,
        )?;
    }

    Ok(())
}

fn require_success(response: &HttpJsonResponse) -> Result<(), CliError> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }

    Err(CliError::UnexpectedStatus {
        status: StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        body: response.body.clone(),
    })
}

struct BulkDataset {
    body: String,
    documents: u64,
    bytes: u64,
}

impl BulkDataset {
    fn load(path: &str, index: &str) -> Result<Self, CliError> {
        let content = read_input_file("dataset", path)?;
        let bytes = content.len() as u64;
        let mut lines = content.lines().filter(|line| !line.trim().is_empty());
        let mut body = String::new();
        let mut documents = 0;

        while let Some(action_line) = lines.next() {
            let source_line = lines.next().ok_or(CliError::UnexpectedResponse(
                "bulk dataset action line is missing a source line",
            ))?;
            let mut action: Value = serde_json::from_str(action_line)?;
            let action_object = action.as_object_mut().ok_or(CliError::UnexpectedResponse(
                "bulk action line must be a JSON object",
            ))?;
            let (_, metadata) = action_object
                .iter_mut()
                .next()
                .ok_or(CliError::UnexpectedResponse("bulk action line is empty"))?;
            let metadata = metadata
                .as_object_mut()
                .ok_or(CliError::UnexpectedResponse(
                    "bulk action metadata must be a JSON object",
                ))?;
            metadata.insert("_index".to_owned(), Value::String(index.to_owned()));

            body.push_str(&serde_json::to_string(&action)?);
            body.push('\n');
            body.push_str(source_line);
            body.push('\n');
            documents += 1;
        }

        if documents == 0 {
            return Err(CliError::UnexpectedResponse(
                "bulk dataset must contain at least one document",
            ));
        }

        Ok(Self {
            body,
            documents,
            bytes,
        })
    }
}

#[derive(Clone, Debug)]
struct OracleRequest {
    name: String,
    method: String,
    path: String,
    body: Option<String>,
    expected_status: u16,
    expected_response: Option<Value>,
    comparison: OracleComparison,
}

#[derive(Clone, Debug)]
struct OracleComparison {
    ignored_paths: Vec<Vec<String>>,
    score_tolerance: f64,
}

impl OracleComparison {
    fn from_replay(value: &Value) -> Self {
        let comparison = value.get("comparison").unwrap_or(&Value::Null);
        let ignored_paths = comparison
            .get("ignored_paths")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|path| path.split('.').map(str::to_owned).collect())
            .collect();
        let score_tolerance = comparison
            .get("score_tolerance")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);

        Self {
            ignored_paths,
            score_tolerance,
        }
    }

    fn ignores(&self, path: &[String]) -> bool {
        self.ignored_paths.iter().any(|ignored| {
            ignored.len() == path.len()
                && ignored
                    .iter()
                    .zip(path)
                    .all(|(pattern, segment)| pattern == "*" || pattern == segment)
        })
    }

    fn number_tolerance(&self, path: &[String]) -> f64 {
        if path
            .last()
            .is_some_and(|segment| segment == "_score" || segment == "max_score")
        {
            self.score_tolerance
        } else {
            0.0
        }
    }
}

fn load_oracle_requests(path: &str, index: &str) -> Result<Vec<OracleRequest>, CliError> {
    let content = read_input_file("oracle", path)?;
    let value: Value = serde_json::from_str(&content)?;
    let requests =
        value
            .get("requests")
            .and_then(Value::as_array)
            .ok_or(CliError::UnexpectedResponse(
                "oracle replay must contain a requests array",
            ))?;
    let mut oracle = Vec::new();
    let comparison = OracleComparison::from_replay(&value);

    for request in requests {
        let name = request
            .get("name")
            .and_then(Value::as_str)
            .ok_or(CliError::UnexpectedResponse("oracle request misses name"))?
            .to_owned();
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .ok_or(CliError::UnexpectedResponse("oracle request misses method"))?
            .to_owned();
        let path = request
            .get("path")
            .and_then(Value::as_str)
            .ok_or(CliError::UnexpectedResponse("oracle request misses path"))?;
        let body = request.get("body").map(Value::to_string);
        let expected_status = request
            .get("expected_status")
            .and_then(Value::as_u64)
            .ok_or(CliError::UnexpectedResponse(
                "oracle request misses expected_status",
            ))? as u16;
        let mut expected_response = request.get("expected_response").cloned();
        if let Some(expected_response) = &mut expected_response {
            rewrite_expected_index(expected_response, index);
        }

        oracle.push(OracleRequest {
            name,
            method,
            path: rewrite_oracle_index_path(path, index),
            body,
            expected_status,
            expected_response,
            comparison: comparison.clone(),
        });
    }

    if oracle.is_empty() {
        return Err(CliError::UnexpectedResponse(
            "oracle replay must contain at least one request",
        ));
    }

    Ok(oracle)
}

fn rewrite_oracle_index_path(path: &str, index: &str) -> String {
    if path == "/ban_tiny" {
        return format!("/{index}");
    }
    if let Some(suffix) = path.strip_prefix("/ban_tiny/") {
        return format!("/{index}/{suffix}");
    }
    path.to_owned()
}

fn rewrite_expected_index(value: &mut Value, index: &str) {
    match value {
        Value::String(text) if text == "ban_tiny" => {
            *text = index.to_owned();
        }
        Value::Array(items) => {
            for item in items {
                rewrite_expected_index(item, index);
            }
        }
        Value::Object(fields) => {
            for value in fields.values_mut() {
                rewrite_expected_index(value, index);
            }
        }
        _ => {}
    }
}

fn compare_json_response(
    expected: &Value,
    actual: &Value,
    path: &mut Vec<String>,
    comparison: &OracleComparison,
) -> Result<(), CliError> {
    if comparison.ignores(path) {
        return Ok(());
    }

    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            for (key, expected_value) in expected {
                path.push(key.clone());
                if comparison.ignores(path) {
                    path.pop();
                    continue;
                }
                let actual_value = actual.get(key).ok_or_else(|| {
                    CliError::OracleMismatch(format!(
                        "oracle response missing path {}",
                        format_json_path(path)
                    ))
                })?;
                compare_json_response(expected_value, actual_value, path, comparison)?;
                path.pop();
            }
            for key in actual.keys() {
                if expected.contains_key(key) {
                    continue;
                }
                path.push(key.clone());
                let ignored = comparison.ignores(path);
                let formatted = format_json_path(path);
                path.pop();
                if !ignored {
                    return Err(CliError::OracleMismatch(format!(
                        "oracle response had unexpected path {formatted}"
                    )));
                }
            }
            Ok(())
        }
        (Value::Array(expected), Value::Array(actual)) => {
            if expected.len() != actual.len() {
                return Err(CliError::OracleMismatch(format!(
                    "oracle response array length mismatch at {}",
                    format_json_path(path)
                )));
            }
            for (index, (expected_value, actual_value)) in expected.iter().zip(actual).enumerate() {
                path.push(index.to_string());
                compare_json_response(expected_value, actual_value, path, comparison)?;
                path.pop();
            }
            Ok(())
        }
        (Value::Number(expected), Value::Number(actual)) => {
            let expected = expected.as_f64().ok_or_else(|| {
                CliError::OracleMismatch(format!(
                    "oracle expected non-finite number at {}",
                    format_json_path(path)
                ))
            })?;
            let actual = actual.as_f64().ok_or_else(|| {
                CliError::OracleMismatch(format!(
                    "oracle actual non-finite number at {}",
                    format_json_path(path)
                ))
            })?;
            let tolerance = comparison.number_tolerance(path);
            if (expected - actual).abs() <= tolerance {
                Ok(())
            } else {
                Err(CliError::OracleMismatch(format!(
                    "oracle number mismatch at {}",
                    format_json_path(path)
                )))
            }
        }
        _ if expected == actual => Ok(()),
        _ => Err(CliError::OracleMismatch(format!(
            "oracle response mismatch at {}",
            format_json_path(path)
        ))),
    }
}

fn format_json_path(path: &[String]) -> String {
    if path.is_empty() {
        "$".to_owned()
    } else {
        format!("$.{}", path.join("."))
    }
}

struct EngineBenchReport {
    name: String,
    url: String,
    metrics: Vec<OperationMetric>,
}

impl EngineBenchReport {
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "url": self.url,
            "metrics": self.metrics.iter().map(OperationMetric::to_json).collect::<Vec<_>>()
        })
    }
}

struct OperationMetric {
    operation: String,
    status: u16,
    iterations: usize,
    samples: Vec<Duration>,
    docs: Option<u64>,
    bytes: Option<u64>,
    error_count: u64,
    hits: Option<u64>,
    top_hit_id: Option<String>,
    server_took_ms: Option<u64>,
}

impl OperationMetric {
    fn latency_summary(&self) -> LatencySummary {
        LatencySummary::from_samples(&self.samples)
    }

    fn to_json(&self) -> Value {
        let latency = self.latency_summary();
        json!({
            "operation": self.operation,
            "status": self.status,
            "iterations": self.iterations,
            "docs": self.docs,
            "bytes": self.bytes,
            "docs_per_second": rate_per_second(self.docs, latency.total),
            "bytes_per_second": rate_per_second(self.bytes, latency.total),
            "error_count": self.error_count,
            "hits": self.hits,
            "top_hit_id": self.top_hit_id,
            "server_took_ms": self.server_took_ms,
            "latency_us": {
                "min": micros_u64(latency.min),
                "p50": micros_u64(latency.p50),
                "p95": micros_u64(latency.p95),
                "p99": micros_u64(latency.p99),
                "max": micros_u64(latency.max),
                "total": micros_u64(latency.total)
            },
            "samples_us": self.samples.iter().copied().map(micros_u64).collect::<Vec<_>>()
        })
    }
}

#[derive(Clone, Debug)]
struct HttpEndpoint {
    base_url: String,
    authority: String,
    host: String,
    port: u16,
    base_path: String,
}

impl HttpEndpoint {
    fn parse(value: &str) -> Result<Self, CliError> {
        let rest = value.strip_prefix("http://").ok_or_else(|| {
            CliError::Usage("ban-http-bench currently supports http:// URLs only".to_owned())
        })?;
        let (authority, base_path) = rest
            .split_once('/')
            .map_or((rest, ""), |(authority, path)| (authority, path));
        if authority.is_empty() {
            return Err(CliError::Usage(
                "HTTP URL host must not be empty".to_owned(),
            ));
        }
        let (host, port) = parse_authority(authority)?;
        let base_path = if base_path.is_empty() {
            String::new()
        } else {
            format!("/{}", base_path.trim_end_matches('/'))
        };

        Ok(Self {
            base_url: value.trim_end_matches('/').to_owned(),
            authority: authority.to_owned(),
            host,
            port,
            base_path,
        })
    }

    fn connect(&self, timeout: Duration) -> Result<BufReader<TcpStream>, CliError> {
        let address = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|error| CliError::Http(format!("failed to resolve {}: {error}", self.host)))?
            .next()
            .ok_or_else(|| CliError::Http(format!("failed to resolve {}", self.host)))?;
        let stream = TcpStream::connect_timeout(&address, timeout).map_err(|error| {
            CliError::Http(format!("failed to connect to {}: {error}", self.base_url))
        })?;
        stream
            .set_nodelay(true)
            .map_err(|error| CliError::Http(error.to_string()))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|error| CliError::Http(error.to_string()))?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|error| CliError::Http(error.to_string()))?;
        Ok(BufReader::new(stream))
    }

    fn request_path(&self, path: &str) -> String {
        let path = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        if self.base_path.is_empty() {
            path
        } else {
            format!("{}{}", self.base_path, path)
        }
    }
}

struct HttpClient {
    endpoint: HttpEndpoint,
    timeout: Duration,
    reader: Option<BufReader<TcpStream>>,
}

impl HttpClient {
    fn new(endpoint: HttpEndpoint, timeout: Duration) -> Self {
        Self {
            endpoint,
            timeout,
            reader: None,
        }
    }

    fn request_json(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&str>,
        content_type: &str,
    ) -> Result<HttpJsonResponse, CliError> {
        let start = Instant::now();
        let body = body.unwrap_or_default();
        let request_path = self.endpoint.request_path(path);
        let mut request = format!(
            "{method} {request_path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: surch-demo/ban-http-bench\r\nAccept: application/json\r\nConnection: keep-alive\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
            self.endpoint.authority,
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body.as_bytes());

        let response = {
            let reader = self.reader()?;
            reader
                .get_mut()
                .write_all(&request)
                .map_err(|error| CliError::Http(error.to_string()))?;
            reader
                .get_mut()
                .flush()
                .map_err(|error| CliError::Http(error.to_string()))?;
            read_http_response(reader)?
        };
        let elapsed = start.elapsed();
        if response_should_close(&response.headers) {
            self.reader = None;
        }
        let body = if response.body.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&response.body).map_err(|error| {
                CliError::Http(format!("HTTP response body was not JSON: {error}"))
            })?
        };

        Ok(HttpJsonResponse {
            status: response.status,
            body,
            elapsed,
        })
    }

    fn reader(&mut self) -> Result<&mut BufReader<TcpStream>, CliError> {
        if self.reader.is_none() {
            self.reader = Some(self.endpoint.connect(self.timeout)?);
        }
        self.reader.as_mut().ok_or(CliError::UnexpectedResponse(
            "HTTP client missed connection",
        ))
    }
}

fn parse_authority(authority: &str) -> Result<(String, u16), CliError> {
    if authority.starts_with('[') {
        let end = authority
            .find(']')
            .ok_or_else(|| CliError::Usage("invalid IPv6 host in URL".to_owned()))?;
        let host = authority[1..end].to_owned();
        let port = authority[end + 1..]
            .strip_prefix(':')
            .map(parse_port)
            .transpose()?
            .unwrap_or(80);
        return Ok((host, port));
    }

    let (host, port) = authority
        .rsplit_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    let port = port.map(parse_port).transpose()?.unwrap_or(80);
    Ok((host.to_owned(), port))
}

fn parse_port(port: &str) -> Result<u16, CliError> {
    port.parse::<u16>()
        .map_err(|_| CliError::Usage("URL port must be a valid u16".to_owned()))
}

struct HttpJsonResponse {
    status: u16,
    body: Value,
    elapsed: Duration,
}

struct ParsedHttpResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn read_http_response(reader: &mut BufReader<TcpStream>) -> Result<ParsedHttpResponse, CliError> {
    let mut status_line = String::new();
    if reader
        .read_line(&mut status_line)
        .map_err(|error| CliError::Http(error.to_string()))?
        == 0
    {
        return Err(CliError::UnexpectedResponse(
            "HTTP response missed status line",
        ));
    }
    let status_line = status_line.trim_end_matches(['\r', '\n']);
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or(CliError::UnexpectedResponse(
            "HTTP response status line missed status code",
        ))?
        .parse::<u16>()
        .map_err(|_| CliError::UnexpectedResponse("HTTP status code was invalid"))?;

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader
            .read_line(&mut line)
            .map_err(|error| CliError::Http(error.to_string()))?
            == 0
        {
            return Err(CliError::UnexpectedResponse(
                "HTTP response ended before headers completed",
            ));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }

    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        read_chunked_body(reader)?
    } else if let Some(length) = headers.get("content-length") {
        let length = length
            .parse::<usize>()
            .map_err(|_| CliError::UnexpectedResponse("HTTP content-length was invalid"))?;
        let mut body = vec![0; length];
        reader
            .read_exact(&mut body)
            .map_err(|error| CliError::Http(error.to_string()))?;
        body
    } else {
        let mut body = Vec::new();
        reader
            .read_to_end(&mut body)
            .map_err(|error| CliError::Http(error.to_string()))?;
        body
    };

    Ok(ParsedHttpResponse {
        status,
        headers,
        body,
    })
}

fn read_chunked_body(reader: &mut BufReader<TcpStream>) -> Result<Vec<u8>, CliError> {
    let mut decoded = Vec::new();
    loop {
        let mut size_line = String::new();
        if reader
            .read_line(&mut size_line)
            .map_err(|error| CliError::Http(error.to_string()))?
            == 0
        {
            return Err(CliError::UnexpectedResponse(
                "chunked response missed chunk size",
            ));
        }
        let size_text = size_line.trim_end_matches(['\r', '\n']);
        let size_text = size_text.split(';').next().unwrap_or(size_text).trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| CliError::UnexpectedResponse("chunked response chunk size was invalid"))?;
        if size == 0 {
            loop {
                let mut trailer = String::new();
                if reader
                    .read_line(&mut trailer)
                    .map_err(|error| CliError::Http(error.to_string()))?
                    == 0
                {
                    return Err(CliError::UnexpectedResponse(
                        "chunked response ended before trailer terminator",
                    ));
                }
                if trailer.trim_end_matches(['\r', '\n']).is_empty() {
                    return Ok(decoded);
                }
            }
        }

        let mut chunk = vec![0; size];
        reader
            .read_exact(&mut chunk)
            .map_err(|error| CliError::Http(error.to_string()))?;
        decoded.extend_from_slice(&chunk);

        let mut crlf = [0; 2];
        reader
            .read_exact(&mut crlf)
            .map_err(|error| CliError::Http(error.to_string()))?;
        if crlf != *b"\r\n" {
            return Err(CliError::UnexpectedResponse(
                "chunked response chunk missed trailing CRLF",
            ));
        }
    }
}

fn response_should_close(headers: &BTreeMap<String, String>) -> bool {
    headers
        .get("connection")
        .is_some_and(|value| value.to_ascii_lowercase().contains("close"))
}

fn print_ban_http_bench_result(
    plan: &BanHttpBenchPlan,
    dataset: &BulkDataset,
    reports: &[EngineBenchReport],
) {
    println!("Surch BAN HTTP bench");
    println!("mode: executed");
    println!("dataset: {}", plan.dataset);
    println!("documents: {}", dataset.documents);
    println!("dataset_bytes: {}", dataset.bytes);
    println!("index: {}", plan.index);
    println!("iterations: {}", plan.iterations);
    println!("warmup: {}", plan.warmup);
    println!("timeout_seconds: {}", plan.timeout_seconds);
    println!("oracle: {}", plan.oracle);
    for report in reports {
        println!("engine: {} url={}", report.name, report.url);
        for metric in &report.metrics {
            let latency = metric.latency_summary();
            println!(
                "  {}: status={} iterations={} min_us={} p50_us={} p95_us={} p99_us={} max_us={} docs={} bytes={} errors={} hits={} top_hit_id={}",
                metric.operation,
                metric.status,
                metric.iterations,
                micros_u64(latency.min),
                micros_u64(latency.p50),
                micros_u64(latency.p95),
                micros_u64(latency.p99),
                micros_u64(latency.max),
                optional_u64(metric.docs),
                optional_u64(metric.bytes),
                metric.error_count,
                optional_u64(metric.hits),
                metric.top_hit_id.as_deref().unwrap_or("-")
            );
        }
    }
    println!("guardrails:");
    println!("  - oracle validation passed before measured query iterations");
    println!("  - Surch and Elasticsearch used the same persistent Rust HTTP client path");
    println!("  - no global performance ratio emitted");
}

fn optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

fn write_json_report(path: &str, value: Value) -> Result<(), CliError> {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| {
                CliError::Http(format!(
                    "failed to create report directory `{}`: {error}",
                    parent.display()
                ))
            })?;
        }
    }
    let body = serde_json::to_string_pretty(&value)?;
    fs::write(path, format!("{body}\n"))
        .map_err(|error| CliError::Http(format!("failed to write report `{path}`: {error}")))
}

fn micros_u64(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

fn rate_per_second(count: Option<u64>, duration: Duration) -> Option<f64> {
    let count = count?;
    let seconds = duration.as_secs_f64();
    if seconds == 0.0 {
        return None;
    }
    Some(count as f64 / seconds)
}

fn read_input_file(kind: &'static str, path: &str) -> Result<String, CliError> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(error) if !Path::new(path).is_absolute() => {
            let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
            let workspace_root = manifest_dir
                .parent()
                .and_then(Path::parent)
                .unwrap_or(manifest_dir);
            let fallback = workspace_root.join(path);
            fs::read_to_string(&fallback).map_err(|fallback_error| {
                CliError::Http(format!(
                    "failed to read {kind} `{path}`: {error}; fallback `{}` failed: {fallback_error}",
                    fallback.display()
                ))
            })
        }
        Err(error) => Err(CliError::Http(format!(
            "failed to read {kind} `{path}`: {error}"
        ))),
    }
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

fn parse_positive_u64(option: &'static str, value: &str) -> Result<u64, CliError> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| CliError::Usage(format!("{option} must be a positive integer")))?;
    if parsed == 0 {
        return Err(CliError::Usage(format!(
            "{option} must be greater than zero"
        )));
    }

    Ok(parsed)
}

fn validate_http_url(option: &'static str, value: &str) -> Result<(), CliError> {
    if value.starts_with("http://") {
        return Ok(());
    }

    Err(CliError::Usage(format!("{option} must start with http://")))
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
    println!("Elasticsearch over HTTP: measure only after healthcheck and index reset");
    println!("Surch in-process: current smoke path; label separately from HTTP runtimes");
    println!("operations: create index, bulk ingest, refresh, count, match, bool, fuzzy");
    println!("metrics: ingest duration, docs/s, min, p50, p95, p99, max, errors, top hit id");
    println!(
        "guardrail: no global ratio until Surch and Elasticsearch use symmetric HTTP engine paths"
    );
    println!("guardrail: does not start Elasticsearch, Docker, or a Surch server");
}

fn print_ban_http_bench_help() {
    println!("Surch BAN HTTP bench");
    println!("usage: surch-demo ban-http-bench [OPTIONS]");
    println!("executes a symmetric HTTP benchmark unless --dry-run is supplied");
    println!("options:");
    println!(
        "  --surch-url URL        Surch HTTP base URL (default: {DEFAULT_HTTP_BENCH_SURCH_URL})"
    );
    println!(
        "  --elasticsearch-url URL Elasticsearch HTTP base URL (default: {DEFAULT_HTTP_BENCH_ELASTICSEARCH_URL})"
    );
    println!("  --opensearch-url URL   legacy alias for --elasticsearch-url");
    println!("  --index NAME           target index name (default: {DEFAULT_HTTP_BENCH_INDEX})");
    println!("  --iterations N         requests per measured operation (default: {DEFAULT_BENCH_ITERATIONS})");
    println!(
        "  --dataset PATH         NDJSON dataset path (default: {DEFAULT_HTTP_BENCH_DATASET})"
    );
    println!("  --oracle PATH          oracle replay path (default: {BAN_TINY_ORACLE})");
    println!("  --warmup N             warmup requests per operation (default: 0)");
    println!("  --report PATH          optional report output path");
    println!(
        "  --timeout-seconds N    connect/read/write timeout in seconds (default: {DEFAULT_HTTP_TIMEOUT_SECONDS})"
    );
    println!("  --dry-run              print the benchmark plan without sending HTTP requests");
    println!("  -h, --help             print this help");
}

fn print_ban_http_bench_plan(plan: &BanHttpBenchPlan) -> Result<(), CliError> {
    let dataset = BulkDataset::load(&plan.dataset, &plan.index)?;
    let oracle = load_oracle_requests(&plan.oracle, &plan.index)?;

    println!("Surch BAN HTTP bench plan");
    println!("mode: dry-run");
    println!("dataset: {}", plan.dataset);
    println!("documents: {}", dataset.documents);
    println!("dataset_bytes: {}", dataset.bytes);
    println!("index: {}", plan.index);
    println!("iterations: {}", plan.iterations);
    println!("warmup: {}", plan.warmup);
    println!("timeout_seconds: {}", plan.timeout_seconds);
    println!("surch_url: {}", plan.surch_url);
    println!("elasticsearch_url: {}", plan.elasticsearch_url);
    println!("oracle: {}", plan.oracle);
    match &plan.report {
        Some(report) => println!("report: {report}"),
        None => println!("report: <none>"),
    }
    println!("operations:");
    println!("  - create_index");
    println!("  - bulk_ingest");
    println!("  - refresh");
    for request in &oracle {
        println!("  - {}", request.name);
    }
    println!("metrics: status, latency_us, docs, bytes, hits_total, top_hit_id, error_count");
    println!("guardrail: dry-run mode sends no HTTP requests");
    println!("guardrail: does not start Elasticsearch, Docker, or a Surch server");
    println!("guardrail: compare only on the same host/build only");
    Ok(())
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Http(String),
    Json(serde_json::Error),
    MissingHit(&'static str),
    UnexpectedResponse(&'static str),
    UnexpectedStatus { status: StatusCode, body: Value },
    OracleMismatch(String),
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
            Self::OracleMismatch(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CliError {}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_metric_to_json_serializes_latency_rates_and_hits() {
        let metric = OperationMetric {
            operation: "search_ban_tiny_by_label".to_owned(),
            status: 200,
            iterations: 5,
            samples: vec![
                Duration::from_micros(500),
                Duration::from_micros(100),
                Duration::from_micros(300),
                Duration::from_micros(200),
                Duration::from_micros(400),
            ],
            docs: Some(3),
            bytes: Some(900),
            error_count: 0,
            hits: Some(1),
            top_hit_id: Some("75101_0001_00001".to_owned()),
            server_took_ms: Some(7),
        };

        let value = metric.to_json();

        assert_eq!(value["operation"], json!("search_ban_tiny_by_label"));
        assert_eq!(value["status"], json!(200));
        assert_eq!(value["iterations"], json!(5));
        assert_eq!(value["docs"], json!(3));
        assert_eq!(value["bytes"], json!(900));
        assert_eq!(value["error_count"], json!(0));
        assert_eq!(value["hits"], json!(1));
        assert_eq!(value["top_hit_id"], json!("75101_0001_00001"));
        assert_eq!(value["server_took_ms"], json!(7));
        assert_eq!(
            value["latency_us"],
            json!({
                "min": 100,
                "p50": 300,
                "p95": 500,
                "p99": 500,
                "max": 500,
                "total": 1500
            })
        );
        assert_eq!(value["samples_us"], json!([500, 100, 300, 200, 400]));
        assert_eq!(value["docs_per_second"].as_f64().unwrap(), 2_000.0);
        assert_eq!(value["bytes_per_second"].as_f64().unwrap(), 600_000.0);
    }

    #[test]
    fn ban_http_bench_report_json_serializes_metadata_engines_and_guardrails() {
        let plan = BanHttpBenchPlan {
            surch_url: "http://127.0.0.1:7700".to_owned(),
            elasticsearch_url: "http://127.0.0.1:9200".to_owned(),
            index: "ban_ci".to_owned(),
            iterations: 2,
            dataset: "tests/fixtures/ban.ndjson".to_owned(),
            oracle: "tests/fixtures/ban_oracle.json".to_owned(),
            warmup: 1,
            report: Some("target/ban-http-bench.json".to_owned()),
            timeout_seconds: 15,
            dry_run: false,
        };
        let dataset = BulkDataset {
            body: String::new(),
            documents: 3,
            bytes: 941,
        };
        let reports = vec![EngineBenchReport {
            name: "surch".to_owned(),
            url: "http://127.0.0.1:7700".to_owned(),
            metrics: vec![OperationMetric {
                operation: "bulk_ingest".to_owned(),
                status: 200,
                iterations: 1,
                samples: vec![Duration::from_micros(400)],
                docs: Some(3),
                bytes: Some(941),
                error_count: 0,
                hits: None,
                top_hit_id: None,
                server_took_ms: Some(0),
            }],
        }];

        let value = ban_http_bench_report_json(&plan, &dataset, &reports);

        assert_eq!(value["schema"], json!("surch.bench.ban_http.v1"));
        assert_eq!(value["benchmark"], json!("ban-http-bench"));
        assert_eq!(value["dataset"], json!("tests/fixtures/ban.ndjson"));
        assert_eq!(value["dataset_bytes"], json!(941));
        assert_eq!(value["documents"], json!(3));
        assert_eq!(value["index"], json!("ban_ci"));
        assert_eq!(value["oracle"], json!("tests/fixtures/ban_oracle.json"));
        assert_eq!(value["iterations"], json!(2));
        assert_eq!(value["warmup"], json!(1));
        assert_eq!(value["timeout_seconds"], json!(15));
        assert_eq!(value["runtime"], json!("symmetric HTTP"));
        assert_eq!(
            value["http_client"],
            json!("persistent HTTP/1.1 keep-alive per engine")
        );
        assert_eq!(value["engines"][0]["name"], json!("surch"));
        assert_eq!(value["engines"][0]["url"], json!("http://127.0.0.1:7700"));
        assert_eq!(
            value["engines"][0]["metrics"][0]["operation"],
            json!("bulk_ingest")
        );
        assert_eq!(value["engines"][0]["metrics"][0]["docs"], json!(3));
        assert!(value["guardrails"]
            .as_array()
            .expect("guardrails should be an array")
            .contains(&json!("no global Surch/Elasticsearch ratio is emitted")));
    }

    #[test]
    fn validate_oracle_response_rejects_total_hit_mismatch() {
        let request = oracle_search_request();
        let response = HttpJsonResponse {
            status: 200,
            body: json!({
                "hits": {
                    "total": { "value": 2 },
                    "hits": [{ "_id": "75101_0001_00001" }]
                }
            }),
            elapsed: Duration::from_micros(100),
        };

        let error = validate_oracle_response(&request, &response).unwrap_err();

        match error {
            CliError::OracleMismatch(message) => {
                assert!(message.contains("$.hits.total.value"));
            }
            other => panic!("expected oracle mismatch, got {other:?}"),
        }
    }

    #[test]
    fn validate_oracle_response_rejects_top_hit_mismatch() {
        let request = oracle_search_request();
        let response = HttpJsonResponse {
            status: 200,
            body: json!({
                "hits": {
                    "total": { "value": 1 },
                    "hits": [{ "_id": "wrong_top_hit" }]
                }
            }),
            elapsed: Duration::from_micros(100),
        };

        let error = validate_oracle_response(&request, &response).unwrap_err();

        match error {
            CliError::OracleMismatch(message) => {
                assert!(message.contains("$.hits.hits.0._id"));
            }
            other => panic!("expected oracle mismatch, got {other:?}"),
        }
    }

    fn oracle_search_request() -> OracleRequest {
        OracleRequest {
            name: "search_ban_tiny_by_label".to_owned(),
            method: "POST".to_owned(),
            path: "/ban_ci/_search".to_owned(),
            body: None,
            expected_status: 200,
            expected_response: Some(json!({
                "hits": {
                    "total": { "value": 1 },
                    "hits": [{ "_id": "75101_0001_00001" }]
                }
            })),
            comparison: OracleComparison {
                ignored_paths: Vec::new(),
                score_tolerance: 0.0,
            },
        }
    }
}
