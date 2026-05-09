//! `mycel skill install` — symlink <repo>/skills/mycel-graph-care into
//! ~/.claude/skills/mycel-graph-care. Idempotent. Refuses to overwrite
//! a non-symlink at the target.

use anyhow::{Context, Result, bail};
use camino::{Utf8Path, Utf8PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub source: Utf8PathBuf,
    pub target: Utf8PathBuf,
    pub linked: bool,
}

pub fn source_for_repo(repo: &Utf8Path) -> Utf8PathBuf {
    repo.join("skills").join("mycel-graph-care")
}

pub fn default_target() -> Result<Utf8PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let home: Utf8PathBuf = home.into();
    Ok(home.join(".claude").join("skills").join("mycel-graph-care"))
}

/// Install the skill: create the symlink target's parent if needed, then
/// create a symlink from `target` to `source`. Idempotent.
pub fn install(source: &Utf8Path, target: &Utf8Path) -> Result<InstallOutcome> {
    if !source.exists() {
        bail!("source does not exist: {source}");
    }
    let target_path = target.as_std_path();
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent.as_std_path())
            .with_context(|| format!("create parent dir {parent}"))?;
    }
    if target_path.exists() || target_path.is_symlink() {
        if target_path.is_symlink() {
            let dest = std::fs::read_link(target_path)
                .with_context(|| format!("read symlink {target}"))?;
            let dest_for_msg = dest.clone();
            let dest_canon = dest.canonicalize().unwrap_or(dest);
            let source_canon = source
                .as_std_path()
                .canonicalize()
                .with_context(|| format!("canonicalize source {source}"))?;
            if dest_canon == source_canon {
                return Ok(InstallOutcome {
                    source: source.to_path_buf(),
                    target: target.to_path_buf(),
                    linked: true,
                });
            }
            bail!(
                "{target} is already a symlink, but points elsewhere ({dest_for_msg:?}). \
                 Remove it manually and retry."
            );
        }
        bail!("{target} exists and is not a symlink. Remove it manually and retry.");
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(source.as_std_path(), target_path)
        .with_context(|| format!("symlink {source} -> {target}"))?;

    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(source.as_std_path(), target_path)
        .with_context(|| format!("symlink {source} -> {target}"))?;

    Ok(InstallOutcome {
        source: source.to_path_buf(),
        target: target.to_path_buf(),
        linked: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> Utf8PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mycel-skill-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Utf8PathBuf::from_path_buf(p).unwrap()
    }

    #[test]
    fn install_creates_symlink() {
        let dir = tmp();
        let source = dir.join("src/skill");
        std::fs::create_dir_all(source.as_std_path()).unwrap();
        let target = dir.join("home/.claude/skills/x");

        let outcome = install(&source, &target).unwrap();
        assert!(outcome.linked);
        assert!(target.as_std_path().is_symlink());
    }

    #[test]
    fn install_is_idempotent() {
        let dir = tmp();
        let source = dir.join("src/skill");
        std::fs::create_dir_all(source.as_std_path()).unwrap();
        let target = dir.join("home/.claude/skills/x");

        install(&source, &target).unwrap();
        install(&source, &target).unwrap();
    }

    #[test]
    fn install_refuses_to_overwrite_existing_file() {
        let dir = tmp();
        let source = dir.join("src/skill");
        std::fs::create_dir_all(source.as_std_path()).unwrap();
        let target = dir.join("home/.claude/skills/x");
        std::fs::create_dir_all(target.parent().unwrap().as_std_path()).unwrap();
        std::fs::write(target.as_std_path(), "user data").unwrap();

        let err = install(&source, &target).unwrap_err();
        assert!(err.to_string().contains("not a symlink"));
    }

    #[test]
    fn install_refuses_to_overwrite_wrong_symlink() {
        let dir = tmp();
        let real_source = dir.join("src/skill");
        std::fs::create_dir_all(real_source.as_std_path()).unwrap();
        let other = dir.join("src/other");
        std::fs::create_dir_all(other.as_std_path()).unwrap();
        let target = dir.join("home/.claude/skills/x");
        std::fs::create_dir_all(target.parent().unwrap().as_std_path()).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(other.as_std_path(), target.as_std_path()).unwrap();

        let err = install(&real_source, &target).unwrap_err();
        assert!(err.to_string().contains("points elsewhere"));
    }
}
