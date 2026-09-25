//! Prints the device log lines that name a Wallet card, as they arrive.
//!
//! Card discovery is the one thing that needs the phone to be doing something:
//! iOS logs a card's `.pkpass` path when Wallet renders that card, and there is no
//! other channel -- AFC cannot list the card directory, refuses `..`, and has no
//! symlink opcode, so the log is it.
//!
//!   cargo run --example log_scan -- [seconds]
//!
//! Then open Wallet on the phone and swipe through the cards. Anything printed as
//! a card hash is a handle the app can save, restore and flash against.
//!
//! Needs an unlocked, trusted iPhone on the cable. One session at a time: two at
//! once abort the process.

use std::time::{Duration, Instant};

use aircard_device::{hashes_in, list_devices, LogStream};

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(120);

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

    let mut stream = match LogStream::open(&udid) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("could not open the device log stream: {error}");
            std::process::exit(1);
        }
    };

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut lines = 0_u64;
    let mut found: Vec<String> = Vec::new();

    while Instant::now() < deadline {
        match stream.next_line() {
            Ok(None) => continue,
            Ok(Some(line)) => {
                lines += 1;
                for hash in hashes_in(&line) {
                    if !found.contains(&hash) {
                        println!("card hash: {hash}");
                        found.push(hash);
                    }
                }
            }
            Err(error) => {
                eprintln!("stream ended: {error}");
                break;
            }
        }
    }

    println!(
        "{lines} log lines read, {} card hash(es) found",
        found.len()
    );
}
