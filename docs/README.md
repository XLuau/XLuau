# XLuau Docs

XLuau is a superset of Luau that transpiles to readable Luau with no runtime dependency.

These docs are written for people learning the language, not just implementing the compiler.
They explain what XLuau is for, how the syntax works, when to use each feature, and what the emitted Luau looks like.

## Start Here

- [Getting Started](./getting-started.md)
- [Language Tour](./language-tour.md)
- [Feature Status](./feature-status.md)
- [Compile-Time Evaluation](./comptime.md)
- [Tooling and Project Setup](./tooling.md)

## Guides

### Implemented Today

- [Modules and Imports](./guides/modules.md)
- [Expressions and Operators](./guides/expressions.md)
- [Bindings, Const, and Destructuring](./guides/bindings.md)
- [Control Flow and Data](./guides/control-flow.md)

### Designed Language Features

- [Objects and Task Functions](./guides/objects-and-tasks.md)
- [Signals and Reactive State](./guides/signals-and-state.md)
- [Type System Extensions](./guides/type-system.md)
- [Pattern Literals and Readonly Sugar](./guides/patterns-and-readonly.md)

## Reference

- [Configuration Reference](./reference/config.md)
- [CLI Reference](./reference/cli.md)
- [Transpilation Model](./reference/transpilation.md)
- [Source Maps and Debugging](./reference/source-maps.md)

## Reading Order

If you are new to XLuau, this order works well:

1. Read [Getting Started](./getting-started.md).
2. Skim [Language Tour](./language-tour.md).
3. Read the implemented guides in order.
4. Use [Feature Status](./feature-status.md) to separate what works today from what is still planned.
5. Use the reference pages when setting up a real project.
