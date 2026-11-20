//! Keeps pull request code off AnkiQuest's self-hosted runners. Their
//! runner group admits every workflow and ref (float3/nixos
//! `nixos/runner-policy.nix`), so only a job's `runs-on` decides where
//! it runs. Each workflow in `.github/workflows/` is parsed as YAML, and
//! every job's `runs-on` must be one of:
//!
//! - a GitHub-hosted label (`ubuntu-*`, `macos-*`, `windows-*`);
//! - `${{ matrix.<key> }}`, with every value of `<key>` in the job's
//!   matrix, `include` entries too, a GitHub-hosted label;
//! - the routing expression: self-hosted labels for merge groups and
//!   pushes to the default branch, a GitHub-hosted label for every other
//!   event, `pull_request` and `pull_request_target` included.
//!
//! Anything else fails, including forms this check does not accept (a
//! list or mapping, runner groups, variables, inputs, merge keys,
//! invalid YAML). A job without `runs-on` must call a reusable
//! workflow that is local, so its jobs are checked here too, or one of
//! AnkiQuest-Dev's shared ones on its default branch, `master`, which
//! its `runner-routing` check covers there.

use std::fs;
use std::path::Path;

use yaml_rust2::{Yaml, YamlLoader};

/// The workflow directory, relative to the repository root.
const WORKFLOWS: &str = ".github/workflows";

/// The events that may run on self-hosted runners: merge groups and
/// pushes to the default branch.
const SELF_HOSTED_EVENTS: &str = "(github.event_name == 'merge_group' || \
(github.event_name == 'push' && github.ref == \
format('refs/heads/{0}', github.event.repository.default_branch)))";

/// Local reusable workflows, checked with the rest of the directory.
const LOCAL: &str = "./.github/workflows/";

/// AnkiQuest-Dev's shared reusable workflows, and the only ref of them
/// allowed: its default branch.
const SHARED: &str = "AnkiQuest/AnkiQuest-Dev/.github/workflows/";
const SHARED_REF: &str = "@master";

/// The YAML merge key, which would hide a job's keys from this check.
const MERGE_KEY: &str = "<<";

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

/// Whether `uses` names one of AnkiQuest-Dev's shared workflows on master.
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

/// The labels `value` gives a matrix key: none when it is absent, `None`
/// when it is not a string or a list of strings.
fn labels(value: &Yaml) -> Option<Vec<&str>> {
    match value {
        Yaml::BadValue => Some(Vec::new()),
        Yaml::String(label) => Some(vec![label]),
        Yaml::Array(items) => items.iter().map(Yaml::as_str).collect(),
        _ => None,
    }
}

/// Whether every value of the matrix key `key` in `job` is hosted.
fn hosted_matrix(job: &Yaml, key: &str) -> bool {
    let matrix = &job["strategy"]["matrix"];
    if matrix.as_hash().is_none() {
        return false;
    }

    // The key's own values.
    let Some(mut found) = labels(&matrix[key]) else {
        return false;
    };

    // Values `include` entries add.
    let include = &matrix["include"];
    let entries: &[Yaml] = match include {
        Yaml::BadValue => &[],
        Yaml::Array(entries) => entries,
        _ => return false,
    };
    for entry in entries {
        if entry.as_hash().is_none() {
            return false;
        }
        let Some(more) = labels(&entry[key]) else {
            return false;
        };
        found.extend(more);
    }

    !found.is_empty() && found.iter().all(|label| hosted(label))
}

/// The problem with one `runs-on` value, if any.
fn runs_on_problem(job: &Yaml, runs_on: &Yaml) -> Option<String> {
    let Some(runs_on) = runs_on.as_str() else {
        return Some("runs-on must be one label or expression".to_string());
    };
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
        if hosted_matrix(job, key) {
            return None;
        }
        return Some(format!(
            "matrix.{key} has values that are not GitHub-hosted labels"
        ));
    }
    Some(format!(
        "`{runs_on}` is not a GitHub-hosted label or the routing expression"
    ))
}

/// The problem with a job's reusable workflow `uses`, if any.
fn uses_problem(uses: &Yaml) -> Option<String> {
    let Some(uses) = uses.as_str() else {
        return Some("uses must be a workflow path".to_string());
    };
    if uses.starts_with(LOCAL) || shared(uses) {
        return None;
    }
    Some(format!("reusable workflow `{uses}` is not local"))
}

/// The problems with one job.
fn job_problems(job: &Yaml) -> Vec<String> {
    if job.as_hash().is_none() {
        return vec!["is not a mapping".to_string()];
    }

    // A job runs on runs-on or calls a reusable workflow; GitHub rejects
    // one with both, so check each that is there.
    let runs_on = &job["runs-on"];
    let uses = &job["uses"];
    if runs_on.is_badvalue() && uses.is_badvalue() {
        return vec!["has neither runs-on nor uses".to_string()];
    }
    let mut found = Vec::new();
    if !runs_on.is_badvalue() {
        found.extend(runs_on_problem(job, runs_on));
    }
    if !uses.is_badvalue() {
        found.extend(uses_problem(uses));
    }
    found
}

/// Whether `value` holds a merge key anywhere.
fn merges(value: &Yaml) -> bool {
    match value {
        Yaml::Hash(hash) => hash
            .iter()
            .any(|(key, value)| key.as_str() == Some(MERGE_KEY) || merges(value)),
        Yaml::Array(items) => items.iter().any(merges),
        _ => false,
    }
}

/// The problems in one workflow.
fn problems(text: &str) -> Vec<String> {
    let documents = match YamlLoader::load_from_str(text) {
        Ok(documents) => documents,
        Err(error) => return vec![format!("not valid YAML: {error}")],
    };
    let [workflow] = documents.as_slice() else {
        return vec!["must be one YAML document".to_string()];
    };

    // GitHub may expand merge keys this check does not.
    if merges(workflow) {
        return vec![format!("merge keys (`{MERGE_KEY}`) are not supported")];
    }

    let Some(jobs) = workflow["jobs"].as_hash() else {
        return vec!["jobs must be a mapping".to_string()];
    };
    let mut found = Vec::new();
    for (id, job) in jobs {
        let Some(id) = id.as_str() else {
            found.push(format!("job id `{id:?}` is not a string"));
            continue;
        };
        found.extend(
            job_problems(job)
                .into_iter()
                .map(|problem| format!("job `{id}`: {problem}")),
        );
    }
    found
}

/// The problems in every workflow under `root`, as `file: problem`.
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
                .map(move |problem| format!("{name}: {problem}"))
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
        let text = "jobs:\n  remote:\n    uses: AnkiQuest/AnkiQuest-Server/.github/workflows/ci.yml@master\n  local:\n    uses: ./.github/workflows/ci.yml\n  shared:\n    uses: AnkiQuest/AnkiQuest-Dev/.github/workflows/labels.yml@master\n  main:\n    uses: AnkiQuest/AnkiQuest-Dev/.github/workflows/labels.yml@main\n  branch:\n    uses: AnkiQuest/AnkiQuest-Dev/.github/workflows/labels.yml@feature\n";
        assert_eq!(problems(text).len(), 3);
    }

    #[test]
    fn aliases_are_resolved() {
        let text = "x: &label self-hosted\njobs:\n  test:\n    runs-on: *label\n";
        assert_eq!(problems(text).len(), 1);
        let text = "x: &label ubuntu-24.04\njobs:\n  test:\n    runs-on: *label\n";
        assert_eq!(problems(text), Vec::<String>::new());
    }

    #[test]
    fn merge_keys_fail() {
        let text =
            "x: &base {runs-on: self-hosted}\njobs:\n  test:\n    <<: *base\n    steps: []\n";
        assert_eq!(problems(text).len(), 1);
    }

    #[test]
    fn unreadable_workflows_fail() {
        for text in [
            "jobs:\n  test:\n    runs-on: ubuntu-24.04\n    runs-on: self-hosted\n",
            "jobs: [test]\n",
            "jobs:\n  test: ubuntu-24.04\n",
            "jobs:\n  test:\n    steps: []\n",
            "jobs: {}\n---\njobs: {}\n",
        ] {
            assert_eq!(problems(text).len(), 1, "{text}");
        }
    }

    #[test]
    fn matrix_of_other_job_fails() {
        let text = "jobs:\n  a:\n    strategy:\n      matrix:\n        os: [ubuntu-24.04]\n    runs-on: ubuntu-24.04\n  b:\n    runs-on: ${{ matrix.os }}\n";
        assert_eq!(problems(text).len(), 1);
    }

    #[test]
    fn matrix_include_expression_fails() {
        let text = "jobs:\n  test:\n    strategy:\n      matrix:\n        os: [ubuntu-24.04]\n        include: ${{ fromJSON(vars.EXTRA) }}\n    runs-on: ${{ matrix.os }}\n";
        assert_eq!(problems(text).len(), 1);
    }
}
