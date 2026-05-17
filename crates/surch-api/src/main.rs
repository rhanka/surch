#![forbid(unsafe_code)]

use std::{env, error::Error};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 7700;
const HELP: &str = "\
surch-api

Run the local Surch HTTP API server.

Environment:
  SURCH_HOST  Bind host, defaults to 127.0.0.1
  SURCH_PORT  Bind port, defaults to 7700
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

    let shared = surch_api::AppRouterState::default();
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
    use super::{run_mode_from_args, server_config_from_env, RunMode};

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
