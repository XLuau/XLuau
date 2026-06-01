use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceMap {
    pub version: u32,
    pub source_file: String,
    pub emitted_file: String,
    pub mappings: Vec<SourceMapEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceMapEntry {
    pub emitted_line: usize,
    pub source_line: usize,
    pub source_col: usize,
}

pub fn finalize_output(
    formatted: &str,
    keep_pragmas: bool,
    source_file: &Path,
    emitted_file: &Path,
) -> (String, SourceMap) {
    let mut output_lines = Vec::new();
    let mut mappings = Vec::new();
    let mut current_source = None::<usize>;

    for line in formatted.lines() {
        if let Some(source_line) = parse_line_pragma(line) {
            current_source = Some(source_line);
            if keep_pragmas {
                output_lines.push(line.to_string());
            }
            continue;
        }

        output_lines.push(line.to_string());
        if let Some(source_line) = current_source {
            mappings.push(SourceMapEntry {
                emitted_line: output_lines.len(),
                source_line,
                source_col: 1,
            });
        }
    }

    let mut output = output_lines.join("\n");
    if formatted.ends_with('\n') {
        output.push('\n');
    }

    (
        output,
        SourceMap {
            version: 1,
            source_file: normalize_path(source_file),
            emitted_file: normalize_path(emitted_file),
            mappings,
        },
    )
}

pub fn remap_trace(trace: &str, cwd: &Path) -> String {
    trace
        .lines()
        .map(|line| remap_trace_line(line, cwd))
        .collect::<Vec<_>>()
        .join("\n")
}

fn remap_trace_line(line: &str, cwd: &Path) -> String {
    let Some((path_text, emitted_line)) = extract_luau_location(line) else {
        return line.to_string();
    };
    let luau_path = absolutize(cwd, Path::new(&path_text));
    let map_path = PathBuf::from(format!("{}.map", luau_path.display()));
    let Ok(contents) = std::fs::read_to_string(&map_path) else {
        return line.to_string();
    };
    let Ok(map) = serde_json::from_str::<SourceMap>(&contents) else {
        return line.to_string();
    };
    let Some(entry) = map
        .mappings
        .iter()
        .rev()
        .find(|entry| entry.emitted_line <= emitted_line)
    else {
        return line.to_string();
    };

    let needle = format!("{path_text}:{emitted_line}");
    line.replacen(
        &needle,
        &format!("{}:{}", map.source_file, entry.source_line),
        1,
    )
}

fn extract_luau_location(line: &str) -> Option<(String, usize)> {
    let marker = ".luau:";
    let start = line.find(marker)?;
    let prefix = &line[..start + marker.len() - 1];
    let digits = line[start + marker.len()..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return None;
    }
    let emitted_line = digits.parse().ok()?;
    Some((prefix.to_string(), emitted_line))
}

fn parse_line_pragma(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("--@line ")?;
    let digits = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn absolutize(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{SourceMap, finalize_output, remap_trace};

    #[test]
    fn strips_pragmas_and_emits_line_mappings() {
        let formatted =
            "--@line 3 \"src/main.xl\"\nlocal a = 1\n--@line 8 \"src/main.xl\"\nprint(a)\n";
        let (output, map) = finalize_output(
            formatted,
            false,
            Path::new("src/main.xl"),
            Path::new("out/main.luau"),
        );

        assert_eq!(output, "local a = 1\nprint(a)\n");
        assert_eq!(
            map.mappings,
            vec![
                super::SourceMapEntry {
                    emitted_line: 1,
                    source_line: 3,
                    source_col: 1
                },
                super::SourceMapEntry {
                    emitted_line: 2,
                    source_line: 8,
                    source_col: 1
                }
            ]
        );
    }

    #[test]
    fn preserves_pragmas_when_requested() {
        let formatted = "--@line 3 \"src/main.xl\"\nlocal a = 1\n";
        let (output, map) = finalize_output(
            formatted,
            true,
            Path::new("src/main.xl"),
            Path::new("out/main.luau"),
        );

        assert_eq!(output, formatted);
        assert_eq!(
            map,
            SourceMap {
                version: 1,
                source_file: "src/main.xl".to_string(),
                emitted_file: "out/main.luau".to_string(),
                mappings: vec![super::SourceMapEntry {
                    emitted_line: 2,
                    source_line: 3,
                    source_col: 1,
                }],
            }
        );
    }

    #[test]
    fn remaps_stack_trace_lines() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("xluau_sourcemap_{nonce}"));
        fs::create_dir_all(root.join("out")).expect("mkdir");
        let map_path = root.join("out/main.luau.map");
        let map = SourceMap {
            version: 1,
            source_file: "src/main.xl".to_string(),
            emitted_file: root
                .join("out/main.luau")
                .to_string_lossy()
                .replace('\\', "/"),
            mappings: vec![super::SourceMapEntry {
                emitted_line: 10,
                source_line: 4,
                source_col: 1,
            }],
        };
        fs::write(&map_path, serde_json::to_string(&map).expect("json")).expect("map");

        let trace = format!(
            "{}:12: attempt to call nil",
            root.join("out/main.luau").display()
        );
        let remapped = remap_trace(&trace, Path::new("/unused"));
        assert!(remapped.contains("src/main.xl:4"));
    }
}
