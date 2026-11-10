use super::*;

// Codex is only a registry key here; behavior tests supply synthetic rules.
fn remote_manifest(version: &str, state: &str, contains: &str) -> String {
    format!(
        r#"
id = "codex"
version = "{version}"
min_engine_version = 1
updated_at = "2026-06-10T12:00:00Z"

[[rules]]
id = "test"
state = "{state}"
contains = ["{contains}"]
"#
    )
}

fn local_manifest(state: &str, contains: &str) -> String {
    format!(
        r#"
id = "codex"

[[rules]]
id = "test"
state = "{state}"
contains = ["{contains}"]
"#
    )
}

fn rules_manifest(rules: &str) -> String {
    format!(
        r#"
id = "codex"

{rules}
"#
    )
}

fn with_manifest_dirs<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let old_config = std::env::var_os("XDG_CONFIG_HOME");
    let old_state = std::env::var_os("XDG_STATE_HOME");
    let base = std::env::temp_dir().join(format!(
        "herdr-manifest-loader-{name}-{}",
        std::process::id()
    ));
    let config_dir = base.join("config");
    let state_dir = base.join("state");
    let _ = std::fs::remove_dir_all(&base);
    std::env::set_var("XDG_CONFIG_HOME", &config_dir);
    std::env::set_var("XDG_STATE_HOME", &state_dir);
    reload_manifests();
    let result = f();
    match old_config {
        Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }
    match old_state {
        Some(value) => std::env::set_var("XDG_STATE_HOME", value),
        None => std::env::remove_var("XDG_STATE_HOME"),
    }
    reload_manifests();
    let _ = std::fs::remove_dir_all(&base);
    result
}

fn write_remote_codex(content: &str) {
    let path = crate::detect::manifest_update::remote_manifest_path(Agent::Codex);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
    reload_manifests();
}

fn write_remote_codex_without_reload(content: &str) {
    let path = crate::detect::manifest_update::remote_manifest_path(Agent::Codex);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn write_local_codex(content: &str) {
    let path = override_path(Agent::Codex).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
    reload_manifests();
}

#[test]
fn known_agent_no_match_defaults_to_idle_fallback() {
    with_manifest_dirs("no-match", || {
        write_local_codex(&local_manifest("working", "active-marker"));
        let explain = explain(Agent::Codex, "unmatched-marker");

        assert_eq!(explain.state, AgentState::Idle);
        assert!(!explain.visible_idle);
        assert_eq!(
            explain.fallback_reason.as_deref(),
            Some(DEFAULT_KNOWN_AGENT_IDLE_FALLBACK)
        );
    });
}

#[test]
fn rule_semantics_apply_gates_priority_and_line_regex() {
    with_manifest_dirs("rule-semantics", || {
        write_local_codex(&rules_manifest(
            r#"
[[rules]]
id = "low_contains"
state = "idle"
priority = 1
contains = ["match"]

[[rules]]
id = "high_nested_gates"
state = "working"
priority = 10
contains = ["match"]
all = [
  { any = [{ regex = ["w[io]n"] }, { contains = ["fallback"] }] },
]
not = [
  { contains = ["blocked"] },
]

[[rules]]
id = "line_regex"
state = "blocked"
priority = 20
line_regex = ["^exact line$"]
"#,
        ));

        let high = explain(Agent::Codex, "match win");
        assert_eq!(high.state, AgentState::Working);
        assert_eq!(
            high.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("high_nested_gates")
        );

        let not_gate = explain(Agent::Codex, "match win blocked");
        assert_eq!(not_gate.state, AgentState::Idle);
        assert_eq!(
            not_gate.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("low_contains")
        );

        let line = explain(Agent::Codex, "before\nexact line\nafter");
        assert_eq!(line.state, AgentState::Blocked);
        assert_eq!(
            line.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("line_regex")
        );
    });
}

// A synthetic terminal protocol: live controls may precede a named divider
// and an optional panel, but a newer prompt/output makes them historical.
#[test]
fn end_anchored_footer_gates_allow_only_auxiliary_tail() {
    with_manifest_dirs("footer-tail", || {
        write_local_codex(&rules_manifest(
            r#"
[[rules]]
id = "live_controls"
state = "blocked"
region = "whole_recent"
regex = ['(?i)(?:^|\n)controls:\s+(?:confirm-marker|select-marker\s+navigate-marker)\s+cancel-marker[ \t]*\n---[^\n]*(?:\npanel: \d+(?:\nrow: [^\n]+)+(?:\n… \+\d+ pending)?)?(?:\n[ \t]*)*\z']
"#,
        ));
        let footer = "controls: select-marker navigate-marker cancel-marker";
        for tail in [
            "--- name",
            "--- name\npanel: 5\nrow: active\nrow: pending\nrow: pending\nrow: pending\nrow: done\n",
            "--- name\npanel: 5\nrow: active\nrow: done\n… +3 pending",
        ] {
            let live = format!("old output\n{footer}\n{tail}");
            assert_eq!(explain(Agent::Codex, &live).state, AgentState::Blocked);
            for later in [
                "new output",
                "prompt:",
                "--- next\nprompt:",
                "prompt: continue…",
                "working: processing…",
                "…ordinary output",
                "finished…",
                "… +2 arbitrary words",
            ] {
                assert_eq!(
                    explain(Agent::Codex, &format!("{live}\n{later}")).state,
                    AgentState::Idle
                );
            }
        }
        for tail in [
            "--- name\n… +2 pending",
            "--- name\nrow: active",
            "--- name\npanel: 5",
            "--- name\npanel: 5\nrow: active\n  working: processing…",
            "--- name\npanel: 5\nrow: active\n  prompt: continue…",
        ] {
            assert_eq!(
                explain(Agent::Codex, &format!("{footer}\n{tail}")).state,
                AgentState::Idle
            );
        }
        for controls in [
            "controls: confirm-marker cancel-marker",
            "controls: select-marker\nnavigate-marker cancel-marker",
            "controls:\nselect-marker\nnavigate-marker\ncancel-marker",
        ] {
            assert_eq!(
                explain(Agent::Codex, &format!("{controls}\n--- name")).state,
                AgentState::Blocked
            );
        }
        for controls in [
            "controls: select-marker cancel-marker",
            "controls: navigate-marker cancel-marker",
            "controls: confirm-marker",
            "controls: select-marker unrelated navigate-marker cancel-marker",
            "controls: select-marker\nprompt: navigate-marker cancel-marker",
        ] {
            assert_eq!(
                explain(
                    Agent::Codex,
                    &format!("confirm-marker navigate-marker\n{controls}\n--- name")
                )
                .state,
                AgentState::Idle
            );
        }
    });
}

#[test]
fn paired_alternatives_require_complete_gates_at_bottom_buffer() {
    with_manifest_dirs("paired-alternatives", || {
        write_local_codex(&rules_manifest(
            r#"
[[rules]]
id = "paired"
state = "blocked"
region = "bottom_non_empty_lines(3)"
visible_blocker = true
any = [
  { all = [
    { contains = ["alpha heading"] },
    { regex = ['alpha\s+control\s*\z'] },
  ] },
  { all = [
    { contains = ["beta heading"] },
    { regex = ['beta\s+control\s*\z'] },
  ] },
]
"#,
        ));
        for text in [
            "alpha heading\nalpha control",
            "beta heading\nbeta\ncontrol",
        ] {
            let result = explain(Agent::Codex, text);
            assert_eq!(result.state, AgentState::Blocked);
            assert!(result.visible_blocker);
        }
        for text in [
            "alpha heading\nbeta control",
            "beta heading\nalpha control",
            "beta control",
            "beta heading",
            "beta heading\nbeta control\nnewer content",
            "beta heading\nfiller one\nfiller two\nbeta control",
        ] {
            let result = explain(Agent::Codex, text);
            assert!(result.matched_rule.is_none(), "{text}");
            assert!(!result.visible_blocker, "{text}");
        }
    });
}

#[test]
fn remote_manifest_loads_between_local_override_and_bundled() {
    with_manifest_dirs("remote-source", || {
        write_remote_codex(&remote_manifest("9999.01.01.1", "blocked", "remote-ready"));

        let explain = explain(Agent::Codex, "remote-ready");

        assert_eq!(explain.state, AgentState::Blocked);
        assert!(matches!(
            explain.source,
            Some(ManifestSource::Remote { .. })
        ));
        assert_eq!(explain.manifest_version.as_deref(), Some("9999.01.01.1"));
        assert_eq!(
            explain.cached_remote_version.as_deref(),
            Some("9999.01.01.1")
        );
    });
}

#[test]
fn fallback_explain_preserves_active_manifest_version() {
    with_manifest_dirs("fallback-version", || {
        write_remote_codex(&remote_manifest("9999.01.01.1", "blocked", "remote-ready"));

        let explain = explain(Agent::Codex, "ordinary prompt text");

        assert_eq!(explain.state, AgentState::Idle);
        assert_eq!(
            explain.fallback_reason.as_deref(),
            Some(DEFAULT_KNOWN_AGENT_IDLE_FALLBACK)
        );
        assert_eq!(explain.manifest_version.as_deref(), Some("9999.01.01.1"));
        assert!(matches!(
            explain.source,
            Some(ManifestSource::Remote { .. })
        ));
    });
}

#[test]
fn older_cached_remote_manifest_does_not_shadow_newer_bundled_manifest() {
    with_manifest_dirs("older-remote-bundled-fallback", || {
        write_remote_codex(&remote_manifest("2026.06.10.0", "blocked", "remote-ready"));

        let explain = explain(Agent::Codex, "remote-ready");

        assert!(matches!(explain.source, Some(ManifestSource::Bundled)));
        assert_eq!(
            explain.cached_remote_version.as_deref(),
            Some("2026.06.10.0")
        );
        assert!(explain
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("older than bundled")));
    });
}

#[test]
fn local_override_shadows_cached_remote_manifest() {
    with_manifest_dirs("local-shadows-remote", || {
        write_remote_codex(&remote_manifest("9999.01.01.1", "blocked", "remote-ready"));
        write_local_codex(&local_manifest("idle", "local-ready"));

        let explain = explain(Agent::Codex, "local-ready");

        assert_eq!(explain.state, AgentState::Idle);
        assert!(matches!(explain.source, Some(ManifestSource::Override(_))));
        assert!(explain.local_override_shadowing_remote);
        assert_eq!(
            explain.cached_remote_version.as_deref(),
            Some("9999.01.01.1")
        );
    });
}

#[test]
fn invalid_local_override_falls_back_to_cached_remote_manifest() {
    with_manifest_dirs("invalid-local-remote-fallback", || {
        write_remote_codex(&remote_manifest("9999.01.01.1", "blocked", "remote-ready"));
        write_local_codex("id = ");

        let explain = explain(Agent::Codex, "remote-ready");

        assert_eq!(explain.state, AgentState::Blocked);
        assert!(matches!(
            explain.source,
            Some(ManifestSource::Remote { .. })
        ));
        assert!(explain.warning.is_some());
    });
}

#[test]
fn detection_uses_cached_manifest_until_explicit_reload() {
    with_manifest_dirs("cache-boundary", || {
        write_remote_codex(&remote_manifest("9999.01.01.1", "blocked", "cached-ready"));

        let cached = explain(Agent::Codex, "cached-ready");
        assert_eq!(cached.state, AgentState::Blocked);
        assert!(matches!(cached.source, Some(ManifestSource::Remote { .. })));
        assert_eq!(
            cached.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("test")
        );

        write_remote_codex_without_reload(&remote_manifest("9999.01.01.2", "working", "new-ready"));

        let unchanged = explain(Agent::Codex, "new-ready");
        assert_eq!(unchanged.state, AgentState::Idle);
        assert_eq!(
            unchanged.fallback_reason.as_deref(),
            Some(DEFAULT_KNOWN_AGENT_IDLE_FALLBACK)
        );
        assert_eq!(
            unchanged.cached_remote_version.as_deref(),
            Some("9999.01.01.1")
        );

        reload_manifests();

        let reloaded = explain(Agent::Codex, "new-ready");
        assert_eq!(reloaded.state, AgentState::Working);
        assert_eq!(
            reloaded.cached_remote_version.as_deref(),
            Some("9999.01.01.2")
        );
        assert_eq!(
            reloaded.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("test")
        );
    });
}

#[test]
fn compiled_rules_are_shared_until_manifest_reload() {
    with_manifest_dirs("shared-compiled-rules", || {
        write_remote_codex(&format!(
            "{}\nregex = ['^cached-[a-z]+$']\n",
            remote_manifest("9999.01.01.1", "blocked", "cached-ready")
        ));
        let first = load_manifest(Agent::Codex).unwrap();
        let second = load_manifest(Agent::Codex).unwrap();
        assert!(!first.compiled_rules.is_empty());
        assert_eq!(
            first.compiled_rules.as_ptr(),
            second.compiled_rules.as_ptr(),
            "cached loads must retain the same compiled rules and regex search caches"
        );

        write_remote_codex_without_reload(&format!(
            "{}\nregex = ['^new-[a-z]+$']\n",
            remote_manifest("9999.01.01.2", "working", "new-ready")
        ));
        let unchanged = load_manifest(Agent::Codex).unwrap();
        assert_eq!(
            first.compiled_rules.as_ptr(),
            unchanged.compiled_rules.as_ptr()
        );

        reload_manifests_for_agents(&[Agent::Codex]);
        let reloaded = load_manifest(Agent::Codex).unwrap();
        let shared_reload = load_manifest(Agent::Codex).unwrap();
        assert_ne!(
            first.compiled_rules.as_ptr(),
            reloaded.compiled_rules.as_ptr()
        );
        assert_eq!(
            reloaded.compiled_rules.as_ptr(),
            shared_reload.compiled_rules.as_ptr()
        );
        assert!(compiled_rule_matches(
            &first.compiled_rules[0],
            "cached-ready"
        ));
        assert!(!compiled_rule_matches(
            &first.compiled_rules[0],
            "new-ready"
        ));
        assert_eq!(
            explain(Agent::Codex, "new-ready").state,
            AgentState::Working
        );

        std::thread::scope(|scope| {
            for _ in 0..4 {
                let reloaded = &reloaded;
                scope.spawn(move || {
                    let loaded = load_manifest(Agent::Codex).unwrap();
                    assert_eq!(
                        loaded.compiled_rules.as_ptr(),
                        reloaded.compiled_rules.as_ptr()
                    );
                    for _ in 0..8 {
                        assert_eq!(detect(Agent::Codex, "new-ready").state, AgentState::Working);
                    }
                });
            }
        });
    });
}

#[test]
fn osc_regions_use_separate_inputs_and_share_rule_priority() {
    with_manifest_dirs("osc-regions", || {
        write_local_codex(&rules_manifest(
            r#"
[[rules]]
id = "screen"
state = "idle"
priority = 10
region = "whole_recent"
visible_idle = true
contains = ["screen-marker"]

[[rules]]
id = "title"
state = "working"
priority = 20
region = "osc_title"
visible_working = true
regex = ['^title-marker$']

[[rules]]
id = "progress"
state = "blocked"
priority = 30
region = "osc_progress"
visible_blocker = true
regex = ['^progress-marker$']
"#,
        ));
        for (screen, title, progress, state, rule) in [
            ("screen-marker", "", "", AgentState::Idle, "screen"),
            (
                "screen-marker",
                "title-marker",
                "",
                AgentState::Working,
                "title",
            ),
            (
                "screen-marker",
                "title-marker",
                "progress-marker",
                AgentState::Blocked,
                "progress",
            ),
            (
                "screen-marker title-marker progress-marker",
                "",
                "",
                AgentState::Idle,
                "screen",
            ),
        ] {
            let input = DetectionInput {
                screen,
                osc_title: title,
                osc_progress: progress,
            };
            let result = explain_with_input(Agent::Codex, input);
            assert_eq!(result.state, state);
            assert_eq!(
                result
                    .matched_rule
                    .as_ref()
                    .map(|matched| matched.id.as_str()),
                Some(rule)
            );
            let detection =
                crate::detect::detect_agent_with_osc(Some(Agent::Codex), screen, title, progress);
            assert_eq!(detection.state, state);
            assert_eq!(detection.visible_idle, state == AgentState::Idle);
            assert_eq!(detection.visible_working, state == AgentState::Working);
            assert_eq!(detection.visible_blocker, state == AgentState::Blocked);
        }
        let swapped = explain_with_input(
            Agent::Codex,
            DetectionInput {
                screen: "",
                osc_title: "progress-marker",
                osc_progress: "title-marker",
            },
        );
        assert!(swapped.matched_rule.is_none());
    });
}

#[test]
fn skip_rule_suppresses_state_update_without_visible_state_evidence() {
    with_manifest_dirs("skip-rule", || {
        write_local_codex(&rules_manifest(
            r#"
[[rules]]
id = "activity"
state = "working"
priority = 10
visible_working = true
contains = ["activity-marker"]

[[rules]]
id = "overlay"
state = "unknown"
priority = 20
skip_state_update = true
contains = ["overlay-marker"]
"#,
        ));
        let screen = "activity-marker overlay-marker";
        let result = explain(Agent::Codex, screen);
        assert_eq!(result.state, AgentState::Unknown);
        assert!(result.skip_state_update);
        assert_eq!(
            result.skipped_update_reason.as_deref(),
            Some("matched_rule:overlay")
        );
        assert!(!result.visible_idle);
        assert!(!result.visible_working);
        assert!(!result.visible_blocker);
        assert!(detect(Agent::Codex, screen).skip_state_update);
    });
}

#[test]
fn screen_regions_extract_structure_without_classifying_agent_state() {
    for (screen, spec, expected) in [
        ("old\n\nnew\n", "bottom_lines(2)", "\nnew\n"),
        (
            "before\n› input\nafter\n",
            "after_last_prompt_marker",
            "after\n",
        ),
        (
            "before\n› input\nafter\n",
            "before_current_prompt_marker",
            "before\n",
        ),
        (
            "before\n› input\nafter\n",
            "whole_recent_without_current_prompt_marker",
            "",
        ),
        (
            "no marker\n",
            "whole_recent_without_current_prompt_marker",
            "no marker\n",
        ),
        (
            "• old\n■ latest\n› input\n",
            "current_prompt_block_marker",
            "■ latest",
        ),
        (
            "• old\n■ latest\n› input\n",
            "after_current_prompt_block_marker",
            "■ latest\n› input\n",
        ),
        ("› old\n• new\n", "current_prompt_block_marker", ""),
        (
            "above\n\n───\nbody\n───\nfooter\n",
            "above_prompt_box",
            "above\n\n",
        ),
        (
            "above\n\n───\nbody\n───\nfooter\n",
            "last_non_empty_above_prompt_box",
            "above",
        ),
        (
            "above\n───\nbody\n───\nfooter\n",
            "prompt_box_body",
            "body\n",
        ),
        (
            "above\n───\nbody\n───\nfooter\n",
            "after_last_horizontal_rule",
            "footer\n",
        ),
    ] {
        assert_eq!(
            region(
                DetectionInput {
                    screen,
                    osc_title: "",
                    osc_progress: ""
                },
                spec
            ),
            expected,
            "region={spec}"
        );
    }
}

#[test]
fn all_bundled_manifests_parse_and_validate() {
    for agent in Agent::SCREEN_MANIFEST_AGENTS {
        assert!(
            bundled_manifest(agent).is_some(),
            "missing bundled manifest for {}",
            agent_label(agent)
        );
    }
}

#[test]
fn minimax_manifest_detects_idle_working_and_blocked_states() {
    with_manifest_dirs("minimax-states", || {
        // Working (thinking): the "Loading" footer replaces the input box.
        // All three markers — "Ctrl+X steer", "Ctrl+O details", "Esc stop" —
        // are present together only in this sub-state.
        let working_thinking = explain(
            Agent::MiniMax,
            "    └ Completed in 7s · ⚡ 105.2 tok/s\n\n   › list the contents of the /etc directory\n\n  └ • Thinking…\n\n    ⠸ Loading 1s · Enter queue · Ctrl+X steer · Ctrl+O details · Esc stop\n  ────────────────────────────────────────────────────────────────────────\n  ›\n  ────────────────────────────────────────────────────────────────────────\n\n  ~ │ ◇ … │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(working_thinking.state, AgentState::Working);
        assert!(working_thinking.visible_working);
        assert_eq!(
            working_thinking
                .matched_rule
                .as_ref()
                .map(|rule| rule.id.as_str()),
            Some("working_active_turn_footer")
        );

        // Working (running tool): the "Running" footer omits the steer/
        // details pair but still carries "Enter queue" + "Esc stop". The
        // rule must match this sub-state too — otherwise the agent
        // silently regresses to "idle" while a tool is in flight (the
        // input box is still visible, so the prompt_box_idle rule would
        // otherwise win).
        let working_running = explain(
            Agent::MiniMax,
            "  └ • Running  date · 1 output line\n\n    ⠸ Running 7s · ⚡ ~278.4 tok/s · Enter queue · Esc stop\n  ────────────────────────────────────────────────────────────────────────\n  ›\n  ────────────────────────────────────────────────────────────────────────\n\n  ~ │ ◇ … │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(working_running.state, AgentState::Working);
        assert_eq!(
            working_running
                .matched_rule
                .as_ref()
                .map(|rule| rule.id.as_str()),
            Some("working_active_turn_footer")
        );

        // Idle: empty prompt box between the two horizontal rules.
        let idle = explain(
            Agent::MiniMax,
            "    VirtualBox guest artifacts\n    - 7× .vboxclient-*.pid files\n\n    Anything specific you want to dig into?\n\n  ↓ End to latest · ↑ 75 earlier rows · PgUp · ↓ 1 newer row · PgDn\n\n    Message · Enter send · Shift+Enter newline\n  ────────────────────────────────────────────────────────────────────────\n  ›\n  ────────────────────────────────────────────────────────────────────────\n\n  ~ │ ◇ … │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(idle.state, AgentState::Idle);
        assert!(idle.visible_idle);
        assert_eq!(
            idle.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("prompt_box_idle")
        );

        // User composing input inside the prompt box; still idle.
        let idle_composing = explain(
            Agent::MiniMax,
            "    Want me to add that for you?\n\n  ↓ End to latest · ↑ 42 earlier rows · PgUp · ↓ 1 newer row · PgDn\n\n    Message · Enter send · Shift+Enter newline\n  ────────────────────────────────────────────────────────────────────────\n  ›  list files in\n  ────────────────────────────────────────────────────────────────────────\n\n  ~ │ ◇ … │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(idle_composing.state, AgentState::Idle);
        assert!(idle_composing.visible_idle);

        // Historical completion lines in the scrollback must not regress to
        // working state when the active-turn footer is absent.
        let historical = explain(
            Agent::MiniMax,
            "  ├ • Thought for 4.5s\n  └ • Running  cat /etc/passwd",
        );
        assert_eq!(historical.state, AgentState::Idle);
        assert!(!historical.visible_working);

        // Blocked: the permission request dialog appears when the user is in
        // a permission mode stricter than "Full access" (i.e. "Ask" or
        // "Auto"). The dialog is structured as a centered "◆ Approval
        // needed" header followed by a boxed action card with three options
        // (1 Allow for this conversation / 2 Always allow this action / 3
        // Deny) and navigation hints ("enter confirm · esc deny"). The
        // action label (e.g. "Bash", "Read") and question ("Run this
        // command?" / "Allow read?") vary by action type, but the option
        // labels and nav hints are stable across all action types.
        let blocked = explain(
            Agent::MiniMax,
            "   › show me the contents of /etc/passwd\n\n  ├ • Thought for 4.5s\n  └ • Running  cat /etc/passwd\n\n  ◆ Approval needed  bash\n\n  │ Approval needed                                                                     Bash\n  │ Run this command?\n  │   $ {\"command\":\"cat /etc/passwd\"}\n  │   Reason: Needs confirmation: filesystem path is outside the workspace or configured\n  │   allow paths. Path: /etc/passwd\n  │\n  │ → 1 Allow for this conversation  Ask again in a new conversation\n  │   2 Always allow this action     Review and save the exact scope\n  │   3 Deny                         Optionally tell MCode what to do instead\n  │\n  │ ↑/↓ select · 1/2/3 choose · enter confirm · esc deny · ctrl+c stop\n\n  ~ │ ◇ … │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(blocked.state, AgentState::Blocked);
        assert!(blocked.visible_blocker);
        assert_eq!(
            blocked.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("permission_prompt_blocked")
        );
    });
}

#[test]
fn minimax_manifest_detects_plan_mode_dialogs() {
    with_manifest_dirs("minimax-plan-mode", || {
        // 1. "Use Plan mode?" — entry confirmation in non-PLAN mode. The
        // agent offers to enter plan mode for a non-trivial task; the user
        // must confirm. Real v0.1.2 capture.
        let plan_confirm = explain(
            Agent::MiniMax,
            "  ● Let me enter Plan Mode so I can investigate the workspace, ask a couple of high-leverage design questions, and hand you a concrete, approval-ready plan first.\n\n  └ • Enterplanmode\n\n    └ Completed in 12s · ⚡ 99.2 tok/s\n\n  ╭─ Use Plan mode? ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╮\n  │ Plan mode structures complex tasks before execution.                                                                                                                    │\n  │                                                                                                                                                                         │\n  │ › Continue with plan                                                                                                                                                    │\n  │   Deny                                                                                                                                                                  │\n  ├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤\n  │ ↑/↓ select · Enter confirm · Esc deny                                                                                                                                   │\n  ╰─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯\n\n  ~ │ ◇ … │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(plan_confirm.state, AgentState::Blocked);
        assert!(plan_confirm.visible_blocker);
        assert_eq!(
            plan_confirm
                .matched_rule
                .as_ref()
                .map(|rule| rule.id.as_str()),
            Some("plan_mode_dialog_blocked")
        );

        // 2. "Ask ─ 0/3 answered" — multi-question clarification dialog in
        // PLAN mode. Real v0.1.2 capture (3 tabs: Worker deliver / Trigger
        // types / Service topolo).
        let ask_multi = explain(
            Agent::MiniMax,
            "  ● The workspace is essentially empty (just 3 unrelated .txt files), so this is a clean greenfield project. Before I write the plan, I need to lock down three high-leverage design decisions that will materially change the architecture.\n\n  ╭─ Ask ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── 0/3 answered ─╮\n  │  ● Worker deliver   ○ Trigger types    ○ Service topolo                                                                                                                 │\n  ├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤\n  │ How should the scheduler deliver jobs to handlers?                                                                                                                      │\n  │                                                                                                                                                                         │\n  │ › 1  Embedded handlers (Recommended)  Handlers are Spring beans registered with @JobHandler in the same JVM. Scheduler pulls from DB/Redis and invokes locally.         │\n  │   2  HTTP webhook push                Scheduler POSTs job payloads to registered HTTP endpoints.                                                                          │\n  │   3  Both (mixed)                     Default embedded; an explicit webhook handler type POSTs externally.                                                              │\n  │   4  Other…                           Others...                                                                                                                         │\n  ├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤\n  │ ↑↓ move · 1-4 select · Enter next · Tab/←/→ questions · Esc cancel                                                                                                      │\n  ╰─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯\n\n  ~ │ ◇ … │ PLAN │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(ask_multi.state, AgentState::Blocked);
        assert!(ask_multi.visible_blocker);
        assert_eq!(
            ask_multi.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("plan_mode_dialog_blocked")
        );

        // 3. "Ask ─ 1 of 1" — single-question clarification dialog in PLAN
        // mode. Real v0.1.2 capture. Footer is "Enter send" (vs. "Enter
        // next" in multi-question).
        let ask_single = explain(
            Agent::MiniMax,
            "  ● I'll ask you to make the call.\n\n    └ Completed in 13s · ⚡ 109.9 tok/s\n\n  ╭─ Ask ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── 1 of 1 ─╮\n  │ Auth strategy                                                                                                                                                           │\n  │ Which auth strategy should the new Spring Boot 3.5 service use?                                                                                                         │\n  │ Pick one. I'll update the plan to match.                                                                                                                                │\n  │                                                                                                                                                                         │\n  │ › 1  Spring Security + custom JWT          Service issues and validates its own JWTs (HMAC or RSA). No separate auth server. Simpler, fewer moving parts.               │\n  │   2  Spring Authorization Server (OAuth2)  Full OAuth2 Authorization Server with /oauth2/token, registered clients, scopes, grant types. Heavier, but standardized for  │\n  │                                            multi-client / third-party / open-platform use.                                                                              │\n  │   3  Other…                                Others...                                                                                                                    │\n  ├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤\n  │ ↑↓ move · 1-3 select · Enter send · Esc cancel                                                                                                                          │\n  ╰─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯\n\n  ~ │ ◇ … │ PLAN │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(ask_single.state, AgentState::Blocked);
        assert!(ask_single.visible_blocker);
        assert_eq!(
            ask_single
                .matched_rule
                .as_ref()
                .map(|rule| rule.id.as_str()),
            Some("plan_mode_dialog_blocked")
        );

        // 4. "Plan Review ─ Frozen Runtime snapshot" — plan approval gate
        // shown after the agent writes the plan and calls ExitPlanMode. The
        // "Frozen Runtime snapshot" subtitle is locale-stable chrome. Real
        // v0.1.2 capture.
        let plan_review = explain(
            Agent::MiniMax,
            "  ● The plan is fully written to the canonical file (31.9 KB, 14 numbered sections plus build/run notes). The ExitPlanMode tool keeps returning a queued work must drain error.\n\n  ╭─ Plan Review ───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────── Frozen Runtime snapshot ─╮\n  │ Lines 1-35 of 648 · wheel/PgUp/PgDn scroll                                                                                                                              │\n  ├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤\n  │ Plan ── implementation plan ──                                                                                                                        │\n  │                                                                                                                                                                         │\n  │ Plan complete. What would you like to do?                                                                                                                               │\n  │ › Agree and start implementation                                                                                                                                        │\n  │   Skip for now                                                                                                                                                          │\n  │   Add context to revise                                                                                                                                                 │\n  ├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤\n  │ ↑/↓ select · Enter confirm · Esc skip · PgUp/PgDn scroll                                                                                                                │\n  ╰─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯\n\n  ~ │ ◇ … │ PLAN │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(plan_review.state, AgentState::Blocked);
        assert!(plan_review.visible_blocker);
        assert_eq!(
            plan_review
                .matched_rule
                .as_ref()
                .map(|rule| rule.id.as_str()),
            Some("plan_mode_dialog_blocked")
        );

        // 5. Sanity: when ONLY a working footer is present (no dialog), the
        // rule must NOT match — that's working, not blocked.
        let working_only = explain(
            Agent::MiniMax,
            "  ⠸ Running 7s · ⚡ ~278.4 tok/s · Enter queue · Esc stop\n  ────────────────────────────────────────────────────────────────────────\n  ›\n  ────────────────────────────────────────────────────────────────────────\n\n  ~ │ ◇ … │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(working_only.state, AgentState::Working);

        // 6. Anti-regression: idle chat text containing bare
        // "answered" (substring of "unanswered") and " of 1 " (a
        // common English phrase) must NOT match the rule. The
        // chrome anchor "ask ─" (Ask followed by U+2500) is the
        // only Ask-related needle, so free-form prose never fires
        // it. The screen ends with the prompt box `›` so the
        // fallback prompt_box_idle rule should win.
        let false_positive_bare = explain(
            Agent::MiniMax,
            "  ● 1 of 1 user reply was unanswered, will follow up tomorrow.

        Message · Enter send · Shift+Enter newline
      ────────────────────────────────────────────────────────────────────────
      ›
      ────────────────────────────────────────────────────────────────────────

      ~ │ ◇ … │ Ask │ ✦ MiniMax-M3 · Thinking On",
        );
        assert_eq!(false_positive_bare.state, AgentState::Idle);
    });
}

#[test]
fn manifest_validation_rejects_unknown_fields_empty_rules_invalid_regions_and_regexes() {
    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "typo"
state = "working"
contain = ["Working"]
"#
    )
    .is_err());
    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "empty"
state = "working"
"#
    )
    .is_err());
    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "bad_region"
state = "working"
region = "after_last_promt_marker"
contains = ["Working"]
"#
    )
    .is_err());
    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "bad_regex"
state = "working"
regex = ["["]
"#
    )
    .is_err());
    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "bad_nested_regex"
state = "working"
any = [{ line_regex = ["["] }]
"#
    )
    .is_err());
}

#[test]
fn manifest_validation_keeps_skip_rules_neutral() {
    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "bad_skip_state"
state = "idle"
skip_state_update = true
contains = ["menu"]
"#
    )
    .is_err());
    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "bad_skip_visible"
state = "unknown"
skip_state_update = true
visible_blocker = true
contains = ["menu"]
"#
    )
    .is_err());
}

#[test]
fn manifest_validation_rejects_excessive_rule_count() {
    let mut manifest = String::from(
        r#"
id = "codex"
"#,
    );
    for index in 0..129 {
        manifest.push_str(&format!(
            r#"
[[rules]]
id = "rule_{index}"
state = "idle"
contains = ["ready"]
"#
        ));
    }
    assert!(parse_manifest(&manifest).is_err());
}

#[test]
fn manifest_validation_rejects_excessive_gate_depth() {
    let manifest = r#"
id = "codex"

[[rules]]
id = "deep"
state = "idle"
contains = ["ready"]
all = [
  { contains = ["1"], all = [
    { contains = ["2"], all = [
      { contains = ["3"], all = [
        { contains = ["4"], all = [
          { contains = ["5"], all = [
            { contains = ["6"], all = [
              { contains = ["7"], all = [
                { contains = ["8"], all = [
                  { contains = ["9"] },
                ] },
              ] },
            ] },
          ] },
        ] },
      ] },
    ] },
  ] },
]
"#;
    assert!(parse_manifest(manifest).is_err());
}

#[test]
fn manifest_validation_rejects_excessive_matchers() {
    let matchers = (0..33)
        .map(|index| format!(r#""m{index}""#))
        .collect::<Vec<_>>()
        .join(", ");
    let manifest = format!(
        r#"
id = "codex"

[[rules]]
id = "many"
state = "idle"
contains = [{matchers}]
"#
    );
    assert!(parse_manifest(&manifest).is_err());
}

#[test]
fn bottom_non_empty_lines_uses_bottom_occurrence_for_repeated_text() {
    let content = "marker\nold\n\nmiddle\nmarker\nnew\n";
    assert_eq!(
        region(
            DetectionInput {
                screen: content,
                osc_title: "",
                osc_progress: ""
            },
            "bottom_non_empty_lines(2)"
        ),
        "marker\nnew\n"
    );
}

#[test]
fn top_non_empty_lines_uses_top_occurrence_for_repeated_text() {
    let content = "\nmarker\nold\n\nmiddle\nmarker\nnew\n";
    assert_eq!(
        region(
            DetectionInput {
                screen: content,
                osc_title: "",
                osc_progress: ""
            },
            "top_non_empty_lines(2)"
        ),
        "\nmarker\nold\n"
    );
}

#[test]
fn top_non_empty_lines_requires_a_canonical_positive_bounded_count() {
    let name = "top_non_empty_lines";
    assert!(validate_region_name(&format!("{name}(1)")).is_ok());
    assert!(validate_region_name(&format!("{name}({})", u16::MAX)).is_ok());
    for count in ["0", "01", "+1", "65536", "999999999999999999999999"] {
        assert!(
            validate_region_name(&format!("{name}({count})")).is_err(),
            "{name} accepted invalid count {count}"
        );
    }
}

#[test]
fn top_non_empty_lines_requires_engine_three_when_declared() {
    let manifest = r#"
id = "codex"
version = "1"
min_engine_version = 2

[[rules]]
id = "background"
state = "working"
region = " top_non_empty_lines(1) "
contains = ["active"]
"#;
    assert!(parse_manifest(manifest).is_err());
}
