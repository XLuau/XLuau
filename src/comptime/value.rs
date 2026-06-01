use std::{fmt, sync::Arc};

use crate::{
    ast::{Block, Expr, Param, TableField},
    compiler::{CompilerError, Result},
};

use super::{ComptimeOptions, env::CtEnv};

#[derive(Debug, Clone)]
pub enum CtValue {
    Nil,
    Bool(bool),
    Number(f64),
    String(String),
    Array(CtArray),
    Table(CtTable),
    Function(CtFunctionHandle),
}

#[derive(Debug, Clone)]
pub struct CtArray {
    pub items: Vec<CtValue>,
    pub frozen: bool,
}

#[derive(Debug, Clone)]
pub struct CtTable {
    pub entries: Vec<(String, CtValue)>,
    pub frozen: bool,
}

pub type CtFunctionHandle = Arc<CtFunction>;

#[derive(Debug, Clone)]
pub struct CtFunction {
    pub name: Option<String>,
    pub params: Vec<Param>,
    pub body: Block,
    pub env: CtEnv,
    pub options: ComptimeOptions,
}

impl CtValue {
    pub fn truthy(&self) -> bool {
        !matches!(self, CtValue::Nil | CtValue::Bool(false))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            CtValue::Nil => "nil",
            CtValue::Bool(_) => "boolean",
            CtValue::Number(_) => "number",
            CtValue::String(_) => "string",
            CtValue::Array(_) | CtValue::Table(_) => "table",
            CtValue::Function(_) => "function",
        }
    }

    pub fn with_frozen(self, frozen: bool) -> Result<Self> {
        match self {
            CtValue::Array(mut array) => {
                array.frozen = frozen;
                Ok(CtValue::Array(array))
            }
            CtValue::Table(mut table) => {
                table.frozen = frozen;
                Ok(CtValue::Table(table))
            }
            other => Err(CompilerError::Other(format!(
                "Only tables can be frozen at compile time, got {}.",
                other.type_name()
            ))),
        }
    }

    pub fn to_expr(&self) -> Result<Expr> {
        match self {
            CtValue::Nil => Ok(Expr::Nil),
            CtValue::Bool(value) => Ok(Expr::Bool(*value)),
            CtValue::Number(value) => Ok(Expr::Number(format_number(*value))),
            CtValue::String(value) => Ok(Expr::String(serde_json::to_string(value).map_err(
                |error| {
                    CompilerError::Other(format!(
                        "failed to serialize compile-time string literal: {error}"
                    ))
                },
            )?)),
            CtValue::Array(array) => {
                let expr = Expr::Table(
                    array
                        .items
                        .iter()
                        .map(|value| value.to_expr().map(TableField::Value))
                        .collect::<Result<Vec<_>>>()?,
                );
                if array.frozen {
                    Ok(Expr::Freeze(Box::new(expr)))
                } else {
                    Ok(expr)
                }
            }
            CtValue::Table(table) => {
                let expr = Expr::Table(
                    table
                        .entries
                        .iter()
                        .map(|(key, value)| {
                            let value = value.to_expr()?;
                            if is_identifier(key) {
                                Ok(TableField::Named(key.clone(), value))
                            } else {
                                Ok(TableField::Indexed(
                                    Expr::String(serde_json::to_string(key).map_err(|error| {
                                        CompilerError::Other(format!(
                                            "failed to serialize compile-time table key: {error}"
                                        ))
                                    })?),
                                    value,
                                ))
                            }
                        })
                        .collect::<Result<Vec<_>>>()?,
                );
                if table.frozen {
                    Ok(Expr::Freeze(Box::new(expr)))
                } else {
                    Ok(expr)
                }
            }
            CtValue::Function(_) => Err(CompilerError::Other(
                "Compile-time functions cannot be embedded into runtime code.".to_string(),
            )),
        }
    }
}

impl PartialEq for CtValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CtValue::Nil, CtValue::Nil) => true,
            (CtValue::Bool(left), CtValue::Bool(right)) => left == right,
            (CtValue::Number(left), CtValue::Number(right)) => left == right,
            (CtValue::String(left), CtValue::String(right)) => left == right,
            (CtValue::Array(left), CtValue::Array(right)) => left.items == right.items,
            (CtValue::Table(left), CtValue::Table(right)) => left.entries == right.entries,
            (CtValue::Function(left), CtValue::Function(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Eq for CtValue {}

impl fmt::Display for CtValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CtValue::Nil => write!(f, "nil"),
            CtValue::Bool(value) => write!(f, "{value}"),
            CtValue::Number(value) => write!(f, "{}", format_number(*value)),
            CtValue::String(value) => write!(f, "{value}"),
            CtValue::Array(array) => {
                let rendered = array
                    .items
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "[{rendered}]")
            }
            CtValue::Table(table) => {
                let rendered = table
                    .entries
                    .iter()
                    .map(|(key, value)| format!("{key} = {value}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{{{rendered}}}")
            }
            CtValue::Function(_) => write!(f, "<comptime function>"),
        }
    }
}

pub fn decode_string_literal(raw: &str) -> Result<String> {
    if raw.starts_with('[') {
        let equals = raw.chars().skip(1).take_while(|ch| *ch == '=').count();
        let start = 2 + equals;
        let end = raw.len().saturating_sub(2 + equals);
        return Ok(raw[start..end].to_string());
    }

    let mut chars = raw.chars();
    let Some(quote) = chars.next() else {
        return Ok(String::new());
    };
    let mut inner = raw[quote.len_utf8()..].to_string();
    inner.pop();

    let mut decoded = String::new();
    let mut escaped = false;
    for ch in inner.chars() {
        if escaped {
            decoded.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '\'' => '\'',
                '"' => '"',
                '`' => '`',
                other => other,
            });
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
        } else {
            decoded.push(ch);
        }
    }

    if escaped {
        decoded.push('\\');
    }

    Ok(decoded)
}

pub fn parse_number_literal(raw: &str) -> Result<f64> {
    let text = raw.replace('_', "");
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16)
            .map(|value| value as f64)
            .map_err(|_| CompilerError::Other(format!("Unsupported numeric literal `{raw}`.")));
    }
    text.parse::<f64>()
        .map_err(|_| CompilerError::Other(format!("Unsupported numeric literal `{raw}`.")))
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some(ch) if ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
