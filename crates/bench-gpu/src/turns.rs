//! Benchmark 2's workload: several models taking turns on one GPU, each
//! started cold, run once and stopped, with idle gaps between them so the
//! GPU memory sampler sees what a scaled-to-zero host holds.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::workload::{Outcome, Runner};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub turns: Vec<Turn>,
    #[serde(default = "one")]
    pub rounds: u32,
    #[serde(default)]
    pub idle_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub name: String,
    pub wasm: PathBuf,
    /// The full argument vector, program name first.
    pub args: Vec<String>,
}

fn one() -> u32 {
    1
}

/// One line of the events file; `t_ms` is wall-clock milliseconds, the same
/// clock the GPU memory sampler stamps its samples with.
#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event<'a> {
    TurnStart {
        t_ms: u64,
        round: u32,
        name: &'a str,
    },
    TurnEnd {
        t_ms: u64,
        round: u32,
        name: &'a str,
        #[serde(flatten)]
        outcome: &'a Outcome,
        /// The engine's high-water mark since the plan started, not this
        /// turn's own peak: the budget has no per-turn reset.
        guest_memory_high_water: u64,
    },
    Idle {
        t_ms: u64,
        until_ms: u64,
    },
}

pub fn load_plan(path: &Path) -> Result<Plan> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// Run the plan, writing one event per line to `events`. Returns whether
/// every turn exited 0.
pub async fn run(runner: &Runner, plan: &Plan, events: &mut impl std::io::Write) -> Result<bool> {
    let mut wasm = BTreeMap::new();
    for turn in &plan.turns {
        if !wasm.contains_key(&turn.wasm) {
            let bytes = std::fs::read(&turn.wasm)
                .with_context(|| format!("read {}", turn.wasm.display()))?;
            wasm.insert(turn.wasm.clone(), bytes);
        }
    }
    let idle = Duration::from_millis(plan.idle_ms);
    let mut all_ok = true;
    for round in 0..plan.rounds {
        for turn in &plan.turns {
            let bytes = wasm.get(&turn.wasm).cloned().unwrap_or_default();
            emit(
                events,
                &Event::TurnStart {
                    t_ms: now_ms(),
                    round,
                    name: &turn.name,
                },
            )?;
            let outcome = runner.run(&turn.name, bytes, &turn.args).await?;
            all_ok &= outcome.exit_code == Some(0);
            emit(
                events,
                &Event::TurnEnd {
                    t_ms: now_ms(),
                    round,
                    name: &turn.name,
                    outcome: &outcome,
                    guest_memory_high_water: runner.guest_memory_peak(),
                },
            )?;
            if !idle.is_zero() {
                let t_ms = now_ms();
                emit(
                    events,
                    &Event::Idle {
                        t_ms,
                        until_ms: t_ms + plan.idle_ms,
                    },
                )?;
                tokio::time::sleep(idle).await;
            }
        }
    }
    Ok(all_ok)
}

fn emit(out: &mut impl std::io::Write, event: &Event<'_>) -> Result<()> {
    serde_json::to_writer(&mut *out, event)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_defaults_to_one_round_without_idle() -> Result<()> {
        let plan: Plan = serde_json::from_str(
            r#"{"turns":[{"name":"a","wasm":"a.wasm","args":["a","-n","1"]}]}"#,
        )?;
        assert_eq!(plan.rounds, 1);
        assert_eq!(plan.idle_ms, 0);
        assert_eq!(plan.turns.len(), 1);
        Ok(())
    }

    #[test]
    fn plan_rejects_unknown_fields() {
        assert!(serde_json::from_str::<Plan>(r#"{"turns":[],"round":2}"#).is_err());
    }

    #[test]
    fn turn_end_flattens_the_outcome() -> Result<()> {
        let outcome = Outcome {
            exit_code: Some(0),
            error: None,
            resolve_ms: 1.5,
            run_ms: 2.5,
        };
        let mut out = Vec::new();
        emit(
            &mut out,
            &Event::TurnEnd {
                t_ms: 7,
                round: 0,
                name: "a",
                outcome: &outcome,
                guest_memory_high_water: 9,
            },
        )?;
        let line: serde_json::Value = serde_json::from_slice(&out)?;
        assert_eq!(line["event"], "turn_end");
        assert_eq!(line["exit_code"], 0);
        assert_eq!(line["run_ms"], 2.5);
        assert_eq!(line["guest_memory_high_water"], 9);
        Ok(())
    }
}
