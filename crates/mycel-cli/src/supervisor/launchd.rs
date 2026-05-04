#![cfg(target_os = "macos")]
#![allow(clippy::duplicated_attributes)]
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

const TEMPLATE: &str = include_str!("launchd.plist.tmpl");

fn home() -> Result<PathBuf> {
    dirs::home_dir().context("no home")
}
fn plist_path() -> Result<PathBuf> {
    Ok(home()?.join("Library/LaunchAgents/com.gav.mycel.plist"))
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
    let plist = TEMPLATE
        .replace("__BINARY_PATH__", &binary_path()?)
        .replace("__LOG_PATH__", &log_path()?.to_string_lossy())
        .replace("__MISE_SHIMS__", &mise_shims_path()?);
    std::fs::write(plist_path()?, plist)?;
    let status = Command::new("launchctl")
        .args(["load", plist_path()?.to_str().unwrap()])
        .status()?;
    anyhow::ensure!(status.success(), "launchctl load failed");
    println!("installed and loaded");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let _ = Command::new("launchctl")
        .args(["unload", plist_path()?.to_str().unwrap()])
        .status();
    let _ = std::fs::remove_file(plist_path()?);
    println!("uninstalled");
    Ok(())
}

pub fn start() -> Result<()> {
    let status = Command::new("launchctl")
        .args(["start", "com.gav.mycel"])
        .status()?;
    anyhow::ensure!(status.success(), "launchctl start failed");
    Ok(())
}

pub fn stop() -> Result<()> {
    let status = Command::new("launchctl")
        .args(["stop", "com.gav.mycel"])
        .status()?;
    anyhow::ensure!(status.success(), "launchctl stop failed");
    Ok(())
}

pub fn status() -> Result<()> {
    Command::new("launchctl")
        .args(["list", "com.gav.mycel"])
        .status()?;
    Ok(())
}
