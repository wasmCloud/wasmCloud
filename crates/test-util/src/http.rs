use anyhow::{Context as _, Result};
use tokio::time::Duration;

/// Wait for a given URL to be accessible (i.e. return any successful error code)
pub async fn wait_for_url(url: impl AsRef<str>) -> Result<()> {
    let url = url.as_ref();
    tokio::time::timeout(Duration::from_secs(20), async move {
        loop {
            if reqwest::get(url)
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
    .await
    .context("failed to access running application")?;
    Ok(())
}
