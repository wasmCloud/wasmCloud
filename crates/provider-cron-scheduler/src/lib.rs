use std::collections::HashMap;
mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wasmcloud:cron/scheduler@0.1.0-draft":generate,
        }
    });
}
struct CronProvider {
    jobs: HashMap<String, String>,
}

pub async fn run() -> anyhow::Result<()> {
    Ok(())
}

impl CronProvider {
    fn new() -> Self {
        Self {
            jobs: HashMap::new(),
        }
    }
}
