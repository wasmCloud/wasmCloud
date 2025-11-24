use crate::bindings::wrpc::extension::types::InterfaceConfig;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct CalculatorConfig {
    enable_addition: bool,
    enable_subtraction: bool,
    enable_multiplication: bool,
    enable_division: bool,
}

impl CalculatorConfig {
    /// Parse config from HashMap with string values
    pub fn from_config(config: HashMap<String, String>) -> Result<Self, String> {
        let mut calc_config = CalculatorConfig::default();

        for (key, value) in config {
            match key.as_str() {
                "enable_addition" => {
                    calc_config.enable_addition = parse_bool(&value)
                        .map_err(|e| format!("Invalid value for 'enable_addition': {}", e))?;
                }
                "enable_subtraction" => {
                    calc_config.enable_subtraction = parse_bool(&value)
                        .map_err(|e| format!("Invalid value for 'enable_subtraction': {}", e))?;
                }
                "enable_multiplication" => {
                    calc_config.enable_multiplication = parse_bool(&value)
                        .map_err(|e| format!("Invalid value for 'enable_multiplication': {}", e))?;
                }
                "enable_division" => {
                    calc_config.enable_division = parse_bool(&value)
                        .map_err(|e| format!("Invalid value for 'enable_division': {}", e))?;
                }
                _ => {
                    // Ignore unknown config keys or return error if you want strict parsing
                    eprintln!("Unknown config key: {}", key);
                }
            }
        }

        Ok(calc_config)
    }
}

#[derive(Default, Debug, Clone)]
pub struct InterfaceSpecificConfig {
    /// The maximum number a calculator can return from a calculation
    pub max_number: u64,
    /// The minimum number a calculator can return from a calculation
    pub min_number: u64,
}

impl TryFrom<InterfaceConfig> for InterfaceSpecificConfig {
    type Error = String;

    fn try_from(config: InterfaceConfig) -> Result<Self, Self::Error> {
        let config: HashMap<String, String> = config.config.into_iter().collect();

        let max_number = config
            .get("max_number")
            .ok_or("Missing 'max_number' in config")?
            .parse()
            .map_err(|e| format!("Invalid value for 'max_number': {}", e))?;
        let min_number = config
            .get("min_number")
            .ok_or("Missing 'min_number' in config")?
            .parse()
            .map_err(|e| format!("Invalid value for 'min_number': {}", e))?;
        Ok(InterfaceSpecificConfig {
            max_number,
            min_number,
        })
    }
}

/// Parse boolean from string with flexible handling
fn parse_bool(value: &str) -> Result<bool, String> {
    match value.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" | "enabled" => Ok(true),
        "false" | "0" | "no" | "off" | "disabled" => Ok(false),
        _ => Err(format!("Cannot parse '{}' as boolean. Expected: true/false, 1/0, yes/no, on/off, enabled/disabled", value))
    }
}
