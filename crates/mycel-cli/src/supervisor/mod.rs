use crate::cli::DaemonAction;
use anyhow::Result;

pub async fn dispatch(_action: DaemonAction) -> Result<()> {
    anyhow::bail!("supervisor not yet implemented (Task 7.3)")
}
