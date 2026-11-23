use super::*;
use serde_json::json;

fn sample_run() -> Result<Run, serde_json::Error> {
    serde_json::from_value(json!({
        "id": "run-test-1",
        "threadId": "th-test-1",
        "ordinal": 1,
        "providerInstanceId": "devReplay",
        "modelSelection": { "instanceId": "devReplay", "model": "showcase" },
        "providerThreadId": null,
        "userMessageId": "msg-test-1",
        "rootNodeId": null,
        "activeAttemptId": null,
        "status": "running",
        "requestedAt": "2026-11-22T00:00:00.000Z",
        "startedAt": null,
        "completedAt": null,
        "checkpointId": null,
        "contextHandoffId": null
    }))
}

#[test]
fn resolves_dev_replay_before_legacy_acp_settings() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let settings_path = dir.path().join("settings.json");
    std::fs::write(
        &settings_path,
        json!({
            "providerInstances": {
                "devReplay": {
                    "driver": "acpRegistry",
                    "config": { "source": "local", "commandPath": "node", "commandArgs": ["x.ts"] }
                }
            }
        })
        .to_string(),
    )?;
    let run = sample_run()?;
    let resolved = resolve_dynamic_driver(
        "devReplay",
        &run,
        RuntimeMode::FullAccess,
        Some(&settings_path),
    )?;
    assert!(matches!(resolved, ProviderDriverMode::DevReplay));
    Ok(())
}

#[test]
fn resolves_acp_registry_command_path_and_command_args() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let fake_bin = dir.path().join("custom-acp.exe");
    std::fs::write(&fake_bin, b"stub")?;
    let settings_path = dir.path().join("settings.json");
    std::fs::write(
        &settings_path,
        json!({
            "providerInstances": {
                "custom-acp": {
                    "driver": "acpRegistry",
                    "config": {
                        "source": "local",
                        "commandPath": fake_bin,
                        "commandArgs": ["--acp", "--stdio"]
                    }
                }
            }
        })
        .to_string(),
    )?;
    let run = sample_run()?;
    let resolved = resolve_dynamic_driver(
        "custom-acp",
        &run,
        RuntimeMode::FullAccess,
        Some(&settings_path),
    )?;
    match resolved {
        ProviderDriverMode::AcpCli(spec) => {
            assert_eq!(spec.args, vec!["--acp", "--stdio"]);
        }
        other => return Err(format!("expected AcpCli, got {other:?}").into()),
    }
    Ok(())
}
