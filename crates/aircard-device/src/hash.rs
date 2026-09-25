//! Finding a card's hash in the device log.
//!
//! AirCard cannot list `/var/mobile/Library/Passes/Cards`. AFC is rooted at
//! `Media`, `..` is refused (status `INVALID_ARG`, measured on iOS 27), and AFC's
//! `MAKE_LINK` is not supported there at all, so no symlink can be planted either.
//! The only channel left is the device log: iOS logs a card's `.pkpass` path when
//! Wallet renders that card. That is why discovering a new card means opening
//! Wallet and showing the card once -- flashing itself needs none of that, because
//! a known hash is enough to build the path.

use std::sync::OnceLock;

use regex::Regex;

/// Hashes that show up in Wallet logs but are not cards.
pub const PLACEHOLDER_HASHES: [&str; 3] = [
    "M6nDwZrkYbFlsodLgCbvyFZQ1cc=",
    "kJL-D0rr-SZhbj2c8nK-OQ9hCMY=",
    "hwAtAmHKYwsQrJbT5cTNDsaxVME=",
];

/// Processes whose log lines can carry a card path.
const SUBSYSTEMS: [&str; 7] = [
    "passd",
    "passbook",
    "passkit",
    "stockholm",
    "nanopassd",
    "wallet",
    "/cards/",
];

/// Words that put a line in a card context. A Wallet process logging about a
/// network connection is not a card, so both lists have to match.
const CONTEXT: [&str; 10] = [
    "card",
    "pass",
    "payment",
    "pkpass",
    "uniqueid",
    "identifier",
    "face",
    "cache",
    "stockholm",
    "/cards/",
];

/// Characters a card hash is made of.
const HASH_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/_-";

/// A card path, an `X.pkpass` leaf, and a bare hash.
///
/// The bare-hash pattern carries no lookaround: the `regex` crate does not support
/// it, so that boundary check is done by hand in [`hashes_in`].
fn patterns() -> &'static [Regex; 3] {
    static PATTERNS: OnceLock<[Regex; 3]> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            Regex::new(
                r#"/(?:Cards|Passes/Cards)/([-A-Za-z0-9_+=]{20,44})(?:\.pkpass|\.cache|\.pkcache|/|\s|"|'|\)|,|$)"#,
            )
            .expect("card path pattern"),
            Regex::new(r"/([-A-Za-z0-9_+=]{20,44})\.(?:pkpass|cache|pkcache)")
                .expect("card leaf pattern"),
            Regex::new(r"[A-Za-z0-9+/_-]{27}=").expect("bare hash pattern"),
        ]
    })
}

/// Could this line be talking about a card at all?
pub fn is_wallet_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    SUBSYSTEMS.iter().any(|word| lower.contains(word))
        && CONTEXT.iter().any(|word| lower.contains(word))
}

/// True for something that cannot be a card hash: a UUID, or a known placeholder.
pub fn is_rejected(candidate: &str) -> bool {
    (candidate.len() == 36 && candidate.contains('-')) || PLACEHOLDER_HASHES.contains(&candidate)
}

/// True when the match stands on its own rather than being part of a longer run.
fn is_word_bounded(line: &str, start: usize, end: usize) -> bool {
    let before = line[..start].chars().next_back();
    let after = line[end..].chars().next();
    !before.is_some_and(|c| HASH_CHARS.contains(c))
        && !after.is_some_and(|c| HASH_CHARS.contains(c))
}

fn push_unique(found: &mut Vec<String>, candidate: &str) {
    if is_rejected(candidate) || found.iter().any(|seen| seen == candidate) {
        return;
    }
    found.push(candidate.to_owned());
}

/// Every card hash this line mentions, in the order it mentions them.
pub fn hashes_in(line: &str) -> Vec<String> {
    if !is_wallet_line(line) {
        return Vec::new();
    }
    let [path_pattern, leaf_pattern, bare_pattern] = patterns();
    let mut found: Vec<String> = Vec::new();

    for capture in path_pattern
        .captures_iter(line)
        .chain(leaf_pattern.captures_iter(line))
    {
        push_unique(&mut found, &capture[1]);
    }
    for hit in bare_pattern.find_iter(line) {
        if is_word_bounded(line, hit.start(), hit.end()) {
            push_unique(&mut found, hit.as_str());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "ql2MjCQ86Xc6xp7nPee-xDWNSRI=";

    fn card_line(card: &str) -> String {
        format!(
            "passd(CorePass): resource lookup for \
             /var/mobile/Library/Passes/Cards/{card}.pkpass/cardBackgroundCombined@3x.png"
        )
    }

    #[test]
    fn a_card_path_in_a_wallet_line_is_found() {
        assert_eq!(hashes_in(&card_line(REAL)), vec![REAL.to_owned()]);
    }

    #[test]
    fn the_leaf_form_is_found_too() {
        let line = format!("passkit(Wallet): opening /var/mobile/Media/x/{REAL}.pkpass for card");
        assert_eq!(hashes_in(&line), vec![REAL.to_owned()]);
    }

    #[test]
    fn a_line_with_no_wallet_words_is_ignored() {
        let line = format!(
            "kernel(AppleCentauri): tcp_connection_summary process: lockdownd flow {}",
            REAL
        );
        assert!(hashes_in(&line).is_empty());
    }

    #[test]
    fn a_card_path_matches_even_from_another_process() {
        // `/cards/` sits in both lists, so a line naming a card path is kept even
        // when the process is not one of the Wallet daemons. That is what the
        // ported filter did, and it is harmless: extraction still needs a
        // card-shaped token.
        let line = format!("someprocess: opened /var/mobile/Library/Passes/Cards/{REAL}.pkpass");
        assert_eq!(hashes_in(&line), vec![REAL.to_owned()]);
    }

    #[test]
    fn a_wallet_line_without_card_context_is_ignored() {
        let line = format!("wallet(network): connection to {REAL} established");
        assert!(hashes_in(&line).is_empty());
    }

    #[test]
    fn a_uuid_is_not_a_card() {
        let line = format!(
            "passd(card): uniqueIdentifier \
             FEEDEEEE-DDDD-CCCC-BBBB-0000000001F5 /var/mobile/Library/Passes/Cards/{REAL}.pkpass"
        );
        assert_eq!(hashes_in(&line), vec![REAL.to_owned()]);
    }

    #[test]
    fn a_placeholder_hash_is_never_reported() {
        assert!(hashes_in(&card_line(PLACEHOLDER_HASHES[0])).is_empty());
    }

    #[test]
    fn the_real_hash_shapes_survive_intact() {
        for card in [
            "ql2MjCQ86Xc6xp7nPee-xDWNSRI=",
            "2Do5+0cj+vG1zMfmFbPt0D4GPKQ=",
            "HuPplYeKY753l5EqMBiwHbP3aos=",
        ] {
            assert_eq!(hashes_in(&card_line(card)), vec![card.to_owned()]);
            assert!(!is_rejected(card), "{card} must be usable");
        }
    }

    #[test]
    fn the_same_hash_twice_is_reported_once() {
        let line = card_line(REAL) + " " + &card_line(REAL);
        assert_eq!(hashes_in(&line), vec![REAL.to_owned()]);
    }

    #[test]
    fn a_bare_hash_is_only_taken_when_it_stands_alone() {
        let glued = format!("passd(card): blob AAAA{REAL}BBBB done");
        assert!(
            hashes_in(&glued).is_empty(),
            "a hash inside a longer token is not a hash"
        );

        let alone = format!("passd(card): opened {REAL} ok");
        assert_eq!(hashes_in(&alone), vec![REAL.to_owned()]);
    }

    #[test]
    fn matching_is_case_insensitive_about_the_process_name() {
        let line = format!("PassD(CorePass): card cache miss {}", card_line(REAL));
        assert!(hashes_in(&line).contains(&REAL.to_owned()));
    }
}
