use std::{
    fs,
    path::{Path, PathBuf},
};

use xluau::Compiler;

fn assert_module_fixture(project: &str, entry_file: &str) {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = repo_root
        .join("tests")
        .join("module_projects")
        .join(project);
    let source_path = fixture_dir.join(entry_file);
    let expected_path = fixture_dir.join("expected.luau");

    let expected = fs::read_to_string(&expected_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", expected_path.display()));

    let compiler = Compiler::discover(&fixture_dir).expect("compiler");
    let artifact = compiler.build_file(&source_path).expect("build artifact");

    assert_eq!(
        expected,
        artifact.luau,
        "build_file output mismatch for fixture {}",
        display_relative(&repo_root, &source_path)
    );
}

fn assert_cycle_fixture(project: &str, entry_file: &str, expected_paths: &[&str]) {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = repo_root
        .join("tests")
        .join("module_projects")
        .join(project);
    let source_path = fixture_dir.join(entry_file);
    let compiler = Compiler::discover(&fixture_dir).expect("compiler");
    let error = compiler.build_file(&source_path).expect_err("cycle error");
    let rendered = error.to_string();

    assert!(
        rendered.contains("Circular dependency detected"),
        "expected cycle error for fixture {} but got: {}",
        display_relative(&repo_root, &source_path),
        rendered
    );

    for path in expected_paths {
        assert!(
            rendered.contains(path),
            "expected cycle error for fixture {} to mention {} but got: {}",
            display_relative(&repo_root, &source_path),
            path,
            rendered
        );
    }
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn module_project_filesystem_alias_index() {
    assert_module_fixture("filesystem_alias_index", "src/main.xl");
}

#[test]
fn module_project_roblox_alias() {
    assert_module_fixture("roblox_alias", "src/server/main.xl");
}

#[test]
fn module_project_roblox_charsafe() {
    assert_module_fixture("roblox_charsafe", "src/server/main.xl");
}

#[test]
fn module_project_custom_alias() {
    assert_module_fixture("custom_alias", "src/main.xl");
}

#[test]
fn module_project_detects_cycle() {
    assert_cycle_fixture(
        "cycle",
        "src/main.xl",
        &["src/a.xl", "src/b.xl", "src/c.xl"],
    );
}

#[test]
fn module_project_task_roblox_adapter() {
    assert_module_fixture("task_roblox_adapter", "src/main.xl");
}
