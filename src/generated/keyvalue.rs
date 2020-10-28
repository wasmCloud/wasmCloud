extern crate rmp_serde as rmps;
use rmps::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

extern crate log;
//extern crate wapc_guest as guest;
//use guest::prelude::*;

//use lazy_static::lazy_static;
//use std::sync::RwLock;
/*
pub struct Host {
    binding: String,
}

impl Default for Host {
    fn default() -> Self {
        Host {
            binding: "default".to_string(),
        }
    }
}

/// Creates a named host binding for the key-value store capability
pub fn host(binding: &str) -> Host {
    Host {
        binding: binding.to_string(),
    }
}

/// Creates the default host binding for the key-value store capability
pub fn default() -> Host {
    Host::default()
}

impl Host {
    pub fn get(&self, key: String) -> HandlerResult<GetResponse> {
        let input_args = GetArgs { key: key };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "Get",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<GetResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn add(&self, key: String, value: i32) -> HandlerResult<AddResponse> {
        let input_args = AddArgs {
            key: key,
            value: value,
        };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "Add",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<AddResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn set(&self, key: String, value: String, expires: i32) -> HandlerResult<SetResponse> {
        let input_args = SetArgs {
            key: key,
            value: value,
            expires: expires,
        };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "Set",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<SetResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn del(&self, key: String) -> HandlerResult<DelResponse> {
        let input_args = DelArgs { key: key };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "Del",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<DelResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn clear(&self, key: String) -> HandlerResult<DelResponse> {
        let input_args = ClearArgs { key: key };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "Clear",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<DelResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn range(&self, key: String, start: i32, stop: i32) -> HandlerResult<ListRangeResponse> {
        let input_args = RangeArgs {
            key: key,
            start: start,
            stop: stop,
        };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "Range",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<ListRangeResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn push(&self, key: String, value: String) -> HandlerResult<ListResponse> {
        let input_args = PushArgs {
            key: key,
            value: value,
        };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "Push",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<ListResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn list_item_delete(&self, key: String, value: String) -> HandlerResult<ListResponse> {
        let input_args = ListItemDeleteArgs {
            key: key,
            value: value,
        };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "ListItemDelete",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<ListResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn set_add(&self, key: String, value: String) -> HandlerResult<SetOperationResponse> {
        let input_args = SetAddArgs {
            key: key,
            value: value,
        };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "SetAdd",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<SetOperationResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn set_remove(&self, key: String, value: String) -> HandlerResult<SetOperationResponse> {
        let input_args = SetRemoveArgs {
            key: key,
            value: value,
        };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "SetRemove",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<SetOperationResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn set_union(&self, keys: Vec<String>) -> HandlerResult<SetQueryResponse> {
        let input_args = SetUnionArgs { keys: keys };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "SetUnion",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<SetQueryResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn set_intersection(&self, keys: Vec<String>) -> HandlerResult<SetQueryResponse> {
        let input_args = SetIntersectionArgs { keys: keys };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "SetIntersection",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<SetQueryResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn set_query(&self, key: String) -> HandlerResult<SetQueryResponse> {
        let input_args = SetQueryArgs { key: key };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "SetQuery",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<SetQueryResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }

    pub fn key_exists(&self, key: String) -> HandlerResult<GetResponse> {
        let input_args = KeyExistsArgs { key: key };
        host_call(
            &self.binding,
            "wascc:keyvalue",
            "KeyExists",
            &serialize(input_args)?,
        )
        .map(|vec| {
            let resp = deserialize::<GetResponse>(vec.as_ref()).unwrap();
            resp
        })
        .map_err(|e| e.into())
    }
}

pub struct Handlers {}

impl Handlers {
    pub fn register_get(f: fn(String) -> HandlerResult<GetResponse>) {
        *GET.write().unwrap() = Some(f);
        register_function(&"Get", get_wrapper);
    }
    pub fn register_add(f: fn(String, i32) -> HandlerResult<AddResponse>) {
        *ADD.write().unwrap() = Some(f);
        register_function(&"Add", add_wrapper);
    }
    pub fn register_set(f: fn(String, String, i32) -> HandlerResult<SetResponse>) {
        *SET.write().unwrap() = Some(f);
        register_function(&"Set", set_wrapper);
    }
    pub fn register_del(f: fn(String) -> HandlerResult<DelResponse>) {
        *DEL.write().unwrap() = Some(f);
        register_function(&"Del", del_wrapper);
    }
    pub fn register_clear(f: fn(String) -> HandlerResult<DelResponse>) {
        *CLEAR.write().unwrap() = Some(f);
        register_function(&"Clear", clear_wrapper);
    }
    pub fn register_range(f: fn(String, i32, i32) -> HandlerResult<ListRangeResponse>) {
        *RANGE.write().unwrap() = Some(f);
        register_function(&"Range", range_wrapper);
    }
    pub fn register_push(f: fn(String, String) -> HandlerResult<ListResponse>) {
        *PUSH.write().unwrap() = Some(f);
        register_function(&"Push", push_wrapper);
    }
    pub fn register_list_item_delete(f: fn(String, String) -> HandlerResult<ListResponse>) {
        *LIST_ITEM_DELETE.write().unwrap() = Some(f);
        register_function(&"ListItemDelete", list_item_delete_wrapper);
    }
    pub fn register_set_add(f: fn(String, String) -> HandlerResult<SetOperationResponse>) {
        *SET_ADD.write().unwrap() = Some(f);
        register_function(&"SetAdd", set_add_wrapper);
    }
    pub fn register_set_remove(f: fn(String, String) -> HandlerResult<SetOperationResponse>) {
        *SET_REMOVE.write().unwrap() = Some(f);
        register_function(&"SetRemove", set_remove_wrapper);
    }
    pub fn register_set_union(f: fn(Vec<String>) -> HandlerResult<SetQueryResponse>) {
        *SET_UNION.write().unwrap() = Some(f);
        register_function(&"SetUnion", set_union_wrapper);
    }
    pub fn register_set_intersection(f: fn(Vec<String>) -> HandlerResult<SetQueryResponse>) {
        *SET_INTERSECTION.write().unwrap() = Some(f);
        register_function(&"SetIntersection", set_intersection_wrapper);
    }
    pub fn register_set_query(f: fn(String) -> HandlerResult<SetQueryResponse>) {
        *SET_QUERY.write().unwrap() = Some(f);
        register_function(&"SetQuery", set_query_wrapper);
    }
    pub fn register_key_exists(f: fn(String) -> HandlerResult<GetResponse>) {
        *KEY_EXISTS.write().unwrap() = Some(f);
        register_function(&"KeyExists", key_exists_wrapper);
    }
}

lazy_static! {
    static ref GET: RwLock<Option<fn(String) -> HandlerResult<GetResponse>>> = RwLock::new(None);
    static ref ADD: RwLock<Option<fn(String, i32) -> HandlerResult<AddResponse>>> =
        RwLock::new(None);
    static ref SET: RwLock<Option<fn(String, String, i32) -> HandlerResult<SetResponse>>> =
        RwLock::new(None);
    static ref DEL: RwLock<Option<fn(String) -> HandlerResult<DelResponse>>> = RwLock::new(None);
    static ref CLEAR: RwLock<Option<fn(String) -> HandlerResult<DelResponse>>> = RwLock::new(None);
    static ref RANGE: RwLock<Option<fn(String, i32, i32) -> HandlerResult<ListRangeResponse>>> =
        RwLock::new(None);
    static ref PUSH: RwLock<Option<fn(String, String) -> HandlerResult<ListResponse>>> =
        RwLock::new(None);
    static ref LIST_ITEM_DELETE: RwLock<Option<fn(String, String) -> HandlerResult<ListResponse>>> =
        RwLock::new(None);
    static ref SET_ADD: RwLock<Option<fn(String, String) -> HandlerResult<SetOperationResponse>>> =
        RwLock::new(None);
    static ref SET_REMOVE: RwLock<Option<fn(String, String) -> HandlerResult<SetOperationResponse>>> =
        RwLock::new(None);
    static ref SET_UNION: RwLock<Option<fn(Vec<String>) -> HandlerResult<SetQueryResponse>>> =
        RwLock::new(None);
    static ref SET_INTERSECTION: RwLock<Option<fn(Vec<String>) -> HandlerResult<SetQueryResponse>>> =
        RwLock::new(None);
    static ref SET_QUERY: RwLock<Option<fn(String) -> HandlerResult<SetQueryResponse>>> =
        RwLock::new(None);
    static ref KEY_EXISTS: RwLock<Option<fn(String) -> HandlerResult<GetResponse>>> =
        RwLock::new(None);
}

fn get_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<GetArgs>(input_payload)?;
    let lock = GET.read().unwrap().unwrap();
    let result = lock(input.key)?;
    Ok(serialize(result)?)
}

fn add_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<AddArgs>(input_payload)?;
    let lock = ADD.read().unwrap().unwrap();
    let result = lock(input.key, input.value)?;
    Ok(serialize(result)?)
}

fn set_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<SetArgs>(input_payload)?;
    let lock = SET.read().unwrap().unwrap();
    let result = lock(input.key, input.value, input.expires)?;
    Ok(serialize(result)?)
}

fn del_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<DelArgs>(input_payload)?;
    let lock = DEL.read().unwrap().unwrap();
    let result = lock(input.key)?;
    Ok(serialize(result)?)
}

fn clear_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<ClearArgs>(input_payload)?;
    let lock = CLEAR.read().unwrap().unwrap();
    let result = lock(input.key)?;
    Ok(serialize(result)?)
}

fn range_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<RangeArgs>(input_payload)?;
    let lock = RANGE.read().unwrap().unwrap();
    let result = lock(input.key, input.start, input.stop)?;
    Ok(serialize(result)?)
}

fn push_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<PushArgs>(input_payload)?;
    let lock = PUSH.read().unwrap().unwrap();
    let result = lock(input.key, input.value)?;
    Ok(serialize(result)?)
}

fn list_item_delete_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<ListItemDeleteArgs>(input_payload)?;
    let lock = LIST_ITEM_DELETE.read().unwrap().unwrap();
    let result = lock(input.key, input.value)?;
    Ok(serialize(result)?)
}

fn set_add_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<SetAddArgs>(input_payload)?;
    let lock = SET_ADD.read().unwrap().unwrap();
    let result = lock(input.key, input.value)?;
    Ok(serialize(result)?)
}

fn set_remove_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<SetRemoveArgs>(input_payload)?;
    let lock = SET_REMOVE.read().unwrap().unwrap();
    let result = lock(input.key, input.value)?;
    Ok(serialize(result)?)
}

fn set_union_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<SetUnionArgs>(input_payload)?;
    let lock = SET_UNION.read().unwrap().unwrap();
    let result = lock(input.keys)?;
    Ok(serialize(result)?)
}

fn set_intersection_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<SetIntersectionArgs>(input_payload)?;
    let lock = SET_INTERSECTION.read().unwrap().unwrap();
    let result = lock(input.keys)?;
    Ok(serialize(result)?)
}

fn set_query_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<SetQueryArgs>(input_payload)?;
    let lock = SET_QUERY.read().unwrap().unwrap();
    let result = lock(input.key)?;
    Ok(serialize(result)?)
}

fn key_exists_wrapper(input_payload: &[u8]) -> CallResult {
    let input = deserialize::<KeyExistsArgs>(input_payload)?;
    let lock = KEY_EXISTS.read().unwrap().unwrap();
    let result = lock(input.key)?;
    Ok(serialize(result)?)
}
*/
#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct GetArgs {
    #[serde(rename = "key")]
    pub key: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct AddArgs {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "value")]
    pub value: i32,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct SetArgs {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "value")]
    pub value: String,
    #[serde(rename = "expires")]
    pub expires: i32,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct DelArgs {
    #[serde(rename = "key")]
    pub key: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct ClearArgs {
    #[serde(rename = "key")]
    pub key: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct RangeArgs {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "start")]
    pub start: i32,
    #[serde(rename = "stop")]
    pub stop: i32,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct PushArgs {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "value")]
    pub value: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct ListItemDeleteArgs {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "value")]
    pub value: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct SetAddArgs {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "value")]
    pub value: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct SetRemoveArgs {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "value")]
    pub value: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct SetUnionArgs {
    #[serde(rename = "keys")]
    pub keys: Vec<String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct SetIntersectionArgs {
    #[serde(rename = "keys")]
    pub keys: Vec<String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct SetQueryArgs {
    #[serde(rename = "key")]
    pub key: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct KeyExistsArgs {
    #[serde(rename = "key")]
    pub key: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct GetResponse {
    #[serde(rename = "value")]
    pub value: String,
    #[serde(rename = "exists")]
    pub exists: bool,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct AddResponse {
    #[serde(rename = "value")]
    pub value: i32,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct DelResponse {
    #[serde(rename = "key")]
    pub key: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct ListRangeResponse {
    #[serde(rename = "values")]
    pub values: Vec<String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct ListResponse {
    #[serde(rename = "newCount")]
    pub new_count: i32,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct SetResponse {
    #[serde(rename = "value")]
    pub value: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct SetOperationResponse {
    #[serde(rename = "new_count")]
    pub new_count: i32,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default)]
pub struct SetQueryResponse {
    #[serde(rename = "values")]
    pub values: Vec<String>,
}

/// The standard function for serializing codec structs into a format that can be
/// used for message exchange between actor and host. Use of any other function to
/// serialize could result in breaking incompatibilities.
fn serialize<T>(item: T) -> ::std::result::Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>
where
    T: Serialize,
{
    let mut buf = Vec::new();
    item.serialize(&mut Serializer::new(&mut buf).with_struct_map())?;
    Ok(buf)
}

/// The standard function for de-serializing codec structs from a format suitable
/// for message exchange between actor and host. Use of any other function to
/// deserialize could result in breaking incompatibilities.
fn deserialize<'de, T: Deserialize<'de>>(
    buf: &[u8],
) -> ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>> {
    let mut de = Deserializer::new(Cursor::new(buf));
    match Deserialize::deserialize(&mut de) {
        Ok(t) => Ok(t),
        Err(e) => Err(format!("Failed to de-serialize: {}", e).into()),
    }
}
