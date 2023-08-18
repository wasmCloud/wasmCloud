use serde::{Deserialize, Serialize};

/// Input range for RandomInRange, inclusive. Result will be >= min and <= max
/// Example:
/// random_in_range(RangeLimit{0,4}) returns one the values, 0, 1, 2, 3, or 4.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RangeLimit {
    #[serde(default)]
    pub min: u32,
    #[serde(default)]
    pub max: u32,
}
