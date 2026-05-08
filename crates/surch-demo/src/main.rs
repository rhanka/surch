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
const DEFAULT_BENCH_ITERATIONS: usize = 1_000;

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
    println!("documents: 3");
    println!("count: {count}");
    println!("match label: {match_label}");
    println!("bool address: {bool_address}");
    println!("fuzzy label: {fuzzy_label}");
    println!("oracle: tests/opensearch_compat/oracle/replays/ban_tiny_search.json");

    Ok(())
}

async fn run_ban_bench(iterations: usize) -> Result<(), CliError> {
    let load_elapsed = bench_load(iterations).await?;
    let router = load_ban_tiny().await?;
    let count_elapsed = bench_count(router.clone(), iterations).await?;
    let match_elapsed = bench_search(
        router.clone(),
        iterations,
        json!({
            "query": {
                "match": {
                    "label": "Rue de Rivoli"
                }
            }
        }),
    )
    .await?;
    let bool_elapsed = bench_search(
        router.clone(),
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
    )
    .await?;
    let fuzzy_elapsed = bench_search(
        router,
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
    )
    .await?;

    println!("Surch BAN bench");
    println!("dataset: ban_tiny");
    println!("documents: 3");
    println!("iterations: {iterations}");
    print_metric("load_ban_tiny", iterations, load_elapsed);
    print_metric("count_match_all", iterations, count_elapsed);
    print_metric("search_match_label", iterations, match_elapsed);
    print_metric("search_bool_address", iterations, bool_elapsed);
    print_metric("search_fuzzy_label", iterations, fuzzy_elapsed);
    println!("note: local in-memory router benchmark; compare only on the same host/build.");

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
    let response = execute_json(router, Method::POST, "/ban_tiny/_count", None).await?;
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

async fn bench_load(iterations: usize) -> Result<Duration, CliError> {
    let start = Instant::now();
    for _ in 0..iterations {
        let _router = load_ban_tiny().await?;
    }

    Ok(start.elapsed())
}

async fn bench_count(router: Router, iterations: usize) -> Result<Duration, CliError> {
    let start = Instant::now();
    for _ in 0..iterations {
        let count = count_documents(router.clone()).await?;
        if count != 3 {
            return Err(CliError::UnexpectedResponse(
                "count benchmark expected 3 docs",
            ));
        }
    }

    Ok(start.elapsed())
}

async fn bench_search(
    router: Router,
    iterations: usize,
    body: Value,
) -> Result<Duration, CliError> {
    let start = Instant::now();
    for _ in 0..iterations {
        let response = execute_json(
            router.clone(),
            Method::POST,
            "/ban_tiny/_search",
            Some(body.to_string()),
        )
        .await?;
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
    }

    Ok(start.elapsed())
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

fn print_metric(label: &str, iterations: usize, elapsed: Duration) {
    let seconds = elapsed.as_secs_f64().max(f64::EPSILON);
    let micros_per_op = seconds * 1_000_000.0 / iterations as f64;
    let ops_per_second = iterations as f64 / seconds;
    println!("{label}: {micros_per_op:.2} us/op, {ops_per_second:.2} ops/s");
}

fn print_help() {
    println!("Surch demo commands");
    println!("  ban-poc");
    println!("  ban-bench [--iterations N]");
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
