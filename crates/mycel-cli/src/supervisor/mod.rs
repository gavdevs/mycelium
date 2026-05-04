use crate::cli::DaemonAction;
use anyhow::Result;
use camino::Utf8PathBuf;
use std::path::PathBuf;
use std::process::Command;

#[cfg(target_os = "macos")]
mod launchd;
#[cfg(target_os = "linux")]
mod systemd;

pub async fn dispatch(action: DaemonAction) -> Result<()> {
    match action {
        DaemonAction::Install { repo } => {
            let repo = match repo {
                Some(p) => p,
                None => {
                    let cwd = std::env::current_dir()?;
                    Utf8PathBuf::from_path_buf(cwd)
                        .map_err(|_| anyhow::anyhow!("non-utf8 cwd"))?
                }
            };
            platform_install(&repo)
        }
        DaemonAction::Uninstall => platform_uninstall(),
        DaemonAction::Start => platform_start(),
        DaemonAction::Stop => platform_stop(),
        DaemonAction::Status => platform_status(),
        DaemonAction::Logs { follow } => tail_logs(follow),
        DaemonAction::Run => run_foreground().await,
    }
}

#[cfg(target_os = "macos")]
fn platform_install(repo: &camino::Utf8Path) -> Result<()> {
    launchd::install(repo)
}
#[cfg(target_os = "linux")]
fn platform_install(repo: &camino::Utf8Path) -> Result<()> {
    systemd::install(repo)
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_install(_repo: &camino::Utf8Path) -> Result<()> {
    anyhow::bail!("supervisor unsupported on this platform — use `mycel daemon run`")
}

#[cfg(target_os = "macos")]
fn platform_uninstall() -> Result<()> {
    launchd::uninstall()
}
#[cfg(target_os = "linux")]
fn platform_uninstall() -> Result<()> {
    systemd::uninstall()
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_uninstall() -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn platform_start() -> Result<()> {
    launchd::start()
}
#[cfg(target_os = "linux")]
fn platform_start() -> Result<()> {
    systemd::start()
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_start() -> Result<()> {
    anyhow::bail!("use `mycel daemon run`")
}

#[cfg(target_os = "macos")]
fn platform_stop() -> Result<()> {
    launchd::stop()
}
#[cfg(target_os = "linux")]
fn platform_stop() -> Result<()> {
    systemd::stop()
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_stop() -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn platform_status() -> Result<()> {
    launchd::status()
}
#[cfg(target_os = "linux")]
fn platform_status() -> Result<()> {
    systemd::status()
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_status() -> Result<()> {
    Ok(())
}

fn log_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("no home"))?
        .join(".cache/mycel/daemon.log"))
}

fn tail_logs(follow: bool) -> Result<()> {
    let p = log_path()?;
    let mut cmd = Command::new("tail");
    if follow {
        cmd.arg("-f");
    }
    cmd.arg(&p);
    cmd.status()?;
    Ok(())
}

async fn run_foreground() -> Result<()> {
    // Exec the daemon binary in the same process group; it handles signals.
    let cur = std::env::current_exe()?;
    let bin = cur.with_file_name("mycel-daemon");
    let status = Command::new(&bin).status()?;
    anyhow::ensure!(status.success(), "mycel-daemon exited with {status}");
    Ok(())
}
