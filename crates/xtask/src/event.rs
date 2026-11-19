//! Prints the commit a CI run compares against: the pull request's base,
//! the merge group's base, or the commit before a push. Other events have
//! none; then it prints the checked commit, so a comparison finds no
//! change. Reads the variables GitHub Actions sets for every step.

use std::env;
use std::fs;
use std::process::ExitCode;

use serde_json::Value;

/// The event that started the run.
const EVENT_NAME: &str = "GITHUB_EVENT_NAME";

/// The file holding the event's payload.
const EVENT_PATH: &str = "GITHUB_EVENT_PATH";

/// The commit the run checks.
const SHA: &str = "GITHUB_SHA";

/// The `before` of a push that created its branch.
const NO_COMMIT: &str = "0000000000000000000000000000000000000000";

/// The events with a base commit.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Event {
    PullRequest,
    Push,
    MergeGroup,
    Other,
}

impl Event {
    /// The event for a `GITHUB_EVENT_NAME` value.
    fn parse(name: &str) -> Self {
        match name {
            "pull_request" | "pull_request_target" => Self::PullRequest,
            "push" => Self::Push,
            "merge_group" => Self::MergeGroup,
            _ => Self::Other,
        }
    }
}

/// Prints the base commit, or the checked commit when there is none.
pub fn run(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("usage: xtask event-base");
        return ExitCode::FAILURE;
    }
    let Ok(head) = env::var(SHA) else {
        eprintln!("{SHA} is not set");
        return ExitCode::FAILURE;
    };

    // No event file (a local run) means no base.
    let event = Event::parse(&env::var(EVENT_NAME).unwrap_or_default());
    let payload = env::var(EVENT_PATH)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    println!("{}", base(event, &payload).unwrap_or(&head));
    ExitCode::SUCCESS
}

/// The base commit `payload` names for `event`, if any.
fn base(event: Event, payload: &Value) -> Option<&str> {
    let sha = match event {
        Event::PullRequest => &payload["pull_request"]["base"]["sha"],
        Event::Push => &payload["before"],
        Event::MergeGroup => &payload["merge_group"]["base_sha"],
        Event::Other => return None,
    };
    sha.as_str().filter(|sha| *sha != NO_COMMIT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pull_request_uses_its_base() {
        let payload = json!({"pull_request": {"base": {"sha": "b"}, "head": {"sha": "h"}}});
        assert_eq!(Some("b"), base(Event::parse("pull_request"), &payload));
    }

    #[test]
    fn push_uses_the_previous_commit() {
        let payload = json!({"before": "b", "after": "h"});
        assert_eq!(Some("b"), base(Event::parse("push"), &payload));

        // A push that created the branch has no previous commit.
        let created = json!({"before": NO_COMMIT, "after": "h"});
        assert_eq!(None, base(Event::parse("push"), &created));
    }

    #[test]
    fn merge_group_uses_its_base() {
        // AnkiQuest/AnkiQuest-Server#58: the queue has no pull_request or
        // before; comparing with the queued commit checked nothing.
        let payload = json!({"merge_group": {"base_sha": "b", "head_sha": "h"}});
        assert_eq!(Some("b"), base(Event::parse("merge_group"), &payload));
    }

    #[test]
    fn other_events_have_no_base() {
        let payload = json!({"before": "b"});
        assert_eq!(None, base(Event::parse("workflow_dispatch"), &payload));
    }
}
