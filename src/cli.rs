//! The command line: which config to read.

use crate::store::Error;

const USAGE: &str = "usage: ankiquest [config.json]\n       ankiquest [config.json] admin <command>   (see ankiquest admin help)";

/// Reads the optional config path, the only argument the server takes.
pub(crate) fn parse_args(args: &[String]) -> Result<Option<String>, Error> {
    match args {
        [] => Ok(None),
        [path] if !path.starts_with('-') => Ok(Some(path.clone())),
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn the_command_line_names_at_most_a_config() {
        assert_eq!(parse_args(&words(&[])).unwrap(), None);
        assert_eq!(
            parse_args(&words(&["/etc/ankiquest.json"]))
                .unwrap()
                .as_deref(),
            Some("/etc/ankiquest.json")
        );
        for bad in [
            vec!["--help"],
            vec!["/etc/ankiquest.json", "serve"],
            vec!["/etc/ankiquest.json", "message", "aldanita", "hello"],
        ] {
            assert!(
                parse_args(&words(&bad)).is_err(),
                "{bad:?} should not parse"
            );
        }
    }
}
