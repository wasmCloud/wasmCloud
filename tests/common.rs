use std::time::Duration;
use wascc_host::{Host, Result};

pub async fn await_actor_count(
    h: &Host,
    count: usize,
    backoff: Duration,
    max_attempts: i32,
) -> Result<()> {
    let mut attempt = 0;
    loop {
        if h.get_actors().await?.len() == count {
            break;
        }
        ::std::thread::sleep(backoff);
        attempt = attempt + 1;
        if attempt > max_attempts {
            return Err("Exceeded max attempts".into());
        }
    }
    Ok(())
}

pub async fn await_provider_count(
    h: &Host,
    count: usize,
    backoff: Duration,
    max_attempts: i32,
) -> Result<()> {
    let mut attempt = 0;
    loop {
        if h.get_providers().await?.len() == count {
            break;
        }
        ::std::thread::sleep(backoff);
        attempt = attempt + 1;
        if attempt > max_attempts {
            return Err("Exceeded max attempts".into());
        }
    }
    Ok(())
}
