//! Prints the device log lines that name a Wallet card, as they arrive.
//!
//! Card discovery is the one thing that needs the phone to be doing something:
//! iOS logs a card's `.pkpass` path when Wallet renders that card, and there is no
//! other channel -- AFC cannot list the card directory, refuses `..`, and has no
//! symlink opcode, so the log is it.
//!
//!   cargo run --example log_scan -- [seconds] [dump-path]
//!
//! Then open Wallet on the phone and swipe through the cards. Anything printed as
//! a card hash is a handle the app can save, restore and flash against.
//!
//! With a dump path, every line that mentions Wallet or a pass is written there
//! as it arrives. A run that finds no card at all still says what the phone did
//! log, which is the only way to tell "the rule is wrong on this version" from
//! "the phone never rendered a card".
//!
//! Needs an unlocked, trusted iPhone on the cable. One session at a time: two at
//! once abort the process.

use std::fs::File;
use std::io::Write;
use std::time::{Duration, Instant};

use aircard_device::{hashes_in, list_devices, LogStream};

/// How often to say it is still alive. A silent multi-minute run is
/// indistinguishable from a stream that died on the first read.
const REPORT: Duration = Duration::from_secs(15);

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(120);
    let dump_path = std::env::args().nth(2);

    let udid = match std::env::var("AIRCARD_TEST_UDID") {
        Ok(udid) => udid,
        Err(_) => {
            let devices = list_devices().expect("device discovery");
            let Some(first) = devices.first() else {
                eprintln!("no iPhone found -- connect one and unlock it");
                std::process::exit(1);
            };
            first.udid.clone()
        }
    };

    println!("watching {udid} for {seconds}s");
    println!("open Wallet on the phone and swipe through every card, including the bank card");
    let mut dump = dump_path.map(|path| {
        let file = File::create(&path).expect("the dump file could not be made");
        println!("lines that mention Wallet or a pass go to {path}");
        file
    });

    let mut stream = match LogStream::open(&udid) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("could not open the device log stream: {error}");
            std::process::exit(1);
        }
    };

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut report = Instant::now();
    let mut lines = 0_u64;
    let mut kept = 0_u64;
    let mut found: Vec<String> = Vec::new();

    while Instant::now() < deadline {
        match stream.next_line() {
            Ok(None) => continue,
            Ok(Some(line)) => {
                lines += 1;
                if let Some(file) = dump.as_mut() {
                    if mentions_wallet(&line) {
                        kept += 1;
                        writeln!(file, "{line}").ok();
                    }
                }
                for hash in hashes_in(&line) {
                    if !found.contains(&hash) {
                        println!("card hash: {hash}");
                        found.push(hash);
                    }
                }
                if report.elapsed() >= REPORT {
                    println!(
                        "  .. {lines} lines read, {kept} kept, {} card hash(es)",
                        found.len()
                    );
                    report = Instant::now();
                }
            }
            Err(error) => {
                eprintln!("stream ended: {error}");
                break;
            }
        }
    }

    if let Some(file) = dump.as_mut() {
        file.flush().ok();
    }
    println!(
        "{lines} log lines read, {kept} kept, {} card hash(es) found",
        found.len()
    );
}

/// Broad on purpose: this is the net that has to catch a line on a version whose
/// spelling nobody has seen yet.
fn mentions_wallet(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("pkpass") || lower.contains("wallet") || lower.contains("passes")
}
