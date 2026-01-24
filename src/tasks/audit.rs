use crate::config::Config;
use anyhow::Result;
use std::path::Path;

pub async fn run(config: &Config, dir: &Path) -> Result<()> {
    super::run_task(config, "audit", dir).await
}
