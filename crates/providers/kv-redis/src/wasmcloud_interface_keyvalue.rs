use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use wasmcloud_provider_sdk::Context;

#[allow(dead_code)]
pub const SMITHY_VERSION: &str = "1.0";

/// Response to get request
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GetResponse {
    /// the value, if it existed
    #[serde(default)]
    pub value: String,
    /// whether or not the value existed
    #[serde(default)]
    pub exists: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IncrementRequest {
    /// name of value to increment
    #[serde(default)]
    pub key: String,
    /// amount to add to value
    #[serde(default)]
    pub value: i32,
}

/// Parameter to ListAdd operation
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ListAddRequest {
    /// name of the list to modify
    #[serde(rename = "listName")]
    #[serde(default)]
    pub list_name: String,
    /// value to append to the list
    #[serde(default)]
    pub value: String,
}

/// Removes an item from the list. If the item occurred more than once,
/// removes only the first item.
/// Returns true if the item was found.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ListDelRequest {
    /// name of list to modify
    #[serde(rename = "listName")]
    #[serde(default)]
    pub list_name: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ListRangeRequest {
    /// name of list
    #[serde(rename = "listName")]
    #[serde(default)]
    pub list_name: String,
    /// start index of the range, 0-based, inclusive.
    #[serde(default)]
    pub start: i32,
    /// end index of the range, 0-based, inclusive.
    #[serde(default)]
    pub stop: i32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetAddRequest {
    /// name of the set
    #[serde(rename = "setName")]
    #[serde(default)]
    pub set_name: String,
    /// value to add to the set
    #[serde(default)]
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetDelRequest {
    #[serde(rename = "setName")]
    #[serde(default)]
    pub set_name: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetRequest {
    /// the key name to change (or create)
    #[serde(default)]
    pub key: String,
    /// the new value
    #[serde(default)]
    pub value: String,
    /// expiration time in seconds 0 for no expiration
    #[serde(default)]
    pub expires: u32,
}

/// list of strings
pub type StringList = Vec<String>;

/// wasmbus.contractId: wasmcloud:keyvalue
/// wasmbus.providerReceive
#[async_trait]
pub trait KeyValue {
    /// returns the capability contract id for this interface
    fn contract_id() -> &'static str {
        "wasmcloud:keyvalue"
    }
    /// Increments a numeric value, returning the new value
    async fn increment(&self, ctx: Context, arg: IncrementRequest) -> Result<i32, String>;
    /// returns whether the store contains the key
    async fn contains(&self, ctx: Context, arg: String) -> Result<bool, String>;
    /// Deletes a key, returning true if the key was deleted
    async fn del(&self, ctx: Context, arg: String) -> Result<bool, String>;
    /// Gets a value for a specified key. If the key exists,
    /// the return structure contains exists: true and the value,
    /// otherwise the return structure contains exists == false.
    async fn get(&self, ctx: Context, arg: String) -> Result<GetResponse, String>;
    /// Append a value onto the end of a list. Returns the new list size
    async fn list_add(&self, ctx: Context, arg: ListAddRequest) -> Result<u32, String>;
    /// Deletes a list and its contents
    /// input: list name
    /// output: true if the list existed and was deleted
    async fn list_clear(&self, ctx: Context, arg: String) -> Result<bool, String>;
    /// Deletes a value from a list. Returns true if the item was removed.
    async fn list_del(&self, ctx: Context, arg: ListDelRequest) -> Result<bool, String>;
    /// Retrieves a range of values from a list using 0-based indices.
    /// Start and end values are inclusive, for example, (0,10) returns
    /// 11 items if the list contains at least 11 items. If the stop value
    /// is beyond the end of the list, it is treated as the end of the list.
    async fn list_range(&self, ctx: Context, arg: ListRangeRequest) -> Result<StringList, String>;
    /// Sets the value of a key.
    /// expires is an optional number of seconds before the value should be automatically deleted,
    /// or 0 for no expiration.
    async fn set(&self, ctx: Context, arg: SetRequest) -> Result<(), String>;
    /// Add an item into a set. Returns number of items added (1 or 0)
    async fn set_add(&self, ctx: Context, arg: SetAddRequest) -> Result<u32, String>;
    /// Deletes an item from the set. Returns number of items removed from the set (1 or 0)
    async fn set_del(&self, ctx: Context, arg: SetDelRequest) -> Result<u32, String>;
    /// perform intersection of sets and returns values from the intersection.
    /// input: list of sets for performing intersection (at least two)
    /// output: values
    async fn set_intersection(&self, ctx: Context, arg: StringList) -> Result<StringList, String>;
    /// Retrieves all items from a set
    /// input: String
    /// output: set members
    async fn set_query(&self, ctx: Context, arg: String) -> Result<StringList, String>;
    /// perform union of sets and returns values from the union
    /// input: list of sets for performing union (at least two)
    /// output: union of values
    async fn set_union(&self, ctx: Context, arg: StringList) -> Result<StringList, String>;
    /// clears all values from the set and removes it
    /// input: set name
    /// output: true if the set existed and was deleted
    async fn set_clear(&self, ctx: Context, arg: String) -> Result<bool, String>;
}
