use std::collections::HashMap;

struct CronProvider {
    jobs: HashMap<String, String>,
}

pub async fn run()  -> anyhow::Result<()> {
    Ok(())
}

impl CronProvider {
    fn new() -> Self {
        Self {
            jobs: HashMap::new(),
        }
    }
}
