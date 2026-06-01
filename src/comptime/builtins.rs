use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::Value as JsonValue;

use crate::compiler::{CompilerError, Result};

use super::{
    ComptimeOptions,
    value::{CtArray, CtTable, CtValue},
};

pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "len"
            | "keys"
            | "values"
            | "has"
            | "freeze"
            | "httpGet"
            | "httpJson"
            | "upper"
            | "lower"
            | "replace"
            | "startsWith"
            | "endsWith"
            | "error"
            | "warn"
            | "join"
            | "split"
            | "trim"
    )
}

pub fn call_builtin(name: &str, args: Vec<CtValue>, options: &ComptimeOptions) -> Result<CtValue> {
    match name {
        "len" => {
            expect_arg_count(name, &args, 1)?;
            Ok(CtValue::Number(match &args[0] {
                CtValue::String(value) => value.chars().count() as f64,
                CtValue::Array(array) => array.items.len() as f64,
                CtValue::Table(table) => table.entries.len() as f64,
                other => {
                    return Err(CompilerError::Other(format!(
                        "len expects a string or table, got {}.",
                        other.type_name()
                    )));
                }
            }))
        }
        "keys" => {
            expect_arg_count(name, &args, 1)?;
            match &args[0] {
                CtValue::Array(array) => Ok(CtValue::Array(CtArray {
                    items: (0..array.items.len())
                        .map(|index| CtValue::Number((index + 1) as f64))
                        .collect(),
                    frozen: false,
                })),
                CtValue::Table(table) => Ok(CtValue::Array(CtArray {
                    items: table
                        .entries
                        .iter()
                        .map(|(key, _)| CtValue::String(key.clone()))
                        .collect(),
                    frozen: false,
                })),
                other => Err(CompilerError::Other(format!(
                    "keys expects a table, got {}.",
                    other.type_name()
                ))),
            }
        }
        "values" => {
            expect_arg_count(name, &args, 1)?;
            match &args[0] {
                CtValue::Array(array) => Ok(CtValue::Array(CtArray {
                    items: array.items.clone(),
                    frozen: false,
                })),
                CtValue::Table(table) => Ok(CtValue::Array(CtArray {
                    items: table
                        .entries
                        .iter()
                        .map(|(_, value)| value.clone())
                        .collect(),
                    frozen: false,
                })),
                other => Err(CompilerError::Other(format!(
                    "values expects a table, got {}.",
                    other.type_name()
                ))),
            }
        }
        "has" => {
            expect_arg_count(name, &args, 2)?;
            match (&args[0], &args[1]) {
                (CtValue::Array(array), CtValue::Number(index)) => {
                    let index = as_array_index(*index)?;
                    Ok(CtValue::Bool(index < array.items.len()))
                }
                (CtValue::Table(table), CtValue::String(key)) => Ok(CtValue::Bool(
                    table.entries.iter().any(|(entry, _)| entry == key),
                )),
                (CtValue::Table(_), other) => Err(CompilerError::Other(format!(
                    "has expects a string key for dictionary tables, got {}.",
                    other.type_name()
                ))),
                (other, _) => Err(CompilerError::Other(format!(
                    "has expects a table, got {}.",
                    other.type_name()
                ))),
            }
        }
        "freeze" => {
            expect_arg_count(name, &args, 1)?;
            args[0].clone().with_frozen(true)
        }
        "httpGet" => {
            expect_arg_count(name, &args, 1)?;
            http_get(expect_string(&args[0], name)?, options)
        }
        "httpJson" => {
            expect_arg_count(name, &args, 1)?;
            http_json(expect_string(&args[0], name)?, options)
        }
        "upper" => {
            expect_arg_count(name, &args, 1)?;
            Ok(CtValue::String(
                expect_string(&args[0], name)?.to_uppercase(),
            ))
        }
        "lower" => {
            expect_arg_count(name, &args, 1)?;
            Ok(CtValue::String(
                expect_string(&args[0], name)?.to_lowercase(),
            ))
        }
        "replace" => {
            expect_arg_count(name, &args, 3)?;
            Ok(CtValue::String(expect_string(&args[0], name)?.replace(
                expect_string(&args[1], name)?,
                expect_string(&args[2], name)?,
            )))
        }
        "startsWith" => {
            expect_arg_count(name, &args, 2)?;
            Ok(CtValue::Bool(
                expect_string(&args[0], name)?.starts_with(expect_string(&args[1], name)?),
            ))
        }
        "endsWith" => {
            expect_arg_count(name, &args, 2)?;
            Ok(CtValue::Bool(
                expect_string(&args[0], name)?.ends_with(expect_string(&args[1], name)?),
            ))
        }
        "error" => {
            expect_arg_count(name, &args, 1)?;
            Err(CompilerError::Other(args[0].to_string()))
        }
        "warn" => {
            expect_arg_count(name, &args, 1)?;
            eprintln!("warning: {}", args[0]);
            Ok(CtValue::Nil)
        }
        "join" => {
            expect_arg_count(name, &args, 2)?;
            let CtValue::Array(array) = &args[0] else {
                return Err(CompilerError::Other(format!(
                    "join expects an array, got {}.",
                    args[0].type_name()
                )));
            };
            let separator = expect_string(&args[1], name)?;
            Ok(CtValue::String(
                array
                    .items
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(separator),
            ))
        }
        "split" => {
            expect_arg_count(name, &args, 2)?;
            let value = expect_string(&args[0], name)?;
            let separator = expect_string(&args[1], name)?;
            Ok(CtValue::Array(CtArray {
                items: value
                    .split(separator)
                    .map(|part| CtValue::String(part.to_string()))
                    .collect(),
                frozen: false,
            }))
        }
        "trim" => {
            expect_arg_count(name, &args, 1)?;
            Ok(CtValue::String(
                expect_string(&args[0], name)?.trim().to_string(),
            ))
        }
        _ => Err(CompilerError::Other(format!(
            "Function `{name}` is not available at compile time."
        ))),
    }
}

pub fn call_method(
    receiver: CtValue,
    name: &str,
    mut args: Vec<CtValue>,
    options: &ComptimeOptions,
) -> Result<CtValue> {
    let mut all_args = vec![receiver];
    all_args.append(&mut args);
    call_builtin(name, all_args, options)
}

pub fn table_get(table: &CtTable, key: &str) -> Option<CtValue> {
    table
        .entries
        .iter()
        .find(|(entry, _)| entry == key)
        .map(|(_, value)| value.clone())
}

pub fn table_insert(table: &mut CtTable, key: String, value: CtValue) {
    if let Some((_, existing)) = table.entries.iter_mut().find(|(entry, _)| *entry == key) {
        *existing = value;
    } else {
        table.entries.push((key, value));
    }
}

pub fn as_array_index(value: f64) -> Result<usize> {
    if value.fract() != 0.0 || value < 1.0 {
        return Err(CompilerError::Other(format!(
            "Array indices must be positive integers at compile time, got {value}."
        )));
    }
    Ok((value as usize).saturating_sub(1))
}

fn expect_arg_count(name: &str, args: &[CtValue], count: usize) -> Result<()> {
    if args.len() == count {
        Ok(())
    } else {
        Err(CompilerError::Other(format!(
            "{name} expects {count} argument(s), got {}.",
            args.len()
        )))
    }
}

fn expect_string<'a>(value: &'a CtValue, name: &str) -> Result<&'a str> {
    match value {
        CtValue::String(text) => Ok(text),
        other => Err(CompilerError::Other(format!(
            "{name} expects string arguments, got {}.",
            other.type_name()
        ))),
    }
}

fn http_get(url: &str, options: &ComptimeOptions) -> Result<CtValue> {
    let response = fetch_http(url, options)?;

    Ok(CtValue::Table(CtTable {
        entries: vec![
            ("ok".to_string(), CtValue::Bool(response.ok)),
            (
                "status".to_string(),
                CtValue::Number(response.status as f64),
            ),
            ("url".to_string(), CtValue::String(url.to_string())),
            ("body".to_string(), CtValue::String(response.body)),
        ],
        frozen: true,
    }))
}

fn http_json(url: &str, options: &ComptimeOptions) -> Result<CtValue> {
    let response = fetch_http(url, options)?;
    let parsed: JsonValue = serde_json::from_str(&response.body).map_err(|error| {
        CompilerError::Other(format!(
            "Compile-time HTTP JSON parse failed for `{url}`: {error}"
        ))
    })?;
    Ok(deep_freeze(json_to_ct_value(parsed)?))
}

fn fetch_http(url: &str, options: &ComptimeOptions) -> Result<HttpResponse> {
    let http = &options.http;
    if !http.enabled {
        return Err(CompilerError::Other(
            "Compile-time HTTP is disabled. Enable `comptimeHttp.enabled` in xluau.config.json."
                .to_string(),
        ));
    }

    if !matches!(
        url.split_once("://"),
        Some(("http", _)) | Some(("https", _))
    ) {
        return Err(CompilerError::Other(format!(
            "Compile-time HTTP only supports http:// and https:// URLs, got `{url}`."
        )));
    }

    if !http.allow.iter().any(|allowed| url.starts_with(allowed)) {
        return Err(CompilerError::Other(format!(
            "Compile-time HTTP request to `{url}` is not allowed. Add a matching prefix to `comptimeHttp.allow`."
        )));
    }

    let client = Client::builder()
        .timeout(Duration::from_millis(http.timeout_ms))
        .build()
        .map_err(|error| {
            CompilerError::Other(format!(
                "Failed to initialize compile-time HTTP client: {error}"
            ))
        })?;

    let response = client.get(url).send().map_err(|error| {
        CompilerError::Other(format!("Compile-time HTTP GET failed for `{url}`: {error}"))
    })?;
    let status = response.status();
    let body = response.text().map_err(|error| {
        CompilerError::Other(format!(
            "Failed to read compile-time HTTP response body for `{url}`: {error}"
        ))
    })?;

    Ok(HttpResponse {
        ok: status.is_success(),
        status: status.as_u16(),
        body,
    })
}

fn json_to_ct_value(value: JsonValue) -> Result<CtValue> {
    match value {
        JsonValue::Null => Ok(CtValue::Nil),
        JsonValue::Bool(value) => Ok(CtValue::Bool(value)),
        JsonValue::Number(value) => value.as_f64().map(CtValue::Number).ok_or_else(|| {
            CompilerError::Other("Compile-time JSON numbers must fit in an f64.".to_string())
        }),
        JsonValue::String(value) => Ok(CtValue::String(value)),
        JsonValue::Array(items) => Ok(CtValue::Array(CtArray {
            items: items
                .into_iter()
                .map(json_to_ct_value)
                .collect::<Result<Vec<_>>>()?,
            frozen: false,
        })),
        JsonValue::Object(entries) => Ok(CtValue::Table(CtTable {
            entries: entries
                .into_iter()
                .map(|(key, value)| Ok((key, json_to_ct_value(value)?)))
                .collect::<Result<Vec<_>>>()?,
            frozen: false,
        })),
    }
}

fn deep_freeze(value: CtValue) -> CtValue {
    match value {
        CtValue::Array(array) => CtValue::Array(CtArray {
            items: array.items.into_iter().map(deep_freeze).collect(),
            frozen: true,
        }),
        CtValue::Table(table) => CtValue::Table(CtTable {
            entries: table
                .entries
                .into_iter()
                .map(|(key, value)| (key, deep_freeze(value)))
                .collect(),
            frozen: true,
        }),
        other => other,
    }
}

struct HttpResponse {
    ok: bool,
    status: u16,
    body: String,
}
