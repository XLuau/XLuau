use std::sync::Arc;

use crate::{
    ast::*,
    compiler::{CompilerError, Result},
};

use super::{
    ComptimeOptions,
    builtins::{as_array_index, call_builtin, call_method, is_builtin, table_get, table_insert},
    env::CtEnv,
    value::{
        CtArray, CtFunctionHandle, CtTable, CtValue, decode_string_literal, parse_number_literal,
    },
};

pub struct Evaluator {
    env: CtEnv,
    options: ComptimeOptions,
}

enum Flow {
    Continue,
    Return(Vec<CtValue>),
}

impl Evaluator {
    pub fn new(env: CtEnv, options: ComptimeOptions) -> Self {
        Self { env, options }
    }

    pub fn eval_expr(&mut self, expr: &Expr) -> Result<CtValue> {
        match expr {
            Expr::Comptime(inner) => self.eval_expr(inner),
            Expr::Nil => Ok(CtValue::Nil),
            Expr::Bool(value) => Ok(CtValue::Bool(*value)),
            Expr::Number(value) => Ok(CtValue::Number(parse_number_literal(value)?)),
            Expr::String(value) => Ok(CtValue::String(decode_string_literal(value)?)),
            Expr::Paren(inner) => self.eval_expr(inner),
            Expr::Unary { op, expr } => self.eval_unary(*op, expr),
            Expr::Binary { left, op, right } => self.eval_binary(left, *op, right),
            Expr::Table(fields) => self.eval_table(fields),
            Expr::Freeze(inner) => self.eval_expr(inner)?.with_frozen(true),
            Expr::Name(name) => self.lookup_name(name),
            Expr::Chain { base, segments } => self.eval_chain(base, segments),
            Expr::TypeAssertion { expr, .. } => self.eval_expr(expr),
            Expr::Function(_)
            | Expr::Pattern(_)
            | Expr::VarArg
            | Expr::Yield(_)
            | Expr::IfElse { .. }
            | Expr::DoExpr { .. }
            | Expr::SwitchExpr { .. }
            | Expr::Ternary { .. }
            | Expr::Pipe { .. }
            | Expr::Comprehension(_)
            | Expr::SignalHandler(_) => Err(CompilerError::Other(
                "That expression is not supported at compile time yet.".to_string(),
            )),
        }
    }

    pub fn call_function(
        &mut self,
        function: &CtFunctionHandle,
        args: Vec<CtValue>,
    ) -> Result<CtValue> {
        let mut nested = Evaluator::new(function.env.clone(), function.options.clone());
        nested.env.push_scope();
        if let Some(name) = &function.name {
            nested.env.define_function(name, Arc::clone(function));
        }

        let expected = function
            .params
            .iter()
            .filter(|param| matches!(param, Param::Binding(_)))
            .count();
        if function
            .params
            .iter()
            .any(|param| matches!(param, Param::VarArg(_)))
        {
            return Err(CompilerError::Other(
                "Varargs are not supported in comptime functions yet.".to_string(),
            ));
        }
        if args.len() != expected {
            return Err(CompilerError::Other(format!(
                "Compile-time function expected {expected} argument(s), got {}.",
                args.len()
            )));
        }

        for (param, value) in function.params.iter().zip(args) {
            let Param::Binding(binding) = param else {
                unreachable!("varargs handled above")
            };
            nested.bind_pattern(&binding.pattern, value, false, None)?;
        }

        let result = match nested.exec_block(&function.body)? {
            Flow::Continue => CtValue::Nil,
            Flow::Return(values) => values.into_iter().next().unwrap_or(CtValue::Nil),
        };
        nested.env.pop_scope();
        Ok(result)
    }

    fn exec_block(&mut self, block: &Block) -> Result<Flow> {
        for stmt in block {
            match self.exec_stmt(stmt)? {
                Flow::Continue => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Continue)
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Flow> {
        match stmt {
            Stmt::Local(local) => {
                self.exec_local(local)?;
                Ok(Flow::Continue)
            }
            Stmt::Assignment(assignment) => {
                self.exec_assignment(assignment)?;
                Ok(Flow::Continue)
            }
            Stmt::If(if_stmt) => self.exec_if(if_stmt),
            Stmt::ForNumeric(for_numeric) => self.exec_for_numeric(for_numeric),
            Stmt::ForGeneric(for_generic) => self.exec_for_generic(for_generic),
            Stmt::Return(values, _) => Ok(Flow::Return(
                values
                    .iter()
                    .map(|value| self.eval_expr(value))
                    .collect::<Result<Vec<_>>>()?,
            )),
            Stmt::Call(expr, _) => {
                self.eval_expr(expr)?;
                Ok(Flow::Continue)
            }
            Stmt::Do(block, _) => {
                self.env.push_scope();
                let flow = self.exec_block(block)?;
                self.env.pop_scope();
                Ok(flow)
            }
            Stmt::Switch(_)
            | Stmt::Match(_)
            | Stmt::While(_)
            | Stmt::Repeat(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Fallthrough(_)
            | Stmt::CompoundAssignment { .. }
            | Stmt::NullishAssignment { .. }
            | Stmt::State(_)
            | Stmt::Function(_)
            | Stmt::Object(_)
            | Stmt::Enum(_)
            | Stmt::Signal(_)
            | Stmt::Fire(_)
            | Stmt::SignalHandler(_)
            | Stmt::Watch(_)
            | Stmt::Spawn(_)
            | Stmt::TypeAlias { .. }
            | Stmt::ComptimeIf(_)
            | Stmt::ComptimeSwitch(_) => Err(CompilerError::Other(
                "That statement is not supported inside comptime functions yet.".to_string(),
            )),
        }
    }

    fn exec_local(&mut self, local: &LocalDecl) -> Result<()> {
        let mut values = local
            .values
            .iter()
            .map(|value| self.eval_expr(value))
            .collect::<Result<Vec<_>>>()?;
        while values.len() < local.bindings.len() {
            values.push(CtValue::Nil);
        }

        for (index, binding) in local.bindings.iter().enumerate() {
            let value = values.get(index).cloned().unwrap_or(CtValue::Nil);
            self.bind_pattern(
                &binding.pattern,
                value,
                local.is_const || local.is_comptime,
                None,
            )?;
        }
        Ok(())
    }

    fn exec_assignment(&mut self, assignment: &Assignment) -> Result<()> {
        let values = assignment
            .values
            .iter()
            .map(|value| self.eval_expr(value))
            .collect::<Result<Vec<_>>>()?;
        for (index, target) in assignment.targets.iter().enumerate() {
            let value = values.get(index).cloned().unwrap_or(CtValue::Nil);
            self.assign_target(target, value)?;
        }
        Ok(())
    }

    fn exec_if(&mut self, if_stmt: &IfStmt) -> Result<Flow> {
        for (condition, block) in &if_stmt.branches {
            if self.eval_expr(condition)?.truthy() {
                self.env.push_scope();
                let flow = self.exec_block(block)?;
                self.env.pop_scope();
                return Ok(flow);
            }
        }
        if let Some(block) = &if_stmt.else_block {
            self.env.push_scope();
            let flow = self.exec_block(block)?;
            self.env.pop_scope();
            return Ok(flow);
        }
        Ok(Flow::Continue)
    }

    fn exec_for_numeric(&mut self, for_numeric: &ForNumeric) -> Result<Flow> {
        let start = expect_number(&self.eval_expr(&for_numeric.start)?, "for start")?;
        let end = expect_number(&self.eval_expr(&for_numeric.end)?, "for end")?;
        let step = if let Some(step) = &for_numeric.step {
            expect_number(&self.eval_expr(step)?, "for step")?
        } else {
            1.0
        };
        if step == 0.0 {
            return Err(CompilerError::Other(
                "Compile-time numeric for loops cannot use a zero step.".to_string(),
            ));
        }

        let mut current = start;
        while if step > 0.0 {
            current <= end
        } else {
            current >= end
        } {
            self.env.push_scope();
            self.env
                .define_value(&for_numeric.name, CtValue::Number(current), false);
            match self.exec_block(&for_numeric.block)? {
                Flow::Continue => {}
                flow => {
                    self.env.pop_scope();
                    return Ok(flow);
                }
            }
            self.env.pop_scope();
            current += step;
        }
        Ok(Flow::Continue)
    }

    fn exec_for_generic(&mut self, for_generic: &ForGeneric) -> Result<Flow> {
        if for_generic.iterables.len() != 1 {
            return Err(CompilerError::Other(
                "Compile-time generic for loops currently support exactly one iterable."
                    .to_string(),
            ));
        }
        let iterable = self.eval_expr(&for_generic.iterables[0])?;
        let items = match iterable {
            CtValue::Array(array) => array
                .items
                .into_iter()
                .enumerate()
                .map(|(index, value)| vec![CtValue::Number((index + 1) as f64), value])
                .collect::<Vec<_>>(),
            CtValue::Table(table) => table
                .entries
                .into_iter()
                .map(|(key, value)| vec![CtValue::String(key), value])
                .collect::<Vec<_>>(),
            other => {
                return Err(CompilerError::Other(format!(
                    "Compile-time for loops expect an array or table, got {}.",
                    other.type_name()
                )));
            }
        };

        for values in items {
            self.env.push_scope();
            for (index, binding) in for_generic.bindings.iter().enumerate() {
                self.bind_pattern(
                    &binding.pattern,
                    values.get(index).cloned().unwrap_or(CtValue::Nil),
                    false,
                    None,
                )?;
            }
            match self.exec_block(&for_generic.block)? {
                Flow::Continue => {}
                flow => {
                    self.env.pop_scope();
                    return Ok(flow);
                }
            }
            self.env.pop_scope();
        }
        Ok(Flow::Continue)
    }

    fn eval_unary(&mut self, op: UnaryOp, expr: &Expr) -> Result<CtValue> {
        let value = self.eval_expr(expr)?;
        match op {
            UnaryOp::Negate => Ok(CtValue::Number(-expect_number(&value, "unary -")?)),
            UnaryOp::Not => Ok(CtValue::Bool(!value.truthy())),
            UnaryOp::Length => match value {
                CtValue::String(value) => Ok(CtValue::Number(value.chars().count() as f64)),
                CtValue::Array(array) => Ok(CtValue::Number(array.items.len() as f64)),
                CtValue::Table(table) => Ok(CtValue::Number(table.entries.len() as f64)),
                other => Err(CompilerError::Other(format!(
                    "Cannot take the length of {} at compile time.",
                    other.type_name()
                ))),
            },
        }
    }

    fn eval_binary(&mut self, left: &Expr, op: BinaryOp, right: &Expr) -> Result<CtValue> {
        match op {
            BinaryOp::And => {
                let left = self.eval_expr(left)?;
                if left.truthy() {
                    self.eval_expr(right)
                } else {
                    Ok(left)
                }
            }
            BinaryOp::Or => {
                let left = self.eval_expr(left)?;
                if left.truthy() {
                    Ok(left)
                } else {
                    self.eval_expr(right)
                }
            }
            _ => {
                let left = self.eval_expr(left)?;
                let right = self.eval_expr(right)?;
                match op {
                    BinaryOp::Add => Ok(CtValue::Number(
                        expect_number(&left, "+")? + expect_number(&right, "+")?,
                    )),
                    BinaryOp::Subtract => Ok(CtValue::Number(
                        expect_number(&left, "-")? - expect_number(&right, "-")?,
                    )),
                    BinaryOp::Multiply => Ok(CtValue::Number(
                        expect_number(&left, "*")? * expect_number(&right, "*")?,
                    )),
                    BinaryOp::Divide => Ok(CtValue::Number(
                        expect_number(&left, "/")? / expect_number(&right, "/")?,
                    )),
                    BinaryOp::Modulo => Ok(CtValue::Number(
                        expect_number(&left, "%")? % expect_number(&right, "%")?,
                    )),
                    BinaryOp::Power => Ok(CtValue::Number(
                        expect_number(&left, "^")?.powf(expect_number(&right, "^")?),
                    )),
                    BinaryOp::Concat => Ok(CtValue::String(format!(
                        "{}{}",
                        expect_stringish(&left, "..")?,
                        expect_stringish(&right, "..")?
                    ))),
                    BinaryOp::Equal => Ok(CtValue::Bool(left == right)),
                    BinaryOp::NotEqual => Ok(CtValue::Bool(left != right)),
                    BinaryOp::Less => compare_values(left, right, "<"),
                    BinaryOp::LessEqual => compare_values(left, right, "<="),
                    BinaryOp::Greater => compare_values(left, right, ">"),
                    BinaryOp::GreaterEqual => compare_values(left, right, ">="),
                    BinaryOp::FloorDivide | BinaryOp::Nullish => Err(CompilerError::Other(
                        "That operator is not supported at compile time yet.".to_string(),
                    )),
                    BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),
                }
            }
        }
    }

    fn eval_table(&mut self, fields: &[TableField]) -> Result<CtValue> {
        if fields.is_empty() {
            return Ok(CtValue::Table(CtTable {
                entries: Vec::new(),
                frozen: false,
            }));
        }

        let array_like = fields
            .iter()
            .all(|field| matches!(field, TableField::Value(_)));
        if array_like {
            return Ok(CtValue::Array(CtArray {
                items: fields
                    .iter()
                    .map(|field| match field {
                        TableField::Value(expr) => self.eval_expr(expr),
                        _ => unreachable!("array_like checked above"),
                    })
                    .collect::<Result<Vec<_>>>()?,
                frozen: false,
            }));
        }

        let dict_like = fields
            .iter()
            .all(|field| matches!(field, TableField::Named(_, _) | TableField::Indexed(_, _)));
        if !dict_like {
            return Err(CompilerError::Other(
                "Mixed table literals are not supported at compile time yet.".to_string(),
            ));
        }

        let mut entries = Vec::new();
        for field in fields {
            match field {
                TableField::Named(name, value) => {
                    entries.push((name.clone(), self.eval_expr(value)?))
                }
                TableField::Indexed(key, value) => {
                    let key = self.eval_expr(key)?;
                    let CtValue::String(key) = key else {
                        return Err(CompilerError::Other(
                            "Compile-time dictionary tables require string keys.".to_string(),
                        ));
                    };
                    entries.push((key, self.eval_expr(value)?));
                }
                TableField::Value(_) => unreachable!("dict_like checked above"),
            }
        }

        Ok(CtValue::Table(CtTable {
            entries,
            frozen: false,
        }))
    }

    fn eval_chain(&mut self, base: &Expr, segments: &[ChainSegment]) -> Result<CtValue> {
        if let Expr::Name(name) = base {
            if matches!(segments.first(), Some(ChainSegment::Call { .. }))
                && self.env.lookup_value(name).is_none()
                && self.env.lookup_function(name).is_none()
                && is_builtin(name)
            {
                let ChainSegment::Call { args, .. } = &segments[0] else {
                    unreachable!("matches! checked the shape")
                };
                let args = args
                    .iter()
                    .map(|arg| self.eval_expr(arg))
                    .collect::<Result<Vec<_>>>()?;
                let mut current = call_builtin(name, args, &self.options)?;
                for segment in &segments[1..] {
                    current = self.eval_chain_segment(current, segment)?;
                }
                return Ok(current);
            }

            if self.env.lookup_value(name).is_none() && self.env.lookup_function(name).is_none() {
                if let Some(path) = unavailable_call_path(base, segments) {
                    if path == "Instance.new" {
                        return Err(CompilerError::Other(
                            "Roblox Instances cannot be created at compile time.".to_string(),
                        ));
                    }
                    return Err(CompilerError::Other(format!(
                        "Function '{path}' is not available at compile time."
                    )));
                }
            }
        }

        let mut current = self.eval_expr(base)?;
        for segment in segments {
            current = self.eval_chain_segment(current, segment)?;
        }
        Ok(current)
    }

    fn eval_chain_segment(&mut self, current: CtValue, segment: &ChainSegment) -> Result<CtValue> {
        match segment {
            ChainSegment::Field { name, .. } => match current {
                CtValue::Table(table) => table_get(&table, name).ok_or_else(|| {
                    CompilerError::Other(format!(
                        "Compile-time table does not contain field `{name}`."
                    ))
                }),
                other => Err(CompilerError::Other(format!(
                    "Cannot access field `{name}` on {} at compile time.",
                    other.type_name()
                ))),
            },
            ChainSegment::Index { expr, .. } => {
                let index = self.eval_expr(expr)?;
                self.index_value(current, index)
            }
            ChainSegment::Call { args, .. } => {
                let args = args
                    .iter()
                    .map(|arg| self.eval_expr(arg))
                    .collect::<Result<Vec<_>>>()?;
                match current {
                    CtValue::Function(function) => self.call_function(&function, args),
                    other => Err(CompilerError::Other(format!(
                        "Cannot call {} at compile time.",
                        other.type_name()
                    ))),
                }
            }
            ChainSegment::MethodCall { name, args, .. } => {
                let args = args
                    .iter()
                    .map(|arg| self.eval_expr(arg))
                    .collect::<Result<Vec<_>>>()?;
                call_method(current, name, args, &self.options)
            }
        }
    }

    fn index_value(&self, current: CtValue, index: CtValue) -> Result<CtValue> {
        match (current, index) {
            (CtValue::Array(array), CtValue::Number(index)) => {
                let index = as_array_index(index)?;
                Ok(array.items.get(index).cloned().unwrap_or(CtValue::Nil))
            }
            (CtValue::Table(table), CtValue::String(key)) => {
                Ok(table_get(&table, &key).unwrap_or(CtValue::Nil))
            }
            (CtValue::Table(_), other) => Err(CompilerError::Other(format!(
                "Compile-time dictionary tables require string indices, got {}.",
                other.type_name()
            ))),
            (other, _) => Err(CompilerError::Other(format!(
                "Cannot index {} at compile time.",
                other.type_name()
            ))),
        }
    }

    fn lookup_name(&self, name: &str) -> Result<CtValue> {
        if let Some(value) = self.env.lookup_value(name) {
            return Ok(value);
        }
        if let Some(function) = self.env.lookup_function(name) {
            return Ok(CtValue::Function(function));
        }
        if self.env.is_runtime_name(name) {
            return Err(CompilerError::Other(format!(
                "Cannot use runtime local '{name}' in a compile-time expression."
            )));
        }
        Err(CompilerError::Other(format!(
            "Unknown compile-time name `{name}`."
        )))
    }

    fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        value: CtValue,
        is_const: bool,
        default_value: Option<&Expr>,
    ) -> Result<()> {
        let value = if matches!(value, CtValue::Nil) {
            if let Some(default_value) = default_value {
                self.eval_expr(default_value)?
            } else {
                value
            }
        } else {
            value
        };

        match pattern {
            Pattern::Name(name) => {
                self.env.define_value(name, value, is_const);
                Ok(())
            }
            Pattern::Array { items, rest } => {
                let CtValue::Array(array) = value else {
                    return Err(CompilerError::Other(
                        "Array destructuring requires a compile-time array value.".to_string(),
                    ));
                };
                for (index, item) in items.iter().enumerate() {
                    if let Some(binding) = &item.binding {
                        let value = array.items.get(index).cloned().unwrap_or(CtValue::Nil);
                        self.bind_pattern(
                            &binding.target,
                            value,
                            is_const,
                            binding.default_value.as_ref(),
                        )?;
                    }
                }
                if let Some(rest) = rest {
                    self.env.define_value(
                        rest,
                        CtValue::Array(CtArray {
                            items: array.items.into_iter().skip(items.len()).collect(),
                            frozen: false,
                        }),
                        is_const,
                    );
                }
                Ok(())
            }
            Pattern::Table { entries, rest } => {
                let CtValue::Table(table) = value else {
                    return Err(CompilerError::Other(
                        "Table destructuring requires a compile-time table value.".to_string(),
                    ));
                };
                for entry in entries {
                    let value = table_get(&table, &entry.key).unwrap_or(CtValue::Nil);
                    self.bind_pattern(
                        &entry.binding.target,
                        value,
                        is_const,
                        entry.binding.default_value.as_ref(),
                    )?;
                }
                if let Some(rest) = rest {
                    let remaining = table
                        .entries
                        .into_iter()
                        .filter(|(key, _)| !entries.iter().any(|entry| entry.key == *key))
                        .collect::<Vec<_>>();
                    self.env.define_value(
                        rest,
                        CtValue::Table(CtTable {
                            entries: remaining,
                            frozen: false,
                        }),
                        is_const,
                    );
                }
                Ok(())
            }
        }
    }

    fn assign_target(&mut self, target: &AssignTarget, value: CtValue) -> Result<()> {
        match target {
            AssignTarget::Name(name) => self.env.assign_value(name, value),
            AssignTarget::Field { object, field } => {
                self.assign_path(object, vec![CtPath::Field(field.clone())], value)
            }
            AssignTarget::Index { object, index } => {
                let index_value = self.eval_expr(index)?;
                self.assign_path(object, vec![CtPath::Index(index_value)], value)
            }
        }
    }

    fn assign_path(&mut self, object: &Expr, mut path: Vec<CtPath>, value: CtValue) -> Result<()> {
        let (root, mut prefix) = self.extract_path(object)?;
        prefix.append(&mut path);
        let mut root_value = self.lookup_name(&root)?;
        set_path_value(&mut root_value, &prefix, value)?;
        self.env.assign_value(&root, root_value)
    }

    fn extract_path(&mut self, expr: &Expr) -> Result<(String, Vec<CtPath>)> {
        match expr {
            Expr::Name(name) => Ok((name.clone(), Vec::new())),
            Expr::Chain { base, segments } => {
                let (root, mut path) = self.extract_path(base)?;
                for segment in segments {
                    match segment {
                        ChainSegment::Field { name, .. } => path.push(CtPath::Field(name.clone())),
                        ChainSegment::Index { expr, .. } => {
                            path.push(CtPath::Index(self.eval_expr(expr)?))
                        }
                        ChainSegment::Call { .. } | ChainSegment::MethodCall { .. } => {
                            return Err(CompilerError::Other(
                                "Compile-time assignments cannot target call expressions."
                                    .to_string(),
                            ));
                        }
                    }
                }
                Ok((root, path))
            }
            _ => Err(CompilerError::Other(
                "Unsupported compile-time assignment target.".to_string(),
            )),
        }
    }
}

enum CtPath {
    Field(String),
    Index(CtValue),
}

fn set_path_value(target: &mut CtValue, path: &[CtPath], value: CtValue) -> Result<()> {
    if path.is_empty() {
        *target = value;
        return Ok(());
    }

    match (&path[0], target) {
        (_, CtValue::Array(array)) if array.frozen => Err(CompilerError::Other(
            "Cannot mutate a frozen compile-time array.".to_string(),
        )),
        (_, CtValue::Table(table)) if table.frozen => Err(CompilerError::Other(
            "Cannot mutate a frozen compile-time table.".to_string(),
        )),
        (CtPath::Field(key), CtValue::Table(table)) => {
            if path.len() == 1 {
                table_insert(table, key.clone(), value);
                return Ok(());
            }
            let entry = if let Some((_, entry)) =
                table.entries.iter_mut().find(|(entry, _)| *entry == *key)
            {
                entry
            } else {
                table.entries.push((
                    key.clone(),
                    CtValue::Table(CtTable {
                        entries: Vec::new(),
                        frozen: false,
                    }),
                ));
                &mut table.entries.last_mut().expect("just pushed").1
            };
            set_path_value(entry, &path[1..], value)
        }
        (CtPath::Index(index), CtValue::Array(array)) => {
            let CtValue::Number(index) = index else {
                return Err(CompilerError::Other(
                    "Compile-time arrays require numeric indices.".to_string(),
                ));
            };
            let index = as_array_index(*index)?;
            while array.items.len() <= index {
                array.items.push(CtValue::Nil);
            }
            if path.len() == 1 {
                array.items[index] = value;
                return Ok(());
            }
            set_path_value(&mut array.items[index], &path[1..], value)
        }
        (CtPath::Index(index), CtValue::Table(table)) => {
            let CtValue::String(key) = index else {
                return Err(CompilerError::Other(
                    "Compile-time dictionary tables require string indices.".to_string(),
                ));
            };
            if path.len() == 1 {
                table_insert(table, key.clone(), value);
                return Ok(());
            }
            let entry = if let Some((_, entry)) =
                table.entries.iter_mut().find(|(entry, _)| *entry == *key)
            {
                entry
            } else {
                table.entries.push((
                    key.clone(),
                    CtValue::Table(CtTable {
                        entries: Vec::new(),
                        frozen: false,
                    }),
                ));
                &mut table.entries.last_mut().expect("just pushed").1
            };
            set_path_value(entry, &path[1..], value)
        }
        (CtPath::Field(name), other) => Err(CompilerError::Other(format!(
            "Cannot assign field `{name}` on {} at compile time.",
            other.type_name()
        ))),
        (CtPath::Index(_), other) => Err(CompilerError::Other(format!(
            "Cannot index-assign {} at compile time.",
            other.type_name()
        ))),
    }
}

fn unavailable_call_path(base: &Expr, segments: &[ChainSegment]) -> Option<String> {
    let Expr::Name(root) = base else {
        return None;
    };

    let mut path = root.clone();
    let mut saw_call = false;
    for segment in segments {
        match segment {
            ChainSegment::Field { name, .. } => {
                path.push('.');
                path.push_str(name);
            }
            ChainSegment::MethodCall { name, .. } => {
                path.push(':');
                path.push_str(name);
                saw_call = true;
                break;
            }
            ChainSegment::Call { .. } => {
                saw_call = true;
                break;
            }
            ChainSegment::Index { .. } => return None,
        }
    }

    saw_call.then_some(path)
}

fn expect_number(value: &CtValue, context: &str) -> Result<f64> {
    match value {
        CtValue::Number(number) => Ok(*number),
        other => Err(CompilerError::Other(format!(
            "{context} expects numbers, got {}.",
            other.type_name()
        ))),
    }
}

fn expect_stringish(value: &CtValue, context: &str) -> Result<String> {
    match value {
        CtValue::String(text) => Ok(text.clone()),
        CtValue::Number(number) => Ok(number.to_string()),
        CtValue::Bool(boolean) => Ok(boolean.to_string()),
        CtValue::Nil => Ok("nil".to_string()),
        other => Err(CompilerError::Other(format!(
            "{context} expects string-like values, got {}.",
            other.type_name()
        ))),
    }
}

fn compare_values(left: CtValue, right: CtValue, op: &str) -> Result<CtValue> {
    let result = match (&left, &right) {
        (CtValue::Number(left), CtValue::Number(right)) => match op {
            "<" => left < right,
            "<=" => left <= right,
            ">" => left > right,
            ">=" => left >= right,
            _ => unreachable!("operator handled by caller"),
        },
        (CtValue::String(left), CtValue::String(right)) => match op {
            "<" => left < right,
            "<=" => left <= right,
            ">" => left > right,
            ">=" => left >= right,
            _ => unreachable!("operator handled by caller"),
        },
        _ => {
            return Err(CompilerError::Other(format!(
                "Cannot compare {} and {} with `{op}` at compile time.",
                left.type_name(),
                right.type_name()
            )));
        }
    };
    Ok(CtValue::Bool(result))
}
