#![forbid(unsafe_code)]

use std::{env, fs, io, path::Path};

use opensearch_oracle::ban::{ban_records_to_bulk_ndjson, parse_ban_csv};

const HELP: &str = "\
ban-to-ndjson

Convert an uncompressed BAN CSV extract to OpenSearch bulk NDJSON.

Usage:
  cargo run -p opensearch-oracle --bin ban-to-ndjson -- \\
    --input target/ban-bench/ban-paris-25000.csv \\
    --output target/ban-bench/ban-paris-25000.ndjson \\
    --index ban_paris_25000 \\
    --limit 25000
";

#[derive(Debug)]
struct Config {
    input: String,
    output: String,
    index: String,
    limit: Option<usize>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(config) = Config::parse(env::args().skip(1)).map_err(|message| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("error: {message}"))
    })?
    else {
        print!("{HELP}");
        return Ok(());
    };

    let csv = fs::read_to_string(&config.input)?;
    let mut records = parse_ban_csv(&csv)?;
    if let Some(limit) = config.limit {
        records.truncate(limit);
    }
    let ndjson = ban_records_to_bulk_ndjson(&config.index, &records)?;

    if let Some(parent) = Path::new(&config.output).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(&config.output, ndjson.as_bytes())?;

    println!("input: {}", config.input);
    println!("output: {}", config.output);
    println!("index: {}", config.index);
    println!("documents: {}", records.len());
    println!("bytes: {}", ndjson.len());

    Ok(())
}

impl Config {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut input = None;
        let mut output = None;
        let mut index = None;
        let mut limit = None;
        let mut args = args.into_iter();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => return Ok(None),
                "--input" => input = Some(required_value(&mut args, "--input")?),
                "--output" => output = Some(required_value(&mut args, "--output")?),
                "--index" => index = Some(required_value(&mut args, "--index")?),
                "--limit" => {
                    let value = required_value(&mut args, "--limit")?;
                    let parsed = value
                        .parse::<usize>()
                        .map_err(|_| "--limit must be a positive integer".to_owned())?;
                    if parsed == 0 {
                        return Err("--limit must be greater than zero".to_owned());
                    }
                    limit = Some(parsed);
                }
                unknown => return Err(format!("unknown option `{unknown}`")),
            }
        }

        Ok(Some(Self {
            input: non_empty(input, "--input")?,
            output: non_empty(output, "--output")?,
            index: non_empty(index, "--index")?,
            limit,
        }))
    }
}

fn required_value(
    args: &mut impl Iterator<Item = String>,
    option: &'static str,
) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value after {option}"))
}

fn non_empty(value: Option<String>, option: &'static str) -> Result<String, String> {
    let value = value.ok_or_else(|| format!("missing required option {option}"))?;
    if value.trim().is_empty() {
        Err(format!("{option} must not be empty"))
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn parses_required_options() {
        let config = Config::parse([
            "--input".to_owned(),
            "in.csv".to_owned(),
            "--output".to_owned(),
            "out.ndjson".to_owned(),
            "--index".to_owned(),
            "ban_paris_25000".to_owned(),
            "--limit".to_owned(),
            "25000".to_owned(),
        ])
        .expect("config should parse")
        .expect("help should not be requested");

        assert_eq!(config.input, "in.csv");
        assert_eq!(config.output, "out.ndjson");
        assert_eq!(config.index, "ban_paris_25000");
        assert_eq!(config.limit, Some(25_000));
    }

    #[test]
    fn rejects_zero_limit() {
        let error = Config::parse([
            "--input".to_owned(),
            "in.csv".to_owned(),
            "--output".to_owned(),
            "out.ndjson".to_owned(),
            "--index".to_owned(),
            "ban_paris_25000".to_owned(),
            "--limit".to_owned(),
            "0".to_owned(),
        ])
        .expect_err("zero limit should be rejected");

        assert_eq!(error, "--limit must be greater than zero");
    }
}
