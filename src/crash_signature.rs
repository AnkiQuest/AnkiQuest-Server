//! The signature that groups crash reports of one failure, so the server keeps only a few of
//! each. The app computes the same signature on the device and sends it as
//! `ANKIQUEST_SIGNATURE`; the cases in `tests/fixtures/crash_signatures.json` hold both to it.
//!
//! The signature is the lowercase hex SHA-256 of these lines, joined by `\n`: the exception
//! class, its message without ASCII digits and trailing whitespace, and `class.method` of at
//! most the top [`FRAMES`] frames, without file or line. See `docs/mobile-api.md`.

use crate::web_util::hex;
use sha2::{Digest, Sha256};

/// Frames of the outer exception that are part of the signature.
const FRAMES: usize = 8;
/// Hex digits in a SHA-256.
const HEX_LEN: usize = 64;
/// What separates the exception class from its message in Java's `Throwable.toString`.
const MESSAGE_SEPARATOR: &str = ": ";
/// What starts a frame line, after its indentation.
const FRAME_PREFIX: &str = "at ";

/// The signature of a report with this `STACK_TRACE` (Java's `printStackTrace` text).
pub(crate) fn of_trace(trace: &str) -> String {
    hex(&Sha256::digest(canonical(trace).as_bytes()))
}

/// The lines the signature hashes.
fn canonical(trace: &str) -> String {
    let lines: Vec<&str> = trace
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();

    // The first line, and any lines before the first frame, are `class: message`.
    let header_len = lines
        .iter()
        .position(|line| is_frame(line))
        .unwrap_or(lines.len());
    let header = lines[..header_len].join("\n");
    let (class, message) = header
        .split_once(MESSAGE_SEPARATOR)
        .unwrap_or((&header, ""));
    let message: String = message.chars().filter(|c| !c.is_ascii_digit()).collect();

    // The frames right after it belong to the outer exception; causes and suppressed come later.
    let frames = lines[header_len..]
        .iter()
        .map_while(|line| frame(line))
        .take(FRAMES);
    [class.trim(), message.trim_end()]
        .into_iter()
        .chain(frames)
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_frame(line: &str) -> bool {
    frame(line).is_some()
}

/// `class.method` of a frame line such as `\tat a.B.c(B.kt:12)`.
fn frame(line: &str) -> Option<&str> {
    let call = line.trim_start().strip_prefix(FRAME_PREFIX)?;
    let end = call.find('(').unwrap_or(call.len());
    Some(call[..end].trim())
}

/// The signature the app sent, lowercased, if it is one: 64 hex digits.
pub(crate) fn parse_sent(sent: &str) -> Option<String> {
    let hex = sent.len() == HEX_LEN && sent.chars().all(|c| c.is_ascii_hexdigit());
    hex.then(|| sent.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Fixture {
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        name: String,
        stack_trace: String,
        canonical: String,
        signature: String,
    }

    fn cases() -> Vec<Case> {
        let text = include_str!("../tests/fixtures/crash_signatures.json");
        serde_json::from_str::<Fixture>(text).unwrap().cases
    }

    #[test]
    fn signatures_match_the_shared_cases() {
        for case in cases() {
            assert_eq!(
                canonical(&case.stack_trace),
                case.canonical,
                "{}",
                case.name
            );
            assert_eq!(of_trace(&case.stack_trace), case.signature, "{}", case.name);
        }
    }

    #[test]
    fn only_sixty_four_hex_digits_are_a_signature() {
        let good = "81f3b3d1ad826d834b1ba6df6df1afb43781a2bdba650fb6f8c8bb648c27242f";
        assert_eq!(parse_sent(good).as_deref(), Some(good));
        let upper = good.to_ascii_uppercase();
        assert_eq!(parse_sent(&upper).as_deref(), Some(good));
        for bad in [
            "",
            "abc",
            &good[1..],
            &format!("{good}0"),
            &format!(" {}", &good[1..]),
            &"g".repeat(64),
            "[removed]",
        ] {
            assert_eq!(parse_sent(bad), None, "{bad:?}");
        }
    }
}
