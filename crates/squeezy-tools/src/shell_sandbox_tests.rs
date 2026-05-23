use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::*;

static WORKSPACE_NONCE: AtomicU64 = AtomicU64::new(0);

fn temp_workspace(name: &str) -> PathBuf {
    let base = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let counter = WORKSPACE_NONCE.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "squeezy_shell_sandbox_{name}_{pid}_{base}_{counter}",
        pid = std::process::id()
    ));
    fs::create_dir_all(&root).expect("create temp workspace");
    root
}

fn config(mode: ShellSandboxMode, network: ShellSandboxNetworkPolicy) -> ShellSandboxConfig {
    ShellSandboxConfig {
        mode,
        network,
        ..ShellSandboxConfig::default()
    }
}

fn test_registry(root: &Path, shell_sandbox: ShellSandboxConfig) -> ToolRegistry {
    ToolRegistry::new_inner(
        root,
        ToolOutputConfig::default(),
        WebToolConfig::default(),
        shell_sandbox,
        SkillCatalog::empty(),
        CrawlOptions::default(),
        Arc::new(Redactor::default()),
    )
    .expect("registry")
}

fn fake_plan(backend: &'static str, required: bool) -> ShellSandboxPlan {
    ShellSandboxPlan {
        program: "sh".to_string(),
        args: vec!["-lc".to_string(), "true".to_string()],
        backend,
        mode: if required { "required" } else { "best_effort" },
        network: "denied",
        required,
    }
}

fn sandbox_unavailable_denial(result: &ToolResult) -> bool {
    result.status == ToolStatus::Denied
        && result.content["error"]
            .as_str()
            .is_some_and(|error| error.contains("required shell sandbox"))
}

fn prepare_with_probes(
    command: &str,
    config: &ShellSandboxConfig,
    macos_available: bool,
    linux_available: bool,
) -> std::result::Result<ShellSandboxPlan, String> {
    let analysis = analyze_shell_command(command);
    prepare_shell_sandbox_plan_with_probe(
        command,
        &analysis,
        Path::new("/tmp"),
        config,
        macos_available,
        linux_available,
    )
}

#[test]
fn shell_sandbox_plan_mode_off_returns_direct() {
    let plan = prepare_with_probes(
        "printf ok",
        &config(
            ShellSandboxMode::Off,
            ShellSandboxNetworkPolicy::DenyByDefault,
        ),
        true,
        true,
    )
    .expect("plan");

    assert_eq!(plan.backend, "none");
    assert_eq!(plan.mode, "off");
    assert_eq!(plan.program, "sh");
    assert!(!plan.required);
}

#[test]
#[cfg(target_os = "macos")]
fn shell_sandbox_plan_required_when_sandbox_exec_absent() {
    let err = prepare_with_probes(
        "printf ok",
        &config(
            ShellSandboxMode::Required,
            ShellSandboxNetworkPolicy::DenyByDefault,
        ),
        false,
        true,
    )
    .expect_err("required mode must fail closed");

    assert!(err.contains("/usr/bin/sandbox-exec not found"));
}

#[test]
#[cfg(target_os = "macos")]
fn shell_sandbox_plan_best_effort_when_sandbox_exec_absent() {
    let plan = prepare_with_probes(
        "printf ok",
        &config(
            ShellSandboxMode::BestEffort,
            ShellSandboxNetworkPolicy::DenyByDefault,
        ),
        false,
        true,
    )
    .expect("best effort falls back");

    assert_eq!(plan.backend, "none");
    assert_eq!(plan.mode, "best_effort");
}

#[test]
#[cfg(target_os = "linux")]
fn shell_sandbox_plan_required_when_userns_unavailable() {
    let err = prepare_with_probes(
        "printf ok",
        &config(
            ShellSandboxMode::Required,
            ShellSandboxNetworkPolicy::DenyByDefault,
        ),
        true,
        false,
    )
    .expect_err("required mode must fail closed");

    assert!(err.contains("required shell sandbox unavailable: linux unshare"));
}

#[test]
#[cfg(target_os = "linux")]
fn shell_sandbox_plan_best_effort_when_userns_unavailable() {
    let plan = prepare_with_probes(
        "printf ok",
        &config(
            ShellSandboxMode::BestEffort,
            ShellSandboxNetworkPolicy::DenyByDefault,
        ),
        true,
        false,
    )
    .expect("best effort falls back");

    assert_eq!(plan.backend, "none");
    assert_eq!(plan.mode, "best_effort");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn shell_sandbox_plan_network_posture_allow_when_approved() {
    let plan = prepare_with_probes(
        "curl https://example.com",
        &config(
            ShellSandboxMode::Required,
            ShellSandboxNetworkPolicy::AllowWhenApproved,
        ),
        true,
        true,
    )
    .expect("plan");

    assert_eq!(plan.network, "allowed_approved");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn shell_sandbox_plan_network_posture_denied_classified() {
    let plan = prepare_with_probes(
        "curl https://example.com",
        &config(
            ShellSandboxMode::Required,
            ShellSandboxNetworkPolicy::DenyByDefault,
        ),
        true,
        true,
    )
    .expect("plan");

    assert_eq!(plan.network, "denied_classified");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn shell_sandbox_plan_network_posture_denied_non_network() {
    let plan = prepare_with_probes(
        "printf ok",
        &config(
            ShellSandboxMode::Required,
            ShellSandboxNetworkPolicy::AllowWhenApproved,
        ),
        true,
        true,
    )
    .expect("plan");

    assert_eq!(plan.network, "denied");
}

#[test]
fn shell_sandbox_runtime_unavailable_detects_macos_exit_71_with_sandbox_apply() {
    let plan = fake_plan("macos-sandbox-exec", true);

    assert!(shell_sandbox_runtime_unavailable_with_probe(
        &plan,
        Some(71),
        "sandbox_apply: Operation not permitted",
        true,
    ));
}

#[test]
fn shell_sandbox_runtime_unavailable_detects_linux_exit_1_empty_stderr_when_userns_gone() {
    let plan = fake_plan("linux-direct-syscalls", true);

    assert!(shell_sandbox_runtime_unavailable_with_probe(
        &plan,
        Some(1),
        "",
        false,
    ));
}

#[test]
fn shell_sandbox_runtime_unavailable_ignores_nonzero_exit_with_stderr() {
    let linux_plan = fake_plan("linux-direct-syscalls", true);
    let macos_plan = fake_plan("macos-sandbox-exec", true);

    assert!(!shell_sandbox_runtime_unavailable_with_probe(
        &linux_plan,
        Some(1),
        "command failed",
        false,
    ));
    assert!(!shell_sandbox_runtime_unavailable_with_probe(
        &macos_plan,
        Some(71),
        "ordinary exit",
        true,
    ));
}

#[test]
fn shell_sandbox_runtime_unavailable_ignores_direct_backend() {
    let plan = fake_plan("none", true);

    assert!(!shell_sandbox_runtime_unavailable_with_probe(
        &plan,
        Some(1),
        "",
        false,
    ));
}

#[tokio::test]
#[cfg(target_os = "macos")]
async fn shell_sandbox_exec_runs_benign_command_with_required_mode() {
    if !Path::new("/usr/bin/sandbox-exec").exists() {
        eprintln!("SKIP: /usr/bin/sandbox-exec not present");
        return;
    }

    let root = temp_workspace("macos_required");
    let registry = test_registry(
        &root,
        config(
            ShellSandboxMode::Required,
            ShellSandboxNetworkPolicy::DenyByDefault,
        ),
    );

    let result = registry
        .execute(
            ToolCall {
                call_id: "shell".to_string(),
                name: "shell".to_string(),
                arguments: json!({
                    "command": "printf ok",
                    "description": "check macOS sandbox activation"
                }),
            },
            CancellationToken::new(),
        )
        .await;

    if sandbox_unavailable_denial(&result) {
        eprintln!("SKIP: macOS sandbox backend unavailable at runtime");
        let _ = fs::remove_dir_all(root);
        return;
    }

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(result.content["stdout"], "ok");
    assert_eq!(result.content["sandbox"]["mode"], "required");
    assert_eq!(result.content["sandbox"]["backend"], "macos-sandbox-exec");
    assert_eq!(result.content["sandbox"]["network"], "denied");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
#[cfg(target_os = "macos")]
async fn shell_sandbox_exec_result_carries_network_metadata() {
    if !Path::new("/usr/bin/sandbox-exec").exists() {
        eprintln!("SKIP: /usr/bin/sandbox-exec not present");
        return;
    }

    let root = temp_workspace("macos_network_metadata");
    let registry = test_registry(
        &root,
        config(
            ShellSandboxMode::Required,
            ShellSandboxNetworkPolicy::DenyByDefault,
        ),
    );

    let result = registry
        .execute(
            ToolCall {
                call_id: "shell".to_string(),
                name: "shell".to_string(),
                arguments: json!({
                    "command": "curl --version",
                    "description": "check network metadata"
                }),
            },
            CancellationToken::new(),
        )
        .await;

    if sandbox_unavailable_denial(&result) {
        eprintln!("SKIP: macOS sandbox backend unavailable at runtime");
        let _ = fs::remove_dir_all(root);
        return;
    }

    assert_eq!(result.content["policy"]["network"], "classified");
    assert_eq!(result.content["sandbox"]["mode"], "required");
    assert_eq!(result.content["sandbox"]["backend"], "macos-sandbox-exec");
    assert_eq!(result.content["sandbox"]["network"], "denied_classified");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn shell_linux_userns_runs_benign_command_with_required_mode() {
    if !linux_unshare_supported() {
        eprintln!("SKIP: linux unshare not supported");
        return;
    }

    let root = temp_workspace("linux_required");
    let registry = test_registry(
        &root,
        config(
            ShellSandboxMode::Required,
            ShellSandboxNetworkPolicy::DenyByDefault,
        ),
    );

    let result = registry
        .execute(
            ToolCall {
                call_id: "shell".to_string(),
                name: "shell".to_string(),
                arguments: json!({
                    "command": "printf ok",
                    "description": "check Linux sandbox activation"
                }),
            },
            CancellationToken::new(),
        )
        .await;

    if sandbox_unavailable_denial(&result) {
        eprintln!("SKIP: Linux sandbox backend unavailable at runtime");
        let _ = fs::remove_dir_all(root);
        return;
    }

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(result.content["stdout"], "ok");
    assert_eq!(result.content["sandbox"]["mode"], "required");
    assert_eq!(
        result.content["sandbox"]["backend"],
        "linux-direct-syscalls"
    );
    assert_eq!(result.content["sandbox"]["network"], "denied");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn shell_linux_userns_result_carries_network_metadata() {
    if !linux_unshare_supported() {
        eprintln!("SKIP: linux unshare not supported");
        return;
    }

    let root = temp_workspace("linux_network_metadata");
    let registry = test_registry(
        &root,
        config(
            ShellSandboxMode::Required,
            ShellSandboxNetworkPolicy::DenyByDefault,
        ),
    );

    let result = registry
        .execute(
            ToolCall {
                call_id: "shell".to_string(),
                name: "shell".to_string(),
                arguments: json!({
                    "command": "curl --version",
                    "description": "check network metadata"
                }),
            },
            CancellationToken::new(),
        )
        .await;

    if sandbox_unavailable_denial(&result) {
        eprintln!("SKIP: Linux sandbox backend unavailable at runtime");
        let _ = fs::remove_dir_all(root);
        return;
    }

    assert_eq!(result.content["policy"]["network"], "classified");
    assert_eq!(result.content["sandbox"]["mode"], "required");
    assert_eq!(
        result.content["sandbox"]["backend"],
        "linux-direct-syscalls"
    );
    assert_eq!(result.content["sandbox"]["network"], "denied_classified");
    let _ = fs::remove_dir_all(root);
}
