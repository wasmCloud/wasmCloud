// *** This test requires a running NATS server

use std::{collections::HashMap, time::Duration};

use wascc_codec::{capabilities::CapabilityProvider, core::OP_BIND_ACTOR};
use wasmcloud_actor_core::{deserialize, serialize, CapabilityConfiguration};
use wasmcloud_actor_keyvalue::{
    AddArgs, GetArgs, GetResponse, SetAddArgs, SetQueryArgs, SetQueryResponse, OP_ADD, OP_GET,
    OP_SET_ADD, OP_SET_QUERY,
};
use wasmcloud_nats_kvcache::NatsReplicatedKVProvider;

#[test]
fn steadystate_and_replay() {
    let prov_a = NatsReplicatedKVProvider::new();
    let prov_b = NatsReplicatedKVProvider::new();

    let r = prov_a.handle_call(
        "system",
        OP_BIND_ACTOR,
        &serialize(capability_config()).unwrap(),
    );
    let r2 = prov_b.handle_call(
        "system",
        OP_BIND_ACTOR,
        &serialize(capability_config()).unwrap(),
    );
    assert!(r.is_ok());
    assert!(r2.is_ok());

    // let A handle a cache-mutating event
    let _ = prov_a.handle_call(
        "system",
        OP_ADD,
        &serialize(AddArgs {
            key: "val1".to_string(),
            value: 14,
        })
        .unwrap(),
    );

    ::std::thread::sleep(Duration::from_millis(500)); // allow time to replicate

    let ga = GetArgs {
        key: "val1".to_string(),
    };
    let q = prov_b
        .handle_call("system", OP_GET, &serialize(&ga).unwrap())
        .unwrap();
    let g: GetResponse = deserialize(&q).unwrap();
    let q2 = prov_a
        .handle_call("system", OP_GET, &serialize(&ga).unwrap())
        .unwrap();
    let g2: GetResponse = deserialize(&q2).unwrap();
    assert_eq!(g.value, "14");
    assert_eq!(g.exists, true);
    assert_eq!(g2.value, "14");
    assert_eq!(g2.exists, true);

    let _ = prov_b.handle_call(
        "system",
        OP_SET_ADD,
        &serialize(SetAddArgs {
            key: "set_one".to_string(),
            value: "smurf".to_string(),
        })
        .unwrap(),
    );
    let _ = prov_b.handle_call(
        "system",
        OP_SET_ADD,
        &serialize(SetAddArgs {
            key: "set_one".to_string(),
            value: "gargamel".to_string(),
        })
        .unwrap(),
    );

    std::thread::sleep(Duration::from_millis(500)); // allow time for replication

    let res = prov_a
        .handle_call(
            "system",
            OP_SET_QUERY,
            &serialize(SetQueryArgs {
                key: "set_one".to_string(),
            })
            .unwrap(),
        )
        .unwrap();
    let setresp: SetQueryResponse = deserialize(&res).unwrap();
    assert!(setresp.values.contains(&"smurf".to_string()));
    assert!(setresp.values.contains(&"gargamel".to_string()));

    // ***
    // Create a third provider and bring it up
    // ***
    // Ensure that it automatically acquires the cluster state from
    // one of the two existing providers

    let prov_c = NatsReplicatedKVProvider::new();

    let _ = prov_c
        .handle_call(
            "system",
            OP_BIND_ACTOR,
            &serialize(capability_config()).unwrap(),
        )
        .unwrap();

    std::thread::sleep(Duration::from_secs(1)); // allow time for restore from replay

    let res = prov_c
        .handle_call(
            "system",
            OP_SET_QUERY,
            &serialize(SetQueryArgs {
                key: "set_one".to_string(),
            })
            .unwrap(),
        )
        .unwrap();
    let setresp: SetQueryResponse = deserialize(&res).unwrap();
    assert!(setresp.values.contains(&"smurf".to_string()));
    assert!(setresp.values.contains(&"gargamel".to_string()));
    let ga = GetArgs {
        key: "val1".to_string(),
    };
    let q = prov_c
        .handle_call("system", OP_GET, &serialize(&ga).unwrap())
        .unwrap();
    let g: GetResponse = deserialize(&q).unwrap();
    assert_eq!(g.value, "14");
    assert_eq!(g.exists, true);
}

fn capability_config() -> CapabilityConfiguration {
    CapabilityConfiguration {
        module: "test".to_string(), // because of the nature of this provider, the module (actor ID) doesn't matter
        values: config_values(),
    }
}

fn config_values() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("NATS_URL".to_string(), "nats://0.0.0.0:4222".to_string());
    hm.insert(
        "STATE_REPL_SUBJECT".to_string(),
        "itest.lattice.state.events".to_string(),
    );
    hm.insert(
        "REPLAY_REQ_SUBJECT".to_string(),
        "itest.lattice.state.replay".to_string(),
    );

    hm
}
