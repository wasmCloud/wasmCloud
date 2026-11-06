//! Benchmark 2's workload: several models taking turns on one GPU, each
//! started cold, run once and stopped, with idle gaps between them so the
//! GPU memory sampler sees what a scaled-to-zero host holds. The native side
//! runs the same plan as plain processes, for a baseline.

use std::{
    collections::BTreeMap,
    io::Write as _,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::workload::{Mount, Outcome, Runner};

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
    /// The command component, for the wasmCloud side.
    #[serde(default)]
    pub wasm: Option<PathBuf>,
    /// The same tool built natively, for the native side.
    #[serde(default)]
    pub native: Option<PathBuf>,
    /// The full argument vector, program name first, with guest paths.
    pub args: Vec<String>,
}

/// What runs each turn.
pub enum Side<'a> {
    /// A fresh workload per turn, in one long-lived host.
    Wasm(&'a Runner),
    /// A fresh process per turn. Guest paths in the arguments are mapped back
    /// through the mounts.
    Native(&'a [Mount]),
}

fn one() -> u32 {
    1
}

/// One line of the events file; `t_ms` is wall-clock milliseconds, the same
/// clock the GPU memory sampler stamps its samples with.
#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event<'a> {
    /// The host is up and no turn has run yet.
    Ready {
        t_ms: u64,
    },
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
///
/// The plan's idle gap also runs once before the first turn, so the sampler
/// sees what the host holds before it has served anything.
pub async fn run(side: &Side<'_>, plan: &Plan, events: &mut impl std::io::Write) -> Result<bool> {
    let mut wasm = BTreeMap::new();
    if let Side::Wasm(_) = side {
        for turn in &plan.turns {
            let path = turn
                .wasm
                .as_ref()
                .with_context(|| format!("turn {} has no wasm", turn.name))?;
            if !wasm.contains_key(path) {
                let bytes =
                    std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
                wasm.insert(path.clone(), bytes);
            }
        }
    }
    emit(events, &Event::Ready { t_ms: now_ms() })?;
    idle_gap(plan.idle_ms, events).await?;
    let mut all_ok = true;
    for round in 0..plan.rounds {
        for turn in &plan.turns {
            emit(
                events,
                &Event::TurnStart {
                    t_ms: now_ms(),
                    round,
                    name: &turn.name,
                },
            )?;
            let (outcome, high_water) = match side {
                Side::Wasm(runner) => {
                    let bytes = turn
                        .wasm
                        .as_ref()
                        .and_then(|p| wasm.get(p))
                        .cloned()
                        .unwrap_or_default();
                    let outcome = runner.run(&turn.name, bytes, &turn.args).await?;
                    (outcome, runner.guest_memory_peak())
                }
                Side::Native(mounts) => (run_native(turn, mounts).await?, 0),
            };
            all_ok &= outcome.exit_code == Some(0);
            emit(
                events,
                &Event::TurnEnd {
                    t_ms: now_ms(),
                    round,
                    name: &turn.name,
                    outcome: &outcome,
                    guest_memory_high_water: high_water,
                },
            )?;
            idle_gap(plan.idle_ms, events).await?;
        }
    }
    Ok(all_ok)
}

async fn idle_gap(idle_ms: u64, events: &mut impl std::io::Write) -> Result<()> {
    if idle_ms == 0 {
        return Ok(());
    }
    let t_ms = now_ms();
    emit(
        events,
        &Event::Idle {
            t_ms,
            until_ms: t_ms + idle_ms,
        },
    )?;
    tokio::time::sleep(Duration::from_millis(idle_ms)).await;
    Ok(())
}

async fn run_native(turn: &Turn, mounts: &[Mount]) -> Result<Outcome> {
    let program = turn
        .native
        .as_ref()
        .with_context(|| format!("turn {} has no native binary", turn.name))?;
    let args = turn.args.iter().skip(1).map(|a| host_path(a, mounts));
    let started = Instant::now();
    let output = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("run {}", program.display()))?;
    let run_ms = started.elapsed().as_secs_f64() * 1e3;
    let _ = std::io::stderr().write_all(&output.stderr);
    let error = (!output.status.success()).then(|| output.status.to_string());
    Ok(Outcome::new(
        output.status.code(),
        error,
        None,
        run_ms,
        &String::from_utf8_lossy(&output.stderr),
    ))
}

/// `arg` with a mount's guest path prefix swapped for its host path.
fn host_path(arg: &str, mounts: &[Mount]) -> String {
    for m in mounts {
        if let Some(rest) = arg.strip_prefix(m.guest.as_str())
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("{}{rest}", m.host.display());
        }
    }
    arg.to_string()
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
            resolve_ms: Some(1.5),
            run_ms: 2.5,
            load_ms: Some(1.0),
            first_token_ms: None,
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
        assert_eq!(line["load_ms"], 1.0);
        Ok(())
    }

    #[test]
    fn plan_takes_a_native_binary() -> Result<()> {
        let plan: Plan = serde_json::from_str(
            r#"{"turns":[{"name":"a","wasm":"a.wasm","native":"bin/a","args":["a"]}]}"#,
        )?;
        assert_eq!(plan.turns[0].native.as_deref(), Some(Path::new("bin/a")));
        Ok(())
    }

    #[test]
    fn host_path_maps_guest_prefixes_only() {
        let mounts = [Mount {
            host: "/data/models".into(),
            guest: "/models".into(),
        }];
        assert_eq!(host_path("/models/a.gguf", &mounts), "/data/models/a.gguf");
        assert_eq!(host_path("/models", &mounts), "/data/models");
        assert_eq!(host_path("/modelsx/a", &mounts), "/modelsx/a");
        assert_eq!(host_path("-n", &mounts), "-n");
    }
}
