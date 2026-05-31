use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use xluau::Compiler;

fn temp_project(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("xluau_comptime_http_{name}_{nonce}"));
    fs::create_dir_all(&root).expect("temp project root");
    root
}

fn write_file(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, contents).expect("write file");
}

fn start_test_server(body: &'static str, status_code: u16, status_text: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let address = format!("http://{}", listener.local_addr().expect("addr"));

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buffer = [0u8; 2048];
        let _ = stream.read(&mut buffer);
        let response = format!(
            "HTTP/1.1 {status_code} {status_text}\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("write response");
        stream.flush().expect("flush");
    });

    address
}

#[test]
fn comptime_http_get_embeds_response_values() {
    let root = temp_project("success");
    let base_url = start_test_server("hello from comptime", 200, "OK");
    write_file(
        &root,
        "xluau.config.json",
        &format!(
            r#"{{
  "include": ["src/**/*.xl"],
  "comptimeHttp": {{
    "enabled": true,
    "allow": ["{base_url}/"],
    "timeoutMs": 3000
  }}
}}"#
        ),
    );
    write_file(
        &root,
        "src/main.xl",
        &format!(
            r#"
comptime const RESPONSE = httpGet("{base_url}/message")
local body = comptime RESPONSE.body
local status = comptime RESPONSE.status
local ok = comptime RESPONSE.ok

return body, status, ok
"#
        ),
    );

    let compiler = Compiler::discover(&root).expect("compiler");
    let artifact = compiler.build_file(&root.join("src/main.xl")).expect("artifact");
    assert!(artifact.luau.contains(r#"local body = "hello from comptime""#));
    assert!(artifact.luau.contains("local status = 200"));
    assert!(artifact.luau.contains("local ok = true"));
}

#[test]
fn comptime_http_requires_explicit_enablement() {
    let root = temp_project("disabled");
    write_file(
        &root,
        "xluau.config.json",
        r#"{
  "include": ["src/**/*.xl"]
}"#,
    );
    write_file(
        &root,
        "src/main.xl",
        r#"
comptime const RESPONSE = httpGet("http://127.0.0.1:9999/test")
return comptime RESPONSE.body
"#,
    );

    let compiler = Compiler::discover(&root).expect("compiler");
    let err = compiler.build_file(&root.join("src/main.xl")).expect_err("disabled error");
    assert!(err
        .to_string()
        .contains("Compile-time HTTP is disabled. Enable `comptimeHttp.enabled` in xluau.config.json."));
}

#[test]
fn comptime_http_enforces_allowlist_prefixes() {
    let root = temp_project("allow");
    write_file(
        &root,
        "xluau.config.json",
        r#"{
  "include": ["src/**/*.xl"],
  "comptimeHttp": {
    "enabled": true,
    "allow": ["https://allowed.example/"],
    "timeoutMs": 3000
  }
}"#,
    );
    write_file(
        &root,
        "src/main.xl",
        r#"
comptime const RESPONSE = httpGet("https://blocked.example/data")
return comptime RESPONSE.body
"#,
    );

    let compiler = Compiler::discover(&root).expect("compiler");
    let err = compiler.build_file(&root.join("src/main.xl")).expect_err("allowlist error");
    assert!(err
        .to_string()
        .contains("Compile-time HTTP request to `https://blocked.example/data` is not allowed."));
}
