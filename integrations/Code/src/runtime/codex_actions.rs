//! Explicit user actions mapped to installed App Server methods, never prose.
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub struct Action {
    pub method: &'static str,
    pub params: Value,
    pub status: &'static str,
}

pub fn review(args: &str) -> Result<Action> {
    let args = args.trim();
    ensure!(
        args.len() <= 16 * 1024 && !args.contains('\0'),
        "Review instructions are too long or invalid"
    );
    let target = if args.is_empty() {
        json!({"type":"uncommittedChanges"})
    } else if let Some(branch) = args.strip_prefix("base ") {
        let branch = branch.trim();
        ensure!(
            !branch.is_empty() && branch.len() <= 1024 && !branch.chars().any(char::is_control),
            "Use /review base followed by a branch name"
        );
        json!({"type":"baseBranch","branch":branch})
    } else if let Some(sha) = args.strip_prefix("commit ") {
        let sha = sha.trim();
        ensure!(
            (7..=64).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "Use /review commit followed by a commit hash"
        );
        json!({"type":"commit","sha":sha})
    } else {
        ensure!(
            args != "base" && args != "commit",
            "Use /review, /review base <branch>, /review commit <hash>, or /review <instructions>"
        );
        json!({"type":"custom","instructions":args})
    };
    Ok(Action {
        method: "review/start",
        params: json!({"delivery":"inline","target":target}),
        status: "Reviewing changes",
    })
}

pub fn goal(args: &str) -> Result<Action> {
    let args = args.trim();
    ensure!(
        !args.contains('\0') && args.len() <= 16 * 1024,
        "Enter a valid goal of at most 4,000 characters"
    );
    let (method, params, status) = match args {
        "" | "show" => ("thread/goal/get", json!({}), "Reading goal"),
        "clear" => (
            "thread/goal/clear",
            json!({"origin":"user"}),
            "Clearing goal",
        ),
        "pause" => (
            "thread/goal/set",
            json!({"status":"paused","origin":"user"}),
            "Pausing goal",
        ),
        "resume" => (
            "thread/goal/set",
            json!({"status":"active","origin":"user"}),
            "Resuming goal",
        ),
        "complete" => (
            "thread/goal/set",
            json!({"status":"complete","origin":"user"}),
            "Completing goal",
        ),
        "budget" | "set" => {
            bail!("Use /goal set <objective> or /goal budget <positive tokens|none>")
        }
        _ if args.starts_with("budget ") => {
            let amount = args[7..].trim();
            let budget = if amount == "none" {
                Value::Null
            } else {
                let value = amount
                    .parse::<i64>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Use a positive whole token budget or 'none'")
                    })?;
                json!(value)
            };
            (
                "thread/goal/set",
                json!({"tokenBudget":budget,"origin":"user"}),
                "Updating goal budget",
            )
        }
        _ => {
            let objective = args.strip_prefix("set ").unwrap_or(args).trim();
            ensure!(
                !objective.is_empty() && objective.chars().count() <= 4000,
                "Enter a goal of at most 4,000 characters"
            );
            (
                "thread/goal/set",
                json!({"objective":objective,"status":"active","origin":"user"}),
                "Setting goal",
            )
        }
    };
    Ok(Action {
        method,
        params,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reviews_use_native_inline_targets_and_validate_commit_ids() {
        let action = review("").unwrap();
        assert_eq!(action.method, "review/start");
        assert_eq!(
            action.params,
            json!({"delivery":"inline","target":{"type":"uncommittedChanges"}})
        );
        assert_eq!(
            review("base release/one").unwrap().params["target"],
            json!({"type":"baseBranch","branch":"release/one"})
        );
        assert_eq!(
            review("commit abcdef12").unwrap().params["target"]["sha"],
            "abcdef12"
        );
        assert_eq!(
            review("Check accessibility").unwrap().params["target"]["type"],
            "custom"
        );
        assert!(review("commit --all").is_err());
        assert!(review("base").is_err());
        assert!(review("base a\nb").is_err());
    }
    #[test]
    fn goals_preserve_budgets_unless_explicitly_supplied() {
        let action = goal("set Ship the update").unwrap();
        assert_eq!(action.params["objective"], "Ship the update");
        assert_eq!(action.params["origin"], "user");
        assert!(action.params.get("tokenBudget").is_none());
        assert_eq!(goal("budget 40000").unwrap().params["tokenBudget"], 40000);
        assert!(goal("budget none").unwrap().params["tokenBudget"].is_null());
        for bad in [
            "budget 0",
            "budget -1",
            "budget 1.5",
            "budget 9223372036854775808",
            "set",
        ] {
            assert!(goal(bad).is_err(), "{bad}");
        }
        for name in ["pause", "resume", "complete"] {
            assert!(goal(name).unwrap().params.get("objective").is_none());
        }
        assert_eq!(goal("").unwrap().method, "thread/goal/get");
        assert_eq!(goal("clear").unwrap().method, "thread/goal/clear");
        assert!(goal(&"x".repeat(4001)).is_err());
        assert!(goal(&"海".repeat(4000)).is_ok());
    }
}
