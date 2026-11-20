//! Keeps pull request code off AnkiQuest's self-hosted runners. Their
//! runner group admits every workflow and ref (float3/nixos
//! `nixos/runner-policy.nix`), so only a job's `runs-on` decides where
//! it runs. Every `runs-on` in `.github/workflows/` must be one of:
//!
//! - a GitHub-hosted label (`ubuntu-*`, `macos-*`, `windows-*`);
//! - `${{ matrix.<key> }}`, with every value of `<key>` in the file a
//!   GitHub-hosted label;
//! - the routing expression: self-hosted labels for merge groups and
//!   pushes to the default branch, a GitHub-hosted label for every other
//!   event, `pull_request` and `pull_request_target` included.
//!
//! Anything else fails, including forms this reader does not parse (a
//! `runs-on` block, runner groups, variables, inputs). Reusable
//! workflows must be local, so their jobs are checked here too, or
//! AnkiQuest-Dev's shared ones on its default branch, `main`, which its
//! `runner-routing` check covers there.

use std::fs;
use std::path::Path;

/// The workflow directory, relative to the repository root.
const WORKFLOWS: &str = ".github/workflows";

/// The events that may run on self-hosted runners: merge groups and
/// pushes to the default branch.
const SELF_HOSTED_EVENTS: &str = "(github.event_name == 'merge_group' || \
(github.event_name == 'push' && github.ref == \
format('refs/heads/{0}', github.event.repository.default_branch)))";

/// AnkiQuest-Dev's shared reusable workflows, and the only ref of them
/// allowed: its default branch.
const SHARED: &str = "AnkiQuest/AnkiQuest-Dev/.github/workflows/";
const SHARED_REF: &str = "@main";

/// Prefixes of GitHub-hosted runner labels.
const HOSTED: [&str; 3] = ["ubuntu-", "macos-", "windows-"];

/// Whether `label` is a GitHub-hosted runner label.
fn hosted(label: &str) -> bool {
    HOSTED.iter().any(|prefix| {
        label.strip_prefix(prefix).is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        })
    })
}

/// Whether `uses` names one of AnkiQuest-Dev's shared workflows on main.
fn shared(uses: &str) -> bool {
    let Some(file) = uses
        .strip_prefix(SHARED)
        .and_then(|rest| rest.strip_suffix(SHARED_REF))
    else {
        return false;
    };
    let name = file
        .strip_suffix(".yml")
        .or_else(|| file.strip_suffix(".yaml"))
        .unwrap_or_default();
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// A line's value after `key:`, without a trailing comment.
fn value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let line = line.trim_start();
    let line = line.strip_prefix("- ").unwrap_or(line).trim_start();
    let rest = line.strip_prefix(key)?.strip_prefix(':')?;
    let rest = rest.split(" #").next().unwrap_or(rest);
    Some(rest.trim())
}

/// The fallback label of the routing expression, if `runs_on` is it.
fn routed(runs_on: &str) -> Option<&str> {
    let inner = runs_on.strip_prefix("${{ ")?.strip_suffix(" }}")?;
    let rest = inner
        .strip_prefix(SELF_HOSTED_EVENTS)?
        .strip_prefix(" && fromJSON('")?;
    let (labels, fallback) = rest.split_once("') || '")?;
    if labels.contains('\'') {
        return None;
    }
    fallback.strip_suffix('\'')
}

/// The matrix key `runs_on` reads, if it is `${{ matrix.<key> }}`.
fn matrix_key(runs_on: &str) -> Option<&str> {
    let key = runs_on.strip_prefix("${{ matrix.")?.strip_suffix(" }}")?;
    let valid = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then_some(key)
}

/// Whether every value of the matrix key `key` in `text` is hosted.
fn hosted_matrix(text: &str, key: &str) -> bool {
    let values: Vec<&str> = text
        .lines()
        .filter_map(|line| value(line, key))
        .flat_map(|value| {
            let list = value.strip_prefix('[').and_then(|v| v.strip_suffix(']'));
            list.map_or(vec![value], |list| list.split(',').collect())
        })
        .map(|label| label.trim().trim_matches(['\'', '"']))
        .collect();
    !values.is_empty() && values.iter().all(|label| hosted(label))
}

/// The problem with one `runs-on` value, if any.
fn runs_on_problem(text: &str, runs_on: &str) -> Option<String> {
    if hosted(runs_on) {
        return None;
    }
    if let Some(fallback) = routed(runs_on) {
        if hosted(fallback) {
            return None;
        }
        return Some(format!("fallback `{fallback}` is not GitHub-hosted"));
    }
    if let Some(key) = matrix_key(runs_on) {
        if hosted_matrix(text, key) {
            return None;
        }
        return Some(format!(
            "matrix.{key} has values that are not GitHub-hosted labels"
        ));
    }
    if runs_on.is_empty() {
        return Some("runs-on must be one line".to_string());
    }
    Some(format!(
        "`{runs_on}` is not a GitHub-hosted label or the routing expression"
    ))
}

/// The problems in one workflow, as `line: problem`.
fn problems(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;

        // Where the job runs.
        if let Some(runs_on) = value(line, "runs-on")
            && let Some(problem) = runs_on_problem(text, runs_on)
        {
            found.push(format!("{number}: {problem}"));
        }

        // A reusable workflow from elsewhere is not checked here.
        if let Some(uses) = value(line, "uses")
            && uses.contains(".github/workflows/")
            && !uses.starts_with("./")
            && !shared(uses)
        {
            found.push(format!("{number}: reusable workflow `{uses}` is not local"));
        }
    }
    found
}

/// The problems in every workflow under `root`, as `file:line: problem`.
fn check(root: &Path) -> Vec<String> {
    let mut files: Vec<_> = fs::read_dir(root.join(WORKFLOWS))
        .expect("the workflow directory is readable")
        .map(|entry| entry.expect("a workflow entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yml" || extension == "yaml")
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no workflows in {WORKFLOWS}");

    files
        .iter()
        .flat_map(|path| {
            let text = fs::read_to_string(path).expect("the workflow is readable");
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            problems(&text)
                .into_iter()
                .map(move |problem| format!("{name}:{problem}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SELF_HOSTED: &str = r#"fromJSON('["self-hosted","Linux","X64","ankiquest","build"]')"#;

    /// A workflow with one job running on `runs_on`.
    fn job(runs_on: &str) -> String {
        format!("on:\n  pull_request:\njobs:\n  test:\n    runs-on: {runs_on}\n    steps: []\n")
    }

    /// The routing expression with `fallback` for other events.
    fn route(fallback: &str) -> String {
        format!("${{{{ {SELF_HOSTED_EVENTS} && {SELF_HOSTED} || '{fallback}' }}}}")
    }

    #[test]
    fn workflows_keep_pull_requests_hosted() {
        assert_eq!(check(&crate::root()), Vec::<String>::new());
    }

    #[test]
    fn hosted_labels_pass() {
        for label in [
            "ubuntu-latest",
            "ubuntu-24.04",
            "macos-15-intel",
            "windows-2025",
        ] {
            assert_eq!(problems(&job(label)), Vec::<String>::new(), "{label}");
        }
    }

    #[test]
    fn routing_expression_passes() {
        assert_eq!(problems(&job(&route("ubuntu-24.04"))), Vec::<String>::new());
    }

    #[test]
    fn self_hosted_labels_fail() {
        for runs_on in [
            "[self-hosted, Linux, X64, ankiquest, build]",
            "self-hosted",
            "ankiquest",
            "'ubuntu-latest'x",
        ] {
            assert_eq!(problems(&job(runs_on)).len(), 1, "{runs_on}");
        }
    }

    // A pull_request_target run's ref is the base branch, so a ref check
    // alone sends it to the self-hosted runners.
    #[test]
    fn ref_only_routing_fails() {
        let runs_on = format!(
            "${{{{ github.ref == 'refs/heads/master' && {SELF_HOSTED} || 'ubuntu-24.04' }}}}"
        );
        assert_eq!(problems(&job(&runs_on)).len(), 1);
    }

    #[test]
    fn self_hosted_fallback_fails() {
        assert_eq!(problems(&job(&route("self-hosted"))).len(), 1);
    }

    #[test]
    fn other_expressions_fail() {
        for runs_on in [
            "${{ vars.RUNNER }}",
            "${{ inputs.runner }}",
            "${{ matrix.os || 'x' }}",
        ] {
            assert_eq!(problems(&job(runs_on)).len(), 1, "{runs_on}");
        }
    }

    #[test]
    fn runs_on_blocks_fail() {
        let text = "jobs:\n  test:\n    runs-on:\n      group: Default\n";
        assert_eq!(problems(text).len(), 1);
    }

    #[test]
    fn hosted_matrix_passes() {
        let text = "jobs:\n  test:\n    strategy:\n      matrix:\n        include:\n          - os: ubuntu-24.04\n          - os: macos-15\n    runs-on: ${{ matrix.os }}\n";
        assert_eq!(problems(text), Vec::<String>::new());
        let text = "jobs:\n  test:\n    strategy:\n      matrix:\n        os: [ubuntu-24.04, 'windows-2025']\n    runs-on: ${{ matrix.os }}\n";
        assert_eq!(problems(text), Vec::<String>::new());
    }

    #[test]
    fn self_hosted_matrix_fails() {
        let text = "jobs:\n  test:\n    strategy:\n      matrix:\n        os: [ubuntu-24.04, self-hosted]\n    runs-on: ${{ matrix.os }}\n";
        assert_eq!(problems(text).len(), 1);
        let text = "jobs:\n  test:\n    strategy:\n      matrix: ${{ fromJSON(needs.plan.outputs.matrix) }}\n    runs-on: ${{ matrix.os }}\n";
        assert_eq!(problems(text).len(), 1);
    }

    // Quoted keys and flow mappings are the same YAML as plain block
    // keys, so they must be checked the same way.
    #[test]
    fn quoted_runs_on_fails() {
        let text =
            "jobs:\n  build:\n    \"runs-on\": [self-hosted, Linux, X64, ankiquest, build]\n";
        assert_eq!(problems(text).len(), 1);
    }

    #[test]
    fn flow_mapping_runs_on_fails() {
        let text = "on: push\njobs: {build: {runs-on: [self-hosted]}}\n";
        assert_eq!(problems(text).len(), 1);
    }

    #[test]
    fn quoted_remote_uses_fails() {
        let text =
            "jobs:\n  build:\n    \"uses\": other/project/.github/workflows/build.yml@main\n";
        assert_eq!(problems(text).len(), 1);
    }

    #[test]
    fn remote_reusable_workflows_fail() {
        let text = "jobs:\n  remote:\n    uses: AnkiQuest/AnkiQuest-Server/.github/workflows/ci.yml@master\n  local:\n    uses: ./.github/workflows/ci.yml\n  shared:\n    uses: AnkiQuest/AnkiQuest-Dev/.github/workflows/labels.yml@main\n  branch:\n    uses: AnkiQuest/AnkiQuest-Dev/.github/workflows/labels.yml@feature\n";
        assert_eq!(problems(text).len(), 2);
    }
}
