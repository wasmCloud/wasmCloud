use aws_sdk_dynamodb::{
    model::{AttributeValue, ReturnConsumedCapacity},
    types::Blob,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::{debug, error, instrument};
use wasmbus_rpc::{minicbor, provider::prelude::*};
use wasmcloud_interface_sqldb::{ExecuteResult, Parameters, QueryResult, SqlDb, Statement};

mod config;
pub use config::StorageConfig;
mod error;
use error::DbError;

/// sqldb-dynamodb capability provider implementation
#[derive(Clone)]
pub struct SqlDbClient {
    dynamodb_client: aws_sdk_dynamodb::Client,
    ld: Option<LinkDefinition>,
}

impl SqlDbClient {
    pub async fn new(config: config::StorageConfig, ld: Option<LinkDefinition>) -> Self {
        let dynamodb_config = aws_sdk_dynamodb::Config::from(&config.configure_aws().await);
        let dynamodb_client = aws_sdk_dynamodb::Client::from_conf(dynamodb_config);
        SqlDbClient {
            dynamodb_client,
            ld,
        }
    }

    /// async implementation of Default
    pub async fn async_default() -> Self {
        Self::new(StorageConfig::default(), None).await
    }

    /// Perform any cleanup necessary for a link + dynamodb connection
    pub async fn close(&self) {
        if let Some(ld) = &self.ld {
            debug!(actor_id = %ld.actor_id, "sqldb-dynamodb dropping linkdef");
        }
    }
}

/// Handle SqlDb methods
#[async_trait]
impl SqlDb for SqlDbClient {
    /// Perform reads and singleton writes using PartiQL
    #[instrument(level = "debug", skip(self, _ctx, stmt), fields(actor_id = ?_ctx.actor))]
    async fn execute(&self, _ctx: &Context, stmt: &Statement) -> RpcResult<ExecuteResult> {
        let parameters: Option<Vec<AttributeValue>> = match convert_parameters(&stmt.parameters) {
            Ok(p) => p,
            Err(e) => return Err(e),
        };
        match self
            .dynamodb_client
            .execute_statement()
            .statement(&stmt.sql)
            .set_parameters(parameters)
            .return_consumed_capacity(ReturnConsumedCapacity::Total)
            .send()
            .await
        {
            Ok(output) => Ok(ExecuteResult {
                rows_affected: match output.consumed_capacity() {
                    Some(capacity) => capacity.capacity_units().unwrap_or(0_f64) as u64,
                    None => 0_u64,
                },
                ..Default::default()
            }),
            Err(db_err) => {
                error!(
                    statement = ?stmt,
                    error = %db_err,
                    "Error executing statement"
                );
                Ok(ExecuteResult {
                    error: Some(DbError::Db(db_err.to_string()).into()),
                    ..Default::default()
                })
            }
        }
    }

    /// perform select query on database, returning all result rows
    #[instrument(level = "debug", skip(self, _ctx, stmt), fields(actor_id = ?_ctx.actor))]
    async fn query(&self, _ctx: &Context, stmt: &Statement) -> RpcResult<QueryResult> {
        // Same as execute, except the result is returned.
        // The dynamodb client does have a query method, however, it requires a number of
        // fields and would require changes to the Statement struct.
        let parameters: Option<Vec<AttributeValue>> = match convert_parameters(&stmt.parameters) {
            Ok(p) => p,
            Err(e) => return Err(e),
        };
        match self
            .dynamodb_client
            .execute_statement()
            .statement(&stmt.sql)
            .set_parameters(parameters)
            .return_consumed_capacity(ReturnConsumedCapacity::Total)
            .send()
            .await
        {
            Ok(output) => match convert_rows(output.items()) {
                Ok(buf) => Ok(QueryResult {
                    num_rows: match output.consumed_capacity() {
                        Some(capacity) => capacity.capacity_units().unwrap_or(0_f64) as u64,
                        None => 0_u64,
                    },
                    rows: buf,
                    ..Default::default()
                }),
                Err(e) => Ok(QueryResult {
                    error: Some(e.into()),
                    ..Default::default()
                }),
            },
            Err(db_err) => {
                error!(
                    statement = ?stmt,
                    error = %db_err,
                    "Error executing statement"
                );
                Ok(QueryResult {
                    error: Some(DbError::Db(db_err.to_string()).into()),
                    ..Default::default()
                })
            }
        }
    }
}

// convert DynamoDB results to CBOR encoded.
fn convert_rows(items: Option<&[HashMap<String, AttributeValue>]>) -> Result<Vec<u8>, DbError> {
    let rows = match items {
        Some(r) => r,
        None => return Ok(Vec::new()),
    };
    // Convert items into a big json value.
    let json_rows: Value = json!(rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|(k, v)| (k.to_owned(), attribute_value_to_json(v.to_owned())))
                .collect::<HashMap<String, Value>>()
        })
        .collect::<Vec<HashMap<String, Value>>>());
    let string_for_serialization = json_rows.to_string();
    debug!("string_for_serialization {}", string_for_serialization);
    let mut buf: Vec<u8> = Vec::with_capacity(string_for_serialization.len());
    minicbor::encode(&string_for_serialization, &mut buf)
        .map_err(|e| DbError::Encoding(e.to_string()))?;
    Ok(buf)
}

// convert AttributeValue to string.
fn attribute_value_to_json(attribute_value: AttributeValue) -> Value {
    match attribute_value {
        // "B": "dGhpcyB0ZXh0IGlzIGJhc2U2NC1lbmNvalZGVk"
        AttributeValue::B(blob) => json!({"B": String::from_utf8(blob.into_inner()).unwrap()}),
        // "BOOL": true
        AttributeValue::Bool(b) => json!({ "BOOL": b }),
        // "BS": ["U3Vubnk=", "UmFpbnk=", "U25vd3k="]
        AttributeValue::Bs(blob_list) => json!({
            "BS":
            blob_list
                .into_iter()
                .map(|blob| String::from_utf8(blob.into_inner()).unwrap())
                .collect::<Vec<String>>()
        }),
        // "L": [ {"S": "Cookies"} , {"S": "Coffee"}, {"N": 5.14159}]
        AttributeValue::L(list) => json!({
            "L":
            list.into_iter()
                .map(attribute_value_to_json)
                .collect::<Vec<Value>>()
        }),
        // "M": {"Name": {"S": "Joe"}, "Age": {"N": 35}}
        AttributeValue::M(map) => json!({
            "M":
                map.into_iter()
                    .map(|(k, v)| (k, attribute_value_to_json(v)))
                    .collect::<HashMap<String, Value>>()
        }),
        // "N": "123.45"
        AttributeValue::N(num) => json!({ "N": num }),
        // "NS": ["42.2", "-19", "7.5", "5.14"]
        AttributeValue::Ns(list) => json!({ "NS": list }),
        // "NULL": true
        AttributeValue::Null(b) => json!({ "NULL": b }),
        // "S": "Hello"
        AttributeValue::S(s) => json!({ "S": s }),
        // "SS": ["Giraffe", "Hippo" ,"Zebra"]
        AttributeValue::Ss(list) => json!({ "SS": list }),
        _ => json!({ "Unknown": "Unknown" }),
    }
}

// convert DynamoDB parameters in JSON form in Vec<u8> into Enum AttributeValue
fn convert_parameters(parameters: &Option<Parameters>) -> RpcResult<Option<Vec<AttributeValue>>> {
    let parameters = match parameters {
        Some(p) => p,
        None => return Ok(None),
    };
    let result = parameters
        .iter()
        .map(|parameter| {
            let json_value = serde_json::from_slice(parameter).map_err(|e| {
                RpcError::InvalidParameter(format!(
                    "parameter {:?} has invalid json format {:?}",
                    parameter, e
                ))
            })?;
            parameter_to_attribute_value(None, json_value).ok_or_else(|| {
                RpcError::InvalidParameter(format!("parameter {:?} has unknown type", parameter))
            })
        })
        .collect::<RpcResult<Vec<AttributeValue>>>()?;
    Ok(Some(result))
}

fn parameter_to_attribute_value(attribute: Option<&str>, value: Value) -> Option<AttributeValue> {
    match (attribute, value) {
        (None, Value::Object(val)) => {
            // This is the first pass for any parameter. Subsequent calls will include the
            // attribute value. ex. S, BS, BOOL. There should only be a single AttributeValue
            // returned here as hashmaps will have the M type.
            if val.keys().len() != 1 {
                error!(
                    "InvalidParameter, only one key should be passed with no AttributeValue
                    value type"
                );
                return None;
            }
            val.into_iter()
                .map(|(k, v)| parameter_to_attribute_value(Some(&k), v))
                .next()
                .unwrap() // We know there is only one element
        }
        (Some("B"), Value::String(val)) => {
            // "B": "dGhpcyB0ZXh0IGlzIGJhc2U2NC1lbmNvalZGVk"
            Some(AttributeValue::B(Blob::new(val)))
        }
        (Some("BOOL"), Value::Bool(val)) => {
            // "BOOL": true
            Some(AttributeValue::Bool(val))
        }
        (Some("BS"), Value::Array(val)) => {
            // "BS": ["U3Vubnk=", "UmFpbnk=", "U25vd3k="]
            Some(AttributeValue::Bs(
                val.into_iter()
                    //to_string() does not strip quotes
                    .map(|v| Blob::new(v.as_str().unwrap()))
                    .collect::<Vec<Blob>>(),
            ))
        }
        (Some("L"), Value::Array(val)) => {
            // "L": [ {"S": "Cookies"} , {"S": "Coffee"}, {"N": "5.14159"}]
            Some(AttributeValue::L(
                val.into_iter()
                    .filter_map(|v| parameter_to_attribute_value(None, v))
                    .collect::<Vec<AttributeValue>>(),
            ))
        }
        (Some("M"), Value::Object(val)) => {
            // "M": {"Name": {"S": "Joe"}, "Age": {"N": "35"}}
            Some(AttributeValue::M(
                val.into_iter()
                    .map(|(k, v)| (k, parameter_to_attribute_value(None, v).unwrap()))
                    .collect::<HashMap<String, AttributeValue>>(),
            ))
        }
        (Some("N"), Value::String(val)) => {
            // "N": "123.45"
            Some(AttributeValue::N(val))
        }
        (Some("N"), Value::Number(val)) => {
            // "N": 123.45, This is not consistent with the DynamoDB API, but is otherwise OK to
            // fix it here.
            Some(AttributeValue::N(val.to_string()))
        }
        (Some("NS"), Value::Array(val)) => {
            // "NS": ["42.2", "-19", "7.5", "5.14"]
            Some(AttributeValue::Ns(
                val.into_iter()
                    .map(|v| match v {
                        Value::String(s) => s,
                        _ => v.to_string(),
                    })
                    .collect::<Vec<String>>(),
            ))
        }
        (Some("NULL"), Value::Bool(val)) => {
            // "NULL": true
            Some(AttributeValue::Null(val))
        }
        (Some("S"), Value::String(val)) => {
            // "S": "Hello"
            Some(AttributeValue::S(val))
        }
        (Some("SS"), Value::Array(val)) => {
            // "SS": ["Giraffe", "Hippo" ,"Zebra"]
            Some(AttributeValue::Ss(
                val.into_iter()
                    .map(|s| match s {
                        Value::String(s) => s,
                        _ => s.to_string(),
                    })
                    .collect::<Vec<String>>(),
            ))
        }
        (code, val) => {
            error!("unrecognized parameter {:?}: {:?}", &code, &val);
            None
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_convert_rows() {
        let test_json = json!([{"Name": {"S": "Joe"}}]).to_string();
        let test_length = test_json.len();
        let mut buf: Vec<u8> = Vec::with_capacity(test_length);
        minicbor::encode(&test_json, &mut buf).unwrap();
        let result = convert_rows(Some(&[HashMap::from([(
            "Name".to_string(),
            AttributeValue::S("Joe".to_string()),
        )])]))
        .unwrap();
        println!("{}", std::str::from_utf8(&buf).unwrap());
        println!("{}", std::str::from_utf8(&result).unwrap());
        assert_eq!(result, buf)
    }

    #[test]
    fn test_convert_parameters() {
        assert_eq!(
            vec!(AttributeValue::B(Blob::new(
                "dGhpcyB0ZXh0IGlzIGJhc2U2NC1lbmNvZGVk"
            ))),
            convert_parameters(&Some(vec!(
                br#"{"B": "dGhpcyB0ZXh0IGlzIGJhc2U2NC1lbmNvZGVk"}"#.to_vec()
            )))
            .unwrap()
            .unwrap()
        );

        // Too many parameters at top level.
        let result = convert_parameters(&Some(vec![
            br#"{"B": "dGhpcyB0ZXh0IGlzIGJhc2U2NC1lbmNvZGVk", "S": "Cookies"}"#.to_vec(),
        ]));
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("unknown type"));

        // Invalid JSON
        let result = convert_parameters(&Some(vec![
            br#"[{"B": "dGhpcyB0ZXh0IGlzIGJhc2U2NC1lbmNvZGVk"}, {"S": "Cookies}"#.to_vec(),
        ]));
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("invalid json format"));
    }

    #[test]
    fn test_attribute_value_to_json() {
        assert_eq!(
            attribute_value_to_json(AttributeValue::B(Blob::new(
                "dGhpcyB0ZXh0IGlzIGJhc2U2NC1lbmNvZGVk"
            ))),
            json!({"B": "dGhpcyB0ZXh0IGlzIGJhc2U2NC1lbmNvZGVk"})
        );
        assert_eq!(
            attribute_value_to_json(AttributeValue::Bool(true)),
            json!({"BOOL": true})
        );
        assert_eq!(
            attribute_value_to_json(AttributeValue::Bs(vec!(
                Blob::new("U3Vubnk="),
                Blob::new("UmFpbnk="),
                Blob::new("U25vd3k=")
            ))),
            json!({"BS": ["U3Vubnk=", "UmFpbnk=", "U25vd3k="]})
        );
        assert_eq!(
            attribute_value_to_json(AttributeValue::L(vec!(
                AttributeValue::S("Cookies".to_string()),
                AttributeValue::S("Coffee".to_string()),
                AttributeValue::N("3.14159".to_string())
            ))),
            json!({"L": [ {"S": "Cookies"} , {"S": "Coffee"}, {"N": "3.14159"}]})
        );
        assert_eq!(
            attribute_value_to_json(AttributeValue::M(HashMap::from([
                ("Name".to_string(), AttributeValue::S("Joe".to_string())),
                ("Age".to_string(), AttributeValue::N("35".to_string()))
            ]))),
            json!({"M": {"Name": {"S": "Joe"}, "Age": {"N": "35"}}})
        );
        assert_eq!(
            attribute_value_to_json(AttributeValue::N("123.45".to_string())),
            json!({"N": "123.45"})
        );
        assert_eq!(
            attribute_value_to_json(AttributeValue::Ns(vec!(
                "42.2".to_string(),
                "-19".to_string(),
                "7.5".to_string(),
                "3.14".to_string()
            ))),
            json!({"NS": ["42.2", "-19", "7.5", "3.14"]})
        );
        assert_eq!(
            attribute_value_to_json(AttributeValue::Null(true)),
            json!({"NULL": true})
        );
        assert_eq!(
            attribute_value_to_json(AttributeValue::Ss(vec!(
                "Giraffe".to_string(),
                "Hippo".to_string(),
                "Zebra".to_string()
            ))),
            json!({"SS": ["Giraffe", "Hippo" ,"Zebra"]})
        );
        assert_eq!(
            attribute_value_to_json(AttributeValue::S("hello".to_string())),
            json!({"S": "hello"})
        );
    }

    #[test]
    fn test_parameter_to_attribute_value() {
        assert_eq!(
            AttributeValue::B(Blob::new("dGhpcyB0ZXh0IGlzIGJhc2U2NC1lbmNvZGVk")),
            parameter_to_attribute_value(
                None,
                json!({"B": "dGhpcyB0ZXh0IGlzIGJhc2U2NC1lbmNvZGVk"})
            )
            .unwrap()
        );
        assert_eq!(
            AttributeValue::Bool(true),
            parameter_to_attribute_value(None, json!({"BOOL": true})).unwrap()
        );
        assert_eq!(
            AttributeValue::Bs(vec!(
                Blob::new("U3Vubnk="),
                Blob::new("UmFpbnk="),
                Blob::new("U25vd3k=")
            )),
            parameter_to_attribute_value(None, json!({"BS": ["U3Vubnk=", "UmFpbnk=", "U25vd3k="]}))
                .unwrap()
        );
        assert_eq!(
            AttributeValue::L(vec!(
                AttributeValue::S("Cookies".to_string()),
                AttributeValue::S("Coffee".to_string()),
                AttributeValue::N("5.14159".to_string())
            )),
            parameter_to_attribute_value(
                None,
                json!({"L": [ {"S": "Cookies"} , {"S": "Coffee"}, {"N": 5.14159}]})
            )
            .unwrap()
        );
        assert_eq!(
            AttributeValue::M(HashMap::from([
                ("Name".to_string(), AttributeValue::S("Joe".to_string())),
                ("Age".to_string(), AttributeValue::N("35".to_string()))
            ])),
            parameter_to_attribute_value(
                None,
                json!({"M": {"Name": {"S": "Joe"}, "Age": {"N": "35"}}})
            )
            .unwrap()
        );
        assert_eq!(
            AttributeValue::N("123.45".to_string()),
            parameter_to_attribute_value(None, json!({"N": 123.45})).unwrap()
        );
        assert_eq!(
            AttributeValue::Ns(vec!(
                "42.2".to_string(),
                "-19".to_string(),
                "7.5".to_string(),
                "3.14".to_string()
            )),
            parameter_to_attribute_value(None, json!({"NS": [42.2, "-19", "7.5", "3.14"]}))
                .unwrap()
        );
        assert_eq!(
            AttributeValue::Null(true),
            parameter_to_attribute_value(None, json!({"NULL": true})).unwrap()
        );
        assert_eq!(
            AttributeValue::Ss(vec!(
                "Giraffe".to_string(),
                "Hippo".to_string(),
                "Zebra".to_string()
            )),
            parameter_to_attribute_value(None, json!({"SS": ["Giraffe", "Hippo" ,"Zebra"]}))
                .unwrap()
        );
        assert_eq!(
            AttributeValue::S("hello".to_string()),
            parameter_to_attribute_value(None, json!({"S": "hello"})).unwrap()
        );
    }
}
