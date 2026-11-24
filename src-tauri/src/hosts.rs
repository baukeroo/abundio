//! **Hosts** the user added (docs/plans/remote-hosts.md phase 7d): a name and
//! how to reach it, kept by the desktop app in `hosts.json` beside its data
//! (machine settings, like an ssh alias; not part of any Profile). A Docker
//! Host is created locally with a persistent named volume.

use std::process::Command;
use std::path::PathBuf;
use std::time::Duration;

use abundio_core::host_setup;
use serde::{Deserialize, Serialize};

use crate::error::AbundioError;
use crate::host_router::HostTarget;

const DOCKER_IMAGE: &str = "node:22-bookworm";
const DOCKER_CREATE_LIMIT: Duration = Duration::from_secs(30 * 60);
const DOCKER_COMMAND_LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostEntry {
    pub id: String,
    pub name: String,
    pub target: HostTarget,
}

fn path() -> PathBuf {
    crate::app_paths::shared_root().join("hosts.json")
}

pub fn load() -> Vec<HostEntry> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(hosts: &[HostEntry]) -> Result<(), AbundioError> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(hosts).map_err(|e| AbundioError::InvalidOperation(e.to_string()))?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

pub fn find(id: &str) -> Option<HostEntry> {
    load().into_iter().find(|h| h.id == id)
}

/// Add a Host: a name, and the ssh destination or test server that reaches it.
fn add(hosts: &mut Vec<HostEntry>, name: &str, target: HostTarget) -> Result<HostEntry, AbundioError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AbundioError::InvalidOperation("a Host needs a name".into()));
    }
    if hosts.iter().any(|h| h.name.eq_ignore_ascii_case(name)) {
        return Err(AbundioError::InvalidOperation(format!("there is already a Host named {name:?}")));
    }
    if let HostTarget::Ssh { destination } = &target {
        let d = destination.trim();
        // Never a word ssh would read as an option, nor a command line.
        if d.is_empty() || d.starts_with('-') || d.chars().any(char::is_whitespace) {
            return Err(AbundioError::InvalidOperation(
                "the ssh destination must be an alias from ~/.ssh/config or user@host".into(),
            ));
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let target = match target {
        HostTarget::Docker { container } if container.is_empty() => {
            HostTarget::Docker { container: format!("abundio-host-{}", id.replace('-', "")) }
        }
        HostTarget::Docker { .. } => {
            return Err(AbundioError::InvalidOperation("Docker container names are assigned by Abundio".into()));
        }
        other => other,
    };
    let entry = HostEntry { id, name: name.to_string(), target };
    hosts.push(entry.clone());
    Ok(entry)
}

fn create_docker_container(target: &HostTarget) -> Result<(), AbundioError> {
    let HostTarget::Docker { container } = target else {
        return Ok(());
    };
    let command = docker_run_command(container);
    host_setup::run(command, Box::new(std::io::empty()), DOCKER_CREATE_LIMIT)
        .map_err(AbundioError::Channel)?;
    if let Err(error) = target.ensure_ready() {
        let cleanup = remove_created_docker_container(target);
        return Err(match cleanup {
            Ok(()) => AbundioError::Channel(error),
            Err(cleanup) => AbundioError::InvalidOperation(format!(
                "{error}; could not clean up the new Docker Host: {cleanup}"
            )),
        });
    }
    log::info!("[host] created Docker container {container}");
    Ok(())
}

fn docker_run_command(container: &str) -> Command {
    let volume = format!("{container}-data");
    let mut command = Command::new("docker");
    command
        .args([
            "run",
            "--detach",
            "--init",
            "--name",
            container,
            "--hostname",
            "abundio-host",
            "--restart",
            "unless-stopped",
            "--mount",
        ])
        .arg(format!("type=volume,source={volume},target=/root"))
        .args([
            "--user",
            "0:0",
            "--workdir",
            "/root",
            "--env",
            "HOME=/root",
            "--env",
            "SHELL=/bin/bash",
            "--entrypoint",
            "/bin/sh",
            DOCKER_IMAGE,
            "-c",
            "exec tail -f /dev/null",
        ]);
    command
}

fn remove_created_docker_container(target: &HostTarget) -> Result<(), String> {
    let HostTarget::Docker { container } = target else {
        return Ok(());
    };
    let mut remove = Command::new("docker");
    remove.args(["rm", "--force", container]);
    host_setup::run(remove, Box::new(std::io::empty()), DOCKER_COMMAND_LIMIT)?;

    let volume = format!("{container}-data");
    let mut remove_volume = Command::new("docker");
    remove_volume.args(["volume", "rm", &volume]);
    host_setup::run(remove_volume, Box::new(std::io::empty()), DOCKER_COMMAND_LIMIT)?;
    Ok(())
}

#[tauri::command]
pub fn hosts_list() -> Vec<HostEntry> {
    load()
}

#[tauri::command]
pub async fn hosts_add(app: tauri::AppHandle, name: String, target: HostTarget) -> Result<HostEntry, AbundioError> {
    use tauri::Emitter;
    tauri::async_runtime::spawn_blocking(move || {
        let mut hosts = load();
        let entry = add(&mut hosts, &name, target)?;
        create_docker_container(&entry.target)?;
        if let Err(error) = save(&hosts) {
            if entry.target.is_docker() {
                if let Err(cleanup) = remove_created_docker_container(&entry.target) {
                    return Err(AbundioError::InvalidOperation(format!(
                        "{error}; could not clean up the new Docker Host: {cleanup}"
                    )));
                }
            }
            return Err(error);
        }
        let _ = app.emit("hosts-changed", ());
        Ok(entry)
    })
    .await
    .map_err(|e| AbundioError::InvalidOperation(e.to_string()))?
}

#[tauri::command]
pub fn hosts_remove(app: tauri::AppHandle, id: String) -> Result<(), AbundioError> {
    use tauri::Emitter;
    let mut hosts = load();
    hosts.retain(|h| h.id != id);
    save(&hosts)?;
    let _ = app.emit("hosts-changed", ());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh(d: &str) -> HostTarget {
        HostTarget::Ssh { destination: d.into() }
    }

    #[test]
    fn adds_a_host_with_a_name_of_its_own_and_a_plain_destination() {
        let mut hosts = Vec::new();
        let a = add(&mut hosts, " Build box ", ssh("me@build")).unwrap();
        assert_eq!(a.name, "Build box");
        assert!(add(&mut hosts, "build BOX", ssh("x")).is_err());
        assert!(add(&mut hosts, "", ssh("x")).is_err());
        for bad in ["", "-oProxyCommand=evil", "me@box rm -rf"] {
            assert!(add(&mut hosts, "Other", ssh(bad)).is_err(), "{bad:?}");
        }
        assert_eq!(hosts.len(), 1);
    }

    #[test]
    fn assigns_a_safe_container_name_to_a_new_docker_host() {
        let mut hosts = Vec::new();
        let host = add(&mut hosts, "Local container", HostTarget::Docker { container: String::new() }).unwrap();
        let HostTarget::Docker { container } = host.target else {
            panic!("expected a Docker target");
        };
        assert!(container.starts_with("abundio-host-"));
        assert!(container.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
        assert!(add(&mut hosts, "Other", HostTarget::Docker { container }).is_err());
    }

    #[test]
    fn docker_hosts_use_a_persistent_volume_and_restart_policy() {
        let command = docker_run_command("abundio-host-123");
        assert_eq!(command.get_program(), "docker");
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.windows(2).any(|w| w == ["--restart", "unless-stopped"]));
        assert!(args.iter().any(|a| a == "type=volume,source=abundio-host-123-data,target=/root"));
        assert!(args.iter().any(|a| a == DOCKER_IMAGE));
        assert!(args.windows(2).any(|w| w == ["-c", "exec tail -f /dev/null"]));
    }

    #[test]
    fn reads_what_the_settings_page_sends() {
        let e: HostEntry = serde_json::from_str(
            r#"{"id":"1","name":"Box","target":{"kind":"ssh","destination":"me@box"}}"#,
        )
        .unwrap();
        assert_eq!(e.target, ssh("me@box"));
    }
}
