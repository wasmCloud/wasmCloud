use std::collections::HashMap;

struct CronProvider {
    jobs: HashMap<String, String>,
}

impl CronProvider {
    fn new() -> Self {
        Self {
            jobs: HashMap::new(),
        }
    }
}
