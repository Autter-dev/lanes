//! Options every command shares: `--json` and `--request-id`.

use lane_core::error::{Error, Result};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Common {
    /// Print one JSON object on stdout instead of text.
    pub json: bool,
    /// Makes a write safe to retry: the same id never runs twice. Also `LANE_REQUEST_ID`.
    pub request_id: Option<String>,
}

/// Whether `--json` appears anywhere, so even a command line that fails to parse answers in JSON.
pub fn wants_json(args: &[String]) -> bool {
    args.iter().any(|a| a == "--json")
}

/// Take the shared options out of `args`; the rest is the command's own.
pub fn split(args: &[String]) -> Result<(Common, Vec<String>)> {
    split_with(args, std::env::var("LANE_REQUEST_ID").ok())
}

fn split_with(args: &[String], env_request_id: Option<String>) -> Result<(Common, Vec<String>)> {
    let mut common = Common::default();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--json" {
            common.json = true;
        } else if arg == "--request-id" {
            let id = it.next().ok_or_else(|| {
                Error::usage("--request-id needs a value")
                    .hint("Pass an id, like `--request-id 7f3c-retry-1`.")
            })?;
            common.request_id = Some(id.clone());
        } else if let Some(id) = arg.strip_prefix("--request-id=") {
            common.request_id = Some(id.to_string());
        } else {
            rest.push(arg.clone());
        }
    }
    if common.request_id.is_none() {
        common.request_id = env_request_id.filter(|id| !id.is_empty());
    }
    if let Some(id) = &common.request_id {
        check_request_id(id)?;
    }
    Ok((common, rest))
}

/// Request ids are kept short and plain, as they're stored and printed.
pub fn check_request_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c));
    if ok {
        Ok(())
    } else {
        Err(Error::usage(format!("`{id}` can't be a request id"))
            .hint("Use up to 128 letters, digits, dots, colons, underscores or hyphens."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn shared_options_come_out_anywhere() {
        let (c, rest) = split_with(
            &strings(&["fix-auth", "--json", "--request-id", "r1", "--from", "main"]),
            None,
        )
        .unwrap();
        assert_eq!(
            c,
            Common {
                json: true,
                request_id: Some("r1".into())
            }
        );
        assert_eq!(rest, strings(&["fix-auth", "--from", "main"]));
        let (c, _) = split_with(&strings(&["--request-id=abc:2"]), None).unwrap();
        assert_eq!(c.request_id.as_deref(), Some("abc:2"));
    }

    #[test]
    fn the_environment_supplies_a_request_id() {
        let (c, _) = split_with(&[], Some("from-env".into())).unwrap();
        assert_eq!(c.request_id.as_deref(), Some("from-env"));
        // The flag wins over the environment.
        let (c, _) = split_with(&strings(&["--request-id", "flag"]), Some("env".into())).unwrap();
        assert_eq!(c.request_id.as_deref(), Some("flag"));
    }

    #[test]
    fn bad_request_ids_are_usage_errors() {
        for args in [
            vec!["--request-id"],
            vec!["--request-id", "has space"],
            vec!["--request-id="],
        ] {
            let e = split_with(&strings(&args), None).unwrap_err();
            assert_eq!(e.kind, lane_core::error::ErrorKind::Usage, "{args:?}");
        }
    }
}
