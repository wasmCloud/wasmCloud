wit_bindgen::generate!({ generate_all });

use crate::exports::wasmcloud::cron::scheduler::Guest as CronDemoGuest;

#[derive(Debug)]
struct CronDemo {}

impl CronDemoGuest for CronDemo {
    fn invoke(_payload: Vec<u8>) -> Result<(), String> {
        println!("Hello world");
        Ok(())
    }
}

export!(CronDemo);
