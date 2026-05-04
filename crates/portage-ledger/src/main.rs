#![forbid(unsafe_code)]

use std::path::PathBuf;

use portage_ledger::{check_language_policy, validate_ticket_path};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        usage_and_exit();
    };
    let Some(path) = args.next() else {
        usage_and_exit();
    };
    if args.next().is_some() {
        usage_and_exit();
    }

    let path = PathBuf::from(path);
    let result = match command.as_str() {
        "validate" => validate_ticket_path(&path).map(|count| {
            println!(
                "validated {count} ticket{}",
                if count == 1 { "" } else { "s" }
            );
        }),
        "language-policy" => check_language_policy(&path).map(|()| {
            println!("language policy ok");
        }),
        _ => {
            usage_and_exit();
        }
    };

    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn usage_and_exit() -> ! {
    eprintln!("usage: portage-ledger <validate|language-policy> <path>");
    std::process::exit(2);
}
