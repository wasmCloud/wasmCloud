//! Messaging's share of the instance pool's totals, exercised through
//! [`MessagingLimits`] against a real pool: what the host message ceiling and
//! that share of the pool totals admit together, and what each delivery path
//! reserves from it. The mechanism itself is tested in
//! [`crate::engine::pool_share`].

use std::time::Duration;

use futures::FutureExt as _;
use tokio::task::unconstrained;
use wasmtime::Store;
use wasmtime::component::{Component, Linker};

use super::{AdmissionIdentity, Admitted, MessagingLimits, PoolCharge};
use crate::engine::Engine;
use crate::engine::pool_share::PoolCost;
use crate::engine::pool_share::test_support::{component, config, engine};

fn identity(i: usize) -> AdmissionIdentity {
    AdmissionIdentity::new("review", &format!("workload-{i}"), "component")
}

/// Admit and instantiate, holding every store and permit, until an admission
/// queues: how many messages the host message ceiling and messaging's share
/// of the pool totals admit together,
/// checked against the real pool, which must take each one. Dropping one must
/// let the queued admission through and its instantiation succeed.
async fn fill(engine: &Engine, component: &Component, explicit_host: Option<usize>) -> usize {
    let limits = MessagingLimits::resolve(explicit_host, None, engine);
    let linker = Linker::<()>::new(engine.inner());
    let mut held = Vec::new();
    for i in 0..=limits.host_total() {
        let admission = limits
            .admission(&identity(i), None)
            .with_component(component)
            .unwrap();
        // Resource availability, not whether Tokio's cooperative budget
        // happened to expire in this loop.
        let Some(permit) = unconstrained(admission.acquire(PoolCharge::Reserve))
            .now_or_never()
            .flatten()
        else {
            assert!(!held.is_empty(), "the pool should admit at least one");
            let admitted = held.len();
            drop(held.pop());
            let _permit = unconstrained(admission.acquire(PoolCharge::Reserve))
                .now_or_never()
                .flatten()
                .expect("a completed delivery must release its resources");
            let mut store = Store::new(engine.inner(), ());
            linker
                .instantiate_async(&mut store, component)
                .await
                .unwrap();
            return admitted;
        };
        let mut store = Store::new(engine.inner(), ());
        linker
            .instantiate_async(&mut store, component)
            .await
            .expect("admission must queue before instantiation can exhaust the pool");
        held.push((store, permit));
    }
    panic!("admitted more messages than the host ceiling");
}

#[tokio::test]
async fn a_lowered_core_total_binds_the_ceiling_and_an_explicit_cap_keeps_the_core_share() {
    let mut config = config();
    config.total_core_instances(32);
    let engine = engine(config);
    let component = component(&engine, 1, 0, 4);
    // 32 core instances hold 6 components of the usual shape; the message
    // ceiling is messaging's fraction of that, floored at 6.
    let limits = MessagingLimits::resolve(None, None, &engine);
    assert_eq!(limits.host_total(), 6);
    // Messaging's share of the 32 core instances is floored at the same six
    // instances' worth, so the two bind together: six components at an
    // estimated 5 each, and the pool queues the seventh rather than refusing
    // it.
    assert_eq!(fill(&engine, &component, None).await, 6);
    // An explicit message cap moves the message count, never the share of
    // core instances: with the cap at 64 the share still admits six.
    assert_eq!(fill(&engine, &component, Some(64)).await, 6);
}

#[tokio::test]
async fn ordinary_components_keep_the_larger_ceiling() {
    let engine = engine(config());
    assert_eq!(fill(&engine, &component(&engine, 1, 2, 4), None).await, 21);
}

#[tokio::test]
async fn composed_components_pay_for_every_memory() {
    let engine = engine(config());
    assert_eq!(fill(&engine, &component(&engine, 2, 0, 2), None).await, 10);
}

#[tokio::test]
async fn component_and_stack_overrides_bound_explicit_message_limits() {
    for stacks in [false, true] {
        let mut config = config();
        if stacks {
            config.total_stacks(4);
        } else {
            config.total_component_instances(4);
        }
        let engine = engine(config);
        assert_eq!(
            fill(&engine, &component(&engine, 1, 0, 1), Some(64)).await,
            4
        );
    }
}

#[tokio::test]
async fn an_explicit_single_message_can_hold_multiple_memories() {
    let engine = engine(config());
    assert_eq!(
        fill(&engine, &component(&engine, 2, 0, 2), Some(1)).await,
        1
    );
}

#[test]
fn a_component_needing_more_memories_than_messagings_share_fails_to_bind() {
    // Not admitted and left to fail at instantiation once anything else held a
    // memory, and not handed every memory of messaging's share to run alone:
    // refused where the binding is made, with the knob named.
    let engine = engine(config());
    let limits = MessagingLimits::resolve(None, None, &engine);
    let err = limits
        .admission(&identity(0), None)
        .with_component(&component(&engine, 25, 0, 25))
        .expect_err("25 memories do not fit a share of 21 memories");
    let text = format!("{err:#}");
    assert!(text.contains("messaging may hold at most 21"), "{text}");
    assert!(text.contains("--core-instances"), "{text}");
}

#[tokio::test]
async fn a_warm_instance_delivery_reserves_nothing() {
    // A delivery a warm instance takes holds no pool slot of its own, so it
    // must not spend messaging's share of any pool total; only a store of its
    // own does.
    let engine = engine(config());
    let limits = MessagingLimits::resolve(None, None, &engine);
    let pool_share = limits.pool.clone().unwrap();
    let admission = limits
        .admission(&identity(0), None)
        .with_component(&component(&engine, 2, 1, 2))
        .unwrap();
    let per_total_budget = pool_share.budget();

    let deferred = admission.acquire(PoolCharge::Defer).await.unwrap();
    assert_eq!(pool_share.available(), per_total_budget);
    drop(deferred);

    let reserved = admission.acquire(PoolCharge::Reserve).await.unwrap();
    let available = pool_share.available();
    assert_eq!(available.memories, per_total_budget.memories - 2);
    assert_eq!(available.tables, per_total_budget.tables - 1);
    assert_eq!(
        available.core_instances,
        per_total_budget.core_instances - 2 * PoolCost::USUAL.core_instances
    );
    assert_eq!(
        available.component_instances,
        per_total_budget.component_instances - 1
    );
    assert_eq!(available.stacks, per_total_budget.stacks - 1);
    drop(reserved);
    assert_eq!(pool_share.available(), per_total_budget);

    // A store built after a deferred admission reserves then, inside the
    // permit, which carries the reservation for as long as it lives.
    let deferred = admission.acquire(PoolCharge::Defer).await.unwrap();
    let Admitted::Slot(mut permit) = admission
        .reserve_store_before_deadline(deferred, "comp", "subj")
        .await
    else {
        panic!("a costed component reserves");
    };
    assert_eq!(
        pool_share.available().memories,
        per_total_budget.memories - 2
    );
    // An instance the pool took as its own gives the reservation back and
    // keeps the message permits.
    permit.release_store_reservation();
    assert_eq!(pool_share.available(), per_total_budget);
    drop(permit);
}

#[tokio::test]
async fn resource_waits_use_the_same_deadline_and_release_permits() {
    let engine = engine(config());
    let limits = MessagingLimits::resolve(None, None, &engine).with_admission_wait(Duration::ZERO);
    let pool_share = limits.pool.clone().unwrap();
    let admission = limits
        .admission(&identity(0), None)
        .with_component(&component(&engine, 2, 1, 4))
        .unwrap();
    // Spend every memory of messaging's share, and nothing else.
    let blocker = pool_share
        .reserve(PoolCost {
            memories: pool_share.budget().memories,
            tables: 0,
            core_instances: 0,
            component_instances: 0,
            stacks: 0,
        })
        .await
        .unwrap();
    assert!(matches!(
        admission
            .acquire_before_deadline("comp", "subj", PoolCharge::Reserve)
            .await,
        Admitted::Shed
    ));
    // A deferred admission is not held up by the share of memories at all,
    // and a store reserved after it sheds on what is left of the same
    // deadline — nothing, here — rather than waiting a second whole one.
    let Admitted::Slot(deferred) = admission
        .acquire_before_deadline("comp", "subj", PoolCharge::Defer)
        .await
    else {
        panic!("nothing holds a deferred admission up");
    };
    assert!(matches!(
        admission
            .reserve_store_before_deadline(deferred, "comp", "subj")
            .await,
        Admitted::Shed
    ));
    drop(blocker);
    assert!(
        unconstrained(admission.acquire(PoolCharge::Reserve))
            .now_or_never()
            .flatten()
            .is_some()
    );
}

#[tokio::test]
async fn a_store_reserved_after_admission_gives_the_host_permit_back_while_it_waits() {
    // The lock order is component, pool cost, host. A pooled delivery that
    // kept its host permit while waiting for memories, beside per-message
    // deliveries holding memories while waiting for a host permit, would
    // leave both waiting until they shed. So the pooled delivery gives the
    // host permit back first: while it waits, another component's message
    // can still be admitted, and once the memories come free it takes a host
    // permit again, behind that message.
    let engine = engine(config());
    let limits = MessagingLimits::resolve(Some(1), None, &engine);
    let pool_share = limits.pool.clone().unwrap();
    let pooled = limits
        .admission(&identity(0), None)
        .with_component(&component(&engine, 2, 0, 2))
        .unwrap();
    // A component with no reservation to make (`with_component` never ran),
    // so only the single host permit stands between it and admission.
    let other = limits.admission(&identity(1), None);

    let memories = pool_share
        .reserve(PoolCost {
            memories: pool_share.budget().memories,
            tables: 0,
            core_instances: 0,
            component_instances: 0,
            stacks: 0,
        })
        .await
        .unwrap();
    let deferred = pooled.acquire(PoolCharge::Defer).await.unwrap();
    assert!(
        unconstrained(other.acquire(PoolCharge::Reserve))
            .now_or_never()
            .is_none(),
        "the deferred admission holds the only host permit"
    );

    let mut waiting =
        std::pin::pin!(pooled.reserve_store_before_deadline(deferred, "comp", "subj"));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    let held_by_other = unconstrained(other.acquire(PoolCharge::Reserve))
        .now_or_never()
        .flatten()
        .expect("the host permit was given back while the pool cost is waited for");

    drop(memories);
    assert!(
        futures::poll!(waiting.as_mut()).is_pending(),
        "the memories came free but the host permit is held by the other message"
    );
    drop(held_by_other);
    let Admitted::Slot(permit) = waiting.await else {
        panic!("the host permit came back");
    };
    assert_eq!(
        pool_share.available().memories,
        pool_share.budget().memories - 2
    );
    drop(permit);
    assert_eq!(pool_share.available(), pool_share.budget());
}

#[tokio::test]
async fn clones_draw_on_one_share_of_the_pool_totals_across_backends() {
    let engine = engine(config());
    let small = component(&engine, 1, 0, 1);
    let composed = component(&engine, 2, 0, 2);
    let limits = MessagingLimits::resolve(None, None, &engine);
    let other_backend = limits.clone();
    let mut held = Vec::new();
    for i in 0..14 {
        let (backend, component) = if i < 7 {
            (&limits, &small)
        } else {
            (&other_backend, &composed)
        };
        let admission = backend
            .admission(&identity(i), None)
            .with_component(component)
            .unwrap();
        held.push(
            unconstrained(admission.acquire(PoolCharge::Reserve))
                .now_or_never()
                .flatten()
                .unwrap(),
        );
    }
    // Seven single-memory and seven two-memory deliveries spend all 21
    // memories of messaging's pool_share.
    let extra = limits
        .admission(&identity(99), None)
        .with_component(&small)
        .unwrap();
    assert!(
        unconstrained(extra.acquire(PoolCharge::Reserve))
            .now_or_never()
            .is_none()
    );
    drop(held.pop());
    assert!(
        unconstrained(extra.acquire(PoolCharge::Reserve))
            .now_or_never()
            .flatten()
            .is_some()
    );
}
