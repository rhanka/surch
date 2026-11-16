#![deny(unsafe_code)]

// Track A wp-a-perf-followups.md Lot 1.7: use jemalloc on Linux so
// the heap returns freed pages to the OS aggressively (paired with
// the MALLOC_CONF=dirty_decay_ms:0,muzzy_decay_ms:0 in the runtime
// Dockerfile). Eliminates the ~700 MiB of phantom Surch RSS that
// the glibc default left mapped after `_refresh` finalized the
// PostingsBuilder.
#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::{env, error::Error};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 7700;
const HELP: &str = "\
surch-api

Run the local Surch HTTP API server.

Environment:
  SURCH_HOST      Bind host, defaults to 127.0.0.1
  SURCH_PORT      Bind port, defaults to 7700
  SURCH_DATA_DIR  Persist indices under this directory and reopen them at
                  start (unset: in-memory only). See docs/ops/persistence.md
";

#[derive(Debug, Eq, PartialEq)]
enum RunMode {
    Serve,
    Help,
}

#[derive(Debug, Eq, PartialEq)]
struct ServerConfig {
    host: String,
    port: u16,
}

impl ServerConfig {
    fn bind_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn server_config_from_env<I, K, V>(vars: I) -> Result<ServerConfig, std::num::ParseIntError>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut host = DEFAULT_HOST.to_owned();
    let mut port = DEFAULT_PORT;

    for (key, value) in vars {
        match key.as_ref() {
            "SURCH_HOST" => host = value.as_ref().to_owned(),
            "SURCH_PORT" => port = value.as_ref().parse()?,
            _ => {}
        }
    }

    Ok(ServerConfig { host, port })
}

/// MVP L6: `SURCH_DATA_DIR` when set to a non-empty path.
fn data_dir_from_env<I, K, V>(vars: I) -> Option<std::path::PathBuf>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    vars.into_iter()
        .find(|(key, _)| key.as_ref() == surch_api::persist::DATA_DIR_ENV)
        .map(|(_, value)| value.as_ref().trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

fn run_mode_from_args<I, S>(args: I) -> RunMode
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if args
        .into_iter()
        .skip(1)
        .any(|arg| matches!(arg.as_ref(), "-h" | "--help"))
    {
        RunMode::Help
    } else {
        RunMode::Serve
    }
}

/// Default idle interval of the small-index auto-commit (ms).
const AUTO_COMMIT_INTERVAL_MS_DEFAULT: u64 = 1_000;
/// Default size bound of an auto-committed index (live documents).
const AUTO_COMMIT_MAX_DOCS_DEFAULT: u32 = 100_000;

/// MVP L6: with a data directory, commit small indices left with
/// uncommitted writes once they stay idle for one interval, so a single
/// acknowledged write (matchID's restore marker) survives a restart.
/// `SURCH_AUTO_COMMIT_INTERVAL_MS=0` disables it;
/// `SURCH_AUTO_COMMIT_MAX_DOCS` bounds the index size it applies to.
fn spawn_idle_auto_commit(app: &surch_api::state::AppState) {
    if app.data_dir().is_none() {
        return;
    }
    let interval_ms = env::var("SURCH_AUTO_COMMIT_INTERVAL_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(AUTO_COMMIT_INTERVAL_MS_DEFAULT);
    if interval_ms == 0 {
        return;
    }
    let max_docs = env::var("SURCH_AUTO_COMMIT_MAX_DOCS")
        .ok()
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(AUTO_COMMIT_MAX_DOCS_DEFAULT);
    let app = app.clone();
    tokio::spawn(async move {
        let mut previous = std::collections::BTreeMap::new();
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(interval_ms));
        loop {
            ticker.tick().await;
            let app = app.clone();
            let carried = std::mem::take(&mut previous);
            match tokio::task::spawn_blocking(move || {
                let mut carried = carried;
                app.auto_commit_idle_small_indices(&mut carried, max_docs);
                carried
            })
            .await
            {
                Ok(carried) => previous = carried,
                Err(error) => eprintln!("surch-api: idle auto-commit tick failed: {error}"),
            }
        }
    });
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if run_mode_from_args(env::args()) == RunMode::Help {
        print!("{HELP}");
        return Ok(());
    }

    surch_api::telemetry::init_telemetry();

    let config = server_config_from_env(env::vars())?;
    let bind_addr = config.bind_addr();
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;

    // MVP L6 (`docs/ops/persistence.md`): opt-in data directory. Unset keeps
    // the historical in-memory engine bit-for-bit. Set, every committed
    // index is reopened from its manifest before the listener accepts a
    // request; any corrupt or version-mismatched file aborts the start.
    let shared = match data_dir_from_env(env::vars()) {
        Some(root) => {
            let app = surch_api::state::AppState::open_with_data_dir(&root).map_err(|error| {
                eprintln!(
                    "surch-api: cannot open {} {}: {error}",
                    surch_api::persist::DATA_DIR_ENV,
                    root.display()
                );
                error
            })?;
            // Spill tempfiles on the data directory's filesystem: sealing a
            // segment then hard-links it into its index directory.
            surch_index::postings::set_spill_dir(Some(root.join("tmp")));
            eprintln!(
                "surch-api: data directory {} opened ({} indices)",
                root.display(),
                app.index_names().len()
            );
            surch_api::AppRouterState {
                app,
                ..Default::default()
            }
        }
        None => surch_api::AppRouterState::default(),
    };
    spawn_idle_auto_commit(&shared.app);
    let _slm_scheduler = surch_api::slm::scheduler::spawn(
        surch_api::slm::SchedulerConfig::default(),
        shared.slm_policies.clone(),
        shared.snapshot_repositories.clone(),
        shared.app.clone(),
    );

    eprintln!("surch-api listening on http://{bind_addr}");
    axum::serve(listener, surch_api::app_router_with_state(shared)).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{data_dir_from_env, run_mode_from_args, server_config_from_env, RunMode};

    #[test]
    fn data_dir_is_opt_in() {
        assert_eq!(data_dir_from_env(std::iter::empty::<(&str, &str)>()), None);
        assert_eq!(data_dir_from_env([("SURCH_DATA_DIR", "  ")]), None);
        assert_eq!(
            data_dir_from_env([("SURCH_DATA_DIR", "/var/lib/surch")]),
            Some(std::path::PathBuf::from("/var/lib/surch"))
        );
    }

    #[test]
    fn server_config_uses_default_loopback_address() {
        let config = server_config_from_env(std::iter::empty::<(&str, &str)>())
            .expect("default server config should be valid");

        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 7700);
        assert_eq!(config.bind_addr(), "127.0.0.1:7700");
    }

    #[test]
    fn server_config_uses_surch_host_and_port_env() {
        let config = server_config_from_env([("SURCH_HOST", "0.0.0.0"), ("SURCH_PORT", "17700")])
            .expect("configured server config should be valid");

        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 17700);
        assert_eq!(config.bind_addr(), "0.0.0.0:17700");
    }

    #[test]
    fn cli_help_request_is_detected_before_server_start() {
        assert_eq!(run_mode_from_args(["surch-api", "--help"]), RunMode::Help);
        assert_eq!(run_mode_from_args(["surch-api", "-h"]), RunMode::Help);
        assert_eq!(run_mode_from_args(["surch-api"]), RunMode::Serve);
    }
}
