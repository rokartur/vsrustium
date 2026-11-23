//! Headless check of the extension host seam: start the host on Bun or Node, complete the handshake,
//! round-trip one call each way, then activate every built-in extension that carries code.
//! Prints a JSON report; port/tools/exthost-compare.ts diffs the Bun and Node reports.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use exthost::rpc::{Arg, Reply};
use exthost::scan::{extension_identifier, scan_builtin_extensions};
use exthost::session::{Session, SessionPaths, init_data, workspace_data};
use exthost::{ExtensionHost, HostOptions, Runtime};
use serde_json::{Value, json};

fn main() -> anyhow::Result<()> {
    let mut runtime = Runtime::Bun;
    let mut entry = PathBuf::from("out-exthost/bootstrap-exthost.js");
    let mut extensions_dir = PathBuf::from("extensions");
    let mut workspace: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| anyhow::anyhow!("{flag} needs a value"));
        match flag.as_str() {
            "--runtime" => runtime = if value()? == "node" { Runtime::Node } else { Runtime::Bun },
            "--entry" => entry = value()?.into(),
            "--extensions" => extensions_dir = value()?.into(),
            "--workspace" => workspace = Some(value()?.into()),
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    let extensions_dir = std::fs::canonicalize(extensions_dir)?;
    let user_data = std::env::temp_dir().join(format!("vscode-gpui-probe-{}", std::process::id()));
    std::fs::create_dir_all(&user_data)?;

    let extensions = scan_builtin_extensions(&extensions_dir)?;
    let paths = SessionPaths { app_root: extensions_dir.parent().unwrap(), user_data: &user_data, workspace_folder: workspace.as_deref() };
    let init = init_data(&paths, &extensions, "1.147.0");

    let started = Instant::now();
    let host = ExtensionHost::start(HostOptions { runtime, entry: &entry, extra_env: vec![], extra_args: vec![] }, &init)?;
    let handshake_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut session = Session::new(host);
    session.start(&extensions, workspace.as_deref().map(|folder| workspace_data(&init, folder)))?;
    let started_ms = started.elapsed().as_secs_f64() * 1000.0;

    // Main -> host round trip.
    let latency = session.call("ExtHostExtensionService", "$test_latency", vec![Arg::Json(json!(7))], Duration::from_secs(10))?;
    let main_to_host_ok = latency == Reply::Json(json!(7));

    let mut activations = Vec::new();
    for extension in extensions.iter().filter(|e| e.has_code()) {
        let reason = json!({ "startup": false, "extensionId": extension_identifier(&extension.id), "activationEvent": "api" });
        let begin = Instant::now();
        let result = session.call(
            "ExtHostExtensionService",
            "$activate",
            vec![Arg::Json(extension_identifier(&extension.id)), Arg::Json(reason)],
            Duration::from_secs(60),
        );
        let error = session.log.activation_errors.get(&extension.id).cloned();
        activations.push(json!({
            "id": extension.id,
            "ms": (begin.elapsed().as_secs_f64() * 1000.0).round(),
            "result": match &result { Ok(Reply::Json(value)) => value.clone(), Ok(other) => json!(format!("{other:?}")), Err(e) => json!(format!("error: {e}")) },
            "activationError": error,
        }));
    }
    // Let late activation errors and console output arrive.
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(2) {
        session.pump(Duration::from_millis(100))?;
    }
    for activation in &mut activations {
        let id = activation["id"].as_str().unwrap().to_owned();
        if let Some(error) = session.log.activation_errors.get(&id) {
            activation["activationError"] = json!(error);
        }
    }

    let host_to_main_ok = session.log.host_calls.contains_key("MainThreadExtensionService.$onWillActivateExtension");
    let report = json!({
        "runtime": format!("{runtime:?}"),
        "handshakeMs": handshake_ms.round(),
        "startedMs": started_ms.round(),
        "mainToHostRoundTrip": main_to_host_ok,
        "hostToMainRoundTrip": host_to_main_ok,
        "codeExtensions": activations.len(),
        "activations": activations,
        "hostCalls": session.log.host_calls,
        "console": session.log.console,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    drop(session);
    let _ = std::fs::remove_dir_all(&user_data);
    let _: Value = report;
    Ok(())
}
