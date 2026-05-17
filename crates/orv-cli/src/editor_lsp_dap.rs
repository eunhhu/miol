#![allow(clippy::redundant_pub_crate, clippy::wildcard_imports)]

use super::*;

pub(crate) fn cmd_editor_snapshot(path: &Path) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let value = editor_snapshot_json(&entry)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) fn cmd_editor_reveal(dir: &Path, origin_id: &str) -> anyhow::Result<()> {
    let value = editor_reveal_json(dir, origin_id)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) fn cmd_editor_runtime(path: &Path) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let value = editor_runtime_json(&entry)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) fn cmd_editor_debug(
    path: &Path,
    controls: &[EditorDebugControl],
    breakpoints: &[EditorDebugBreakpoint],
    function_breakpoints: &[String],
    data_breakpoints: &[String],
    exception_filters: &[String],
    watch_expressions: &[String],
) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let value = editor_debug_session_json(
        &entry,
        controls,
        breakpoints,
        function_breakpoints,
        data_breakpoints,
        exception_filters,
        watch_expressions,
    )?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) fn cmd_editor_run_debug(
    state: &Path,
    controls: &[EditorDebugControl],
    breakpoints: &[EditorDebugBreakpoint],
    function_breakpoints: &[String],
    data_breakpoints: &[String],
    exception_filters: &[String],
    watch_expressions: &[String],
) -> anyhow::Result<()> {
    let value = editor_debug_runner_session_json(
        state,
        controls,
        breakpoints,
        function_breakpoints,
        data_breakpoints,
        exception_filters,
        watch_expressions,
    )?;
    write_editor_debug_runner_result_if_configured(state, &value)?;
    write_editor_debug_runner_result_html_if_configured(state, &value)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) fn cmd_editor_export(path: &Path, out: &Path) -> anyhow::Result<()> {
    cmd_editor_export_with_options(path, out, None, None)
}

pub(crate) fn cmd_editor_export_with_options(
    path: &Path,
    out: &Path,
    build: Option<&Path>,
    trace: Option<&Path>,
) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let state = editor_export_state_json_with_trace(&entry, build, trace)?;
    write_json(&out.join("state.json"), &state)?;
    let runner = state
        .pointer("/debug/session_runner")
        .ok_or_else(|| anyhow::anyhow!("editor export state missing debug.session_runner"))?;
    write_json(&out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH), runner)?;
    write_json(
        &out.join(EDITOR_NATIVE_HOST_MANIFEST_PATH),
        &editor_native_host_manifest_json(&entry, &state),
    )?;
    write_text(&out.join("index.html"), &editor_export_html(&state)?)?;
    let runtime_panel_written = write_editor_runtime_panel_html_if_configured(out, &state)?;
    let production_panel_written = write_editor_production_panel_html_if_configured(out, &state)?;
    let trace_panel_written = write_editor_trace_panel_html_if_configured(out, &state)?;
    let mut files = vec![
        "index.html",
        "state.json",
        EDITOR_DEBUG_SESSION_RUNNER_PATH,
        EDITOR_NATIVE_HOST_MANIFEST_PATH,
    ];
    if runtime_panel_written {
        files.push(EDITOR_RUNTIME_PANEL_HTML_PATH);
    }
    if production_panel_written {
        files.push(EDITOR_PRODUCTION_PANEL_HTML_PATH);
    }
    if trace_panel_written {
        files.push(EDITOR_TRACE_PANEL_HTML_PATH);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "kind": "orv.editor.export",
            "entry": entry.display().to_string(),
            "out": out.display().to_string(),
            "files": files,
        }))?
    );
    Ok(())
}

pub(crate) fn cmd_editor_trace(dir: &Path, trace: &Path) -> anyhow::Result<()> {
    let value = editor_trace_json(dir, trace)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) fn cmd_editor_trace_stream(dir: &Path, events: &Path) -> anyhow::Result<()> {
    let value = editor_trace_stream_json(dir, events)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) fn cmd_lsp_snapshot(path: &Path) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let value = lsp_snapshot_json(&entry)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) fn cmd_lsp_reveal(dir: &Path, origin_id: &str) -> anyhow::Result<()> {
    let value = lsp_reveal_json(dir, origin_id)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(crate) fn cmd_lsp_serve(use_stdio: bool) -> anyhow::Result<()> {
    if !use_stdio {
        anyhow::bail!("lsp serve currently requires --stdio");
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    lsp_serve_stdio_stream(&mut reader, &mut writer)
}

pub(crate) fn cmd_dap_serve(use_stdio: bool) -> anyhow::Result<()> {
    if !use_stdio {
        anyhow::bail!("dap serve currently requires --stdio");
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    dap_serve_stdio_stream(&mut reader, &mut writer)
}

pub(crate) fn editor_snapshot_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    let loaded = orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let resolved = orv_resolve::resolve(&loaded.program);
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    let origin_map = orv_compiler::origin_map(&lowered.program);
    let mut diagnostics = Vec::new();
    diagnostics.extend(lsp_diagnostics_json(&loaded.diagnostics, &loaded.files));
    diagnostics.extend(lsp_diagnostics_json(&resolved.diagnostics, &loaded.files));
    diagnostics.extend(lsp_diagnostics_json(&lowered.diagnostics, &loaded.files));
    let project_graph = project_graph_json(&loaded.graph, &origin_map);
    let live_refresh = editor_live_refresh_json(&loaded.files, &project_graph)?;
    Ok(serde_json::json!({
        "schema_version": 1,
        "entry": {
            "path": path.display().to_string(),
            "uri": lsp_file_uri_for_path(path),
        },
        "diagnostics": diagnostics,
        "project_graph": project_graph,
        "live_refresh": live_refresh,
        "panels": {
            "files": editor_files_panel_json(&loaded.files, &loaded.graph),
            "routes": editor_routes_panel_json(&origin_map, &loaded.files),
            "schema": editor_schema_panel_json(&loaded.graph, &loaded.files),
            "domains": editor_domains_panel_json(&loaded.graph, &loaded.files),
        },
    }))
}

pub(crate) fn editor_live_refresh_json(
    files: &[SourceFile],
    project_graph: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "strategy": "source-hash",
        "project_graph_hash": stable_json_hash(project_graph)?,
        "watch": {
            "sources": editor_source_watch_json(files),
        },
    }))
}

pub(crate) fn editor_source_watch_json(files: &[SourceFile]) -> Vec<serde_json::Value> {
    files
        .iter()
        .map(|file| {
            serde_json::json!({
                "file": file.id.0,
                "path": file.path.display().to_string(),
                "uri": lsp_file_uri_for_path(&file.path),
                "content_hash": format!("fnv1a64:{:016x}", fnv1a64(file.source.as_bytes())),
            })
        })
        .collect()
}

pub(crate) fn editor_files_panel_json(
    files: &[SourceFile],
    graph: &ProjectGraph,
) -> Vec<serde_json::Value> {
    files
        .iter()
        .map(|file| {
            let node_id = graph
                .nodes
                .iter()
                .find(|node| node.kind == ProjectNodeKind::File && node.file == file.id)
                .map(|node| node.id);
            serde_json::json!({
                "file": file.id.0,
                "name": file.path.file_name().and_then(std::ffi::OsStr::to_str).unwrap_or(""),
                "path": file.path.display().to_string(),
                "uri": lsp_file_uri_for_path(&file.path),
                "node_id": node_id,
            })
        })
        .collect()
}

pub(crate) fn editor_routes_panel_json(
    origin_map: &orv_compiler::OriginMap,
    files: &[SourceFile],
) -> Vec<serde_json::Value> {
    origin_map
        .entries
        .iter()
        .filter(|entry| entry.kind == "route")
        .map(|entry| {
            let (method, path) = entry
                .name
                .split_once(' ')
                .unwrap_or((entry.name.as_str(), ""));
            serde_json::json!({
                "origin_id": entry.id,
                "method": method,
                "path": path,
                "name": entry.name,
                "location": editor_origin_location_json(entry.span, files),
            })
        })
        .collect()
}

pub(crate) fn editor_schema_panel_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
) -> Vec<serde_json::Value> {
    graph
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind,
                ProjectNodeKind::Struct | ProjectNodeKind::Enum | ProjectNodeKind::TypeAlias
            )
        })
        .map(|node| editor_project_node_panel_item(node, files))
        .collect()
}

pub(crate) fn editor_domains_panel_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
) -> Vec<serde_json::Value> {
    graph
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, ProjectNodeKind::Define | ProjectNodeKind::Domain))
        .map(|node| editor_project_node_panel_item(node, files))
        .collect()
}

pub(crate) fn editor_project_node_panel_item(
    node: &orv_project::ProjectNode,
    files: &[SourceFile],
) -> serde_json::Value {
    serde_json::json!({
        "node_id": node.id,
        "kind": node_kind(node.kind),
        "name": node.name,
        "location": lsp_location_json(node, files),
    })
}

pub(crate) fn editor_origin_location_json(
    span: orv_compiler::OriginSpan,
    files: &[SourceFile],
) -> serde_json::Value {
    let span = Span::new(FileId(span.file), ByteRange::new(span.start, span.end));
    let uri = files.iter().find(|file| file.id == span.file).map_or_else(
        || "file://<unknown>".to_string(),
        |file| lsp_file_uri_for_path(&file.path),
    );
    serde_json::json!({
        "uri": uri,
        "range": lsp_range_json(span, files),
    })
}

pub(crate) fn editor_reveal_json(dir: &Path, origin_id: &str) -> anyhow::Result<serde_json::Value> {
    let reveal = reveal_origin_json(dir, origin_id)?;
    let source = reveal
        .get("source")
        .ok_or_else(|| anyhow::anyhow!("reveal source missing"))?;
    let path = json_str(source, "path", "reveal source")?;
    let start = json_u32(source, "start", "reveal source")?;
    let end = json_u32(source, "end", "reveal source")?;
    let source_text = source
        .get("content")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .map_or_else(
            || {
                std::fs::read_to_string(path)
                    .map_err(|e| anyhow::anyhow!("failed to read reveal source {path}: {e}"))
            },
            Ok,
        )?;
    let origin = reveal
        .get("origin")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let project_graph = reveal
        .get("project_graph")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let production = reveal
        .get("production")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(serde_json::json!({
        "schema_version": 1,
        "origin": origin,
        "focus": editor_reveal_focus_json(&origin, &project_graph, origin_id),
        "source": {
            "file": source.get("file").cloned().unwrap_or(serde_json::Value::Null),
            "path": path,
            "snippet": source.get("snippet").cloned().unwrap_or(serde_json::Value::Null),
            "location": {
                "uri": path,
                "range": lsp_range_for_source(&source_text, start, end),
            },
        },
        "project_graph": project_graph,
        "production": production,
    }))
}

pub(crate) fn editor_reveal_focus_json(
    origin: &serde_json::Value,
    project_graph: &serde_json::Value,
    origin_id: &str,
) -> serde_json::Value {
    let origin_kind = origin
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let panel = match origin_kind {
        "route" => "routes",
        "struct" | "enum" | "type_alias" => "schema",
        "define" | "domain" => "domains",
        _ => "source",
    };
    serde_json::json!({
        "origin_id": origin_id,
        "panel": panel,
        "node_id": project_graph.get("id").cloned().unwrap_or(serde_json::Value::Null),
    })
}

pub(crate) fn editor_trace_json(dir: &Path, trace: &Path) -> anyhow::Result<serde_json::Value> {
    let trace_value = read_json_value(trace)?;
    let trace_path = trace.display().to_string();
    let live_refresh = editor_trace_live_refresh_json(dir, trace)?;
    editor_trace_payload_json(dir, &trace_path, &trace_value, &live_refresh)
}

pub(crate) fn editor_trace_payload_json(
    dir: &Path,
    trace_path: &str,
    trace_value: &serde_json::Value,
    live_refresh: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let frames = trace_value
        .get("frames")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("trace JSON must contain frames array"))?;
    let mut editor_frames = Vec::with_capacity(frames.len());
    let mut status_counts = EditorTraceStatusCounts::default();
    for (index, frame) in frames.iter().enumerate() {
        let origin_id = editor_trace_frame_origin_id(frame);
        let response_origin_id = editor_trace_frame_response_origin_id(frame);
        let navigation = match origin_id {
            Some(origin_id) => editor_reveal_json(dir, origin_id)?,
            None => serde_json::Value::Null,
        };
        let response_navigation = match response_origin_id {
            Some(origin_id) => editor_reveal_json(dir, origin_id)?,
            None => serde_json::Value::Null,
        };
        let request = editor_trace_request_json(frame);
        let summary = editor_trace_summary_json(&request, origin_id, response_origin_id);
        status_counts.record(request.get("status").and_then(serde_json::Value::as_u64));
        editor_frames.push(serde_json::json!({
            "index": index,
            "origin_id": origin_id,
            "response_origin_id": response_origin_id,
            "request": request,
            "summary": summary,
            "navigation": navigation,
            "response_navigation": response_navigation,
        }));
    }
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.trace",
        "build_dir": dir.display().to_string(),
        "trace": {
            "path": trace_path,
            "kind": trace_value.get("kind").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
            "frame_count": editor_frames.len(),
            "status_counts": editor_trace_status_counts_json(&status_counts),
        },
        "live_refresh": live_refresh,
        "stream_runner": editor_trace_stream_runner_json(dir, live_refresh),
        "frames": editor_frames,
    }))
}

pub(crate) fn editor_trace_stream_runner_json(
    dir: &Path,
    live_refresh: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.native_host.trace_stream_runner",
        "event_stream": EDITOR_TRACE_STREAM_EVENTS_PATH,
        "command": [
            "orv",
            "editor",
            "trace-stream",
            dir.display().to_string(),
            "--events",
            EDITOR_TRACE_STREAM_EVENTS_PATH,
        ],
        "transport": live_refresh
            .get("transport")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    })
}

pub(crate) fn editor_trace_stream_json(
    dir: &Path,
    events: &Path,
) -> anyhow::Result<serde_json::Value> {
    let bytes = std::fs::read(events)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", events.display()))?;
    let content_hash = format!("fnv1a64:{:016x}", fnv1a64(&bytes));
    let body = String::from_utf8(bytes)
        .map_err(|e| anyhow::anyhow!("event stream {} must be UTF-8: {e}", events.display()))?;
    let parsed_events = parse_editor_event_source_events(&body);
    let mut trace_events = Vec::new();
    let mut trace_frame_events = Vec::new();
    for (index, event) in parsed_events.iter().enumerate() {
        match event.event.as_str() {
            "orv:trace" => {
                let trace_value: serde_json::Value =
                    serde_json::from_str(&event.data).map_err(|e| {
                        anyhow::anyhow!("failed to parse trace event {index} data as JSON: {e}")
                    })?;
                let trace_path = format!("{}#event:{index}", events.display());
                let live_refresh =
                    editor_trace_stream_live_refresh_json(dir, events, &content_hash)?;
                let trace =
                    editor_trace_payload_json(dir, &trace_path, &trace_value, &live_refresh)?;
                trace_events.push(serde_json::json!({
                    "index": index,
                    "event": event.event,
                    "data_bytes": event.data.len(),
                    "trace": trace,
                }));
            }
            "orv:trace.frame" => {
                let frame_value: serde_json::Value =
                    serde_json::from_str(&event.data).map_err(|e| {
                        anyhow::anyhow!(
                            "failed to parse trace frame event {index} data as JSON: {e}"
                        )
                    })?;
                let frame = frame_value
                    .get("frame")
                    .cloned()
                    .unwrap_or_else(|| frame_value.clone());
                trace_frame_events.push(serde_json::json!({
                    "index": index,
                    "event": event.event,
                    "data_bytes": event.data.len(),
                    "frame": frame,
                }));
            }
            _ => {}
        }
    }
    let latest = if let Some(trace) = trace_events.last().and_then(|event| event.get("trace")) {
        trace.clone()
    } else if trace_frame_events.is_empty() {
        serde_json::Value::Null
    } else {
        let frames = trace_frame_events
            .iter()
            .filter_map(|event| event.get("frame").cloned())
            .collect::<Vec<_>>();
        let trace_value = serde_json::json!({
            "schema_version": 1,
            "kind": "orv.production.trace",
            "frame_count": frames.len(),
            "frames": frames,
        });
        let trace_path = format!("{}#frames", events.display());
        let live_refresh = editor_trace_stream_live_refresh_json(dir, events, &content_hash)?;
        editor_trace_payload_json(dir, &trace_path, &trace_value, &live_refresh)?
    };
    let mut event_values = Vec::with_capacity(trace_events.len() + trace_frame_events.len());
    event_values.extend(trace_events);
    event_values.extend(trace_frame_events);
    event_values.sort_by_key(|event| {
        event
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(u64::MAX)
    });
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.trace.stream",
        "build_dir": dir.display().to_string(),
        "event_stream": {
            "path": events.display().to_string(),
            "content_type": "text/event-stream",
            "content_hash": content_hash,
            "event_count": parsed_events.len(),
            "trace_event_count": event_values.iter().filter(|event| event["event"] == "orv:trace").count(),
            "trace_frame_event_count": event_values.iter().filter(|event| event["event"] == "orv:trace.frame").count(),
        },
        "latest": latest,
        "events": event_values,
    }))
}

pub(crate) fn editor_trace_stream_live_refresh_json(
    dir: &Path,
    events: &Path,
    content_hash: &str,
) -> anyhow::Result<serde_json::Value> {
    let mut refresh = serde_json::json!({
        "strategy": "event-source-snapshot",
        "watch": {
            "event_stream": {
                "path": events.display().to_string(),
                "content_hash": content_hash,
            },
        },
    });
    if let Some(transport) = editor_trace_live_transport_json(dir)? {
        refresh["transport"] = transport;
    }
    Ok(refresh)
}

pub(crate) struct EditorEventSourceEvent {
    pub(crate) event: String,
    pub(crate) data: String,
}

pub(crate) fn parse_editor_event_source_events(body: &str) -> Vec<EditorEventSourceEvent> {
    let mut events = Vec::new();
    let mut event = String::from("message");
    let mut data_lines = Vec::new();
    for line in body.lines() {
        if line.is_empty() {
            flush_editor_event_source_event(&mut events, &mut event, &mut data_lines);
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').map_or((line, ""), |(field, value)| {
            (field, value.strip_prefix(' ').unwrap_or(value))
        });
        match field {
            "event" => event = value.to_string(),
            "data" => data_lines.push(value.to_string()),
            _ => {}
        }
    }
    flush_editor_event_source_event(&mut events, &mut event, &mut data_lines);
    events
}

pub(crate) fn flush_editor_event_source_event(
    events: &mut Vec<EditorEventSourceEvent>,
    event: &mut String,
    data_lines: &mut Vec<String>,
) {
    if !data_lines.is_empty() {
        events.push(EditorEventSourceEvent {
            event: event.clone(),
            data: data_lines.join("\n"),
        });
        data_lines.clear();
    }
    *event = String::from("message");
}

pub(crate) fn editor_trace_live_refresh_json(
    dir: &Path,
    trace: &Path,
) -> anyhow::Result<serde_json::Value> {
    let bytes = std::fs::read(trace)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", trace.display()))?;
    let mut refresh = serde_json::json!({
        "strategy": "trace-file-hash",
        "watch": {
            "trace": {
                "path": trace.display().to_string(),
                "content_hash": format!("fnv1a64:{:016x}", fnv1a64(&bytes)),
            },
        },
    });
    if let Some(transport) = editor_trace_live_transport_json(dir)? {
        refresh["transport"] = transport;
    }
    Ok(refresh)
}

pub(crate) fn editor_trace_live_transport_json(
    dir: &Path,
) -> anyhow::Result<Option<serde_json::Value>> {
    let path = dir.join("server").join("app.orv-runtime.json");
    if !path.is_file() {
        return Ok(None);
    }
    let artifact = read_server_artifact(&path)?;
    let Some(listen) = artifact.listen.as_ref() else {
        return Ok(None);
    };
    if listen.port == Some(0) {
        return Ok(None);
    }
    Ok(Some(serde_json::json!({
        "kind": "event-source",
        "event": "orv:trace",
        "url": deploy_runbook_trace_events_url(Some(listen)),
    })))
}

#[derive(Default)]
pub(crate) struct EditorTraceStatusCounts {
    pub(crate) total: usize,
    pub(crate) ok: usize,
    pub(crate) redirect: usize,
    pub(crate) client_error: usize,
    pub(crate) server_error: usize,
    pub(crate) other: usize,
}

impl EditorTraceStatusCounts {
    fn record(&mut self, status: Option<u64>) {
        self.total += 1;
        match editor_trace_status_class(status) {
            "ok" => self.ok += 1,
            "redirect" => self.redirect += 1,
            "client_error" => self.client_error += 1,
            "server_error" => self.server_error += 1,
            _ => self.other += 1,
        }
    }
}

pub(crate) fn editor_trace_status_counts_json(
    counts: &EditorTraceStatusCounts,
) -> serde_json::Value {
    serde_json::json!({
        "total": counts.total,
        "ok": counts.ok,
        "redirect": counts.redirect,
        "client_error": counts.client_error,
        "server_error": counts.server_error,
        "other": counts.other,
    })
}

pub(crate) fn editor_trace_summary_json(
    request: &serde_json::Value,
    origin_id: Option<&str>,
    response_origin_id: Option<&str>,
) -> serde_json::Value {
    let method = request
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let path = request
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let status = request.get("status").and_then(serde_json::Value::as_u64);
    serde_json::json!({
        "label": editor_trace_request_label(method, path, status),
        "route": editor_trace_route_label(request),
        "status": status,
        "status_class": editor_trace_status_class(status),
        "origin_id": origin_id,
        "response_origin_id": response_origin_id,
    })
}

pub(crate) fn editor_trace_request_label(method: &str, path: &str, status: Option<u64>) -> String {
    let request = match (method.is_empty(), path.is_empty()) {
        (true, true) => "request".to_string(),
        (true, false) => path.to_string(),
        (false, true) => method.to_string(),
        (false, false) => format!("{method} {path}"),
    };
    if let Some(status) = status {
        format!("{request} -> {status}")
    } else {
        request
    }
}

pub(crate) fn editor_trace_route_label(request: &serde_json::Value) -> Option<String> {
    let method = request
        .get("route_method")
        .and_then(serde_json::Value::as_str)
        .filter(|method| !method.is_empty());
    let path = request
        .get("route_path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.is_empty());
    match (method, path) {
        (Some(method), Some(path)) => Some(format!("{method} {path}")),
        (Some(method), None) => Some(method.to_string()),
        (None, Some(path)) => Some(path.to_string()),
        (None, None) => None,
    }
}

pub(crate) const fn editor_trace_status_class(status: Option<u64>) -> &'static str {
    match status {
        Some(200..=299) => "ok",
        Some(300..=399) => "redirect",
        Some(400..=499) => "client_error",
        Some(500..=599) => "server_error",
        _ => "other",
    }
}

pub(crate) fn editor_trace_frame_origin_id(frame: &serde_json::Value) -> Option<&str> {
    frame
        .get("route_origin_id")
        .or_else(|| frame.get("origin_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|origin_id| !origin_id.is_empty())
}

pub(crate) fn editor_trace_frame_response_origin_id(frame: &serde_json::Value) -> Option<&str> {
    frame
        .get("response_origin_id")
        .and_then(serde_json::Value::as_str)
        .filter(|origin_id| !origin_id.is_empty())
}

pub(crate) fn editor_trace_request_json(frame: &serde_json::Value) -> serde_json::Value {
    let mut request = serde_json::Map::new();
    for key in [
        "method",
        "path",
        "status",
        "route_method",
        "route_path",
        "route_origin_id",
        "response_origin_id",
        "params",
        "query",
        "body",
    ] {
        if let Some(value) = frame.get(key) {
            request.insert(key.to_string(), value.clone());
        }
    }
    serde_json::Value::Object(request)
}

pub(crate) fn editor_runtime_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    let loaded = orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let resolved = orv_resolve::resolve(&loaded.program);
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    let diagnostic_count =
        loaded.diagnostics.len() + resolved.diagnostics.len() + lowered.diagnostics.len();
    let sources = editor_dap_sources(&loaded.files);
    let (runtime, frames, _live, long_running) =
        dap_launch_runtime_state(&lowered, diagnostic_count, &loaded.files, &sources, false);
    let async_runtime = dap_async_runtime_state(&lowered.program, long_running);
    Ok(serde_json::json!({
        "schema_version": 1,
        "entry": {
            "path": path.display().to_string(),
            "uri": lsp_file_uri_for_path(path),
        },
        "runtime": dap_runtime_json(&runtime, async_runtime.as_ref()),
        "frames": editor_runtime_frames_json(&frames),
        "panels": {
            "runtime": editor_runtime_panel_json(&runtime, async_runtime.as_ref(), &frames),
        },
    }))
}

pub(crate) fn editor_dap_sources(files: &[SourceFile]) -> Vec<DapSourceInfo> {
    files
        .iter()
        .enumerate()
        .map(|(index, file)| dap_source_info(file, u64::try_from(index + 1).unwrap_or(u64::MAX)))
        .collect()
}

pub(crate) fn editor_runtime_panel_json(
    runtime: &DapRuntimeState,
    async_runtime: Option<&DapAsyncRuntimeState>,
    frames: &[DapFrameState],
) -> serde_json::Value {
    serde_json::json!({
        "status": runtime.status,
        "stdout": runtime.stdout,
        "error": runtime.error,
        "frame_count": frames.len(),
        "async": async_runtime.map(editor_async_runtime_json),
    })
}

pub(crate) fn editor_async_runtime_json(runtime: &DapAsyncRuntimeState) -> serde_json::Value {
    serde_json::json!({
        "kind": runtime.kind,
        "state": runtime.state,
        "listen": runtime.listen.as_ref().map(dap_async_listen_json),
        "route_count": runtime.routes.len(),
        "routes": runtime.routes.iter().map(dap_async_route_json).collect::<Vec<_>>(),
    })
}

pub(crate) fn editor_runtime_frames_json(frames: &[DapFrameState]) -> Vec<serde_json::Value> {
    frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            serde_json::json!({
                "index": index,
                "source": dap_source_json(&frame.source),
                "line": frame.line,
                "locals": frame.locals.iter().map(editor_runtime_variable_json).collect::<Vec<_>>(),
                "stack": frame.stack.iter().map(editor_runtime_stack_json).collect::<Vec<_>>(),
                "output": frame.output,
            })
        })
        .collect()
}

pub(crate) fn editor_runtime_variable_json(variable: &DapVariable) -> serde_json::Value {
    serde_json::json!({
        "name": variable.name,
        "value": variable.value,
        "type": variable.value_type,
        "line": variable.line,
    })
}

pub(crate) fn editor_runtime_stack_json(frame: &DapStackFrameState) -> serde_json::Value {
    serde_json::json!({
        "name": frame.name,
        "source": dap_source_json(&frame.source),
        "line": frame.line,
    })
}

pub(crate) fn editor_debug_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    let loaded = orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(serde_json::json!({
        "schema_version": 1,
        "adapter": editor_debug_adapter_json(),
        "capabilities": editor_debug_capabilities_json(),
        "session_runner": editor_debug_session_runner_json(path),
        "result_artifact": editor_debug_result_artifact_json(),
        "configurations": editor_debug_configurations_json(path),
        "source_inventory": editor_debug_source_inventory_json(&loaded.files),
        "controls": editor_debug_controls_json(),
        "breakpoint_sources": editor_debug_breakpoint_sources_json(&loaded.files),
        "function_breakpoints": editor_debug_function_breakpoints_json(&loaded),
        "data_breakpoints": editor_debug_data_breakpoints_json(&loaded),
        "exception_filters": editor_debug_exception_filters_json(),
    }))
}

pub(crate) fn editor_debug_session_json(
    path: &Path,
    controls: &[EditorDebugControl],
    breakpoints: &[EditorDebugBreakpoint],
    function_breakpoints: &[String],
    data_breakpoints: &[String],
    exception_filters: &[String],
    watch_expressions: &[String],
) -> anyhow::Result<serde_json::Value> {
    editor_debug_session_json_with_source_bundle(EditorDebugSessionInput {
        path,
        controls,
        breakpoints,
        function_breakpoints,
        data_breakpoints,
        exception_filters,
        watch_expressions,
        source_bundle_path: None,
    })
}

pub(crate) struct EditorDebugSessionInput<'a> {
    pub(crate) path: &'a Path,
    pub(crate) controls: &'a [EditorDebugControl],
    pub(crate) breakpoints: &'a [EditorDebugBreakpoint],
    pub(crate) function_breakpoints: &'a [String],
    pub(crate) data_breakpoints: &'a [String],
    pub(crate) exception_filters: &'a [String],
    pub(crate) watch_expressions: &'a [String],
    pub(crate) source_bundle_path: Option<&'a Path>,
}

pub(crate) fn editor_debug_session_json_with_source_bundle(
    input: EditorDebugSessionInput<'_>,
) -> anyhow::Result<serde_json::Value> {
    let EditorDebugSessionInput {
        path,
        controls,
        breakpoints,
        function_breakpoints,
        data_breakpoints,
        exception_filters,
        watch_expressions,
        source_bundle_path,
    } = input;
    let loaded = if let Some(source_bundle_path) = source_bundle_path {
        let source_bundle = read_source_bundle_artifact(source_bundle_path)?;
        load_project_from_source_bundle_artifact(&source_bundle)?
    } else {
        orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?
    };
    let sources = editor_dap_sources(&loaded.files);
    let controls = if controls.is_empty() {
        vec![EditorDebugControl::Next]
    } else {
        controls.to_vec()
    };
    let mut requests = vec![serde_json::json!({
        "seq": 1,
        "type": "request",
        "command": "initialize",
        "arguments": {},
    })];
    let mut next_seq = 2_u64;
    let exception_filter_requests = editor_debug_push_exception_filter_requests(
        &mut requests,
        &mut next_seq,
        exception_filters,
    );
    let function_breakpoint_requests = editor_debug_push_function_breakpoint_requests(
        &mut requests,
        &mut next_seq,
        function_breakpoints,
    );
    let launch_seq = next_seq;
    next_seq += 1;
    let mut launch_arguments = serde_json::json!({
        "program": format!("file://{}", path.display()),
        "live": true,
    });
    if let Some(source_bundle_path) = source_bundle_path {
        launch_arguments["sourceBundle"] =
            serde_json::json!(source_bundle_path.display().to_string());
    }
    requests.push(serde_json::json!({
        "seq": launch_seq,
        "type": "request",
        "command": "launch",
        "arguments": launch_arguments,
    }));
    let loaded_sources_seq = next_seq;
    next_seq += 1;
    requests.push(editor_debug_loaded_sources_request_json(loaded_sources_seq));
    let source_requests = editor_debug_push_source_requests(&mut requests, &mut next_seq, &sources);
    let breakpoint_requests =
        editor_debug_push_breakpoint_requests(&mut requests, &mut next_seq, breakpoints);
    let (data_breakpoint_info_requests, data_breakpoint_set_request) =
        editor_debug_push_data_breakpoint_requests(&mut requests, &mut next_seq, data_breakpoints);
    let control_requests =
        editor_debug_push_control_requests(&mut requests, &mut next_seq, &controls);
    let stack_seq = next_seq;
    requests.push(serde_json::json!({
        "seq": stack_seq,
        "type": "request",
        "command": "stackTrace",
        "arguments": {
            "threadId": 1,
        },
    }));
    let scopes_seq = next_seq + 1;
    requests.push(serde_json::json!({
        "seq": scopes_seq,
        "type": "request",
        "command": "scopes",
        "arguments": {
            "frameId": 1,
        },
    }));
    let project_variables_seq = next_seq + 2;
    requests.push(serde_json::json!({
        "seq": project_variables_seq,
        "type": "request",
        "command": "variables",
        "arguments": {
            "variablesReference": 1,
        },
    }));
    let locals_variables_seq = next_seq + 3;
    requests.push(serde_json::json!({
        "seq": locals_variables_seq,
        "type": "request",
        "command": "variables",
        "arguments": {
            "variablesReference": 2,
        },
    }));
    let mut next_inspection_seq = next_seq + 4;
    let watch_expression_requests = editor_debug_push_watch_expression_requests(
        &mut requests,
        &mut next_inspection_seq,
        watch_expressions,
    );
    let input = dap_protocol_input_frames(&requests)?;
    let mut reader = std::io::Cursor::new(input.as_bytes());
    let mut writer = Vec::new();
    dap_serve_stdio_stream(&mut reader, &mut writer)?;
    let output =
        String::from_utf8(writer).map_err(|e| anyhow::anyhow!("invalid DAP output: {e}"))?;
    let frames = dap_protocol_output_frames(&output)?;
    let breakpoint_summaries = editor_debug_breakpoint_summaries(&frames, breakpoint_requests);
    let function_breakpoint_summaries =
        editor_debug_function_breakpoint_summaries(&frames, function_breakpoint_requests);
    let data_breakpoint_summaries = editor_debug_data_breakpoint_summaries(
        &frames,
        data_breakpoint_info_requests,
        data_breakpoint_set_request,
    );
    let exception_filter_summaries =
        editor_debug_exception_filter_summaries(&frames, exception_filter_requests);
    let control_summaries = editor_debug_control_summaries(&frames, control_requests);
    let watch_expression_summaries =
        editor_debug_watch_expression_summaries(&frames, watch_expression_requests);
    let launch =
        dap_response_for_request_seq(&frames, launch_seq).unwrap_or_else(|| serde_json::json!({}));
    let loaded_sources = dap_response_for_request_seq(&frames, loaded_sources_seq)
        .and_then(|response| response.get("body").cloned())
        .unwrap_or_else(|| serde_json::json!({}));
    let source_snapshot_summaries =
        editor_debug_source_snapshot_summaries(&frames, source_requests);
    let stack = dap_response_for_request_seq(&frames, stack_seq)
        .and_then(|response| response.get("body").cloned())
        .unwrap_or_else(|| serde_json::json!({}));
    let scopes = dap_response_for_request_seq(&frames, scopes_seq)
        .and_then(|response| response.get("body").cloned())
        .unwrap_or_else(|| serde_json::json!({}));
    let project_variables = dap_response_for_request_seq(&frames, project_variables_seq)
        .and_then(|response| response.pointer("/body/variables").cloned())
        .unwrap_or_else(|| serde_json::json!([]));
    let locals = dap_response_for_request_seq(&frames, locals_variables_seq)
        .and_then(|response| response.pointer("/body/variables").cloned())
        .unwrap_or_else(|| serde_json::json!([]));
    let first_control = control_summaries
        .first()
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.debug",
        "program": path.display().to_string(),
        "adapter": editor_debug_adapter_json(),
        "transport": {
            "protocol": "dap",
            "framing": "content-length",
            "request_count": requests.len(),
            "frame_count": frames.len(),
        },
        "breakpoints": breakpoint_summaries,
        "function_breakpoints": function_breakpoint_summaries,
        "data_breakpoints": data_breakpoint_summaries,
        "exception_filters": exception_filter_summaries,
        "launch": launch,
        "loaded_sources": loaded_sources,
        "source_snapshots": source_snapshot_summaries,
        "control": first_control,
        "controls": control_summaries,
        "watch_expressions": watch_expression_summaries,
        "stack": stack,
        "scopes": scopes,
        "project_variables": project_variables,
        "locals": locals,
        "frames": frames,
    }))
}

pub(crate) fn editor_debug_runner_session_json(
    state_path: &Path,
    controls: &[EditorDebugControl],
    breakpoints: &[EditorDebugBreakpoint],
    function_breakpoints: &[String],
    data_breakpoints: &[String],
    exception_filters: &[String],
    watch_expressions: &[String],
) -> anyhow::Result<serde_json::Value> {
    let runner = if state_path.is_dir() {
        editor_debug_runner_from_build_dir(state_path)?
    } else {
        let state = read_json_value(state_path)?;
        match state.get("kind").and_then(serde_json::Value::as_str) {
        Some("orv.editor.export") => state
            .pointer("/debug/session_runner")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("editor export state missing debug.session_runner"))?,
        Some("orv.editor.debug.runner") => state.clone(),
        _ => anyhow::bail!(
            "editor debug runner input must be a build dir, orv.editor.export state, or orv.editor.debug.runner artifact"
        ),
        }
    };
    if runner.get("kind").and_then(serde_json::Value::as_str) != Some("orv.editor.debug.runner") {
        anyhow::bail!("editor debug runner kind is invalid");
    }
    let program = json_str(&runner, "program", "editor debug runner")?;
    let source_bundle = runner
        .get("source_bundle")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from);
    let debug = editor_debug_session_json_with_source_bundle(EditorDebugSessionInput {
        path: Path::new(program),
        controls,
        breakpoints,
        function_breakpoints,
        data_breakpoints,
        exception_filters,
        watch_expressions,
        source_bundle_path: source_bundle.as_deref(),
    })?;
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.debug.runner.result",
        "state": state_path.display().to_string(),
        "runner": runner,
        "production_context": runner
            .get("production_context")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "debug": debug,
        "panels": editor_debug_runner_result_panels_json(&runner, &debug),
    }))
}

pub(crate) fn editor_debug_runner_from_build_dir(
    build_dir: &Path,
) -> anyhow::Result<serde_json::Value> {
    let source_bundle_path = build_dir.join(SOURCE_BUNDLE_PATH);
    let source_bundle = read_source_bundle_artifact(&source_bundle_path)?;
    let entry = source_bundle_entry_path(&source_bundle)?;
    let production = editor_production_summary_json(build_dir)?;
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.debug.runner",
        "program": entry.display().to_string(),
        "source_bundle": source_bundle_path.display().to_string(),
        "production_context": editor_debug_production_context_json(&production),
        "result": editor_debug_result_artifact_json(),
    }))
}

pub(crate) fn editor_debug_runner_result_panels_json(
    runner: &serde_json::Value,
    debug: &serde_json::Value,
) -> serde_json::Value {
    let stack_frames = debug
        .pointer("/stack/stackFrames")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let selected_frame = stack_frames
        .first()
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let stopped_events = editor_debug_event_frames(debug, "stopped");
    let output_events = editor_debug_event_frames(debug, "output");
    let events = editor_debug_all_event_frames(debug);
    let controls = debug
        .get("controls")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let breakpoints = debug
        .get("breakpoints")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let function_breakpoints = debug
        .get("function_breakpoints")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let data_breakpoints = debug
        .get("data_breakpoints")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let exception_filters = debug
        .get("exception_filters")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let watch_expressions = debug
        .get("watch_expressions")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let loaded_sources = debug
        .get("loaded_sources")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let source_snapshots = debug
        .get("source_snapshots")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let production_context = runner
        .get("production_context")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let production_summary = editor_debug_production_summary_from_context(&production_context);
    let source_bundle = editor_debug_launch_source_bundle(debug);
    let session_summary = editor_debug_session_summary_json(
        debug,
        &selected_frame,
        &events,
        &stopped_events,
        &output_events,
    );
    let source_navigation = editor_debug_source_navigation_json(&selected_frame, &stack_frames);
    serde_json::json!({
        "debug": {
            "schema_version": 1,
            "production_context": production_context,
            "production_summary": production_summary,
            "session_summary": session_summary,
            "source_bundle": source_bundle,
            "result_artifact": runner
                .get("result")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({
                    "path": EDITOR_DEBUG_SESSION_RESULT_PATH,
                    "kind": "orv.editor.debug.runner.result",
                    "media_type": "application/json",
                })),
            "selected_frame": selected_frame,
            "stack_frames": stack_frames,
            "source_navigation": source_navigation,
            "scopes": debug
                .get("scopes")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
            "project_variables": debug
                .get("project_variables")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
            "locals": debug
                .get("locals")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
            "control_count": controls.len(),
            "breakpoint_count": breakpoints.len(),
            "function_breakpoint_count": function_breakpoints.len(),
            "data_breakpoint_count": data_breakpoints.len(),
            "exception_filter_count": exception_filters.len(),
            "watch_expression_count": watch_expressions.len(),
            "loaded_source_count": json_array_count(loaded_sources.get("sources")),
            "source_snapshot_count": source_snapshots.len(),
            "controls": controls,
            "breakpoints": breakpoints,
            "function_breakpoints": function_breakpoints,
            "data_breakpoints": data_breakpoints,
            "exception_filters": exception_filters,
            "watch_expressions": watch_expressions,
            "loaded_sources": loaded_sources,
            "source_snapshots": source_snapshots,
            "event_count": events.len(),
            "stopped_event_count": stopped_events.len(),
            "output_event_count": output_events.len(),
            "events": events,
            "stopped_events": stopped_events,
            "output_events": output_events,
        },
    })
}

pub(crate) fn editor_debug_production_summary_from_context(
    production_context: &serde_json::Value,
) -> serde_json::Value {
    if production_context.is_null() {
        return serde_json::Value::Null;
    }
    production_context
        .get("summary")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}))
}

pub(crate) fn editor_debug_all_event_frames(debug: &serde_json::Value) -> Vec<serde_json::Value> {
    debug
        .get("frames")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|frame| frame.get("type").and_then(serde_json::Value::as_str) == Some("event"))
        .cloned()
        .collect()
}

pub(crate) fn editor_debug_event_frames(
    debug: &serde_json::Value,
    event_name: &str,
) -> Vec<serde_json::Value> {
    debug
        .get("frames")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|frame| frame.get("type").and_then(serde_json::Value::as_str) == Some("event"))
        .filter(|frame| frame.get("event").and_then(serde_json::Value::as_str) == Some(event_name))
        .cloned()
        .collect()
}

pub(crate) fn editor_debug_result_artifact_json() -> serde_json::Value {
    serde_json::json!({
        "path": EDITOR_DEBUG_SESSION_RESULT_PATH,
        "html_path": EDITOR_DEBUG_SESSION_RESULT_HTML_PATH,
        "kind": "orv.editor.debug.runner.result",
        "media_type": "application/json",
        "panels": ["debug"],
        "panel_contract": editor_debug_result_panel_contract_json(),
    })
}

pub(crate) fn editor_debug_result_panel_contract_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "root": "panels.debug",
        "sections": [
            {
                "name": "production_context",
                "path": "panels.debug.production_context",
                "kind": "object",
            },
            {
                "name": "production_summary",
                "path": "panels.debug.production_summary",
                "kind": "object",
            },
            {
                "name": "session_summary",
                "path": "panels.debug.session_summary",
                "kind": "object",
            },
            {
                "name": "source_bundle",
                "path": "panels.debug.source_bundle",
                "kind": "object",
            },
            {
                "name": "selected_frame",
                "path": "panels.debug.selected_frame",
                "kind": "object",
            },
            {
                "name": "stack_frames",
                "path": "panels.debug.stack_frames",
                "kind": "array",
            },
            {
                "name": "source_navigation",
                "path": "panels.debug.source_navigation",
                "kind": "object",
            },
            {
                "name": "scopes",
                "path": "panels.debug.scopes",
                "kind": "object",
            },
            {
                "name": "locals",
                "path": "panels.debug.locals",
                "kind": "array",
            },
            {
                "name": "project_variables",
                "path": "panels.debug.project_variables",
                "kind": "array",
            },
            {
                "name": "controls",
                "path": "panels.debug.controls",
                "kind": "array",
            },
            {
                "name": "breakpoints",
                "path": "panels.debug.breakpoints",
                "kind": "array",
            },
            {
                "name": "function_breakpoints",
                "path": "panels.debug.function_breakpoints",
                "kind": "array",
            },
            {
                "name": "data_breakpoints",
                "path": "panels.debug.data_breakpoints",
                "kind": "array",
            },
            {
                "name": "exception_filters",
                "path": "panels.debug.exception_filters",
                "kind": "array",
            },
            {
                "name": "watch_expressions",
                "path": "panels.debug.watch_expressions",
                "kind": "array",
            },
            {
                "name": "loaded_sources",
                "path": "panels.debug.loaded_sources",
                "kind": "object",
            },
            {
                "name": "source_snapshots",
                "path": "panels.debug.source_snapshots",
                "kind": "array",
            },
            {
                "name": "stopped_events",
                "path": "panels.debug.stopped_events",
                "kind": "array",
            },
            {
                "name": "events",
                "path": "panels.debug.events",
                "kind": "array",
            },
            {
                "name": "output_events",
                "path": "panels.debug.output_events",
                "kind": "array",
            },
        ],
    })
}

pub(crate) fn write_editor_debug_runner_result_if_configured(
    state_path: &Path,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
    let Some(result_path) = value
        .pointer("/runner/result/path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
    else {
        return Ok(());
    };
    write_json(
        &resolve_editor_debug_runner_result_path(state_path, result_path),
        value,
    )
}

pub(crate) fn write_editor_debug_runner_result_html_if_configured(
    state_path: &Path,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
    let html_path = value
        .pointer("/runner/result/html_path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .unwrap_or(EDITOR_DEBUG_SESSION_RESULT_HTML_PATH);
    let path = resolve_editor_debug_runner_result_path(state_path, html_path);
    write_text(&path, &editor_debug_runner_result_html(value)?)
}

pub(crate) fn editor_debug_runner_result_html(value: &serde_json::Value) -> anyhow::Result<String> {
    let selected_frame = value
        .pointer("/panels/debug/selected_frame")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let stack_frames = value
        .pointer("/panels/debug/stack_frames")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let stopped_events = value
        .pointer("/panels/debug/stopped_events")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let events = value
        .pointer("/panels/debug/events")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let output_events = value
        .pointer("/panels/debug/output_events")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let session_summary = value
        .pointer("/panels/debug/session_summary")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let production_context = value
        .pointer("/panels/debug/production_context")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let production_summary = value
        .pointer("/panels/debug/production_summary")
        .cloned()
        .unwrap_or_else(|| editor_debug_production_summary_from_context(&production_context));
    let source_navigation = value
        .pointer("/panels/debug/source_navigation")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let locals = value
        .pointer("/panels/debug/locals")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let scopes = value
        .pointer("/panels/debug/scopes/scopes")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let project_variables = value
        .pointer("/panels/debug/project_variables")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let controls = value
        .pointer("/panels/debug/controls")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let breakpoints = value
        .pointer("/panels/debug/breakpoints")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let function_breakpoints = value
        .pointer("/panels/debug/function_breakpoints")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let data_breakpoints = value
        .pointer("/panels/debug/data_breakpoints")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let exception_filters = value
        .pointer("/panels/debug/exception_filters")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let watch_expressions = value
        .pointer("/panels/debug/watch_expressions")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let source_snapshots = value
        .pointer("/panels/debug/source_snapshots")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let control_count = value
        .pointer("/panels/debug/control_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let breakpoint_count = value
        .pointer("/panels/debug/breakpoint_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let function_breakpoint_count = value
        .pointer("/panels/debug/function_breakpoint_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let data_breakpoint_count = value
        .pointer("/panels/debug/data_breakpoint_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let exception_filter_count = value
        .pointer("/panels/debug/exception_filter_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let loaded_source_count = value
        .pointer("/panels/debug/loaded_source_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let source_snapshot_count = value
        .pointer("/panels/debug/source_snapshot_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let production_client_target_count =
        json_usize_field(&production_summary, "client_target_count");
    let production_client_manifest_count =
        json_usize_field(&production_summary, "client_manifest_count");
    let production_native_server_target_count =
        json_usize_field(&production_summary, "native_server_target_count");
    let production_native_server_route_count =
        json_usize_field(&production_summary, "native_server_route_count");
    let production_static_target_count =
        json_usize_field(&production_summary, "static_target_count");
    let production_static_verified_count =
        json_usize_field(&production_summary, "static_verified_count");
    let production_preflight_target_count =
        json_usize_field(&production_summary, "preflight_target_count");
    let production_preflight_smoke_present_count =
        json_usize_field(&production_summary, "preflight_smoke_summary_present_count");
    let production_preflight_smoke_gap_count =
        json_usize_field(&production_summary, "preflight_smoke_summary_missing_count")
            + json_usize_field(
                &production_summary,
                "preflight_smoke_summary_missing_marker_count",
            );
    let mut html = String::new();
    html.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    html.push_str("<title>orv debug result</title>\n");
    html.push_str("<style>body{margin:0;background:#f7f8fb;color:#18202f;font:14px/1.45 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif}.shell{padding:20px;display:grid;gap:14px;grid-template-columns:repeat(2,minmax(0,1fr))}.panel{border:1px solid #d7dce5;background:#fff;border-radius:8px;padding:14px}.wide{grid-column:1/-1}h1{font-size:18px;margin:0}.metric{font-size:26px;font-weight:700}.muted{color:#687386}pre{white-space:pre-wrap;word-break:break-word;margin:0;max-height:320px;overflow:auto;background:#f1f5f9;border:1px solid #d7dce5;padding:10px}.list{list-style:none;margin:0;padding:0;display:grid;gap:6px}.list li{border-top:1px solid #d7dce5;padding-top:6px;color:#475569}@media(max-width:760px){.shell{grid-template-columns:1fr}}</style>\n");
    html.push_str("</head><body><main id=\"orv-debug-result\" class=\"shell\">\n");
    write!(
        &mut html,
        "<section class=\"panel wide\"><h1>Debug Result</h1><p class=\"muted\">DAP runner result rendered for native editor hosts.</p></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Controls</h2><div class=\"metric\">{control_count}</div><p class=\"muted\">executed controls</p></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Breakpoints</h2><div class=\"metric\">{breakpoint_count}</div><p class=\"muted\">requested breakpoints</p></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Function Breakpoints</h2><div class=\"metric\">{function_breakpoint_count}</div><p class=\"muted\">requested function breakpoints</p></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Data Breakpoints</h2><div class=\"metric\">{data_breakpoint_count}</div><p class=\"muted\">requested local data breakpoints</p></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Exception Filters</h2><div class=\"metric\">{exception_filter_count}</div><p class=\"muted\">configured exception filters</p></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Loaded Sources</h2><div class=\"metric\">{loaded_source_count}</div><p class=\"muted\">DAP loadedSources entries</p></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Source Snapshots</h2><div class=\"metric\">{source_snapshot_count}</div><p class=\"muted\">DAP source responses</p></section>"
    )?;
    html.push_str("<section class=\"panel wide\"><h2>Session Summary</h2><pre>");
    html.push_str(&html_escape_text(&editor_debug_session_summary_text(
        &session_summary,
    )));
    html.push_str("</pre></section>\n");
    if !production_context.is_null() {
        writeln!(
            &mut html,
            "<section class=\"panel wide\"><h2>Production Summary</h2><div class=\"metric\">{production_client_target_count}</div><p class=\"muted\">client targets, {production_client_manifest_count} manifests</p><div class=\"metric\">{production_native_server_target_count}</div><p class=\"muted\">native plans, {production_native_server_route_count} routes</p><div class=\"metric\">{production_static_verified_count}/{production_static_target_count}</div><p class=\"muted\">verified static pages</p><div class=\"metric\">{production_preflight_smoke_present_count}/{production_preflight_target_count}</div><p class=\"muted\">smoke summaries, {production_preflight_smoke_gap_count} gaps</p><pre>{}</pre></section>",
            html_escape_text(&serde_json::to_string_pretty(&production_summary)?),
        )?;
        html.push_str("<section class=\"panel wide\"><h2>Production Context</h2><pre>");
        html.push_str(&html_escape_text(&serde_json::to_string_pretty(
            &production_context,
        )?));
        html.push_str("</pre></section>\n");
    }
    html.push_str("<section class=\"panel\"><h2>Selected Frame</h2><pre>");
    html.push_str(&html_escape_text(&editor_debug_frame_summary(
        &selected_frame,
    )));
    html.push_str("</pre></section>\n");
    html.push_str("<section class=\"panel\"><h2>Source Navigation</h2><pre>");
    html.push_str(&html_escape_text(&editor_debug_source_navigation_summary(
        &source_navigation,
    )));
    html.push_str("</pre></section>\n");
    html.push_str("<section class=\"panel\"><h2>Stack Frames</h2><ul class=\"list\">");
    for frame in stack_frames {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_frame_summary(&frame))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Scopes</h2><ul class=\"list\">");
    for scope in scopes {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_scope_summary(&scope))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Locals</h2><ul class=\"list\">");
    for local in locals {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_variable_summary(&local))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Project Variables</h2><ul class=\"list\">");
    for variable in project_variables {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_variable_summary(&variable))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Executed Controls</h2><ul class=\"list\">");
    for control in controls {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_control_summary(&control))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Requested Breakpoints</h2><ul class=\"list\">");
    for breakpoint in breakpoints {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_breakpoint_summary(&breakpoint))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Function Breakpoints</h2><ul class=\"list\">");
    for breakpoint in function_breakpoints {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_function_breakpoint_summary(&breakpoint))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Data Breakpoints</h2><ul class=\"list\">");
    for breakpoint in data_breakpoints {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_data_breakpoint_summary(&breakpoint))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Exception Filters</h2><ul class=\"list\">");
    for filter in exception_filters {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_exception_filter_summary(&filter))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Watch Expressions</h2><ul class=\"list\">");
    for expression in watch_expressions {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_watch_expression_summary(&expression))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel wide\"><h2>Source Snapshots</h2><ul class=\"list\">");
    for snapshot in source_snapshots {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_source_snapshot_summary(&snapshot))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Stopped Events</h2><ul class=\"list\">");
    for event in stopped_events {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_event_summary(&event))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>All Events</h2><ul class=\"list\">");
    for event in events {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_event_summary(&event))
        )?;
    }
    html.push_str("</ul></section>\n");
    html.push_str("<section class=\"panel\"><h2>Output Events</h2><ul class=\"list\">");
    for event in output_events {
        write!(
            &mut html,
            "<li>{}</li>",
            html_escape_text(&editor_debug_event_summary(&event))
        )?;
    }
    html.push_str("</ul></section>\n</main></body></html>\n");
    Ok(html)
}

pub(crate) fn editor_debug_session_summary_json(
    debug: &serde_json::Value,
    selected_frame: &serde_json::Value,
    events: &[serde_json::Value],
    stopped_events: &[serde_json::Value],
    output_events: &[serde_json::Value],
) -> serde_json::Value {
    let selected_line = selected_frame
        .get("line")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let selected_frame_id = selected_frame
        .get("id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let selected_frame_name = selected_frame
        .get("name")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let selected_source = selected_frame
        .pointer("/source/path")
        .cloned()
        .or_else(|| selected_frame.pointer("/source/name").cloned())
        .unwrap_or(serde_json::Value::Null);
    let last_event = events
        .last()
        .and_then(|event| event.get("event"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let last_stopped_reason = stopped_events
        .last()
        .and_then(|event| event.pointer("/body/reason"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let source_bundle = editor_debug_launch_source_bundle(debug);
    let source_bundle_file_count = source_bundle
        .get("fileCount")
        .cloned()
        .unwrap_or_else(|| serde_json::json!(0));
    serde_json::json!({
        "schema_version": 1,
        "program": debug.get("program").cloned().unwrap_or(serde_json::Value::Null),
        "source_bundle": source_bundle,
        "source_bundle_file_count": source_bundle_file_count,
        "selected_frame_id": selected_frame_id,
        "selected_frame": selected_frame_name,
        "selected_line": selected_line,
        "selected_source": selected_source,
        "last_event": last_event,
        "last_stopped_reason": last_stopped_reason,
        "request_count": debug
            .pointer("/transport/request_count")
            .cloned()
            .unwrap_or_else(|| serde_json::json!(0)),
        "frame_count": debug
            .pointer("/transport/frame_count")
            .cloned()
            .unwrap_or_else(|| serde_json::json!(0)),
        "control_count": json_array_count(debug.get("controls")),
        "breakpoint_count": json_array_count(debug.get("breakpoints")),
        "function_breakpoint_count": json_array_count(debug.get("function_breakpoints")),
        "data_breakpoint_count": json_array_count(debug.get("data_breakpoints")),
        "exception_filter_count": json_array_count(debug.get("exception_filters")),
        "watch_expression_count": json_array_count(debug.get("watch_expressions")),
        "event_count": events.len(),
        "stopped_event_count": stopped_events.len(),
        "output_event_count": output_events.len(),
    })
}

pub(crate) fn editor_debug_launch_source_bundle(debug: &serde_json::Value) -> serde_json::Value {
    debug
        .pointer("/launch/body/sourceBundle")
        .cloned()
        .filter(|value| !value.is_null())
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn editor_debug_source_navigation_json(
    selected_frame: &serde_json::Value,
    stack_frames: &[serde_json::Value],
) -> serde_json::Value {
    let frames = stack_frames
        .iter()
        .filter_map(editor_debug_source_navigation_frame_json)
        .collect::<Vec<_>>();
    serde_json::json!({
        "schema_version": 1,
        "selected": editor_debug_source_navigation_frame_json(selected_frame)
            .unwrap_or_else(|| serde_json::json!({})),
        "frame_count": frames.len(),
        "frames": frames,
    })
}

pub(crate) fn editor_debug_source_navigation_frame_json(
    frame: &serde_json::Value,
) -> Option<serde_json::Value> {
    let source_path = frame
        .pointer("/source/path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let source_name = frame
        .pointer("/source/name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(source_path);
    if source_path.is_empty() && source_name.is_empty() {
        return None;
    }
    Some(serde_json::json!({
        "frame_id": frame.get("id").cloned().unwrap_or(serde_json::Value::Null),
        "frame_name": frame
            .get("name")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("frame")),
        "source": {
            "path": source_path,
            "name": source_name,
        },
        "line": frame.get("line").cloned().unwrap_or(serde_json::Value::Null),
        "column": frame
            .get("column")
            .cloned()
            .unwrap_or_else(|| serde_json::json!(1)),
    }))
}

pub(crate) fn editor_debug_session_summary_text(summary: &serde_json::Value) -> String {
    let selected_line = summary
        .get("selected_line")
        .and_then(serde_json::Value::as_u64)
        .map_or_else(|| "line ?".to_string(), |line| format!("line {line}"));
    let source_bundle = summary
        .get("source_bundle")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let source_bundle_line = source_bundle
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(|path| {
            format!(
                "source_bundle {} files {} hash {}",
                path,
                json_u64_field(&source_bundle, "fileCount"),
                json_str_or_empty(&source_bundle, "hash")
            )
        })
        .unwrap_or_default();
    [
        format!("program {}", json_str_or_empty(summary, "program")),
        source_bundle_line,
        format!(
            "selected {} {}",
            json_str_or_empty(summary, "selected_frame"),
            selected_line
        ),
        format!("source {}", json_str_or_empty(summary, "selected_source")),
        format!("last_event {}", json_str_or_empty(summary, "last_event")),
        format!(
            "last_stop {}",
            json_str_or_empty(summary, "last_stopped_reason")
        ),
        format!(
            "requests {} frames {}",
            json_u64_field(summary, "request_count"),
            json_u64_field(summary, "frame_count")
        ),
        format!(
            "controls {} breakpoints {} function_breakpoints {} data_breakpoints {} exception_filters {} watches {} events {} stopped {} output {}",
            json_u64_field(summary, "control_count"),
            json_u64_field(summary, "breakpoint_count"),
            json_u64_field(summary, "function_breakpoint_count"),
            json_u64_field(summary, "data_breakpoint_count"),
            json_u64_field(summary, "exception_filter_count"),
            json_u64_field(summary, "watch_expression_count"),
            json_u64_field(summary, "event_count"),
            json_u64_field(summary, "stopped_event_count"),
            json_u64_field(summary, "output_event_count")
        ),
    ]
    .into_iter()
    .filter(|line| !line.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

pub(crate) fn editor_debug_source_navigation_summary(navigation: &serde_json::Value) -> String {
    let selected = navigation
        .get("selected")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let selected_line = selected
        .get("line")
        .and_then(serde_json::Value::as_u64)
        .map_or_else(|| "line ?".to_string(), |line| format!("line {line}"));
    let selected_path = selected
        .pointer("/source/path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let mut lines = vec![
        format!("selected {}", json_str_or_empty(&selected, "frame_name")),
        format!("{selected_line} {selected_path}"),
        format!("frames {}", json_u64_field(navigation, "frame_count")),
    ];
    if let Some(frames) = navigation
        .get("frames")
        .and_then(serde_json::Value::as_array)
    {
        for frame in frames {
            let line = frame
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .map_or_else(|| "line ?".to_string(), |line| format!("line {line}"));
            let source = frame
                .pointer("/source/path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            lines.push(format!(
                "{} {line} {source}",
                json_str_or_empty(frame, "frame_name")
            ));
        }
    }
    lines
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn editor_debug_frame_summary(frame: &serde_json::Value) -> String {
    let name = frame
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("frame");
    let line = frame
        .get("line")
        .and_then(serde_json::Value::as_u64)
        .map_or_else(|| "line ?".to_string(), |line| format!("line {line}"));
    let source = frame
        .pointer("/source/path")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            frame
                .pointer("/source/name")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("");
    [name.to_string(), line, source.to_string()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn editor_debug_scope_summary(scope: &serde_json::Value) -> String {
    let name = scope
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("scope");
    let reference = scope
        .get("variablesReference")
        .and_then(serde_json::Value::as_u64)
        .map_or_else(String::new, |reference| format!("ref {reference}"));
    let source = scope
        .pointer("/source/path")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            scope
                .pointer("/source/name")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("");
    [name.to_string(), reference, source.to_string()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn editor_debug_event_summary(event: &serde_json::Value) -> String {
    let name = event
        .get("event")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("event");
    let reason = event
        .pointer("/body/reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let thread = event
        .pointer("/body/threadId")
        .and_then(serde_json::Value::as_u64)
        .map_or_else(String::new, |thread| format!("thread {thread}"));
    [name.to_string(), reason.to_string(), thread]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn editor_debug_control_summary(control: &serde_json::Value) -> String {
    let name = control
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("control");
    let command = control
        .pointer("/request/command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let success = control
        .pointer("/response/success")
        .and_then(serde_json::Value::as_bool)
        .map_or_else(String::new, |success| {
            format!("success {}", if success { "true" } else { "false" })
        });
    [name.to_string(), command.to_string(), success]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn editor_debug_breakpoint_summary(breakpoint: &serde_json::Value) -> String {
    let source = breakpoint
        .pointer("/source/path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("source");
    let lines = breakpoint
        .get("lines")
        .and_then(serde_json::Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(serde_json::Value::as_u64)
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let success = breakpoint
        .pointer("/response/success")
        .and_then(serde_json::Value::as_bool)
        .map_or_else(String::new, |success| {
            format!("success {}", if success { "true" } else { "false" })
        });
    [
        source.to_string(),
        if lines.is_empty() {
            String::new()
        } else {
            format!("lines {lines}")
        },
        success,
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

pub(crate) fn editor_debug_function_breakpoint_summary(breakpoint: &serde_json::Value) -> String {
    let names = breakpoint
        .get("names")
        .and_then(serde_json::Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
        .or_else(|| {
            breakpoint
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "function".to_string());
    let success = breakpoint
        .pointer("/response/success")
        .and_then(serde_json::Value::as_bool)
        .map_or_else(String::new, |success| {
            format!("success {}", if success { "true" } else { "false" })
        });
    [format!("functions {names}"), success]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn editor_debug_data_breakpoint_summary(breakpoint: &serde_json::Value) -> String {
    let names = breakpoint
        .get("names")
        .and_then(serde_json::Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
        .or_else(|| {
            breakpoint
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "local".to_string());
    let success = breakpoint
        .pointer("/response/success")
        .and_then(serde_json::Value::as_bool)
        .map_or_else(String::new, |success| {
            format!("success {}", if success { "true" } else { "false" })
        });
    [format!("locals {names}"), success]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn editor_debug_exception_filter_summary(filter: &serde_json::Value) -> String {
    let filters = filter
        .get("filters")
        .and_then(serde_json::Value::as_array)
        .map(|filters| {
            filters
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
        .or_else(|| {
            filter
                .get("filter")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "exception".to_string());
    let success = filter
        .pointer("/response/success")
        .and_then(serde_json::Value::as_bool)
        .map_or_else(String::new, |success| {
            format!("success {}", if success { "true" } else { "false" })
        });
    [format!("filters {filters}"), success]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn editor_debug_watch_expression_summary(expression: &serde_json::Value) -> String {
    let label = expression
        .get("expression")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("expression");
    let result = expression
        .pointer("/response/body/result")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let value_type = expression
        .pointer("/response/body/type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let success = expression
        .pointer("/response/success")
        .and_then(serde_json::Value::as_bool)
        .map_or_else(String::new, |success| {
            format!("success {}", if success { "true" } else { "false" })
        });
    [
        label.to_string(),
        result.to_string(),
        value_type.to_string(),
        success,
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

pub(crate) fn editor_debug_source_snapshot_summary(snapshot: &serde_json::Value) -> String {
    let name = snapshot
        .pointer("/source/name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("source");
    let path = snapshot
        .pointer("/source/path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let checksum = snapshot
        .pointer("/checksum/value")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let length = json_u64_field(snapshot, "content_length");
    let lines = json_u64_field(snapshot, "line_count");
    let success = snapshot
        .pointer("/response/success")
        .and_then(serde_json::Value::as_bool)
        .map_or_else(String::new, |success| {
            format!("success {}", if success { "true" } else { "false" })
        });
    [
        name.to_string(),
        path.to_string(),
        format!("bytes {length}"),
        format!("lines {lines}"),
        checksum.to_string(),
        success,
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

pub(crate) fn editor_debug_variable_summary(variable: &serde_json::Value) -> String {
    let name = variable
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("variable");
    let value = variable
        .get("value")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let value_type = variable
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    [name.to_string(), value.to_string(), value_type.to_string()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn resolve_editor_debug_runner_result_path(
    state_path: &Path,
    result_path: &str,
) -> PathBuf {
    let result_path = Path::new(result_path);
    if result_path.is_absolute() {
        return result_path.to_path_buf();
    }
    editor_debug_runner_artifact_root(state_path).join(result_path)
}

pub(crate) fn editor_debug_runner_artifact_root(state_path: &Path) -> PathBuf {
    if state_path.is_dir() {
        return state_path.to_path_buf();
    }
    let parent = state_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = state_path.file_name().and_then(|name| name.to_str());
    let parent_name = parent.file_name().and_then(|name| name.to_str());
    if file_name == Some("session-runner.json") && parent_name == Some("debug") {
        return parent.parent().unwrap_or(parent).to_path_buf();
    }
    parent.to_path_buf()
}

pub(crate) fn editor_debug_push_source_requests(
    requests: &mut Vec<serde_json::Value>,
    next_seq: &mut u64,
    sources: &[DapSourceInfo],
) -> Vec<(u64, DapSourceInfo, serde_json::Value)> {
    let mut source_requests = Vec::new();
    for source in sources {
        let seq = *next_seq;
        *next_seq += 1;
        let request = editor_debug_source_request_json(seq, source);
        requests.push(request.clone());
        source_requests.push((seq, source.clone(), request));
    }
    source_requests
}

pub(crate) fn editor_debug_push_breakpoint_requests(
    requests: &mut Vec<serde_json::Value>,
    next_seq: &mut u64,
    breakpoints: &[EditorDebugBreakpoint],
) -> Vec<(u64, PathBuf, Vec<u64>, serde_json::Value)> {
    let mut breakpoint_requests = Vec::new();
    for (source_path, lines) in editor_debug_breakpoint_request_groups(breakpoints) {
        let seq = *next_seq;
        *next_seq += 1;
        let request = editor_debug_set_breakpoints_request_json(seq, &source_path, &lines);
        requests.push(request.clone());
        breakpoint_requests.push((seq, source_path, lines, request));
    }
    breakpoint_requests
}

pub(crate) fn editor_debug_push_function_breakpoint_requests(
    requests: &mut Vec<serde_json::Value>,
    next_seq: &mut u64,
    function_breakpoints: &[String],
) -> Vec<(u64, Vec<String>, serde_json::Value)> {
    let names = editor_debug_function_breakpoint_names(function_breakpoints);
    if names.is_empty() {
        return Vec::new();
    }
    let seq = *next_seq;
    *next_seq += 1;
    let request = editor_debug_set_function_breakpoints_request_json(seq, &names);
    requests.push(request.clone());
    vec![(seq, names, request)]
}

pub(crate) fn editor_debug_push_exception_filter_requests(
    requests: &mut Vec<serde_json::Value>,
    next_seq: &mut u64,
    exception_filters: &[String],
) -> Vec<(u64, Vec<String>, serde_json::Value)> {
    let filters = editor_debug_exception_filter_names(exception_filters);
    if filters.is_empty() {
        return Vec::new();
    }
    let seq = *next_seq;
    *next_seq += 1;
    let request = editor_debug_set_exception_breakpoints_request_json(seq, &filters);
    requests.push(request.clone());
    vec![(seq, filters, request)]
}

pub(crate) fn editor_debug_push_data_breakpoint_requests(
    requests: &mut Vec<serde_json::Value>,
    next_seq: &mut u64,
    data_breakpoints: &[String],
) -> (
    Vec<EditorDebugDataBreakpointInfoRequest>,
    Option<EditorDebugDataBreakpointSetRequest>,
) {
    let names = editor_debug_data_breakpoint_names(data_breakpoints);
    if names.is_empty() {
        return (Vec::new(), None);
    }
    let mut info_requests = Vec::new();
    for name in &names {
        let seq = *next_seq;
        *next_seq += 1;
        let request = editor_debug_data_breakpoint_info_request_json(seq, name);
        requests.push(request.clone());
        info_requests.push((seq, name.clone(), request));
    }
    let seq = *next_seq;
    *next_seq += 1;
    let request = editor_debug_set_data_breakpoints_request_json(seq, &names);
    requests.push(request.clone());
    (info_requests, Some((seq, names, request)))
}

pub(crate) fn editor_debug_push_control_requests(
    requests: &mut Vec<serde_json::Value>,
    next_seq: &mut u64,
    controls: &[EditorDebugControl],
) -> Vec<(u64, EditorDebugControl, serde_json::Value)> {
    let mut control_requests = Vec::new();
    for control in controls.iter().copied() {
        let seq = *next_seq;
        *next_seq += 1;
        let control_request = control.request_json();
        let control_command = control_request
            .get("command")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("next"));
        let control_arguments = control_request
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        requests.push(serde_json::json!({
            "seq": seq,
            "type": "request",
            "command": control_command,
            "arguments": control_arguments,
        }));
        control_requests.push((seq, control, control_request));
    }
    control_requests
}

pub(crate) fn editor_debug_push_watch_expression_requests(
    requests: &mut Vec<serde_json::Value>,
    next_seq: &mut u64,
    watch_expressions: &[String],
) -> Vec<(u64, String, serde_json::Value)> {
    let mut watch_requests = Vec::new();
    for expression in watch_expressions
        .iter()
        .map(|expression| expression.trim())
        .filter(|expression| !expression.is_empty())
    {
        let seq = *next_seq;
        *next_seq += 1;
        let request = serde_json::json!({
            "seq": seq,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": expression,
                "frameId": 1,
                "context": "watch",
            },
        });
        requests.push(request.clone());
        watch_requests.push((seq, expression.to_string(), request));
    }
    watch_requests
}

pub(crate) fn editor_debug_breakpoint_summaries(
    frames: &[serde_json::Value],
    breakpoint_requests: Vec<(u64, PathBuf, Vec<u64>, serde_json::Value)>,
) -> Vec<serde_json::Value> {
    breakpoint_requests
        .into_iter()
        .map(|(seq, source_path, lines, request)| {
            serde_json::json!({
                "source": {
                    "path": source_path.display().to_string(),
                },
                "lines": lines,
                "request": request,
                "response": dap_response_for_request_seq(frames, seq)
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect()
}

pub(crate) fn editor_debug_source_snapshot_summaries(
    frames: &[serde_json::Value],
    source_requests: Vec<(u64, DapSourceInfo, serde_json::Value)>,
) -> Vec<serde_json::Value> {
    source_requests
        .into_iter()
        .map(|(seq, source, request)| {
            let response =
                dap_response_for_request_seq(frames, seq).unwrap_or(serde_json::Value::Null);
            let content = response
                .pointer("/body/content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            serde_json::json!({
                "source": dap_source_json(&source),
                "request": request,
                "response": response,
                "content_length": content.len(),
                "line_count": content.lines().count(),
                "checksum": {
                    "algorithm": "SHA256",
                    "value": source.checksum,
                },
            })
        })
        .collect()
}

pub(crate) fn editor_debug_function_breakpoint_summaries(
    frames: &[serde_json::Value],
    function_breakpoint_requests: Vec<(u64, Vec<String>, serde_json::Value)>,
) -> Vec<serde_json::Value> {
    function_breakpoint_requests
        .into_iter()
        .map(|(seq, names, request)| {
            serde_json::json!({
                "names": names,
                "request": request,
                "response": dap_response_for_request_seq(frames, seq)
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect()
}

pub(crate) fn editor_debug_data_breakpoint_summaries(
    frames: &[serde_json::Value],
    info_requests: Vec<EditorDebugDataBreakpointInfoRequest>,
    set_request: Option<EditorDebugDataBreakpointSetRequest>,
) -> Vec<serde_json::Value> {
    let Some((set_seq, names, request)) = set_request else {
        return Vec::new();
    };
    let infos = info_requests
        .into_iter()
        .map(|(seq, name, request)| {
            serde_json::json!({
                "name": name,
                "request": request,
                "response": dap_response_for_request_seq(frames, seq)
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect::<Vec<_>>();
    vec![serde_json::json!({
        "names": names,
        "infos": infos,
        "request": request,
        "response": dap_response_for_request_seq(frames, set_seq)
            .unwrap_or(serde_json::Value::Null),
    })]
}

pub(crate) fn editor_debug_exception_filter_summaries(
    frames: &[serde_json::Value],
    exception_filter_requests: Vec<(u64, Vec<String>, serde_json::Value)>,
) -> Vec<serde_json::Value> {
    exception_filter_requests
        .into_iter()
        .map(|(seq, filters, request)| {
            serde_json::json!({
                "filters": filters,
                "request": request,
                "response": dap_response_for_request_seq(frames, seq)
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect()
}

pub(crate) fn editor_debug_watch_expression_summaries(
    frames: &[serde_json::Value],
    watch_requests: Vec<(u64, String, serde_json::Value)>,
) -> Vec<serde_json::Value> {
    watch_requests
        .into_iter()
        .map(|(seq, expression, request)| {
            serde_json::json!({
                "expression": expression,
                "request": request,
                "response": dap_response_for_request_seq(frames, seq)
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect()
}

pub(crate) fn editor_debug_control_summaries(
    frames: &[serde_json::Value],
    control_requests: Vec<(u64, EditorDebugControl, serde_json::Value)>,
) -> Vec<serde_json::Value> {
    control_requests
        .into_iter()
        .map(|(seq, control, control_request)| {
            serde_json::json!({
                "name": control.label(),
                "request": control_request,
                "response": dap_response_for_request_seq(frames, seq)
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect()
}

pub(crate) fn editor_debug_adapter_json() -> serde_json::Value {
    serde_json::json!({
        "protocol": "dap",
        "command": ["orv", "dap", "serve", "--stdio"],
    })
}

pub(crate) fn editor_debug_capabilities_json() -> serde_json::Value {
    serde_json::json!({
        "supportsConfigurationDoneRequest": true,
        "supportsLoadedSourcesRequest": true,
        "supportsBreakpointLocationsRequest": true,
        "supportsConditionalBreakpoints": true,
        "supportsHitConditionalBreakpoints": true,
        "supportsFunctionBreakpoints": true,
        "supportsDataBreakpoints": true,
        "supportsExceptionInfoRequest": true,
        "supportsRestartRequest": true,
        "supportsSetVariable": true,
        "supportsSetExpression": true,
        "supportsModulesRequest": true,
        "supportsGotoTargetsRequest": true,
        "supportsStepBack": true,
        "supportsStepInTargetsRequest": true,
        "supportsRestartFrame": true,
        "supportsPauseRequest": true,
        "supportsCancelRequest": true,
        "supportsInstructionBreakpoints": true,
        "supportsDisassembleRequest": true,
        "supportsReadMemoryRequest": true,
        "supportsOrvRuntimeAttach": true,
        "supportsOrvRuntimeTracePath": true,
        "supportsOrvSourceBundleLaunch": true,
        "exceptionBreakpointFilters": [
            {
                "filter": "orv.diagnostics",
                "label": "ORV diagnostics",
                "default": true,
            },
            {
                "filter": "orv.runtime",
                "label": "ORV runtime errors",
                "default": true,
            },
        ],
    })
}

pub(crate) fn editor_debug_session_runner_json(path: &Path) -> serde_json::Value {
    let program = path.display().to_string();
    serde_json::json!({
        "kind": "orv.editor.debug.runner",
        "program": program,
        "transport": {
            "protocol": "dap",
            "framing": "content-length",
        },
        "command": editor_debug_control_runner_command(EditorDebugControl::Next),
        "result": editor_debug_result_artifact_json(),
        "session": {
            "launch": {
                "live": true,
            },
            "thread_id": 1,
            "breakpoint_argument": "--breakpoint",
            "breakpoint_format": "<path>:<line>",
            "function_breakpoint_argument": "--function-breakpoint",
            "function_breakpoint_format": "<function-name>",
            "data_breakpoint_argument": "--data-breakpoint",
            "data_breakpoint_format": "<local-name>",
            "exception_filter_argument": "--exception-filter",
            "exception_filter_format": "<orv.diagnostics|orv.runtime>",
            "watch_expression_argument": "--watch-expression",
            "watch_expression_format": "<expression>",
            "reuse_session": true,
        },
        "controls": editor_debug_session_runner_controls_json(),
    })
}

pub(crate) const fn editor_debug_control_order() -> [EditorDebugControl; 13] {
    [
        EditorDebugControl::Continue,
        EditorDebugControl::Pause,
        EditorDebugControl::ReverseContinue,
        EditorDebugControl::Next,
        EditorDebugControl::StepBack,
        EditorDebugControl::StepIn,
        EditorDebugControl::StepInTargets,
        EditorDebugControl::StepOut,
        EditorDebugControl::RestartFrame,
        EditorDebugControl::Restart,
        EditorDebugControl::Terminate,
        EditorDebugControl::TerminateThreads,
        EditorDebugControl::Disconnect,
    ]
}

pub(crate) fn editor_debug_control_runner_command(
    control: EditorDebugControl,
) -> serde_json::Value {
    serde_json::json!([
        "orv",
        "editor",
        "run-debug",
        EDITOR_DEBUG_SESSION_RUNNER_PATH,
        "--control",
        control.cli_value()
    ])
}

pub(crate) fn editor_debug_breakpoint_runner_command(
    path: &Path,
    line: u64,
    control: EditorDebugControl,
) -> serde_json::Value {
    serde_json::json!([
        "orv",
        "editor",
        "run-debug",
        EDITOR_DEBUG_SESSION_RUNNER_PATH,
        "--breakpoint",
        format!("{}:{line}", path.display()),
        "--control",
        control.cli_value()
    ])
}

pub(crate) fn editor_debug_function_breakpoint_runner_command(
    name: &str,
    control: EditorDebugControl,
) -> serde_json::Value {
    serde_json::json!([
        "orv",
        "editor",
        "run-debug",
        EDITOR_DEBUG_SESSION_RUNNER_PATH,
        "--function-breakpoint",
        name,
        "--control",
        control.cli_value()
    ])
}

pub(crate) fn editor_debug_data_breakpoint_runner_command(
    name: &str,
    control: EditorDebugControl,
) -> serde_json::Value {
    serde_json::json!([
        "orv",
        "editor",
        "run-debug",
        EDITOR_DEBUG_SESSION_RUNNER_PATH,
        "--data-breakpoint",
        name,
        "--control",
        control.cli_value()
    ])
}

pub(crate) fn editor_debug_exception_filter_runner_command(
    filter: &str,
    control: EditorDebugControl,
) -> serde_json::Value {
    serde_json::json!([
        "orv",
        "editor",
        "run-debug",
        EDITOR_DEBUG_SESSION_RUNNER_PATH,
        "--exception-filter",
        filter,
        "--control",
        control.cli_value()
    ])
}

pub(crate) fn editor_debug_breakpoint_request_groups(
    breakpoints: &[EditorDebugBreakpoint],
) -> Vec<(PathBuf, Vec<u64>)> {
    let mut grouped = BTreeMap::<PathBuf, BTreeSet<u64>>::new();
    for breakpoint in breakpoints {
        grouped
            .entry(breakpoint.path.clone())
            .or_default()
            .insert(breakpoint.line);
    }
    grouped
        .into_iter()
        .map(|(path, lines)| (path, lines.into_iter().collect()))
        .collect()
}

pub(crate) fn editor_debug_function_breakpoint_names(
    function_breakpoints: &[String],
) -> Vec<String> {
    function_breakpoints
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub(crate) fn editor_debug_data_breakpoint_names(data_breakpoints: &[String]) -> Vec<String> {
    data_breakpoints
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub(crate) fn editor_debug_exception_filter_names(exception_filters: &[String]) -> Vec<String> {
    exception_filters
        .iter()
        .map(|filter| filter.trim())
        .filter(|filter| matches!(*filter, "orv.diagnostics" | "orv.runtime"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub(crate) fn editor_debug_set_breakpoints_request_json(
    seq: u64,
    path: &Path,
    lines: &[u64],
) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": "setBreakpoints",
        "arguments": {
            "source": {
                "path": path.display().to_string(),
            },
            "breakpoints": lines
                .iter()
                .map(|line| serde_json::json!({"line": line}))
                .collect::<Vec<_>>(),
        },
    })
}

pub(crate) fn editor_debug_set_function_breakpoints_request_json(
    seq: u64,
    names: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": "setFunctionBreakpoints",
        "arguments": {
            "breakpoints": names
                .iter()
                .map(|name| serde_json::json!({"name": name}))
                .collect::<Vec<_>>(),
        },
    })
}

pub(crate) fn editor_debug_data_breakpoint_info_request_json(
    seq: u64,
    name: &str,
) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": "dataBreakpointInfo",
        "arguments": {
            "variablesReference": 2,
            "name": name,
        },
    })
}

pub(crate) fn editor_debug_set_data_breakpoints_request_json(
    seq: u64,
    names: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": "setDataBreakpoints",
        "arguments": {
            "breakpoints": names
                .iter()
                .map(|name| {
                    serde_json::json!({
                        "dataId": format!("local:{name}"),
                        "accessType": "write",
                    })
                })
                .collect::<Vec<_>>(),
        },
    })
}

pub(crate) fn editor_debug_set_exception_breakpoints_request_json(
    seq: u64,
    filters: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": "setExceptionBreakpoints",
        "arguments": {
            "filters": filters,
        },
    })
}

pub(crate) fn editor_debug_session_runner_controls_json() -> Vec<serde_json::Value> {
    editor_debug_control_order()
        .into_iter()
        .map(|control| {
            serde_json::json!({
                "name": control.label(),
                "value": control.cli_value(),
                "command": editor_debug_control_runner_command(control),
                "request": control.request_json(),
            })
        })
        .collect()
}

pub(crate) fn editor_debug_configurations_json(path: &Path) -> Vec<serde_json::Value> {
    let program = path.display().to_string();
    vec![
        serde_json::json!({
            "name": "Launch ORV",
            "type": "orv",
            "request": "launch",
            "program": program.clone(),
        }),
        serde_json::json!({
            "name": "Live Launch ORV",
            "type": "orv",
            "request": "launch",
            "program": program.clone(),
            "live": true,
        }),
        serde_json::json!({
            "name": "Attach ORV Runtime",
            "type": "orv",
            "request": "attach",
            "program": program,
            "attachRuntimeMode": "inProcess",
        }),
    ]
}

pub(crate) fn editor_debug_controls_json() -> Vec<serde_json::Value> {
    editor_debug_control_order()
        .into_iter()
        .map(|control| {
            serde_json::json!({
                "name": control.label(),
                "request": control.request_json(),
                "runner_command": editor_debug_control_runner_command(control),
            })
        })
        .collect()
}

pub(crate) fn editor_debug_source_inventory_json(files: &[SourceFile]) -> serde_json::Value {
    let sources = editor_dap_sources(files);
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.debug.source_inventory",
        "protocol": "dap",
        "source_count": sources.len(),
        "loaded_sources_request": editor_debug_loaded_sources_request_json(0),
        "sources": sources
            .iter()
            .map(editor_debug_source_inventory_entry_json)
            .collect::<Vec<_>>(),
    })
}

pub(crate) fn editor_debug_source_inventory_entry_json(
    source: &DapSourceInfo,
) -> serde_json::Value {
    serde_json::json!({
        "source": dap_source_json(source),
        "source_reference": source.reference,
        "path": source.path.display().to_string(),
        "uri": source.uri,
        "checksum": {
            "algorithm": "SHA256",
            "value": source.checksum,
        },
        "request": editor_debug_source_request_json(0, source),
    })
}

pub(crate) fn editor_debug_loaded_sources_request_json(seq: u64) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": "loadedSources",
        "arguments": {},
    })
}

pub(crate) fn editor_debug_source_request_json(
    seq: u64,
    source: &DapSourceInfo,
) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": "source",
        "arguments": {
            "sourceReference": source.reference,
            "source": dap_source_json(source),
        },
    })
}

pub(crate) fn editor_debug_breakpoint_sources_json(files: &[SourceFile]) -> Vec<serde_json::Value> {
    files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let source = dap_source_info(file, u64::try_from(index + 1).unwrap_or(u64::MAX));
            let lines = dap_verified_breakpoint_lines(&file.path).unwrap_or_default();
            let breakpoints = lines
                .iter()
                .map(|line| {
                    serde_json::json!({
                        "line": line,
                        "request": editor_debug_set_breakpoints_request_json(0, &file.path, &[*line]),
                        "runner_command": editor_debug_breakpoint_runner_command(
                            &file.path,
                            *line,
                            EditorDebugControl::Continue,
                        ),
                    })
                })
                .collect::<Vec<_>>();
            serde_json::json!({
                "source": dap_source_json(&source),
                "line_count": lines.len(),
                "lines": lines,
                "breakpoints": breakpoints,
            })
        })
        .collect()
}

pub(crate) fn editor_debug_function_breakpoints_json(
    loaded: &orv_project::LoadedProject,
) -> Vec<serde_json::Value> {
    loaded
        .graph
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind,
                ProjectNodeKind::Function | ProjectNodeKind::Define
            )
        })
        .map(|node| {
            let line = dap_span_line(node.span, &loaded.files).unwrap_or(0);
            let names = vec![node.name.clone()];
            serde_json::json!({
                "name": &node.name,
                "kind": match node.kind {
                    ProjectNodeKind::Define => "define",
                    _ => "function",
                },
                "source": {
                    "path": loaded
                        .files
                        .iter()
                        .find(|file| file.id == node.file)
                        .map(|file| file.path.display().to_string())
                        .unwrap_or_default(),
                    "line": line,
                },
                "request": editor_debug_set_function_breakpoints_request_json(0, &names),
                "runner_command": editor_debug_function_breakpoint_runner_command(
                    &node.name,
                    EditorDebugControl::Continue,
                ),
            })
        })
        .collect()
}

pub(crate) fn editor_debug_data_breakpoints_json(
    loaded: &orv_project::LoadedProject,
) -> Vec<serde_json::Value> {
    let resolved = orv_resolve::resolve(&loaded.program);
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    let diagnostic_count =
        loaded.diagnostics.len() + resolved.diagnostics.len() + lowered.diagnostics.len();
    let sources = editor_dap_sources(&loaded.files);
    let (_runtime, frames, _live, _long_running) =
        dap_launch_runtime_state(&lowered, diagnostic_count, &loaded.files, &sources, false);
    let mut locals = BTreeMap::new();
    for frame in frames {
        for local in frame.locals {
            locals
                .entry(local.name.clone())
                .or_insert_with(|| (local, frame.source.clone()));
        }
    }
    locals
        .into_iter()
        .map(|(name, (local, source))| {
            let names = vec![name.clone()];
            let mut source_json = dap_source_json(&source);
            if let Some(source_object) = source_json.as_object_mut() {
                source_object.insert("line".to_string(), serde_json::json!(local.line));
            }
            serde_json::json!({
                "name": name,
                "data_id": format!("local:{}", local.name),
                "value": local.value,
                "type": local.value_type,
                "source": source_json,
                "info_request": editor_debug_data_breakpoint_info_request_json(0, &local.name),
                "request": editor_debug_set_data_breakpoints_request_json(0, &names),
                "runner_command": editor_debug_data_breakpoint_runner_command(
                    &local.name,
                    EditorDebugControl::Continue,
                ),
            })
        })
        .collect()
}

pub(crate) fn editor_debug_exception_filters_json() -> Vec<serde_json::Value> {
    [
        ("orv.diagnostics", "ORV diagnostics"),
        ("orv.runtime", "ORV runtime errors"),
    ]
    .into_iter()
    .map(|(filter, label)| {
        let filters = vec![filter.to_string()];
        serde_json::json!({
            "filter": filter,
            "label": label,
            "default": true,
            "request": editor_debug_set_exception_breakpoints_request_json(0, &filters),
            "runner_command": editor_debug_exception_filter_runner_command(
                filter,
                EditorDebugControl::Continue,
            ),
        })
    })
    .collect()
}

pub(crate) fn editor_export_state_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.export",
        "snapshot": editor_snapshot_json(path)?,
        "runtime": editor_runtime_json(path)?,
        "debug": editor_debug_json(path)?,
    }))
}

pub(crate) fn editor_export_state_json_with_trace(
    path: &Path,
    build: Option<&Path>,
    trace: Option<&Path>,
) -> anyhow::Result<serde_json::Value> {
    let mut state = editor_export_state_json(path)?;
    if let Some(build) = build {
        state
            .as_object_mut()
            .expect("editor export state is object")
            .insert(
                "production".to_string(),
                editor_production_summary_json(build)?,
            );
        editor_debug_attach_production_context(&mut state);
    }
    if let Some(trace) = trace {
        let build = build.ok_or_else(|| anyhow::anyhow!("--build is required with --trace"))?;
        state
            .as_object_mut()
            .expect("editor export state is object")
            .insert("trace".to_string(), editor_trace_json(build, trace)?);
    }
    Ok(state)
}

pub(crate) fn editor_debug_attach_production_context(state: &mut serde_json::Value) {
    let production_context = editor_debug_production_context_json(
        state.get("production").unwrap_or(&serde_json::Value::Null),
    );
    if production_context.is_null() {
        return;
    }
    let Some(debug) = state
        .get_mut("debug")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    debug.insert("production_context".to_string(), production_context.clone());
    if let Some(runner) = debug
        .get_mut("session_runner")
        .and_then(serde_json::Value::as_object_mut)
    {
        if let Some(source_bundle) = production_context.get("source_bundle").cloned() {
            runner.insert("source_bundle".to_string(), source_bundle);
        }
        runner.insert("production_context".to_string(), production_context);
    }
}

pub(crate) fn editor_debug_production_context_json(
    production: &serde_json::Value,
) -> serde_json::Value {
    if production.is_null() {
        return serde_json::Value::Null;
    }
    let source_bundle = production
        .get("build_dir")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(|path| {
            Path::new(path)
                .join(SOURCE_BUNDLE_PATH)
                .display()
                .to_string()
        })
        .map_or(serde_json::Value::Null, serde_json::Value::String);
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.debug.production_context",
        "build_dir": production
            .get("build_dir")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "source_bundle": source_bundle,
        "graph_contract": production
            .get("graph_contract")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "preflight": production
            .get("preflight")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "summary": production
            .get("summary")
            .cloned()
            .unwrap_or_else(|| production_summary_json(production)),
    })
}

pub(crate) fn editor_production_summary_json(build: &Path) -> anyhow::Result<serde_json::Value> {
    let mut production = serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.production",
        "build_dir": build.display().to_string(),
        "graph_contract": editor_production_graph_contract_targets(build)?,
        "client": reveal_client_bundle_targets(build)?,
        "native_server": editor_production_native_server_targets(build)?,
        "static": editor_production_static_targets(build)?,
        "preflight": reveal_preflight_targets(build)?,
        "db_adapters": reveal_db_adapter_targets(build)?,
        "commerce_adapters": reveal_commerce_adapter_targets(build)?,
    });
    let summary = production_summary_json(&production);
    production
        .as_object_mut()
        .expect("editor production state is object")
        .insert("summary".to_string(), summary);
    Ok(production)
}

pub(crate) fn editor_production_graph_contract_targets(
    build: &Path,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let specs = [
        ("source_bundle", SOURCE_BUNDLE_PATH),
        ("project_graph", "project-graph.json"),
        ("origin_map", "origin-map.json"),
    ];
    specs
        .into_iter()
        .map(|(kind, path)| editor_production_graph_contract_target(build, kind, path))
        .collect()
}

pub(crate) fn editor_production_graph_contract_target(
    build: &Path,
    kind: &str,
    path: &str,
) -> anyhow::Result<serde_json::Value> {
    let target = build.join(path);
    let mut value = serde_json::json!({
        "kind": kind,
        "path": path,
        "exists": target.is_file(),
    });
    if !target.is_file() {
        return Ok(value);
    }
    let artifact = read_json_value(&target)?;
    value["artifact_hash"] = serde_json::json!(stable_json_hash(&artifact)?);
    match kind {
        "source_bundle" => add_editor_source_bundle_contract_fields(&artifact, &mut value),
        "project_graph" => add_editor_project_graph_contract_fields(&artifact, &mut value),
        "origin_map" => add_editor_origin_map_contract_fields(&artifact, &mut value),
        _ => {}
    }
    Ok(value)
}

pub(crate) fn add_editor_source_bundle_contract_fields(
    artifact: &serde_json::Value,
    target: &mut serde_json::Value,
) {
    target["schema_version"] = artifact
        .get("schema_version")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    target["entry"] = artifact
        .get("entry")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let files = artifact
        .get("files")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    target["file_count"] = serde_json::json!(files.len());
    target["files"] = serde_json::Value::Array(
        files
            .iter()
            .map(|file| {
                serde_json::json!({
                    "path": file.get("path").cloned().unwrap_or(serde_json::Value::Null),
                    "content_hash": file
                        .get("content_hash")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                })
            })
            .collect(),
    );
}

pub(crate) fn add_editor_project_graph_contract_fields(
    artifact: &serde_json::Value,
    target: &mut serde_json::Value,
) {
    target["schema_version"] = artifact
        .get("schema_version")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    target["stats"] = artifact
        .get("stats")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    target["node_count"] = serde_json::json!(json_array_count(artifact.get("nodes")));
    target["edge_count"] = serde_json::json!(json_array_count(artifact.get("edges")));
    target["semantic_origin_count"] = serde_json::json!(json_array_count(
        artifact.pointer("/semantic/origin_map/entries")
    ));
    target["semantic_edge_count"] =
        serde_json::json!(json_array_count(artifact.pointer("/semantic/origin_edges")));
    target["semantic_origin_link_count"] =
        serde_json::json!(json_array_count(artifact.pointer("/semantic/origin_links")));
}

pub(crate) fn add_editor_origin_map_contract_fields(
    artifact: &serde_json::Value,
    target: &mut serde_json::Value,
) {
    target["version"] = artifact
        .get("version")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    target["entry_count"] = serde_json::json!(json_array_count(artifact.get("entries")));
    target["edge_count"] = serde_json::json!(json_array_count(artifact.get("edges")));
    let call_edges = artifact
        .get("edges")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|edge| edge.get("kind").and_then(serde_json::Value::as_str) == Some("calls"))
        .count();
    target["call_edge_count"] = serde_json::json!(call_edges);
}

pub(crate) fn editor_production_native_server_targets(
    build: &Path,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let plan = read_json_value(&build.join("bundle-plan.json"))?;
    let Some(bundles) = plan.get("bundles").and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut targets = Vec::new();
    for bundle in bundles {
        if bundle.get("kind").and_then(serde_json::Value::as_str) != Some("native_server_plan") {
            continue;
        }
        let path = json_str(bundle, "path", "bundle target")?;
        let target_path = build.join(path);
        if !target_path.is_file() {
            targets.push(serde_json::json!({
                "kind": "native_server_plan",
                "path": path,
                "exists": false,
            }));
            continue;
        }
        let native_plan = read_json_value(&target_path)?;
        let routes = native_plan
            .get("routes")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        targets.push(native_server_production_target_json(
            build,
            path,
            &native_plan,
            routes,
        )?);
    }
    Ok(targets)
}

pub(crate) fn editor_production_static_targets(
    build: &Path,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let plan = read_json_value(&build.join("bundle-plan.json"))?;
    let Some(bundles) = plan.get("bundles").and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut targets = Vec::new();
    for bundle in bundles.iter().filter(|bundle| {
        bundle.get("kind").and_then(serde_json::Value::as_str) == Some("static_page")
    }) {
        let path = json_str(bundle, "path", "bundle target")?;
        let target_path = build.join(path);
        let exists = target_path.is_file();
        let verified = exists && verify_static_page_target(bundle, &target_path).is_ok();
        targets.push(serde_json::json!({
            "kind": "static_page",
            "path": path,
            "exists": exists,
            "verified": verified,
            "runtime_features": bundle
                .get("runtime_features")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        }));
    }
    Ok(targets)
}

pub(crate) fn native_server_production_target_json(
    dir: &Path,
    path: &str,
    native_plan: &serde_json::Value,
    routes: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "kind": "native_server_plan",
        "path": path,
        "exists": true,
        "status": native_plan
            .get("status")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "artifact": native_plan
            .get("artifact")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "launcher": native_plan
            .get("launcher")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "routes_source": reveal_native_server_routes_source(dir, native_plan)?,
        "router_source": reveal_native_server_router_source(dir, native_plan)?,
        "handlers_source": reveal_native_server_handlers_source(dir, native_plan)?,
        "target": native_plan
            .get("target")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "runtime_image": reveal_native_runtime_image_plan(dir, native_plan)?,
        "commands": native_plan
            .get("commands")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "runtime_features": native_plan
            .get("runtime_features")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "blocked_by": native_plan
            .get("blocked_by")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "route_count": json_array_count(Some(&routes)),
        "routes": routes,
    }))
}

pub(crate) fn editor_native_host_manifest_json(
    entry: &Path,
    state: &serde_json::Value,
) -> serde_json::Value {
    let debug = state.get("debug").unwrap_or(&serde_json::Value::Null);
    let adapter = debug.get("adapter").unwrap_or(&serde_json::Value::Null);
    let capabilities = debug
        .get("capabilities")
        .cloned()
        .unwrap_or_else(editor_debug_capabilities_json);
    let runner = debug
        .get("session_runner")
        .unwrap_or(&serde_json::Value::Null);
    let result_artifact = debug
        .get("result_artifact")
        .cloned()
        .or_else(|| runner.get("result").cloned())
        .unwrap_or_else(editor_debug_result_artifact_json);
    let controls = debug
        .get("controls")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let configurations = debug
        .get("configurations")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let configuration_count = configurations.len();
    let source_inventory = debug.get("source_inventory").cloned().unwrap_or_else(|| {
        serde_json::json!({
            "schema_version": 1,
            "kind": "orv.editor.debug.source_inventory",
            "protocol": "dap",
            "source_count": 0,
            "sources": [],
        })
    });
    let source_count = json_array_count(source_inventory.get("sources"));
    let production_context = debug
        .get("production_context")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let debug_production_context = !production_context.is_null();
    let breakpoint_count = debug
        .get("breakpoint_sources")
        .and_then(serde_json::Value::as_array)
        .map_or(0, |sources| {
            sources
                .iter()
                .map(|source| json_usize_field(source, "line_count"))
                .sum::<usize>()
        });
    let function_breakpoint_count = debug
        .get("function_breakpoints")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let data_breakpoint_count = debug
        .get("data_breakpoints")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let exception_filter_count = debug
        .get("exception_filters")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let control_commands = debug
        .get("controls")
        .and_then(serde_json::Value::as_array)
        .map(|controls| {
            controls
                .iter()
                .map(|control| {
                    serde_json::json!({
                        "name": control.get("name").cloned().unwrap_or_else(|| serde_json::json!("control")),
                        "request": control.get("request").cloned().unwrap_or_else(|| serde_json::json!({})),
                        "command": control.get("runner_command").cloned().unwrap_or_else(|| serde_json::json!([])),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let breakpoint_commands = editor_native_host_breakpoint_commands_json(debug);
    let function_breakpoint_commands = editor_native_host_function_breakpoint_commands_json(debug);
    let data_breakpoint_commands = editor_native_host_data_breakpoint_commands_json(debug);
    let exception_filter_commands = editor_native_host_exception_filter_commands_json(debug);
    let trace_enabled = state.get("trace").is_some();
    let production_enabled = state.get("production").is_some();
    let production_state = state
        .get("production")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let production_adapters = production_adapter_count(&production_state) > 0;
    let client_bundles = production_client_bundle_count(&production_state) > 0;
    let production_preflight = json_array_count(production_state.get("preflight")) > 0;
    let production_route_policies =
        production_preflight_route_policy_count_from_value(&production_state) > 0;
    let production_graph_contract = json_array_count(production_state.get("graph_contract")) > 0;
    let production = editor_native_host_production_json(&production_state);
    let runtime = editor_native_host_runtime_json(state);
    let trace = editor_native_host_trace_json(state);
    let panels =
        editor_native_host_panel_inventory_json(&result_artifact, &runtime, &production, &trace);
    let mut artifacts = serde_json::json!({
        "shell": "index.html",
        "state": "state.json",
        "debug_session_runner": EDITOR_DEBUG_SESSION_RUNNER_PATH,
        "debug_session_result": EDITOR_DEBUG_SESSION_RESULT_PATH,
        "debug_session_result_html": EDITOR_DEBUG_SESSION_RESULT_HTML_PATH,
        "runtime_panel_html": EDITOR_RUNTIME_PANEL_HTML_PATH,
    });
    if trace_enabled {
        artifacts
            .as_object_mut()
            .expect("native host artifacts is object")
            .insert(
                "trace_panel_html".to_string(),
                serde_json::json!(EDITOR_TRACE_PANEL_HTML_PATH),
            );
    }
    if production_enabled {
        artifacts
            .as_object_mut()
            .expect("native host artifacts is object")
            .insert(
                "production_panel_html".to_string(),
                serde_json::json!(EDITOR_PRODUCTION_PANEL_HTML_PATH),
            );
    }
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.native_host",
        "entry": entry.display().to_string(),
        "artifacts": artifacts,
        "debug": {
            "protocol": adapter.get("protocol").cloned().unwrap_or_else(|| serde_json::json!("dap")),
            "adapter_command": adapter.get("command").cloned().unwrap_or_else(|| serde_json::json!(["orv", "dap", "serve", "--stdio"])),
            "capabilities": capabilities,
            "runner_command": runner.get("command").cloned().unwrap_or_else(|| editor_debug_control_runner_command(EditorDebugControl::Next)),
            "configurations": configurations,
            "configuration_count": configuration_count,
            "source_inventory": source_inventory,
            "source_count": source_count,
            "production_context": production_context,
            "control_commands": control_commands,
            "breakpoint_commands": breakpoint_commands,
            "function_breakpoint_commands": function_breakpoint_commands,
            "data_breakpoint_commands": data_breakpoint_commands,
            "exception_filter_commands": exception_filter_commands,
            "panel_contract": editor_native_host_debug_panel_contract_json(),
            "control_count": controls,
            "breakpoint_argument": runner
                .pointer("/session/breakpoint_argument")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("--breakpoint")),
            "breakpoint_format": runner
                .pointer("/session/breakpoint_format")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("<path>:<line>")),
            "function_breakpoint_argument": runner
                .pointer("/session/function_breakpoint_argument")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("--function-breakpoint")),
            "function_breakpoint_format": runner
                .pointer("/session/function_breakpoint_format")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("<function-name>")),
            "data_breakpoint_argument": runner
                .pointer("/session/data_breakpoint_argument")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("--data-breakpoint")),
            "data_breakpoint_format": runner
                .pointer("/session/data_breakpoint_format")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("<local-name>")),
            "exception_filter_argument": runner
                .pointer("/session/exception_filter_argument")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("--exception-filter")),
            "exception_filter_format": runner
                .pointer("/session/exception_filter_format")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("<orv.diagnostics|orv.runtime>")),
            "watch_expression_argument": runner
                .pointer("/session/watch_expression_argument")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("--watch-expression")),
            "watch_expression_format": runner
                .pointer("/session/watch_expression_format")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("<expression>")),
            "result_path": runner
                .pointer("/result/path")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(EDITOR_DEBUG_SESSION_RESULT_PATH)),
            "result_kind": runner
                .pointer("/result/kind")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("orv.editor.debug.runner.result")),
            "result_artifact": result_artifact,
            "breakpoint_count": breakpoint_count,
            "function_breakpoint_count": function_breakpoint_count,
            "data_breakpoint_count": data_breakpoint_count,
            "exception_filter_count": exception_filter_count,
            "reuse_session": runner
                .pointer("/session/reuse_session")
                .cloned()
                .unwrap_or(serde_json::json!(true)),
        },
        "runtime": runtime,
        "production": production,
        "trace": trace,
        "panels": panels,
        "capabilities": {
            "project_graph": true,
            "runtime_inspection": true,
            "dap_controls": controls > 0,
            "dap_sources": source_count > 0,
            "dap_production_context": debug_production_context,
            "production_adapters": production_adapters,
            "production_preflight": production_preflight,
            "production_route_policies": production_route_policies,
            "production_graph_contract": production_graph_contract,
            "client_bundles": client_bundles,
            "trace_navigation": trace_enabled,
        },
    })
}

pub(crate) fn editor_native_host_panel_inventory_json(
    debug_result_artifact: &serde_json::Value,
    runtime: &serde_json::Value,
    production: &serde_json::Value,
    trace: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let mut panels = Vec::new();
    panels.push(editor_native_host_panel_inventory_entry_json(
        "debug_result",
        "Debug Result",
        "debug",
        debug_result_artifact
            .get("path")
            .and_then(serde_json::Value::as_str),
        debug_result_artifact
            .get("kind")
            .and_then(serde_json::Value::as_str),
        debug_result_artifact
            .get("media_type")
            .and_then(serde_json::Value::as_str),
        debug_result_artifact.get("panel_contract"),
    ));
    panels.push(editor_native_host_panel_inventory_entry_json(
        "runtime",
        "Runtime",
        "runtime",
        runtime
            .pointer("/panel_artifact/path")
            .and_then(serde_json::Value::as_str),
        runtime
            .pointer("/panel_artifact/kind")
            .and_then(serde_json::Value::as_str),
        runtime
            .pointer("/panel_artifact/media_type")
            .and_then(serde_json::Value::as_str),
        runtime.get("panel_contract"),
    ));
    if !production.is_null() {
        panels.push(editor_native_host_panel_inventory_entry_json(
            "production",
            "Production",
            "production",
            production
                .pointer("/panel_artifact/path")
                .and_then(serde_json::Value::as_str),
            production
                .pointer("/panel_artifact/kind")
                .and_then(serde_json::Value::as_str),
            production
                .pointer("/panel_artifact/media_type")
                .and_then(serde_json::Value::as_str),
            production.get("panel_contract"),
        ));
    }
    if !trace.is_null() {
        panels.push(editor_native_host_panel_inventory_entry_json(
            "trace",
            "Trace",
            "trace",
            trace
                .pointer("/panel_artifact/path")
                .and_then(serde_json::Value::as_str),
            trace
                .pointer("/panel_artifact/kind")
                .and_then(serde_json::Value::as_str),
            trace
                .pointer("/panel_artifact/media_type")
                .and_then(serde_json::Value::as_str),
            trace.get("panel_contract"),
        ));
    }
    panels
}

pub(crate) fn editor_native_host_panel_inventory_entry_json(
    name: &str,
    title: &str,
    root: &str,
    path: Option<&str>,
    kind: Option<&str>,
    media_type: Option<&str>,
    panel_contract: Option<&serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "title": title,
        "root": root,
        "artifact": {
            "path": path.unwrap_or(""),
            "kind": kind.unwrap_or(""),
            "media_type": media_type.unwrap_or(""),
        },
        "panel_contract": panel_contract.cloned().unwrap_or(serde_json::Value::Null),
    })
}

pub(crate) fn editor_native_host_production_json(
    production: &serde_json::Value,
) -> serde_json::Value {
    let Some(mut object) = production.as_object().cloned() else {
        return serde_json::Value::Null;
    };
    object.insert("summary".to_string(), production_summary_json(production));
    object.insert(
        "panel_contract".to_string(),
        editor_native_host_production_panel_contract_json(),
    );
    object.insert(
        "panel_html_path".to_string(),
        serde_json::json!(EDITOR_PRODUCTION_PANEL_HTML_PATH),
    );
    object.insert(
        "panel_artifact".to_string(),
        editor_production_panel_artifact_json(),
    );
    serde_json::Value::Object(object)
}

pub(crate) fn editor_production_panel_artifact_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.production.panel",
        "path": EDITOR_PRODUCTION_PANEL_HTML_PATH,
        "media_type": "text/html",
        "source": "native-host.production",
        "panel_contract": editor_native_host_production_panel_contract_json(),
    })
}

pub(crate) fn editor_native_host_production_panel_contract_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "root": "production",
        "sections": [
            {
                "name": "summary",
                "path": "production.summary",
                "kind": "object",
            },
            {
                "name": "graph_contract",
                "path": "production.graph_contract",
                "kind": "array",
            },
            {
                "name": "db_adapters",
                "path": "production.db_adapters",
                "kind": "array",
            },
            {
                "name": "preflight",
                "path": "production.preflight",
                "kind": "array",
            },
            {
                "name": "native_server",
                "path": "production.native_server",
                "kind": "array",
            },
            {
                "name": "static",
                "path": "production.static",
                "kind": "array",
            },
            {
                "name": "route_policies",
                "path": "production.summary.route_policy_kind_counts",
                "kind": "object",
            },
            {
                "name": "client",
                "path": "production.client",
                "kind": "array",
            },
            {
                "name": "commerce_adapters",
                "path": "production.commerce_adapters",
                "kind": "array",
            },
            {
                "name": "panel_artifact",
                "path": "production.panel_artifact",
                "kind": "object",
            },
        ],
    })
}

pub(crate) fn production_summary_json(production: &serde_json::Value) -> serde_json::Value {
    let db_adapters = production
        .get("db_adapters")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let commerce_adapters = production
        .get("commerce_adapters")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let preflight = production
        .get("preflight")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let routes = production
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let native_server = production
        .get("native_server")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let static_targets = production
        .get("static")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let client = production
        .get("client")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let graph_contract = production
        .get("graph_contract")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let db_adapter_count = production_adapter_entry_count(&db_adapters);
    let commerce_adapter_count = production_adapter_entry_count(&commerce_adapters);
    serde_json::json!({
        "schema_version": 1,
        "build_dir": production
            .get("build_dir")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("")),
        "graph_contract_count": graph_contract.len(),
        "source_bundle_file_count": production_graph_contract_number(
            &graph_contract,
            "source_bundle",
            "file_count",
        ),
        "project_graph_node_count": production_graph_contract_number(
            &graph_contract,
            "project_graph",
            "node_count",
        ),
        "origin_entry_count": production_graph_contract_number(
            &graph_contract,
            "origin_map",
            "entry_count",
        ),
        "client_target_count": client.len(),
        "client_manifest_count": production_client_manifest_count(&client),
        "client_capability_surface_count": production_client_capability_surface_count(&client),
        "route_target_count": routes.len(),
        "native_server_target_count": native_server.len(),
        "native_server_route_count": production_native_server_route_count(&native_server),
        "native_server_blocker_count": production_native_server_blocker_count(&native_server),
        "static_target_count": static_targets.len(),
        "static_verified_count": production_static_verified_count(&static_targets),
        "preflight_target_count": preflight.len(),
        "preflight_command_count": production_preflight_command_count(&preflight),
        "preflight_route_count": production_preflight_route_count(&preflight),
        "preflight_required_env_count": production_preflight_env_count(&preflight, "required_env"),
        "preflight_optional_env_count": production_preflight_env_count(&preflight, "optional_env"),
        "preflight_smoke_summary_present_count": production_preflight_smoke_summary_present_count(&preflight),
        "preflight_smoke_summary_missing_count": production_preflight_smoke_summary_missing_count(&preflight),
        "preflight_smoke_summary_missing_marker_count": production_preflight_smoke_summary_missing_marker_count(&preflight),
        "route_policy_count": production_preflight_route_policy_count(&preflight),
        "route_policy_kind_counts": production_preflight_route_policy_kind_counts(&preflight),
        "db_target_count": db_adapters.len(),
        "commerce_target_count": commerce_adapters.len(),
        "db_adapter_count": db_adapter_count,
        "commerce_adapter_count": commerce_adapter_count,
        "adapter_count": db_adapter_count + commerce_adapter_count,
        "missing_artifact_count": production_missing_artifact_count(&graph_contract)
            + production_missing_artifact_count(&db_adapters)
            + production_missing_artifact_count(&commerce_adapters)
            + production_missing_artifact_count(&preflight)
            + production_missing_artifact_count(&native_server)
            + production_missing_artifact_count(&static_targets)
            + production_missing_artifact_count(&client),
    })
}

pub(crate) fn production_graph_contract_number(
    targets: &[serde_json::Value],
    kind: &str,
    key: &str,
) -> usize {
    targets
        .iter()
        .find(|target| target.get("kind").and_then(serde_json::Value::as_str) == Some(kind))
        .and_then(|target| target.get(key))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0)
}

pub(crate) fn production_client_manifest_count(targets: &[serde_json::Value]) -> usize {
    targets
        .iter()
        .filter(|target| {
            target.get("kind").and_then(serde_json::Value::as_str) == Some("client_manifest")
        })
        .count()
}

pub(crate) fn production_client_capability_surface_count(targets: &[serde_json::Value]) -> usize {
    targets
        .iter()
        .find(|target| {
            target.get("kind").and_then(serde_json::Value::as_str) == Some("client_manifest")
        })
        .and_then(|target| target.pointer("/capabilities/surfaces"))
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len)
}

pub(crate) fn production_native_server_route_count(targets: &[serde_json::Value]) -> usize {
    targets
        .iter()
        .map(|target| {
            target
                .get("route_count")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or_else(|| json_array_count(target.get("routes")))
        })
        .sum()
}

pub(crate) fn production_native_server_blocker_count(targets: &[serde_json::Value]) -> usize {
    targets
        .iter()
        .map(|target| json_array_count(target.get("blocked_by")))
        .sum()
}

pub(crate) fn production_static_verified_count(targets: &[serde_json::Value]) -> usize {
    targets
        .iter()
        .filter(|target| target.get("verified").and_then(serde_json::Value::as_bool) == Some(true))
        .count()
}

pub(crate) fn production_adapter_entry_count(targets: &[serde_json::Value]) -> usize {
    targets
        .iter()
        .map(|target| json_array_count(target.get("adapters")))
        .sum()
}

pub(crate) fn production_preflight_env_count(targets: &[serde_json::Value], key: &str) -> usize {
    targets
        .iter()
        .map(|target| json_array_count(target.get(key)))
        .sum()
}

pub(crate) fn production_preflight_command_count(targets: &[serde_json::Value]) -> usize {
    targets
        .iter()
        .map(|target| json_object_count(target.get("commands")))
        .sum()
}

pub(crate) fn production_preflight_route_count(targets: &[serde_json::Value]) -> usize {
    targets
        .iter()
        .map(|target| json_array_count(target.get("routes")))
        .sum()
}

pub(crate) fn production_preflight_smoke_summary_present_count(
    targets: &[serde_json::Value],
) -> usize {
    targets
        .iter()
        .filter(|target| production_preflight_smoke_summary_present(target))
        .count()
}

pub(crate) fn production_preflight_smoke_summary_missing_count(
    targets: &[serde_json::Value],
) -> usize {
    targets
        .iter()
        .filter(|target| target.get("benchmark_evidence").is_some())
        .filter(|target| !production_preflight_smoke_summary_present(target))
        .count()
}

pub(crate) fn production_preflight_smoke_summary_missing_marker_count(
    targets: &[serde_json::Value],
) -> usize {
    targets
        .iter()
        .map(production_preflight_smoke_summary_missing_marker_count_from_target)
        .sum()
}

pub(crate) fn production_preflight_smoke_summary_present(target: &serde_json::Value) -> bool {
    target
        .pointer("/benchmark_evidence/smoke_test_summary/present")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

pub(crate) fn production_preflight_smoke_summary_missing_marker_count_from_target(
    target: &serde_json::Value,
) -> usize {
    target
        .pointer("/benchmark_evidence/smoke_test_summary/missing_markers")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len)
}

pub(crate) fn production_preflight_route_policy_count_from_value(
    production: &serde_json::Value,
) -> usize {
    production
        .get("preflight")
        .and_then(serde_json::Value::as_array)
        .map_or(0, |targets| {
            production_preflight_route_policy_count(targets)
        })
}

pub(crate) fn production_preflight_route_policy_count(targets: &[serde_json::Value]) -> usize {
    targets
        .iter()
        .flat_map(production_preflight_routes)
        .map(|route| json_array_count(route.get("policies")))
        .sum()
}

pub(crate) fn production_preflight_route_policy_kind_counts(
    targets: &[serde_json::Value],
) -> serde_json::Value {
    let mut counts = BTreeMap::new();
    for route in targets.iter().flat_map(production_preflight_routes) {
        for policy in route
            .get("policies")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(kind) = policy.get("kind").and_then(serde_json::Value::as_str) {
                *counts.entry(kind.to_string()).or_insert(0usize) += 1;
            }
        }
    }
    serde_json::to_value(counts).unwrap_or_else(|_| serde_json::json!({}))
}

pub(crate) fn production_preflight_routes(target: &serde_json::Value) -> Vec<&serde_json::Value> {
    target
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .map(|routes| routes.iter().collect())
        .unwrap_or_default()
}

pub(crate) fn production_missing_artifact_count(targets: &[serde_json::Value]) -> usize {
    targets
        .iter()
        .filter(|target| target.get("exists").and_then(serde_json::Value::as_bool) == Some(false))
        .count()
}

pub(crate) fn write_editor_production_panel_html_if_configured(
    out: &Path,
    state: &serde_json::Value,
) -> anyhow::Result<bool> {
    let Some(production) = state.get("production") else {
        return Ok(false);
    };
    let production = editor_native_host_production_json(production);
    let html = editor_production_panel_html(&production)?;
    write_text(&out.join(EDITOR_PRODUCTION_PANEL_HTML_PATH), &html)?;
    Ok(true)
}

pub(crate) fn editor_production_panel_html(
    production: &serde_json::Value,
) -> anyhow::Result<String> {
    let summary = production
        .get("summary")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let build_dir = html_escape_text(
        production
            .get("build_dir")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    );
    let db_target_count = json_usize_field(&summary, "db_target_count");
    let commerce_target_count = json_usize_field(&summary, "commerce_target_count");
    let preflight_target_count = json_usize_field(&summary, "preflight_target_count");
    let preflight_command_count = json_usize_field(&summary, "preflight_command_count");
    let preflight_smoke_summary_present_count =
        json_usize_field(&summary, "preflight_smoke_summary_present_count");
    let preflight_smoke_summary_missing_count =
        json_usize_field(&summary, "preflight_smoke_summary_missing_count");
    let preflight_smoke_summary_missing_marker_count =
        json_usize_field(&summary, "preflight_smoke_summary_missing_marker_count");
    let route_policy_count = json_usize_field(&summary, "route_policy_count");
    let native_server_target_count = json_usize_field(&summary, "native_server_target_count");
    let native_server_route_count = json_usize_field(&summary, "native_server_route_count");
    let native_server_blocker_count = json_usize_field(&summary, "native_server_blocker_count");
    let static_target_count = json_usize_field(&summary, "static_target_count");
    let static_verified_count = json_usize_field(&summary, "static_verified_count");
    let client_target_count = json_usize_field(&summary, "client_target_count");
    let graph_contract_count = json_usize_field(&summary, "graph_contract_count");
    let adapter_count = json_usize_field(&summary, "adapter_count");
    let missing_artifact_count = json_usize_field(&summary, "missing_artifact_count");
    let graph_contract = html_escape_text(&serde_json::to_string_pretty(
        production
            .get("graph_contract")
            .unwrap_or(&serde_json::Value::Null),
    )?);
    let client = html_escape_text(&serde_json::to_string_pretty(
        production.get("client").unwrap_or(&serde_json::Value::Null),
    )?);
    let native_server = html_escape_text(&serde_json::to_string_pretty(
        production
            .get("native_server")
            .unwrap_or(&serde_json::Value::Null),
    )?);
    let static_targets = html_escape_text(&serde_json::to_string_pretty(
        production.get("static").unwrap_or(&serde_json::Value::Null),
    )?);
    let db_adapters = html_escape_text(&serde_json::to_string_pretty(
        production
            .get("db_adapters")
            .unwrap_or(&serde_json::Value::Null),
    )?);
    let commerce_adapters = html_escape_text(&serde_json::to_string_pretty(
        production
            .get("commerce_adapters")
            .unwrap_or(&serde_json::Value::Null),
    )?);
    let preflight = html_escape_text(&serde_json::to_string_pretty(
        production
            .get("preflight")
            .unwrap_or(&serde_json::Value::Null),
    )?);
    let route_policies = html_escape_text(&serde_json::to_string_pretty(
        summary
            .get("route_policy_kind_counts")
            .unwrap_or(&serde_json::Value::Null),
    )?);
    let panel_contract = html_escape_text(&serde_json::to_string_pretty(
        production
            .get("panel_contract")
            .unwrap_or(&serde_json::Value::Null),
    )?);
    let production_json = html_script_json(&serde_json::to_string_pretty(production)?);
    let mut html = String::new();
    html.push_str(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>orv Production Panel</title>\n<style>\n:root{color-scheme:light dark;--bg:#f7f6f2;--fg:#151713;--muted:#6b7067;--panel:#fff;--line:#d8d9d2;--accent:#67610f;--bad:#a43737;}\n@media (prefers-color-scheme: dark){:root{--bg:#11130f;--fg:#eef0ea;--muted:#a8aea2;--panel:#191c17;--line:#30362d;--accent:#d8cc65;--bad:#ff9d9d;}}\n*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.45 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;}header{padding:24px 28px 12px;border-bottom:1px solid var(--line);}h1{font-size:24px;margin:0 0 8px}h2{font-size:13px;text-transform:uppercase;letter-spacing:.08em;color:var(--muted);margin:0 0 12px}.muted{color:var(--muted)}.summary{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:10px;margin-top:16px}.metric{border:1px solid var(--line);border-radius:6px;padding:10px;background:var(--panel)}.metric b{display:block;font-size:22px;line-height:1.1}.metric .bad{color:var(--bad)}main{display:grid;grid-template-columns:1fr 1fr;gap:16px;padding:16px 28px 28px}.panel{background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:16px}.wide{grid-column:1/-1}pre{margin:0;white-space:pre-wrap;overflow:auto;font:12px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}@media (max-width:900px){main,.summary{grid-template-columns:1fr}main{padding:14px}header{padding:18px 14px 8px}}\n</style>\n</head>\n<body>\n",
    );
    writeln!(
        &mut html,
        "<header><h1>Production Panel</h1><div class=\"muted\">{build_dir}</div><section class=\"summary\"><div class=\"metric\"><span>Graph Contracts</span><b>{graph_contract_count}</b></div><div class=\"metric\"><span>Client Targets</span><b>{client_target_count}</b></div><div class=\"metric\"><span>Native Plans</span><b>{native_server_target_count}</b></div><div class=\"metric\"><span>Native Routes</span><b>{native_server_route_count}</b></div><div class=\"metric\"><span>Native Blockers</span><b class=\"{}\">{native_server_blocker_count}</b></div><div class=\"metric\"><span>Static Pages</span><b>{static_verified_count}/{static_target_count}</b></div><div class=\"metric\"><span>Preflight</span><b>{preflight_target_count}</b></div><div class=\"metric\"><span>Preflight Commands</span><b>{preflight_command_count}</b></div><div class=\"metric\"><span>Smoke Summary</span><b>{preflight_smoke_summary_present_count}/{preflight_target_count}</b></div><div class=\"metric\"><span>Smoke Gaps</span><b class=\"{}\">{}</b></div><div class=\"metric\"><span>Route Policies</span><b>{route_policy_count}</b></div><div class=\"metric\"><span>DB Targets</span><b>{db_target_count}</b></div><div class=\"metric\"><span>Commerce Targets</span><b>{commerce_target_count}</b></div><div class=\"metric\"><span>Adapters</span><b>{adapter_count}</b></div><div class=\"metric\"><span>Missing</span><b class=\"{}\">{missing_artifact_count}</b></div></section></header>",
        if native_server_blocker_count == 0 { "" } else { "bad" },
        if preflight_smoke_summary_missing_count + preflight_smoke_summary_missing_marker_count == 0 {
            ""
        } else {
            "bad"
        },
        preflight_smoke_summary_missing_count + preflight_smoke_summary_missing_marker_count,
        if missing_artifact_count == 0 { "" } else { "bad" }
    )?;
    writeln!(
        &mut html,
        "<main><section class=\"panel wide\"><h2>Graph Contract</h2><pre>{graph_contract}</pre></section><section class=\"panel wide\"><h2>Client Bundles</h2><pre>{client}</pre></section><section class=\"panel wide\"><h2>Native Server</h2><pre>{native_server}</pre></section><section class=\"panel wide\"><h2>Static Pages</h2><pre>{static_targets}</pre></section><section class=\"panel wide\"><h2>Preflight</h2><pre>{preflight}</pre></section><section class=\"panel\"><h2>Route Policy Summary</h2><pre>{route_policies}</pre></section><section class=\"panel\"><h2>DB Adapters</h2><pre>{db_adapters}</pre></section><section class=\"panel\"><h2>Commerce Adapters</h2><pre>{commerce_adapters}</pre></section><section class=\"panel wide\"><h2>Panel Contract</h2><pre>{panel_contract}</pre></section></main>"
    )?;
    writeln!(
        &mut html,
        "<script id=\"orv-production\" type=\"application/json\">{production_json}</script>"
    )?;
    html.push_str("</body>\n</html>\n");
    Ok(html)
}

pub(crate) fn editor_native_host_runtime_json(state: &serde_json::Value) -> serde_json::Value {
    let runtime_state = state.get("runtime").unwrap_or(&serde_json::Value::Null);
    let runtime = runtime_state
        .get("runtime")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let frames = runtime_state
        .get("frames")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let panel = runtime_state
        .pointer("/panels/runtime")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    serde_json::json!({
        "schema_version": 1,
        "status": runtime
            .get("status")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("unknown")),
        "stdout": runtime
            .get("stdout")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("")),
        "error": runtime
            .get("error")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("")),
        "async": runtime
            .get("async")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "frame_count": frames.len(),
        "frames": frames,
        "panel": panel,
        "panel_html_path": EDITOR_RUNTIME_PANEL_HTML_PATH,
        "panel_artifact": editor_runtime_panel_artifact_json(),
        "panel_contract": editor_native_host_runtime_panel_contract_json(),
    })
}

pub(crate) fn editor_runtime_panel_artifact_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.runtime.panel",
        "path": EDITOR_RUNTIME_PANEL_HTML_PATH,
        "media_type": "text/html",
        "source": "native-host.runtime",
        "panel_contract": editor_native_host_runtime_panel_contract_json(),
    })
}

pub(crate) fn editor_native_host_runtime_panel_contract_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "root": "runtime",
        "sections": [
            {
                "name": "panel",
                "path": "runtime.panel",
                "kind": "object",
            },
            {
                "name": "frames",
                "path": "runtime.frames",
                "kind": "array",
            },
            {
                "name": "async",
                "path": "runtime.async",
                "kind": "object",
            },
            {
                "name": "panel_artifact",
                "path": "runtime.panel_artifact",
                "kind": "object",
            },
        ],
    })
}

pub(crate) fn editor_native_host_debug_panel_contract_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "root": "debug",
        "sections": [
            {
                "name": "adapter",
                "path": "debug.adapter_command",
                "kind": "array",
            },
            {
                "name": "capabilities",
                "path": "debug.capabilities",
                "kind": "object",
            },
            {
                "name": "configurations",
                "path": "debug.configurations",
                "kind": "array",
            },
            {
                "name": "source_inventory",
                "path": "debug.source_inventory",
                "kind": "object",
            },
            {
                "name": "production_context",
                "path": "debug.production_context",
                "kind": "object",
            },
            {
                "name": "control_commands",
                "path": "debug.control_commands",
                "kind": "array",
            },
            {
                "name": "breakpoint_commands",
                "path": "debug.breakpoint_commands",
                "kind": "array",
            },
            {
                "name": "function_breakpoint_commands",
                "path": "debug.function_breakpoint_commands",
                "kind": "array",
            },
            {
                "name": "data_breakpoint_commands",
                "path": "debug.data_breakpoint_commands",
                "kind": "array",
            },
            {
                "name": "exception_filter_commands",
                "path": "debug.exception_filter_commands",
                "kind": "array",
            },
            {
                "name": "function_breakpoint_argument",
                "path": "debug.function_breakpoint_argument",
                "kind": "string",
            },
            {
                "name": "data_breakpoint_argument",
                "path": "debug.data_breakpoint_argument",
                "kind": "string",
            },
            {
                "name": "exception_filter_argument",
                "path": "debug.exception_filter_argument",
                "kind": "string",
            },
            {
                "name": "watch_expression_argument",
                "path": "debug.watch_expression_argument",
                "kind": "string",
            },
            {
                "name": "result_artifact",
                "path": "debug.result_artifact",
                "kind": "object",
            },
        ],
    })
}

pub(crate) fn production_adapter_count(production: &serde_json::Value) -> usize {
    json_array_count(production.get("db_adapters"))
        + json_array_count(production.get("commerce_adapters"))
}

pub(crate) fn production_client_bundle_count(production: &serde_json::Value) -> usize {
    json_array_count(production.get("client"))
}

pub(crate) fn editor_native_host_breakpoint_commands_json(
    debug: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let mut commands = Vec::new();
    let Some(sources) = debug
        .get("breakpoint_sources")
        .and_then(serde_json::Value::as_array)
    else {
        return commands;
    };
    for source_group in sources {
        let source = source_group
            .get("source")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let Some(breakpoints) = source_group
            .get("breakpoints")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for breakpoint in breakpoints {
            commands.push(serde_json::json!({
                "source": source,
                "line": breakpoint.get("line").cloned().unwrap_or(serde_json::Value::Null),
                "request": breakpoint
                    .get("request")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                "command": breakpoint
                    .get("runner_command")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([])),
            }));
        }
    }
    commands
}

pub(crate) fn editor_native_host_function_breakpoint_commands_json(
    debug: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let Some(function_breakpoints) = debug
        .get("function_breakpoints")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    function_breakpoints
        .iter()
        .map(|breakpoint| {
            serde_json::json!({
                "name": breakpoint
                    .get("name")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!("")),
                "kind": breakpoint
                    .get("kind")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!("function")),
                "source": breakpoint
                    .get("source")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                "request": breakpoint
                    .get("request")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                "command": breakpoint
                    .get("runner_command")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([])),
            })
        })
        .collect()
}

pub(crate) fn editor_native_host_data_breakpoint_commands_json(
    debug: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let Some(data_breakpoints) = debug
        .get("data_breakpoints")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    data_breakpoints
        .iter()
        .map(|breakpoint| {
            serde_json::json!({
                "name": breakpoint
                    .get("name")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!("")),
                "data_id": breakpoint
                    .get("data_id")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!("")),
                "source": breakpoint
                    .get("source")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                "info_request": breakpoint
                    .get("info_request")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                "request": breakpoint
                    .get("request")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                "command": breakpoint
                    .get("runner_command")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([])),
            })
        })
        .collect()
}

pub(crate) fn editor_native_host_exception_filter_commands_json(
    debug: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let Some(exception_filters) = debug
        .get("exception_filters")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    exception_filters
        .iter()
        .map(|filter| {
            serde_json::json!({
                "filter": filter
                    .get("filter")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!("")),
                "label": filter
                    .get("label")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!("")),
                "request": filter
                    .get("request")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                "command": filter
                    .get("runner_command")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([])),
            })
        })
        .collect()
}

pub(crate) fn editor_native_host_trace_json(state: &serde_json::Value) -> serde_json::Value {
    let Some(trace) = state.get("trace") else {
        return serde_json::Value::Null;
    };
    let build_dir = trace
        .get("build_dir")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let live_refresh = trace
        .get("live_refresh")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let stream_runner = trace
        .get("stream_runner")
        .cloned()
        .unwrap_or_else(|| editor_trace_stream_runner_json(Path::new(""), &live_refresh));
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.native_host.trace",
        "build_dir": build_dir,
        "trace_path": trace.pointer("/trace/path").cloned().unwrap_or_else(|| serde_json::json!("")),
        "frame_count": trace.pointer("/trace/frame_count").cloned().unwrap_or_else(|| serde_json::json!(0)),
        "status_counts": trace.pointer("/trace/status_counts").cloned().unwrap_or_else(|| serde_json::json!({})),
        "summary": editor_native_host_trace_summary_json(trace),
        "status_filters": editor_native_host_trace_status_filters_json(trace),
        "frames": editor_native_host_trace_frames_json(trace, build_dir),
        "live_refresh": live_refresh,
        "transport": trace.pointer("/live_refresh/transport").cloned().unwrap_or(serde_json::Value::Null),
        "stream_runner": stream_runner,
        "panel_html_path": EDITOR_TRACE_PANEL_HTML_PATH,
        "panel_artifact": editor_trace_panel_artifact_json(),
        "panel_contract": editor_native_host_trace_panel_contract_json(),
    })
}

pub(crate) fn editor_trace_panel_artifact_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.editor.trace.panel",
        "path": EDITOR_TRACE_PANEL_HTML_PATH,
        "media_type": "text/html",
        "source": "native-host.trace",
        "panel_contract": editor_native_host_trace_panel_contract_json(),
    })
}

pub(crate) fn editor_native_host_trace_panel_contract_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "root": "trace",
        "sections": [
            {
                "name": "summary",
                "path": "trace.summary",
                "kind": "object",
            },
            {
                "name": "status_filters",
                "path": "trace.status_filters",
                "kind": "array",
            },
            {
                "name": "frames",
                "path": "trace.frames",
                "kind": "array",
            },
            {
                "name": "transport",
                "path": "trace.transport",
                "kind": "object",
            },
            {
                "name": "stream_runner",
                "path": "trace.stream_runner",
                "kind": "object",
            },
            {
                "name": "panel_artifact",
                "path": "trace.panel_artifact",
                "kind": "object",
            },
        ],
    })
}

pub(crate) fn editor_native_host_trace_summary_json(
    trace: &serde_json::Value,
) -> serde_json::Value {
    let frames = trace
        .get("frames")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let first_request = frames
        .first()
        .and_then(|frame| frame.get("summary"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let last_request = frames
        .last()
        .and_then(|frame| frame.get("summary"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "schema_version": 1,
        "build_dir": trace
            .get("build_dir")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("")),
        "trace_path": trace
            .pointer("/trace/path")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("")),
        "frame_count": trace
            .pointer("/trace/frame_count")
            .cloned()
            .unwrap_or_else(|| serde_json::json!(frames.len())),
        "status_counts": trace
            .pointer("/trace/status_counts")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        "first_request": first_request,
        "last_request": last_request,
    })
}

pub(crate) fn editor_native_host_trace_status_filters_json(
    trace: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let counts = trace
        .pointer("/trace/status_counts")
        .unwrap_or(&serde_json::Value::Null);
    [
        ("all", "All", "total"),
        ("ok", "OK", "ok"),
        ("redirect", "3xx", "redirect"),
        ("client_error", "4xx", "client_error"),
        ("server_error", "5xx", "server_error"),
        ("other", "Other", "other"),
    ]
    .into_iter()
    .map(|(name, label, field)| {
        serde_json::json!({
            "name": name,
            "label": label,
            "count": json_usize_field(counts, field),
        })
    })
    .collect()
}

pub(crate) fn editor_native_host_trace_frames_json(
    trace: &serde_json::Value,
    build_dir: &str,
) -> Vec<serde_json::Value> {
    trace
        .get("frames")
        .and_then(serde_json::Value::as_array)
        .map(|frames| {
            frames
                .iter()
                .map(|frame| {
                    let navigation = frame
                        .get("navigation")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let response_navigation = frame
                        .get("response_navigation")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let origin_id = frame.get("origin_id").and_then(serde_json::Value::as_str);
                    let response_origin_id = frame
                        .get("response_origin_id")
                        .and_then(serde_json::Value::as_str);
                    serde_json::json!({
                        "index": frame.get("index").cloned().unwrap_or(serde_json::Value::Null),
                        "origin_id": frame.get("origin_id").cloned().unwrap_or(serde_json::Value::Null),
                        "response_origin_id": frame.get("response_origin_id").cloned().unwrap_or(serde_json::Value::Null),
                        "request": frame.get("request").cloned().unwrap_or_else(|| serde_json::json!({})),
                        "summary": frame.get("summary").cloned().unwrap_or_else(|| serde_json::json!({})),
                        "source": navigation
                            .get("source")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "production": navigation
                            .get("production")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "response_source": response_navigation
                            .get("source")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "response_production": response_navigation
                            .get("production")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "reveal_command": editor_trace_frame_reveal_command_json(build_dir, origin_id),
                        "response_reveal_command": editor_trace_frame_reveal_command_json(build_dir, response_origin_id),
                        "navigation": navigation,
                        "response_navigation": response_navigation,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn editor_trace_frame_reveal_command_json(
    build_dir: &str,
    origin_id: Option<&str>,
) -> serde_json::Value {
    editor_reveal_command_json(build_dir, origin_id)
}

pub(crate) fn editor_reveal_command_json(
    build_dir: &str,
    origin_id: Option<&str>,
) -> serde_json::Value {
    let Some(origin_id) = origin_id else {
        return serde_json::Value::Null;
    };
    if build_dir.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::json!(["orv", "editor", "reveal", build_dir, origin_id])
}

pub(crate) fn write_editor_runtime_panel_html_if_configured(
    out: &Path,
    state: &serde_json::Value,
) -> anyhow::Result<bool> {
    if state.get("runtime").is_none() {
        return Ok(false);
    }
    let runtime = editor_native_host_runtime_json(state);
    let html = editor_runtime_panel_html(&runtime)?;
    write_text(&out.join(EDITOR_RUNTIME_PANEL_HTML_PATH), &html)?;
    Ok(true)
}

pub(crate) fn editor_runtime_panel_html(runtime: &serde_json::Value) -> anyhow::Result<String> {
    let status = html_escape_text(
        runtime
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown"),
    );
    let stdout = html_escape_text(
        runtime
            .get("stdout")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    );
    let error = html_escape_text(
        runtime
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    );
    let frame_count = json_usize_field(runtime, "frame_count");
    let async_json = html_escape_text(&serde_json::to_string_pretty(
        runtime.get("async").unwrap_or(&serde_json::Value::Null),
    )?);
    let panel_json = html_escape_text(&serde_json::to_string_pretty(
        runtime.get("panel").unwrap_or(&serde_json::Value::Null),
    )?);
    let panel_contract_json = html_escape_text(&serde_json::to_string_pretty(
        runtime
            .get("panel_contract")
            .unwrap_or(&serde_json::Value::Null),
    )?);
    let runtime_json = html_script_json(&serde_json::to_string_pretty(runtime)?);
    let mut html = String::new();
    html.push_str(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>orv Runtime Panel</title>\n<style>\n:root{color-scheme:light dark;--bg:#f8f7f3;--fg:#151713;--muted:#697067;--panel:#fff;--line:#d8d9d2;--accent:#375f94;--accent-weak:#dde8f5;--bad:#a43737;}\n@media (prefers-color-scheme: dark){:root{--bg:#11130f;--fg:#eef0ea;--muted:#a8aea2;--panel:#191c17;--line:#30362d;--accent:#8ab8f0;--accent-weak:#1e314a;--bad:#ff9d9d;}}\n*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.45 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;}header{padding:24px 28px 12px;border-bottom:1px solid var(--line);}h1{font-size:24px;margin:0 0 12px}h2{font-size:13px;text-transform:uppercase;letter-spacing:.08em;color:var(--muted);margin:0 0 12px}.summary{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px}.metric{border:1px solid var(--line);border-radius:6px;padding:10px;background:var(--panel)}.metric b{display:block;font-size:22px;line-height:1.1}.ok{color:var(--accent)}.err{color:var(--bad)}main{display:grid;grid-template-columns:minmax(280px,380px) minmax(0,1fr);gap:16px;padding:16px 28px 28px}.panel{background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:16px}.list{list-style:none;margin:0;padding:0;display:grid;gap:8px}.list li{border:1px solid var(--line);border-radius:6px;padding:10px;cursor:pointer;background:var(--bg)}.list li:focus,.list li:hover{outline:2px solid var(--accent);outline-offset:1px}pre{margin:0;white-space:pre-wrap;overflow:auto;font:12px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}.detail-grid{display:grid;grid-template-columns:1fr 1fr;gap:16px}.wide{grid-column:1/-1}@media (max-width:900px){main,.summary,.detail-grid{grid-template-columns:1fr}main{padding:14px}header{padding:18px 14px 8px}}\n</style>\n</head>\n<body>\n",
    );
    writeln!(
        &mut html,
        "<header><h1>Runtime Panel</h1><section class=\"summary\"><div class=\"metric\"><span>Status</span><b class=\"{}\">{status}</b></div><div class=\"metric\"><span>Frames</span><b>{frame_count}</b></div></section></header>",
        if status == "ok" { "ok" } else { "err" }
    )?;
    html.push_str("<main><section class=\"panel\"><h2>Frames</h2><ul id=\"runtime-frame-list\" class=\"list\"></ul></section><section class=\"panel\"><h2>Selected Frame</h2><pre id=\"runtime-frame-detail\">No runtime frame selected.</pre></section><section class=\"detail-grid\">\n");
    writeln!(
        &mut html,
        "<section class=\"panel\"><h2>Stdout</h2><pre>{stdout}</pre></section><section class=\"panel\"><h2>Error</h2><pre>{error}</pre></section><section class=\"panel\"><h2>Async Runtime</h2><pre>{async_json}</pre></section><section class=\"panel\"><h2>Runtime Panel</h2><pre>{panel_json}</pre></section><section class=\"panel wide\"><h2>Panel Contract</h2><pre>{panel_contract_json}</pre></section></section></main>"
    )?;
    writeln!(
        &mut html,
        "<script id=\"orv-runtime\" type=\"application/json\">{runtime_json}</script>"
    )?;
    html.push_str(
        "<script>\nconst runtime = JSON.parse(document.getElementById('orv-runtime').textContent);\nconst frames = Array.isArray(runtime.frames) ? runtime.frames : [];\nconst list = document.getElementById('runtime-frame-list');\nconst detail = document.getElementById('runtime-frame-detail');\nfunction frameLabel(frame){\n  const source = frame?.source || {};\n  const label = source.name || source.path || 'frame';\n  const line = frame?.line ? `:${frame.line}` : '';\n  return `#${(frame?.index ?? 0) + 1} ${label}${line}`;\n}\nfunction renderDetail(frame){\n  if (!frame) { detail.textContent = 'No runtime frame selected.'; return; }\n  const source = frame.source || {};\n  const locals = (frame.locals || []).map(local => `  ${local.name}: ${local.value}${local.type ? ` (${local.type})` : ''}`);\n  const stack = (frame.stack || []).map(call => `  ${call.name || 'frame'} ${call.source?.name || call.source?.path || ''}:${call.line || ''}`.trim());\n  const lines = [\n    frameLabel(frame),\n    source.path ? `source ${source.path}${frame.line ? `:${frame.line}` : ''}` : '',\n    frame.output ? `output ${String(frame.output).trimEnd()}` : '',\n    locals.length ? `locals\\n${locals.join('\\n')}` : '',\n    stack.length ? `stack\\n${stack.join('\\n')}` : ''\n  ].filter(Boolean);\n  detail.textContent = lines.join('\\n');\n}\nfor (const frame of frames) {\n  const row = document.createElement('li');\n  row.textContent = frameLabel(frame);\n  row.tabIndex = 0;\n  row.addEventListener('click', () => renderDetail(frame));\n  row.addEventListener('keydown', event => {\n    if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); renderDetail(frame); }\n  });\n  list.appendChild(row);\n}\nrenderDetail(frames[0]);\n</script>\n</body>\n</html>\n",
    );
    Ok(html)
}

pub(crate) fn write_editor_trace_panel_html_if_configured(
    out: &Path,
    state: &serde_json::Value,
) -> anyhow::Result<bool> {
    if state.get("trace").is_none() {
        return Ok(false);
    }
    let trace = editor_native_host_trace_json(state);
    let html = editor_trace_panel_html(&trace)?;
    write_text(&out.join(EDITOR_TRACE_PANEL_HTML_PATH), &html)?;
    Ok(true)
}

pub(crate) fn editor_trace_panel_html(trace: &serde_json::Value) -> anyhow::Result<String> {
    let summary = trace
        .get("summary")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let transport = trace
        .get("transport")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let stream_runner = trace
        .get("stream_runner")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let panel_contract = trace
        .get("panel_contract")
        .cloned()
        .unwrap_or_else(editor_native_host_trace_panel_contract_json);
    let trace_json = html_script_json(&serde_json::to_string_pretty(trace)?);
    let transport_json = html_escape_text(&serde_json::to_string_pretty(&transport)?);
    let stream_runner_json = html_escape_text(&serde_json::to_string_pretty(&stream_runner)?);
    let panel_contract_json = html_escape_text(&serde_json::to_string_pretty(&panel_contract)?);
    let trace_path = html_escape_text(
        summary
            .get("trace_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    );
    let build_dir = html_escape_text(
        summary
            .get("build_dir")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    );
    let frame_count = json_usize_field(&summary, "frame_count");
    let status_counts = summary
        .get("status_counts")
        .unwrap_or(&serde_json::Value::Null);
    let ok_count = json_usize_field(status_counts, "ok");
    let client_error_count = json_usize_field(status_counts, "client_error");
    let server_error_count = json_usize_field(status_counts, "server_error");
    let first_request = trace_panel_request_label(summary.get("first_request"));
    let last_request = trace_panel_request_label(summary.get("last_request"));
    let mut html = String::new();
    html.push_str(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>orv Trace Panel</title>\n<style>\n:root{color-scheme:light dark;--bg:#f7f7f4;--fg:#161714;--muted:#6b6f69;--panel:#ffffff;--line:#d7d9d2;--accent:#0d6b5f;--accent-weak:#dcefeb;--bad:#a43434;--warn:#8a5a00;}\n@media (prefers-color-scheme: dark){:root{--bg:#11130f;--fg:#ecefe8;--muted:#a8aea2;--panel:#191c17;--line:#30362d;--accent:#67c7b5;--accent-weak:#203a35;--bad:#ff9d9d;--warn:#e8c06b;}}\n*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.45 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;}header{padding:24px 28px 12px;border-bottom:1px solid var(--line);}h1{font-size:24px;margin:0 0 8px;}h2{font-size:13px;text-transform:uppercase;letter-spacing:.08em;color:var(--muted);margin:0 0 12px}.muted{color:var(--muted)}main{display:grid;grid-template-columns:minmax(280px,380px) minmax(0,1fr);gap:16px;padding:16px 28px 28px}.panel{background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:16px}.summary{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:10px;margin-top:16px}.metric{border:1px solid var(--line);border-radius:6px;padding:10px;background:var(--bg)}.metric b{display:block;font-size:22px;line-height:1.1}.filterbar{display:flex;flex-wrap:wrap;gap:8px}.filterbar button{border:1px solid var(--line);background:var(--bg);color:var(--fg);border-radius:6px;padding:7px 10px;cursor:pointer}.filterbar button[aria-pressed=\"true\"]{border-color:var(--accent);background:var(--accent-weak)}.list{list-style:none;margin:0;padding:0;display:grid;gap:8px}.list li{border:1px solid var(--line);border-radius:6px;padding:10px;cursor:pointer;background:var(--bg)}.list li:focus,.list li:hover{outline:2px solid var(--accent);outline-offset:1px}.status-client_error,.status-server_error{color:var(--bad)}.status-redirect,.status-other{color:var(--warn)}pre{margin:0;white-space:pre-wrap;overflow:auto;font:12px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}.detail-grid{display:grid;grid-template-columns:1fr 1fr;gap:16px}.wide{grid-column:1/-1}@media (max-width:900px){main{grid-template-columns:1fr;padding:14px}.summary,.detail-grid{grid-template-columns:1fr}header{padding:18px 14px 8px}}\n</style>\n</head>\n<body>\n",
    );
    writeln!(
        &mut html,
        "<header><h1>Trace Panel</h1><div class=\"muted\">{trace_path}</div><div class=\"muted\">{build_dir}</div><section class=\"summary\"><div class=\"metric\"><span>Frames</span><b>{frame_count}</b></div><div class=\"metric\"><span>OK</span><b>{ok_count}</b></div><div class=\"metric\"><span>Client Err</span><b>{client_error_count}</b></div><div class=\"metric\"><span>Server Err</span><b>{server_error_count}</b></div></section></header>"
    )?;
    writeln!(
        &mut html,
        "<main><section class=\"panel\"><h2>Status Filters</h2><div id=\"trace-filterbar\" class=\"filterbar\"></div><p class=\"muted\">First: {}</p><p class=\"muted\">Last: {}</p></section>",
        html_escape_text(&first_request),
        html_escape_text(&last_request)
    )?;
    html.push_str(
        "<section class=\"panel\"><h2>Frame Detail</h2><pre id=\"trace-frame-detail\">No trace frame selected.</pre></section>\n<section class=\"panel\"><h2>Frames</h2><ul id=\"trace-frame-list\" class=\"list\"></ul></section>\n<section class=\"detail-grid\">\n",
    );
    writeln!(
        &mut html,
        "<section class=\"panel\"><h2>Transport</h2><pre>{transport_json}</pre></section><section class=\"panel\"><h2>Trace Stream Runner</h2><pre>{stream_runner_json}</pre></section><section class=\"panel wide\"><h2>Panel Contract</h2><pre>{panel_contract_json}</pre></section></section></main>"
    )?;
    writeln!(
        &mut html,
        "<script id=\"orv-trace\" type=\"application/json\">{trace_json}</script>"
    )?;
    html.push_str(
        "<script>\nconst trace = JSON.parse(document.getElementById('orv-trace').textContent);\nconst frames = Array.isArray(trace.frames) ? trace.frames : [];\nconst filters = Array.isArray(trace.status_filters) ? trace.status_filters : [];\nconst filterbar = document.getElementById('trace-filterbar');\nconst list = document.getElementById('trace-frame-list');\nconst detail = document.getElementById('trace-frame-detail');\nfunction frameLabel(frame){\n  return frame?.summary?.label || `${frame?.request?.method || ''} ${frame?.request?.path || ''}`.trim() || frame?.origin_id || 'request';\n}\nfunction renderDetail(frame){\n  if (!frame) { detail.textContent = 'No trace frame selected.'; return; }\n  const source = frame.source || {};\n  const production = frame.production || {};\n  const request = frame.request || {};\n  const lines = [\n    frameLabel(frame),\n    frame.summary?.status_class ? `status ${frame.summary.status_class}` : '',\n    frame.origin_id ? `origin ${frame.origin_id}` : '',\n    source.path ? `source ${source.path}${source.location?.line ? `:${source.location.line}` : ''}` : '',\n    production.path ? `production ${production.path}` : '',\n    Array.isArray(frame.reveal_command) ? `reveal ${frame.reveal_command.join(' ')}` : '',\n    request.params && Object.keys(request.params).length ? `params ${JSON.stringify(request.params)}` : '',\n    request.query && Object.keys(request.query).length ? `query ${JSON.stringify(request.query)}` : '',\n    request.body ? `body ${request.body}` : '',\n    source.snippet || ''\n  ].filter(Boolean);\n  detail.textContent = lines.join('\\n');\n}\nfunction renderFrames(filter){\n  const rows = filter === 'all' ? frames : frames.filter(frame => frame.summary?.status_class === filter);\n  list.textContent = '';\n  for (const frame of rows) {\n    const row = document.createElement('li');\n    const status = frame.summary?.status_class || 'other';\n    row.className = `status-${status}`;\n    row.textContent = frameLabel(frame);\n    row.tabIndex = 0;\n    row.addEventListener('click', () => renderDetail(frame));\n    row.addEventListener('keydown', event => {\n      if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); renderDetail(frame); }\n    });\n    list.appendChild(row);\n  }\n  renderDetail(rows[0]);\n}\nfor (const filter of filters) {\n  const button = document.createElement('button');\n  button.type = 'button';\n  button.dataset.filter = filter.name || 'all';\n  button.setAttribute('aria-pressed', button.dataset.filter === 'all' ? 'true' : 'false');\n  button.textContent = `${filter.label || filter.name || 'Filter'} ${filter.count ?? 0}`;\n  button.addEventListener('click', () => {\n    for (const item of filterbar.querySelectorAll('button')) item.setAttribute('aria-pressed', 'false');\n    button.setAttribute('aria-pressed', 'true');\n    renderFrames(button.dataset.filter || 'all');\n  });\n  filterbar.appendChild(button);\n}\nif (!filters.length) {\n  const empty = document.createElement('span');\n  empty.className = 'muted';\n  empty.textContent = 'No trace filters.';\n  filterbar.appendChild(empty);\n}\nrenderFrames('all');\n</script>\n</body>\n</html>\n",
    );
    Ok(html)
}

pub(crate) fn trace_panel_request_label(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(|value| value.get("label"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

pub(crate) fn html_script_json(value: &str) -> String {
    value.replace('&', "\\u0026").replace('<', "\\u003c")
}

pub(crate) struct EditorGraphPanel {
    pub(crate) node_count: usize,
    pub(crate) edge_count: usize,
    pub(crate) source_depth: usize,
    pub(crate) semantic_depth: usize,
    pub(crate) svg: String,
}

pub(crate) fn editor_graph_panel_from_state(state: &serde_json::Value) -> EditorGraphPanel {
    let graph_stats = state
        .pointer("/snapshot/project_graph/stats")
        .unwrap_or(&serde_json::Value::Null);
    let graph_nodes = state
        .pointer("/snapshot/project_graph/nodes")
        .and_then(serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let graph_edges = state
        .pointer("/snapshot/project_graph/edges")
        .and_then(serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    EditorGraphPanel {
        node_count: json_usize_field(graph_stats, "node_count"),
        edge_count: json_usize_field(graph_stats, "edge_count"),
        source_depth: json_usize_field(graph_stats, "max_source_contains_depth"),
        semantic_depth: json_usize_field(graph_stats, "max_semantic_contains_depth"),
        svg: project_graph_view_svg(graph_nodes, graph_edges),
    }
}

pub(crate) fn write_editor_graph_panel_html(
    html: &mut String,
    graph: &EditorGraphPanel,
) -> anyhow::Result<()> {
    write!(
        html,
        "<section class=\"panel graph-panel\"><h2>Project Graph</h2><div class=\"metric\">{}</div><p class=\"muted\">{} edges, source depth {}, semantic depth {}.</p><div id=\"editor-graph-view\" class=\"graph-view\">{}</div></section>",
        graph.node_count, graph.edge_count, graph.source_depth, graph.semantic_depth, graph.svg
    )?;
    Ok(())
}

pub(crate) fn editor_export_html(state: &serde_json::Value) -> anyhow::Result<String> {
    let entry = state
        .pointer("/snapshot/entry/path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("app.orv");
    let file_count = json_array_count(state.pointer("/snapshot/panels/files"));
    let route_count = json_array_count(state.pointer("/snapshot/panels/routes"));
    let schema_count = json_array_count(state.pointer("/snapshot/panels/schema"));
    let domain_count = json_array_count(state.pointer("/snapshot/panels/domains"));
    let diagnostic_count = json_array_count(state.pointer("/snapshot/diagnostics"));
    let graph_panel = editor_graph_panel_from_state(state);
    let runtime_frame_count = json_array_count(state.pointer("/runtime/frames"));
    let debug_config_count = json_array_count(state.pointer("/debug/configurations"));
    let debug_control_count = json_array_count(state.pointer("/debug/controls"));
    let debug_capability_count = editor_debug_capability_count_from_state(state);
    let debug_breakpoint_count = editor_debug_breakpoint_count_from_state(state);
    let debug_function_breakpoint_count = editor_debug_function_breakpoint_count_from_state(state);
    let debug_data_breakpoint_count = editor_debug_data_breakpoint_count_from_state(state);
    let debug_exception_filter_count = editor_debug_exception_filter_count_from_state(state);
    let production_client_target_count = json_array_count(state.pointer("/production/client"));
    let production_native_server_target_count =
        json_array_count(state.pointer("/production/native_server"));
    let production_static_target_count = json_array_count(state.pointer("/production/static"));
    let production_preflight_count = json_array_count(state.pointer("/production/preflight"));
    let production_db_adapter_count = json_array_count(state.pointer("/production/db_adapters"));
    let production_commerce_adapter_count =
        json_array_count(state.pointer("/production/commerce_adapters"));
    let production_summary = editor_production_summary_text(state);
    let trace_count = json_array_count(state.pointer("/trace/frames"));
    let trace_status_counts = editor_trace_status_counts_from_state(state);
    let runtime_status = state
        .pointer("/runtime/runtime/status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let stdout = state
        .pointer("/runtime/runtime/stdout")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let state_json = serde_json::to_string(state)?.replace("</", "<\\/");
    let mut html = String::new();
    html.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    html.push_str("<title>orv editor</title>\n");
    html.push_str("<style>\n");
    html.push_str(":root{color-scheme:light;--bg:#f7f8fb;--ink:#18202f;--muted:#687386;--line:#d7dce5;--panel:#ffffff;--accent:#0f766e;--warn:#b45309;}\n");
    html.push_str("*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:14px/1.45 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif}#orv-editor{min-height:100vh;display:grid;grid-template-columns:240px 1fr;grid-template-rows:auto 1fr}.sidebar{grid-row:1/3;border-right:1px solid var(--line);background:#111827;color:#f8fafc;padding:20px 16px}.brand{font-weight:700;font-size:18px;margin-bottom:18px}.nav{display:grid;gap:8px}.nav span{display:flex;justify-content:space-between;border:1px solid #334155;padding:8px 10px}.topbar{border-bottom:1px solid var(--line);background:var(--panel);padding:14px 20px}.topbar h1{font-size:18px;margin:0}.topbar p{margin:4px 0 0;color:var(--muted)}.workspace{padding:18px 20px;display:grid;gap:14px;grid-template-columns:repeat(2,minmax(0,1fr))}.panel{border:1px solid var(--line);background:var(--panel);border-radius:8px;padding:14px;min-height:132px}.panel h2{font-size:14px;margin:0 0 10px}.metric{font-size:28px;font-weight:700}.muted{color:var(--muted)}.list{list-style:none;margin:10px 0 0;padding:0;display:grid;gap:6px}.list li{border-top:1px solid var(--line);padding-top:6px;color:var(--muted);word-break:break-word;cursor:pointer}.list li:focus{outline:2px solid var(--accent);outline-offset:2px}.filterbar{display:flex;flex-wrap:wrap;gap:6px;margin:10px 0}.filterbar button{border:1px solid var(--line);background:#f8fafc;color:var(--ink);padding:5px 8px;font:inherit;cursor:pointer}.filterbar button[aria-pressed=\"true\"]{border-color:var(--accent);color:var(--accent);font-weight:700}.detail{min-height:120px}pre{white-space:pre-wrap;word-break:break-word;margin:0;max-height:240px;overflow:auto;background:#f1f5f9;border:1px solid var(--line);padding:10px}@media(max-width:760px){#orv-editor{display:block}.sidebar{border-right:0}.workspace{grid-template-columns:1fr}}\n");
    html.push_str(".graph-panel{grid-column:1/-1}.graph-view{overflow:auto;border:1px solid var(--line);background:#fff}.graph-view svg{display:block;min-width:900px}\n");
    html.push_str("</style>\n</head>\n<body>\n");
    html.push_str("<main id=\"orv-editor\">\n");
    html.push_str(
        "<aside class=\"sidebar\"><div class=\"brand\">orv editor</div><nav class=\"nav\">",
    );
    write!(&mut html, "<span>Files<b>{file_count}</b></span>")?;
    write!(&mut html, "<span>Routes<b>{route_count}</b></span>")?;
    write!(&mut html, "<span>Schema<b>{schema_count}</b></span>")?;
    write!(&mut html, "<span>Domains<b>{domain_count}</b></span>")?;
    write!(
        &mut html,
        "<span>Graph<b>{}</b></span>",
        graph_panel.node_count
    )?;
    write!(
        &mut html,
        "<span>Runtime Frames<b>{runtime_frame_count}</b></span>"
    )?;
    write!(&mut html, "<span>Debug<b>{debug_config_count}</b></span>")?;
    write!(
        &mut html,
        "<span>Debug Controls<b>{debug_control_count}</b></span>"
    )?;
    write!(
        &mut html,
        "<span>DAP Caps<b>{debug_capability_count}</b></span>"
    )?;
    write!(
        &mut html,
        "<span>Production<b>{}</b></span>",
        production_client_target_count
            + production_native_server_target_count
            + production_static_target_count
            + production_preflight_count
            + production_db_adapter_count
            + production_commerce_adapter_count
    )?;
    write!(&mut html, "<span>Trace<b>{trace_count}</b></span>")?;
    html.push_str("</nav></aside>\n");
    html.push_str("<header class=\"topbar\">");
    write!(
        &mut html,
        "<h1>{}</h1><p>First-party editor export backed by shared ProjectGraph.</p>",
        html_escape_text(entry)
    )?;
    html.push_str("</header>\n<section class=\"workspace\">\n");
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Routes</h2><div class=\"metric\">{route_count}</div><p class=\"muted\">Graph-backed route panel entries.</p><ul id=\"routes-list\" class=\"list\"></ul></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Schema</h2><div class=\"metric\">{schema_count}</div><p class=\"muted\">Struct, enum, and type alias nodes.</p><ul id=\"schema-list\" class=\"list\"></ul></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Domains</h2><div class=\"metric\">{domain_count}</div><p class=\"muted\">Project domain and define nodes.</p><ul id=\"domains-list\" class=\"list\"></ul></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Diagnostics</h2><div class=\"metric\">{diagnostic_count}</div><p class=\"muted\">Project loader, resolver, and analyzer diagnostics.</p></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Debug</h2><div class=\"metric\">{debug_config_count}</div><p class=\"muted\">DAP launch and attach configurations.</p><ul id=\"debug-config-list\" class=\"list\"></ul></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Debug Controls</h2><div class=\"metric\">{debug_control_count}</div><p class=\"muted\">DAP live-control request payloads.</p><ul id=\"debug-control-list\" class=\"list\"></ul></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>DAP Capabilities</h2><div class=\"metric\">{debug_capability_count}</div><p class=\"muted\">Adapter features exposed to native hosts.</p><ul id=\"debug-capability-list\" class=\"list\"></ul></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Breakpoints</h2><div class=\"metric\">{debug_breakpoint_count}</div><p class=\"muted\">Executable source lines for DAP setBreakpoints.</p><ul id=\"debug-breakpoint-list\" class=\"list\"></ul></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Function Breakpoints</h2><div class=\"metric\">{debug_function_breakpoint_count}</div><p class=\"muted\">Named functions for DAP setFunctionBreakpoints.</p><ul id=\"debug-function-breakpoint-list\" class=\"list\"></ul></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Data Breakpoints</h2><div class=\"metric\">{debug_data_breakpoint_count}</div><p class=\"muted\">Local variables for DAP setDataBreakpoints.</p><ul id=\"debug-data-breakpoint-list\" class=\"list\"></ul></section>"
    )?;
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Exception Filters</h2><div class=\"metric\">{debug_exception_filter_count}</div><p class=\"muted\">DAP exception filter presets.</p><ul id=\"debug-exception-filter-list\" class=\"list\"></ul></section>"
    )?;
    write_editor_graph_panel_html(&mut html, &graph_panel)?;
    html.push_str("<section class=\"panel\"><h2>Debug Runner</h2><pre id=\"debug-runner-detail\" class=\"detail\"></pre></section>");
    html.push_str("<section class=\"panel\"><h2>Debug Result</h2><pre id=\"debug-result-detail\" class=\"detail\"></pre></section>");
    html.push_str("<section class=\"panel\"><h2>Runner Command</h2><pre id=\"debug-control-command\" class=\"detail\"></pre></section>");
    html.push_str("<section class=\"panel\"><h2>Selected Debug</h2><pre id=\"debug-detail\" class=\"detail\"></pre></section>");
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Production</h2><div class=\"metric\">{}</div><p class=\"muted\">Client Bundles {production_client_target_count} · Preflight {production_preflight_count} · DB Adapters {production_db_adapter_count} · Commerce Adapters {production_commerce_adapter_count}</p><pre>{}</pre></section>",
        production_client_target_count
            + production_preflight_count
            + production_db_adapter_count
            + production_commerce_adapter_count,
        html_escape_text(&production_summary)
    )?;
    write_trace_panel_html(&mut html, trace_count, &trace_status_counts)?;
    html.push_str("<section class=\"panel\"><h2>Selected Trace</h2><pre id=\"trace-detail\" class=\"detail\"></pre></section>");
    html.push_str("<section class=\"panel\"><h2>Trace Transport</h2><pre id=\"trace-transport-detail\" class=\"detail\"></pre></section>");
    html.push_str("<section class=\"panel\"><h2>Trace Stream Runner</h2><pre id=\"trace-stream-runner-detail\" class=\"detail\"></pre></section>");
    html.push_str("<section class=\"panel\"><h2>Runtime</h2>");
    write!(
        &mut html,
        "<div class=\"metric\">{}</div><p class=\"muted\">Reference runtime status.</p><pre>{}</pre>",
        html_escape_text(runtime_status),
        html_escape_text(stdout)
    )?;
    html.push_str("</section>\n");
    write!(
        &mut html,
        "<section class=\"panel\"><h2>Runtime Frames</h2><div class=\"metric\">{runtime_frame_count}</div><ul id=\"runtime-frame-list\" class=\"list\"></ul></section>"
    )?;
    html.push_str("<section class=\"panel\"><h2>Selected Runtime</h2><pre id=\"runtime-frame-detail\" class=\"detail\"></pre></section>");
    html.push_str("</section>\n");
    html.push_str("</main>\n");
    html.push_str("<script id=\"orv-editor-state\" type=\"application/json\">");
    html.push_str(&state_json);
    html.push_str("</script>\n");
    html.push_str(
        "<script>\nfunction renderTraceDetail(frame){\n  const target = document.getElementById('trace-detail');\n  if (!target) return;\n  if (!frame) {\n    target.textContent = 'No trace frame selected.';\n    return;\n  }\n  const request = frame.request || {};\n  const summary = frame.summary || {};\n  const navigation = frame.navigation || {};\n  const source = navigation.source || {};\n  const location = source.location || {};\n  const params = request.params && Object.keys(request.params).length ? `params ${JSON.stringify(request.params)}` : '';\n  const query = request.query && Object.keys(request.query).length ? `query ${JSON.stringify(request.query)}` : '';\n  const body = request.body ? `body ${request.body}` : '';\n  const lines = [\n    summary.label || `${request.method || ''} ${request.path || ''}`.trim(),\n    summary.route ? `route ${summary.route}` : '',\n    summary.status_class ? `status ${summary.status_class}` : '',\n    frame.origin_id ? `origin ${frame.origin_id}` : '',\n    params,\n    query,\n    body,\n    source.path || location.uri || '',\n    source.snippet || ''\n  ].filter(Boolean);\n  target.textContent = lines.join('\\n');\n}\nfunction renderRuntimeDetail(frame){\n  const target = document.getElementById('runtime-frame-detail');\n  if (!target) return;\n  if (!frame) {\n    target.textContent = 'No runtime frame selected.';\n    return;\n  }\n  const source = frame.source || {};\n  const locals = (frame.locals || []).map(local => `  ${local.name}: ${local.value}${local.type ? ` (${local.type})` : ''}`);\n  const stack = (frame.stack || []).map(call => `  ${call.name || 'frame'} ${call.source?.name || call.source?.path || ''}:${call.line || ''}`.trim());\n  const output = frame.output ? `output ${String(frame.output).trimEnd()}` : '';\n  const lines = [\n    `frame #${(frame.index ?? 0) + 1}`,\n    source.path ? `source ${source.path}:${frame.line || ''}` : (frame.line ? `line ${frame.line}` : ''),\n    output,\n    locals.length ? `locals\\n${locals.join('\\n')}` : '',\n    stack.length ? `stack\\n${stack.join('\\n')}` : ''\n  ].filter(Boolean);\n  target.textContent = lines.join('\\n');\n}\nfunction renderDebugDetail(value){\n  const target = document.getElementById('debug-detail');\n  if (!target) return;\n  if (!value) {\n    target.textContent = 'No debug item selected.';\n    return;\n  }\n  target.textContent = JSON.stringify(value, null, 2);\n}\nfunction renderDebugRunner(runner){\n  const target = document.getElementById('debug-runner-detail');\n  if (!target) return;\n  target.textContent = runner ? JSON.stringify(runner, null, 2) : 'No debug runner.';\n}\nfunction renderDebugResultArtifact(result){\n  const target = document.getElementById('debug-result-detail');\n  if (!target) return;\n  if (!result) {\n    target.textContent = 'No debug result artifact.';\n    return;\n  }\n  const panels = Array.isArray(result.panels) ? result.panels.join(', ') : '';\n  target.textContent = [result.kind, result.path, result.media_type, panels ? `panels ${panels}` : ''].filter(Boolean).join('\\n');\n}\nfunction debugBreakpointRows(state){\n  const rows = [];\n  for (const group of state.debug?.breakpoint_sources || []) {\n    const breakpoints = group.breakpoints || (group.lines || []).map(line => ({line}));\n    for (const breakpoint of breakpoints) {\n      rows.push({...breakpoint, source: group.source || {}, line: breakpoint.line});\n    }\n  }\n  return rows;\n}\nfunction filterTraceFrames(frames, filter){\n  if (filter === 'all') return frames;\n  return frames.filter(frame => frame.summary?.status_class === filter);\n}\nfunction renderTraceTransport(state){\n  const target = document.getElementById('trace-transport-detail');\n  if (!target) return;\n  const transport = state.trace?.live_refresh?.transport;\n  if (!transport) {\n    target.textContent = 'No trace transport.';\n    return;\n  }\n  target.textContent = [transport.kind, transport.event, transport.url].filter(Boolean).join('\\n');\n}\nfunction renderTraceStreamRunner(state){\n  const target = document.getElementById('trace-stream-runner-detail');\n  if (!target) return;\n  const runner = state.trace?.stream_runner;\n  if (!runner) {\n    target.textContent = 'No trace stream runner.';\n    return;\n  }\n  const command = Array.isArray(runner.command) ? runner.command.join(' ') : '';\n  target.textContent = [runner.kind, runner.event_stream, command].filter(Boolean).join('\\n');\n}\nfunction renderEditorState(){\n  const state = JSON.parse(document.getElementById('orv-editor-state').textContent);\n  const put = (id, items, label, onPick) => {\n    const target = document.getElementById(id);\n    if (!target) return;\n    target.textContent = '';\n    for (const item of items || []) {\n      const row = document.createElement('li');\n      row.textContent = label(item);\n      if (onPick) {\n        row.tabIndex = 0;\n        row.addEventListener('click', () => onPick(item));\n        row.addEventListener('keydown', event => {\n          if (event.key === 'Enter' || event.key === ' ') {\n            event.preventDefault();\n            onPick(item);\n          }\n        });\n      }\n      target.appendChild(row);\n    }\n  };\n  put('routes-list', state.snapshot?.panels?.routes, item => `${item.method || ''} ${item.path || item.name || ''}`.trim() || item.origin_id || 'route');\n  put('schema-list', state.snapshot?.panels?.schema, item => item.name || item.kind || 'schema');\n  put('domains-list', state.snapshot?.panels?.domains, item => item.name || item.kind || 'domain');\n  const debugConfigs = state.debug?.configurations || [];\n  put('debug-config-list', debugConfigs, item => item.name || item.request || 'debug', renderDebugDetail);\n  const debugBreakpoints = debugBreakpointRows(state);\n  put('debug-breakpoint-list', debugBreakpoints, breakpoint => {\n    const source = breakpoint.source || {};\n    return `${source.name || source.path || 'source'}:${breakpoint.line}`;\n  }, breakpoint => {\n    const request = breakpoint.request || {\n      command: 'setBreakpoints',\n      arguments: {source: breakpoint.source, breakpoints: [{line: breakpoint.line}]}\n    };\n    renderDebugControlCommand({runner_command: breakpoint.runner_command || []});\n    renderDebugDetail({request, runner_command: breakpoint.runner_command || []});\n  });\n  renderDebugRunner(state.debug?.session_runner);\n  renderDebugResultArtifact(state.debug?.result_artifact || state.debug?.session_runner?.result);\n  renderDebugDetail(debugConfigs[0]);\n  const runtimeFrames = state.runtime?.frames || [];\n  put('runtime-frame-list', runtimeFrames, frame => {\n    const source = frame.source || {};\n    const label = source.name || source.path || 'frame';\n    const line = frame.line ? `:${frame.line}` : '';\n    return `#${(frame.index ?? 0) + 1} ${label}${line}`;\n  }, renderRuntimeDetail);\n  renderRuntimeDetail(runtimeFrames[0]);\n  const traceFrames = state.trace?.frames || [];\n  const traceButtons = Array.from(document.querySelectorAll('[data-trace-filter]'));\n  const renderTraceList = filter => {\n    const frames = filterTraceFrames(traceFrames, filter);\n    put('trace-list', frames, frame => frame.summary?.label || frame.origin_id || 'request', renderTraceDetail);\n    renderTraceDetail(frames[0]);\n  };\n  for (const button of traceButtons) {\n    button.addEventListener('click', () => {\n      for (const item of traceButtons) item.setAttribute('aria-pressed', 'false');\n      button.setAttribute('aria-pressed', 'true');\n      renderTraceList(button.dataset.traceFilter || 'all');\n    });\n  }\n  renderTraceList('all');\n  renderTraceTransport(state);\n  renderTraceStreamRunner(state);\n}\nrenderEditorState();\n</script>\n",
    );
    html.push_str(
        "<script>\nfunction renderDebugControlCommand(control){\n  const target = document.getElementById('debug-control-command');\n  if (!target) return;\n  const command = control?.runner_command || control?.command || [];\n  target.textContent = Array.isArray(command) ? command.join(' ') : JSON.stringify(command, null, 2);\n}\nfunction renderDebugControls(){\n  const stateNode = document.getElementById('orv-editor-state');\n  const target = document.getElementById('debug-control-list');\n  if (!stateNode || !target) return;\n  const state = JSON.parse(stateNode.textContent);\n  target.textContent = '';\n  const controls = state.debug?.controls || [];\n  for (const control of controls) {\n    const row = document.createElement('li');\n    row.textContent = control.name || control.request?.command || 'control';\n    row.tabIndex = 0;\n    const show = () => {\n      renderDebugControlCommand(control);\n      renderDebugDetail(control.request || control);\n    };\n    row.addEventListener('click', show);\n    row.addEventListener('keydown', event => {\n      if (event.key === 'Enter' || event.key === ' ') {\n        event.preventDefault();\n        show();\n      }\n    });\n    target.appendChild(row);\n  }\n  if (controls.length) renderDebugControlCommand(controls[0]);\n}\nrenderDebugControls();\n</script>\n",
    );
    html.push_str(
        "<script>\nfunction renderFunctionBreakpoints(){\n  const stateNode = document.getElementById('orv-editor-state');\n  const target = document.getElementById('debug-function-breakpoint-list');\n  if (!stateNode || !target) return;\n  const state = JSON.parse(stateNode.textContent);\n  target.textContent = '';\n  for (const breakpoint of state.debug?.function_breakpoints || []) {\n    const row = document.createElement('li');\n    const source = breakpoint.source || {};\n    row.textContent = `${breakpoint.name || 'function'}${source.line ? `:${source.line}` : ''}`;\n    row.tabIndex = 0;\n    const show = () => {\n      renderDebugControlCommand({runner_command: breakpoint.runner_command || []});\n      renderDebugDetail({request: breakpoint.request || {}, runner_command: breakpoint.runner_command || [], source});\n    };\n    row.addEventListener('click', show);\n    row.addEventListener('keydown', event => {\n      if (event.key === 'Enter' || event.key === ' ') {\n        event.preventDefault();\n        show();\n      }\n    });\n    target.appendChild(row);\n  }\n}\nrenderFunctionBreakpoints();\n</script>\n",
    );
    html.push_str(
        "<script>\nfunction renderDataBreakpoints(){\n  const stateNode = document.getElementById('orv-editor-state');\n  const target = document.getElementById('debug-data-breakpoint-list');\n  if (!stateNode || !target) return;\n  const state = JSON.parse(stateNode.textContent);\n  target.textContent = '';\n  for (const breakpoint of state.debug?.data_breakpoints || []) {\n    const row = document.createElement('li');\n    const source = breakpoint.source || {};\n    const line = source.line ? `:${source.line}` : '';\n    row.textContent = `${breakpoint.name || 'local'}${line}`;\n    row.tabIndex = 0;\n    const show = () => {\n      renderDebugControlCommand({runner_command: breakpoint.runner_command || []});\n      renderDebugDetail({info_request: breakpoint.info_request || {}, request: breakpoint.request || {}, runner_command: breakpoint.runner_command || [], source});\n    };\n    row.addEventListener('click', show);\n    row.addEventListener('keydown', event => {\n      if (event.key === 'Enter' || event.key === ' ') {\n        event.preventDefault();\n        show();\n      }\n    });\n    target.appendChild(row);\n  }\n}\nrenderDataBreakpoints();\n</script>\n",
    );
    html.push_str(
        "<script>\nfunction renderExceptionFilters(){\n  const stateNode = document.getElementById('orv-editor-state');\n  const target = document.getElementById('debug-exception-filter-list');\n  if (!stateNode || !target) return;\n  const state = JSON.parse(stateNode.textContent);\n  target.textContent = '';\n  for (const filter of state.debug?.exception_filters || []) {\n    const row = document.createElement('li');\n    row.textContent = filter.label || filter.filter || 'exception filter';\n    row.tabIndex = 0;\n    const show = () => {\n      renderDebugControlCommand({runner_command: filter.runner_command || []});\n      renderDebugDetail({request: filter.request || {}, runner_command: filter.runner_command || []});\n    };\n    row.addEventListener('click', show);\n    row.addEventListener('keydown', event => {\n      if (event.key === 'Enter' || event.key === ' ') {\n        event.preventDefault();\n        show();\n      }\n    });\n    target.appendChild(row);\n  }\n}\nrenderExceptionFilters();\n</script>\n",
    );
    html.push_str(
        "<script>\nfunction renderDebugCapabilities(){\n  const stateNode = document.getElementById('orv-editor-state');\n  const target = document.getElementById('debug-capability-list');\n  if (!stateNode || !target) return;\n  const state = JSON.parse(stateNode.textContent);\n  target.textContent = '';\n  for (const [name, value] of Object.entries(state.debug?.capabilities || {})) {\n    if (value !== true && !Array.isArray(value)) continue;\n    const row = document.createElement('li');\n    row.textContent = Array.isArray(value) ? `${name} (${value.length})` : name;\n    row.tabIndex = 0;\n    const show = () => renderDebugDetail({name, value});\n    row.addEventListener('click', show);\n    row.addEventListener('keydown', event => {\n      if (event.key === 'Enter' || event.key === ' ') {\n        event.preventDefault();\n        show();\n      }\n    });\n    target.appendChild(row);\n  }\n}\nrenderDebugCapabilities();\n</script>\n</body>\n</html>\n",
    );
    Ok(html)
}

pub(crate) fn write_trace_panel_html(
    html: &mut String,
    trace_count: usize,
    trace_status_counts: &EditorTraceStatusCounts,
) -> anyhow::Result<()> {
    write!(
        html,
        "<section class=\"panel\"><h2>Trace</h2><div class=\"metric\">{trace_count}</div><div id=\"trace-status-summary\" class=\"nav\">"
    )?;
    write!(
        html,
        "<span>OK<b>{}</b></span><span>Client Err<b>{}</b></span><span>Server Err<b>{}</b></span>",
        trace_status_counts.ok, trace_status_counts.client_error, trace_status_counts.server_error
    )?;
    html.push_str("</div><div class=\"filterbar\">");
    for (filter, label, count) in [
        ("all", "All", trace_status_counts.total),
        ("ok", "OK", trace_status_counts.ok),
        ("redirect", "3xx", trace_status_counts.redirect),
        ("client_error", "4xx", trace_status_counts.client_error),
        ("server_error", "5xx", trace_status_counts.server_error),
        ("other", "Other", trace_status_counts.other),
    ] {
        write!(
            html,
            "<button type=\"button\" data-trace-filter=\"{}\" aria-pressed=\"{}\">{}<b>{}</b></button>",
            filter,
            if filter == "all" { "true" } else { "false" },
            label,
            count
        )?;
    }
    html.push_str("</div><ul id=\"trace-list\" class=\"list\"></ul></section>");
    Ok(())
}

pub(crate) fn editor_debug_breakpoint_count_from_state(state: &serde_json::Value) -> usize {
    state
        .pointer("/debug/breakpoint_sources")
        .and_then(serde_json::Value::as_array)
        .map_or(0, |sources| {
            sources
                .iter()
                .map(|source| json_array_count(source.get("lines")))
                .sum()
        })
}

pub(crate) fn editor_debug_function_breakpoint_count_from_state(
    state: &serde_json::Value,
) -> usize {
    json_array_count(state.pointer("/debug/function_breakpoints"))
}

pub(crate) fn editor_debug_data_breakpoint_count_from_state(state: &serde_json::Value) -> usize {
    json_array_count(state.pointer("/debug/data_breakpoints"))
}

pub(crate) fn editor_debug_exception_filter_count_from_state(state: &serde_json::Value) -> usize {
    json_array_count(state.pointer("/debug/exception_filters"))
}

pub(crate) fn editor_debug_capability_count_from_state(state: &serde_json::Value) -> usize {
    state
        .pointer("/debug/capabilities")
        .and_then(serde_json::Value::as_object)
        .map_or(0, |capabilities| {
            capabilities
                .values()
                .filter(|value| value.as_bool() == Some(true) || value.is_array())
                .count()
        })
}

pub(crate) fn editor_trace_status_counts_from_state(
    state: &serde_json::Value,
) -> EditorTraceStatusCounts {
    let mut counts = EditorTraceStatusCounts::default();
    let Some(value) = state.pointer("/trace/trace/status_counts") else {
        return counts;
    };
    counts.total = json_usize_field(value, "total");
    counts.ok = json_usize_field(value, "ok");
    counts.redirect = json_usize_field(value, "redirect");
    counts.client_error = json_usize_field(value, "client_error");
    counts.server_error = json_usize_field(value, "server_error");
    counts.other = json_usize_field(value, "other");
    counts
}

pub(crate) fn editor_production_summary_text(state: &serde_json::Value) -> String {
    let Some(production) = state.get("production") else {
        return "No production build attached.".to_string();
    };
    let mut lines = Vec::new();
    if let Some(build_dir) = production
        .get("build_dir")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("build {build_dir}"));
    }
    for target in production
        .get("graph_contract")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = json_str_or_empty(target, "kind");
        let path = json_str_or_empty(target, "path");
        let hash = json_str_or_empty(target, "artifact_hash");
        let exists = target
            .get("exists")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        lines.push(format!(
            "Graph {kind} {path} (exists {exists}, hash {hash})"
        ));
    }
    for target in production
        .get("client")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = json_str_or_empty(target, "kind");
        let path = json_str_or_empty(target, "path");
        lines.push(format!("Client {kind} {path}"));
    }
    for target in production
        .get("preflight")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let path = json_str_or_empty(target, "path");
        let commands = json_object_count(target.get("commands"));
        let required_env = json_array_count(target.get("required_env"));
        let optional_env = json_array_count(target.get("optional_env"));
        let route_count = json_array_count(target.get("routes"));
        let route_policies = production_preflight_route_policy_count(std::slice::from_ref(target));
        let smoke_summary_present = production_preflight_smoke_summary_present(target);
        let smoke_summary_missing_markers =
            production_preflight_smoke_summary_missing_marker_count_from_target(target);
        lines.push(format!(
            "Preflight {path} (commands {commands}, routes {route_count}, route_policies {route_policies}, required_env {required_env}, optional_env {optional_env}, smoke_summary_present {smoke_summary_present}, smoke_summary_missing_markers {smoke_summary_missing_markers})"
        ));
    }
    for target in production
        .get("db_adapters")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let path = json_str_or_empty(target, "path");
        let adapters = json_array_count(target.get("adapters"));
        lines.push(format!("DB Adapters {path} ({adapters})"));
    }
    for target in production
        .get("commerce_adapters")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let path = json_str_or_empty(target, "path");
        let adapters = json_array_count(target.get("adapters"));
        lines.push(format!("Commerce Adapters {path} ({adapters})"));
    }
    if lines.is_empty() {
        "No production contracts.".to_string()
    } else {
        lines.join("\n")
    }
}

pub(crate) fn json_usize_field(value: &serde_json::Value, key: &str) -> usize {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(0)
}

pub(crate) fn json_u64_field(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

pub(crate) fn json_str_or_empty<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

pub(crate) fn json_array_count(value: Option<&serde_json::Value>) -> usize {
    value
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len)
}

pub(crate) fn json_object_count(value: Option<&serde_json::Value>) -> usize {
    value
        .and_then(serde_json::Value::as_object)
        .map_or(0, serde_json::Map::len)
}

pub(crate) fn html_escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn lsp_snapshot_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    let loaded = orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let resolved = orv_resolve::resolve(&loaded.program);
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    let origin_map = orv_compiler::origin_map(&lowered.program);
    let mut diagnostics = Vec::new();
    diagnostics.extend(lsp_diagnostics_json(&loaded.diagnostics, &loaded.files));
    diagnostics.extend(lsp_diagnostics_json(&resolved.diagnostics, &loaded.files));
    diagnostics.extend(lsp_diagnostics_json(&lowered.diagnostics, &loaded.files));
    Ok(serde_json::json!({
        "schema_version": 1,
        "uri": path.display().to_string(),
        "diagnostics": diagnostics,
        "project_graph": project_graph_json(&loaded.graph, &origin_map),
        "document_symbols": lsp_document_symbols_json(&loaded.graph, &loaded.files),
    }))
}

pub(crate) fn lsp_reveal_json(dir: &Path, origin_id: &str) -> anyhow::Result<serde_json::Value> {
    let reveal = reveal_origin_json(dir, origin_id)?;
    let source = reveal
        .get("source")
        .ok_or_else(|| anyhow::anyhow!("reveal source missing"))?;
    let path = json_str(source, "path", "reveal source")?;
    let start = json_u32(source, "start", "reveal source")?;
    let end = json_u32(source, "end", "reveal source")?;
    let source_text = source
        .get("content")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .map_or_else(
            || {
                std::fs::read_to_string(path)
                    .map_err(|e| anyhow::anyhow!("failed to read reveal source {path}: {e}"))
            },
            Ok,
        )?;
    Ok(serde_json::json!({
        "schema_version": 1,
        "origin": reveal.get("origin").cloned().unwrap_or(serde_json::Value::Null),
        "location": {
            "uri": path,
            "range": lsp_range_for_source(&source_text, start, end),
        },
        "project_graph": reveal.get("project_graph").cloned().unwrap_or(serde_json::Value::Null),
        "production": reveal.get("production").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

#[cfg(test)]
pub(crate) fn lsp_jsonrpc_response(request: &serde_json::Value) -> serde_json::Value {
    LspSession::default().jsonrpc_response(request)
}

#[derive(Default)]
pub(crate) struct LspSession {
    pub(crate) open_documents: HashMap<PathBuf, String>,
    pub(crate) workspace_root: Option<PathBuf>,
}

impl LspSession {
    pub(crate) fn message_response(
        &mut self,
        request: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        if request.get("id").is_none() {
            self.handle_notification(request);
            return None;
        }
        Some(self.jsonrpc_response(request))
    }

    pub(crate) fn jsonrpc_response(&mut self, request: &serde_json::Value) -> serde_json::Value {
        let id = request
            .get("id")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        match request.get("method").and_then(serde_json::Value::as_str) {
            Some("initialize") => self.initialize_response(request, &id),
            Some("shutdown") => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": serde_json::Value::Null,
            }),
            Some("textDocument/documentSymbol") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.document_symbol_result(request))
            }
            Some("textDocument/codeLens") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.code_lens_result(request))
            }
            Some("textDocument/codeAction") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.code_action_result(request))
            }
            Some("textDocument/documentLink") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.document_link_result(request))
            }
            Some("textDocument/foldingRange") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.folding_range_result(request))
            }
            Some("textDocument/selectionRange") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.selection_range_result(request))
            }
            Some("textDocument/semanticTokens/full") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.semantic_tokens_result(request))
            }
            Some("textDocument/diagnostic") => lsp_jsonrpc_result_or_invalid_params(
                &id,
                self.text_document_diagnostic_result(request),
            ),
            Some("workspace/diagnostic") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.workspace_diagnostic_result())
            }
            Some("workspace/executeCommand") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.execute_command_result(request))
            }
            Some(
                method @ ("textDocument/definition"
                | "textDocument/declaration"
                | "textDocument/implementation"
                | "textDocument/typeDefinition"
                | "textDocument/moniker"),
            ) => lsp_jsonrpc_result_or_invalid_params(&id, self.navigation_result(method, request)),
            Some(
                method @ ("textDocument/prepareCallHierarchy"
                | "textDocument/prepareTypeHierarchy"
                | "callHierarchy/outgoingCalls"
                | "callHierarchy/incomingCalls"
                | "typeHierarchy/supertypes"
                | "typeHierarchy/subtypes"),
            ) => lsp_jsonrpc_result_or_invalid_params(&id, self.hierarchy_result(method, request)),
            Some(method @ ("textDocument/documentColor" | "textDocument/colorPresentation")) => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.color_result(method, request))
            }
            Some("textDocument/linkedEditingRange") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.linked_editing_range_result(request))
            }
            Some("textDocument/references") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.references_result(request))
            }
            Some("textDocument/documentHighlight") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.document_highlight_result(request))
            }
            Some("textDocument/prepareRename") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.prepare_rename_result(request))
            }
            Some("textDocument/rename") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.rename_result(request))
            }
            Some("textDocument/hover") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.hover_result(request))
            }
            Some("textDocument/signatureHelp") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.signature_help_result(request))
            }
            Some("textDocument/inlayHint") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.inlay_hint_result(request))
            }
            Some(
                method @ ("textDocument/formatting"
                | "textDocument/rangeFormatting"
                | "textDocument/onTypeFormatting"),
            ) => lsp_jsonrpc_result_or_invalid_params(&id, self.formatting_result(method, request)),
            Some("textDocument/completion") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.completion_result(request))
            }
            Some("workspace/symbol") => {
                lsp_jsonrpc_result_or_invalid_params(&id, self.workspace_symbol_result(request))
            }
            Some(method) => lsp_jsonrpc_method_not_found(&id, method),
            None => lsp_jsonrpc_error(&id, -32600, "invalid request"),
        }
    }

    fn color_result(
        &self,
        method: &str,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        match method {
            "textDocument/documentColor" => self.document_color_result(request),
            "textDocument/colorPresentation" => Self::color_presentation_result(request),
            _ => unreachable!("color method dispatch is filtered by jsonrpc_response"),
        }
    }

    fn formatting_result(
        &self,
        method: &str,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        match method {
            "textDocument/formatting" => self.document_formatting_result(request),
            "textDocument/rangeFormatting" => self.range_formatting_result(request),
            "textDocument/onTypeFormatting" => self.on_type_formatting_result(request),
            _ => unreachable!("formatting method dispatch is filtered by jsonrpc_response"),
        }
    }

    fn navigation_result(
        &self,
        method: &str,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        match method {
            "textDocument/definition"
            | "textDocument/declaration"
            | "textDocument/implementation" => self.definition_result(request),
            "textDocument/typeDefinition" => self.type_definition_result(request),
            "textDocument/moniker" => self.moniker_result(request),
            _ => unreachable!("navigation method dispatch is filtered by jsonrpc_response"),
        }
    }

    fn hierarchy_result(
        &self,
        method: &str,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        match method {
            "textDocument/prepareCallHierarchy" => self.prepare_call_hierarchy_result(request),
            "textDocument/prepareTypeHierarchy" => self.prepare_type_hierarchy_result(request),
            "callHierarchy/outgoingCalls" => self.call_hierarchy_outgoing_result(request),
            "callHierarchy/incomingCalls" => self.call_hierarchy_incoming_result(request),
            "typeHierarchy/supertypes" | "typeHierarchy/subtypes" => {
                Self::empty_type_hierarchy_result(request)
            }
            _ => unreachable!("hierarchy method dispatch is filtered by jsonrpc_response"),
        }
    }

    fn initialize_response(
        &mut self,
        request: &serde_json::Value,
        id: &serde_json::Value,
    ) -> serde_json::Value {
        self.handle_initialize(request);
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "serverInfo": {
                    "name": "orv-lsp",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "textDocumentSync": {
                        "openClose": true,
                        "change": 1,
                        "save": {
                            "includeText": true,
                        },
                    },
                    "documentSymbolProvider": true,
                    "codeLensProvider": {
                        "resolveProvider": false,
                    },
                    "codeActionProvider": {
                        "codeActionKinds": ["quickfix"],
                    },
                    "executeCommandProvider": {
                        "commands": ["orv.revealSourceNode", "orv.revealDiagnostic"],
                    },
                    "documentLinkProvider": {
                        "resolveProvider": false,
                    },
                    "foldingRangeProvider": true,
                    "selectionRangeProvider": true,
                    "semanticTokensProvider": {
                        "legend": {
                            "tokenTypes": ["namespace", "type", "function"],
                            "tokenModifiers": ["declaration"],
                        },
                        "full": true,
                        "range": false,
                    },
                    "workspaceSymbolProvider": true,
                    "definitionProvider": true,
                    "declarationProvider": true,
                    "typeDefinitionProvider": true,
                    "implementationProvider": true,
                    "monikerProvider": true,
                    "callHierarchyProvider": true,
                    "typeHierarchyProvider": true,
                    "colorProvider": true,
                    "linkedEditingRangeProvider": true,
                    "referencesProvider": true,
                    "documentHighlightProvider": true,
                    "renameProvider": {
                        "prepareProvider": true,
                    },
                    "hoverProvider": true,
                    "signatureHelpProvider": {
                        "triggerCharacters": ["(", ","],
                    },
                    "inlayHintProvider": true,
                    "documentFormattingProvider": true,
                    "documentRangeFormattingProvider": true,
                    "documentOnTypeFormattingProvider": {
                        "firstTriggerCharacter": "}",
                        "moreTriggerCharacter": ["{", "\n"],
                    },
                    "completionProvider": {
                        "triggerCharacters": ["@", ".", ":"],
                    },
                    "diagnosticProvider": {
                        "interFileDependencies": true,
                        "workspaceDiagnostics": true,
                    },
                },
            },
        })
    }

    fn handle_initialize(&mut self, request: &serde_json::Value) {
        let Some(root_uri) = request
            .pointer("/params/rootUri")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        if let Ok(path) = lsp_file_uri_path(root_uri) {
            self.workspace_root = Some(path);
        }
    }

    fn text_document_diagnostic_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let diagnostics = lsp_diagnostics_for_loaded_project(&loaded);
        Ok(serde_json::json!({
            "kind": "full",
            "items": diagnostics,
        }))
    }

    fn workspace_diagnostic_result(&self) -> anyhow::Result<serde_json::Value> {
        let root = self.workspace_root.as_ref().ok_or_else(|| {
            anyhow::anyhow!("initialize.params.rootUri is required before workspace/diagnostic")
        })?;
        let entry = project_entry_path(root)?;
        let loaded = self.loaded_project_for_path(&entry)?;
        Ok(serde_json::json!({
            "items": lsp_workspace_diagnostic_items_json(&loaded),
        }))
    }

    fn execute_command_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let command = request
            .pointer("/params/command")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("command must be a string"))?;
        match command {
            "orv.revealSourceNode" => self.execute_reveal_source_node(request),
            "orv.revealDiagnostic" => Ok(lsp_execute_reveal_diagnostic_json(request)),
            _ => Err(anyhow::anyhow!("unsupported LSP command `{command}`")),
        }
    }

    fn execute_reveal_source_node(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let node_id = request
            .pointer("/params/arguments/0")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("orv.revealSourceNode requires source node id"))?;
        let node_id = ProjectNodeId::try_from(node_id)
            .map_err(|_| anyhow::anyhow!("source node id is too large"))?;
        let root = self.workspace_root.as_ref().ok_or_else(|| {
            anyhow::anyhow!("initialize.params.rootUri is required before workspace/executeCommand")
        })?;
        let entry = project_entry_path(root)?;
        let loaded = self.loaded_project_for_path(&entry)?;
        let node = loaded
            .graph
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .ok_or_else(|| anyhow::anyhow!("unknown source node `{node_id}`"))?;
        Ok(serde_json::json!({
            "command": "orv.revealSourceNode",
            "source_node": node.id,
            "name": node.name,
            "kind": lsp_symbol_kind(node.kind).unwrap_or("Symbol"),
            "location": lsp_location_json(node, &loaded.files),
        }))
    }

    fn definition_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Null);
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some(name) = identifier_at_byte(&file.source, byte) else {
            return Ok(serde_json::Value::Null);
        };
        let Some(node) = lsp_definition_node(&loaded.graph, name) else {
            return Ok(serde_json::Value::Null);
        };
        Ok(lsp_location_json(node, &loaded.files))
    }

    fn type_definition_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Null);
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some(name) = identifier_at_byte(&file.source, byte) else {
            return Ok(serde_json::Value::Null);
        };
        let Some(node) = lsp_type_definition_node(&loaded.graph, name) else {
            return Ok(serde_json::Value::Null);
        };
        Ok(lsp_location_json(node, &loaded.files))
    }

    fn document_color_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        Ok(serde_json::Value::Array(lsp_document_colors_json(
            &file.source,
        )))
    }

    fn color_presentation_result(request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let _uri = lsp_text_document_uri(request)?;
        let range = request
            .pointer("/params/range")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("range must be an object"))?;
        let (red, green, blue, alpha) = lsp_color_param(request)?;
        let label = lsp_hex_color_label(red, green, blue, alpha);
        Ok(serde_json::Value::Array(vec![serde_json::json!({
            "label": label,
            "textEdit": {
                "range": range,
                "newText": label,
            },
        })]))
    }

    fn moniker_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Null);
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some(name) = identifier_at_byte(&file.source, byte) else {
            return Ok(serde_json::Value::Null);
        };
        let Some(node) = lsp_definition_node(&loaded.graph, name) else {
            return Ok(serde_json::Value::Null);
        };
        Ok(serde_json::Value::Array(vec![lsp_moniker_json(node)]))
    }

    fn prepare_call_hierarchy_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Null);
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some(name) = identifier_at_byte(&file.source, byte) else {
            return Ok(serde_json::Value::Null);
        };
        let Some(function) = lsp_function_stmt_by_name(&loaded.program, name) else {
            return Ok(serde_json::Value::Null);
        };
        Ok(serde_json::Value::Array(vec![
            lsp_call_hierarchy_item_json(function, &loaded.files),
        ]))
    }

    fn prepare_type_hierarchy_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Null);
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some(name) = identifier_at_byte(&file.source, byte) else {
            return Ok(serde_json::Value::Null);
        };
        let Some(node) = lsp_type_definition_node(&loaded.graph, name) else {
            return Ok(serde_json::Value::Null);
        };
        Ok(serde_json::Value::Array(vec![
            lsp_type_hierarchy_item_json(node, &loaded.files),
        ]))
    }

    fn empty_type_hierarchy_result(
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        request
            .pointer("/params/item/name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("typeHierarchy item.name must be a string"))?;
        Ok(serde_json::Value::Array(Vec::new()))
    }

    fn call_hierarchy_outgoing_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let (loaded, caller_name) = self.loaded_project_for_call_hierarchy_item(request)?;
        let Some(caller) = lsp_function_stmt_by_name(&loaded.program, caller_name) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        Ok(serde_json::Value::Array(lsp_call_hierarchy_outgoing_calls(
            caller,
            &loaded.program,
            &loaded.files,
        )))
    }

    fn call_hierarchy_incoming_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let (loaded, callee_name) = self.loaded_project_for_call_hierarchy_item(request)?;
        Ok(serde_json::Value::Array(lsp_call_hierarchy_incoming_calls(
            callee_name,
            &loaded.program,
            &loaded.files,
        )))
    }

    fn loaded_project_for_call_hierarchy_item<'a>(
        &self,
        request: &'a serde_json::Value,
    ) -> anyhow::Result<(orv_project::LoadedProject, &'a str)> {
        let name = request
            .pointer("/params/item/name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("callHierarchy item.name must be a string"))?;
        let uri = request
            .pointer("/params/item/uri")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("callHierarchy item.uri must be a string"))?;
        let path = lsp_file_uri_path(uri)?;
        Ok((self.loaded_project_for_path(&path)?, name))
    }

    fn linked_editing_range_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Null);
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some((_, _, name)) = lsp_renamable_identifier_span_at_byte(&file.source, byte) else {
            return Ok(serde_json::Value::Null);
        };
        Ok(lsp_linked_editing_range_json(&file.source, name))
    }

    fn references_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        let byte = lsp_position_to_byte(&file.source, position);
        if let Some(field) = lsp_domain_field_at_byte(&file.source, byte) {
            return Ok(serde_json::Value::Array(
                lsp_domain_field_reference_locations_json(&loaded.files, field.kind, field.name),
            ));
        }
        let Some((_, _, name)) = lsp_renamable_identifier_span_at_byte(&file.source, byte) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        Ok(serde_json::Value::Array(lsp_reference_locations_json(
            &loaded.files,
            name,
        )))
    }

    fn document_highlight_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        let byte = lsp_position_to_byte(&file.source, position);
        if let Some(field) = lsp_domain_field_at_byte(&file.source, byte) {
            return Ok(serde_json::Value::Array(
                lsp_domain_field_occurrences(&file.source, field.kind, field.name)
                    .into_iter()
                    .map(|(start, end)| {
                        serde_json::json!({
                            "range": lsp_range_for_source(
                                &file.source,
                                u32::try_from(start).unwrap_or(u32::MAX),
                                u32::try_from(end).unwrap_or(u32::MAX),
                            ),
                            "kind": 1,
                        })
                    })
                    .collect(),
            ));
        }
        let Some((_, _, name)) = lsp_renamable_identifier_span_at_byte(&file.source, byte) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        Ok(serde_json::Value::Array(
            identifier_occurrences(&file.source, name)
                .into_iter()
                .map(|(start, end)| {
                    serde_json::json!({
                        "range": lsp_range_for_source(
                            &file.source,
                            u32::try_from(start).unwrap_or(u32::MAX),
                            u32::try_from(end).unwrap_or(u32::MAX),
                        ),
                        "kind": 1,
                    })
                })
                .collect(),
        ))
    }

    fn prepare_rename_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Null);
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some((start, end, name)) = lsp_renamable_identifier_span_at_byte(&file.source, byte)
        else {
            return Ok(serde_json::Value::Null);
        };
        Ok(serde_json::json!({
            "range": lsp_range_for_source(
                &file.source,
                u32::try_from(start).unwrap_or(u32::MAX),
                u32::try_from(end).unwrap_or(u32::MAX),
            ),
            "placeholder": name,
        }))
    }

    fn rename_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let new_name = request
            .pointer("/params/newName")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("newName must be a string"))?;
        if !lsp_renamable_identifier_name(new_name) {
            return Err(anyhow::anyhow!(
                "newName must be a valid non-keyword identifier"
            ));
        }
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::json!({ "changes": {} }));
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some((_, _, name)) = lsp_renamable_identifier_span_at_byte(&file.source, byte) else {
            return Ok(serde_json::json!({ "changes": {} }));
        };
        let mut changes = serde_json::Map::new();
        for file in &loaded.files {
            let edits: Vec<_> = identifier_occurrences(&file.source, name)
                .into_iter()
                .map(|(start, end)| {
                    serde_json::json!({
                        "range": lsp_range_for_source(
                            &file.source,
                            u32::try_from(start).unwrap_or(u32::MAX),
                            u32::try_from(end).unwrap_or(u32::MAX),
                        ),
                        "newText": new_name,
                    })
                })
                .collect();
            if !edits.is_empty() {
                changes.insert(
                    lsp_file_uri_for_path(&file.path),
                    serde_json::Value::Array(edits),
                );
            }
        }
        Ok(serde_json::json!({ "changes": changes }))
    }

    fn hover_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Null);
        };
        let byte = lsp_position_to_byte(&file.source, position);
        if let Some(hover) = lsp_domain_field_hover_json(&file.source, byte) {
            return Ok(hover);
        }
        let Some(name) = identifier_at_byte(&file.source, byte) else {
            return Ok(serde_json::Value::Null);
        };
        let Some(node) = lsp_definition_node(&loaded.graph, name) else {
            return Ok(serde_json::Value::Null);
        };
        Ok(lsp_hover_json(node, &loaded.files))
    }

    fn signature_help_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Null);
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some((name, active_parameter)) = lsp_call_signature_context(&file.source, byte) else {
            return Ok(serde_json::Value::Null);
        };
        let Some(function) = lsp_function_stmt_by_name(&loaded.program, name) else {
            return Ok(serde_json::Value::Null);
        };
        Ok(lsp_signature_help_json(function, active_parameter))
    }

    fn inlay_hint_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let requested_range = lsp_request_range(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        let start = lsp_position_to_byte(&file.source, requested_range.0);
        let end = lsp_position_to_byte(&file.source, requested_range.1);
        Ok(serde_json::Value::Array(lsp_inlay_hints_json(
            &loaded.program,
            &file.source,
            start,
            end,
        )))
    }

    fn document_formatting_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        let formatted = lsp_format_source(
            &file.source,
            lsp_formatting_tab_size(request),
            lsp_formatting_insert_spaces(request),
        );
        if formatted == file.source {
            return Ok(serde_json::Value::Array(Vec::new()));
        }
        Ok(serde_json::Value::Array(vec![serde_json::json!({
            "range": lsp_full_document_range(&file.source),
            "newText": formatted,
        })]))
    }

    fn range_formatting_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let requested_range = lsp_request_range(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        let (start, end, edit_range) = lsp_line_range_for_formatting(&file.source, requested_range);
        let Some(source_slice) = file.source.get(start..end) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        let formatted = lsp_format_source_with_initial_indent(
            source_slice,
            lsp_formatting_tab_size(request),
            lsp_formatting_insert_spaces(request),
            lsp_indent_level_before(&file.source, start),
        );
        if formatted == source_slice {
            return Ok(serde_json::Value::Array(Vec::new()));
        }
        Ok(serde_json::Value::Array(vec![serde_json::json!({
            "range": edit_range,
            "newText": formatted,
        })]))
    }

    fn on_type_formatting_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let trigger = request
            .pointer("/params/ch")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("onTypeFormatting ch must be a string"))?;
        if !matches!(trigger, "}" | "{" | "\n") {
            return Ok(serde_json::Value::Array(Vec::new()));
        }
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        if trigger == "\n" {
            return Ok(lsp_newline_on_type_formatting_edit_json(
                &file.source,
                position.0,
                lsp_formatting_tab_size(request),
                lsp_formatting_insert_spaces(request),
            ));
        }
        let (start, end, edit_range) =
            lsp_current_line_range_for_formatting(&file.source, position.0);
        let Some(source_slice) = file.source.get(start..end) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        let formatted = lsp_format_source_with_initial_indent(
            source_slice,
            lsp_formatting_tab_size(request),
            lsp_formatting_insert_spaces(request),
            lsp_indent_level_before(&file.source, start),
        );
        if formatted == source_slice {
            return Ok(serde_json::Value::Array(Vec::new()));
        }
        Ok(serde_json::Value::Array(vec![serde_json::json!({
            "range": edit_range,
            "newText": formatted,
        })]))
    }

    fn document_symbol_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let loaded = self.loaded_project_for_path(&path)?;
        Ok(serde_json::Value::Array(
            lsp_document_symbols_protocol_json(&loaded.graph, &loaded.files),
        ))
    }

    fn code_lens_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        Ok(serde_json::Value::Array(lsp_code_lenses_json(
            &loaded.graph,
            &loaded.files,
            file.id,
        )))
    }

    fn code_action_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let requested_range = lsp_request_range(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        let start = lsp_position_to_byte(&file.source, requested_range.0);
        let end = lsp_position_to_byte(&file.source, requested_range.1);
        Ok(serde_json::Value::Array(lsp_code_actions_json(
            &loaded, file, start, end,
        )))
    }

    fn document_link_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        Ok(serde_json::Value::Array(lsp_document_links_json(
            &loaded.graph,
            &loaded.files,
            file.id,
        )))
    }

    fn folding_range_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        Ok(serde_json::Value::Array(lsp_folding_ranges_json(
            &loaded.graph,
            &loaded.files,
            file.id,
        )))
    }

    fn selection_range_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        let positions = request
            .pointer("/params/positions")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("positions must be an array"))?;
        let mut ranges = Vec::with_capacity(positions.len());
        for position in positions {
            let position = lsp_position_value(position)?;
            let byte = lsp_position_to_byte(&file.source, position);
            ranges.push(
                lsp_selection_range_json(&loaded.graph, &loaded.files, file.id, byte)
                    .unwrap_or_else(|| {
                        let byte = u32::try_from(byte).unwrap_or(u32::MAX);
                        serde_json::json!({
                            "range": lsp_range_for_source(&file.source, byte, byte),
                        })
                    }),
            );
        }
        Ok(serde_json::Value::Array(ranges))
    }

    fn semantic_tokens_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::json!({ "data": [] }));
        };
        Ok(lsp_semantic_tokens_json(
            &loaded.graph,
            &loaded.files,
            file.id,
        ))
    }

    fn completion_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let context = if let Some(file) = lsp_source_file_for_path(&loaded.files, &path) {
            let position = lsp_text_document_position(request)?;
            let byte = lsp_position_to_byte(&file.source, position);
            lsp_completion_context(&file.source, byte)
        } else {
            LspCompletionContext::General
        };
        Ok(serde_json::json!({
            "isIncomplete": false,
            "items": lsp_completion_items_json(&loaded.graph, &loaded.files, context),
        }))
    }

    fn workspace_symbol_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let query = request
            .pointer("/params/query")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let root = self.workspace_root.as_ref().ok_or_else(|| {
            anyhow::anyhow!("initialize.params.rootUri is required before workspace/symbol")
        })?;
        let entry = project_entry_path(root)?;
        let loaded = self.loaded_project_for_path(&entry)?;
        Ok(serde_json::Value::Array(lsp_workspace_symbols_json(
            &loaded.graph,
            &loaded.files,
            query,
        )))
    }

    fn loaded_project_for_path(&self, path: &Path) -> anyhow::Result<orv_project::LoadedProject> {
        if let Some(source) = self.open_documents.get(path) {
            return orv_project::load_project_from_sources(
                path,
                [(path.to_path_buf(), source.clone())],
            )
            .map_err(|e| anyhow::anyhow!("{e}"));
        }
        orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub(crate) fn handle_notification(&mut self, request: &serde_json::Value) {
        match request.get("method").and_then(serde_json::Value::as_str) {
            Some("textDocument/didOpen") => self.handle_did_open(request),
            Some("textDocument/didChange") => self.handle_did_change(request),
            Some("textDocument/didSave") => self.handle_did_save(request),
            Some("textDocument/didClose") => self.handle_did_close(request),
            _ => {}
        }
    }

    fn handle_did_open(&mut self, request: &serde_json::Value) {
        let Some(uri) = request
            .pointer("/params/textDocument/uri")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let Some(text) = request
            .pointer("/params/textDocument/text")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let Ok(path) = lsp_file_uri_path(uri) else {
            return;
        };
        self.open_documents.insert(path, text.to_string());
    }

    fn handle_did_close(&mut self, request: &serde_json::Value) {
        let Some(uri) = request
            .pointer("/params/textDocument/uri")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let Ok(path) = lsp_file_uri_path(uri) else {
            return;
        };
        self.open_documents.remove(&path);
    }

    fn handle_did_save(&mut self, request: &serde_json::Value) {
        let Some(uri) = request
            .pointer("/params/textDocument/uri")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let Ok(path) = lsp_file_uri_path(uri) else {
            return;
        };
        let Some(text) = request
            .pointer("/params/text")
            .and_then(serde_json::Value::as_str)
        else {
            self.open_documents.remove(&path);
            return;
        };
        self.open_documents.insert(path, text.to_string());
    }

    fn handle_did_change(&mut self, request: &serde_json::Value) {
        let Some(uri) = request
            .pointer("/params/textDocument/uri")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let Some(text) = request
            .pointer("/params/contentChanges")
            .and_then(serde_json::Value::as_array)
            .and_then(|changes| changes.last())
            .and_then(|change| change.get("text"))
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let Ok(path) = lsp_file_uri_path(uri) else {
            return;
        };
        self.open_documents.insert(path, text.to_string());
    }
}

#[cfg(test)]
pub(crate) fn dap_protocol_response(request: &serde_json::Value) -> serde_json::Value {
    DapSession::default()
        .message_response(request)
        .expect("DAP response")
}

#[derive(Default)]
pub(crate) struct DapSession {
    pub(crate) next_seq: u64,
    pub(crate) launched: Option<DapLaunchState>,
    pub(crate) breakpoints: HashMap<PathBuf, Vec<DapBreakpoint>>,
    pub(crate) function_breakpoints: Vec<DapFunctionBreakpoint>,
    pub(crate) instruction_breakpoints: Vec<DapInstructionBreakpoint>,
    pub(crate) data_breakpoints: Vec<DapDataBreakpoint>,
    pub(crate) exception_filters: Option<HashSet<String>>,
    pub(crate) pending_events: Vec<DapPendingEvent>,
}

pub(crate) struct DapLaunchState {
    pub(crate) path: PathBuf,
    pub(crate) uri: String,
    pub(crate) name: String,
    pub(crate) source_bundle: Option<DapLaunchSourceBundle>,
    pub(crate) program: orv_hir::HirProgram,
    pub(crate) node_count: usize,
    pub(crate) diagnostic_count: usize,
    pub(crate) stopped_line: u64,
    pub(crate) stopped_reason: String,
    pub(crate) executable_lines: Vec<u64>,
    pub(crate) runtime: DapRuntimeState,
    pub(crate) sources: Vec<DapSourceInfo>,
    pub(crate) files: Vec<SourceFile>,
    pub(crate) frames: Vec<DapFrameState>,
    pub(crate) current_frame_index: usize,
    pub(crate) live_requested: bool,
    pub(crate) live: Option<DapLiveState>,
    pub(crate) long_running: bool,
    pub(crate) attach_runtime_requested: bool,
    pub(crate) attach_runtime_mode: DapRuntimeAttachMode,
    pub(crate) runtime_request_trace_path: Option<PathBuf>,
    pub(crate) runtime_process: Option<DapRuntimeProcess>,
    pub(crate) attached_server: Option<orv_runtime::server::AttachedServer>,
    pub(crate) async_runtime: Option<DapAsyncRuntimeState>,
}

#[derive(Clone)]
pub(crate) struct DapLaunchSourceBundle {
    pub(crate) path: PathBuf,
    pub(crate) entry: PathBuf,
    pub(crate) file_count: usize,
    pub(crate) hash: String,
}

pub(crate) struct DapLaunchProject {
    pub(crate) loaded: orv_project::LoadedProject,
    pub(crate) entry_path_for_lookup: PathBuf,
    pub(crate) source_bundle: Option<DapLaunchSourceBundle>,
}

pub(crate) struct DapPendingEvent {
    pub(crate) event: String,
    pub(crate) body: serde_json::Value,
}

pub(crate) struct DapLiveState {
    pub(crate) stepper: orv_runtime::DebugStepper<Vec<u8>>,
}

pub(crate) struct DapRuntimeProcess {
    pub(crate) child: Child,
}

impl DapRuntimeProcess {
    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for DapRuntimeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for DapLaunchState {
    fn drop(&mut self) {
        self.attached_server = None;
        self.runtime_process = None;
    }
}

impl DapLaunchState {
    fn ensure_runtime_process_running(&mut self) -> anyhow::Result<()> {
        if !self.attach_runtime_requested {
            return Ok(());
        }
        match self.attach_runtime_mode {
            DapRuntimeAttachMode::Process => self.ensure_child_runtime_process_running(),
            DapRuntimeAttachMode::InProcess => self.ensure_in_process_runtime_running(),
        }
    }

    fn ensure_child_runtime_process_running(&mut self) -> anyhow::Result<()> {
        if let Some(process) = self.runtime_process.as_mut() {
            if let Some(status) = process.child.try_wait()? {
                let pid = process.pid();
                self.runtime_process = None;
                self.set_transport_state("exited", Some(pid), None);
                anyhow::bail!("runtime process exited with {status}");
            }
            let pid = process.pid();
            dap_send_process_signal(pid, "CONT")?;
            self.set_transport_state("running", Some(pid), None);
            return Ok(());
        }

        let exe =
            std::env::current_exe().map_err(|e| anyhow::anyhow!("current_exe failed: {e}"))?;
        let child = ProcessCommand::new(&exe)
            .arg("run")
            .arg(&self.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to start runtime process: {e}"))?;
        let pid = child.id();
        self.runtime_process = Some(DapRuntimeProcess { child });
        self.set_transport_state("running", Some(pid), None);
        Ok(())
    }

    fn ensure_in_process_runtime_running(&mut self) -> anyhow::Result<()> {
        if let Some(server) = &self.attached_server {
            self.set_transport_state("running", None, Some(server.addr().to_string()));
            return Ok(());
        }
        let server = orv_runtime::server::spawn_attached_server(self.program.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let address = server.addr().to_string();
        self.attached_server = Some(server);
        self.set_transport_state("running", None, Some(address));
        Ok(())
    }

    fn suspend_runtime_process(&mut self) -> anyhow::Result<()> {
        if !self.attach_runtime_requested {
            return Ok(());
        }
        match self.attach_runtime_mode {
            DapRuntimeAttachMode::Process => self.suspend_child_runtime_process(),
            DapRuntimeAttachMode::InProcess => {
                self.suspend_in_process_runtime();
                Ok(())
            }
        }
    }

    fn suspend_child_runtime_process(&mut self) -> anyhow::Result<()> {
        let Some(process) = self.runtime_process.as_mut() else {
            self.set_transport_state("detached", None, None);
            return Ok(());
        };
        if let Some(status) = process.child.try_wait()? {
            let pid = process.pid();
            self.runtime_process = None;
            self.set_transport_state("exited", Some(pid), None);
            anyhow::bail!("runtime process exited with {status}");
        }
        let pid = process.pid();
        dap_send_process_signal(pid, "STOP")?;
        self.set_transport_state("suspended", Some(pid), None);
        Ok(())
    }

    fn suspend_in_process_runtime(&mut self) {
        let address = self
            .attached_server
            .as_ref()
            .map(|server| server.addr().to_string());
        self.attached_server = None;
        self.set_transport_state("suspended", None, address);
    }

    fn set_transport_state(
        &mut self,
        state: &str,
        process_id: Option<u32>,
        address: Option<String>,
    ) {
        let Some(async_runtime) = self.async_runtime.as_mut() else {
            return;
        };
        let transport = async_runtime
            .transport
            .get_or_insert_with(DapAsyncTransportState::process_detached);
        transport.state = state.to_string();
        transport.process_id = process_id.map(u64::from);
        transport.address = address;
    }

    fn write_runtime_request_trace_file(&self) -> anyhow::Result<()> {
        let Some(path) = &self.runtime_request_trace_path else {
            return Ok(());
        };
        let frames = self.attached_server.as_ref().map_or_else(
            Vec::new,
            orv_runtime::server::AttachedServer::request_frames,
        );
        orv_runtime::server::write_request_trace_file(path, &frames)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum DapRuntimeAttachMode {
    Process,
    InProcess,
}

impl DapRuntimeAttachMode {
    const fn protocol_name(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::InProcess => "inProcess",
        }
    }
}

pub(crate) fn dap_send_process_signal(pid: u32, signal: &str) -> anyhow::Result<()> {
    let status = ProcessCommand::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()
        .map_err(|e| anyhow::anyhow!("failed to signal runtime process {pid}: {e}"))?;
    if !status.success() {
        anyhow::bail!("failed to signal runtime process {pid} with {signal}: {status}");
    }
    Ok(())
}

pub(crate) enum DapLiveAdvance {
    Frame { index: usize, output: String },
    Skipped,
    Done,
    Error { message: String },
}

#[derive(Clone)]
pub(crate) struct DapSourceInfo {
    pub(crate) reference: u64,
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) uri: String,
    pub(crate) checksum: String,
}

#[derive(Clone)]
pub(crate) struct DapBreakpoint {
    pub(crate) id: u64,
    pub(crate) line: u64,
    pub(crate) verified: bool,
    pub(crate) condition: Option<String>,
    pub(crate) hit_condition: Option<String>,
    pub(crate) log_message: Option<String>,
    pub(crate) message: Option<String>,
}

#[derive(Clone)]
pub(crate) struct DapFunctionBreakpoint {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) verified: bool,
    pub(crate) message: Option<String>,
}

#[derive(Clone)]
pub(crate) struct DapDataBreakpoint {
    pub(crate) id: u64,
    pub(crate) data_id: String,
    pub(crate) verified: bool,
    pub(crate) message: Option<String>,
}

#[derive(Clone)]
pub(crate) struct DapInstructionBreakpoint {
    pub(crate) id: u64,
    pub(crate) instruction_reference: String,
    pub(crate) offset: i64,
    pub(crate) frame_index: Option<usize>,
    pub(crate) verified: bool,
    pub(crate) message: Option<String>,
}

#[derive(Clone)]
pub(crate) struct DapRuntimeState {
    pub(crate) status: String,
    pub(crate) stdout: String,
    pub(crate) error: String,
}

#[derive(Clone)]
pub(crate) struct DapAsyncRuntimeState {
    pub(crate) kind: String,
    pub(crate) state: String,
    pub(crate) resume_count: u64,
    pub(crate) pause_count: u64,
    pub(crate) listen: Option<DapAsyncListenState>,
    pub(crate) routes: Vec<DapAsyncRouteState>,
    pub(crate) transport: Option<DapAsyncTransportState>,
}

#[derive(Clone)]
pub(crate) struct DapAsyncRouteState {
    pub(crate) method: String,
    pub(crate) path: String,
}

#[derive(Clone)]
pub(crate) struct DapAsyncTransportState {
    pub(crate) kind: String,
    pub(crate) state: String,
    pub(crate) process_id: Option<u64>,
    pub(crate) address: Option<String>,
}

#[derive(Clone)]
pub(crate) struct DapAsyncListenState {
    pub(crate) kind: String,
    pub(crate) display: String,
    pub(crate) port: Option<u64>,
    pub(crate) variable: Option<String>,
    pub(crate) default_port: Option<u64>,
}

impl DapAsyncRuntimeState {
    fn server(listen: Option<DapAsyncListenState>, routes: Vec<DapAsyncRouteState>) -> Self {
        Self {
            kind: "server".to_string(),
            state: "paused".to_string(),
            resume_count: 0,
            pause_count: 0,
            listen,
            routes,
            transport: None,
        }
    }
}

impl DapAsyncTransportState {
    fn process_detached() -> Self {
        Self {
            kind: "process".to_string(),
            state: "detached".to_string(),
            process_id: None,
            address: None,
        }
    }

    fn in_process_detached() -> Self {
        Self {
            kind: "in-process".to_string(),
            state: "detached".to_string(),
            process_id: None,
            address: None,
        }
    }
}

pub(crate) fn dap_attach_runtime_transport_if_requested(
    async_runtime: &mut Option<DapAsyncRuntimeState>,
    attach_runtime_requested: bool,
    attach_runtime_mode: DapRuntimeAttachMode,
) {
    if !attach_runtime_requested {
        return;
    }
    let Some(async_runtime) = async_runtime.as_mut() else {
        return;
    };
    async_runtime.transport = Some(match attach_runtime_mode {
        DapRuntimeAttachMode::Process => DapAsyncTransportState::process_detached(),
        DapRuntimeAttachMode::InProcess => DapAsyncTransportState::in_process_detached(),
    });
}

#[derive(Clone)]
pub(crate) struct DapVariable {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) value_type: String,
    pub(crate) line: u64,
    pub(crate) variables_reference: u64,
}

#[derive(Clone)]
pub(crate) struct DapFrameState {
    pub(crate) source: DapSourceInfo,
    pub(crate) line: u64,
    pub(crate) locals: Vec<DapVariable>,
    pub(crate) stack: Vec<DapStackFrameState>,
    pub(crate) output: String,
}

#[derive(Clone)]
pub(crate) struct DapStackFrameState {
    pub(crate) name: String,
    pub(crate) source: DapSourceInfo,
    pub(crate) line: u64,
}

impl DapSession {
    pub(crate) fn message_response(
        &mut self,
        request: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        if request.get("type").and_then(serde_json::Value::as_str) != Some("request") {
            return None;
        }
        let seq = self.next_response_seq();
        let request_seq = request
            .get("seq")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let command = request
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let result = match command {
            "initialize" => {
                self.queue_event("initialized", serde_json::json!({}));
                Ok(serde_json::json!({
                    "supportsConfigurationDoneRequest": true,
                    "supportsTerminateRequest": true,
                    "supportsTerminateThreadsRequest": true,
                    "supportsLoadedSourcesRequest": true,
                    "supportsEvaluateForHovers": true,
                    "supportsCompletionsRequest": true,
                    "supportsBreakpointLocationsRequest": true,
                    "supportsConditionalBreakpoints": true,
                    "supportsHitConditionalBreakpoints": true,
                    "supportsFunctionBreakpoints": true,
                    "supportsDataBreakpoints": true,
                    "supportsExceptionInfoRequest": true,
                    "supportsRestartRequest": true,
                    "supportsSetVariable": true,
                    "supportsSetExpression": true,
                    "supportsModulesRequest": true,
                    "supportsGotoTargetsRequest": true,
                    "supportsStepBack": true,
                    "supportsStepInTargetsRequest": true,
                    "supportsRestartFrame": true,
                    "supportsPauseRequest": true,
                    "supportsCancelRequest": true,
                    "supportsInstructionBreakpoints": true,
                    "supportsDisassembleRequest": true,
                    "supportsReadMemoryRequest": true,
                    "supportsOrvRuntimeAttach": true,
                    "supportsOrvRuntimeTracePath": true,
                    "supportsOrvSourceBundleLaunch": true,
                    "exceptionBreakpointFilters": [
                        {
                            "filter": "orv.diagnostics",
                            "label": "ORV diagnostics",
                            "default": true,
                        },
                        {
                            "filter": "orv.runtime",
                            "label": "ORV runtime errors",
                            "default": true,
                        },
                    ],
                }))
            }
            "launch" => self.launch_result(request),
            "attach" => self.attach_result(request),
            "restart" => self.restart_result(request),
            "configurationDone" => self.configuration_done_result(),
            "cancel" => Ok(serde_json::json!({})),
            "setExceptionBreakpoints" => self.set_exception_breakpoints_result(request),
            "setBreakpoints" => self.set_breakpoints_result(request),
            "setFunctionBreakpoints" => self.set_function_breakpoints_result(request),
            "setInstructionBreakpoints" => self.set_instruction_breakpoints_result(request),
            "dataBreakpointInfo" => self.data_breakpoint_info_result(request),
            "setDataBreakpoints" => self.set_data_breakpoints_result(request),
            "breakpointLocations" => self.breakpoint_locations_result(request),
            "gotoTargets" => self.goto_targets_result(request),
            "threads" => Ok(serde_json::json!({
                "threads": [
                    {
                        "id": 1,
                        "name": "orv reference runtime",
                    },
                ],
            })),
            "stackTrace" => self.stack_trace_result(request),
            "scopes" => self.scopes_result(request),
            "variables" => self.variables_result(request),
            "setVariable" => self.set_variable_result(request),
            "evaluate" => self.evaluate_result(request),
            "setExpression" => self.set_expression_result(request),
            "completions" => self.completions_result(request),
            "exceptionInfo" => self.exception_info_result(request),
            "loadedSources" => self.loaded_sources_result(),
            "modules" => self.modules_result(request),
            "source" => self.source_result(request),
            "disassemble" => self.disassemble_result(request),
            "readMemory" => self.read_memory_result(request),
            "continue" => self.continue_result(request),
            "reverseContinue" => self.reverse_continue_result(request),
            "goto" => self.goto_result(request),
            "stepBack" => self.step_back_result(request),
            "restartFrame" => self.restart_frame_result(request),
            "next" => self.next_result(request),
            "stepInTargets" => self.step_in_targets_result(request),
            "stepIn" => self.step_in_result(request),
            "stepOut" => self.step_out_result(request),
            "pause" => self.pause_result(request),
            "terminateThreads" => self.terminate_threads_result(request),
            "disconnect" | "terminate" => {
                let flush = self
                    .launched
                    .as_ref()
                    .map_or_else(|| Ok(()), DapLaunchState::write_runtime_request_trace_file);
                flush.map(|()| {
                    self.queue_event("terminated", serde_json::json!({}));
                    self.launched = None;
                    serde_json::json!({})
                })
            }
            _ => Err(anyhow::anyhow!("unsupported DAP command `{command}`")),
        };
        Some(match result {
            Ok(body) => dap_success_response(seq, request_seq, command, &body),
            Err(err) => dap_error_response(seq, request_seq, command, &err.to_string()),
        })
    }

    const fn next_response_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    fn require_reference_thread(request: &serde_json::Value, command: &str) -> anyhow::Result<()> {
        let thread_id = request
            .pointer("/arguments/threadId")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("{command}.arguments.threadId is required"))?;
        if thread_id != 1 {
            anyhow::bail!("unknown ORV thread id {thread_id}");
        }
        Ok(())
    }

    fn launch_result(&mut self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let path = dap_program_path(request)?;
        let project = dap_loaded_project_for_launch(request, &path)?;
        let DapLaunchProject {
            loaded,
            entry_path_for_lookup,
            source_bundle,
        } = project;
        let file = lsp_source_file_for_path(&loaded.files, &entry_path_for_lookup)
            .or_else(|| lsp_source_file_for_path(&loaded.files, &path))
            .ok_or_else(|| anyhow::anyhow!("launch program is not part of loaded project"))?;
        let resolved = orv_resolve::resolve(&loaded.program);
        let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
        let diagnostic_count =
            loaded.diagnostics.len() + resolved.diagnostics.len() + lowered.diagnostics.len();
        let entry_path = file.path.clone();
        let entry_uri = lsp_file_uri_for_path(&entry_path);
        let entry_name = entry_path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("app.orv")
            .to_string();
        let sources: Vec<DapSourceInfo> = loaded
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                dap_source_info(file, u64::try_from(index + 1).unwrap_or(u64::MAX))
            })
            .collect();
        let live_requested = dap_launch_live(request);
        let attach_runtime_requested = dap_launch_attach_runtime(request);
        let attach_runtime_mode = dap_launch_attach_runtime_mode(request)?;
        let runtime_request_trace_path = dap_launch_runtime_request_trace_path(request)?;
        let (runtime, mut frames, live, long_running) = dap_launch_runtime_state(
            &lowered,
            diagnostic_count,
            &loaded.files,
            &sources,
            live_requested,
        );
        let mut async_runtime = dap_async_runtime_state(&lowered.program, long_running);
        dap_attach_runtime_transport_if_requested(
            &mut async_runtime,
            attach_runtime_requested,
            attach_runtime_mode,
        );
        self.revalidate_instruction_breakpoints(frames.len());
        let executable_lines = dap_launch_executable_lines(&entry_path, &frames);
        let current_frame_index = self.first_verified_breakpoint_frame(&frames).unwrap_or(0);
        let stopped_line = frames
            .get(current_frame_index)
            .map_or(executable_lines[0], |frame| frame.line);
        let stopped_reason = self.launch_stopped_reason(&runtime, &frames, current_frame_index);
        let source_bundle_json = dap_launch_source_bundle_json(source_bundle.as_ref());
        self.launched = Some(DapLaunchState {
            path: entry_path.clone(),
            uri: entry_uri.clone(),
            name: entry_name.clone(),
            source_bundle,
            program: lowered.program,
            node_count: loaded.graph.nodes.len(),
            diagnostic_count,
            stopped_line,
            stopped_reason,
            executable_lines,
            runtime: runtime.clone(),
            sources,
            files: loaded.files.clone(),
            frames: std::mem::take(&mut frames),
            current_frame_index,
            live_requested,
            live,
            long_running,
            attach_runtime_requested,
            attach_runtime_mode,
            runtime_request_trace_path,
            runtime_process: None,
            attached_server: None,
            async_runtime: async_runtime.clone(),
        });
        if self
            .launched
            .as_ref()
            .is_some_and(|launched| !launched.frames.is_empty())
        {
            self.queue_frame_outputs(0, current_frame_index);
        } else if !runtime.stdout.is_empty() {
            self.queue_stdout_output(&runtime.stdout);
        }
        if !runtime.error.is_empty() {
            self.queue_event(
                "output",
                serde_json::json!({
                    "category": "stderr",
                    "output": runtime.error,
                }),
            );
        }
        Ok(serde_json::json!({
            "entry": {
                "name": entry_name,
                "path": entry_path.display().to_string(),
                "uri": entry_uri,
            },
            "projectGraphNodes": loaded.graph.nodes.len(),
            "sourceBundle": source_bundle_json,
            "diagnostics": diagnostic_count,
            "runtime": dap_runtime_json(&runtime, async_runtime.as_ref()),
        }))
    }

    fn attach_result(&mut self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let mut arguments = request
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let arguments_object = arguments
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("attach.arguments must be an object"))?;
        arguments_object.insert("attachRuntime".to_string(), serde_json::Value::Bool(true));
        self.launch_result(&serde_json::json!({
            "arguments": arguments,
        }))
    }

    fn launch_stopped_reason(
        &self,
        runtime: &DapRuntimeState,
        frames: &[DapFrameState],
        current_frame_index: usize,
    ) -> String {
        if self.exception_filter_enabled(runtime.status.as_str()) {
            "exception".to_string()
        } else if let Some(reason) = self.breakpoint_frame_reason(frames, current_frame_index) {
            reason.to_string()
        } else {
            "entry".to_string()
        }
    }

    fn set_exception_breakpoints_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let filters = request
            .pointer("/arguments/filters")
            .and_then(serde_json::Value::as_array)
            .map_or_else(HashSet::new, |filters| {
                filters
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter(|filter| matches!(*filter, "orv.diagnostics" | "orv.runtime"))
                    .map(str::to_string)
                    .collect()
            });
        self.exception_filters = Some(filters);
        Ok(dap_set_exception_breakpoints_result(request))
    }

    fn exception_filter_enabled(&self, runtime_status: &str) -> bool {
        let filter = match runtime_status {
            "diagnostics" => "orv.diagnostics",
            "error" => "orv.runtime",
            _ => return false,
        };
        self.exception_filters
            .as_ref()
            .is_none_or(|filters| filters.contains(filter))
    }

    fn configuration_done_result(&mut self) -> anyhow::Result<serde_json::Value> {
        self.require_launch("configurationDone")?;
        self.queue_stopped_event();
        Ok(serde_json::json!({}))
    }

    fn restart_result(&mut self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let live_requested = request
            .pointer("/arguments/live")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_else(|| {
                self.launched
                    .as_ref()
                    .is_some_and(|launched| launched.live_requested)
            });
        let attach_runtime_requested = request
            .pointer("/arguments/attachRuntime")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_else(|| {
                self.launched
                    .as_ref()
                    .is_some_and(|launched| launched.attach_runtime_requested)
            });
        let attach_runtime_mode = if request.pointer("/arguments/attachRuntimeMode").is_some() {
            dap_launch_attach_runtime_mode(request)?
        } else {
            self.launched
                .as_ref()
                .map_or(DapRuntimeAttachMode::Process, |launched| {
                    launched.attach_runtime_mode
                })
        };
        let runtime_request_trace_path =
            dap_launch_runtime_request_trace_path(request)?.or_else(|| {
                self.launched
                    .as_ref()
                    .and_then(|launched| launched.runtime_request_trace_path.clone())
            });
        let path = request
            .pointer("/arguments/program")
            .and_then(serde_json::Value::as_str)
            .map(dap_path_from_protocol_string)
            .transpose()?
            .or_else(|| self.launched.as_ref().map(|launched| launched.path.clone()))
            .ok_or_else(|| anyhow::anyhow!("launch is required before restart"))?;
        let has_program_override = request.pointer("/arguments/program").is_some();
        let source_bundle_path = dap_launch_source_bundle_path(request)?.or_else(|| {
            if has_program_override {
                None
            } else {
                self.launched
                    .as_ref()
                    .and_then(|launched| launched.source_bundle.as_ref())
                    .map(|source_bundle| source_bundle.path.clone())
            }
        });
        let mut arguments = serde_json::json!({
                "program": path.display().to_string(),
                "live": live_requested,
                "attachRuntime": attach_runtime_requested,
                "attachRuntimeMode": attach_runtime_mode.protocol_name(),
        });
        if let Some(path) = source_bundle_path {
            arguments["sourceBundle"] = serde_json::json!(path.display().to_string());
        }
        if let Some(path) = runtime_request_trace_path {
            arguments["runtimeRequestTracePath"] = serde_json::json!(path.display().to_string());
        }
        let restart_request = serde_json::json!({
            "arguments": arguments,
        });
        self.launch_result(&restart_request)
    }

    fn loaded_sources_result(&self) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before loadedSources"))?;
        Ok(serde_json::json!({
            "sources": launched
                .sources
                .iter()
                .map(dap_source_json)
                .collect::<Vec<_>>(),
        }))
    }

    fn modules_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before modules"))?;
        let start = request
            .pointer("/arguments/startModule")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0);
        let total = launched.sources.len();
        let available = total.saturating_sub(start);
        let module_count = request
            .pointer("/arguments/moduleCount")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(available);
        Ok(serde_json::json!({
            "modules": launched
                .sources
                .iter()
                .skip(start)
                .take(module_count)
                .map(dap_module_json)
                .collect::<Vec<_>>(),
            "totalModules": total,
        }))
    }

    fn source_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before source"))?;
        let source = if let Some(reference) = dap_source_reference(request) {
            launched
                .sources
                .iter()
                .find(|source| source.reference == reference)
                .ok_or_else(|| anyhow::anyhow!("unknown sourceReference {reference}"))?
        } else {
            let requested_path = dap_normalize_path(&dap_source_path(request)?);
            launched
                .sources
                .iter()
                .find(|source| dap_normalize_path(&source.path) == requested_path)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "source `{}` is not part of the launched project",
                        requested_path.display()
                    )
                })?
        };
        let content = launched
            .files
            .iter()
            .find(|file| dap_normalize_path(&file.path) == dap_normalize_path(&source.path))
            .map(|file| file.source.clone())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "source `{}` is not part of the loaded project snapshot",
                    source.path.display()
                )
            })?;
        Ok(serde_json::json!({
            "content": content,
            "mimeType": "text/x-orv",
        }))
    }

    fn disassemble_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before disassemble"))?;
        let memory_reference = request
            .pointer("/arguments/memoryReference")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("disassemble.arguments.memoryReference is required"))?;
        let instruction_offset = request
            .pointer("/arguments/instructionOffset")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let start = dap_disassemble_start_index(memory_reference, instruction_offset)?;
        let available = launched.frames.len().saturating_sub(start);
        let instruction_count = request
            .pointer("/arguments/instructionCount")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(available);
        Ok(serde_json::json!({
            "instructions": launched
                .frames
                .iter()
                .enumerate()
                .skip(start)
                .take(instruction_count)
                .map(|(index, frame)| dap_disassembled_instruction_json(index, frame))
                .collect::<Vec<_>>(),
        }))
    }

    fn read_memory_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before readMemory"))?;
        let memory_reference = request
            .pointer("/arguments/memoryReference")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("readMemory.arguments.memoryReference is required"))?;
        let frame_index = dap_memory_reference_frame_index(memory_reference, "readMemory")?;
        let frame = launched
            .frames
            .get(frame_index)
            .ok_or_else(|| anyhow::anyhow!("unknown ORV memoryReference `{memory_reference}`"))?;
        let offset = request
            .pointer("/arguments/offset")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        if offset < 0 {
            anyhow::bail!("readMemory.arguments.offset must be non-negative");
        }
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        let count = request
            .pointer("/arguments/count")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| anyhow::anyhow!("readMemory.arguments.count is required"))?;
        let source = launched
            .files
            .iter()
            .find(|file| dap_normalize_path(&file.path) == dap_normalize_path(&frame.source.path))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "source `{}` is not part of the loaded project snapshot",
                    frame.source.path.display()
                )
            })?;
        let line = source
            .source
            .lines()
            .nth(usize::try_from(frame.line.saturating_sub(1)).unwrap_or(usize::MAX))
            .ok_or_else(|| anyhow::anyhow!("frame line {} is outside source", frame.line))?;
        let bytes = line.as_bytes();
        let start = offset.min(bytes.len());
        let end = start.saturating_add(count).min(bytes.len());
        let data = &bytes[start..end];
        Ok(serde_json::json!({
            "address": memory_reference,
            "data": dap_base64_encode(data),
            "unreadableBytes": count.saturating_sub(data.len()),
        }))
    }

    fn set_breakpoints_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let path = dap_normalize_path(&dap_breakpoint_source_path(
            self.launched.as_ref(),
            request,
        )?);
        let verified_lines = dap_verified_breakpoint_lines(&path).unwrap_or_default();
        let breakpoints = request
            .pointer("/arguments/breakpoints")
            .and_then(serde_json::Value::as_array)
            .map_or_else(Vec::new, |items| {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, breakpoint)| {
                        let line = breakpoint
                            .get("line")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0);
                        let verified = line > 0 && verified_lines.binary_search(&line).is_ok();
                        DapBreakpoint {
                            id: u64::try_from(index + 1).unwrap_or(u64::MAX),
                            line,
                            verified,
                            condition: breakpoint
                                .get("condition")
                                .and_then(serde_json::Value::as_str)
                                .map(str::trim)
                                .filter(|condition| !condition.is_empty())
                                .map(str::to_string),
                            hit_condition: breakpoint
                                .get("hitCondition")
                                .and_then(serde_json::Value::as_str)
                                .map(str::trim)
                                .filter(|condition| !condition.is_empty())
                                .map(str::to_string),
                            log_message: breakpoint
                                .get("logMessage")
                                .and_then(serde_json::Value::as_str)
                                .map(str::trim)
                                .filter(|message| !message.is_empty())
                                .map(str::to_string),
                            message: (!verified)
                                .then(|| "no executable ORV node on this line".to_string()),
                        }
                    })
                    .collect()
            });
        self.breakpoints.insert(path, breakpoints.clone());
        let response_breakpoints = breakpoints
            .iter()
            .map(|breakpoint| {
                let mut value = serde_json::json!({
                    "id": breakpoint.id,
                    "verified": breakpoint.verified,
                    "line": breakpoint.line,
                });
                if let Some(message) = &breakpoint.message {
                    value["message"] = serde_json::Value::String(message.clone());
                }
                value
            })
            .collect::<Vec<_>>();
        Ok(serde_json::json!({
            "breakpoints": response_breakpoints,
        }))
    }

    fn breakpoint_locations_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let path = dap_breakpoint_source_path(self.launched.as_ref(), request)?;
        let loaded = orv_project::load_project(&path).map_err(|e| anyhow::anyhow!("{e}"))?;
        let file = lsp_source_file_for_path(&loaded.files, &path)
            .ok_or_else(|| anyhow::anyhow!("breakpoint source is not part of loaded project"))?;
        let line = request
            .pointer("/arguments/line")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1);
        let end_line = request
            .pointer("/arguments/endLine")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(line);
        Ok(serde_json::json!({
            "breakpoints": dap_breakpoint_locations_json(
                &loaded.graph,
                &loaded.files,
                file.id,
                line,
                end_line,
            ),
        }))
    }

    fn set_function_breakpoints_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let breakpoints = request
            .pointer("/arguments/breakpoints")
            .and_then(serde_json::Value::as_array)
            .map_or_else(Vec::new, |items| {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, breakpoint)| {
                        let name = breakpoint
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .unwrap_or("");
                        let verified = !name.is_empty();
                        DapFunctionBreakpoint {
                            id: u64::try_from(index + 1).unwrap_or(u64::MAX),
                            name: name.to_string(),
                            verified,
                            message: (!verified)
                                .then(|| "function breakpoint name must not be empty".to_string()),
                        }
                    })
                    .collect()
            });
        let response_breakpoints = breakpoints
            .iter()
            .map(|breakpoint| {
                let mut value = serde_json::json!({
                    "id": breakpoint.id,
                    "verified": breakpoint.verified,
                });
                if let Some(message) = &breakpoint.message {
                    value["message"] = serde_json::Value::String(message.clone());
                }
                value
            })
            .collect::<Vec<_>>();
        self.function_breakpoints = breakpoints;
        Ok(serde_json::json!({
            "breakpoints": response_breakpoints,
        }))
    }

    fn data_breakpoint_info_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before dataBreakpointInfo"))?;
        let variables_reference = request
            .pointer("/arguments/variablesReference")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                anyhow::anyhow!("dataBreakpointInfo.arguments.variablesReference is required")
            })?;
        let name = request
            .pointer("/arguments/name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow::anyhow!("dataBreakpointInfo.arguments.name is required"))?;
        if variables_reference != 2
            || !dap_current_locals(launched)
                .iter()
                .any(|local| local.name == name)
        {
            return Ok(serde_json::json!({
                "dataId": null,
                "description": format!("no ORV local data breakpoint for {name}"),
                "accessTypes": [],
                "canPersist": false,
            }));
        }
        Ok(serde_json::json!({
            "dataId": format!("local:{name}"),
            "description": format!("local {name}"),
            "accessTypes": ["write", "readWrite"],
            "canPersist": true,
        }))
    }

    fn set_data_breakpoints_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let breakpoints = request
            .pointer("/arguments/breakpoints")
            .and_then(serde_json::Value::as_array)
            .map_or_else(Vec::new, |items| {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, breakpoint)| {
                        let data_id = breakpoint
                            .get("dataId")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .unwrap_or("");
                        let verified = dap_data_breakpoint_local_name(data_id).is_some();
                        DapDataBreakpoint {
                            id: u64::try_from(index + 1).unwrap_or(u64::MAX),
                            data_id: data_id.to_string(),
                            verified,
                            message: (!verified)
                                .then(|| "unsupported ORV data breakpoint".to_string()),
                        }
                    })
                    .collect()
            });
        let response_breakpoints = breakpoints
            .iter()
            .map(|breakpoint| {
                let mut value = serde_json::json!({
                    "id": breakpoint.id,
                    "verified": breakpoint.verified,
                });
                if let Some(message) = &breakpoint.message {
                    value["message"] = serde_json::Value::String(message.clone());
                }
                value
            })
            .collect::<Vec<_>>();
        self.data_breakpoints = breakpoints;
        Ok(serde_json::json!({
            "breakpoints": response_breakpoints,
        }))
    }

    fn set_instruction_breakpoints_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let frame_count = self.launched.as_ref().map(|launched| launched.frames.len());
        let breakpoints = request
            .pointer("/arguments/breakpoints")
            .and_then(serde_json::Value::as_array)
            .map_or_else(Vec::new, |items| {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, breakpoint)| {
                        let instruction_reference = breakpoint
                            .get("instructionReference")
                            .and_then(serde_json::Value::as_str)
                            .map_or("", str::trim)
                            .to_string();
                        let offset = breakpoint
                            .get("offset")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0);
                        dap_instruction_breakpoint(
                            u64::try_from(index + 1).unwrap_or(u64::MAX),
                            instruction_reference,
                            offset,
                            frame_count,
                        )
                    })
                    .collect()
            });
        let response_breakpoints = breakpoints
            .iter()
            .map(dap_instruction_breakpoint_json)
            .collect::<Vec<_>>();
        self.instruction_breakpoints = breakpoints;
        Ok(serde_json::json!({
            "breakpoints": response_breakpoints,
        }))
    }

    fn goto_targets_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before gotoTargets"))?;
        let path = dap_breakpoint_source_path(Some(launched), request)?;
        let normalized = dap_normalize_path(&path);
        let source = launched
            .sources
            .iter()
            .find(|source| dap_normalize_path(&source.path) == normalized)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "source `{}` is not part of the launched project",
                    path.display()
                )
            })?;
        let line = request
            .pointer("/arguments/line")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1);
        let end_line = request
            .pointer("/arguments/endLine")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(line);
        let verified_lines = dap_verified_breakpoint_lines(&path).unwrap_or_default();
        Ok(serde_json::json!({
            "targets": verified_lines
                .into_iter()
                .filter(|target_line| *target_line >= line && *target_line <= end_line)
                .map(|target_line| dap_goto_target_json(source, target_line))
                .collect::<Vec<_>>(),
        }))
    }

    fn stack_trace_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::require_reference_thread(request, "stackTrace")?;
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before stackTrace"))?;
        let frames = dap_stack_frames_json(launched);
        let total_frames = frames.len();
        let frames = dap_paginate_json_values(frames, request, "startFrame", "levels");
        Ok(serde_json::json!({
            "stackFrames": frames,
            "totalFrames": total_frames,
        }))
    }

    fn scopes_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before scopes"))?;
        let frame_id = request
            .pointer("/arguments/frameId")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("scopes.arguments.frameId is required"))?;
        if frame_id != 1 {
            return dap_non_current_scopes_result(launched, frame_id);
        }
        let (source, _) = dap_current_source_and_line(launched);
        let project_variable_count = dap_project_variables(launched).len();
        let local_variable_count = dap_current_locals(launched).len();
        let scope_source = dap_source_json_with_reference(&source, 0);
        Ok(serde_json::json!({
            "scopes": [
                {
                    "name": "Project",
                    "variablesReference": 1,
                    "namedVariables": project_variable_count,
                    "expensive": false,
                    "source": scope_source,
                },
                {
                    "name": "Locals",
                    "variablesReference": 2,
                    "namedVariables": local_variable_count,
                    "expensive": false,
                    "source": scope_source,
                },
            ],
        }))
    }

    fn variables_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before variables"))?;
        let variables_reference = request
            .pointer("/arguments/variablesReference")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("variables.arguments.variablesReference is required"))?;
        if variables_reference == 2 {
            let variables = dap_current_locals(launched)
                .iter()
                .map(dap_variable_json)
                .collect::<Vec<_>>();
            return Ok(serde_json::json!({
                "variables": dap_filter_and_paginate_variables(variables, request),
            }));
        }
        if variables_reference != 1 {
            anyhow::bail!("unknown variablesReference {variables_reference}");
        }
        let variables = dap_project_variables(launched);
        Ok(serde_json::json!({
            "variables": dap_filter_and_paginate_variables(variables, request),
        }))
    }

    fn evaluate_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before evaluate"))?;
        let expression = request
            .pointer("/arguments/expression")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|expression| !expression.is_empty())
            .ok_or_else(|| anyhow::anyhow!("evaluate.arguments.expression is required"))?;
        let (result, value_type) = dap_evaluate_project_value(launched, expression)
            .ok_or_else(|| anyhow::anyhow!("unknown evaluate expression `{expression}`"))?;
        Ok(serde_json::json!({
            "result": result,
            "type": value_type,
            "variablesReference": 0,
        }))
    }

    fn set_variable_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let variables_reference = request
            .pointer("/arguments/variablesReference")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                anyhow::anyhow!("setVariable.arguments.variablesReference is required")
            })?;
        if variables_reference != 2 {
            anyhow::bail!("setVariable currently supports only Locals variablesReference");
        }
        let name = request
            .pointer("/arguments/name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow::anyhow!("setVariable.arguments.name is required"))?;
        let value = request
            .pointer("/arguments/value")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("setVariable.arguments.value is required"))?;
        let variable = self.set_current_local_value(name, value)?;
        Ok(dap_set_value_json(&variable))
    }

    fn set_expression_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let expression = request
            .pointer("/arguments/expression")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|expression| !expression.is_empty())
            .ok_or_else(|| anyhow::anyhow!("setExpression.arguments.expression is required"))?;
        let value = request
            .pointer("/arguments/value")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("setExpression.arguments.value is required"))?;
        let variable = self.set_current_local_value(expression, value)?;
        Ok(dap_set_value_json(&variable))
    }

    fn completions_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before completions"))?;
        let prefix = request
            .pointer("/arguments/text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        Ok(serde_json::json!({
            "targets": dap_completion_targets_json(launched, prefix),
        }))
    }

    fn exception_info_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        Self::require_reference_thread(request, "exceptionInfo")?;
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before exceptionInfo"))?;
        Ok(dap_exception_info_json(&launched.runtime))
    }

    fn continue_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        Self::require_reference_thread(request, "continue")?;
        if self.launch_is_long_running() {
            return self.continue_long_running_result();
        }
        if self.launch_is_live() {
            return self.continue_live_result();
        }
        let (next_breakpoint, start_frame, has_frames) = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before continue"))?;
            (
                self.next_verified_breakpoint_frame(launched),
                launched.current_frame_index.saturating_add(1),
                !launched.frames.is_empty(),
            )
        };
        self.queue_event(
            "continued",
            serde_json::json!({
                "threadId": 1,
                "allThreadsContinued": false,
            }),
        );
        if let Some(index) = next_breakpoint {
            self.queue_frame_outputs(start_frame, index);
            let stopped = self.launched.as_ref().and_then(|launched| {
                launched.frames.get(index).map(|frame| {
                    (
                        frame.line,
                        self.breakpoint_frame_reason(&launched.frames, index)
                            .unwrap_or("breakpoint"),
                    )
                })
            });
            let launched = self
                .launched
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("launch is required before continue"))?;
            if let Some((line, reason)) = stopped {
                launched.stopped_line = line;
                launched.stopped_reason = reason.to_string();
            }
            launched.current_frame_index = index;
            self.queue_stopped_event();
            return Ok(serde_json::json!({
                "allThreadsContinued": false,
            }));
        }
        if has_frames {
            let end_frame = self
                .launched
                .as_ref()
                .and_then(|launched| launched.frames.len().checked_sub(1))
                .unwrap_or(0);
            self.queue_frame_outputs(start_frame, end_frame);
        }
        self.queue_event("terminated", serde_json::json!({}));
        self.launched = None;
        Ok(serde_json::json!({
            "allThreadsContinued": false,
        }))
    }

    fn reverse_continue_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        Self::require_reference_thread(request, "reverseContinue")?;
        let target_frame = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before reverseContinue"))?;
            self.previous_verified_breakpoint_frame(launched)
                .or_else(|| (launched.current_frame_index > 0).then_some(0))
        };
        let Some(target_frame) = target_frame else {
            anyhow::bail!("no previous runtime frame");
        };
        self.queue_event(
            "continued",
            serde_json::json!({
                "threadId": 1,
                "allThreadsContinued": false,
            }),
        );
        let stopped_reason = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before reverseContinue"))?;
            launched
                .frames
                .get(target_frame)
                .and_then(|_| self.breakpoint_frame_reason(&launched.frames, target_frame))
                .unwrap_or("entry")
        };
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before reverseContinue"))?;
        launched.current_frame_index = target_frame;
        if let Some(frame) = launched.frames.get(target_frame) {
            launched.stopped_line = frame.line;
        }
        launched.stopped_reason = stopped_reason.to_string();
        self.queue_stopped_event();
        Ok(serde_json::json!({
            "allThreadsContinued": false,
        }))
    }

    fn goto_result(&mut self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::require_reference_thread(request, "goto")?;
        let target_id = request
            .pointer("/arguments/targetId")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("goto.arguments.targetId is required"))?;
        let target_frame = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before goto"))?;
            launched
                .frames
                .iter()
                .enumerate()
                .find_map(|(index, frame)| {
                    (dap_goto_target_id(frame.source.reference, frame.line) == target_id)
                        .then_some(index)
                })
        };
        let Some(target_frame) = target_frame else {
            anyhow::bail!("unknown goto targetId {target_id}");
        };
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before goto"))?;
        let line = launched.frames[target_frame].line;
        launched.current_frame_index = target_frame;
        launched.stopped_line = line;
        launched.stopped_reason = "goto".to_string();
        self.queue_stopped_event();
        Ok(serde_json::json!({}))
    }

    fn step_back_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        Self::require_reference_thread(request, "stepBack")?;
        let target_frame = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before stepBack"))?;
            (launched.current_frame_index > 0).then_some(launched.current_frame_index - 1)
        };
        let Some(target_frame) = target_frame else {
            anyhow::bail!("no previous runtime frame");
        };
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before stepBack"))?;
        launched.current_frame_index = target_frame;
        if let Some(frame) = launched.frames.get(target_frame) {
            launched.stopped_line = frame.line;
        }
        launched.stopped_reason = "step".to_string();
        self.queue_stopped_event();
        Ok(serde_json::json!({}))
    }

    fn restart_frame_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let frame_id = request
            .pointer("/arguments/frameId")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("restartFrame.arguments.frameId is required"))?;
        let target_frame = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before restartFrame"))?;
            dap_restart_frame_target_index(launched, frame_id)
                .ok_or_else(|| anyhow::anyhow!("no restartable runtime frame"))?
        };
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before restartFrame"))?;
        launched.current_frame_index = target_frame;
        if let Some(frame) = launched.frames.get(target_frame) {
            launched.stopped_line = frame.line;
        }
        launched.stopped_reason = "restart".to_string();
        self.queue_stopped_event();
        Ok(serde_json::json!({}))
    }

    fn next_result(&mut self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::require_reference_thread(request, "next")?;
        if self.launch_is_live() {
            return self.next_live_result();
        }
        let (start_frame, target_frame) = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before next"))?;
            let current = launched
                .frames
                .get(launched.current_frame_index)
                .ok_or_else(|| anyhow::anyhow!("no current runtime frame"))?;
            let current_depth = current.stack.len();
            let start = launched.current_frame_index.saturating_add(1);
            let target = launched
                .frames
                .iter()
                .enumerate()
                .skip(start)
                .find_map(|(index, frame)| (frame.stack.len() <= current_depth).then_some(index));
            (start, target)
        };
        let Some(target_frame) = target_frame else {
            self.launched = None;
            self.queue_event("terminated", serde_json::json!({}));
            return Ok(serde_json::json!({}));
        };
        self.queue_frame_outputs(start_frame, target_frame);
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before next"))?;
        launched.current_frame_index = target_frame;
        if let Some(frame) = launched.frames.get(target_frame) {
            launched.stopped_line = frame.line;
        }
        launched.stopped_reason = "step".to_string();
        self.queue_stopped_event();
        Ok(serde_json::json!({}))
    }

    fn step_out_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        Self::require_reference_thread(request, "stepOut")?;
        if self.launch_is_live() {
            return self.step_out_live_result();
        }
        let (start_frame, target_frame) = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before stepOut"))?;
            let current = launched
                .frames
                .get(launched.current_frame_index)
                .ok_or_else(|| anyhow::anyhow!("no current runtime frame"))?;
            let current_depth = current.stack.len();
            if current_depth == 0 {
                anyhow::bail!("no caller frame");
            }
            let start = launched.current_frame_index.saturating_add(1);
            let target = launched
                .frames
                .iter()
                .enumerate()
                .skip(start)
                .find_map(|(index, frame)| (frame.stack.len() < current_depth).then_some(index));
            (start, target)
        };
        let Some(target_frame) = target_frame else {
            self.launched = None;
            self.queue_event("terminated", serde_json::json!({}));
            return Ok(serde_json::json!({}));
        };
        self.queue_frame_outputs(start_frame, target_frame);
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before stepOut"))?;
        launched.current_frame_index = target_frame;
        if let Some(frame) = launched.frames.get(target_frame) {
            launched.stopped_line = frame.line;
        }
        launched.stopped_reason = "step".to_string();
        self.queue_stopped_event();
        Ok(serde_json::json!({}))
    }

    fn step_in_targets_result(
        &self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let frame_id = request
            .pointer("/arguments/frameId")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("stepInTargets.arguments.frameId is required"))?;
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before stepInTargets"))?;
        if frame_id != 1 {
            if dap_stack_scope_frame(launched, frame_id).is_none() {
                anyhow::bail!("unknown ORV frameId {frame_id}");
            }
            return Ok(serde_json::json!({
                "targets": [],
            }));
        }
        Ok(serde_json::json!({
            "targets": dap_step_in_targets_json(launched),
        }))
    }

    fn step_in_result(&mut self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::require_reference_thread(request, "stepIn")?;
        if self.launch_is_live() {
            if request
                .pointer("/arguments/targetId")
                .and_then(serde_json::Value::as_u64)
                .is_some()
            {
                anyhow::bail!("stepIn targetId is unavailable in live debug mode");
            }
            return self.step_in_live_result();
        }
        if let Some(target_id) = request
            .pointer("/arguments/targetId")
            .and_then(serde_json::Value::as_u64)
        {
            let (start_frame, target_frame) = {
                let launched = self
                    .launched
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("launch is required before stepIn"))?;
                let target_frame = dap_step_in_target_indices(launched)
                    .into_iter()
                    .find(|index| dap_step_in_target_id(*index) == target_id)
                    .ok_or_else(|| anyhow::anyhow!("unknown stepIn targetId {target_id}"))?;
                (launched.current_frame_index.saturating_add(1), target_frame)
            };
            self.queue_frame_outputs(start_frame, target_frame);
            let launched = self
                .launched
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("launch is required before stepIn"))?;
            launched.current_frame_index = target_frame;
            if let Some(frame) = launched.frames.get(target_frame) {
                launched.stopped_line = frame.line;
            }
            launched.stopped_reason = "step".to_string();
            self.queue_stopped_event();
            return Ok(serde_json::json!({}));
        }
        let next_frame = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before debug control"))?;
            (!launched.frames.is_empty()).then_some(launched.current_frame_index + 1)
        };
        if let Some(next_frame) = next_frame {
            let launched = self
                .launched
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("launch is required before debug control"))?;
            let Some(frame) = launched.frames.get(next_frame) else {
                self.launched = None;
                self.queue_event("terminated", serde_json::json!({}));
                return Ok(serde_json::json!({}));
            };
            launched.current_frame_index = next_frame;
            launched.stopped_line = frame.line;
            launched.stopped_reason = "step".to_string();
            self.queue_current_frame_output();
            self.queue_stopped_event();
            return Ok(serde_json::json!({}));
        }
        let next_line = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before debug control"))?;
            dap_following_executable_line(&launched.executable_lines, launched.stopped_line)
        };
        let Some(next_line) = next_line else {
            self.launched = None;
            self.queue_event("terminated", serde_json::json!({}));
            return Ok(serde_json::json!({}));
        };
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before debug control"))?;
        launched.stopped_line = next_line;
        launched.stopped_reason = "step".to_string();
        self.queue_stopped_event();
        Ok(serde_json::json!({}))
    }

    fn continue_live_result(&mut self) -> anyhow::Result<serde_json::Value> {
        self.queue_event(
            "continued",
            serde_json::json!({
                "threadId": 1,
                "allThreadsContinued": false,
            }),
        );
        loop {
            match self.advance_live_frame()? {
                DapLiveAdvance::Frame { index, output } => {
                    self.queue_stdout_output(&output);
                    let stopped = self.launched.as_ref().and_then(|launched| {
                        launched.frames.get(index).and_then(|frame| {
                            self.breakpoint_frame_reason(&launched.frames, index)
                                .map(|reason| (frame.line, reason.to_string()))
                        })
                    });
                    if let Some((line, reason)) = stopped {
                        let launched = self
                            .launched
                            .as_mut()
                            .ok_or_else(|| anyhow::anyhow!("launch is required before continue"))?;
                        launched.current_frame_index = index;
                        launched.stopped_line = line;
                        launched.stopped_reason = reason;
                        self.queue_stopped_event();
                        return Ok(serde_json::json!({
                            "allThreadsContinued": false,
                        }));
                    }
                }
                DapLiveAdvance::Skipped => {}
                DapLiveAdvance::Done => {
                    self.queue_event("terminated", serde_json::json!({}));
                    self.launched = None;
                    return Ok(serde_json::json!({
                        "allThreadsContinued": false,
                    }));
                }
                DapLiveAdvance::Error { message } => {
                    self.queue_event(
                        "output",
                        serde_json::json!({
                            "category": "stderr",
                            "output": message,
                        }),
                    );
                    if let Some(launched) = self.launched.as_mut() {
                        launched.stopped_reason = "exception".to_string();
                    }
                    self.queue_stopped_event();
                    return Ok(serde_json::json!({
                        "allThreadsContinued": false,
                    }));
                }
            }
        }
    }

    fn continue_long_running_result(&mut self) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before continue"))?;
        launched.ensure_runtime_process_running()?;
        launched.runtime.status = "running".to_string();
        if let Some(async_runtime) = launched.async_runtime.as_mut() {
            if async_runtime.state != "running" {
                async_runtime.resume_count = async_runtime.resume_count.saturating_add(1);
            }
            async_runtime.state = "running".to_string();
        }
        self.queue_event(
            "continued",
            serde_json::json!({
                "threadId": 1,
                "allThreadsContinued": false,
            }),
        );
        Ok(serde_json::json!({
            "allThreadsContinued": false,
        }))
    }

    fn next_live_result(&mut self) -> anyhow::Result<serde_json::Value> {
        let current_depth = self
            .launched
            .as_ref()
            .and_then(|launched| launched.frames.get(launched.current_frame_index))
            .map(|frame| frame.stack.len())
            .ok_or_else(|| anyhow::anyhow!("no current runtime frame"))?;
        self.advance_live_until(|frame| frame.stack.len() <= current_depth, "step")
    }

    fn step_in_live_result(&mut self) -> anyhow::Result<serde_json::Value> {
        self.advance_live_until(|_| true, "step")
    }

    fn step_out_live_result(&mut self) -> anyhow::Result<serde_json::Value> {
        let current_depth = self
            .launched
            .as_ref()
            .and_then(|launched| launched.frames.get(launched.current_frame_index))
            .map(|frame| frame.stack.len())
            .ok_or_else(|| anyhow::anyhow!("no current runtime frame"))?;
        if current_depth == 0 {
            anyhow::bail!("no caller frame");
        }
        self.advance_live_until(|frame| frame.stack.len() < current_depth, "step")
    }

    fn advance_live_until(
        &mut self,
        mut is_target: impl FnMut(&DapFrameState) -> bool,
        stopped_reason: &str,
    ) -> anyhow::Result<serde_json::Value> {
        loop {
            match self.advance_live_frame()? {
                DapLiveAdvance::Frame { index, output } => {
                    self.queue_stdout_output(&output);
                    let target = self
                        .launched
                        .as_ref()
                        .and_then(|launched| launched.frames.get(index))
                        .is_some_and(&mut is_target);
                    if target {
                        let launched = self.launched.as_mut().ok_or_else(|| {
                            anyhow::anyhow!("launch is required before debug control")
                        })?;
                        launched.current_frame_index = index;
                        if let Some(frame) = launched.frames.get(index) {
                            launched.stopped_line = frame.line;
                        }
                        launched.stopped_reason = stopped_reason.to_string();
                        self.queue_stopped_event();
                        return Ok(serde_json::json!({}));
                    }
                }
                DapLiveAdvance::Skipped => {}
                DapLiveAdvance::Done => {
                    self.launched = None;
                    self.queue_event("terminated", serde_json::json!({}));
                    return Ok(serde_json::json!({}));
                }
                DapLiveAdvance::Error { message } => {
                    self.queue_event(
                        "output",
                        serde_json::json!({
                            "category": "stderr",
                            "output": message,
                        }),
                    );
                    if let Some(launched) = self.launched.as_mut() {
                        launched.stopped_reason = "exception".to_string();
                    }
                    self.queue_stopped_event();
                    return Ok(serde_json::json!({}));
                }
            }
        }
    }

    fn advance_live_frame(&mut self) -> anyhow::Result<DapLiveAdvance> {
        let step = {
            let launched = self
                .launched
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("launch is required before debug control"))?;
            let live = launched
                .live
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("launch is not in live debug mode"))?;
            live.stepper.step()
        };
        match step {
            Ok(Some(debug_frame)) => {
                let launched = self
                    .launched
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("launch is required before debug control"))?;
                let frames = dap_runtime_frames(&[debug_frame], &launched.files, &launched.sources);
                let Some(frame) = frames.into_iter().next() else {
                    return Ok(DapLiveAdvance::Skipped);
                };
                let output = frame.output.clone();
                launched.runtime.stdout.push_str(&output);
                launched.frames.push(frame);
                Ok(DapLiveAdvance::Frame {
                    index: launched.frames.len().saturating_sub(1),
                    output,
                })
            }
            Ok(None) => {
                if let Some(launched) = self.launched.as_mut() {
                    launched.runtime.status = "ok".to_string();
                    launched.live = None;
                }
                Ok(DapLiveAdvance::Done)
            }
            Err(err) => {
                let message = err.to_string();
                if let Some(launched) = self.launched.as_mut() {
                    launched.runtime.status = "error".to_string();
                    launched.runtime.error.clone_from(&message);
                    launched.live = None;
                }
                Ok(DapLiveAdvance::Error { message })
            }
        }
    }

    fn launch_is_live(&self) -> bool {
        self.launched
            .as_ref()
            .is_some_and(|launched| launched.live.is_some())
    }

    fn launch_is_long_running(&self) -> bool {
        self.launched
            .as_ref()
            .is_some_and(|launched| launched.long_running)
    }

    fn pause_result(&mut self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Self::require_reference_thread(request, "pause")?;
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before debug control"))?;
        if launched.long_running {
            launched.write_runtime_request_trace_file()?;
            launched.suspend_runtime_process()?;
            launched.runtime.status = "paused".to_string();
            if let Some(async_runtime) = launched.async_runtime.as_mut() {
                if async_runtime.state != "paused" {
                    async_runtime.pause_count = async_runtime.pause_count.saturating_add(1);
                }
                async_runtime.state = "paused".to_string();
            }
        }
        launched.stopped_reason = "pause".to_string();
        self.queue_stopped_event();
        Ok(serde_json::json!({}))
    }

    fn terminate_threads_result(
        &mut self,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.require_launch("terminateThreads")?;
        let terminates_reference_thread = request
            .pointer("/arguments/threadIds")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|thread_ids| {
                thread_ids
                    .iter()
                    .any(|thread_id| thread_id.as_u64() == Some(1))
            });
        if !terminates_reference_thread {
            anyhow::bail!("unknown ORV thread id");
        }
        if let Some(launched) = &self.launched {
            launched.write_runtime_request_trace_file()?;
        }
        self.queue_event("terminated", serde_json::json!({}));
        self.launched = None;
        Ok(serde_json::json!({}))
    }

    fn require_launch(&self, command: &str) -> anyhow::Result<()> {
        self.launched
            .as_ref()
            .map(|_| ())
            .ok_or_else(|| anyhow::anyhow!("launch is required before {command}"))
    }

    fn queue_stopped_event(&mut self) {
        let Some(launched) = &self.launched else {
            return;
        };
        self.queue_event(
            "stopped",
            serde_json::json!({
                "reason": launched.stopped_reason,
                "threadId": 1,
                "allThreadsStopped": false,
            }),
        );
    }

    fn queue_event(&mut self, event: &str, body: serde_json::Value) {
        self.pending_events.push(DapPendingEvent {
            event: event.to_string(),
            body,
        });
    }

    fn revalidate_instruction_breakpoints(&mut self, frame_count: usize) {
        for breakpoint in &mut self.instruction_breakpoints {
            *breakpoint = dap_instruction_breakpoint(
                breakpoint.id,
                breakpoint.instruction_reference.clone(),
                breakpoint.offset,
                Some(frame_count),
            );
        }
    }

    fn set_current_local_value(&mut self, name: &str, value: &str) -> anyhow::Result<DapVariable> {
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before setting variables"))?;
        let frame = launched
            .frames
            .get_mut(launched.current_frame_index)
            .ok_or_else(|| anyhow::anyhow!("no current runtime frame"))?;
        let variable = frame
            .locals
            .iter_mut()
            .find(|variable| variable.name == name)
            .ok_or_else(|| anyhow::anyhow!("unknown local variable `{name}`"))?;
        variable.value = value.to_string();
        Ok(variable.clone())
    }

    fn queue_current_frame_output(&mut self) {
        let output = self
            .launched
            .as_ref()
            .and_then(|launched| launched.frames.get(launched.current_frame_index))
            .map(|frame| frame.output.clone())
            .unwrap_or_default();
        self.queue_stdout_output(&output);
    }

    fn queue_frame_outputs(&mut self, start: usize, end: usize) {
        let outputs = self.launched.as_ref().map_or_else(Vec::new, |launched| {
            if start > end {
                return Vec::new();
            }
            launched
                .frames
                .iter()
                .enumerate()
                .skip(start)
                .take(end.saturating_sub(start).saturating_add(1))
                .flat_map(|(index, frame)| {
                    let mut outputs = Vec::new();
                    if !frame.output.is_empty() {
                        outputs.push(("stdout".to_string(), frame.output.clone()));
                    }
                    outputs.extend(
                        self.logpoint_outputs(&launched.frames, index)
                            .into_iter()
                            .map(|output| ("console".to_string(), output)),
                    );
                    outputs
                })
                .collect()
        });
        for (category, output) in outputs {
            self.queue_output(&category, &output);
        }
    }

    fn queue_stdout_output(&mut self, output: &str) {
        self.queue_output("stdout", output);
    }

    fn queue_output(&mut self, category: &str, output: &str) {
        if output.is_empty() {
            return;
        }
        self.queue_event(
            "output",
            serde_json::json!({
                "category": category,
                "output": output,
            }),
        );
    }

    pub(crate) fn drain_pending_events(&mut self) -> Vec<serde_json::Value> {
        std::mem::take(&mut self.pending_events)
            .into_iter()
            .map(|event| {
                dap_event_response(self.next_response_seq(), event.event.as_str(), &event.body)
            })
            .collect()
    }

    fn first_verified_breakpoint_frame(&self, frames: &[DapFrameState]) -> Option<usize> {
        frames
            .iter()
            .enumerate()
            .find_map(|(index, _)| self.breakpoint_frame_reason(frames, index).map(|_| index))
    }

    fn next_verified_breakpoint_frame(&self, launched: &DapLaunchState) -> Option<usize> {
        launched
            .frames
            .iter()
            .enumerate()
            .skip(launched.current_frame_index.saturating_add(1))
            .find_map(|(index, _)| {
                self.breakpoint_frame_reason(&launched.frames, index)
                    .map(|_| index)
            })
    }

    fn previous_verified_breakpoint_frame(&self, launched: &DapLaunchState) -> Option<usize> {
        (0..launched.current_frame_index).rev().find(|index| {
            self.breakpoint_frame_reason(&launched.frames, *index)
                .is_some()
        })
    }

    fn breakpoint_frame_reason(
        &self,
        frames: &[DapFrameState],
        index: usize,
    ) -> Option<&'static str> {
        let frame = frames.get(index)?;
        if self.has_verified_line_breakpoint(frames, index) {
            return Some("breakpoint");
        }
        if self.has_verified_function_breakpoint(frame) {
            return Some("function breakpoint");
        }
        if self.has_verified_instruction_breakpoint(index) {
            return Some("instruction breakpoint");
        }
        self.has_verified_data_breakpoint(frames, index)
            .then_some("data breakpoint")
    }

    fn has_verified_line_breakpoint(&self, frames: &[DapFrameState], index: usize) -> bool {
        let Some(frame) = frames.get(index) else {
            return false;
        };
        let normalized = dap_normalize_path(&frame.source.path);
        self.breakpoints
            .get(&normalized)
            .is_some_and(|breakpoints| {
                breakpoints.iter().any(|breakpoint| {
                    breakpoint.verified
                        && breakpoint.log_message.is_none()
                        && breakpoint.line == frame.line
                        && dap_breakpoint_condition_matches(frame, breakpoint.condition.as_deref())
                        && self.line_breakpoint_hit_condition_matches(
                            frames,
                            index,
                            &normalized,
                            breakpoint,
                        )
                })
            })
    }

    fn logpoint_outputs(&self, frames: &[DapFrameState], index: usize) -> Vec<String> {
        let Some(frame) = frames.get(index) else {
            return Vec::new();
        };
        let normalized = dap_normalize_path(&frame.source.path);
        self.breakpoints
            .get(&normalized)
            .map_or_else(Vec::new, |breakpoints| {
                breakpoints
                    .iter()
                    .filter(|breakpoint| {
                        breakpoint.verified
                            && breakpoint.line == frame.line
                            && breakpoint.log_message.is_some()
                            && dap_breakpoint_condition_matches(
                                frame,
                                breakpoint.condition.as_deref(),
                            )
                            && self.line_breakpoint_hit_condition_matches(
                                frames,
                                index,
                                &normalized,
                                breakpoint,
                            )
                    })
                    .filter_map(|breakpoint| breakpoint.log_message.as_deref())
                    .map(dap_logpoint_output)
                    .collect()
            })
    }

    fn line_breakpoint_hit_condition_matches(
        &self,
        frames: &[DapFrameState],
        index: usize,
        normalized_path: &Path,
        breakpoint: &DapBreakpoint,
    ) -> bool {
        let Some(hit_condition) = breakpoint.hit_condition.as_deref() else {
            return true;
        };
        let hit_count = frames[..=index]
            .iter()
            .filter(|frame| {
                dap_normalize_path(&frame.source.path) == normalized_path
                    && frame.line == breakpoint.line
                    && dap_breakpoint_condition_matches(frame, breakpoint.condition.as_deref())
            })
            .count();
        dap_hit_condition_matches(hit_condition, hit_count)
    }

    fn has_verified_function_breakpoint(&self, frame: &DapFrameState) -> bool {
        let Some(function_name) = frame.stack.last().map(|frame| frame.name.as_str()) else {
            return false;
        };
        self.function_breakpoints
            .iter()
            .any(|breakpoint| breakpoint.verified && breakpoint.name == function_name)
    }

    fn has_verified_instruction_breakpoint(&self, index: usize) -> bool {
        self.instruction_breakpoints
            .iter()
            .any(|breakpoint| breakpoint.verified && breakpoint.frame_index == Some(index))
    }

    fn has_verified_data_breakpoint(&self, frames: &[DapFrameState], index: usize) -> bool {
        let Some(frame) = frames.get(index) else {
            return false;
        };
        self.data_breakpoints
            .iter()
            .filter(|breakpoint| breakpoint.verified)
            .any(|breakpoint| {
                let Some(name) = dap_data_breakpoint_local_name(&breakpoint.data_id) else {
                    return false;
                };
                let Some(current) = dap_frame_local_value(frame, name) else {
                    return false;
                };
                let previous = frames[..index]
                    .iter()
                    .rev()
                    .find_map(|frame| dap_frame_local_value(frame, name));
                previous != Some(current)
            })
    }
}

pub(crate) fn dap_runtime_state(
    lowered: &orv_analyzer::LowerResult,
    diagnostic_count: usize,
    files: &[SourceFile],
    sources: &[DapSourceInfo],
) -> (DapRuntimeState, Vec<DapFrameState>) {
    if diagnostic_count > 0 {
        return (
            DapRuntimeState {
                status: "diagnostics".to_string(),
                stdout: String::new(),
                error: "diagnostics present".to_string(),
            },
            Vec::new(),
        );
    }
    let mut stdout = Vec::new();
    let (debug, result) = orv_runtime::run_with_debug(&lowered.program, &mut stdout);
    let runtime = match result {
        Ok(()) => DapRuntimeState {
            status: "ok".to_string(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            error: String::new(),
        },
        Err(err) => DapRuntimeState {
            status: "error".to_string(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            error: err.to_string(),
        },
    };
    (
        runtime,
        dap_runtime_frames(debug.frames.as_slice(), files, sources),
    )
}

pub(crate) fn dap_launch_runtime_state(
    lowered: &orv_analyzer::LowerResult,
    diagnostic_count: usize,
    files: &[SourceFile],
    sources: &[DapSourceInfo],
    live_requested: bool,
) -> (
    DapRuntimeState,
    Vec<DapFrameState>,
    Option<DapLiveState>,
    bool,
) {
    if diagnostic_count == 0 && dap_program_has_long_running_runtime(&lowered.program) {
        let (runtime, frames) = dap_long_running_runtime_state(&lowered.program, files, sources);
        return (runtime, frames, None, true);
    }
    if live_requested && diagnostic_count == 0 {
        let (runtime, frames, live) = dap_live_runtime_state(lowered, files, sources);
        return (runtime, frames, live, false);
    }
    let (runtime, frames) = dap_runtime_state(lowered, diagnostic_count, files, sources);
    (runtime, frames, None, false)
}

pub(crate) fn dap_live_runtime_state(
    lowered: &orv_analyzer::LowerResult,
    files: &[SourceFile],
    sources: &[DapSourceInfo],
) -> (DapRuntimeState, Vec<DapFrameState>, Option<DapLiveState>) {
    let mut stepper = orv_runtime::DebugStepper::new(lowered.program.clone(), Vec::new());
    let mut runtime = DapRuntimeState {
        status: "running".to_string(),
        stdout: String::new(),
        error: String::new(),
    };
    match stepper.step() {
        Ok(Some(debug_frame)) => {
            let frames = dap_runtime_frames(&[debug_frame], files, sources);
            for frame in &frames {
                runtime.stdout.push_str(&frame.output);
            }
            (runtime, frames, Some(DapLiveState { stepper }))
        }
        Ok(None) => {
            runtime.status = "ok".to_string();
            (runtime, Vec::new(), None)
        }
        Err(err) => {
            runtime.status = "error".to_string();
            runtime.error = err.to_string();
            (runtime, Vec::new(), None)
        }
    }
}

pub(crate) fn dap_program_has_long_running_runtime(program: &orv_hir::HirProgram) -> bool {
    program.items.iter().any(dap_stmt_has_long_running_runtime)
}

pub(crate) const fn dap_stmt_has_long_running_runtime(stmt: &orv_hir::HirStmt) -> bool {
    match stmt {
        orv_hir::HirStmt::Expr(expr) => dap_expr_has_long_running_runtime(expr),
        _ => false,
    }
}

pub(crate) const fn dap_expr_has_long_running_runtime(expr: &orv_hir::HirExpr) -> bool {
    matches!(expr.kind, orv_hir::HirExprKind::Server { .. })
}

pub(crate) fn dap_async_runtime_state(
    program: &orv_hir::HirProgram,
    long_running: bool,
) -> Option<DapAsyncRuntimeState> {
    long_running.then(|| {
        DapAsyncRuntimeState::server(
            dap_async_server_listen(program),
            dap_async_server_routes(program),
        )
    })
}

pub(crate) fn dap_async_server_listen(
    program: &orv_hir::HirProgram,
) -> Option<DapAsyncListenState> {
    program.items.iter().find_map(|stmt| match stmt {
        orv_hir::HirStmt::Expr(expr) => dap_expr_async_server_listen(expr),
        _ => None,
    })
}

pub(crate) fn dap_expr_async_server_listen(expr: &orv_hir::HirExpr) -> Option<DapAsyncListenState> {
    let orv_hir::HirExprKind::Server { listen, .. } = &expr.kind else {
        return None;
    };
    let listen = listen.as_ref()?;
    if let Some(listen) = dap_async_env_listen(listen) {
        return Some(listen);
    }
    match &listen.kind {
        orv_hir::HirExprKind::Integer(value) => Some(DapAsyncListenState {
            kind: "static".to_string(),
            display: value.clone(),
            port: value.parse::<u64>().ok(),
            variable: None,
            default_port: None,
        }),
        _ => Some(DapAsyncListenState {
            kind: "expression".to_string(),
            display: "<expression>".to_string(),
            port: None,
            variable: None,
            default_port: None,
        }),
    }
}

pub(crate) fn dap_async_env_listen(expr: &orv_hir::HirExpr) -> Option<DapAsyncListenState> {
    let orv_hir::HirExprKind::Call { callee, args } = &expr.kind else {
        return None;
    };
    if dap_hir_call_name(callee) != "int.from" || args.len() != 1 {
        return None;
    }
    let arg = args.first()?;
    let (env_expr, default_port) = match &arg.kind {
        orv_hir::HirExprKind::Binary {
            op: orv_hir::BinaryOp::Coalesce,
            lhs,
            rhs,
        } => (lhs.as_ref(), dap_string_port(rhs.as_ref())),
        _ => (arg, None),
    };
    let variable = dap_env_variable(env_expr)?;
    let display = default_port.map_or_else(
        || variable.clone(),
        |port| format!("{variable} default {port}"),
    );
    Some(DapAsyncListenState {
        kind: "env".to_string(),
        display,
        port: default_port,
        variable: Some(variable),
        default_port,
    })
}

pub(crate) fn dap_hir_call_name(expr: &orv_hir::HirExpr) -> String {
    match &expr.kind {
        orv_hir::HirExprKind::Ident(ident) => ident.name.clone(),
        orv_hir::HirExprKind::Field { target, field, .. } => {
            format!("{}.{}", dap_hir_call_name(target), field)
        }
        orv_hir::HirExprKind::OptionalField { target, field, .. } => {
            format!("{}?.{}", dap_hir_call_name(target), field)
        }
        orv_hir::HirExprKind::Domain { name, .. } => format!("@{name}"),
        orv_hir::HirExprKind::TypeName(name) => name.clone(),
        _ => "<expr>".to_string(),
    }
}

pub(crate) fn dap_env_variable(expr: &orv_hir::HirExpr) -> Option<String> {
    let orv_hir::HirExprKind::Field { target, field, .. } = &expr.kind else {
        return None;
    };
    let orv_hir::HirExprKind::Domain { name, args, .. } = &target.kind else {
        return None;
    };
    (name == "env" && args.is_empty()).then(|| field.clone())
}

pub(crate) fn dap_string_port(expr: &orv_hir::HirExpr) -> Option<u64> {
    let orv_hir::HirExprKind::String(segments) = &expr.kind else {
        return None;
    };
    let [orv_hir::HirStringSegment::Str(raw)] = segments.as_slice() else {
        return None;
    };
    raw.parse::<u64>().ok()
}

pub(crate) fn dap_async_server_routes(program: &orv_hir::HirProgram) -> Vec<DapAsyncRouteState> {
    program
        .items
        .iter()
        .flat_map(|stmt| match stmt {
            orv_hir::HirStmt::Expr(expr) => dap_expr_async_server_routes(expr),
            _ => Vec::new(),
        })
        .collect()
}

pub(crate) fn dap_expr_async_server_routes(expr: &orv_hir::HirExpr) -> Vec<DapAsyncRouteState> {
    let orv_hir::HirExprKind::Server { routes, .. } = &expr.kind else {
        return Vec::new();
    };
    routes
        .iter()
        .filter_map(|route| {
            let orv_hir::HirExprKind::Route { method, path, .. } = &route.kind else {
                return None;
            };
            Some(DapAsyncRouteState {
                method: method.clone(),
                path: path.clone(),
            })
        })
        .collect()
}

pub(crate) fn dap_long_running_runtime_state(
    program: &orv_hir::HirProgram,
    files: &[SourceFile],
    sources: &[DapSourceInfo],
) -> (DapRuntimeState, Vec<DapFrameState>) {
    let frames = program
        .items
        .iter()
        .filter(|stmt| dap_stmt_has_long_running_runtime(stmt))
        .filter_map(|stmt| dap_long_running_frame(stmt.span(), files, sources))
        .collect::<Vec<_>>();
    (
        DapRuntimeState {
            status: "paused".to_string(),
            stdout: String::new(),
            error: String::new(),
        },
        frames,
    )
}

pub(crate) fn dap_long_running_frame(
    span: Span,
    files: &[SourceFile],
    sources: &[DapSourceInfo],
) -> Option<DapFrameState> {
    let source = dap_source_for_span(span, files, sources)?;
    let line = dap_span_line(span, files)?;
    Some(DapFrameState {
        source: source.clone(),
        line,
        locals: Vec::new(),
        stack: vec![DapStackFrameState {
            name: "server runtime".to_string(),
            source,
            line,
        }],
        output: String::new(),
    })
}

pub(crate) fn dap_runtime_json(
    runtime: &DapRuntimeState,
    async_runtime: Option<&DapAsyncRuntimeState>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "status": runtime.status,
        "stdout": runtime.stdout,
        "error": runtime.error,
    });
    if let Some(async_runtime) = async_runtime {
        value["async"] = serde_json::json!({
            "kind": async_runtime.kind,
            "state": async_runtime.state,
            "resume_count": async_runtime.resume_count,
            "pause_count": async_runtime.pause_count,
            "listen": async_runtime.listen.as_ref().map(dap_async_listen_json),
            "route_count": async_runtime.routes.len(),
            "routes": async_runtime.routes.iter().map(dap_async_route_json).collect::<Vec<_>>(),
            "transport": async_runtime.transport.as_ref().map(dap_async_transport_json),
        });
    }
    value
}

pub(crate) fn dap_async_listen_json(listen: &DapAsyncListenState) -> serde_json::Value {
    let mut value = serde_json::json!({
        "kind": listen.kind,
        "display": listen.display,
    });
    if let Some(port) = listen.port {
        value["port"] = serde_json::json!(port);
    }
    if let Some(variable) = &listen.variable {
        value["variable"] = serde_json::json!(variable);
    }
    if let Some(default_port) = listen.default_port {
        value["default_port"] = serde_json::json!(default_port);
    }
    value
}

pub(crate) fn dap_async_route_json(route: &DapAsyncRouteState) -> serde_json::Value {
    serde_json::json!({
        "method": route.method,
        "path": route.path,
    })
}

pub(crate) fn dap_async_transport_json(transport: &DapAsyncTransportState) -> serde_json::Value {
    let mut value = serde_json::json!({
        "kind": transport.kind,
        "state": transport.state,
    });
    if let Some(process_id) = transport.process_id {
        value["process_id"] = serde_json::json!(process_id);
    }
    if let Some(address) = &transport.address {
        value["address"] = serde_json::json!(address);
    }
    value
}

pub(crate) fn dap_async_routes_display(routes: &[DapAsyncRouteState]) -> String {
    routes
        .iter()
        .map(|route| format!("{} {}", route.method, route.path))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn dap_async_transport_display(transport: &DapAsyncTransportState) -> String {
    if let Some(address) = &transport.address {
        return format!("{} {} {address}", transport.kind, transport.state);
    }
    if let Some(pid) = transport.process_id {
        return format!("{} {} pid {pid}", transport.kind, transport.state);
    }
    format!("{} {}", transport.kind, transport.state)
}

pub(crate) fn dap_project_variables(launched: &DapLaunchState) -> Vec<serde_json::Value> {
    let mut variables = vec![
        serde_json::json!({
            "name": "entry",
            "value": launched.path.display().to_string(),
            "type": "source",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "projectGraphNodes",
            "value": launched.node_count.to_string(),
            "type": "usize",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "diagnostics",
            "value": launched.diagnostic_count.to_string(),
            "type": "usize",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "runtimeStatus",
            "value": launched.runtime.status,
            "type": "string",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "stdout",
            "value": launched.runtime.stdout,
            "type": "string",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "runtimeError",
            "value": launched.runtime.error,
            "type": "string",
            "variablesReference": 0,
        }),
    ];
    if let Some(async_runtime) = &launched.async_runtime {
        variables.extend(dap_async_runtime_variables(launched, async_runtime));
    }
    variables
}

pub(crate) fn dap_async_runtime_variables(
    launched: &DapLaunchState,
    async_runtime: &DapAsyncRuntimeState,
) -> Vec<serde_json::Value> {
    let mut variables = vec![
        serde_json::json!({
            "name": "runtimeKind",
            "value": async_runtime.kind,
            "type": "string",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "runtimeAsyncState",
            "value": async_runtime.state,
            "type": "string",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "runtimeResumeCount",
            "value": async_runtime.resume_count.to_string(),
            "type": "usize",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "runtimePauseCount",
            "value": async_runtime.pause_count.to_string(),
            "type": "usize",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "runtimeRouteCount",
            "value": async_runtime.routes.len().to_string(),
            "type": "usize",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "runtimeRoutes",
            "value": dap_async_routes_display(&async_runtime.routes),
            "type": "string",
            "variablesReference": 0,
        }),
    ];
    variables.extend(dap_runtime_request_variables(launched));
    if let Some(listen) = &async_runtime.listen {
        variables.extend([
            serde_json::json!({
                "name": "runtimeListen",
                "value": listen.display,
                "type": "string",
                "variablesReference": 0,
            }),
            serde_json::json!({
                "name": "runtimeListenPort",
                "value": listen.port.map_or_else(String::new, |port| port.to_string()),
                "type": "usize",
                "variablesReference": 0,
            }),
        ]);
    }
    if let Some(transport) = &async_runtime.transport {
        variables.extend([
            serde_json::json!({
                "name": "runtimeTransport",
                "value": dap_async_transport_display(transport),
                "type": "string",
                "variablesReference": 0,
            }),
            serde_json::json!({
                "name": "runtimeProcessId",
                "value": transport.process_id.map_or_else(String::new, |pid| pid.to_string()),
                "type": "usize",
                "variablesReference": 0,
            }),
        ]);
    }
    variables
}

pub(crate) fn dap_runtime_request_variables(launched: &DapLaunchState) -> Vec<serde_json::Value> {
    let request_frames = dap_runtime_request_frames(launched);
    let mut variables = vec![
        serde_json::json!({
            "name": "runtimeRequestCount",
            "value": request_frames.len().to_string(),
            "type": "usize",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "runtimeLastRequest",
            "value": request_frames
                .last()
                .map_or_else(String::new, dap_server_request_frame_display),
            "type": "string",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "runtimeRequestFrames",
            "value": dap_server_request_frames_display(&request_frames),
            "type": "string",
            "variablesReference": 0,
        }),
        serde_json::json!({
            "name": "runtimeRequestTrace",
            "value": dap_server_request_trace_display(&request_frames),
            "type": "json",
            "variablesReference": 0,
        }),
    ];
    if let Some(path) = &launched.runtime_request_trace_path {
        variables.push(serde_json::json!({
            "name": "runtimeRequestTracePath",
            "value": path.display().to_string(),
            "type": "path",
            "variablesReference": 0,
        }));
    }
    variables
}

pub(crate) fn dap_runtime_request_frames(
    launched: &DapLaunchState,
) -> Vec<orv_runtime::server::ServerRequestFrame> {
    launched.attached_server.as_ref().map_or_else(
        Vec::new,
        orv_runtime::server::AttachedServer::request_frames,
    )
}

pub(crate) fn dap_server_request_frames_display(
    frames: &[orv_runtime::server::ServerRequestFrame],
) -> String {
    frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            format!(
                "#{} {}",
                index.saturating_add(1),
                dap_server_request_frame_display(frame)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn dap_server_request_frame_display(
    frame: &orv_runtime::server::ServerRequestFrame,
) -> String {
    let mut parts = vec![format!(
        "{} {} -> {}",
        frame.method, frame.path, frame.status
    )];
    if let (Some(method), Some(path)) = (&frame.route_method, &frame.route_path) {
        parts.push(format!("route {method} {path}"));
    }
    if let Some(origin_id) = &frame.response_origin_id {
        parts.push(format!("response {origin_id}"));
    }
    if !frame.params.is_empty() {
        parts.push(format!("params {}", dap_string_map_display(&frame.params)));
    }
    if !frame.query.is_empty() {
        parts.push(format!("query {}", dap_string_map_display(&frame.query)));
    }
    if !frame.body.is_empty() {
        parts.push(format!("body {}", frame.body));
    }
    parts.join(" ")
}

pub(crate) fn dap_server_request_trace_display(
    frames: &[orv_runtime::server::ServerRequestFrame],
) -> String {
    serde_json::to_string(&orv_runtime::server::request_trace_json(frames)).unwrap_or_else(|_| {
        "{\"schema_version\":1,\"kind\":\"orv.production.trace\",\"frames\":[]}".to_string()
    })
}

pub(crate) fn dap_string_map_display(values: &HashMap<String, String>) -> String {
    let mut entries = values
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    entries.sort();
    entries.join(",")
}

pub(crate) fn dap_runtime_frames(
    frames: &[orv_runtime::DebugFrame],
    files: &[SourceFile],
    sources: &[DapSourceInfo],
) -> Vec<DapFrameState> {
    frames
        .iter()
        .filter_map(|frame| {
            let source = dap_source_for_span(frame.span, files, sources)?;
            let line = dap_span_line(frame.span, files)?;
            let locals = frame
                .locals
                .iter()
                .map(|variable| dap_runtime_variable(variable, line))
                .collect();
            let stack = frame
                .stack
                .iter()
                .filter_map(|stack_frame| {
                    Some(DapStackFrameState {
                        name: stack_frame.name.clone(),
                        source: dap_source_for_span(stack_frame.span, files, sources)?,
                        line: dap_span_line(stack_frame.span, files)?,
                    })
                })
                .collect();
            Some(DapFrameState {
                source,
                line,
                locals,
                stack,
                output: frame.output.clone(),
            })
        })
        .collect()
}

pub(crate) fn dap_source_for_span(
    span: Span,
    files: &[SourceFile],
    sources: &[DapSourceInfo],
) -> Option<DapSourceInfo> {
    let file = files.iter().find(|file| file.id == span.file)?;
    sources
        .iter()
        .find(|source| dap_normalize_path(&file.path) == dap_normalize_path(&source.path))
        .cloned()
}

pub(crate) fn dap_runtime_variable(
    variable: &orv_runtime::DebugVariable,
    line: u64,
) -> DapVariable {
    let (value, value_type) = dap_runtime_value_display(&variable.value);
    DapVariable {
        name: variable.name.clone(),
        value,
        value_type,
        line,
        variables_reference: 0,
    }
}

pub(crate) fn dap_runtime_value_display(value: &orv_runtime::Value) -> (String, String) {
    match value {
        orv_runtime::Value::Int(value) => (value.to_string(), "int".to_string()),
        orv_runtime::Value::Float(value) => (value.to_string(), "float".to_string()),
        orv_runtime::Value::Str(value) => (
            serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\"")),
            "string".to_string(),
        ),
        orv_runtime::Value::Regex { pattern, flags } => {
            (format!("r\"{pattern}\"{flags}"), "regex".to_string())
        }
        orv_runtime::Value::Bool(value) => (value.to_string(), "bool".to_string()),
        orv_runtime::Value::Void => ("void".to_string(), "void".to_string()),
        orv_runtime::Value::Array(items) => {
            let items = items
                .iter()
                .map(|item| dap_runtime_value_display(item).0)
                .collect::<Vec<_>>()
                .join(", ");
            (format!("[{items}]"), "array".to_string())
        }
        orv_runtime::Value::Tuple(items) => {
            let items = items
                .iter()
                .map(|item| dap_runtime_value_display(item).0)
                .collect::<Vec<_>>()
                .join(", ");
            (format!("({items})"), "tuple".to_string())
        }
        orv_runtime::Value::Object(fields) => {
            let fields = fields
                .iter()
                .map(|(name, value)| {
                    let (value, _) = dap_runtime_value_display(value);
                    format!("{name}: {value}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            (format!("{{ {fields} }}"), "object".to_string())
        }
        orv_runtime::Value::Function(_)
        | orv_runtime::Value::Lambda(_)
        | orv_runtime::Value::BoundMethod { .. }
        | orv_runtime::Value::Db(_)
        | orv_runtime::Value::TypeName(_)
        | orv_runtime::Value::Builtin(_) => (value.to_string(), "runtime".to_string()),
    }
}

pub(crate) fn dap_current_source_and_line(launched: &DapLaunchState) -> (DapSourceInfo, u64) {
    if let Some(frame) = launched.frames.get(launched.current_frame_index) {
        return (frame.source.clone(), frame.line);
    }
    let source = dap_entry_source(launched);
    (source, launched.stopped_line)
}

pub(crate) fn dap_entry_source(launched: &DapLaunchState) -> DapSourceInfo {
    launched
        .sources
        .iter()
        .find(|source| dap_normalize_path(&source.path) == dap_normalize_path(&launched.path))
        .cloned()
        .unwrap_or_else(|| DapSourceInfo {
            reference: 0,
            name: launched.name.clone(),
            path: launched.path.clone(),
            uri: launched.uri.clone(),
            checksum: String::new(),
        })
}

pub(crate) fn dap_non_current_scopes_result(
    launched: &DapLaunchState,
    frame_id: u64,
) -> anyhow::Result<serde_json::Value> {
    let frame = dap_stack_scope_frame(launched, frame_id)
        .ok_or_else(|| anyhow::anyhow!("unknown ORV frameId {frame_id}"))?;
    Ok(serde_json::json!({
        "scopes": [
            {
                "name": frame.name,
                "variablesReference": 0,
                "namedVariables": 0,
                "expensive": false,
                "source": dap_source_json_with_reference(&frame.source, 0),
                "line": frame.line,
                "column": 1,
            },
        ],
    }))
}

pub(crate) struct DapScopeFrame {
    pub(crate) name: String,
    pub(crate) source: DapSourceInfo,
    pub(crate) line: u64,
}

pub(crate) fn dap_stack_scope_frame(
    launched: &DapLaunchState,
    frame_id: u64,
) -> Option<DapScopeFrame> {
    if frame_id <= 1 {
        return None;
    }
    if let Some(stack_frame) = dap_stack_call_for_frame_id(launched, frame_id) {
        return Some(DapScopeFrame {
            name: format!("Frame {}", stack_frame.name),
            source: stack_frame.source,
            line: stack_frame.line,
        });
    }
    (dap_stack_entry_frame_id(launched) == Some(frame_id)).then(|| DapScopeFrame {
        name: "Frame orv entry".to_string(),
        source: dap_entry_source(launched),
        line: 1,
    })
}

pub(crate) fn dap_stack_call_for_frame_id(
    launched: &DapLaunchState,
    frame_id: u64,
) -> Option<DapStackFrameState> {
    if frame_id <= 1 {
        return None;
    }
    let current_frame = launched.frames.get(launched.current_frame_index)?;
    let stack_index = usize::try_from(frame_id.saturating_sub(2)).ok()?;
    current_frame
        .stack
        .iter()
        .rev()
        .skip(1)
        .nth(stack_index)
        .cloned()
}

pub(crate) fn dap_stack_entry_frame_id(launched: &DapLaunchState) -> Option<u64> {
    let current_frame = launched.frames.get(launched.current_frame_index)?;
    (!current_frame.stack.is_empty())
        .then(|| u64::try_from(current_frame.stack.len().saturating_add(1)).ok())
        .flatten()
}

pub(crate) fn dap_stack_frames_json(launched: &DapLaunchState) -> Vec<serde_json::Value> {
    let current_frame = launched.frames.get(launched.current_frame_index);
    let (current_source, line) = dap_current_source_and_line(launched);
    let current_name = current_frame
        .and_then(|frame| frame.stack.last())
        .map_or_else(|| "orv entry".to_string(), |frame| frame.name.clone());
    let mut frames = vec![dap_stack_frame_json(
        1,
        &current_name,
        &current_source,
        line,
    )];
    if let Some(current_frame) = current_frame {
        for (index, stack_frame) in current_frame.stack.iter().rev().skip(1).enumerate() {
            frames.push(dap_stack_frame_json(
                u64::try_from(index + 2).unwrap_or(u64::MAX),
                &stack_frame.name,
                &stack_frame.source,
                stack_frame.line,
            ));
        }
        if !current_frame.stack.is_empty() {
            let entry_source = dap_entry_source(launched);
            frames.push(dap_stack_frame_json(
                u64::try_from(frames.len() + 1).unwrap_or(u64::MAX),
                "orv entry",
                &entry_source,
                1,
            ));
        }
    }
    frames
}

pub(crate) fn dap_paginate_json_values(
    values: Vec<serde_json::Value>,
    request: &serde_json::Value,
    start_name: &str,
    count_name: &str,
) -> Vec<serde_json::Value> {
    let total = values.len();
    let start = dap_usize_argument(request, start_name)
        .unwrap_or(0)
        .min(total);
    let count =
        dap_usize_argument(request, count_name).unwrap_or_else(|| total.saturating_sub(start));
    values.into_iter().skip(start).take(count).collect()
}

pub(crate) fn dap_filter_and_paginate_variables(
    values: Vec<serde_json::Value>,
    request: &serde_json::Value,
) -> Vec<serde_json::Value> {
    if dap_str_argument(request, "filter") == Some("indexed") {
        return Vec::new();
    }
    dap_paginate_json_values(values, request, "start", "count")
}

pub(crate) fn dap_str_argument<'a>(request: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    request
        .get("arguments")
        .and_then(|arguments| arguments.get(name))
        .and_then(serde_json::Value::as_str)
}

pub(crate) fn dap_usize_argument(request: &serde_json::Value, name: &str) -> Option<usize> {
    request
        .get("arguments")
        .and_then(|arguments| arguments.get(name))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

pub(crate) fn dap_stack_frame_json(
    id: u64,
    name: &str,
    source: &DapSourceInfo,
    line: u64,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "source": dap_source_json_with_reference(source, 0),
        "line": line,
        "column": 1,
    })
}

pub(crate) fn dap_disassemble_start_index(
    memory_reference: &str,
    instruction_offset: i64,
) -> anyhow::Result<usize> {
    let base = dap_memory_reference_frame_index(memory_reference, "disassemble")?;
    if instruction_offset < 0 {
        Ok(base.saturating_sub(
            usize::try_from(instruction_offset.saturating_abs()).unwrap_or(usize::MAX),
        ))
    } else {
        Ok(base.saturating_add(usize::try_from(instruction_offset).unwrap_or(usize::MAX)))
    }
}

pub(crate) fn dap_memory_reference_frame_index(
    memory_reference: &str,
    command: &str,
) -> anyhow::Result<usize> {
    let frame = memory_reference
        .strip_prefix("orv:frame:")
        .ok_or_else(|| {
            anyhow::anyhow!("unsupported ORV {command} memoryReference `{memory_reference}`")
        })?
        .parse::<usize>()
        .map_err(|_| {
            anyhow::anyhow!("invalid ORV {command} memoryReference `{memory_reference}`")
        })?;
    if frame == 0 {
        anyhow::bail!("invalid ORV {command} memoryReference `{memory_reference}`");
    }
    Ok(frame - 1)
}

pub(crate) fn dap_base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        encoded.push(char::from(TABLE[usize::from(first >> 2)]));
        encoded.push(char::from(
            TABLE[usize::from(((first & 0b0000_0011) << 4) | (second >> 4))],
        ));
        if chunk.len() > 1 {
            encoded.push(char::from(
                TABLE[usize::from(((second & 0b0000_1111) << 2) | (third >> 6))],
            ));
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(char::from(TABLE[usize::from(third & 0b0011_1111)]));
        } else {
            encoded.push('=');
        }
    }
    encoded
}

pub(crate) fn dap_disassembled_instruction_json(
    index: usize,
    frame: &DapFrameState,
) -> serde_json::Value {
    let name = frame
        .stack
        .last()
        .map_or("orv entry", |stack| stack.name.as_str());
    serde_json::json!({
        "address": format!("orv:frame:{}", index.saturating_add(1)),
        "instruction": format!("{name} line {}", frame.line),
        "location": dap_source_json_with_reference(&frame.source, 0),
        "line": frame.line,
        "column": 1,
    })
}

pub(crate) fn dap_step_in_target_id(frame_index: usize) -> u64 {
    u64::try_from(frame_index.saturating_add(1)).unwrap_or(u64::MAX)
}

pub(crate) fn dap_step_in_target_indices(launched: &DapLaunchState) -> Vec<usize> {
    let Some(current_frame) = launched.frames.get(launched.current_frame_index) else {
        return Vec::new();
    };
    let current_depth = current_frame.stack.len();
    let mut seen = Vec::<(String, u64, u64)>::new();
    let mut targets = Vec::new();
    for (index, frame) in launched
        .frames
        .iter()
        .enumerate()
        .skip(launched.current_frame_index.saturating_add(1))
    {
        let depth = frame.stack.len();
        if depth <= current_depth {
            break;
        }
        if depth != current_depth.saturating_add(1) {
            continue;
        }
        let Some(call_frame) = frame.stack.last() else {
            continue;
        };
        let key = (
            call_frame.name.clone(),
            call_frame.source.reference,
            call_frame.line,
        );
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        targets.push(index);
    }
    targets
}

pub(crate) fn dap_step_in_targets_json(launched: &DapLaunchState) -> Vec<serde_json::Value> {
    dap_step_in_target_indices(launched)
        .into_iter()
        .filter_map(|index| {
            let frame = launched.frames.get(index)?;
            let call_frame = frame.stack.last()?;
            Some(serde_json::json!({
                "id": dap_step_in_target_id(index),
                "label": call_frame.name,
                "line": call_frame.line,
                "column": 1,
                "source": dap_source_json_with_reference(&call_frame.source, 0),
            }))
        })
        .collect()
}

pub(crate) fn dap_restart_frame_target_index(
    launched: &DapLaunchState,
    frame_id: u64,
) -> Option<usize> {
    if frame_id != 1 {
        return dap_non_current_restart_frame_target_index(launched, frame_id);
    }
    let current_index = launched.current_frame_index;
    let current_frame = launched.frames.get(current_index)?;
    let Some(current_call) = current_frame.stack.last() else {
        return Some(0);
    };
    let current_depth = current_frame.stack.len();
    let mut target = current_index;
    for index in (0..=current_index).rev() {
        let frame = launched.frames.get(index)?;
        if frame.stack.len() < current_depth {
            break;
        }
        let Some(call) = frame.stack.last() else {
            continue;
        };
        if call.name == current_call.name
            && call.source.reference == current_call.source.reference
            && call.line == current_call.line
        {
            target = index;
        }
    }
    Some(target)
}

pub(crate) fn dap_non_current_restart_frame_target_index(
    launched: &DapLaunchState,
    frame_id: u64,
) -> Option<usize> {
    if dap_stack_entry_frame_id(launched) == Some(frame_id) {
        return Some(0);
    }
    let target_call = dap_stack_call_for_frame_id(launched, frame_id)?;
    let current_index = launched.current_frame_index;
    let mut target = None;
    for index in (0..=current_index).rev() {
        let frame = launched.frames.get(index)?;
        let Some(call) = frame.stack.last() else {
            continue;
        };
        if dap_same_stack_call(call, &target_call) {
            target = Some(index);
        }
    }
    target
}

pub(crate) fn dap_same_stack_call(left: &DapStackFrameState, right: &DapStackFrameState) -> bool {
    left.name == right.name
        && left.source.reference == right.source.reference
        && left.line == right.line
}

pub(crate) fn dap_current_locals(launched: &DapLaunchState) -> &[DapVariable] {
    launched
        .frames
        .get(launched.current_frame_index)
        .map_or(&[], |frame| frame.locals.as_slice())
}

pub(crate) fn dap_data_breakpoint_local_name(data_id: &str) -> Option<&str> {
    data_id
        .strip_prefix("local:")
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

pub(crate) fn dap_frame_local_value<'a>(frame: &'a DapFrameState, name: &str) -> Option<&'a str> {
    frame
        .locals
        .iter()
        .find(|local| local.name == name)
        .map(|local| local.value.as_str())
}

pub(crate) fn dap_logpoint_output(message: &str) -> String {
    let mut output = message.to_string();
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

pub(crate) fn dap_breakpoint_condition_matches(
    frame: &DapFrameState,
    condition: Option<&str>,
) -> bool {
    let Some(condition) = condition
        .map(str::trim)
        .filter(|condition| !condition.is_empty())
    else {
        return true;
    };
    match condition {
        "true" => return true,
        "false" => return false,
        _ => {}
    }
    for op in ["==", "!=", ">=", "<=", ">", "<"] {
        if let Some((left, right)) = condition.split_once(op) {
            return dap_compare_breakpoint_condition(frame, left.trim(), op, right.trim());
        }
    }
    dap_frame_local_value(frame, condition).is_some_and(dap_condition_value_truthy)
}

pub(crate) fn dap_compare_breakpoint_condition(
    frame: &DapFrameState,
    left: &str,
    op: &str,
    right: &str,
) -> bool {
    let Some(left_value) = dap_frame_local_value(frame, left) else {
        return false;
    };
    if matches!(op, ">" | "<" | ">=" | "<=") {
        let Some(result) = dap_compare_condition_numbers(left_value, op, right) else {
            return false;
        };
        return result;
    }
    let right_value = dap_normalize_condition_literal(right);
    match op {
        "==" => left_value == right_value,
        "!=" => left_value != right_value,
        _ => false,
    }
}

pub(crate) fn dap_compare_condition_numbers(left: &str, op: &str, right: &str) -> Option<bool> {
    let left = left.parse::<f64>().ok()?;
    let right = right.parse::<f64>().ok()?;
    Some(match op {
        ">" => left > right,
        "<" => left < right,
        ">=" => left >= right,
        "<=" => left <= right,
        _ => return None,
    })
}

pub(crate) fn dap_normalize_condition_literal(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        let decoded = serde_json::from_str::<String>(trimmed)
            .unwrap_or_else(|_| trimmed.trim_matches('"').to_string());
        return serde_json::to_string(&decoded).unwrap_or(decoded);
    }
    trimmed.to_string()
}

pub(crate) fn dap_condition_value_truthy(value: &str) -> bool {
    !matches!(value, "" | "false" | "0" | "0.0" | "void" | "\"\"")
}

pub(crate) fn dap_hit_condition_matches(condition: &str, hit_count: usize) -> bool {
    let condition = condition.trim();
    if let Some(modulo) = condition
        .strip_prefix('%')
        .and_then(|value| value.trim_start_matches('=').trim().parse::<usize>().ok())
    {
        return modulo > 0 && hit_count % modulo == 0;
    }
    for op in [">=", "<=", ">", "<", "==", "="] {
        if let Some((_, right)) = condition.split_once(op) {
            let Some(expected) = right.trim().parse::<usize>().ok() else {
                return false;
            };
            return match op {
                ">=" => hit_count >= expected,
                "<=" => hit_count <= expected,
                ">" => hit_count > expected,
                "<" => hit_count < expected,
                "==" | "=" => hit_count == expected,
                _ => false,
            };
        }
    }
    condition
        .parse::<usize>()
        .is_ok_and(|expected| hit_count == expected)
}

pub(crate) fn dap_set_exception_breakpoints_result(
    request: &serde_json::Value,
) -> serde_json::Value {
    let breakpoints = request
        .pointer("/arguments/filters")
        .and_then(serde_json::Value::as_array)
        .map_or_else(Vec::new, |filters| {
            filters
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(|filter| {
                    let verified = matches!(filter, "orv.diagnostics" | "orv.runtime");
                    let mut breakpoint = serde_json::json!({
                        "verified": verified,
                        "filter": filter,
                    });
                    if !verified {
                        breakpoint["message"] = serde_json::Value::String(
                            "unsupported ORV exception filter".to_string(),
                        );
                    }
                    breakpoint
                })
                .collect()
        });
    serde_json::json!({
        "breakpoints": breakpoints,
    })
}

pub(crate) fn dap_instruction_breakpoint(
    id: u64,
    instruction_reference: String,
    offset: i64,
    frame_count: Option<usize>,
) -> DapInstructionBreakpoint {
    let frame_index = frame_count.and_then(|count| {
        dap_instruction_breakpoint_frame_index(count, &instruction_reference, offset)
    });
    let verified = frame_index.is_some();
    let message = if verified {
        None
    } else if frame_count.is_none() {
        Some("launch is required before verifying ORV instruction breakpoints".to_string())
    } else {
        Some(format!(
            "unknown ORV instructionReference `{instruction_reference}`"
        ))
    };
    DapInstructionBreakpoint {
        id,
        instruction_reference,
        offset,
        frame_index,
        verified,
        message,
    }
}

pub(crate) fn dap_instruction_breakpoint_frame_index(
    frame_count: usize,
    instruction_reference: &str,
    offset: i64,
) -> Option<usize> {
    let index = dap_disassemble_start_index(instruction_reference, offset).ok()?;
    (index < frame_count).then_some(index)
}

pub(crate) fn dap_instruction_breakpoint_json(
    breakpoint: &DapInstructionBreakpoint,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "id": breakpoint.id,
        "verified": breakpoint.verified,
        "instructionReference": breakpoint.instruction_reference.as_str(),
        "offset": breakpoint.offset,
    });
    if let Some(message) = &breakpoint.message {
        value["message"] = serde_json::Value::String(message.clone());
    }
    value
}

pub(crate) fn dap_exception_info_json(runtime: &DapRuntimeState) -> serde_json::Value {
    let (exception_id, description, break_mode) = match runtime.status.as_str() {
        "diagnostics" => ("orv.diagnostics", "diagnostics present", "always"),
        "error" => ("orv.runtime", runtime.error.as_str(), "always"),
        _ => ("orv.none", "no exception", "never"),
    };
    serde_json::json!({
        "exceptionId": exception_id,
        "description": description,
        "breakMode": break_mode,
        "details": {
            "message": description,
            "typeName": runtime.status,
            "stackTrace": "",
        },
    })
}

pub(crate) fn dap_variable_json(variable: &DapVariable) -> serde_json::Value {
    serde_json::json!({
        "name": variable.name,
        "value": variable.value,
        "type": variable.value_type,
        "variablesReference": variable.variables_reference,
    })
}

pub(crate) fn dap_set_value_json(variable: &DapVariable) -> serde_json::Value {
    serde_json::json!({
        "value": variable.value,
        "type": variable.value_type,
        "variablesReference": variable.variables_reference,
    })
}

pub(crate) fn dap_span_line(span: Span, files: &[SourceFile]) -> Option<u64> {
    let file = files.iter().find(|file| file.id == span.file)?;
    let start = byte_position(&file.source, span.range.start);
    Some(u64::try_from(start.0 + 1).unwrap_or(u64::MAX))
}

pub(crate) fn dap_evaluate_project_value(
    launched: &DapLaunchState,
    expression: &str,
) -> Option<(String, String)> {
    if let Some(local) = dap_current_locals(launched)
        .iter()
        .find(|local| local.name == expression)
    {
        return Some((local.value.clone(), local.value_type.clone()));
    }
    match expression {
        "entry" => Some((launched.path.display().to_string(), "source".to_string())),
        "projectGraphNodes" => Some((launched.node_count.to_string(), "usize".to_string())),
        "diagnostics" => Some((launched.diagnostic_count.to_string(), "usize".to_string())),
        "runtimeStatus" => Some((launched.runtime.status.clone(), "string".to_string())),
        "stdout" => Some((launched.runtime.stdout.clone(), "string".to_string())),
        "runtimeError" => Some((launched.runtime.error.clone(), "string".to_string())),
        _ => dap_evaluate_async_runtime_value(launched, expression),
    }
}

pub(crate) fn dap_evaluate_async_runtime_value(
    launched: &DapLaunchState,
    expression: &str,
) -> Option<(String, String)> {
    let runtime = launched.async_runtime.as_ref()?;
    match expression {
        "runtimeKind" => Some((runtime.kind.clone(), "string".to_string())),
        "runtimeAsyncState" => Some((runtime.state.clone(), "string".to_string())),
        "runtimeResumeCount" => Some((runtime.resume_count.to_string(), "usize".to_string())),
        "runtimePauseCount" => Some((runtime.pause_count.to_string(), "usize".to_string())),
        "runtimeRouteCount" => Some((runtime.routes.len().to_string(), "usize".to_string())),
        "runtimeRoutes" => Some((
            dap_async_routes_display(&runtime.routes),
            "string".to_string(),
        )),
        "runtimeRequestCount" => Some((
            dap_runtime_request_frames(launched).len().to_string(),
            "usize".to_string(),
        )),
        "runtimeLastRequest" => {
            let frames = dap_runtime_request_frames(launched);
            Some((
                frames
                    .last()
                    .map_or_else(String::new, dap_server_request_frame_display),
                "string".to_string(),
            ))
        }
        "runtimeRequestFrames" => Some((
            dap_server_request_frames_display(&dap_runtime_request_frames(launched)),
            "string".to_string(),
        )),
        "runtimeRequestTrace" => Some((
            dap_server_request_trace_display(&dap_runtime_request_frames(launched)),
            "json".to_string(),
        )),
        "runtimeRequestTracePath" => launched
            .runtime_request_trace_path
            .as_ref()
            .map(|path| (path.display().to_string(), "path".to_string())),
        "runtimeListen" => runtime
            .listen
            .as_ref()
            .map(|listen| (listen.display.clone(), "string".to_string())),
        "runtimeListenPort" => runtime.listen.as_ref().map(|listen| {
            (
                listen
                    .port
                    .map_or_else(String::new, |port| port.to_string()),
                "usize".to_string(),
            )
        }),
        "runtimeTransport" => runtime
            .transport
            .as_ref()
            .map(|transport| (dap_async_transport_display(transport), "string".to_string())),
        "runtimeProcessId" => runtime.transport.as_ref().map(|transport| {
            (
                transport
                    .process_id
                    .map_or_else(String::new, |pid| pid.to_string()),
                "usize".to_string(),
            )
        }),
        _ => None,
    }
}

pub(crate) fn dap_completion_targets_json(
    launched: &DapLaunchState,
    prefix: &str,
) -> Vec<serde_json::Value> {
    const EXPRESSIONS: &[&str] = &[
        "entry",
        "projectGraphNodes",
        "diagnostics",
        "runtimeStatus",
        "stdout",
        "runtimeError",
    ];
    let mut targets = EXPRESSIONS
        .iter()
        .filter(|expression| expression.starts_with(prefix))
        .map(|expression| {
            serde_json::json!({
                "label": expression,
                "type": "property",
                "sortText": expression,
            })
        })
        .collect::<Vec<_>>();
    if launched.async_runtime.is_some() {
        targets.extend(
            [
                "runtimeKind",
                "runtimeAsyncState",
                "runtimeResumeCount",
                "runtimePauseCount",
                "runtimeRouteCount",
                "runtimeRoutes",
                "runtimeRequestCount",
                "runtimeLastRequest",
                "runtimeRequestFrames",
                "runtimeRequestTrace",
                "runtimeRequestTracePath",
                "runtimeListen",
                "runtimeListenPort",
                "runtimeTransport",
                "runtimeProcessId",
            ]
            .into_iter()
            .filter(|expression| expression.starts_with(prefix))
            .map(|expression| {
                serde_json::json!({
                    "label": expression,
                    "type": "property",
                    "sortText": expression,
                })
            }),
        );
    }
    targets.extend(
        dap_current_locals(launched)
            .iter()
            .filter(|local| local.name.starts_with(prefix))
            .map(|local| {
                serde_json::json!({
                    "label": local.name,
                    "type": "variable",
                    "sortText": local.name,
                })
            }),
    );
    targets.sort_by_key(|target| {
        target
            .get("sortText")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    });
    targets.dedup_by(|left, right| left["label"] == right["label"]);
    targets
}

pub(crate) fn dap_breakpoint_locations_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
    file_id: FileId,
    line: u64,
    end_line: u64,
) -> Vec<serde_json::Value> {
    let start_line = line.min(end_line);
    let end_line = line.max(end_line);
    let mut locations = graph
        .nodes
        .iter()
        .filter(|node| node.file == file_id)
        .filter(|node| lsp_selectable_node_kind(node.kind))
        .filter_map(|node| {
            let file = files.iter().find(|file| file.id == node.file)?;
            let start = byte_position(&file.source, node.span.range.start);
            let line = u64::try_from(start.0 + 1).unwrap_or(u64::MAX);
            let column = u64::try_from(start.1 + 1).unwrap_or(u64::MAX);
            if line < start_line || line > end_line {
                return None;
            }
            Some(serde_json::json!({
                "line": line,
                "column": column,
            }))
        })
        .collect::<Vec<_>>();
    locations.sort_by_key(|location| {
        (
            location
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(u64::MAX),
            location
                .get("column")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(u64::MAX),
        )
    });
    locations
        .dedup_by(|left, right| left["line"] == right["line"] && left["column"] == right["column"]);
    locations
}

pub(crate) fn dap_verified_breakpoint_lines(path: &Path) -> anyhow::Result<Vec<u64>> {
    let loaded = orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let file = lsp_source_file_for_path(&loaded.files, path)
        .ok_or_else(|| anyhow::anyhow!("breakpoint source is not part of loaded project"))?;
    let mut lines = loaded
        .graph
        .nodes
        .iter()
        .filter(|node| node.file == file.id)
        .filter(|node| lsp_selectable_node_kind(node.kind))
        .filter_map(|node| {
            let file = loaded.files.iter().find(|file| file.id == node.file)?;
            let start = byte_position(&file.source, node.span.range.start);
            Some(u64::try_from(start.0 + 1).unwrap_or(u64::MAX))
        })
        .collect::<Vec<_>>();
    for stmt in &loaded.program.items {
        dap_collect_stmt_breakpoint_lines(stmt, file.id, &loaded.files, &mut lines);
    }
    lines.sort_unstable();
    lines.dedup();
    Ok(lines)
}

pub(crate) fn dap_collect_stmt_breakpoint_lines(
    stmt: &Stmt,
    file_id: FileId,
    files: &[SourceFile],
    lines: &mut Vec<u64>,
) {
    dap_push_span_line(stmt.span(), file_id, files, lines);
    match stmt {
        Stmt::Let(stmt) => dap_collect_expr_breakpoint_lines(&stmt.init, file_id, files, lines),
        Stmt::Const(stmt) => dap_collect_expr_breakpoint_lines(&stmt.init, file_id, files, lines),
        Stmt::Function(stmt) => {
            dap_collect_function_body_breakpoint_lines(&stmt.body, file_id, files, lines);
        }
        Stmt::Enum(stmt) => {
            for variant in &stmt.variants {
                dap_collect_expr_breakpoint_lines(&variant.value, file_id, files, lines);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                dap_collect_expr_breakpoint_lines(value, file_id, files, lines);
            }
        }
        Stmt::Expr(expr) => dap_collect_expr_breakpoint_lines(expr, file_id, files, lines),
        Stmt::Struct(_) | Stmt::TypeAlias(_) | Stmt::Import(_) => {}
    }
}

pub(crate) fn dap_collect_function_body_breakpoint_lines(
    body: &FunctionBody,
    file_id: FileId,
    files: &[SourceFile],
    lines: &mut Vec<u64>,
) {
    match body {
        FunctionBody::Block(block) => {
            dap_collect_block_breakpoint_lines(block, file_id, files, lines);
        }
        FunctionBody::Expr(expr) => dap_collect_expr_breakpoint_lines(expr, file_id, files, lines),
    }
}

pub(crate) fn dap_collect_block_breakpoint_lines(
    block: &Block,
    file_id: FileId,
    files: &[SourceFile],
    lines: &mut Vec<u64>,
) {
    for stmt in &block.stmts {
        dap_collect_stmt_breakpoint_lines(stmt, file_id, files, lines);
    }
}

pub(crate) fn dap_collect_expr_breakpoint_lines(
    expr: &Expr,
    file_id: FileId,
    files: &[SourceFile],
    lines: &mut Vec<u64>,
) {
    dap_push_span_line(expr.span, file_id, files, lines);
    match &expr.kind {
        ExprKind::Unary { expr, .. }
        | ExprKind::Paren(expr)
        | ExprKind::Await(expr)
        | ExprKind::Throw(expr)
        | ExprKind::Cast { expr, .. } => {
            dap_collect_expr_breakpoint_lines(expr, file_id, files, lines);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            dap_collect_expr_breakpoint_lines(lhs, file_id, files, lines);
            dap_collect_expr_breakpoint_lines(rhs, file_id, files, lines);
        }
        ExprKind::Domain { args, .. } | ExprKind::Tuple(args) | ExprKind::Array(args) => {
            for arg in args {
                dap_collect_expr_breakpoint_lines(arg, file_id, files, lines);
            }
        }
        ExprKind::Block(block) => dap_collect_block_breakpoint_lines(block, file_id, files, lines),
        ExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            dap_collect_expr_breakpoint_lines(cond, file_id, files, lines);
            dap_collect_block_breakpoint_lines(then, file_id, files, lines);
            if let Some(else_branch) = else_branch {
                dap_collect_expr_breakpoint_lines(else_branch, file_id, files, lines);
            }
        }
        ExprKind::When { scrutinee, arms } => {
            dap_collect_expr_breakpoint_lines(scrutinee, file_id, files, lines);
            for arm in arms {
                dap_collect_expr_breakpoint_lines(&arm.body, file_id, files, lines);
            }
        }
        ExprKind::Assign { value, .. } => {
            dap_collect_expr_breakpoint_lines(value, file_id, files, lines);
        }
        ExprKind::Call { callee, args } => {
            dap_collect_expr_breakpoint_lines(callee, file_id, files, lines);
            for arg in args {
                dap_collect_expr_breakpoint_lines(arg, file_id, files, lines);
            }
        }
        ExprKind::AssignField { object, value, .. } => {
            dap_collect_expr_breakpoint_lines(object, file_id, files, lines);
            dap_collect_expr_breakpoint_lines(value, file_id, files, lines);
        }
        ExprKind::AssignIndex {
            object,
            index,
            value,
        } => {
            dap_collect_expr_breakpoint_lines(object, file_id, files, lines);
            dap_collect_expr_breakpoint_lines(index, file_id, files, lines);
            dap_collect_expr_breakpoint_lines(value, file_id, files, lines);
        }
        ExprKind::For { iter, body, .. } => {
            dap_collect_expr_breakpoint_lines(iter, file_id, files, lines);
            dap_collect_block_breakpoint_lines(body, file_id, files, lines);
        }
        ExprKind::While { cond, body } => {
            dap_collect_expr_breakpoint_lines(cond, file_id, files, lines);
            dap_collect_block_breakpoint_lines(body, file_id, files, lines);
        }
        ExprKind::Range { start, end, .. } => {
            dap_collect_expr_breakpoint_lines(start, file_id, files, lines);
            dap_collect_expr_breakpoint_lines(end, file_id, files, lines);
        }
        ExprKind::Object(fields) | ExprKind::TypedObject { fields, .. } => {
            for field in fields {
                dap_collect_expr_breakpoint_lines(&field.value, file_id, files, lines);
            }
        }
        ExprKind::Index { target, index } => {
            dap_collect_expr_breakpoint_lines(target, file_id, files, lines);
            dap_collect_expr_breakpoint_lines(index, file_id, files, lines);
        }
        ExprKind::Slice { target, start, end } => {
            dap_collect_expr_breakpoint_lines(target, file_id, files, lines);
            if let Some(start) = start {
                dap_collect_expr_breakpoint_lines(start, file_id, files, lines);
            }
            if let Some(end) = end {
                dap_collect_expr_breakpoint_lines(end, file_id, files, lines);
            }
        }
        ExprKind::Field { target, .. } | ExprKind::OptionalField { target, .. } => {
            dap_collect_expr_breakpoint_lines(target, file_id, files, lines);
        }
        ExprKind::Lambda { body, .. } => {
            dap_collect_function_body_breakpoint_lines(body, file_id, files, lines);
        }
        ExprKind::Try { try_block, catch } => {
            dap_collect_block_breakpoint_lines(try_block, file_id, files, lines);
            if let Some(catch) = catch {
                dap_collect_block_breakpoint_lines(&catch.body, file_id, files, lines);
            }
        }
        ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Regex { .. }
        | ExprKind::True
        | ExprKind::False
        | ExprKind::Void
        | ExprKind::Ident(_)
        | ExprKind::TypeName(_)
        | ExprKind::Break
        | ExprKind::Continue => {}
    }
}

pub(crate) fn dap_push_span_line(
    span: Span,
    file_id: FileId,
    files: &[SourceFile],
    lines: &mut Vec<u64>,
) {
    if span.file != file_id {
        return;
    }
    let Some(file) = files.iter().find(|file| file.id == span.file) else {
        return;
    };
    let start = byte_position(&file.source, span.range.start);
    lines.push(u64::try_from(start.0 + 1).unwrap_or(u64::MAX));
}

pub(crate) fn dap_following_executable_line(lines: &[u64], current: u64) -> Option<u64> {
    lines.iter().copied().find(|line| *line > current)
}

pub(crate) fn dap_source_info(file: &SourceFile, reference: u64) -> DapSourceInfo {
    let name = file
        .path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("source.orv")
        .to_string();
    DapSourceInfo {
        reference,
        name,
        path: file.path.clone(),
        uri: lsp_file_uri_for_path(&file.path),
        checksum: sha256_hex(file.source.as_bytes()),
    }
}

pub(crate) fn dap_source_json(source: &DapSourceInfo) -> serde_json::Value {
    dap_source_json_with_reference(source, source.reference)
}

pub(crate) fn dap_source_json_with_reference(
    source: &DapSourceInfo,
    source_reference: u64,
) -> serde_json::Value {
    serde_json::json!({
        "name": source.name,
        "path": source.path.display().to_string(),
        "sourceReference": source_reference,
        "uri": source.uri,
        "checksums": [
            {
                "algorithm": "SHA256",
                "checksum": source.checksum,
            },
        ],
    })
}

pub(crate) fn dap_module_json(source: &DapSourceInfo) -> serde_json::Value {
    serde_json::json!({
        "id": source.reference,
        "name": source.name,
        "path": source.path.display().to_string(),
        "isUserCode": true,
        "symbolStatus": "loaded",
    })
}

pub(crate) fn dap_goto_target_json(source: &DapSourceInfo, line: u64) -> serde_json::Value {
    serde_json::json!({
        "id": dap_goto_target_id(source.reference, line),
        "label": format!("{}:{line}", source.name),
        "line": line,
        "column": 1,
    })
}

pub(crate) const fn dap_goto_target_id(source_reference: u64, line: u64) -> u64 {
    source_reference
        .saturating_mul(1_000_000)
        .saturating_add(line)
}

pub(crate) fn dap_launch_live(request: &serde_json::Value) -> bool {
    request
        .pointer("/arguments/live")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub(crate) fn dap_launch_attach_runtime(request: &serde_json::Value) -> bool {
    request
        .pointer("/arguments/attachRuntime")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub(crate) fn dap_launch_attach_runtime_mode(
    request: &serde_json::Value,
) -> anyhow::Result<DapRuntimeAttachMode> {
    match request
        .pointer("/arguments/attachRuntimeMode")
        .and_then(serde_json::Value::as_str)
    {
        None | Some("process") => Ok(DapRuntimeAttachMode::Process),
        Some("inProcess" | "in-process") => Ok(DapRuntimeAttachMode::InProcess),
        Some(mode) => anyhow::bail!("unsupported attachRuntimeMode `{mode}`"),
    }
}

pub(crate) fn dap_launch_runtime_request_trace_path(
    request: &serde_json::Value,
) -> anyhow::Result<Option<PathBuf>> {
    request
        .pointer("/arguments/runtimeRequestTracePath")
        .or_else(|| request.pointer("/arguments/requestTracePath"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(dap_path_from_protocol_string)
        .transpose()
}

pub(crate) fn dap_launch_executable_lines(entry_path: &Path, frames: &[DapFrameState]) -> Vec<u64> {
    let mut executable_lines = if frames.is_empty() {
        dap_verified_breakpoint_lines(entry_path).unwrap_or_else(|_| vec![1])
    } else {
        frames.iter().map(|frame| frame.line).collect::<Vec<_>>()
    };
    if executable_lines.is_empty() {
        executable_lines.push(1);
    }
    executable_lines.sort_unstable();
    executable_lines.dedup();
    executable_lines
}

pub(crate) fn dap_program_path(request: &serde_json::Value) -> anyhow::Result<PathBuf> {
    let program = request
        .pointer("/arguments/program")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("launch.arguments.program must be a path or file URI"))?;
    dap_path_from_protocol_string(program)
}

pub(crate) fn dap_loaded_project_for_launch(
    request: &serde_json::Value,
    path: &Path,
) -> anyhow::Result<DapLaunchProject> {
    let Some(source_bundle_path) = dap_launch_source_bundle_path(request)? else {
        return Ok(DapLaunchProject {
            loaded: orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?,
            entry_path_for_lookup: path.to_path_buf(),
            source_bundle: None,
        });
    };
    let source_bundle = read_source_bundle_artifact(&source_bundle_path)?;
    let entry = source_bundle_entry_path(&source_bundle)?;
    let hash = stable_json_hash(&serde_json::to_value(&source_bundle)?)?;
    let source_bundle_meta = DapLaunchSourceBundle {
        path: source_bundle_path,
        entry: PathBuf::from(&source_bundle.entry),
        file_count: source_bundle.files.len(),
        hash,
    };
    let loaded = load_project_from_source_bundle_artifact(&source_bundle)?;
    Ok(DapLaunchProject {
        loaded,
        entry_path_for_lookup: entry,
        source_bundle: Some(source_bundle_meta),
    })
}

pub(crate) fn dap_launch_source_bundle_json(
    bundle: Option<&DapLaunchSourceBundle>,
) -> serde_json::Value {
    bundle.map_or(serde_json::Value::Null, |bundle| {
        serde_json::json!({
            "path": bundle.path.display().to_string(),
            "entry": bundle.entry.display().to_string(),
            "fileCount": bundle.file_count,
            "hash": bundle.hash,
        })
    })
}

pub(crate) fn dap_launch_source_bundle_path(
    request: &serde_json::Value,
) -> anyhow::Result<Option<PathBuf>> {
    request
        .pointer("/arguments/sourceBundle")
        .or_else(|| request.pointer("/arguments/source_bundle"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(dap_path_from_protocol_string)
        .transpose()
}

pub(crate) fn dap_source_path(request: &serde_json::Value) -> anyhow::Result<PathBuf> {
    let path = request
        .pointer("/arguments/source/path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("source.path must be a path or file URI"))?;
    dap_path_from_protocol_string(path)
}

pub(crate) fn dap_source_reference(request: &serde_json::Value) -> Option<u64> {
    request
        .pointer("/arguments/sourceReference")
        .and_then(serde_json::Value::as_u64)
        .filter(|reference| *reference > 0)
}

pub(crate) fn dap_breakpoint_source_path(
    launched: Option<&DapLaunchState>,
    request: &serde_json::Value,
) -> anyhow::Result<PathBuf> {
    if let Some(reference) = request
        .pointer("/arguments/source/sourceReference")
        .and_then(serde_json::Value::as_u64)
        .filter(|reference| *reference > 0)
    {
        let launched = launched
            .ok_or_else(|| anyhow::anyhow!("launch is required before sourceReference lookup"))?;
        return launched
            .sources
            .iter()
            .find(|source| source.reference == reference)
            .map(|source| source.path.clone())
            .ok_or_else(|| anyhow::anyhow!("unknown sourceReference {reference}"));
    }
    let path = request
        .pointer("/arguments/source/path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("source.path must be a path or file URI"))?;
    dap_path_from_protocol_string(path)
}

pub(crate) fn dap_path_from_protocol_string(path: &str) -> anyhow::Result<PathBuf> {
    if path.starts_with("file://") {
        lsp_file_uri_path(path)
    } else {
        Ok(PathBuf::from(path))
    }
}

pub(crate) fn dap_normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn dap_success_response(
    seq: u64,
    request_seq: u64,
    command: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "response",
        "request_seq": request_seq,
        "success": true,
        "command": command,
        "body": body,
    })
}

pub(crate) fn dap_error_response(
    seq: u64,
    request_seq: u64,
    command: &str,
    message: &str,
) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "response",
        "request_seq": request_seq,
        "success": false,
        "command": command,
        "message": message,
    })
}

pub(crate) fn dap_event_response(
    seq: u64,
    event: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "event",
        "event": event,
        "body": body,
    })
}

pub(crate) fn lsp_text_document_uri(request: &serde_json::Value) -> anyhow::Result<&str> {
    request
        .pointer("/params/textDocument/uri")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("textDocument.uri must be a file URI"))
}

pub(crate) fn lsp_text_document_position(
    request: &serde_json::Value,
) -> anyhow::Result<(usize, usize)> {
    let position = request
        .pointer("/params/position")
        .ok_or_else(|| anyhow::anyhow!("position must be an object"))?;
    lsp_position_value(position)
}

pub(crate) fn lsp_request_range(
    request: &serde_json::Value,
) -> anyhow::Result<((usize, usize), (usize, usize))> {
    let start = request
        .pointer("/params/range/start")
        .ok_or_else(|| anyhow::anyhow!("range.start must be an object"))?;
    let end = request
        .pointer("/params/range/end")
        .ok_or_else(|| anyhow::anyhow!("range.end must be an object"))?;
    Ok((lsp_position_value(start)?, lsp_position_value(end)?))
}

pub(crate) fn lsp_position_value(value: &serde_json::Value) -> anyhow::Result<(usize, usize)> {
    let line = value
        .get("line")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("position.line must be an integer"))?;
    let character = value
        .get("character")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("position.character must be an integer"))?;
    Ok((
        usize::try_from(line).map_err(|_| anyhow::anyhow!("position.line is too large"))?,
        usize::try_from(character)
            .map_err(|_| anyhow::anyhow!("position.character is too large"))?,
    ))
}

pub(crate) fn lsp_formatting_tab_size(request: &serde_json::Value) -> usize {
    request
        .pointer("/params/options/tabSize")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(2)
        .clamp(1, 8)
}

pub(crate) fn lsp_formatting_insert_spaces(request: &serde_json::Value) -> bool {
    request
        .pointer("/params/options/insertSpaces")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}

pub(crate) fn lsp_format_source(source: &str, tab_size: usize, insert_spaces: bool) -> String {
    lsp_format_source_with_initial_indent(source, tab_size, insert_spaces, 0)
}

pub(crate) fn lsp_format_source_with_initial_indent(
    source: &str,
    tab_size: usize,
    insert_spaces: bool,
    initial_indent: usize,
) -> String {
    let indent_unit = if insert_spaces {
        " ".repeat(tab_size)
    } else {
        "\t".to_string()
    };
    let mut formatted = Vec::new();
    let mut indent = initial_indent;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            formatted.push(String::new());
            continue;
        }
        let (line_indent, next_indent) = lsp_format_line_indent(indent, trimmed);
        let mut next = indent_unit.repeat(line_indent);
        next.push_str(trimmed);
        formatted.push(next);
        indent = next_indent;
    }
    if formatted.is_empty() {
        String::new()
    } else {
        format!("{}\n", formatted.join("\n"))
    }
}

pub(crate) fn lsp_format_line_indent(indent: usize, trimmed: &str) -> (usize, usize) {
    let leading_close = lsp_leading_closing_braces(trimmed).min(indent);
    let line_indent = indent.saturating_sub(leading_close);
    let (opens, closes) = lsp_line_brace_counts(trimmed);
    let non_leading_closes = closes.saturating_sub(leading_close);
    (
        line_indent,
        line_indent
            .saturating_add(opens)
            .saturating_sub(non_leading_closes),
    )
}

pub(crate) fn lsp_leading_closing_braces(trimmed: &str) -> usize {
    trimmed.chars().take_while(|ch| *ch == '}').count()
}

pub(crate) fn lsp_line_brace_counts(line: &str) -> (usize, usize) {
    let mut opens = 0usize;
    let mut closes = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(quote_ch) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote_ch {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '/' if chars.peek() == Some(&'/') => break,
            '{' => opens += 1,
            '}' => closes += 1,
            _ => {}
        }
    }
    (opens, closes)
}

pub(crate) fn lsp_full_document_range(source: &str) -> serde_json::Value {
    lsp_range_for_source(source, 0, u32::try_from(source.len()).unwrap_or(u32::MAX))
}

pub(crate) fn lsp_indent_level_before(source: &str, byte: usize) -> usize {
    let prefix = source.get(..byte.min(source.len())).unwrap_or(source);
    let mut indent = 0usize;
    for line in prefix.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        indent = lsp_format_line_indent(indent, trimmed).1;
    }
    indent
}

pub(crate) fn lsp_line_range_for_formatting(
    source: &str,
    requested_range: ((usize, usize), (usize, usize)),
) -> (usize, usize, serde_json::Value) {
    let ((start_line, _), end_position) = requested_range;
    let start = lsp_line_start_byte(source, start_line);
    let end_line = if end_position.1 == 0 {
        end_position.0
    } else {
        end_position.0.saturating_add(1)
    };
    let end = lsp_line_start_byte(source, end_line).max(start);
    (
        start,
        end,
        lsp_range_for_source(
            source,
            u32::try_from(start).unwrap_or(u32::MAX),
            u32::try_from(end).unwrap_or(u32::MAX),
        ),
    )
}

pub(crate) fn lsp_current_line_range_for_formatting(
    source: &str,
    line: usize,
) -> (usize, usize, serde_json::Value) {
    let start = lsp_line_start_byte(source, line);
    let end = lsp_line_start_byte(source, line.saturating_add(1)).max(start);
    (
        start,
        end,
        lsp_range_for_source(
            source,
            u32::try_from(start).unwrap_or(u32::MAX),
            u32::try_from(end).unwrap_or(u32::MAX),
        ),
    )
}

pub(crate) fn lsp_newline_on_type_formatting_edit_json(
    source: &str,
    line: usize,
    tab_size: usize,
    insert_spaces: bool,
) -> serde_json::Value {
    let (start, end, edit_range) = lsp_current_line_range_for_formatting(source, line);
    let Some(source_slice) = source.get(start..end) else {
        return serde_json::Value::Array(Vec::new());
    };
    if !source_slice.trim().is_empty() {
        return serde_json::Value::Array(Vec::new());
    }
    let indent_unit = if insert_spaces {
        " ".repeat(tab_size)
    } else {
        "\t".to_string()
    };
    let mut new_text = indent_unit.repeat(lsp_indent_level_before(source, start));
    if source_slice.ends_with('\n') {
        new_text.push('\n');
    }
    if new_text == source_slice {
        return serde_json::Value::Array(Vec::new());
    }
    serde_json::Value::Array(vec![serde_json::json!({
        "range": edit_range,
        "newText": new_text,
    })])
}

pub(crate) fn lsp_line_start_byte(source: &str, target_line: usize) -> usize {
    if target_line == 0 {
        return 0;
    }
    let mut line = 0usize;
    for (byte, ch) in source.char_indices() {
        if ch == '\n' {
            line += 1;
            if line == target_line {
                return byte.saturating_add(1).min(source.len());
            }
        }
    }
    source.len()
}

pub(crate) fn lsp_diagnostics_for_loaded_project(
    loaded: &orv_project::LoadedProject,
) -> Vec<serde_json::Value> {
    let diagnostics = lsp_project_diagnostics(loaded);
    lsp_diagnostics_json(&diagnostics, &loaded.files)
}

pub(crate) fn lsp_project_diagnostics(
    loaded: &orv_project::LoadedProject,
) -> Vec<orv_diagnostics::Diagnostic> {
    let resolved = orv_resolve::resolve(&loaded.program);
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    let mut diagnostics = Vec::new();
    diagnostics.extend(loaded.diagnostics.clone());
    diagnostics.extend(resolved.diagnostics);
    diagnostics.extend(lowered.diagnostics);
    diagnostics
}

pub(crate) fn lsp_workspace_diagnostic_items_json(
    loaded: &orv_project::LoadedProject,
) -> Vec<serde_json::Value> {
    let resolved = orv_resolve::resolve(&loaded.program);
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    loaded
        .files
        .iter()
        .filter_map(|file| {
            let mut diagnostics = Vec::new();
            diagnostics.extend(lsp_diagnostics_json_for_file(
                &loaded.diagnostics,
                &loaded.files,
                file.id,
            ));
            diagnostics.extend(lsp_diagnostics_json_for_file(
                &resolved.diagnostics,
                &loaded.files,
                file.id,
            ));
            diagnostics.extend(lsp_diagnostics_json_for_file(
                &lowered.diagnostics,
                &loaded.files,
                file.id,
            ));
            if diagnostics.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "uri": lsp_file_uri_for_path(&file.path),
                "version": serde_json::Value::Null,
                "kind": "full",
                "items": diagnostics,
            }))
        })
        .collect()
}

pub(crate) fn lsp_source_file_for_path<'a>(
    files: &'a [SourceFile],
    path: &Path,
) -> Option<&'a SourceFile> {
    let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    files
        .iter()
        .find(|file| file.path == path || file.path == normalized)
}

pub(crate) fn lsp_definition_node<'a>(
    graph: &'a ProjectGraph,
    name: &str,
) -> Option<&'a orv_project::ProjectNode> {
    graph.nodes.iter().find(|node| {
        node.name == name
            && matches!(
                node.kind,
                ProjectNodeKind::Struct
                    | ProjectNodeKind::Enum
                    | ProjectNodeKind::TypeAlias
                    | ProjectNodeKind::Function
                    | ProjectNodeKind::Define
            )
    })
}

pub(crate) fn lsp_type_definition_node<'a>(
    graph: &'a ProjectGraph,
    name: &str,
) -> Option<&'a orv_project::ProjectNode> {
    graph.nodes.iter().find(|node| {
        node.name == name
            && matches!(
                node.kind,
                ProjectNodeKind::Struct | ProjectNodeKind::Enum | ProjectNodeKind::TypeAlias
            )
    })
}

pub(crate) fn lsp_function_stmt_by_name<'a>(
    program: &'a Program,
    name: &str,
) -> Option<&'a FunctionStmt> {
    program.items.iter().find_map(|stmt| match stmt {
        Stmt::Function(function) if function.name.name == name => Some(function.as_ref()),
        _ => None,
    })
}

pub(crate) fn lsp_function_stmts(program: &Program) -> Vec<&FunctionStmt> {
    program
        .items
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Function(function) => Some(function.as_ref()),
            _ => None,
        })
        .collect()
}

pub(crate) fn lsp_call_hierarchy_item_json(
    function: &FunctionStmt,
    files: &[SourceFile],
) -> serde_json::Value {
    let uri = files
        .iter()
        .find(|file| file.id == function.span.file)
        .map_or_else(
            || "file://<unknown>".to_string(),
            |file| lsp_file_uri_for_path(&file.path),
        );
    serde_json::json!({
        "name": function.name.name,
        "kind": 12,
        "detail": "function",
        "uri": uri,
        "range": lsp_range_json(function.span, files),
        "selectionRange": lsp_range_json(function.name.span, files),
    })
}

pub(crate) fn lsp_type_hierarchy_item_json(
    node: &orv_project::ProjectNode,
    files: &[SourceFile],
) -> serde_json::Value {
    let uri = files.iter().find(|file| file.id == node.file).map_or_else(
        || "file://<unknown>".to_string(),
        |file| lsp_file_uri_for_path(&file.path),
    );
    let selection_range = files
        .iter()
        .find(|file| file.id == node.file)
        .and_then(|file| {
            lsp_node_name_span(&file.source, node)
                .map(|(start, end)| lsp_range_for_source(&file.source, start, end))
        })
        .unwrap_or_else(|| lsp_range_json(node.span, files));
    serde_json::json!({
        "name": node.name,
        "kind": lsp_symbol_kind_code(node.kind).unwrap_or(23),
        "detail": lsp_symbol_kind(node.kind).unwrap_or("Type"),
        "uri": uri,
        "range": lsp_range_json(node.span, files),
        "selectionRange": selection_range,
        "data": {
            "source_node": node.id,
        },
    })
}

pub(crate) fn lsp_moniker_json(node: &orv_project::ProjectNode) -> serde_json::Value {
    serde_json::json!({
        "scheme": "orv",
        "identifier": format!("{}:{}", lsp_moniker_symbol_kind(node.kind), node.name),
        "unique": "project",
        "kind": "export",
        "data": {
            "source_node": node.id,
        },
    })
}

pub(crate) const fn lsp_moniker_symbol_kind(kind: ProjectNodeKind) -> &'static str {
    match kind {
        ProjectNodeKind::Struct => "struct",
        ProjectNodeKind::Enum => "enum",
        ProjectNodeKind::TypeAlias => "type",
        ProjectNodeKind::Function | ProjectNodeKind::Define => "function",
        ProjectNodeKind::Domain => "domain",
        ProjectNodeKind::File | ProjectNodeKind::Import => "symbol",
    }
}

pub(crate) fn lsp_document_colors_json(source: &str) -> Vec<serde_json::Value> {
    let bytes = source.as_bytes();
    let mut colors = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'#' {
            if let Some((length, red, green, blue)) = lsp_hex_color_at(bytes, index) {
                let start = u32::try_from(index).unwrap_or(u32::MAX);
                let end = u32::try_from(index.saturating_add(length)).unwrap_or(u32::MAX);
                colors.push(serde_json::json!({
                    "range": lsp_range_for_source(source, start, end),
                    "color": {
                        "red": f64::from(red) / 255.0,
                        "green": f64::from(green) / 255.0,
                        "blue": f64::from(blue) / 255.0,
                        "alpha": 1.0,
                    },
                }));
                index = index.saturating_add(length);
                continue;
            }
        }
        index = index.saturating_add(1);
    }
    colors
}

pub(crate) fn lsp_hex_color_at(bytes: &[u8], index: usize) -> Option<(usize, u8, u8, u8)> {
    let start = index.checked_add(1)?;
    lsp_hex_color_with_digits(bytes, start, 6)
        .map(|(red, green, blue)| (7, red, green, blue))
        .or_else(|| {
            lsp_hex_color_with_digits(bytes, start, 3)
                .map(|(red, green, blue)| (4, red, green, blue))
        })
}

pub(crate) fn lsp_hex_color_with_digits(
    bytes: &[u8],
    start: usize,
    digits: usize,
) -> Option<(u8, u8, u8)> {
    let end = start.checked_add(digits)?;
    if end > bytes.len()
        || bytes
            .get(end)
            .and_then(|byte| lsp_hex_value(*byte))
            .is_some()
    {
        return None;
    }
    match digits {
        6 => Some((
            lsp_hex_pair(bytes[start], bytes[start + 1])?,
            lsp_hex_pair(bytes[start + 2], bytes[start + 3])?,
            lsp_hex_pair(bytes[start + 4], bytes[start + 5])?,
        )),
        3 => {
            let red = lsp_hex_value(bytes[start])?;
            let green = lsp_hex_value(bytes[start + 1])?;
            let blue = lsp_hex_value(bytes[start + 2])?;
            Some((red * 17, green * 17, blue * 17))
        }
        _ => None,
    }
}

pub(crate) fn lsp_hex_pair(high: u8, low: u8) -> Option<u8> {
    Some(lsp_hex_value(high)? * 16 + lsp_hex_value(low)?)
}

pub(crate) const fn lsp_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn lsp_color_param(request: &serde_json::Value) -> anyhow::Result<(u8, u8, u8, u8)> {
    let color = request
        .pointer("/params/color")
        .ok_or_else(|| anyhow::anyhow!("color must be an object"))?;
    Ok((
        lsp_color_channel_param(color, "red")?,
        lsp_color_channel_param(color, "green")?,
        lsp_color_channel_param(color, "blue")?,
        lsp_color_channel_param(color, "alpha")?,
    ))
}

pub(crate) fn lsp_color_channel_param(
    color: &serde_json::Value,
    field: &str,
) -> anyhow::Result<u8> {
    let value = color
        .get(field)
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("color.{field} must be a number"))?;
    Ok(lsp_color_channel(value))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn lsp_color_channel(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub(crate) fn lsp_hex_color_label(red: u8, green: u8, blue: u8, alpha: u8) -> String {
    if alpha == u8::MAX {
        format!("#{red:02x}{green:02x}{blue:02x}")
    } else {
        format!("#{red:02x}{green:02x}{blue:02x}{alpha:02x}")
    }
}

pub(crate) fn lsp_call_hierarchy_outgoing_calls(
    caller: &FunctionStmt,
    program: &Program,
    files: &[SourceFile],
) -> Vec<serde_json::Value> {
    let Some(source) = lsp_source_file_for_span(files, caller.span) else {
        return Vec::new();
    };
    lsp_function_stmts(program)
        .into_iter()
        .filter_map(|callee| {
            let ranges = lsp_function_call_ranges(&source.source, caller, &callee.name.name);
            if ranges.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "to": lsp_call_hierarchy_item_json(callee, files),
                "fromRanges": ranges,
            }))
        })
        .collect()
}

pub(crate) fn lsp_call_hierarchy_incoming_calls(
    callee_name: &str,
    program: &Program,
    files: &[SourceFile],
) -> Vec<serde_json::Value> {
    lsp_function_stmts(program)
        .into_iter()
        .filter_map(|caller| {
            let source = lsp_source_file_for_span(files, caller.span)?;
            let ranges = lsp_function_call_ranges(&source.source, caller, callee_name);
            if ranges.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "from": lsp_call_hierarchy_item_json(caller, files),
                "fromRanges": ranges,
            }))
        })
        .collect()
}

pub(crate) fn lsp_function_call_ranges(
    source: &str,
    caller: &FunctionStmt,
    callee_name: &str,
) -> Vec<serde_json::Value> {
    let mut ranges = Vec::new();
    let mut search_from = usize::try_from(caller.span.range.start).unwrap_or(usize::MAX);
    let end = usize::try_from(caller.span.range.end)
        .unwrap_or(usize::MAX)
        .min(source.len());
    search_from = search_from.min(end);
    while let Some(relative) = source[search_from..end].find(callee_name) {
        let name_start = search_from + relative;
        let Some(open) = lsp_call_open_after_name(source, name_start, callee_name) else {
            search_from = name_start.saturating_add(callee_name.len());
            continue;
        };
        if lsp_call_is_function_declaration(source, name_start) {
            search_from = open.saturating_add(1);
            continue;
        }
        let name_end = name_start.saturating_add(callee_name.len());
        ranges.push(lsp_range_for_source(
            source,
            u32::try_from(name_start).unwrap_or(u32::MAX),
            u32::try_from(name_end).unwrap_or(u32::MAX),
        ));
        search_from = open.saturating_add(1);
    }
    ranges
}

pub(crate) fn lsp_source_file_for_span(files: &[SourceFile], span: Span) -> Option<&SourceFile> {
    files.iter().find(|file| file.id == span.file)
}

pub(crate) fn lsp_location_json(
    node: &orv_project::ProjectNode,
    files: &[SourceFile],
) -> serde_json::Value {
    let uri = files.iter().find(|file| file.id == node.file).map_or_else(
        || "file://<unknown>".to_string(),
        |file| lsp_file_uri_for_path(&file.path),
    );
    serde_json::json!({
        "uri": uri,
        "range": lsp_range_json(node.span, files),
    })
}

pub(crate) fn lsp_hover_json(
    node: &orv_project::ProjectNode,
    files: &[SourceFile],
) -> serde_json::Value {
    let kind = lsp_symbol_kind(node.kind).unwrap_or("Symbol");
    serde_json::json!({
        "contents": {
            "kind": "markdown",
            "value": format!("**{kind}** `{}`", node.name),
        },
        "range": lsp_range_json(node.span, files),
    })
}

#[derive(Clone, Copy)]
pub(crate) struct LspDomainFieldKind {
    pub(crate) domain: &'static str,
    pub(crate) marker: &'static str,
    pub(crate) label: &'static str,
}

pub(crate) struct LspDomainField<'a> {
    pub(crate) kind: LspDomainFieldKind,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) name: &'a str,
}

const LSP_DOMAIN_FIELD_KINDS: &[LspDomainFieldKind] = &[
    LspDomainFieldKind {
        domain: "body",
        marker: "@body.",
        label: "Request body field",
    },
    LspDomainFieldKind {
        domain: "param",
        marker: "@param.",
        label: "Route parameter",
    },
    LspDomainFieldKind {
        domain: "query",
        marker: "@query.",
        label: "Query parameter",
    },
    LspDomainFieldKind {
        domain: "env",
        marker: "@env.",
        label: "Environment value",
    },
];

pub(crate) fn lsp_domain_field_hover_json(source: &str, byte: usize) -> Option<serde_json::Value> {
    let field = lsp_domain_field_at_byte(source, byte)?;
    Some(serde_json::json!({
        "contents": {
            "kind": "markdown",
            "value": format!("**{}** `{}`", field.kind.label, field.name),
        },
        "range": lsp_range_for_source(
            source,
            u32::try_from(field.start).unwrap_or(u32::MAX),
            u32::try_from(field.end).unwrap_or(u32::MAX),
        ),
    }))
}

pub(crate) fn lsp_domain_field_at_byte(source: &str, byte: usize) -> Option<LspDomainField<'_>> {
    let (start, end, name) = identifier_span_at_byte(source, byte)?;
    let kind = lsp_domain_field_kind_at_name_start(source, start)?;
    Some(LspDomainField {
        kind,
        start,
        end,
        name,
    })
}

pub(crate) fn lsp_domain_field_kind_at_name_start(
    source: &str,
    name_start: usize,
) -> Option<LspDomainFieldKind> {
    LSP_DOMAIN_FIELD_KINDS.iter().copied().find(|kind| {
        name_start >= kind.marker.len()
            && source
                .as_bytes()
                .get(name_start - kind.marker.len()..name_start)
                == Some(kind.marker.as_bytes())
    })
}

pub(crate) fn lsp_domain_field_kind_for_domain(domain: &str) -> Option<LspDomainFieldKind> {
    LSP_DOMAIN_FIELD_KINDS
        .iter()
        .copied()
        .find(|kind| kind.domain == domain)
}

pub(crate) fn lsp_signature_help_json(
    function: &FunctionStmt,
    active_parameter: usize,
) -> serde_json::Value {
    let parameters = function
        .params
        .iter()
        .map(lsp_signature_parameter_label)
        .collect::<Vec<_>>();
    let label = lsp_signature_label(function, &parameters);
    let max_parameter = parameters.len().saturating_sub(1);
    serde_json::json!({
        "signatures": [
            {
                "label": label,
                "parameters": parameters
                    .iter()
                    .map(|parameter| serde_json::json!({ "label": parameter }))
                    .collect::<Vec<_>>(),
            },
        ],
        "activeSignature": 0,
        "activeParameter": active_parameter.min(max_parameter),
    })
}

pub(crate) fn lsp_signature_label(function: &FunctionStmt, parameters: &[String]) -> String {
    let mut label = format!("{}({})", function.name.name, parameters.join(", "));
    if let Some(return_ty) = &function.return_ty {
        label.push_str(": ");
        label.push_str(&type_ref_string(return_ty));
    }
    label
}

pub(crate) fn lsp_signature_parameter_label(param: &orv_syntax::ast::Param) -> String {
    param.ty.as_ref().map_or_else(
        || param.name.name.clone(),
        |ty| format!("{}: {}", param.name.name, type_ref_string(ty)),
    )
}

pub(crate) fn lsp_inlay_hints_json(
    program: &Program,
    source: &str,
    start: usize,
    end: usize,
) -> Vec<serde_json::Value> {
    let functions = program
        .items
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Function(function) => Some(function.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut hints = Vec::new();
    for function in functions {
        let mut search_from = start.min(source.len());
        let end = end.min(source.len());
        while let Some(relative) = source[search_from..end].find(function.name.name.as_str()) {
            let name_start = search_from + relative;
            let Some(open) = lsp_call_open_after_name(source, name_start, &function.name.name)
            else {
                search_from = name_start.saturating_add(function.name.name.len());
                continue;
            };
            if lsp_call_is_function_declaration(source, name_start) {
                search_from = open.saturating_add(1);
                continue;
            }
            for (index, argument_start) in lsp_call_argument_starts(source, open, end)
                .into_iter()
                .enumerate()
                .take(function.params.len())
            {
                let label = format!("{}:", function.params[index].name.name);
                let position =
                    byte_position(source, u32::try_from(argument_start).unwrap_or(u32::MAX));
                hints.push(serde_json::json!({
                    "position": {
                        "line": position.0,
                        "character": position.1,
                    },
                    "label": label,
                    "kind": 2,
                    "paddingRight": true,
                }));
            }
            search_from = open.saturating_add(1);
        }
    }
    hints
}

pub(crate) fn lsp_call_open_after_name(
    source: &str,
    name_start: usize,
    name: &str,
) -> Option<usize> {
    if name_start > 0 && is_identifier_byte(source.as_bytes()[name_start - 1]) {
        return None;
    }
    let name_end = name_start.checked_add(name.len())?;
    if source
        .as_bytes()
        .get(name_end)
        .is_some_and(|byte| is_identifier_byte(*byte))
    {
        return None;
    }
    let offset = source[name_end..].find(|ch: char| !ch.is_whitespace())?;
    let open = name_end + offset;
    (source.as_bytes().get(open) == Some(&b'(')).then_some(open)
}

pub(crate) fn lsp_call_is_function_declaration(source: &str, name_start: usize) -> bool {
    source[..name_start]
        .split_whitespace()
        .last()
        .is_some_and(|word| matches!(word, "function" | "define"))
}

pub(crate) fn lsp_call_argument_starts(source: &str, open: usize, end: usize) -> Vec<usize> {
    let mut starts = Vec::new();
    let bytes = source.as_bytes();
    let limit = end.min(bytes.len());
    let mut depth = 0usize;
    let mut index = open.saturating_add(1);
    while index < limit {
        match bytes[index] {
            b' ' | b'\t' | b'\n' | b'\r' if depth == 0 => {
                index += 1;
            }
            b')' if depth == 0 => break,
            _ => break,
        }
    }
    if index < limit && bytes[index] != b')' {
        starts.push(index);
    }
    while index < limit {
        match bytes[index] {
            b'(' | b'[' | b'{' => depth = depth.saturating_add(1),
            b')' if depth == 0 => break,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                index += 1;
                while index < limit && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                if index < limit && bytes[index] != b')' {
                    starts.push(index);
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    starts
}

pub(crate) fn lsp_call_signature_context(source: &str, byte: usize) -> Option<(&str, usize)> {
    let open = lsp_call_open_paren(source, byte)?;
    let name_end = source[..open].trim_end().len();
    let name = identifier_span_at_byte(source, name_end.checked_sub(1)?)?.2;
    let active_parameter = lsp_active_parameter_index(&source[open.saturating_add(1)..byte]);
    Some((name, active_parameter))
}

pub(crate) fn lsp_call_open_paren(source: &str, byte: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut index = byte.min(bytes.len());
    while index > 0 {
        index -= 1;
        match bytes[index] {
            b')' | b']' | b'}' => depth = depth.saturating_add(1),
            b'(' if depth == 0 => return Some(index),
            b'(' | b'[' | b'{' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

pub(crate) fn lsp_active_parameter_index(source: &str) -> usize {
    let mut depth = 0usize;
    let mut active = 0usize;
    for byte in source.bytes() {
        match byte {
            b'(' | b'[' | b'{' => depth = depth.saturating_add(1),
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => active = active.saturating_add(1),
            _ => {}
        }
    }
    active
}

pub(crate) fn lsp_file_uri_for_path(path: &Path) -> String {
    format!("file://{}", path.display())
}

pub(crate) fn lsp_position_to_byte(source: &str, position: (usize, usize)) -> usize {
    let (target_line, target_character) = position;
    let mut line = 0;
    let mut character = 0;
    for (byte, ch) in source.char_indices() {
        if line == target_line && character == target_character {
            return byte;
        }
        if ch == '\n' {
            if line == target_line {
                return byte;
            }
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }
    source.len()
}

pub(crate) fn identifier_at_byte(source: &str, byte: usize) -> Option<&str> {
    identifier_span_at_byte(source, byte).map(|(_, _, name)| name)
}

pub(crate) fn identifier_span_at_byte(source: &str, byte: usize) -> Option<(usize, usize, &str)> {
    let bytes = source.as_bytes();
    let byte = byte.min(bytes.len());
    let mut start = byte;
    while start > 0 && is_identifier_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = byte;
    while end < bytes.len() && is_identifier_byte(bytes[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    source.get(start..end).map(|name| (start, end, name))
}

pub(crate) fn lsp_renamable_identifier_span_at_byte(
    source: &str,
    byte: usize,
) -> Option<(usize, usize, &str)> {
    let (start, end, name) = identifier_span_at_byte(source, byte)?;
    if !lsp_renamable_identifier_name(name)
        || lsp_is_builtin_domain_identifier(source, start, name)
        || lsp_domain_field_kind_at_name_start(source, start).is_some()
    {
        return None;
    }
    Some((start, end, name))
}

pub(crate) fn lsp_reference_locations_json(
    files: &[SourceFile],
    name: &str,
) -> Vec<serde_json::Value> {
    files
        .iter()
        .flat_map(|file| {
            lsp_identifier_ranges_json(&file.source, name)
                .into_iter()
                .map(move |range| {
                    serde_json::json!({
                        "uri": lsp_file_uri_for_path(&file.path),
                        "range": range,
                    })
                })
        })
        .collect()
}

pub(crate) fn lsp_domain_field_reference_locations_json(
    files: &[SourceFile],
    kind: LspDomainFieldKind,
    name: &str,
) -> Vec<serde_json::Value> {
    files
        .iter()
        .flat_map(|file| {
            lsp_domain_field_occurrences(&file.source, kind, name)
                .into_iter()
                .map(move |(start, end)| {
                    serde_json::json!({
                        "uri": lsp_file_uri_for_path(&file.path),
                        "range": lsp_range_for_source(
                            &file.source,
                            u32::try_from(start).unwrap_or(u32::MAX),
                            u32::try_from(end).unwrap_or(u32::MAX),
                        ),
                    })
                })
        })
        .collect()
}

pub(crate) fn lsp_linked_editing_range_json(source: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "ranges": lsp_identifier_ranges_json(source, name),
        "wordPattern": "[A-Za-z_][A-Za-z0-9_]*",
    })
}

pub(crate) fn lsp_identifier_ranges_json(source: &str, name: &str) -> Vec<serde_json::Value> {
    identifier_occurrences(source, name)
        .into_iter()
        .map(|(start, end)| {
            lsp_range_for_source(
                source,
                u32::try_from(start).unwrap_or(u32::MAX),
                u32::try_from(end).unwrap_or(u32::MAX),
            )
        })
        .collect()
}

pub(crate) fn lsp_domain_field_occurrences(
    source: &str,
    kind: LspDomainFieldKind,
    name: &str,
) -> Vec<(usize, usize)> {
    lsp_domain_field_spans(source, kind)
        .into_iter()
        .filter_map(|(start, end, candidate)| (candidate == name).then_some((start, end)))
        .collect()
}

pub(crate) fn lsp_domain_field_spans(
    source: &str,
    kind: LspDomainFieldKind,
) -> Vec<(usize, usize, &str)> {
    let marker = kind.marker.as_bytes();
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut index = 0usize;
    while index <= bytes.len().saturating_sub(marker.len()) {
        if bytes.get(index..index + marker.len()) != Some(marker) {
            index += 1;
            continue;
        }
        let name_start = index + marker.len();
        let mut name_end = name_start;
        while bytes
            .get(name_end)
            .is_some_and(|byte| is_identifier_byte(*byte))
        {
            name_end += 1;
        }
        if name_end > name_start {
            if let Some(name) = source.get(name_start..name_end) {
                out.push((name_start, name_end, name));
            }
            index = name_end;
        } else {
            index = name_start.saturating_add(1);
        }
    }
    out
}

pub(crate) fn identifier_occurrences(source: &str, name: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if is_identifier_byte(bytes[index]) {
            let start = index;
            index += 1;
            while index < bytes.len() && is_identifier_byte(bytes[index]) {
                index += 1;
            }
            if source.get(start..index) == Some(name) {
                out.push((start, index));
            }
        } else {
            index += 1;
        }
    }
    out
}

pub(crate) const fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub(crate) fn lsp_valid_identifier_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_') && bytes.all(is_identifier_byte)
}

pub(crate) fn lsp_renamable_identifier_name(name: &str) -> bool {
    lsp_valid_identifier_name(name) && !lsp_reserved_identifier_name(name)
}

pub(crate) fn lsp_reserved_identifier_name(name: &str) -> bool {
    matches!(
        name,
        "let"
            | "mut"
            | "sig"
            | "const"
            | "function"
            | "async"
            | "await"
            | "return"
            | "if"
            | "else"
            | "when"
            | "for"
            | "in"
            | "while"
            | "break"
            | "continue"
            | "try"
            | "catch"
            | "throw"
            | "struct"
            | "enum"
            | "type"
            | "define"
            | "pub"
            | "import"
            | "void"
            | "as"
            | "true"
            | "false"
            | "null"
            | "int"
            | "float"
            | "string"
            | "bool"
    )
}

pub(crate) fn lsp_is_builtin_domain_identifier(source: &str, start: usize, name: &str) -> bool {
    let Some(previous) = start.checked_sub(1) else {
        return false;
    };
    if source.as_bytes().get(previous) != Some(&b'@') {
        return false;
    }
    name.bytes()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
}

pub(crate) fn lsp_file_uri_path(uri: &str) -> anyhow::Result<PathBuf> {
    let raw_path = uri
        .strip_prefix("file://")
        .ok_or_else(|| anyhow::anyhow!("textDocument.uri must use file://"))?;
    Ok(PathBuf::from(percent_decode_uri_path(raw_path)?))
}

pub(crate) fn percent_decode_uri_path(raw: &str) -> anyhow::Result<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = bytes
                .get(index + 1)
                .and_then(|byte| uri_hex_value(*byte))
                .ok_or_else(|| anyhow::anyhow!("invalid percent escape in file URI"))?;
            let lo = bytes
                .get(index + 2)
                .and_then(|byte| uri_hex_value(*byte))
                .ok_or_else(|| anyhow::anyhow!("invalid percent escape in file URI"))?;
            out.push((hi << 4) | lo);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|e| anyhow::anyhow!("file URI path is not UTF-8: {e}"))
}

pub(crate) const fn uri_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn lsp_jsonrpc_result(
    id: &serde_json::Value,
    result: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

pub(crate) fn lsp_jsonrpc_result_or_invalid_params(
    id: &serde_json::Value,
    result: anyhow::Result<serde_json::Value>,
) -> serde_json::Value {
    match result {
        Ok(result) => lsp_jsonrpc_result(id, &result),
        Err(err) => lsp_jsonrpc_error(id, -32602, &err.to_string()),
    }
}

pub(crate) fn lsp_jsonrpc_method_not_found(
    id: &serde_json::Value,
    method: &str,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32601,
            "message": "method not found",
            "data": {
                "method": method,
            },
        },
    })
}

pub(crate) fn lsp_jsonrpc_error(
    id: &serde_json::Value,
    code: i32,
    message: &str,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    })
}

pub(crate) fn lsp_diagnostics_json(
    diagnostics: &[orv_diagnostics::Diagnostic],
    files: &[SourceFile],
) -> Vec<serde_json::Value> {
    diagnostics
        .iter()
        .map(|diagnostic| lsp_diagnostic_json(diagnostic, files))
        .collect()
}

pub(crate) fn lsp_diagnostics_json_for_file(
    diagnostics: &[orv_diagnostics::Diagnostic],
    files: &[SourceFile],
    file_id: FileId,
) -> Vec<serde_json::Value> {
    diagnostics
        .iter()
        .filter(|diagnostic| lsp_diagnostic_file_id(diagnostic) == Some(file_id))
        .map(|diagnostic| lsp_diagnostic_json(diagnostic, files))
        .collect()
}

pub(crate) fn lsp_diagnostic_json(
    diagnostic: &orv_diagnostics::Diagnostic,
    files: &[SourceFile],
) -> serde_json::Value {
    let span = lsp_diagnostic_span(diagnostic);
    serde_json::json!({
        "source": "orv",
        "severity": lsp_severity(diagnostic.severity),
        "code": diagnostic.code,
        "message": diagnostic.message,
        "range": lsp_range_json(span, files),
    })
}

pub(crate) fn lsp_diagnostic_span(diagnostic: &orv_diagnostics::Diagnostic) -> Span {
    diagnostic
        .primary
        .as_ref()
        .map(|label| label.span)
        .or_else(|| diagnostic.secondary.first().map(|label| label.span))
        .unwrap_or(Span::DUMMY)
}

pub(crate) fn lsp_diagnostic_file_id(diagnostic: &orv_diagnostics::Diagnostic) -> Option<FileId> {
    diagnostic
        .primary
        .as_ref()
        .map(|label| label.span.file)
        .or_else(|| diagnostic.secondary.first().map(|label| label.span.file))
}

pub(crate) fn lsp_document_symbols_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
) -> Vec<serde_json::Value> {
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            lsp_symbol_kind(node.kind).map(|kind| {
                serde_json::json!({
                    "name": node.name,
                    "kind": kind,
                    "range": lsp_range_json(node.span, files),
                    "selectionRange": lsp_range_json(node.span, files),
                    "source_node": node.id,
                })
            })
        })
        .collect()
}

pub(crate) fn lsp_document_symbols_protocol_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
) -> Vec<serde_json::Value> {
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            lsp_symbol_kind_code(node.kind).map(|kind| {
                serde_json::json!({
                    "name": node.name,
                    "kind": kind,
                    "range": lsp_range_json(node.span, files),
                    "selectionRange": lsp_range_json(node.span, files),
                    "data": {
                        "source_node": node.id,
                    },
                })
            })
        })
        .collect()
}

pub(crate) fn lsp_code_lenses_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
    file_id: FileId,
) -> Vec<serde_json::Value> {
    graph
        .nodes
        .iter()
        .filter(|node| node.file == file_id)
        .filter_map(|node| {
            let kind = lsp_symbol_kind(node.kind)?;
            Some(serde_json::json!({
                "range": lsp_range_json(node.span, files),
                "command": {
                    "title": format!("Reveal {kind} {}", node.name),
                    "command": "orv.revealSourceNode",
                    "arguments": [node.id, node.name],
                },
                "data": {
                    "source_node": node.id,
                },
            }))
        })
        .collect()
}

pub(crate) fn lsp_code_actions_json(
    loaded: &orv_project::LoadedProject,
    file: &SourceFile,
    requested_start: usize,
    requested_end: usize,
) -> Vec<serde_json::Value> {
    let uri = lsp_file_uri_for_path(&file.path);
    let start = u32::try_from(requested_start.min(requested_end)).unwrap_or(u32::MAX);
    let end = u32::try_from(requested_start.max(requested_end)).unwrap_or(u32::MAX);
    lsp_project_diagnostics(loaded)
        .iter()
        .filter(|diagnostic| lsp_diagnostic_file_id(diagnostic) == Some(file.id))
        .filter(|diagnostic| lsp_span_overlaps_range(lsp_diagnostic_span(diagnostic), start, end))
        .flat_map(|diagnostic| {
            let diagnostic_json = lsp_diagnostic_json(diagnostic, &loaded.files);
            let range = diagnostic_json
                .get("range")
                .cloned()
                .unwrap_or_else(|| lsp_range_for_source(&file.source, start, end));
            let mut actions =
                lsp_diagnostic_edit_code_actions_json(diagnostic, &diagnostic_json, &uri, &range);
            actions.push(serde_json::json!({
                "title": format!("Reveal diagnostic: {}", diagnostic.message),
                "kind": "quickfix",
                "diagnostics": [diagnostic_json],
                "command": {
                    "title": "Reveal diagnostic",
                    "command": "orv.revealDiagnostic",
                    "arguments": [
                        uri,
                        range,
                        diagnostic.code.clone().unwrap_or_default(),
                        diagnostic.message,
                    ],
                },
            }));
            actions
        })
        .collect()
}

pub(crate) fn lsp_diagnostic_edit_code_actions_json(
    diagnostic: &orv_diagnostics::Diagnostic,
    diagnostic_json: &serde_json::Value,
    uri: &str,
    range: &serde_json::Value,
) -> Vec<serde_json::Value> {
    match (diagnostic.code.as_deref(), diagnostic.message.as_str()) {
        (Some("syntax/route-method"), _) | (None, "expected HTTP method after `@route`") => {
            lsp_insert_text_code_action_json(
                "Insert default GET route head",
                uri,
                range,
                "GET /path ",
                diagnostic_json,
            )
            .into_iter()
            .collect()
        }
        (Some("syntax/route-path"), _)
        | (None, "expected path starting with `/` or `*` after HTTP method") => {
            lsp_insert_text_code_action_json(
                "Insert default route path",
                uri,
                range,
                "/path ",
                diagnostic_json,
            )
            .into_iter()
            .collect()
        }
        _ => Vec::new(),
    }
}

pub(crate) fn lsp_insert_text_code_action_json(
    title: &str,
    uri: &str,
    range: &serde_json::Value,
    new_text: &str,
    diagnostic_json: &serde_json::Value,
) -> Option<serde_json::Value> {
    let start = range.get("start")?.clone();
    let edit_range = serde_json::json!({
        "start": start,
        "end": start,
    });
    let mut changes = serde_json::Map::new();
    changes.insert(
        uri.to_string(),
        serde_json::json!([{
            "range": edit_range,
            "newText": new_text,
        }]),
    );
    Some(serde_json::json!({
        "title": title,
        "kind": "quickfix",
        "diagnostics": [diagnostic_json.clone()],
        "edit": {
            "changes": changes,
        },
    }))
}

pub(crate) fn lsp_execute_reveal_diagnostic_json(request: &serde_json::Value) -> serde_json::Value {
    let uri = request
        .pointer("/params/arguments/0")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let range = request
        .pointer("/params/arguments/1")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let code = request
        .pointer("/params/arguments/2")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let message = request
        .pointer("/params/arguments/3")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "command": "orv.revealDiagnostic",
        "uri": uri,
        "range": range,
        "code": code,
        "message": message,
    })
}

pub(crate) const fn lsp_span_overlaps_range(span: Span, start: u32, end: u32) -> bool {
    span.range.start <= end && start <= span.range.end
}

pub(crate) fn lsp_document_links_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
    file_id: FileId,
) -> Vec<serde_json::Value> {
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == ProjectNodeKind::Import && node.file == file_id)
        .filter_map(|node| {
            let target = graph
                .edges
                .iter()
                .find(|edge| edge.kind == ProjectEdgeKind::Imports && edge.from == node.id)?;
            let target_node = graph
                .nodes
                .iter()
                .find(|candidate| candidate.id == target.to)?;
            let target_file = files.iter().find(|file| file.id == target_node.file)?;
            Some(serde_json::json!({
                "range": lsp_range_json(node.span, files),
                "target": lsp_file_uri_for_path(&target_file.path),
                "tooltip": format!("Open {}", target_node.name),
            }))
        })
        .collect()
}

pub(crate) fn lsp_folding_ranges_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
    file_id: FileId,
) -> Vec<serde_json::Value> {
    graph
        .nodes
        .iter()
        .filter(|node| node.file == file_id)
        .filter(|node| {
            matches!(
                node.kind,
                ProjectNodeKind::Struct
                    | ProjectNodeKind::Enum
                    | ProjectNodeKind::TypeAlias
                    | ProjectNodeKind::Function
                    | ProjectNodeKind::Define
                    | ProjectNodeKind::Domain
            )
        })
        .filter_map(|node| lsp_folding_range_json(node.span, files))
        .collect()
}

pub(crate) fn lsp_folding_range_json(
    span: Span,
    files: &[SourceFile],
) -> Option<serde_json::Value> {
    let file = files.iter().find(|file| file.id == span.file)?;
    let start = byte_position(&file.source, span.range.start);
    let end = byte_position(&file.source, span.range.end);
    if end.0 <= start.0 {
        return None;
    }
    Some(serde_json::json!({
        "startLine": start.0,
        "startCharacter": start.1,
        "endLine": end.0,
        "endCharacter": end.1,
        "kind": "region",
    }))
}

pub(crate) fn lsp_selection_range_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
    file_id: FileId,
    byte: usize,
) -> Option<serde_json::Value> {
    let byte = u32::try_from(byte).unwrap_or(u32::MAX);
    let mut nodes: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.file == file_id)
        .filter(|node| lsp_selectable_node_kind(node.kind))
        .filter(|node| node.span.range.start <= byte && byte <= node.span.range.end)
        .collect();
    nodes.sort_by_key(|node| node.span.range.end.saturating_sub(node.span.range.start));

    let mut current = None;
    for node in nodes.into_iter().rev() {
        current = Some(serde_json::json!({
            "range": lsp_range_json(node.span, files),
            "parent": current.unwrap_or(serde_json::Value::Null),
        }));
    }
    current
}

pub(crate) const fn lsp_selectable_node_kind(kind: ProjectNodeKind) -> bool {
    matches!(
        kind,
        ProjectNodeKind::Struct
            | ProjectNodeKind::Enum
            | ProjectNodeKind::TypeAlias
            | ProjectNodeKind::Function
            | ProjectNodeKind::Define
            | ProjectNodeKind::Domain
            | ProjectNodeKind::Import
    )
}

#[derive(Clone, Copy)]
pub(crate) struct LspSemanticToken {
    pub(crate) line: usize,
    pub(crate) character: usize,
    pub(crate) length: usize,
    pub(crate) token_type: u32,
    pub(crate) modifiers: u32,
}

pub(crate) fn lsp_semantic_tokens_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
    file_id: FileId,
) -> serde_json::Value {
    let Some(file) = files.iter().find(|file| file.id == file_id) else {
        return serde_json::json!({ "data": [] });
    };
    let mut tokens = graph
        .nodes
        .iter()
        .filter(|node| node.file == file_id)
        .filter_map(|node| {
            let token_type = lsp_semantic_token_type(node.kind)?;
            let (start, end) = lsp_node_name_span(&file.source, node)?;
            let start = byte_position(&file.source, start);
            let end = byte_position(&file.source, end);
            if start.0 != end.0 || end.1 <= start.1 {
                return None;
            }
            Some(LspSemanticToken {
                line: start.0,
                character: start.1,
                length: end.1 - start.1,
                token_type,
                modifiers: 1,
            })
        })
        .collect::<Vec<_>>();
    tokens.sort_by_key(|token| (token.line, token.character));

    let mut data = Vec::with_capacity(tokens.len() * 5);
    let mut previous_line = 0;
    let mut previous_character = 0;
    for token in tokens {
        let delta_line = token.line.saturating_sub(previous_line);
        let delta_character = if delta_line == 0 {
            token.character.saturating_sub(previous_character)
        } else {
            token.character
        };
        data.push(u32::try_from(delta_line).unwrap_or(u32::MAX));
        data.push(u32::try_from(delta_character).unwrap_or(u32::MAX));
        data.push(u32::try_from(token.length).unwrap_or(u32::MAX));
        data.push(token.token_type);
        data.push(token.modifiers);
        previous_line = token.line;
        previous_character = token.character;
    }
    serde_json::json!({ "data": data })
}

pub(crate) fn lsp_node_name_span(
    source: &str,
    node: &orv_project::ProjectNode,
) -> Option<(u32, u32)> {
    let start = usize::try_from(node.span.range.start)
        .ok()?
        .min(source.len());
    let end = usize::try_from(node.span.range.end).ok()?.min(source.len());
    let span_source = source.get(start..end)?;
    let offset = span_source.find(&node.name)?;
    let start = start + offset;
    let end = start + node.name.len();
    Some((u32::try_from(start).ok()?, u32::try_from(end).ok()?))
}

pub(crate) const fn lsp_semantic_token_type(kind: ProjectNodeKind) -> Option<u32> {
    match kind {
        ProjectNodeKind::Domain => Some(0),
        ProjectNodeKind::Struct | ProjectNodeKind::Enum | ProjectNodeKind::TypeAlias => Some(1),
        ProjectNodeKind::Function | ProjectNodeKind::Define => Some(2),
        ProjectNodeKind::File | ProjectNodeKind::Import => None,
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum LspCompletionContext {
    General,
    Directive,
    RouteMethod,
    BodyField,
    ParamField,
    QueryField,
    EnvField,
}

pub(crate) struct LspStaticCompletion {
    pub(crate) label: &'static str,
    pub(crate) kind: u8,
    pub(crate) detail: &'static str,
    pub(crate) insert_text: Option<&'static str>,
}

const LSP_GENERAL_COMPLETIONS: &[LspStaticCompletion] = &[
    LspStaticCompletion {
        label: "import",
        kind: 14,
        detail: "Keyword",
        insert_text: Some("import \"${1:path}\""),
    },
    LspStaticCompletion {
        label: "pub",
        kind: 14,
        detail: "Keyword",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "struct",
        kind: 15,
        detail: "Struct declaration",
        insert_text: Some("struct ${1:Name} {\n  ${2:id}: ${3:int}\n}"),
    },
    LspStaticCompletion {
        label: "enum",
        kind: 15,
        detail: "Enum declaration",
        insert_text: Some("enum ${1:Name} {\n  ${2:Variant}\n}"),
    },
    LspStaticCompletion {
        label: "type",
        kind: 15,
        detail: "Type alias",
        insert_text: Some("type ${1:Name} = ${2:int}"),
    },
    LspStaticCompletion {
        label: "let",
        kind: 15,
        detail: "Binding",
        insert_text: Some("let ${1:name} = ${2:value}"),
    },
    LspStaticCompletion {
        label: "let sig",
        kind: 15,
        detail: "Reactive signal",
        insert_text: Some("let sig ${1:name} = ${2:value}"),
    },
    LspStaticCompletion {
        label: "const",
        kind: 15,
        detail: "Constant",
        insert_text: Some("const ${1:name} = ${2:value}"),
    },
    LspStaticCompletion {
        label: "function",
        kind: 15,
        detail: "Function declaration",
        insert_text: Some("function ${1:name}(${2:input}: ${3:int}): ${4:int} -> {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "async function",
        kind: 15,
        detail: "Async function declaration",
        insert_text: Some("async function ${1:name}(${2:input}: ${3:int}): ${4:int} -> {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "define",
        kind: 15,
        detail: "Token-aware define declaration",
        insert_text: Some("define ${1:name}(${2:input}: ${3:int}): ${4:int} -> {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "domain",
        kind: 15,
        detail: "Domain declaration",
        insert_text: Some("domain ${1:Name} {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "if",
        kind: 15,
        detail: "Conditional",
        insert_text: Some("if ${1:condition} {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "for",
        kind: 15,
        detail: "Loop",
        insert_text: Some("for ${1:item} in ${2:items} {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "while",
        kind: 15,
        detail: "Loop",
        insert_text: Some("while ${1:condition} {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "return",
        kind: 14,
        detail: "Keyword",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "await",
        kind: 14,
        detail: "Keyword",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "test",
        kind: 15,
        detail: "Test block",
        insert_text: Some("test \"${1:name}\" {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "true",
        kind: 14,
        detail: "Boolean literal",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "false",
        kind: 14,
        detail: "Boolean literal",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "null",
        kind: 14,
        detail: "Null literal",
        insert_text: None,
    },
];

const LSP_DIRECTIVE_COMPLETIONS: &[LspStaticCompletion] = &[
    LspStaticCompletion {
        label: "@server",
        kind: 15,
        detail: "Server block",
        insert_text: Some("@server {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "@listen",
        kind: 15,
        detail: "Server listen port",
        insert_text: Some("@listen ${1:8080}"),
    },
    LspStaticCompletion {
        label: "@route",
        kind: 15,
        detail: "HTTP route",
        insert_text: Some("@route ${1:GET} ${2:/path} {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "@respond",
        kind: 15,
        detail: "HTTP response",
        insert_text: Some("@respond ${1:200} ${2:{ ok: true }}"),
    },
    LspStaticCompletion {
        label: "@serve",
        kind: 15,
        detail: "HTML response",
        insert_text: Some("@serve @html {\n  @body {\n    $0\n  }\n}"),
    },
    LspStaticCompletion {
        label: "@db.connect",
        kind: 15,
        detail: "Database adapter",
        insert_text: Some("@db.connect(@env.${1:DATABASE_URL} ?? \"sqlite://data/app.sqlite\")"),
    },
    LspStaticCompletion {
        label: "@payment.connect",
        kind: 15,
        detail: "Payment adapter",
        insert_text: Some(
            "@payment.connect(@env.PAYMENT_ADAPTER_URL ?? \"file://data/payments.jsonl\")",
        ),
    },
    LspStaticCompletion {
        label: "@shipping.connect",
        kind: 15,
        detail: "Shipping adapter",
        insert_text: Some(
            "@shipping.connect(@env.SHIPPING_ADAPTER_URL ?? \"file://data/shipments.jsonl\")",
        ),
    },
    LspStaticCompletion {
        label: "@env",
        kind: 6,
        detail: "Environment value",
        insert_text: Some("@env.${1:NAME}"),
    },
    LspStaticCompletion {
        label: "@body",
        kind: 6,
        detail: "Request body",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "@param",
        kind: 6,
        detail: "Route parameter",
        insert_text: Some("@param.${1:name}"),
    },
    LspStaticCompletion {
        label: "@query",
        kind: 6,
        detail: "Query parameter",
        insert_text: Some("@query.${1:name}"),
    },
    LspStaticCompletion {
        label: "@request.rawBody",
        kind: 6,
        detail: "Raw request body",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "@html",
        kind: 14,
        detail: "HTML domain",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "@body block",
        kind: 15,
        detail: "HTML body",
        insert_text: Some("@body {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "@section",
        kind: 15,
        detail: "HTML section",
        insert_text: Some("@section {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "@form",
        kind: 15,
        detail: "HTML form",
        insert_text: Some("@form action=\"${1:/path}\" method=post {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "@input",
        kind: 15,
        detail: "HTML input",
        insert_text: Some("@input type=${1:text} name=${2:name}"),
    },
    LspStaticCompletion {
        label: "@button",
        kind: 15,
        detail: "HTML button",
        insert_text: Some("@button type=submit \"${1:Submit}\""),
    },
    LspStaticCompletion {
        label: "@a",
        kind: 15,
        detail: "HTML anchor",
        insert_text: Some("@a href=\"${1:/}\" \"${2:Link}\""),
    },
    LspStaticCompletion {
        label: "@h1",
        kind: 15,
        detail: "HTML heading",
        insert_text: Some("@h1 \"${1:Heading}\""),
    },
    LspStaticCompletion {
        label: "@p",
        kind: 15,
        detail: "HTML paragraph",
        insert_text: Some("@p \"${1:Text}\""),
    },
    LspStaticCompletion {
        label: "@ul",
        kind: 15,
        detail: "HTML list",
        insert_text: Some("@ul {\n  $0\n}"),
    },
    LspStaticCompletion {
        label: "@li",
        kind: 15,
        detail: "HTML list item",
        insert_text: Some("@li \"${1:Item}\""),
    },
    LspStaticCompletion {
        label: "@label",
        kind: 15,
        detail: "HTML label",
        insert_text: Some("@label \"${1:Label}\""),
    },
];

const LSP_ROUTE_METHOD_COMPLETIONS: &[LspStaticCompletion] = &[
    LspStaticCompletion {
        label: "GET",
        kind: 14,
        detail: "HTTP method",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "POST",
        kind: 14,
        detail: "HTTP method",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "PUT",
        kind: 14,
        detail: "HTTP method",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "PATCH",
        kind: 14,
        detail: "HTTP method",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "DELETE",
        kind: 14,
        detail: "HTTP method",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "OPTIONS",
        kind: 14,
        detail: "HTTP method",
        insert_text: None,
    },
    LspStaticCompletion {
        label: "HEAD",
        kind: 14,
        detail: "HTTP method",
        insert_text: None,
    },
];

pub(crate) fn lsp_completion_items_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
    context: LspCompletionContext,
) -> Vec<serde_json::Value> {
    let mut items = lsp_context_completion_items_json(files, context);
    if matches!(
        context,
        LspCompletionContext::RouteMethod
            | LspCompletionContext::BodyField
            | LspCompletionContext::ParamField
            | LspCompletionContext::QueryField
            | LspCompletionContext::EnvField
    ) {
        return items;
    }
    for node in &graph.nodes {
        let Some(kind) = lsp_completion_item_kind_code(node.kind) else {
            continue;
        };
        if lsp_completion_item_exists(&items, node.name.as_str(), kind) {
            continue;
        }
        items.push(serde_json::json!({
            "label": node.name.clone(),
            "kind": kind,
            "detail": lsp_symbol_kind(node.kind).unwrap_or("Symbol"),
            "data": {
                "source_node": node.id,
            },
        }));
    }
    items
}

pub(crate) fn lsp_context_completion_items_json(
    files: &[SourceFile],
    context: LspCompletionContext,
) -> Vec<serde_json::Value> {
    match context {
        LspCompletionContext::BodyField => {
            lsp_domain_field_completion_items_json(files, "body", 10, "@body field")
        }
        LspCompletionContext::ParamField => {
            lsp_domain_field_completion_items_json(files, "param", 10, "@param field")
        }
        LspCompletionContext::QueryField => {
            lsp_domain_field_completion_items_json(files, "query", 10, "@query field")
        }
        LspCompletionContext::EnvField => {
            lsp_domain_field_completion_items_json(files, "env", 21, "@env value")
        }
        LspCompletionContext::General
        | LspCompletionContext::Directive
        | LspCompletionContext::RouteMethod => lsp_static_completion_items_json(context),
    }
}

pub(crate) fn lsp_domain_field_completion_items_json(
    files: &[SourceFile],
    domain: &str,
    kind: u8,
    detail: &str,
) -> Vec<serde_json::Value> {
    lsp_domain_field_names(files, domain)
        .into_iter()
        .map(|label| {
            serde_json::json!({
                "label": label,
                "kind": kind,
                "detail": detail,
            })
        })
        .collect()
}

pub(crate) fn lsp_domain_field_names(files: &[SourceFile], domain: &str) -> Vec<String> {
    let Some(kind) = lsp_domain_field_kind_for_domain(domain) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for file in files {
        if domain == "param" {
            names.extend(lsp_route_path_param_names(&file.source));
        }
        names.extend(
            lsp_domain_field_spans(&file.source, kind)
                .into_iter()
                .map(|(_, _, name)| name.to_string()),
        );
    }
    names.sort();
    names.dedup();
    names
}

pub(crate) fn lsp_route_path_param_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut search_from = 0usize;
    while let Some(offset) = source[search_from..].find("@route") {
        let route_start = search_from + offset;
        let route_tail = &source[route_start..];
        let head_end = route_tail
            .find('{')
            .or_else(|| route_tail.find('\n'))
            .unwrap_or(route_tail.len());
        names.extend(lsp_route_head_param_names(&route_tail[..head_end]));
        search_from = route_start + "@route".len();
    }
    names
}

pub(crate) fn lsp_route_head_param_names(route_head: &str) -> Vec<String> {
    let bytes = route_head.as_bytes();
    let mut names = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b':' {
            index += 1;
            continue;
        }
        let name_start = index + 1;
        let Some(first) = bytes.get(name_start) else {
            break;
        };
        if !(first.is_ascii_alphabetic() || *first == b'_') {
            index = name_start;
            continue;
        }
        let mut name_end = name_start + 1;
        while bytes
            .get(name_end)
            .is_some_and(|byte| is_identifier_byte(*byte))
        {
            name_end += 1;
        }
        if let Some(name) = route_head.get(name_start..name_end) {
            names.push(name.to_string());
        }
        index = name_end;
    }
    names
}

pub(crate) fn lsp_static_completion_items_json(
    context: LspCompletionContext,
) -> Vec<serde_json::Value> {
    let specs = match context {
        LspCompletionContext::General => LSP_GENERAL_COMPLETIONS,
        LspCompletionContext::Directive => LSP_DIRECTIVE_COMPLETIONS,
        LspCompletionContext::RouteMethod => LSP_ROUTE_METHOD_COMPLETIONS,
        LspCompletionContext::BodyField
        | LspCompletionContext::ParamField
        | LspCompletionContext::QueryField
        | LspCompletionContext::EnvField => &[],
    };
    let mut items = Vec::new();
    for spec in specs {
        if lsp_completion_item_exists(&items, spec.label, spec.kind) {
            continue;
        }
        let mut item = serde_json::json!({
            "label": spec.label,
            "kind": spec.kind,
            "detail": spec.detail,
        });
        if let Some(insert_text) = spec.insert_text {
            item["insertText"] = serde_json::json!(insert_text);
            item["insertTextFormat"] = serde_json::json!(2);
        }
        items.push(item);
    }
    items
}

pub(crate) fn lsp_completion_item_exists(
    items: &[serde_json::Value],
    label: &str,
    kind: u8,
) -> bool {
    items.iter().any(|item| {
        item.get("label").and_then(serde_json::Value::as_str) == Some(label)
            && item.get("kind").and_then(serde_json::Value::as_u64) == Some(u64::from(kind))
    })
}

pub(crate) fn lsp_completion_context(source: &str, byte: usize) -> LspCompletionContext {
    let prefix = &source[..byte.min(source.len())];
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let line_prefix = &prefix[line_start..];
    let trimmed = line_prefix.trim_start();
    if let Some(context) = lsp_domain_field_completion_context(line_prefix) {
        return context;
    }
    if lsp_is_route_method_completion(trimmed) {
        return LspCompletionContext::RouteMethod;
    }
    if lsp_line_has_open_at_token(line_prefix) {
        return LspCompletionContext::Directive;
    }
    LspCompletionContext::General
}

pub(crate) fn lsp_domain_field_completion_context(
    line_prefix: &str,
) -> Option<LspCompletionContext> {
    let token = line_prefix
        .rsplit(|ch: char| ch.is_whitespace() || matches!(ch, '(' | '{' | '[' | ',' | ':' | '='))
        .next()?;
    if token.starts_with("@body.") {
        return Some(LspCompletionContext::BodyField);
    }
    if token.starts_with("@param.") {
        return Some(LspCompletionContext::ParamField);
    }
    if token.starts_with("@query.") {
        return Some(LspCompletionContext::QueryField);
    }
    if token.starts_with("@env.") {
        return Some(LspCompletionContext::EnvField);
    }
    None
}

pub(crate) fn lsp_is_route_method_completion(trimmed_line_prefix: &str) -> bool {
    let Some(rest) = trimmed_line_prefix.strip_prefix("@route") else {
        return false;
    };
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return false;
    }
    let after_route = rest.trim_start();
    after_route.is_empty() || !after_route.contains(char::is_whitespace)
}

pub(crate) fn lsp_line_has_open_at_token(line_prefix: &str) -> bool {
    line_prefix
        .rsplit(|ch: char| ch.is_whitespace() || matches!(ch, '(' | '{' | '[' | ',' | ':' | '='))
        .next()
        .is_some_and(|token| token.starts_with('@'))
}

pub(crate) fn lsp_workspace_symbols_json(
    graph: &ProjectGraph,
    files: &[SourceFile],
    query: &str,
) -> Vec<serde_json::Value> {
    let normalized_query = query.to_ascii_lowercase();
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            let kind = lsp_symbol_kind_code(node.kind)?;
            if !normalized_query.is_empty()
                && !node
                    .name
                    .to_ascii_lowercase()
                    .contains(normalized_query.as_str())
            {
                return None;
            }
            Some(serde_json::json!({
                "name": node.name,
                "kind": kind,
                "location": lsp_location_json(node, files),
                "data": {
                    "source_node": node.id,
                },
            }))
        })
        .collect()
}

pub(crate) const fn lsp_severity(severity: orv_diagnostics::Severity) -> u8 {
    match severity {
        orv_diagnostics::Severity::Error => 1,
        orv_diagnostics::Severity::Warning => 2,
        orv_diagnostics::Severity::Note => 3,
        orv_diagnostics::Severity::Help => 4,
    }
}

pub(crate) const fn lsp_symbol_kind(kind: ProjectNodeKind) -> Option<&'static str> {
    match kind {
        ProjectNodeKind::Struct => Some("Struct"),
        ProjectNodeKind::Enum => Some("Enum"),
        ProjectNodeKind::TypeAlias => Some("TypeAlias"),
        ProjectNodeKind::Function => Some("Function"),
        ProjectNodeKind::Define => Some("Function"),
        ProjectNodeKind::Domain => Some("Event"),
        ProjectNodeKind::File | ProjectNodeKind::Import => None,
    }
}

pub(crate) const fn lsp_symbol_kind_code(kind: ProjectNodeKind) -> Option<u8> {
    match kind {
        ProjectNodeKind::Struct | ProjectNodeKind::TypeAlias => Some(23),
        ProjectNodeKind::Enum => Some(10),
        ProjectNodeKind::Function | ProjectNodeKind::Define => Some(12),
        ProjectNodeKind::Domain => Some(24),
        ProjectNodeKind::File | ProjectNodeKind::Import => None,
    }
}

pub(crate) const fn lsp_completion_item_kind_code(kind: ProjectNodeKind) -> Option<u8> {
    match kind {
        ProjectNodeKind::Struct | ProjectNodeKind::TypeAlias => Some(22),
        ProjectNodeKind::Enum => Some(13),
        ProjectNodeKind::Function | ProjectNodeKind::Define => Some(3),
        ProjectNodeKind::Domain => Some(23),
        ProjectNodeKind::File | ProjectNodeKind::Import => None,
    }
}

pub(crate) fn lsp_range_json(span: Span, files: &[SourceFile]) -> serde_json::Value {
    let Some(file) = files.iter().find(|file| file.id == span.file) else {
        return serde_json::json!({
            "start": { "line": 0, "character": 0 },
            "end": { "line": 0, "character": 0 },
        });
    };
    let start = byte_position(&file.source, span.range.start);
    let end = byte_position(&file.source, span.range.end);
    lsp_range_from_positions(start, end)
}

pub(crate) fn lsp_range_for_source(source: &str, start: u32, end: u32) -> serde_json::Value {
    lsp_range_from_positions(byte_position(source, start), byte_position(source, end))
}

pub(crate) fn lsp_range_from_positions(
    start: (usize, usize),
    end: (usize, usize),
) -> serde_json::Value {
    serde_json::json!({
        "start": {
            "line": start.0,
            "character": start.1,
        },
        "end": {
            "line": end.0,
            "character": end.1,
        },
    })
}

pub(crate) fn lsp_serve_stdio_stream<R, W>(reader: &mut R, writer: &mut W) -> anyhow::Result<()>
where
    R: std::io::BufRead,
    W: std::io::Write,
{
    let mut session = LspSession::default();
    loop {
        let Some(content_length) = read_lsp_content_length(reader)? else {
            return Ok(());
        };
        let mut body = vec![0_u8; content_length];
        std::io::Read::read_exact(reader, &mut body)?;
        let request: serde_json::Value = serde_json::from_slice(&body)?;
        if let Some(response) = session.message_response(&request) {
            write_lsp_response_frame(writer, &response)?;
            writer.flush()?;
        }
    }
}

pub(crate) fn dap_serve_stdio_stream<R, W>(reader: &mut R, writer: &mut W) -> anyhow::Result<()>
where
    R: std::io::BufRead,
    W: std::io::Write,
{
    let mut session = DapSession::default();
    loop {
        let Some(content_length) = read_lsp_content_length(reader)? else {
            return Ok(());
        };
        let mut body = vec![0_u8; content_length];
        std::io::Read::read_exact(reader, &mut body)?;
        let request: serde_json::Value = serde_json::from_slice(&body)?;
        if let Some(response) = session.message_response(&request) {
            write_lsp_response_frame(writer, &response)?;
            for event in session.drain_pending_events() {
                write_lsp_response_frame(writer, &event)?;
            }
            writer.flush()?;
        }
    }
}

#[cfg(test)]
pub(crate) fn lsp_stdio_response(input: &str) -> anyhow::Result<String> {
    let mut reader = std::io::Cursor::new(input.as_bytes());
    let mut writer = Vec::new();
    lsp_serve_stdio_stream(&mut reader, &mut writer)?;
    String::from_utf8(writer).map_err(|e| anyhow::anyhow!("invalid utf-8 LSP response: {e}"))
}

#[cfg(test)]
pub(crate) fn dap_stdio_response(input: &str) -> anyhow::Result<String> {
    let mut reader = std::io::Cursor::new(input.as_bytes());
    let mut writer = Vec::new();
    dap_serve_stdio_stream(&mut reader, &mut writer)?;
    String::from_utf8(writer).map_err(|e| anyhow::anyhow!("invalid utf-8 DAP response: {e}"))
}

pub(crate) fn dap_protocol_input_frames(requests: &[serde_json::Value]) -> anyhow::Result<String> {
    let mut input = String::new();
    for request in requests {
        let body = serde_json::to_string(request)?;
        write!(&mut input, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    }
    Ok(input)
}

pub(crate) fn dap_protocol_output_frames(output: &str) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut reader = std::io::Cursor::new(output.as_bytes());
    let mut frames = Vec::new();
    loop {
        let Some(content_length) = read_lsp_content_length(&mut reader)? else {
            return Ok(frames);
        };
        let mut body = vec![0_u8; content_length];
        std::io::Read::read_exact(&mut reader, &mut body)?;
        frames.push(serde_json::from_slice(&body)?);
    }
}

pub(crate) fn dap_response_for_request_seq(
    frames: &[serde_json::Value],
    request_seq: u64,
) -> Option<serde_json::Value> {
    frames
        .iter()
        .find(|frame| {
            frame.get("type").and_then(serde_json::Value::as_str) == Some("response")
                && frame.get("request_seq").and_then(serde_json::Value::as_u64) == Some(request_seq)
        })
        .cloned()
}

pub(crate) fn read_lsp_content_length<R: std::io::BufRead>(
    reader: &mut R,
) -> anyhow::Result<Option<usize>> {
    let mut content_length = None;
    let mut saw_header = false;
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            if saw_header {
                anyhow::bail!("incomplete LSP header");
            }
            return Ok(None);
        }
        let header = line.trim_end_matches('\n').trim_end_matches('\r');
        if header.is_empty() {
            break;
        }
        saw_header = true;
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|e| anyhow::anyhow!("invalid Content-Length: {e}"))?,
            );
        }
    }
    content_length
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("missing Content-Length header"))
}

pub(crate) fn write_lsp_response_frame<W: std::io::Write>(
    writer: &mut W,
    response: &serde_json::Value,
) -> anyhow::Result<()> {
    let body = serde_json::to_string(response)?;
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    Ok(())
}
