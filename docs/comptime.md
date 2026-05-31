# Compile-Time Evaluation

`comptime` adds a small, deterministic compile-time evaluation layer to XLuau.

It is not a C-style preprocessor, and it is not arbitrary Luau execution inside the compiler.

The compiler only evaluates a restricted set of values and expressions, then lowers the result back into normal Luau literals and statements.

## What It Supports

This first version supports:

- `comptime const`
- `comptime function`
- `comptime if`
- `comptime switch`
- inline `comptime expression`
- explicit compile-time folding in runtime `const` and `local` initializers

Supported compile-time values:

- `nil`
- booleans
- numbers
- strings
- array-like tables
- dictionary-like tables with string keys
- compile-time functions

## `comptime const`

Use `comptime const` for values that only exist during compilation:

```lua
comptime const TARGET = "roblox"
comptime const DEBUG = true
```

These declarations do not emit runtime Luau by themselves.

To move a compile-time value into runtime code, embed it explicitly:

```lua
local target = comptime TARGET
```

Emitted Luau:

```lua
local target = "roblox"
```

## `comptime function`

Compile-time functions run in the restricted evaluator:

```lua
comptime function makeName(prefix: string, name: string): string
    return prefix .. "-" .. name
end

local pluginName = comptime makeName("rbxup", "plugin")
```

Emitted Luau:

```lua
local pluginName = "rbxup-plugin"
```

This version supports:

- parameters
- local variables
- `return`
- `if` / `elseif` / `else`
- numeric `for`
- generic `for` over compile-time arrays and tables
- table mutation on compile-time tables
- calls to other `comptime function`s
- calls to safe builtins

Current limits:

- no varargs
- no arbitrary runtime function calls
- no arbitrary Luau closures or coroutines

## `comptime if`

`comptime if` selects a branch during compilation:

```lua
comptime const DEBUG = false

comptime if DEBUG then
    print("debug")
else
    print("release")
end
```

Emitted Luau:

```lua
print("release")
```

The condition must evaluate to a boolean.

## `comptime switch`

`comptime switch` works like branch selection over compile-time values:

```lua
comptime const TARGET = "roblox"

comptime switch TARGET
    case "roblox"
        const ADAPTER = "task"
    default
        error("unknown target")
end
```

Only the selected branch remains in the emitted program.

## Inline `comptime expression`

Use inline `comptime` when you want a literal embedded into runtime code:

```lua
local timeout = comptime 30 * 2
const ENDPOINT = comptime "/api/" .. "v1"
```

Emitted Luau:

```lua
local timeout = 60
local ENDPOINT = "/api/v1"
```

## Safe Builtins

Builtins available at compile time:

- `len`
- `keys`
- `values`
- `has`
- `freeze`
- `upper`
- `lower`
- `replace`
- `startsWith`
- `endsWith`
- `error`
- `warn`
- `join`
- `split`
- `trim`

String helpers can also be used with method syntax where it maps cleanly:

```lua
comptime function upperName(name: string): string
    return name:upper()
end
```

## What Is Intentionally Not Supported

This version does not support:

- full macros
- AST quote/splice
- compile-time `require`
- compile-time Roblox APIs
- compile-time HTTP
- compile-time filesystem access
- compile-time package installs
- `os`, `io`, `debug`, or host process access
- arbitrary runtime Luau execution

The same source and config should produce the same output every time.

## Common Errors

Runtime locals cannot be used in compile-time expressions:

```lua
local x = getValue()
comptime const y = x
```

Typical diagnostic:

```txt
Cannot use runtime local 'x' in a compile-time expression.
```

Unsupported runtime functions are rejected:

```lua
comptime const now = os.clock()
```

Typical diagnostic:

```txt
Function 'os.clock' is not available at compile time.
```

`comptime if` conditions must be booleans:

```lua
comptime if "yes" then
    print("bad")
end
```

Typical diagnostic:

```txt
comptime if condition must evaluate to a boolean.
```

## Example

```lua
comptime const MAIN_PROPERTIES = freeze {
    BasePart = {
        "CFrame",
        "Size",
        "Anchored",
    },
}

comptime function makeSet(list)
    local out = {}
    for _, value in list do
        out[value] = true
    end
    return freeze(out)
end

const BASEPART_MAIN_SET = comptime makeSet(MAIN_PROPERTIES.BasePart)
```

Emitted Luau:

```lua
local BASEPART_MAIN_SET = table.freeze({
    CFrame = true,
    Size = true,
    Anchored = true,
})
```

## Future Work

Planned follow-up areas:

- compile-time modules
- derive/code generation helpers
- AST macros
- quote/splice support
- richer type integration
- generated serializers and codecs
- optional controlled file inputs
- explicitly enabled compile-time JSON imports
