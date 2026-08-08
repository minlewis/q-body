//! Outbound tool call schema validation middleware
//!
//! Intercepts hallucinated tool names and validates argument types
//! before any tool call is dispatched to its backend.
//!
//! Inspired by IBM/mcp-context-forge's guardrails architecture:
//! - Tool registry with known schemas
//! - Pre-flight validation of tool name + args
//! - Structured error responses for invalid calls
//!
//! ## Design
//!
//! ```
//! LLM generates tool call
//!     │
//!     ▼
//! router::validate_tool_call(name, args)
//!     │
//!     ├── name not in registry → ToolValidationError::UnknownTool
//!     ├── args missing required field → ToolValidationError::MissingField
//!     ├── args type mismatch → ToolValidationError::TypeMismatch
//!     └── valid → ValidatedToolCall { name, params }
//! ```
//!
//! ## Usage (future — handler.rs runtime wiring)
//!
//! ```ignore
//! let registry = ToolRegistry::new();
//! registry.register(ToolSchema::new("search", ...));
//! let validated = registry.validate("search", &args)?;
//! ```

use std::collections::HashMap;

/// Definition of a single tool's expected input schema.
///
/// Uses a simple field-type map rather than a full JSON Schema
/// dependency to keep the crate surface small.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSchema {
    /// Canonical tool name (matched case-insensitively)
    pub name: String,
    /// Human-readable description of what this tool does
    pub description: String,
    /// Expected parameter fields and their types.
    /// `None` = field is optional (may be absent or null).
    pub params: HashMap<String, ParamType>,
}

/// Supported parameter types for tool argument validation.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamType {
    String,
    Integer,
    Float,
    Boolean,
    Array,
    Object,
    /// Any JSON value (no type checking)
    Any,
}

impl ParamType {
    /// Check if a given JSON value matches this type
    fn matches(&self, value: &serde_json::Value) -> bool {
        match self {
            ParamType::String => value.is_string(),
            ParamType::Integer => value.is_i64() || value.is_u64(),
            ParamType::Float => value.is_f64() || value.is_i64() || value.is_u64(),
            ParamType::Boolean => value.is_boolean(),
            ParamType::Array => value.is_array(),
            ParamType::Object => value.is_object(),
            ParamType::Any => true,
        }
    }
}

/// A tool call that passed validation.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedToolCall {
    /// Canonical tool name (as registered)
    pub name: String,
    /// Validated parameters (subset of the original args that matched schema)
    pub params: HashMap<String, serde_json::Value>,
}

/// Validation error for an outbound tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolValidationError {
    /// Tool name not found in registry
    UnknownTool {
        name: String,
        known_tools: Vec<String>,
    },
    /// Required field is missing from args
    MissingField {
        tool: String,
        field: String,
        param_type: ParamType,
    },
    /// Field value does not match expected type
    TypeMismatch {
        tool: String,
        field: String,
        expected: ParamType,
        actual: String,
    },
}

impl std::fmt::Display for ToolValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolValidationError::UnknownTool { name, known_tools } => {
                write!(
                    f,
                    "Unknown tool '{}'. Known tools: [{}]",
                    name,
                    known_tools.join(", ")
                )
            }
            ToolValidationError::MissingField {
                tool,
                field,
                param_type,
            } => {
                write!(
                    f,
                    "Tool '{}' missing required field '{}' (expected: {:?})",
                    tool, field, param_type
                )
            }
            ToolValidationError::TypeMismatch {
                tool,
                field,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "Tool '{}' field '{}' type mismatch: expected {:?}, got {}",
                    tool, field, expected, actual
                )
            }
        }
    }
}

impl std::error::Error for ToolValidationError {}

/// Registry of known tool schemas.
///
/// Provides schema validation middleware for outbound tool calls.
/// Inspired by IBM/mcp-context-forge's guardrails pattern:
/// register tools at startup, validate every call before dispatch.
#[derive(Debug, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, ToolSchema>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Create an empty tool registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool schema.
    ///
    /// Tool names are stored case-insensitively (lowercased).
    /// If a tool with the same name already exists, it is overwritten.
    pub fn register(&mut self, schema: ToolSchema) {
        let key = schema.name.to_lowercase();
        self.tools.insert(key, schema);
    }

    /// Check if a tool name is registered.
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(&name.to_lowercase())
    }

    /// Get the schema for a registered tool.
    pub fn get_schema(&self, name: &str) -> Option<&ToolSchema> {
        self.tools.get(&name.to_lowercase())
    }

    /// List all registered tool names.
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.values().map(|s| s.name.clone()).collect();
        names.sort();
        names
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Validate an outbound tool call.
    ///
    /// Returns `Ok(ValidatedToolCall)` if the tool name exists and
    /// all required params match their expected types.
    /// Returns `Err(ToolValidationError)` on first validation failure.
    ///
    /// ## Validation rules
    ///
    /// 1. **Tool name**: Must be registered (case-insensitive match).
    /// 2. **Required params**: Every expected param with `ParamType != Any` must
    ///    be present in the args and match its type.
    /// 3. **Optional params**: Fields with `ParamType::Any` may be absent.
    /// 4. **Unknown fields**: Extra fields in args beyond the schema are
    ///    silently ignored (forward-compatible).
    pub fn validate(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<ValidatedToolCall, ToolValidationError> {
        // 1. Validate tool name
        let schema = self
            .get_schema(name)
            .ok_or_else(|| ToolValidationError::UnknownTool {
                name: name.to_string(),
                known_tools: self.tool_names(),
            })?;

        // 2. Validate args — must be an object
        let args_map = match args {
            serde_json::Value::Object(map) => map,
            _ => {
                // Non-object args: all params must be optional (Any) or we fail
                let non_any: Vec<&str> = schema
                    .params
                    .iter()
                    .filter(|(_, pt)| **pt != ParamType::Any)
                    .map(|(name, _)| name.as_str())
                    .collect();

                if non_any.is_empty() {
                    return Ok(ValidatedToolCall {
                        name: schema.name.clone(),
                        params: HashMap::new(),
                    });
                }

                return Err(ToolValidationError::MissingField {
                    tool: schema.name.clone(),
                    field: non_any[0].to_string(),
                    param_type: schema.params[non_any[0]].clone(),
                });
            }
        };

        let mut validated_params = HashMap::new();

        for (field_name, expected_type) in &schema.params {
            match args_map.get(field_name) {
                Some(value) => {
                    if value.is_null() && *expected_type == ParamType::Any {
                        // Null is acceptable for optional/Any fields
                        continue;
                    }
                    if !expected_type.matches(value) {
                        return Err(ToolValidationError::TypeMismatch {
                            tool: schema.name.clone(),
                            field: field_name.clone(),
                            expected: expected_type.clone(),
                            actual: type_name(value),
                        });
                    }
                    validated_params.insert(field_name.clone(), value.clone());
                }
                None => {
                    // Required field missing
                    return Err(ToolValidationError::MissingField {
                        tool: schema.name.clone(),
                        field: field_name.clone(),
                        param_type: expected_type.clone(),
                    });
                }
            }
        }

        Ok(ValidatedToolCall {
            name: schema.name.clone(),
            params: validated_params,
        })
    }
}

/// Human-readable type name for a JSON value.
fn type_name(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(_) => "boolean".to_string(),
        serde_json::Value::Number(n) => {
            if n.is_f64() {
                "float".to_string()
            } else {
                "integer".to_string()
            }
        }
        serde_json::Value::String(_) => "string".to_string(),
        serde_json::Value::Array(_) => "array".to_string(),
        serde_json::Value::Object(_) => "object".to_string(),
    }
}

// ============================================================
// Builder helpers
// ============================================================

/// Build a parameter schema map using a concise DSL.
///
/// # Example
///
/// ```
/// let params = tool_params! {
///     "query" => ParamType::String,
///     "max_results" => ParamType::Integer,
///     "optional_flag" => ParamType::Any,
/// };
/// ```
#[macro_export]
macro_rules! tool_params {
    ( $( $key:expr => $type:expr ),* $(,)? ) => {{
        let mut map = std::collections::HashMap::new();
        $(
            map.insert($key.to_string(), $type);
        )*
        map
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(ToolSchema {
            name: "search".into(),
            description: "Search for information".into(),
            params: tool_params! {
                "query" => ParamType::String,
                "max_results" => ParamType::Integer,
            },
        });
        reg.register(ToolSchema {
            name: "calculate".into(),
            description: "Perform a calculation".into(),
            params: tool_params! {
                "expression" => ParamType::String,
            },
        });
        reg.register(ToolSchema {
            name: "get_weather".into(),
            description: "Get weather for a location".into(),
            params: tool_params! {
                "location" => ParamType::String,
                "units" => ParamType::Any, // optional
            },
        });
        reg
    }

    // ── ToolRegistry basics ──

    #[test]
    fn test_empty_registry() {
        let reg = ToolRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(!reg.has_tool("anything"));
    }

    #[test]
    fn test_register_and_query() {
        let reg = test_registry();
        assert!(!reg.is_empty());
        assert_eq!(reg.len(), 3);
        assert!(reg.has_tool("search"));
        assert!(reg.has_tool("SEARCH"));
        assert!(reg.has_tool("calculate"));
        assert!(!reg.has_tool("nonexistent"));
    }

    #[test]
    fn test_tool_names_sorted() {
        let reg = test_registry();
        let names = reg.tool_names();
        assert_eq!(names, vec!["calculate", "get_weather", "search"]);
    }

    #[test]
    fn test_get_schema() {
        let reg = test_registry();
        let schema = reg.get_schema("search").unwrap();
        assert_eq!(schema.name, "search");
        assert_eq!(schema.description, "Search for information");
        assert!(schema.params.contains_key("query"));
        assert!(schema.params.contains_key("max_results"));
    }

    // ── Validation: success paths ──

    #[test]
    fn test_validate_valid_call() {
        let reg = test_registry();
        let args = serde_json::json!({"query": "rust tooling", "max_results": 10});
        let result = reg.validate("search", &args);
        assert!(result.is_ok());
        let validated = result.unwrap();
        assert_eq!(validated.name, "search");
        assert_eq!(validated.params.get("query").unwrap(), "rust tooling");
        assert_eq!(validated.params.get("max_results").unwrap(), &10);
    }

    #[test]
    fn test_validate_case_insensitive_name() {
        let reg = test_registry();
        let args = serde_json::json!({"query": "test", "max_results": 5});
        let result = reg.validate("SEARCH", &args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name, "search");
    }

    #[test]
    fn test_validate_optional_field_absent() {
        let reg = test_registry();
        let args = serde_json::json!({"location": "Beijing"});
        let result = reg.validate("get_weather", &args);
        assert!(result.is_ok());
        let validated = result.unwrap();
        assert_eq!(validated.name, "get_weather");
        assert_eq!(validated.params.get("location").unwrap(), "Beijing");
        // units is optional — absent is fine
        assert!(!validated.params.contains_key("units"));
    }

    #[test]
    fn test_validate_optional_field_present() {
        let reg = test_registry();
        let args = serde_json::json!({"location": "Beijing", "units": "metric"});
        let result = reg.validate("get_weather", &args);
        assert!(result.is_ok());
        let validated = result.unwrap();
        assert_eq!(validated.params.get("units").unwrap(), "metric");
    }

    #[test]
    fn test_validate_extra_fields_ignored() {
        let reg = test_registry();
        let args = serde_json::json!({
            "query": "test",
            "max_results": 5,
            "extra_field": "should be ignored"
        });
        let result = reg.validate("search", &args);
        assert!(result.is_ok());
        // extra_field is not in validated params
        let validated = result.unwrap();
        assert!(!validated.params.contains_key("extra_field"));
    }

    #[test]
    fn test_validate_float_accepts_integer() {
        let reg = test_registry();
        // Register a tool with float param
        let mut custom = ToolRegistry::new();
        custom.register(ToolSchema {
            name: "measure".into(),
            description: "Take a measurement".into(),
            params: tool_params! {
                "value" => ParamType::Float,
            },
        });
        // Integer should match float
        let args = serde_json::json!({"value": 42});
        assert!(custom.validate("measure", &args).is_ok());
        // Float should also match
        let args2 = serde_json::json!({"value": 3.14});
        assert!(custom.validate("measure", &args2).is_ok());
    }

    // ── Validation: error paths ──

    #[test]
    fn test_validate_unknown_tool() {
        let reg = test_registry();
        let args = serde_json::json!({"query": "test"});
        let err = reg.validate("nonexistent_tool", &args).unwrap_err();
        assert_eq!(
            err,
            ToolValidationError::UnknownTool {
                name: "nonexistent_tool".into(),
                known_tools: vec!["calculate".into(), "get_weather".into(), "search".into()],
            }
        );
    }

    #[test]
    fn test_validate_missing_required_field() {
        let reg = test_registry();
        let args = serde_json::json!({"max_results": 10});
        // "query" is required but missing
        let err = reg.validate("search", &args).unwrap_err();
        assert_eq!(
            err,
            ToolValidationError::MissingField {
                tool: "search".into(),
                field: "query".into(),
                param_type: ParamType::String,
            }
        );
    }

    #[test]
    fn test_validate_type_mismatch_string() {
        let reg = test_registry();
        let args = serde_json::json!({"query": 42, "max_results": 10});
        // query expects string, but got integer
        let err = reg.validate("search", &args).unwrap_err();
        assert_eq!(
            err,
            ToolValidationError::TypeMismatch {
                tool: "search".into(),
                field: "query".into(),
                expected: ParamType::String,
                actual: "integer".into(),
            }
        );
    }

    #[test]
    fn test_validate_type_mismatch_integer() {
        let reg = test_registry();
        let args = serde_json::json!({"query": "test", "max_results": "ten"});
        // max_results expects integer, but got string
        let err = reg.validate("search", &args).unwrap_err();
        assert_eq!(
            err,
            ToolValidationError::TypeMismatch {
                tool: "search".into(),
                field: "max_results".into(),
                expected: ParamType::Integer,
                actual: "string".into(),
            }
        );
    }

    #[test]
    fn test_validate_non_object_args() {
        let reg = test_registry();
        // args is a string, not an object, and tool has required params
        let args = serde_json::json!("just a string");
        let err = reg.validate("search", &args).unwrap_err();
        assert!(matches!(err, ToolValidationError::MissingField { .. }));
    }

    #[test]
    fn test_validate_null_args() {
        let reg = test_registry();
        let args = serde_json::Value::Null;
        let err = reg.validate("search", &args).unwrap_err();
        assert!(matches!(err, ToolValidationError::MissingField { .. }));
    }

    #[test]
    fn test_validate_empty_args_with_all_optional() {
        let mut reg = ToolRegistry::new();
        reg.register(ToolSchema {
            name: "ping".into(),
            description: "Simple health check".into(),
            params: tool_params! {
                "message" => ParamType::Any,
            },
        });
        let args = serde_json::json!({});
        let result = reg.validate("ping", &args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().params.len(), 0);
    }

    // ── Error Display ──

    #[test]
    fn test_unknown_tool_display() {
        let err = ToolValidationError::UnknownTool {
            name: "fly".into(),
            known_tools: vec!["search".into(), "calculate".into()],
        };
        let msg = err.to_string();
        assert!(msg.contains("fly"));
        assert!(msg.contains("search"));
        assert!(msg.contains("calculate"));
    }

    #[test]
    fn test_missing_field_display() {
        let err = ToolValidationError::MissingField {
            tool: "search".into(),
            field: "query".into(),
            param_type: ParamType::String,
        };
        let msg = err.to_string();
        assert!(msg.contains("search"));
        assert!(msg.contains("query"));
    }

    #[test]
    fn test_type_mismatch_display() {
        let err = ToolValidationError::TypeMismatch {
            tool: "calculate".into(),
            field: "expression".into(),
            expected: ParamType::String,
            actual: "integer".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("calculate"));
        assert!(msg.contains("expression"));
        assert!(msg.contains("integer"));
    }

    // ── Serialization roundtrip ──

    #[test]
    fn test_tool_schema_serialize() {
        let schema = ToolSchema {
            name: "search".into(),
            description: "Search".into(),
            params: tool_params! {
                "query" => ParamType::String,
            },
        };
        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains("search"));
        assert!(json.contains("query"));
    }

    #[test]
    fn test_validated_tool_call_serialize() {
        let call = ValidatedToolCall {
            name: "search".into(),
            params: [("query".into(), serde_json::json!("test"))].into(),
        };
        let json = serde_json::to_string(&call).unwrap();
        assert!(json.contains("search"));
        assert!(json.contains("test"));
    }

    // ── Edge cases ──

    #[test]
    fn test_register_overwrites() {
        let mut reg = ToolRegistry::new();
        reg.register(ToolSchema {
            name: "dup".into(),
            description: "first".into(),
            params: tool_params! {},
        });
        reg.register(ToolSchema {
            name: "DUP".into(),
            description: "second".into(),
            params: tool_params! {},
        });
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get_schema("dup").unwrap().description, "second");
    }

    #[test]
    fn test_validate_boolean_type() {
        let mut reg = ToolRegistry::new();
        reg.register(ToolSchema {
            name: "toggle".into(),
            description: "Toggle a feature".into(),
            params: tool_params! {
                "enabled" => ParamType::Boolean,
            },
        });
        assert!(reg.validate("toggle", &serde_json::json!({"enabled": true})).is_ok());
        assert!(reg.validate("toggle", &serde_json::json!({"enabled": "yes"})).is_err());
    }

    #[test]
    fn test_validate_array_type() {
        let mut reg = ToolRegistry::new();
        reg.register(ToolSchema {
            name: "batch".into(),
            description: "Batch process items".into(),
            params: tool_params! {
                "items" => ParamType::Array,
            },
        });
        assert!(reg.validate("batch", &serde_json::json!({"items": [1, 2, 3]})).is_ok());
        assert!(reg.validate("batch", &serde_json::json!({"items": "not-array"})).is_err());
    }

    #[test]
    fn test_validate_object_type() {
        let mut reg = ToolRegistry::new();
        reg.register(ToolSchema {
            name: "config".into(),
            description: "Set configuration".into(),
            params: tool_params! {
                "settings" => ParamType::Object,
            },
        });
        assert!(reg.validate("config", &serde_json::json!({"settings": {"key": "val"}})).is_ok());
        assert!(reg.validate("config", &serde_json::json!({"settings": "string"})).is_err());
    }
}