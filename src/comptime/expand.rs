use std::sync::Arc;

use crate::{
    ast::*,
    compiler::{CompilerError, Result},
};

use super::{
    ComptimeOptions,
    env::CtEnv,
    eval::Evaluator,
    value::{CtFunction, CtValue},
};

pub fn expand_program(program: &Program, options: ComptimeOptions) -> Result<Program> {
    let mut expander = Expander::new(options);
    Ok(Program {
        block: expander.expand_inline_block(&program.block)?,
    })
}

struct Expander {
    env: CtEnv,
    options: ComptimeOptions,
}

impl Expander {
    fn new(options: ComptimeOptions) -> Self {
        Self {
            env: CtEnv::default(),
            options,
        }
    }
}

impl Expander {
    fn expand_inline_block(&mut self, block: &Block) -> Result<Block> {
        let mut expanded = Vec::new();
        for stmt in block {
            expanded.extend(self.expand_stmt(stmt)?);
        }
        Ok(expanded)
    }

    fn expand_scoped_block(&mut self, block: &Block) -> Result<Block> {
        self.env.push_scope();
        let result = self.expand_inline_block(block);
        self.env.pop_scope();
        result
    }

    fn expand_stmt(&mut self, stmt: &Stmt) -> Result<Vec<Stmt>> {
        match stmt {
            Stmt::Local(local) if local.is_comptime => {
                self.define_comptime_local(local)?;
                Ok(Vec::new())
            }
            Stmt::Local(local) => {
                let local = LocalDecl {
                    span: local.span,
                    is_const: local.is_const,
                    is_comptime: false,
                    bindings: local.bindings.clone(),
                    values: local
                        .values
                        .iter()
                        .map(|value| self.expand_expr(value))
                        .collect::<Result<Vec<_>>>()?,
                };
                self.define_runtime_pattern_names(&local.bindings);
                Ok(vec![Stmt::Local(local)])
            }
            Stmt::Function(function) if function.is_comptime => {
                self.define_comptime_function(function)?;
                Ok(Vec::new())
            }
            Stmt::Function(function) => {
                let body = self.expand_function_body(
                    function.local_name.then_some(function.name.root.as_str()),
                    &function.params,
                    &function.body,
                )?;
                let function = FunctionDecl {
                    span: function.span,
                    local_name: function.local_name,
                    is_task: function.is_task,
                    is_comptime: false,
                    name: function.name.clone(),
                    generics: function.generics.clone(),
                    params: function.params.clone(),
                    return_type: function.return_type.clone(),
                    body,
                };
                self.define_runtime_function_name(&function);
                Ok(vec![Stmt::Function(function)])
            }
            Stmt::ComptimeIf(if_stmt) => self.expand_comptime_if(if_stmt),
            Stmt::ComptimeSwitch(switch_stmt) => self.expand_comptime_switch(switch_stmt),
            Stmt::If(if_stmt) => Ok(vec![Stmt::If(IfStmt {
                span: if_stmt.span,
                branches: if_stmt
                    .branches
                    .iter()
                    .map(|(condition, block)| {
                        Ok((
                            self.expand_expr(condition)?,
                            self.expand_scoped_block(block)?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?,
                else_block: if_stmt
                    .else_block
                    .as_ref()
                    .map(|block| self.expand_scoped_block(block))
                    .transpose()?,
            })]),
            Stmt::Switch(switch_stmt) => Ok(vec![Stmt::Switch(SwitchStmt {
                span: switch_stmt.span,
                value: self.expand_expr(&switch_stmt.value)?,
                cases: switch_stmt
                    .cases
                    .iter()
                    .map(|case| {
                        Ok(SwitchCase {
                            value: self.expand_expr(&case.value)?,
                            block: self.expand_scoped_block(&case.block)?,
                            fallthrough: case.fallthrough,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
                default: switch_stmt
                    .default
                    .as_ref()
                    .map(|block| self.expand_scoped_block(block))
                    .transpose()?,
            })]),
            Stmt::Match(match_stmt) => Ok(vec![Stmt::Match(MatchStmt {
                span: match_stmt.span,
                value: self.expand_expr(&match_stmt.value)?,
                cases: match_stmt
                    .cases
                    .iter()
                    .map(|case| {
                        self.env.push_scope();
                        self.define_match_pattern_names(&case.pattern);
                        let guard = case
                            .guard
                            .as_ref()
                            .map(|guard| self.expand_expr(guard))
                            .transpose();
                        let block = self.expand_inline_block(&case.block);
                        self.env.pop_scope();
                        Ok(MatchCase {
                            pattern: case.pattern.clone(),
                            guard: guard?,
                            block: block?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            })]),
            Stmt::While(while_stmt) => Ok(vec![Stmt::While(WhileStmt {
                span: while_stmt.span,
                condition: self.expand_expr(&while_stmt.condition)?,
                block: self.expand_scoped_block(&while_stmt.block)?,
            })]),
            Stmt::Repeat(repeat_stmt) => Ok(vec![Stmt::Repeat(RepeatStmt {
                span: repeat_stmt.span,
                block: self.expand_scoped_block(&repeat_stmt.block)?,
                condition: self.expand_expr(&repeat_stmt.condition)?,
            })]),
            Stmt::ForNumeric(for_numeric) => {
                self.env.push_scope();
                self.env.define_runtime_name(&for_numeric.name);
                let block = self.expand_inline_block(&for_numeric.block);
                self.env.pop_scope();
                Ok(vec![Stmt::ForNumeric(ForNumeric {
                    span: for_numeric.span,
                    name: for_numeric.name.clone(),
                    start: self.expand_expr(&for_numeric.start)?,
                    end: self.expand_expr(&for_numeric.end)?,
                    step: for_numeric
                        .step
                        .as_ref()
                        .map(|step| self.expand_expr(step))
                        .transpose()?,
                    block: block?,
                })])
            }
            Stmt::ForGeneric(for_generic) => {
                let iterables = for_generic
                    .iterables
                    .iter()
                    .map(|iterable| self.expand_expr(iterable))
                    .collect::<Result<Vec<_>>>()?;
                self.env.push_scope();
                self.define_runtime_pattern_names(&for_generic.bindings);
                let block = self.expand_inline_block(&for_generic.block);
                self.env.pop_scope();
                Ok(vec![Stmt::ForGeneric(ForGeneric {
                    span: for_generic.span,
                    bindings: for_generic.bindings.clone(),
                    iterables,
                    block: block?,
                })])
            }
            Stmt::Do(block, span) => Ok(vec![Stmt::Do(self.expand_scoped_block(block)?, *span)]),
            Stmt::Assignment(assignment) => Ok(vec![Stmt::Assignment(Assignment {
                span: assignment.span,
                targets: assignment.targets.clone(),
                values: assignment
                    .values
                    .iter()
                    .map(|value| self.expand_expr(value))
                    .collect::<Result<Vec<_>>>()?,
            })]),
            Stmt::CompoundAssignment {
                span,
                target,
                op,
                value,
            } => Ok(vec![Stmt::CompoundAssignment {
                span: *span,
                target: target.clone(),
                op: *op,
                value: self.expand_expr(value)?,
            }]),
            Stmt::NullishAssignment {
                span,
                target,
                value,
            } => Ok(vec![Stmt::NullishAssignment {
                span: *span,
                target: target.clone(),
                value: self.expand_expr(value)?,
            }]),
            Stmt::Call(expr, span) => Ok(vec![Stmt::Call(self.expand_expr(expr)?, *span)]),
            Stmt::Return(values, span) => Ok(vec![Stmt::Return(
                values
                    .iter()
                    .map(|value| self.expand_expr(value))
                    .collect::<Result<Vec<_>>>()?,
                *span,
            )]),
            Stmt::State(state) => {
                let state = StateDecl {
                    span: state.span,
                    binding: state.binding.clone(),
                    value: state
                        .value
                        .as_ref()
                        .map(|value| self.expand_expr(value))
                        .transpose()?,
                };
                if let Pattern::Name(name) = &state.binding.pattern {
                    self.env.define_runtime_name(name);
                }
                Ok(vec![Stmt::State(state)])
            }
            Stmt::Object(object) => {
                let methods = object
                    .methods
                    .iter()
                    .map(|method| self.expand_object_method(object, method))
                    .collect::<Result<Vec<_>>>()?;
                self.env.define_runtime_name(&object.name);
                Ok(vec![Stmt::Object(ObjectDecl {
                    span: object.span,
                    name: object.name.clone(),
                    extends: object.extends.clone(),
                    fields: object.fields.clone(),
                    methods,
                })])
            }
            Stmt::Enum(decl) => {
                self.env.define_runtime_name(&decl.name);
                Ok(vec![Stmt::Enum(EnumDecl {
                    span: decl.span,
                    name: decl.name.clone(),
                    base_type: decl.base_type.clone(),
                    members: decl
                        .members
                        .iter()
                        .map(|member| {
                            Ok(EnumMember {
                                name: member.name.clone(),
                                value: member
                                    .value
                                    .as_ref()
                                    .map(|value| self.expand_expr(value))
                                    .transpose()?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?,
                })])
            }
            Stmt::Signal(signal) => {
                self.env.define_runtime_name(&signal.name);
                Ok(vec![stmt.clone()])
            }
            Stmt::Fire(fire) => Ok(vec![Stmt::Fire(FireStmt {
                span: fire.span,
                signal: self.expand_expr(&fire.signal)?,
                args: fire
                    .args
                    .iter()
                    .map(|arg| self.expand_expr(arg))
                    .collect::<Result<Vec<_>>>()?,
            })]),
            Stmt::SignalHandler(handler) => {
                self.env.push_scope();
                for param in &handler.params {
                    self.env.define_runtime_name(param);
                }
                let body = self.expand_inline_block(&handler.body);
                self.env.pop_scope();
                Ok(vec![Stmt::SignalHandler(SignalHandlerStmt {
                    span: handler.span,
                    signal: self.expand_expr(&handler.signal)?,
                    params: handler.params.clone(),
                    body: body?,
                    once: handler.once,
                })])
            }
            Stmt::Watch(watch) => {
                self.env.push_scope();
                for param in &watch.params {
                    self.env.define_runtime_name(param);
                }
                let body = self.expand_inline_block(&watch.body);
                self.env.pop_scope();
                Ok(vec![Stmt::Watch(WatchStmt {
                    span: watch.span,
                    name: watch.name.clone(),
                    params: watch.params.clone(),
                    body: body?,
                })])
            }
            Stmt::Spawn(spawn) => Ok(vec![Stmt::Spawn(SpawnStmt {
                span: spawn.span,
                call: self.expand_expr(&spawn.call)?,
                then_handler: spawn
                    .then_handler
                    .as_ref()
                    .map(|handler| self.expand_spawn_handler(handler))
                    .transpose()?,
                catch_handler: spawn
                    .catch_handler
                    .as_ref()
                    .map(|handler| self.expand_spawn_handler(handler))
                    .transpose()?,
            })]),
            Stmt::TypeAlias { .. } | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Fallthrough(_) => {
                Ok(vec![stmt.clone()])
            }
        }
    }

    fn expand_expr(&mut self, expr: &Expr) -> Result<Expr> {
        match expr {
            Expr::Comptime(inner) => self.evaluate_comptime_expr(inner)?.to_expr(),
            Expr::Freeze(inner) => Ok(Expr::Freeze(Box::new(self.expand_expr(inner)?))),
            Expr::Yield(inner) => Ok(Expr::Yield(Box::new(self.expand_expr(inner)?))),
            Expr::Paren(inner) => Ok(Expr::Paren(Box::new(self.expand_expr(inner)?))),
            Expr::Unary { op, expr } => Ok(Expr::Unary {
                op: *op,
                expr: Box::new(self.expand_expr(expr)?),
            }),
            Expr::TypeAssertion { expr, annotation } => Ok(Expr::TypeAssertion {
                expr: Box::new(self.expand_expr(expr)?),
                annotation: annotation.clone(),
            }),
            Expr::Binary { left, op, right } => Ok(Expr::Binary {
                left: Box::new(self.expand_expr(left)?),
                op: *op,
                right: Box::new(self.expand_expr(right)?),
            }),
            Expr::Ternary {
                condition,
                then_expr,
                else_expr,
            } => Ok(Expr::Ternary {
                condition: Box::new(self.expand_expr(condition)?),
                then_expr: Box::new(self.expand_expr(then_expr)?),
                else_expr: Box::new(self.expand_expr(else_expr)?),
            }),
            Expr::IfElse {
                branches,
                else_expr,
            } => Ok(Expr::IfElse {
                branches: branches
                    .iter()
                    .map(|(condition, value)| {
                        Ok((self.expand_expr(condition)?, self.expand_expr(value)?))
                    })
                    .collect::<Result<Vec<_>>>()?,
                else_expr: Box::new(self.expand_expr(else_expr)?),
            }),
            Expr::DoExpr { block, result } => {
                self.env.push_scope();
                let block = self.expand_inline_block(block);
                let result_expr = self.expand_expr(result);
                self.env.pop_scope();
                Ok(Expr::DoExpr {
                    block: block?,
                    result: Box::new(result_expr?),
                })
            }
            Expr::SwitchExpr {
                value,
                cases,
                default,
            } => Ok(Expr::SwitchExpr {
                value: Box::new(self.expand_expr(value)?),
                cases: cases
                    .iter()
                    .map(|case| {
                        Ok(SwitchExprCase {
                            value: self.expand_expr(&case.value)?,
                            result: self.expand_expr(&case.result)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
                default: Box::new(self.expand_expr(default)?),
            }),
            Expr::Table(fields) => Ok(Expr::Table(
                fields
                    .iter()
                    .map(|field| match field {
                        TableField::Named(name, value) => {
                            Ok(TableField::Named(name.clone(), self.expand_expr(value)?))
                        }
                        TableField::Indexed(key, value) => Ok(TableField::Indexed(
                            self.expand_expr(key)?,
                            self.expand_expr(value)?,
                        )),
                        TableField::Value(value) => Ok(TableField::Value(self.expand_expr(value)?)),
                    })
                    .collect::<Result<Vec<_>>>()?,
            )),
            Expr::Function(function) => {
                self.env.push_scope();
                for param in &function.params {
                    if let Param::Binding(binding) = param {
                        self.define_runtime_pattern_name(&binding.pattern);
                    }
                }
                let body = self.expand_inline_block(&function.body);
                self.env.pop_scope();
                Ok(Expr::Function(FunctionExpr {
                    generics: function.generics.clone(),
                    params: function.params.clone(),
                    return_type: function.return_type.clone(),
                    body: body?,
                }))
            }
            Expr::Chain { base, segments } => Ok(Expr::Chain {
                base: Box::new(self.expand_expr(base)?),
                segments: segments
                    .iter()
                    .map(|segment| match segment {
                        ChainSegment::Field { name, safe } => Ok(ChainSegment::Field {
                            name: name.clone(),
                            safe: *safe,
                        }),
                        ChainSegment::Index { expr, safe } => Ok(ChainSegment::Index {
                            expr: Box::new(self.expand_expr(expr)?),
                            safe: *safe,
                        }),
                        ChainSegment::Call { type_args, args } => Ok(ChainSegment::Call {
                            type_args: type_args.clone(),
                            args: args
                                .iter()
                                .map(|arg| self.expand_expr(arg))
                                .collect::<Result<Vec<_>>>()?,
                        }),
                        ChainSegment::MethodCall {
                            name,
                            type_args,
                            args,
                            safe,
                        } => Ok(ChainSegment::MethodCall {
                            name: name.clone(),
                            type_args: type_args.clone(),
                            args: args
                                .iter()
                                .map(|arg| self.expand_expr(arg))
                                .collect::<Result<Vec<_>>>()?,
                            safe: *safe,
                        }),
                    })
                    .collect::<Result<Vec<_>>>()?,
            }),
            Expr::Pipe { left, stages } => {
                self.env.push_scope();
                self.env.define_runtime_name("_");
                let stages = stages
                    .iter()
                    .map(|stage| match stage {
                        PipeStage::Method { name, args } => Ok(PipeStage::Method {
                            name: name.clone(),
                            args: args
                                .iter()
                                .map(|arg| self.expand_expr(arg))
                                .collect::<Result<Vec<_>>>()?,
                        }),
                        PipeStage::Expr { callee } => Ok(PipeStage::Expr {
                            callee: Box::new(self.expand_expr(callee)?),
                        }),
                        PipeStage::Call { callee, args } => Ok(PipeStage::Call {
                            callee: Box::new(self.expand_expr(callee)?),
                            args: args
                                .iter()
                                .map(|arg| self.expand_expr(arg))
                                .collect::<Result<Vec<_>>>()?,
                        }),
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.env.pop_scope();
                Ok(Expr::Pipe {
                    left: Box::new(self.expand_expr(left)?),
                    stages,
                })
            }
            Expr::Comprehension(comprehension) => self.expand_comprehension(comprehension),
            Expr::SignalHandler(handler) => {
                self.env.push_scope();
                for param in &handler.params {
                    self.env.define_runtime_name(param);
                }
                let body = self.expand_inline_block(&handler.body);
                self.env.pop_scope();
                Ok(Expr::SignalHandler(Box::new(SignalHandlerExpr {
                    signal: self.expand_expr(&handler.signal)?,
                    params: handler.params.clone(),
                    body: body?,
                    once: handler.once,
                })))
            }
            Expr::Nil
            | Expr::Bool(_)
            | Expr::Number(_)
            | Expr::String(_)
            | Expr::Pattern(_)
            | Expr::VarArg
            | Expr::Name(_) => Ok(expr.clone()),
        }
    }

    fn expand_comprehension(&mut self, comprehension: &TableComprehension) -> Result<Expr> {
        self.env.push_scope();
        let clauses = comprehension
            .clauses
            .iter()
            .map(|clause| match clause {
                ComprehensionClause::GenericFor {
                    bindings,
                    iterables,
                } => {
                    let iterables = iterables
                        .iter()
                        .map(|iterable| self.expand_expr(iterable))
                        .collect::<Result<Vec<_>>>()?;
                    self.define_runtime_pattern_names(bindings);
                    Ok(ComprehensionClause::GenericFor {
                        bindings: bindings.clone(),
                        iterables,
                    })
                }
                ComprehensionClause::NumericFor {
                    name,
                    start,
                    end,
                    step,
                } => {
                    let start = self.expand_expr(start)?;
                    let end = self.expand_expr(end)?;
                    let step = step
                        .as_ref()
                        .map(|step| self.expand_expr(step))
                        .transpose()?;
                    self.env.define_runtime_name(name);
                    Ok(ComprehensionClause::NumericFor {
                        name: name.clone(),
                        start,
                        end,
                        step,
                    })
                }
                ComprehensionClause::Filter(expr) => {
                    Ok(ComprehensionClause::Filter(self.expand_expr(expr)?))
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let kind = match &comprehension.kind {
            TableComprehensionKind::Array { value } => TableComprehensionKind::Array {
                value: Box::new(self.expand_expr(value)?),
            },
            TableComprehensionKind::Map { key, value } => TableComprehensionKind::Map {
                key: Box::new(self.expand_expr(key)?),
                value: Box::new(self.expand_expr(value)?),
            },
        };
        self.env.pop_scope();
        Ok(Expr::Comprehension(Box::new(TableComprehension {
            kind,
            clauses,
        })))
    }

    fn expand_spawn_handler(&mut self, handler: &SpawnHandler) -> Result<SpawnHandler> {
        self.env.push_scope();
        for param in &handler.params {
            self.env.define_runtime_name(param);
        }
        let block = self.expand_inline_block(&handler.block);
        self.env.pop_scope();
        Ok(SpawnHandler {
            params: handler.params.clone(),
            block: block?,
        })
    }

    fn expand_object_method(
        &mut self,
        object: &ObjectDecl,
        method: &ObjectMethod,
    ) -> Result<ObjectMethod> {
        self.env.push_scope();
        if !method.is_static {
            self.env.define_runtime_name("self");
        }
        if object.extends.is_some() {
            self.env.define_runtime_name("super");
        }
        for param in &method.params {
            if let Param::Binding(binding) = param {
                self.define_runtime_pattern_name(&binding.pattern);
            }
        }
        let body = self.expand_inline_block(&method.body);
        self.env.pop_scope();
        Ok(ObjectMethod {
            name: method.name.clone(),
            is_static: method.is_static,
            generics: method.generics.clone(),
            params: method.params.clone(),
            return_type: method.return_type.clone(),
            body: body?,
        })
    }

    fn expand_function_body(
        &mut self,
        self_name: Option<&str>,
        params: &[Param],
        body: &Block,
    ) -> Result<Block> {
        self.env.push_scope();
        if let Some(self_name) = self_name {
            self.env.define_runtime_name(self_name);
        }
        for param in params {
            if let Param::Binding(binding) = param {
                self.define_runtime_pattern_name(&binding.pattern);
            }
        }
        let body = self.expand_inline_block(body);
        self.env.pop_scope();
        body
    }

    fn expand_comptime_if(&mut self, if_stmt: &IfStmt) -> Result<Vec<Stmt>> {
        for (condition, block) in &if_stmt.branches {
            let value = self.evaluate_comptime_expr(condition)?;
            let CtValue::Bool(condition) = value else {
                return Err(CompilerError::Other(
                    "comptime if condition must evaluate to a boolean.".to_string(),
                ));
            };
            if condition {
                return self.expand_inline_block(block);
            }
        }

        if let Some(block) = &if_stmt.else_block {
            return self.expand_inline_block(block);
        }

        Ok(Vec::new())
    }

    fn expand_comptime_switch(&mut self, switch_stmt: &SwitchStmt) -> Result<Vec<Stmt>> {
        let subject = self.evaluate_comptime_expr(&switch_stmt.value)?;
        for case in &switch_stmt.cases {
            if self.evaluate_comptime_expr(&case.value)? == subject {
                return self.expand_inline_block(&case.block);
            }
        }

        if let Some(default) = &switch_stmt.default {
            return self.expand_inline_block(default);
        }

        Ok(Vec::new())
    }

    fn define_comptime_local(&mut self, local: &LocalDecl) -> Result<()> {
        let mut evaluator = Evaluator::new(self.env.clone(), self.options.clone());
        let mut values = local
            .values
            .iter()
            .map(|value| evaluator.eval_expr(value))
            .collect::<Result<Vec<_>>>()?;
        while values.len() < local.bindings.len() {
            values.push(CtValue::Nil);
        }
        for (index, binding) in local.bindings.iter().enumerate() {
            bind_comptime_pattern(
                &mut self.env,
                &binding.pattern,
                values.get(index).cloned().unwrap_or(CtValue::Nil),
                local.is_const || local.is_comptime,
                &mut evaluator,
                None,
            )?;
        }
        Ok(())
    }

    fn define_comptime_function(&mut self, function: &FunctionDecl) -> Result<()> {
        if !function.local_name
            || !function.name.fields.is_empty()
            || function.name.method.is_some()
        {
            return Err(CompilerError::Other(
                "comptime function declarations must use a simple local name.".to_string(),
            ));
        }
        if function.is_task {
            return Err(CompilerError::Other(
                "task comptime functions are not supported.".to_string(),
            ));
        }
        let handle = Arc::new(CtFunction {
            name: Some(function.name.root.clone()),
            params: function.params.clone(),
            body: function.body.clone(),
            env: self.env.clone(),
            options: self.options.clone(),
        });
        self.env.define_function(&function.name.root, handle);
        Ok(())
    }

    fn evaluate_comptime_expr(&self, expr: &Expr) -> Result<CtValue> {
        let mut evaluator = Evaluator::new(self.env.clone(), self.options.clone());
        evaluator.eval_expr(expr)
    }

    fn define_runtime_pattern_names(&mut self, bindings: &[Binding]) {
        for binding in bindings {
            self.define_runtime_pattern_name(&binding.pattern);
        }
    }

    fn define_runtime_pattern_name(&mut self, pattern: &Pattern) {
        for name in pattern_names(pattern) {
            self.env.define_runtime_name(&name);
        }
    }

    fn define_match_pattern_names(&mut self, pattern: &MatchPattern) {
        for name in match_pattern_names(pattern) {
            self.env.define_runtime_name(&name);
        }
    }

    fn define_runtime_function_name(&mut self, function: &FunctionDecl) {
        if function.local_name {
            self.env.define_runtime_name(&function.name.root);
        }
    }
}

fn bind_comptime_pattern(
    env: &mut CtEnv,
    pattern: &Pattern,
    value: CtValue,
    is_const: bool,
    evaluator: &mut Evaluator,
    default_value: Option<&Expr>,
) -> Result<()> {
    let value = if matches!(value, CtValue::Nil) {
        if let Some(default_value) = default_value {
            evaluator.eval_expr(default_value)?
        } else {
            value
        }
    } else {
        value
    };

    match pattern {
        Pattern::Name(name) => {
            env.define_value(name, value, is_const);
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
                    bind_comptime_pattern(
                        env,
                        &binding.target,
                        array.items.get(index).cloned().unwrap_or(CtValue::Nil),
                        is_const,
                        evaluator,
                        binding.default_value.as_ref(),
                    )?;
                }
            }
            if let Some(rest) = rest {
                env.define_value(
                    rest,
                    CtValue::Array(super::value::CtArray {
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
                let value = table
                    .entries
                    .iter()
                    .find(|(key, _)| key == &entry.key)
                    .map(|(_, value)| value.clone())
                    .unwrap_or(CtValue::Nil);
                bind_comptime_pattern(
                    env,
                    &entry.binding.target,
                    value,
                    is_const,
                    evaluator,
                    entry.binding.default_value.as_ref(),
                )?;
            }
            if let Some(rest) = rest {
                env.define_value(
                    rest,
                    CtValue::Table(super::value::CtTable {
                        entries: table
                            .entries
                            .into_iter()
                            .filter(|(key, _)| !entries.iter().any(|entry| entry.key == *key))
                            .collect(),
                        frozen: false,
                    }),
                    is_const,
                );
            }
            Ok(())
        }
    }
}

fn pattern_names(pattern: &Pattern) -> Vec<String> {
    let mut names = Vec::new();
    match pattern {
        Pattern::Name(name) => names.push(name.clone()),
        Pattern::Table { entries, rest } => {
            for entry in entries {
                names.extend(pattern_names(&entry.binding.target));
            }
            if let Some(rest) = rest {
                names.push(rest.clone());
            }
        }
        Pattern::Array { items, rest } => {
            for item in items {
                if let Some(binding) = &item.binding {
                    names.extend(pattern_names(&binding.target));
                }
            }
            if let Some(rest) = rest {
                names.push(rest.clone());
            }
        }
    }
    names
}

fn match_pattern_names(pattern: &MatchPattern) -> Vec<String> {
    match pattern {
        MatchPattern::Literal(_) => Vec::new(),
        MatchPattern::Bind(name) => vec![name.clone()],
        MatchPattern::Table(fields) => fields
            .iter()
            .flat_map(|field| match_pattern_names(&field.pattern))
            .collect(),
    }
}
