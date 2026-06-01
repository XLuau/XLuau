use std::path::PathBuf;

use xluau::Compiler;

fn compiler() -> Compiler {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Compiler::discover(&repo_root).expect("compiler")
}

#[test]
fn comptime_rejects_runtime_value_capture() {
    let source = r#"
local x = getValue()
comptime const y = x
"#;
    let err = compiler()
        .compile_source(source)
        .expect_err("compile-time error");
    assert!(
        err.to_string()
            .contains("Cannot use runtime local 'x' in a compile-time expression.")
    );
}

#[test]
fn comptime_rejects_unsupported_host_call() {
    let source = r#"
comptime const now = os.clock()
"#;
    let err = compiler()
        .compile_source(source)
        .expect_err("compile-time error");
    assert!(
        err.to_string()
            .contains("Function 'os.clock' is not available at compile time.")
    );
}

#[test]
fn comptime_requires_boolean_if_conditions() {
    let source = r#"
comptime if "yes" then
    print("bad")
end
"#;
    let err = compiler()
        .compile_source(source)
        .expect_err("compile-time error");
    assert!(
        err.to_string()
            .contains("comptime if condition must evaluate to a boolean.")
    );
}
