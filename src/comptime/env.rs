use std::collections::{HashMap, HashSet};

use crate::{compiler::{CompilerError, Result}};

use super::value::{CtFunctionHandle, CtValue};

#[derive(Debug, Clone)]
pub struct CtEnv {
    scopes: Vec<CtScope>,
}

#[derive(Debug, Clone, Default)]
struct CtScope {
    values: HashMap<String, CtValue>,
    functions: HashMap<String, CtFunctionHandle>,
    runtime_names: HashSet<String>,
    const_names: HashSet<String>,
}

impl Default for CtEnv {
    fn default() -> Self {
        Self {
            scopes: vec![CtScope::default()],
        }
    }
}

impl CtEnv {
    pub fn push_scope(&mut self) {
        self.scopes.push(CtScope::default());
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    pub fn define_value(&mut self, name: &str, value: CtValue, is_const: bool) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.values.insert(name.to_string(), value);
            if is_const {
                scope.const_names.insert(name.to_string());
            }
        }
    }

    pub fn define_function(&mut self, name: &str, function: CtFunctionHandle) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.functions.insert(name.to_string(), function);
        }
    }

    pub fn define_runtime_name(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.runtime_names.insert(name.to_string());
        }
    }

    pub fn lookup_value(&self, name: &str) -> Option<CtValue> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.values.get(name).cloned())
    }

    pub fn lookup_function(&self, name: &str) -> Option<CtFunctionHandle> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.functions.get(name).cloned())
    }

    pub fn is_runtime_name(&self, name: &str) -> bool {
        self.scopes
            .iter()
            .rev()
            .any(|scope| scope.runtime_names.contains(name))
    }

    pub fn assign_value(&mut self, name: &str, value: CtValue) -> Result<()> {
        for scope in self.scopes.iter_mut().rev() {
            if scope.values.contains_key(name) {
                if scope.const_names.contains(name) {
                    return Err(CompilerError::Other(format!(
                        "Cannot assign to compile-time const `{name}`."
                    )));
                }
                scope.values.insert(name.to_string(), value);
                return Ok(());
            }
        }

        if self.is_runtime_name(name) {
            return Err(CompilerError::Other(format!(
                "Cannot assign to runtime local `{name}` in compile-time code."
            )));
        }

        Err(CompilerError::Other(format!(
            "Unknown compile-time local `{name}`."
        )))
    }
}
