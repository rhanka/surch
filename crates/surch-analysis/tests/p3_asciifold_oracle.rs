use std::{
    env, fs,
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use surch_analysis::{Analyzer, NormAnalyzer};

#[test]
fn awk_p3_asciifolding_matches_the_rust_norm_analyzer_on_accepted_terms() {
    let terms = [
        "ÉVRARD",
        "ÆGIR",
        "ŒUVRE",
        "Straße",
        "Straẞe",
        "ÞÓR",
        "ŁUKASZ",
        "ĐURIĆ",
        "ØDEGAARD",
        "ĦAKON",
        "ƏLİSE",
        "MiXeD42",
    ];
    let input = env::temp_dir().join(format!(
        "surch-p3-asciifold-{}-{}",
        process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("horloge système après Unix epoch")
            .as_nanos()
    ));
    fs::write(&input, format!("{}\n", terms.join("\n"))).expect("écriture des termes oracle");

    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("deploy/bench-local/p2-asciifold.awk");
    let output = Command::new("awk")
        .args(["-v", "p2_asciifold_emit=1", "-f"])
        .arg(&script)
        .arg(&input)
        .output()
        .expect("awk doit être disponible dans le runner CI");
    let _ = fs::remove_file(&input);
    assert!(
        output.status.success(),
        "oracle AWK en échec: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let awk_terms: Vec<&str> = std::str::from_utf8(&output.stdout)
        .expect("sortie AWK UTF-8")
        .lines()
        .collect();

    let analyzer = NormAnalyzer;
    let rust_terms: Vec<String> = terms
        .iter()
        .map(|term| {
            let tokens = analyzer.token_stream(term);
            assert_eq!(tokens.len(), 1, "terme représentatif mono-token: {term}");
            tokens[0].term.clone()
        })
        .collect();

    assert_eq!(awk_terms, rust_terms);
    assert!(rust_terms.iter().all(|term| {
        !term.is_empty()
            && term
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
    }));
}
