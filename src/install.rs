use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;

const DESKTOP: &str = include_str!("../assets/hyper-sync.desktop");
const SERVICE: &str = include_str!("../systemd/hyper-sync.service");

#[derive(clap::Subcommand, Clone, Copy)]
pub enum InstallTarget {
    /// systemd user service (starts on login)
    Service,
    /// Session autostart desktop entry only
    App,
}

pub fn run_install(target: InstallTarget) -> anyhow::Result<()> {
    match target {
        InstallTarget::App => install_app(),
        InstallTarget::Service => install_service(),
    }
}

pub fn run_uninstall(target: InstallTarget) -> anyhow::Result<()> {
    match target {
        InstallTarget::App => uninstall_app(),
        InstallTarget::Service => uninstall_service(),
    }
}

fn xdg_config_home() -> anyhow::Result<PathBuf> {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| {
            std::env::var("HOME").map(|h| PathBuf::from(h).join(".config"))
        })
        .context("HOME or XDG_CONFIG_HOME required")
}

fn autostart_path() -> anyhow::Result<PathBuf> {
    Ok(xdg_config_home()?.join("autostart/hyper-sync.desktop"))
}

fn service_unit_path() -> anyhow::Result<PathBuf> {
    Ok(xdg_config_home()?.join("systemd/user/hyper-sync.service"))
}

fn install_app() -> anyhow::Result<()> {
    let path = autostart_path()?;
    write_file(&path, DESKTOP)?;
    eprintln!("installed {}", path.display());
    Ok(())
}

fn uninstall_app() -> anyhow::Result<()> {
    let path = autostart_path()?;
    remove_file_if_exists(&path)?;
    eprintln!("removed {}", path.display());
    Ok(())
}

fn install_service() -> anyhow::Result<()> {
    let path = service_unit_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    write_file(&path, SERVICE)?;
    systemctl(&["--user", "daemon-reload"])?;
    systemctl(&["--user", "enable", "--now", "hyper-sync.service"])?;
    eprintln!("installed and started {}", path.display());
    Ok(())
}

fn uninstall_service() -> anyhow::Result<()> {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "hyper-sync.service"])
        .status();
    let path = service_unit_path()?;
    remove_file_if_exists(&path)?;
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    eprintln!("removed {}", path.display());
    Ok(())
}

fn write_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove {}", path.display())),
    }
}

fn systemctl(args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new("systemctl")
        .args(args)
        .status()
        .with_context(|| format!("run systemctl {}", args.join(" ")))?;
    anyhow::ensure!(
        status.success(),
        "systemctl {} failed (exit {:?})",
        args.join(" "),
        status.code()
    );
    Ok(())
}
