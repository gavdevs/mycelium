#![cfg(target_os = "linux")]
#![allow(clippy::duplicated_attributes)]
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

const TEMPLATE: &str = include_str!("mycel.service.tmpl");

fn home() -> Result<PathBuf> {
    dirs::home_dir().context("no home")
}
fn unit_path() -> Result<PathBuf> {
    let p = home()?.join(".config/systemd/user/mycel.service");
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(p)
}
fn log_path() -> Result<PathBuf> {
    let p = home()?.join(".cache/mycel/daemon.log");
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(p)
}
fn binary_path() -> Result<String> {
    let cur = std::env::current_exe()?;
    Ok(cur.with_file_name("mycel-daemon").to_string_lossy().into_owned())
}
fn mise_shims_path() -> Result<String> {
    Ok(home()?
        .join(".local/share/mise/shims")
        .to_string_lossy()
        .into_owned())
}

pub fn install() -> Result<()> {
    let unit = TEMPLATE
        .replace("__BINARY_PATH__", &binary_path()?)
        .replace("__LOG_PATH__", &log_path()?.to_string_lossy())
        .replace("__MISE_SHIMS__", &mise_shims_path()?);
    std::fs::write(unit_path()?, unit)?;
    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()?;
    Command::new("systemctl")
        .args(["--user", "enable", "--now", "mycel.service"])
        .status()?;
    println!("installed and started");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    Command::new("systemctl")
        .args(["--user", "disable", "--now", "mycel.service"])
        .status()?;
    let _ = std::fs::remove_file(unit_path()?);
    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()?;
    println!("uninstalled");
    Ok(())
}

pub fn start() -> Result<()> {
    Command::new("systemctl")
        .args(["--user", "start", "mycel.service"])
        .status()?;
    Ok(())
}

pub fn stop() -> Result<()> {
    Command::new("systemctl")
        .args(["--user", "stop", "mycel.service"])
        .status()?;
    Ok(())
}

pub fn status() -> Result<()> {
    Command::new("systemctl")
        .args(["--user", "status", "mycel.service"])
        .status()?;
    Ok(())
}
