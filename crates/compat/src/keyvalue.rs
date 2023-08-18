use serde::{Deserialize, Serialize};

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
