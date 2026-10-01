//! The static risk table (spec §7.1), the matchers of rules and session
//! allowances, and the evaluation order (spec §7.2). Everything here is
//! pure: callers pass in the policy, rules, and allowances they read.

use anima_core::{DataValue, ToolCall};

use super::{
    ApprovalMatcher, ApprovalPolicy, ApprovalRule, MatcherKind, PolicyAction, RiskClass,
    SessionAllowance, MATCHER_KIND_NOT_FOR_TOOL, MATCHER_VALUE_INVALID, MAX_MATCHER_VALUE_CHARS,
};

/// Tools that only read (spec §7.1). `load_skill` and `list_automations`
/// arrive in M5 and M6.
const READ_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "glob",
    "grep",
    "todo_read",
    "memory_search",
    "recent_memories",
    "get_current_time",
    "calculate",
    "bg_list",
    "bg_output",
    "list_workspace_agents",
    "calendar_list_events",
    "mail_list_messages",
    "search_conversations",
    "load_skill",
    "list_automations",
];
/// Tools that change files and records (spec §7.1). The mail draft and the
/// calendar writes still only create records the owner approves in
/// Connectors.
const WRITE_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "multi_edit",
    "todo_write",
    "memory_add",
    "propose_skill",
    "create_automation",
    "pause_automation",
    "mail_create_draft",
    "calendar_create_event",
    "calendar_update_event",
    "calendar_delete_event",
];
const EXEC_TOOLS: &[&str] = &["bash", "bg_start", "bg_stop"];
const NETWORK_TOOLS: &[&str] = &["web_fetch", "exa_search"];
const DELEGATE_TOOLS: &[&str] = &[
    "delegate_to_agent",
    "spawn_helper",
    "send_message",
    "broadcast_message",
];

/// Characters that chain, substitute, or redirect in a shell. A command
/// holding any of them never matches a command-prefix rule or allowance, so
/// "Always allow `git status`" cannot approve `git status; rm -rf ~`.
const SHELL_OPERATORS: &[char] = &[';', '&', '|', '`', '$', '>', '<', '(', ')', '\n', '\r'];

/// What a call needs before it runs (spec §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Allow,
    Ask,
    Deny,
}

const TABLE: [(&[&str], RiskClass); 5] = [
    (READ_TOOLS, RiskClass::Read),
    (WRITE_TOOLS, RiskClass::Write),
    (EXEC_TOOLS, RiskClass::Exec),
    (NETWORK_TOOLS, RiskClass::Network),
    (DELEGATE_TOOLS, RiskClass::Delegate),
];

fn named_class(tool: &str) -> Option<RiskClass> {
    TABLE
        .iter()
        .find(|(tools, _)| tools.contains(&tool))
        .map(|(_, class)| *class)
}

/// A tool's class; one the table does not name is `exec` (spec §7.1).
pub(crate) fn risk_class(tool: &str) -> RiskClass {
    named_class(tool).unwrap_or(RiskClass::Exec)
}

/// Whether the table names `tool` itself rather than defaulting it; the
/// test that every registered tool is classified uses it.
#[cfg(test)]
pub(crate) fn is_classified(tool: &str) -> bool {
    named_class(tool).is_some()
}

/// The matcher kinds a rule or allowance for `tool` may use, the narrowest
/// first; `any` fits every tool.
pub(crate) fn matcher_kinds(tool: &str) -> &'static [MatcherKind] {
    match tool {
        "bash" | "bg_start" => &[MatcherKind::CommandPrefix, MatcherKind::Any],
        "write_file" | "edit_file" | "multi_edit" => &[MatcherKind::PathGlob, MatcherKind::Any],
        "web_fetch" => &[MatcherKind::Domain, MatcherKind::Any],
        _ => &[MatcherKind::Any],
    }
}

fn string_arg<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
    match call.args.get(key) {
        Some(DataValue::String(value)) => Some(value.as_str()),
        _ => None,
    }
}

/// Whether `matcher` covers `call`; a kind that does not fit the tool
/// covers nothing.
pub(crate) fn matcher_matches(matcher: &ApprovalMatcher, call: &ToolCall) -> bool {
    if !matcher_kinds(&call.name).contains(&matcher.kind) {
        return false;
    }
    match matcher.kind {
        MatcherKind::Any => true,
        MatcherKind::CommandPrefix => string_arg(call, "command")
            .is_some_and(|command| command_has_prefix(command, &matcher.value)),
        MatcherKind::PathGlob => {
            string_arg(call, "file_path").is_some_and(|path| path_matches(&matcher.value, path))
        }
        MatcherKind::Domain => {
            string_arg(call, "url").is_some_and(|url| url_in_domain(url, &matcher.value))
        }
    }
}

/// Whether `command`'s first words are `prefix`'s words, and `command`
/// holds no shell operator.
pub(crate) fn command_has_prefix(command: &str, prefix: &str) -> bool {
    if command.contains(SHELL_OPERATORS) {
        return false;
    }
    let prefix = prefix.split_whitespace().collect::<Vec<_>>();
    let words = command.split_whitespace().collect::<Vec<_>>();
    !prefix.is_empty() && words.len() >= prefix.len() && words[..prefix.len()] == prefix[..]
}

/// A workspace-relative path's components, or `None` for an absolute path,
/// one that climbs with `..`, or one naming a drive.
fn relative_components(path: &str) -> Option<Vec<&str>> {
    if path.starts_with(['/', '\\']) {
        return None;
    }
    let mut components = Vec::new();
    for component in path.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => return None,
            component if component.contains(':') => return None,
            component => components.push(component),
        }
    }
    (!components.is_empty()).then_some(components)
}

/// Whether `path` matches `glob`: `*` and `?` within a component, `**`
/// across any number of components. Absolute and climbing paths never match.
pub(crate) fn path_matches(glob: &str, path: &str) -> bool {
    match (relative_components(glob), relative_components(path)) {
        (Some(pattern), Some(path)) => segments_match(&pattern, &path),
        _ => false,
    }
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if *first == "**" => {
            (0..=path.len()).any(|skip| segments_match(rest, &path[skip..]))
        }
        Some((first, rest)) => path.split_first().is_some_and(|(component, remaining)| {
            component_matches(first, component) && segments_match(rest, remaining)
        }),
    }
}

/// `*` matches any run of characters and `?` exactly one, within a component.
fn component_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let text = text.chars().collect::<Vec<_>>();
    let (mut at, mut read) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while read < text.len() {
        if at < pattern.len() && (pattern[at] == '?' || pattern[at] == text[read]) {
            at += 1;
            read += 1;
        } else if at < pattern.len() && pattern[at] == '*' {
            star = Some((at, read));
            at += 1;
        } else if let Some((star_at, star_read)) = star {
            at = star_at + 1;
            read = star_read + 1;
            star = Some((star_at, star_read + 1));
        } else {
            return false;
        }
    }
    pattern[at..].iter().all(|character| *character == '*')
}

fn http_host(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed
        .host_str()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Whether `url` is an `http`/`https` URL on `domain` or a subdomain of it.
pub(crate) fn url_in_domain(url: &str, domain: &str) -> bool {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    !domain.is_empty()
        && http_host(url)
            .is_some_and(|host| host == domain || host.ends_with(&format!(".{domain}")))
}

/// The command's first word, plus its second when that reads as a
/// subcommand (`git status`, `npm test`), cut at the first shell operator.
fn command_suggestion(command: &str) -> String {
    let head = command.split(SHELL_OPERATORS).next().unwrap_or_default();
    let mut words = head.split_whitespace();
    let mut prefix = words.next().unwrap_or_default().to_string();
    if let Some(second) = words.next() {
        let subcommand = second
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic())
            && second.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            });
        if subcommand {
            prefix.push(' ');
            prefix.push_str(second);
        }
    }
    prefix
}

/// The file's folder and everything under it, or the file itself at the root.
fn path_suggestion(path: &str) -> String {
    match relative_components(path) {
        Some(components) if components.len() > 1 => {
            format!("{}/**", components[..components.len() - 1].join("/"))
        }
        Some(components) => components[0].to_string(),
        None => path.trim().to_string(),
    }
}

/// What "Always allow" and "Allow for this session" cover unless the owner
/// edits it (spec §7.3 `suggestedMatcher`): the narrowest kind the tool
/// takes, filled from the call.
pub(crate) fn suggested_matcher(call: &ToolCall) -> ApprovalMatcher {
    let kind = matcher_kinds(&call.name)[0];
    let value = match kind {
        MatcherKind::CommandPrefix => string_arg(call, "command").map(command_suggestion),
        MatcherKind::PathGlob => string_arg(call, "file_path").map(path_suggestion),
        MatcherKind::Domain => string_arg(call, "url").and_then(http_host),
        MatcherKind::Any => None,
    };
    match value {
        Some(value) => ApprovalMatcher { kind, value },
        None => ApprovalMatcher::any(),
    }
}

fn is_domain(value: &str) -> bool {
    let value = value.trim_end_matches('.');
    !value.is_empty()
        && !value.starts_with('.')
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
}

/// An owner's matcher for `tool`, normalized, or why it is refused.
pub(crate) fn validate_matcher(
    tool: &str,
    matcher: &ApprovalMatcher,
) -> Result<ApprovalMatcher, &'static str> {
    if !matcher_kinds(tool).contains(&matcher.kind) {
        return Err(MATCHER_KIND_NOT_FOR_TOOL);
    }
    let value = matcher.value.trim();
    let value = match matcher.kind {
        MatcherKind::Any => return Ok(ApprovalMatcher::any()),
        _ if value.is_empty() || value.chars().count() > MAX_MATCHER_VALUE_CHARS => {
            return Err(MATCHER_VALUE_INVALID)
        }
        MatcherKind::CommandPrefix if !value.contains(SHELL_OPERATORS) => {
            value.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        MatcherKind::PathGlob if relative_components(value).is_some() => value.to_string(),
        MatcherKind::Domain if is_domain(value) => value.trim_end_matches('.').to_ascii_lowercase(),
        _ => return Err(MATCHER_VALUE_INVALID),
    };
    Ok(ApprovalMatcher {
        kind: matcher.kind,
        value,
    })
}

/// Spec §7.2's order: a class `deny` is denied; a matching rule or session
/// allowance is allowed; a class `ask` asks; everything else is allowed.
/// Read-class tools are always allowed.
pub(crate) fn evaluate(
    policy: &ApprovalPolicy,
    rules: &[&ApprovalRule],
    allowances: &[SessionAllowance],
    call: &ToolCall,
) -> Verdict {
    let Some(action) = policy.action(risk_class(&call.name)) else {
        return Verdict::Allow;
    };
    if action == PolicyAction::Deny {
        return Verdict::Deny;
    }
    let covered = rules
        .iter()
        .any(|rule| rule.tool == call.name && matcher_matches(&rule.matcher, call))
        || allowances.iter().any(|allowance| {
            allowance.tool == call.name && matcher_matches(&allowance.matcher, call)
        });
    if covered || action == PolicyAction::Allow {
        Verdict::Allow
    } else {
        Verdict::Ask
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use anima_core::{DataValue, ToolCall};

    use super::*;
    use crate::approvals::{
        ApprovalMatcher, ApprovalPolicy, ApprovalRule, MatcherKind, PolicyAction, RiskClass,
        SessionAllowance, MATCHER_KIND_NOT_FOR_TOOL, MATCHER_VALUE_INVALID,
    };

    fn call(name: &str, args: &[(&str, &str)]) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: name.into(),
            args: args
                .iter()
                .map(|(key, value)| (key.to_string(), DataValue::String(value.to_string())))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn matcher(kind: MatcherKind, value: &str) -> ApprovalMatcher {
        ApprovalMatcher {
            kind,
            value: value.into(),
        }
    }

    fn rule(tool: &str, matcher: ApprovalMatcher) -> ApprovalRule {
        ApprovalRule {
            id: "rule-1".into(),
            agent_id: "agent-1".into(),
            tool: tool.into(),
            matcher,
            created_at_ms: 1,
            from_approval_id: None,
        }
    }

    #[test]
    fn the_table_classes_the_tools_spec_7_1_lists() {
        for (class, tools) in [
            (
                RiskClass::Read,
                &[
                    "read_file",
                    "list_dir",
                    "glob",
                    "grep",
                    "todo_read",
                    "memory_search",
                    "recent_memories",
                    "get_current_time",
                    "calculate",
                    "bg_list",
                    "bg_output",
                    "list_workspace_agents",
                    "calendar_list_events",
                    "mail_list_messages",
                    "search_conversations",
                    "load_skill",
                    "list_automations",
                ][..],
            ),
            (
                RiskClass::Write,
                &[
                    "write_file",
                    "edit_file",
                    "multi_edit",
                    "todo_write",
                    "memory_add",
                    "propose_skill",
                    "create_automation",
                    "pause_automation",
                    "mail_create_draft",
                    "calendar_create_event",
                    "calendar_update_event",
                    "calendar_delete_event",
                ][..],
            ),
            (RiskClass::Exec, &["bash", "bg_start", "bg_stop"][..]),
            (RiskClass::Network, &["web_fetch", "exa_search"][..]),
            (
                RiskClass::Delegate,
                &[
                    "delegate_to_agent",
                    "spawn_helper",
                    "send_message",
                    "broadcast_message",
                ][..],
            ),
        ] {
            for tool in tools {
                assert_eq!(risk_class(tool), class, "{tool}");
                assert!(is_classified(tool), "{tool}");
            }
        }
    }

    #[test]
    fn an_unknown_tool_is_exec() {
        assert_eq!(risk_class("teleport"), RiskClass::Exec);
        assert!(!is_classified("teleport"));
    }

    #[test]
    fn every_registered_tool_has_an_explicit_class() {
        for tool in crate::tools::ToolRegistry::new().tool_names() {
            assert!(
                is_classified(&tool),
                "classify {tool} in approvals/policy.rs"
            );
        }
    }

    /// A tripwire on tool names only, not on reachability: policies and rules
    /// change only through the owner-authorized routes (spec §7.2), and those
    /// are reachable only through an exec call (see M4 Task 7's Limits).
    #[test]
    fn no_tool_is_named_after_approvals() {
        for tool in crate::tools::ToolRegistry::new().tool_names() {
            assert!(
                !tool.contains("approval") && !tool.contains("policy"),
                "{tool} must not reach approval state"
            );
        }
    }

    #[test]
    fn the_default_policy_asks_only_before_exec() {
        let policy = ApprovalPolicy::default();
        assert_eq!(policy.action(RiskClass::Read), None);
        assert_eq!(policy.action(RiskClass::Write), Some(PolicyAction::Allow));
        assert_eq!(policy.action(RiskClass::Exec), Some(PolicyAction::Ask));
        assert_eq!(policy.action(RiskClass::Network), Some(PolicyAction::Allow));
        assert_eq!(
            policy.action(RiskClass::Delegate),
            Some(PolicyAction::Allow)
        );
        assert_eq!(
            policy.with(RiskClass::Read, PolicyAction::Deny),
            policy,
            "read-class tools have no policy entry"
        );
    }

    #[test]
    fn evaluation_follows_deny_then_rules_then_ask() {
        let ask_all = ApprovalPolicy {
            write: PolicyAction::Ask,
            exec: PolicyAction::Ask,
            network: PolicyAction::Ask,
            delegate: PolicyAction::Ask,
        };
        let deny_all = ApprovalPolicy {
            write: PolicyAction::Deny,
            exec: PolicyAction::Deny,
            network: PolicyAction::Deny,
            delegate: PolicyAction::Deny,
        };
        let remember = call("memory_add", &[("content", "the plan")]);
        let covering = rule("memory_add", ApprovalMatcher::any());
        let allowance = SessionAllowance {
            tool: "memory_add".into(),
            matcher: ApprovalMatcher::any(),
            created_at_ms: 1,
            from_approval_id: "apr_1".into(),
        };

        assert_eq!(evaluate(&ask_all, &[], &[], &remember), Verdict::Ask);
        assert_eq!(
            evaluate(&ask_all, &[&covering], &[], &remember),
            Verdict::Allow
        );
        assert_eq!(
            evaluate(&ask_all, &[], &[allowance.clone()], &remember),
            Verdict::Allow
        );
        assert_eq!(
            evaluate(&deny_all, &[&covering], &[allowance], &remember),
            Verdict::Deny,
            "a class deny beats every rule and allowance"
        );
        assert_eq!(
            evaluate(&ApprovalPolicy::default(), &[], &[], &remember),
            Verdict::Allow
        );
        assert_eq!(
            evaluate(
                &deny_all,
                &[],
                &[],
                &call("calculate", &[("expression", "1+1")])
            ),
            Verdict::Allow,
            "read-class tools never ask and are never denied"
        );
        let other_tool = rule("todo_write", ApprovalMatcher::any());
        assert_eq!(
            evaluate(&ask_all, &[&other_tool], &[], &remember),
            Verdict::Ask,
            "a rule covers only its own tool"
        );
    }

    #[test]
    fn a_command_prefix_matches_whole_words_and_never_a_command_with_shell_operators() {
        for command in ["git status", "git  status --short", "  git status -sb  "] {
            assert!(command_has_prefix(command, "git status"), "{command}");
        }
        for command in [
            "git statusx",
            "git",
            "git status; rm -rf ~",
            "git status && curl example.com",
            "git status | sh",
            "git status || true",
            "git status $(whoami)",
            "git status `whoami`",
            "git status > out.txt",
            "git status < in.txt",
            "git status & sleep 1",
            "git status\nrm -rf ~",
            "(git status)",
        ] {
            assert!(!command_has_prefix(command, "git status"), "{command:?}");
        }
        assert!(
            !command_has_prefix("git status", "  "),
            "an empty prefix matches nothing"
        );
    }

    #[test]
    fn a_path_glob_is_workspace_relative() {
        for (glob, path) in [
            ("notes/**", "notes/today.md"),
            ("notes/**", "notes/2026/today.md"),
            ("notes/**", "./notes/today.md"),
            ("src/*.rs", "src/main.rs"),
            ("docs/?.md", "docs/a.md"),
            ("**", "anything/at/all.txt"),
            ("README.md", "README.md"),
        ] {
            assert!(path_matches(glob, path), "{glob} ~ {path}");
        }
        for (glob, path) in [
            ("notes/**", "notes/../secrets.txt"),
            ("notes/**", "/etc/passwd"),
            ("notes/**", "other/today.md"),
            ("src/*.rs", "src/nested/main.rs"),
            ("docs/?.md", "docs/ab.md"),
            ("**", "/etc/passwd"),
            ("../**", "../outside.txt"),
            ("notes/**", "C:notes/today.md"),
        ] {
            assert!(!path_matches(glob, path), "{glob} !~ {path}");
        }
    }

    #[test]
    fn a_domain_covers_its_host_and_subdomains_over_http() {
        for url in [
            "https://example.com/page",
            "http://docs.example.com/a?b=c",
            "https://EXAMPLE.com",
        ] {
            assert!(url_in_domain(url, "example.com"), "{url}");
        }
        for url in [
            "https://badexample.com",
            "https://example.com.evil.net/",
            "ftp://example.com/file",
            "not a url",
        ] {
            assert!(!url_in_domain(url, "example.com"), "{url}");
        }
    }

    #[test]
    fn matchers_apply_only_to_the_tools_they_fit() {
        let bash = call("bash", &[("command", "npm test")]);
        assert!(matcher_matches(
            &matcher(MatcherKind::CommandPrefix, "npm"),
            &bash
        ));
        assert!(matcher_matches(&ApprovalMatcher::any(), &bash));
        assert!(!matcher_matches(
            &matcher(MatcherKind::PathGlob, "**"),
            &bash
        ));
        let write = call(
            "write_file",
            &[("file_path", "notes/a.md"), ("content", "x")],
        );
        assert!(matcher_matches(
            &matcher(MatcherKind::PathGlob, "notes/**"),
            &write
        ));
        let fetch = call("web_fetch", &[("url", "https://docs.rs/serde")]);
        assert!(matcher_matches(
            &matcher(MatcherKind::Domain, "docs.rs"),
            &fetch
        ));
        assert_eq!(
            matcher_kinds("memory_add"),
            &[MatcherKind::Any][..],
            "other tools take only `any`"
        );
    }

    #[test]
    fn suggestions_are_narrow_enough_to_read_before_always_allowing() {
        assert_eq!(
            suggested_matcher(&call("bash", &[("command", "git status --short")])),
            matcher(MatcherKind::CommandPrefix, "git status")
        );
        assert_eq!(
            suggested_matcher(&call("bash", &[("command", "ls -la")])),
            matcher(MatcherKind::CommandPrefix, "ls")
        );
        assert_eq!(
            suggested_matcher(&call("bash", &[("command", "npm test; rm -rf ~")])),
            matcher(MatcherKind::CommandPrefix, "npm test")
        );
        assert_eq!(
            suggested_matcher(&call("write_file", &[("file_path", "notes/today.md")])),
            matcher(MatcherKind::PathGlob, "notes/**")
        );
        assert_eq!(
            suggested_matcher(&call("edit_file", &[("file_path", "README.md")])),
            matcher(MatcherKind::PathGlob, "README.md")
        );
        assert_eq!(
            suggested_matcher(&call("web_fetch", &[("url", "https://Docs.Example.com/a")])),
            matcher(MatcherKind::Domain, "docs.example.com")
        );
        assert_eq!(
            suggested_matcher(&call("memory_add", &[("content", "x")])),
            ApprovalMatcher::any()
        );
    }

    #[test]
    fn an_owner_matcher_is_normalized_or_refused() {
        assert_eq!(
            validate_matcher(
                "bash",
                &matcher(MatcherKind::CommandPrefix, "  git   status ")
            ),
            Ok(matcher(MatcherKind::CommandPrefix, "git status"))
        );
        assert_eq!(
            validate_matcher("web_fetch", &matcher(MatcherKind::Domain, "Docs.RS.")),
            Ok(matcher(MatcherKind::Domain, "docs.rs"))
        );
        assert_eq!(
            validate_matcher("bash", &matcher(MatcherKind::Any, "ignored")),
            Ok(ApprovalMatcher::any())
        );
        assert_eq!(
            validate_matcher("memory_add", &matcher(MatcherKind::PathGlob, "**")),
            Err(MATCHER_KIND_NOT_FOR_TOOL)
        );
        for (tool, refused) in [
            (
                "bash",
                matcher(MatcherKind::CommandPrefix, "git status; rm"),
            ),
            ("bash", matcher(MatcherKind::CommandPrefix, "   ")),
            ("write_file", matcher(MatcherKind::PathGlob, "../**")),
            ("write_file", matcher(MatcherKind::PathGlob, "/etc/*")),
            (
                "web_fetch",
                matcher(MatcherKind::Domain, "https://example.com"),
            ),
            ("web_fetch", matcher(MatcherKind::Domain, ".example.com")),
            (
                "bash",
                matcher(MatcherKind::CommandPrefix, &"x".repeat(513)),
            ),
        ] {
            assert_eq!(
                validate_matcher(tool, &refused),
                Err(MATCHER_VALUE_INVALID),
                "{refused:?}"
            );
        }
    }
}
