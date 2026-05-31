use std::{
    fs,
    path::{Path, PathBuf},
};

use xluau::Compiler;

fn assert_fixture(project: &str, entry_file: &str) {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = repo_root.join("tests").join("projects").join(project);
    let source_path = fixture_dir.join(entry_file);
    let expected_path = fixture_dir.join("expected.luau");

    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()));
    let expected = fs::read_to_string(&expected_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", expected_path.display()));

    let compiler = Compiler::discover(&repo_root).expect("compiler");
    let actual = compiler.compile_source(&source).expect("compiled output");

    assert_eq!(
        expected,
        actual,
        "compiled output mismatch for fixture {}",
        display_relative(&repo_root, &source_path)
    );

    let artifact = compiler.build_file(&source_path).expect("build artifact");
    assert_eq!(
        expected,
        artifact.luau,
        "build_file output mismatch for fixture {}",
        display_relative(&repo_root, &source_path)
    );
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn project_nullish_ternary() {
    assert_fixture("nullish_ternary", "main.xl");
}

#[test]
fn project_optional_pipe() {
    assert_fixture("optional_pipe", "main.xl");
}

#[test]
fn project_const_destructure() {
    assert_fixture("const_destructure", "main.xl");
}

#[test]
fn project_mixed_features() {
    assert_fixture("mixed_features", "main.xl");
}

#[test]
fn project_switch_enum_do() {
    assert_fixture("switch_enum_do", "main.xl");
}

#[test]
fn project_match_comprehension() {
    assert_fixture("match_comprehension", "main.xl");
}

#[test]
fn project_all_features() {
    assert_fixture("all_features", "main.xl");
}

#[test]
fn project_phase5_type_system() {
    assert_fixture("phase5_type_system", "main.xl");
}

#[test]
fn project_all_features_phase5() {
    assert_fixture("all_features_phase5", "main.xl");
}

#[test]
fn project_object_task() {
    assert_fixture("object_task", "main.xl");
}

#[test]
fn project_all_features_phase6() {
    assert_fixture("all_features_phase6", "main.xl");
}

#[test]
fn project_phase7_signals_state() {
    assert_fixture("phase7_signals_state", "main.xl");
}

#[test]
fn project_all_features_phase7() {
    assert_fixture("all_features_phase7", "main.xl");
}

#[test]
fn project_comptime() {
    assert_fixture("comptime", "main.xl");
}

#[test]
fn project_luau_passthrough() {
    assert_fixture("luau_passthrough", "main.luau");
}
