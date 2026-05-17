#![allow(clippy::redundant_pub_crate, clippy::wildcard_imports)]

use super::*;
pub(crate) fn cmd_verify_build(dir: &Path) -> anyhow::Result<()> {
    verify_build_dir(dir)?;
    println!("build: {} verified", dir.display());
    Ok(())
}

pub(crate) fn cmd_deploy_env_check(dir: &Path) -> anyhow::Result<()> {
    deploy_env_check_with_lookup(dir, |env| std::env::var(env).ok())?;
    println!("deploy env: {} verified", dir.display());
    Ok(())
}

pub(crate) fn cmd_benchmark_report(dir: &Path, require_pass: bool) -> anyhow::Result<()> {
    let report = benchmark_report_value(dir)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if require_pass && report.get("status").and_then(serde_json::Value::as_str) != Some("passed") {
        anyhow::bail!("benchmark report status must be passed");
    }
    Ok(())
}

pub(crate) fn benchmark_report_value(dir: &Path) -> anyhow::Result<serde_json::Value> {
    verify_build_dir(dir)?;
    let deploy = read_json_value(&dir.join("deploy").join("manifest.json"))?;
    let server = deploy
        .get("server")
        .filter(|value| !value.is_null())
        .ok_or_else(|| anyhow::anyhow!("deploy manifest server target is required"))?;
    let evidence_rel = json_str(server, "benchmark_evidence", "deploy server")?;
    let evidence = read_json_value(&dir.join(evidence_rel))?;
    let preflight_rel = json_str(&evidence, "preflight", "benchmark evidence")?;
    let benchmark = evidence
        .get("benchmark")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let max_elapsed_minutes = benchmark
        .get("max_elapsed_minutes")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(300.0);
    let task_report = benchmark_report_tasks(&evidence, max_elapsed_minutes)?;
    let smoke_output_rel = evidence
        .pointer("/artifacts/smoke_output")
        .and_then(serde_json::Value::as_str);
    let smoke_output_contract = evidence
        .get("smoke_output_contract")
        .cloned()
        .or_else(|| smoke_output_rel.map(smoke_output_contract_value))
        .unwrap_or(serde_json::Value::Null);
    let data_report = benchmark_report_data(&evidence, Some(dir), smoke_output_rel)?;
    let status = benchmark_report_status_summary(&task_report, &data_report, max_elapsed_minutes);
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.benchmark.shop_5h.report",
        "build_dir": dir.display().to_string(),
        "status": status.status,
        "contract_verified": true,
        "evidence": evidence_rel,
        "preflight": preflight_rel,
        "preflight_hash": evidence
            .get("preflight_hash")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "smoke_output_contract": smoke_output_contract,
        "recording_status": evidence
            .get("recording_status")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "max_elapsed_minutes": max_elapsed_minutes,
        "total_elapsed_minutes": task_report
            .get("total_elapsed_minutes")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "time_over_limit": status.time_over_limit,
        "tasks": task_report,
        "data": data_report,
        "automated_gate": benchmark
            .get("automated_gate")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "success_criteria": benchmark
            .get("success_criteria")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "limitations": [
            "benchmark-report verifies artifact/evidence shape and summarizes recorded evidence; it does not run the generated smoke test",
            "human-run claims require the recorded evidence file and raw participant notes/output to be retained",
        ],
    }))
}

pub(crate) struct BenchmarkReportStatusSummary {
    pub(crate) status: &'static str,
    pub(crate) failed_task_count: usize,
    pub(crate) missing_task_count: usize,
    pub(crate) missing_data_count: usize,
    pub(crate) total_elapsed_minutes: Option<f64>,
    pub(crate) time_over_limit: bool,
}

pub(crate) fn benchmark_report_status_summary(
    task_report: &serde_json::Value,
    data_report: &serde_json::Value,
    max_elapsed_minutes: f64,
) -> BenchmarkReportStatusSummary {
    let failed_task_count = json_array_count(task_report.get("failed_tasks"));
    let missing_task_count = json_array_count(task_report.get("missing_tasks"));
    let missing_data_count = json_array_count(data_report.get("missing_data"));
    let total_elapsed_minutes = task_report
        .get("total_elapsed_minutes")
        .and_then(serde_json::Value::as_f64);
    let time_over_limit = total_elapsed_minutes.is_some_and(|value| value > max_elapsed_minutes);
    let status = if failed_task_count > 0 || time_over_limit {
        "failed"
    } else if missing_task_count > 0 || missing_data_count > 0 {
        "incomplete"
    } else {
        "passed"
    };
    BenchmarkReportStatusSummary {
        status,
        failed_task_count,
        missing_task_count,
        missing_data_count,
        total_elapsed_minutes,
        time_over_limit,
    }
}

pub(crate) fn benchmark_report_tasks(
    evidence: &serde_json::Value,
    max_elapsed_minutes: f64,
) -> anyhow::Result<serde_json::Value> {
    let entries = evidence
        .get("task_entries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("benchmark evidence task_entries must be an array"))?;
    let mut tasks = Vec::with_capacity(entries.len());
    let mut missing_tasks = Vec::new();
    let mut failed_tasks = Vec::new();
    let mut over_budget_tasks = Vec::new();
    let mut recorded_task_count = 0usize;
    let mut total_elapsed_minutes = 0.0f64;
    let mut all_elapsed_recorded = true;
    for entry in entries {
        let task = json_str(entry, "task", "benchmark task")?;
        let target_minutes = entry
            .get("target_minutes")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("benchmark task target_minutes must be a number"))?;
        let elapsed_minutes = entry
            .get("elapsed_minutes")
            .and_then(serde_json::Value::as_f64);
        let status = entry
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("not_recorded");
        let notes = entry
            .get("notes")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let recorded = elapsed_minutes.is_some() && !benchmark_report_status_is_missing(status);
        if recorded {
            recorded_task_count += 1;
        } else {
            all_elapsed_recorded = false;
            missing_tasks.push(serde_json::json!({
                "task": task,
                "status": status,
                "elapsed_minutes": elapsed_minutes,
            }));
        }
        if let Some(elapsed) = elapsed_minutes {
            total_elapsed_minutes += elapsed;
            if elapsed > target_minutes {
                over_budget_tasks.push(serde_json::json!({
                    "task": task,
                    "target_minutes": target_minutes,
                    "elapsed_minutes": elapsed,
                    "over_by_minutes": elapsed - target_minutes,
                }));
            }
        } else {
            all_elapsed_recorded = false;
        }
        if benchmark_report_status_is_failed(status) {
            failed_tasks.push(serde_json::json!({
                "task": task,
                "status": status,
                "elapsed_minutes": elapsed_minutes,
            }));
        }
        tasks.push(serde_json::json!({
            "task": task,
            "target_minutes": target_minutes,
            "elapsed_minutes": elapsed_minutes,
            "status": status,
            "notes": notes,
            "recorded": recorded,
        }));
    }
    let total = if all_elapsed_recorded {
        serde_json::json!(total_elapsed_minutes)
    } else {
        serde_json::Value::Null
    };
    Ok(serde_json::json!({
        "task_count": entries.len(),
        "recorded_task_count": recorded_task_count,
        "missing_task_count": missing_tasks.len(),
        "failed_task_count": failed_tasks.len(),
        "max_elapsed_minutes": max_elapsed_minutes,
        "total_elapsed_minutes": total,
        "missing_tasks": missing_tasks,
        "failed_tasks": failed_tasks,
        "over_budget_tasks": over_budget_tasks,
        "entries": tasks,
    }))
}

pub(crate) fn benchmark_report_data(
    evidence: &serde_json::Value,
    build_dir: Option<&Path>,
    smoke_output_rel: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let data = evidence
        .get("data")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("benchmark evidence data must be an object"))?;
    let mut missing = Vec::new();
    for key in ["docs_help_lookups", "compiler_runtime_errors"] {
        if data.get(key).is_none_or(serde_json::Value::is_null) {
            missing.push(key.to_string());
        }
    }
    let compiler_errors = data
        .get("compiler_runtime_errors")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            data.get("compiler_runtime_errors")
                .and_then(serde_json::Value::as_i64)
                .and_then(|value| u64::try_from(value).ok())
        });
    if compiler_errors.is_some_and(|count| count > 0)
        && data
            .get("first_error_to_fix_minutes")
            .is_none_or(serde_json::Value::is_null)
    {
        missing.push("first_error_to_fix_minutes".to_string());
    }
    let (smoke_test_output, smoke_test_output_source) =
        benchmark_smoke_test_output_value(data, build_dir, smoke_output_rel);
    if smoke_test_output
        .as_str()
        .is_none_or(|value| value.trim().is_empty())
    {
        missing.push("smoke_test_output".to_string());
    }
    let smoke_test_summary = benchmark_smoke_test_output_summary(&smoke_test_output);
    for marker in smoke_test_summary
        .get("missing_markers")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
    {
        missing.push(format!("smoke_test_output.{marker}"));
    }
    Ok(serde_json::json!({
        "missing_data": missing,
        "docs_help_lookups": data.get("docs_help_lookups").cloned().unwrap_or(serde_json::Value::Null),
        "compiler_runtime_errors": data.get("compiler_runtime_errors").cloned().unwrap_or(serde_json::Value::Null),
        "first_error_to_fix_minutes": data.get("first_error_to_fix_minutes").cloned().unwrap_or(serde_json::Value::Null),
        "manual_config_edits": data.get("manual_config_edits").cloned().unwrap_or_else(|| serde_json::json!([])),
        "smoke_test_required_markers": data
            .get("smoke_test_required_markers")
            .cloned()
            .unwrap_or_else(deploy_benchmark::smoke_required_markers_value),
        "smoke_test_output": smoke_test_output,
        "smoke_test_output_source": smoke_test_output_source,
        "smoke_test_summary": smoke_test_summary,
        "participant_notes": data.get("participant_notes").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

pub(crate) fn benchmark_smoke_test_output_summary(output: &serde_json::Value) -> serde_json::Value {
    let Some(output) = output.as_str().filter(|value| !value.trim().is_empty()) else {
        return serde_json::json!({
            "present": false,
            "passed_marker": false,
            "graph_contract_verified": false,
            "dap_summary_verified": false,
            "dap_source_bundle_verified": false,
            "server_routes": null,
            "trace_stream_requested": null,
            "build_dir": null,
            "base_url": null,
            "client": null,
            "required_markers": deploy_benchmark::smoke_required_markers_value(),
            "missing_markers": [],
        });
    };
    let fields = benchmark_smoke_test_output_fields(output);
    let passed_marker = output
        .lines()
        .any(|line| line.trim() == "orv deploy smoke test passed");
    let graph_contract_verified = fields
        .get("graph_contract")
        .is_some_and(|value| value == "verified");
    let dap_summary_verified = fields
        .get("dap_summary")
        .is_some_and(|value| value == "verified");
    let dap_source_bundle_verified = fields
        .get("dap_source_bundle")
        .is_some_and(|value| value == "verified");
    let server_routes = fields
        .get("server_routes")
        .and_then(|value| value.parse::<u64>().ok());
    let trace_stream_requested = fields
        .get("trace_stream_requested")
        .and_then(|value| benchmark_smoke_test_output_bool(value));
    let build_dir = fields
        .get("build_dir")
        .filter(|value| !value.trim().is_empty())
        .cloned();
    let base_url = fields
        .get("base_url")
        .filter(|value| !value.trim().is_empty())
        .cloned();
    let missing_markers = deploy_benchmark::SMOKE_REQUIRED_MARKERS
        .iter()
        .copied()
        .filter(|marker| match *marker {
            "pass_marker" => !passed_marker,
            "build_dir" => build_dir.is_none(),
            "base_url" => base_url.is_none(),
            "graph_contract" => !graph_contract_verified,
            "dap_summary" => !dap_summary_verified,
            "dap_source_bundle" => !dap_source_bundle_verified,
            "server_routes" => server_routes.is_none_or(|routes| routes == 0),
            "trace_stream_requested" => trace_stream_requested.is_none(),
            marker => fields
                .get(marker)
                .is_none_or(|value| value.trim().is_empty()),
        })
        .collect::<Vec<_>>();
    let client = benchmark_smoke_test_output_client_summary(&fields);
    serde_json::json!({
        "present": true,
        "passed_marker": passed_marker,
        "graph_contract_verified": graph_contract_verified,
        "dap_summary_verified": dap_summary_verified,
        "dap_source_bundle_verified": dap_source_bundle_verified,
        "server_routes": server_routes,
        "trace_stream_requested": trace_stream_requested,
        "build_dir": build_dir,
        "base_url": base_url,
        "client": client,
        "required_markers": deploy_benchmark::smoke_required_markers_value(),
        "missing_markers": missing_markers,
    })
}

pub(crate) fn benchmark_smoke_test_output_fields(output: &str) -> BTreeMap<String, String> {
    output
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .filter(|(key, _)| !key.is_empty())
        .collect()
}

pub(crate) fn benchmark_smoke_test_output_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

pub(crate) fn benchmark_smoke_test_output_client_summary(
    fields: &BTreeMap<String, String>,
) -> serde_json::Value {
    let mut client = serde_json::Map::new();
    for (field, key) in [
        ("manifest", "client_manifest"),
        ("reactive_plan", "client_reactive_plan"),
        ("page", "client_page"),
        ("loader", "client_loader"),
        ("wasm", "client_wasm"),
    ] {
        if let Some(value) = fields.get(key).filter(|value| !value.trim().is_empty()) {
            client.insert(field.to_string(), serde_json::json!(value));
        }
    }
    if client.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(client)
    }
}

pub(crate) fn benchmark_smoke_test_output_value(
    data: &serde_json::Map<String, serde_json::Value>,
    build_dir: Option<&Path>,
    smoke_output_rel: Option<&str>,
) -> (serde_json::Value, serde_json::Value) {
    let evidence_value = data
        .get("smoke_test_output")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if evidence_value
        .as_str()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return (evidence_value, serde_json::json!("evidence"));
    }
    let Some(build_dir) = build_dir else {
        return (evidence_value, serde_json::Value::Null);
    };
    let Some(smoke_output_rel) = smoke_output_rel else {
        return (evidence_value, serde_json::Value::Null);
    };
    let smoke_output_path = build_dir.join(smoke_output_rel);
    match std::fs::read_to_string(&smoke_output_path) {
        Ok(output) if !output.trim().is_empty() => (
            serde_json::json!(output),
            serde_json::json!(smoke_output_rel),
        ),
        _ => (evidence_value, serde_json::Value::Null),
    }
}

pub(crate) fn benchmark_report_status_is_missing(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "" | "not_recorded" | "missing" | "todo" | "incomplete"
    )
}

pub(crate) fn benchmark_report_status_is_failed(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "failed" | "fail" | "blocked"
    )
}

pub(crate) fn verify_build_dir(dir: &Path) -> anyhow::Result<()> {
    let manifest = read_json_value(&dir.join("build-manifest.json"))?;
    let plan = read_json_value(&dir.join("bundle-plan.json"))?;
    let origin_map = read_origin_map(dir)?;
    verify_origin_map_contract(&origin_map)?;
    let source_bundle = read_source_bundle_artifact(&dir.join("source-bundle.json"))?;
    verify_project_graph_contract(dir, &origin_map, &source_bundle)?;
    verify_bundle_targets(dir, &plan, &origin_map, &source_bundle)?;
    verify_manifest_artifacts(dir, &manifest)?;
    verify_deploy_manifest_if_present(dir, &origin_map, &source_bundle)?;
    verify_dev_hmr_session_if_present(dir, &plan)?;
    verify_dev_hmr_transport_if_present(dir)?;
    verify_dev_hmr_server_if_present(dir)?;
    verify_dev_watch_session_if_present(dir, &plan)?;
    verify_dev_watch_events_if_present(dir)
}

pub(crate) fn verify_origin_map_contract(
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<()> {
    if origin_map.version != orv_compiler::ORIGIN_MAP_VERSION {
        anyhow::bail!(
            "origin-map.json version must be {}",
            orv_compiler::ORIGIN_MAP_VERSION
        );
    }
    let mut ids = HashSet::new();
    for entry in &origin_map.entries {
        if entry.id.trim().is_empty() {
            anyhow::bail!("origin-map.json contains entry with empty id");
        }
        if !ids.insert(entry.id.as_str()) {
            let id = &entry.id;
            anyhow::bail!("origin-map.json contains duplicate entry id `{id}`");
        }
        if entry.kind.trim().is_empty() {
            let id = &entry.id;
            anyhow::bail!("origin-map.json entry `{id}` has empty kind");
        }
        if entry.name.trim().is_empty() {
            let id = &entry.id;
            anyhow::bail!("origin-map.json entry `{id}` has empty name");
        }
        if entry.span.start > entry.span.end {
            let id = &entry.id;
            anyhow::bail!("origin-map.json entry `{id}` has invalid span");
        }
    }
    for edge in &origin_map.edges {
        if edge.kind.trim().is_empty() {
            anyhow::bail!("origin-map.json contains edge with empty kind");
        }
        if !ids.contains(edge.from.as_str()) {
            let from = &edge.from;
            anyhow::bail!("origin-map.json edge from `{from}` does not reference an entry");
        }
        if !ids.contains(edge.to.as_str()) {
            let to = &edge.to;
            anyhow::bail!("origin-map.json edge to `{to}` does not reference an entry");
        }
    }
    Ok(())
}

pub(crate) fn verify_project_graph_contract(
    dir: &Path,
    origin_map: &orv_compiler::OriginMap,
    source_bundle: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<()> {
    let graph = read_json_value(&dir.join("project-graph.json"))?;
    if graph
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("project-graph.json schema_version must be 1");
    }
    let nodes = graph
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("project-graph.json nodes must be an array"))?;
    let edges = graph
        .get("edges")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("project-graph.json edges must be an array"))?;
    verify_project_graph_stats(&graph, nodes, edges, origin_map)?;
    let semantic_origin_map = graph
        .pointer("/semantic/origin_map")
        .ok_or_else(|| anyhow::anyhow!("project-graph.json semantic.origin_map is missing"))?;
    if semantic_origin_map != &serde_json::to_value(origin_map)? {
        anyhow::bail!("project-graph.json semantic origin_map does not match origin-map.json");
    }
    let semantic_origin_edges = graph
        .pointer("/semantic/origin_edges")
        .ok_or_else(|| anyhow::anyhow!("project-graph.json semantic.origin_edges is missing"))?;
    if semantic_origin_edges != &serde_json::Value::Array(origin_edges(origin_map)) {
        anyhow::bail!("project-graph.json semantic origin_edges do not match origin-map.json");
    }
    let node_ids = verify_project_graph_nodes(nodes, source_bundle)?;
    verify_project_graph_edges(edges, &node_ids)?;
    verify_project_graph_origin_links(&graph, nodes, origin_map, &node_ids)?;
    Ok(())
}

pub(crate) fn verify_project_graph_stats(
    graph: &serde_json::Value,
    nodes: &[serde_json::Value],
    edges: &[serde_json::Value],
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<()> {
    let stats = graph
        .get("stats")
        .ok_or_else(|| anyhow::anyhow!("project-graph.json stats is missing"))?;
    verify_project_graph_stat(stats, "node_count", nodes.len())?;
    verify_project_graph_stat(stats, "edge_count", edges.len())?;
    verify_project_graph_stat(stats, "semantic_origin_count", origin_map.entries.len())?;
    verify_project_graph_stat(stats, "semantic_edge_count", origin_map.edges.len())?;
    let call_edges = origin_map
        .edges
        .iter()
        .filter(|edge| edge.kind == "calls")
        .count();
    verify_project_graph_stat(stats, "semantic_call_edge_count", call_edges)?;
    Ok(())
}

pub(crate) fn verify_project_graph_stat(
    stats: &serde_json::Value,
    key: &str,
    expected: usize,
) -> anyhow::Result<()> {
    let actual = stats
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("project-graph.json stats.{key} must be an integer"))?;
    if actual != expected as u64 {
        anyhow::bail!("project-graph.json stats.{key} does not match graph content");
    }
    Ok(())
}

pub(crate) fn verify_project_graph_nodes(
    nodes: &[serde_json::Value],
    source_bundle: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<HashSet<u64>> {
    let mut node_ids = HashSet::new();
    let mut file_paths = HashSet::new();
    for node in nodes {
        let id = node
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("project-graph.json node id must be an integer"))?;
        if !node_ids.insert(id) {
            anyhow::bail!("project-graph.json contains duplicate node id {id}");
        }
        let kind = json_str(node, "kind", "project graph node")?;
        let name = json_str(node, "name", "project graph node")?;
        if kind == "file" {
            file_paths.insert(normalized_artifact_path(name));
        }
    }
    for file in &source_bundle.files {
        let path = normalized_artifact_path(&file.path);
        if !file_paths.contains(&path) {
            anyhow::bail!("project-graph.json is missing source-bundle file node {path}");
        }
    }
    if file_paths.len() != source_bundle.files.len() {
        anyhow::bail!("project-graph.json file nodes do not match source-bundle files");
    }
    Ok(node_ids)
}

pub(crate) fn verify_project_graph_edges(
    edges: &[serde_json::Value],
    node_ids: &HashSet<u64>,
) -> anyhow::Result<()> {
    for edge in edges {
        let from = edge
            .get("from")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("project-graph.json edge from must be an integer"))?;
        let to = edge
            .get("to")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("project-graph.json edge to must be an integer"))?;
        if !node_ids.contains(&from) {
            anyhow::bail!("project-graph.json edge from {from} does not reference a node");
        }
        if !node_ids.contains(&to) {
            anyhow::bail!("project-graph.json edge to {to} does not reference a node");
        }
        let _ = json_str(edge, "kind", "project graph edge")?;
    }
    Ok(())
}

pub(crate) fn verify_project_graph_origin_links(
    graph: &serde_json::Value,
    nodes: &[serde_json::Value],
    origin_map: &orv_compiler::OriginMap,
    node_ids: &HashSet<u64>,
) -> anyhow::Result<()> {
    let origin_links = graph
        .pointer("/semantic/origin_links")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            anyhow::anyhow!("project-graph.json semantic.origin_links must be an array")
        })?;
    let expected = expected_project_graph_origin_links(nodes, origin_map)?;
    if origin_links != &expected {
        anyhow::bail!(
            "project-graph.json semantic origin_links do not match graph nodes and origin-map.json"
        );
    }
    let origin_ids = origin_map
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    for link in origin_links {
        let origin_id = json_str(link, "origin_id", "project graph origin link")?;
        if !origin_ids.contains(origin_id) {
            anyhow::bail!(
                "project-graph.json origin link `{origin_id}` does not reference origin-map.json"
            );
        }
        let node_id = link
            .get("node_id")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                anyhow::anyhow!("project-graph.json origin link node_id must be an integer")
            })?;
        if !node_ids.contains(&node_id) {
            anyhow::bail!(
                "project-graph.json origin link node_id {node_id} does not reference a node"
            );
        }
    }
    Ok(())
}

pub(crate) fn expected_project_graph_origin_links(
    nodes: &[serde_json::Value],
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut links = Vec::new();
    for entry in &origin_map.entries {
        if let Some(node) = nodes
            .iter()
            .find(|node| project_graph_node_matches_origin(node, entry))
        {
            let node_id = node
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| anyhow::anyhow!("project-graph.json node id must be an integer"))?;
            links.push(serde_json::json!({
                "kind": "source_node",
                "origin_id": entry.id,
                "node_id": node_id,
            }));
        }
    }
    Ok(links)
}

pub(crate) fn project_graph_node_matches_origin(
    node: &serde_json::Value,
    entry: &orv_compiler::OriginEntry,
) -> bool {
    node.get("file").and_then(serde_json::Value::as_u64) == Some(u64::from(entry.span.file))
        && node
            .pointer("/span/start")
            .and_then(serde_json::Value::as_u64)
            == Some(u64::from(entry.span.start))
        && node
            .pointer("/span/end")
            .and_then(serde_json::Value::as_u64)
            == Some(u64::from(entry.span.end))
}

pub(crate) fn verify_manifest_artifacts(
    dir: &Path,
    manifest: &serde_json::Value,
) -> anyhow::Result<()> {
    let artifacts = manifest
        .get("artifacts")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("build manifest artifacts must be an array"))?;
    for artifact in artifacts {
        let kind = json_str(artifact, "kind", "build manifest artifact")?;
        let path = json_str(artifact, "path", "build manifest artifact")?;
        let artifact_path = dir.join(path);
        if !artifact_path.is_file() {
            anyhow::bail!(
                "missing manifest artifact {kind}: {}",
                artifact_path.display()
            );
        }
        if kind == "source_bundle" {
            let source_bundle = read_source_bundle_artifact(&artifact_path)?;
            orv_compiler::verify_source_bundle_artifact(&source_bundle)
                .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
        }
    }
    Ok(())
}

pub(crate) fn verify_bundle_targets(
    dir: &Path,
    plan: &serde_json::Value,
    origin_map: &orv_compiler::OriginMap,
    source_bundle: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<()> {
    let bundles = plan
        .get("bundles")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("bundle plan bundles must be an array"))?;
    for bundle in bundles {
        let kind = json_str(bundle, "kind", "bundle target")?;
        let path = json_str(bundle, "path", "bundle target")?;
        let target = dir.join(path);
        if !target.is_file() {
            anyhow::bail!("missing bundle target {kind}: {}", target.display());
        }
        match kind {
            "server_runtime" => {
                let artifact = read_server_artifact(&target)?;
                orv_compiler::verify_server_runtime_artifact(&artifact)
                    .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
                verify_server_runtime_origin_contract(&artifact, origin_map)?;
                verify_server_runtime_source_bundle_contract(&artifact, source_bundle)?;
            }
            "server_launcher" => verify_server_launcher_target(dir, &target)?,
            "native_server_plan" => verify_native_server_plan_target(dir, &target)?,
            "native_runtime_image_plan" => verify_native_runtime_image_plan_target(dir, &target)?,
            "native_runtime_image_dockerfile" => verify_native_runtime_image_dockerfile(&target)?,
            "native_server_launcher_source" => {
                let artifact = read_server_artifact(&dir.join(SERVER_ARTIFACT_PATH))?;
                verify_native_server_launcher_source(
                    &target,
                    SERVER_ARTIFACT_PATH,
                    NATIVE_SERVER_PLAN_PATH,
                    &artifact,
                )?;
            }
            "native_server_routes_source" => {
                let artifact = read_server_artifact(&dir.join(SERVER_ARTIFACT_PATH))?;
                verify_native_server_routes_source(&target, &artifact)?;
            }
            "native_server_router_source" => {
                verify_native_server_router_source(&target)?;
            }
            "native_server_handlers_source" => {
                let artifact = read_server_artifact(&dir.join(SERVER_ARTIFACT_PATH))?;
                verify_native_server_handlers_source(&target, &artifact)?;
            }
            "native_server_launcher_package" => verify_native_server_launcher_package(&target)?,
            "static_page" => verify_static_page_target(bundle, &target)?,
            "client_manifest" => verify_client_manifest_target(dir, bundle, &target)?,
            "client_reactive_plan" => verify_client_reactive_plan_target(dir, bundle, &target)?,
            "client_page" => verify_client_page_target(bundle, &target)?,
            "client_js" => verify_client_js_target(&target)?,
            "client_wasm" => verify_client_wasm_target(dir, &target)?,
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn verify_server_runtime_origin_contract(
    artifact: &orv_compiler::ServerRuntimeArtifact,
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<()> {
    let entries_by_id: HashMap<&str, &orv_compiler::OriginEntry> = origin_map
        .entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let contains_edges: HashSet<(&str, &str)> = origin_map
        .edges
        .iter()
        .filter(|edge| edge.kind == "contains")
        .map(|edge| (edge.from.as_str(), edge.to.as_str()))
        .collect();
    if let Some(listen) = &artifact.listen {
        let Some(entry) = entries_by_id.get(listen.origin_id.as_str()).copied() else {
            let origin_id = &listen.origin_id;
            anyhow::bail!("server listen origin_id `{origin_id}` not found in origin-map.json");
        };
        if entry.kind != "listen" {
            let origin_id = &listen.origin_id;
            anyhow::bail!("server listen origin_id `{origin_id}` must reference origin-map listen");
        }
        if entry.name != listen.name {
            let origin_id = &listen.origin_id;
            anyhow::bail!("server listen origin_id `{origin_id}` name does not match origin-map");
        }
    }
    let mut route_ids = HashSet::new();
    for route in &artifact.routes {
        if !route_ids.insert(route.origin_id.as_str()) {
            let origin_id = &route.origin_id;
            anyhow::bail!("server route origin_id `{origin_id}` is duplicated");
        }
        let Some(entry) = entries_by_id.get(route.origin_id.as_str()).copied() else {
            let origin_id = &route.origin_id;
            anyhow::bail!(
                "server route {} {} origin_id `{origin_id}` not found in origin-map.json",
                route.method,
                route.path
            );
        };
        if entry.kind != "route" {
            let origin_id = &route.origin_id;
            anyhow::bail!(
                "server route {} {} origin_id `{origin_id}` must reference origin-map route",
                route.method,
                route.path
            );
        }
        let expected_name = format!("{} {}", route.method, route.path);
        if entry.name != expected_name {
            let origin_id = &route.origin_id;
            anyhow::bail!(
                "server route {} {} origin_id `{origin_id}` name does not match origin-map",
                route.method,
                route.path
            );
        }
        let expected_response_origin_ids =
            origin_response_ids_for_route(origin_map, &route.origin_id);
        if route.response_origin_ids != expected_response_origin_ids {
            anyhow::bail!(
                "server route {} {} response_origin_ids do not match origin-map contains edges",
                route.method,
                route.path
            );
        }
        let mut response_ids = HashSet::new();
        for response_origin_id in &route.response_origin_ids {
            if !response_ids.insert(response_origin_id.as_str()) {
                anyhow::bail!(
                    "server route {} {} response_origin_id `{response_origin_id}` is duplicated",
                    route.method,
                    route.path
                );
            }
            verify_route_response_origin(
                route,
                response_origin_id,
                &entries_by_id,
                &contains_edges,
            )?;
        }
        for response in &route.responses {
            if !route
                .response_origin_ids
                .iter()
                .any(|origin_id| origin_id == &response.origin_id)
            {
                let origin_id = &response.origin_id;
                anyhow::bail!(
                    "server route {} {} response descriptor `{origin_id}` is missing from response_origin_ids",
                    route.method,
                    route.path
                );
            }
            verify_route_response_origin(
                route,
                &response.origin_id,
                &entries_by_id,
                &contains_edges,
            )?;
        }
        for policy in &route.policies {
            verify_route_policy_origin(route, policy, &entries_by_id, &contains_edges)?;
        }
    }
    Ok(())
}

pub(crate) fn origin_response_ids_for_route(
    origin_map: &orv_compiler::OriginMap,
    route_origin_id: &str,
) -> Vec<String> {
    origin_map
        .edges
        .iter()
        .filter(|edge| edge.from == route_origin_id && edge.kind == "contains")
        .filter_map(|edge| {
            origin_map
                .entries
                .iter()
                .find(|entry| {
                    entry.id == edge.to && entry.kind == "domain" && entry.name == "respond"
                })
                .map(|entry| entry.id.clone())
        })
        .collect()
}

pub(crate) fn verify_route_response_origin(
    route: &orv_compiler::ServerRouteArtifact,
    response_origin_id: &str,
    entries_by_id: &HashMap<&str, &orv_compiler::OriginEntry>,
    contains_edges: &HashSet<(&str, &str)>,
) -> anyhow::Result<()> {
    let Some(entry) = entries_by_id.get(response_origin_id).copied() else {
        anyhow::bail!(
            "server route {} {} response_origin_id `{response_origin_id}` not found in origin-map.json",
            route.method,
            route.path
        );
    };
    if entry.kind != "domain" || entry.name != "respond" {
        anyhow::bail!(
            "server route {} {} response_origin_id `{response_origin_id}` must reference origin-map respond domain",
            route.method,
            route.path
        );
    }
    if !contains_edges.contains(&(route.origin_id.as_str(), response_origin_id)) {
        anyhow::bail!(
            "server route {} {} response_origin_id `{response_origin_id}` is not contained by route origin",
            route.method,
            route.path
        );
    }
    Ok(())
}

pub(crate) fn verify_route_policy_origin(
    route: &orv_compiler::ServerRouteArtifact,
    policy: &orv_compiler::ServerRoutePolicyArtifact,
    entries_by_id: &HashMap<&str, &orv_compiler::OriginEntry>,
    contains_edges: &HashSet<(&str, &str)>,
) -> anyhow::Result<()> {
    let Some(policy_origin_id) = policy.origin_id.as_deref() else {
        return Ok(());
    };
    let Some(entry) = entries_by_id.get(policy_origin_id).copied() else {
        anyhow::bail!(
            "server route {} {} policy `{}` origin_id `{policy_origin_id}` not found in origin-map.json",
            route.method,
            route.path,
            policy.kind
        );
    };
    let expected_domain = match policy.kind.as_str() {
        "auth" => "Auth",
        "csrf" => "csrf",
        "session" => "session",
        _ => return Ok(()),
    };
    if entry.kind != "domain" || entry.name != expected_domain {
        anyhow::bail!(
            "server route {} {} policy `{}` origin_id `{policy_origin_id}` must reference origin-map {expected_domain} domain",
            route.method,
            route.path,
            policy.kind
        );
    }
    if !contains_edges.contains(&(route.origin_id.as_str(), policy_origin_id)) {
        anyhow::bail!(
            "server route {} {} policy `{}` origin_id `{policy_origin_id}` is not contained by route origin",
            route.method,
            route.path,
            policy.kind
        );
    }
    Ok(())
}

pub(crate) fn verify_server_runtime_source_bundle_contract(
    artifact: &orv_compiler::ServerRuntimeArtifact,
    source_bundle: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<()> {
    if artifact.entry != source_bundle.entry {
        anyhow::bail!("server runtime entry does not match source-bundle artifact");
    }
    if artifact.source_bundle.files.len() != source_bundle.files.len() {
        anyhow::bail!("server runtime source bundle does not match build source-bundle artifact");
    }
    for expected in &source_bundle.files {
        let Some(actual) = artifact
            .source_bundle
            .files
            .iter()
            .find(|file| file.path == expected.path)
        else {
            let path = &expected.path;
            anyhow::bail!("server runtime source bundle is missing source file {path}");
        };
        if actual.content_hash != expected.content_hash || actual.source != expected.source {
            let path = &expected.path;
            anyhow::bail!(
                "server runtime source file {path} does not match build source-bundle artifact"
            );
        }
    }
    Ok(())
}

pub(crate) fn verify_server_launcher_target(dir: &Path, target: &Path) -> anyhow::Result<()> {
    let launch = read_server_launch_artifact(target)?;
    if launch.protocol != "http1" {
        anyhow::bail!("server launcher protocol must be http1");
    }
    let expected = vec![
        "orv".to_string(),
        "run-artifact".to_string(),
        launch.artifact.clone(),
    ];
    if launch.command != expected {
        anyhow::bail!("server launcher command must be `orv run-artifact <artifact>`");
    }
    let artifact = read_server_artifact(&dir.join(&launch.artifact))?;
    orv_compiler::verify_server_runtime_artifact(&artifact)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    if launch.runtime != artifact.runtime {
        anyhow::bail!("server launcher runtime does not match runtime artifact");
    }
    if launch.routes != artifact.routes {
        anyhow::bail!("server launcher routes do not match runtime artifact");
    }
    if launch.listen != artifact.listen {
        anyhow::bail!("server launcher listen does not match runtime artifact");
    }
    Ok(())
}

pub(crate) fn verify_native_server_plan_target(dir: &Path, target: &Path) -> anyhow::Result<()> {
    let plan = read_json_value(target)?;
    let artifact = read_server_artifact(&dir.join(SERVER_ARTIFACT_PATH))?;
    verify_native_server_plan_value(
        dir,
        &plan,
        SERVER_ARTIFACT_PATH,
        SERVER_LAUNCH_PATH,
        &artifact,
    )
}

pub(crate) fn verify_native_runtime_image_plan_target(
    dir: &Path,
    target: &Path,
) -> anyhow::Result<()> {
    let plan = read_json_value(target)?;
    let artifact = read_server_artifact(&dir.join(SERVER_ARTIFACT_PATH))?;
    verify_native_runtime_image_plan_value(
        &plan,
        SERVER_ARTIFACT_PATH,
        NATIVE_SERVER_PLAN_PATH,
        &artifact,
    )
}

pub(crate) fn verify_native_server_plan_artifact(
    dir: &Path,
    path: &str,
    artifact_path: &str,
    launcher_path: &str,
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let native_plan_path = dir.join(path);
    if !native_plan_path.is_file() {
        anyhow::bail!(
            "missing native server plan artifact: {}",
            native_plan_path.display()
        );
    }
    let plan = read_json_value(&native_plan_path)?;
    verify_native_server_plan_value(dir, &plan, artifact_path, launcher_path, artifact)
}

pub(crate) fn verify_native_runtime_image_plan_artifact(
    dir: &Path,
    path: &str,
    artifact_path: &str,
    native_plan_path: &str,
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let image_plan_path = dir.join(path);
    if !image_plan_path.is_file() {
        anyhow::bail!(
            "missing native runtime image plan artifact: {}",
            image_plan_path.display()
        );
    }
    let plan = read_json_value(&image_plan_path)?;
    verify_native_runtime_image_plan_value(&plan, artifact_path, native_plan_path, artifact)
}

#[allow(clippy::too_many_lines)]
pub(crate) fn verify_native_server_plan_value(
    dir: &Path,
    plan: &serde_json::Value,
    artifact_path: &str,
    launcher_path: &str,
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    if plan
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(orv_compiler::NATIVE_SERVER_PLAN_ARTIFACT_VERSION))
    {
        anyhow::bail!(
            "native server plan schema_version must be {}",
            orv_compiler::NATIVE_SERVER_PLAN_ARTIFACT_VERSION
        );
    }
    if json_str(plan, "kind", "native server plan")? != "native_server_plan" {
        anyhow::bail!("native server plan kind must be native_server_plan");
    }
    let direct_http = orv_compiler::native_server_direct_http_capable(artifact);
    let expected_status = native_server_plan_status(direct_http);
    if json_str(plan, "status", "native server plan")? != expected_status {
        anyhow::bail!("native server plan status must be {expected_status}");
    }
    if json_str(plan, "artifact", "native server plan")? != artifact_path {
        anyhow::bail!("native server plan artifact must be {artifact_path}");
    }
    if json_str(plan, "launcher", "native server plan")? != launcher_path {
        anyhow::bail!("native server plan launcher must be {launcher_path}");
    }
    let source_path = json_str(plan, "source", "native server plan")?;
    verify_native_server_launcher_source(
        &dir.join(source_path),
        artifact_path,
        NATIVE_SERVER_PLAN_PATH,
        artifact,
    )?;
    verify_native_server_plan_routes_source(dir, plan, artifact)?;
    verify_native_server_plan_router_source(dir, plan)?;
    verify_native_server_plan_handlers_source(dir, plan, artifact)?;
    let package_path = json_str(plan, "package", "native server plan")?;
    verify_native_server_launcher_package(&dir.join(package_path))?;
    verify_native_server_plan_runtime_image(plan)?;
    let expected_build = serde_json::json!([
        "cargo",
        "build",
        "--manifest-path",
        package_path,
        "--release"
    ]);
    if plan.pointer("/commands/build") != Some(&expected_build) {
        anyhow::bail!("native server plan build command must match generated launcher package");
    }
    let expected_run_env = serde_json::json!({ "ORV_BUILD_DIR": "." });
    if plan.pointer("/commands/run/env") != Some(&expected_run_env) {
        anyhow::bail!("native server plan run env must set ORV_BUILD_DIR to build directory");
    }
    let expected_run_command = serde_json::json!([NATIVE_SERVER_LAUNCHER_BINARY_PATH]);
    if plan.pointer("/commands/run/command") != Some(&expected_run_command) {
        anyhow::bail!("native server plan run command must match generated launcher binary");
    }
    let launch = read_server_launch_artifact(&dir.join(launcher_path))?;
    if launch.artifact != artifact_path {
        anyhow::bail!("native server plan launcher artifact does not match server artifact");
    }
    if json_str(plan, "runtime", "native server plan")? != artifact.runtime {
        anyhow::bail!("native server plan runtime does not match runtime artifact");
    }
    if plan.get("runtime_features") != Some(&serde_json::to_value(&artifact.runtime_features)?) {
        anyhow::bail!("native server plan runtime_features do not match runtime artifact");
    }
    let target = plan
        .get("target")
        .ok_or_else(|| anyhow::anyhow!("native server plan target must be an object"))?;
    if json_str(target, "kind", "native server plan target")? != "server_binary" {
        anyhow::bail!("native server plan target kind must be server_binary");
    }
    if json_str(target, "path", "native server plan target")? != NATIVE_SERVER_BINARY_PATH {
        anyhow::bail!("native server plan target path must be {NATIVE_SERVER_BINARY_PATH}");
    }
    if json_str(target, "protocol", "native server plan target")? != "http1" {
        anyhow::bail!("native server plan target protocol must be http1");
    }
    let blocked_by = plan
        .get("blocked_by")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("native server plan blocked_by must be an array"))?;
    if direct_http
        && blocked_by
            .iter()
            .any(|item| item.as_str() == Some("native-codegen"))
    {
        anyhow::bail!("native server plan direct_http must not be blocked by native-codegen");
    }
    if !direct_http
        && !blocked_by
            .iter()
            .any(|item| item.as_str() == Some("native-codegen"))
    {
        anyhow::bail!("native server plan blocked_by must include native-codegen");
    }
    if direct_http
        && blocked_by
            .iter()
            .any(|item| item.as_str() == Some("native-runtime-image"))
    {
        anyhow::bail!("native server plan direct_http must not be blocked by native-runtime-image");
    }
    if !direct_http
        && !blocked_by
            .iter()
            .any(|item| item.as_str() == Some("native-runtime-image"))
    {
        anyhow::bail!("native server plan blocked_by must include native-runtime-image");
    }
    verify_deploy_listen_value(
        plan.get("listen"),
        artifact.listen.as_ref(),
        "native server plan",
    )?;
    let artifact_routes = serde_json::to_value(&artifact.routes)?;
    if plan.get("routes") != Some(&artifact_routes) {
        anyhow::bail!("native server plan routes do not match runtime artifact");
    }
    Ok(())
}

pub(crate) fn verify_native_server_plan_routes_source(
    dir: &Path,
    plan: &serde_json::Value,
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let routes_source_path = json_str(plan, "routes_source", "native server plan")?;
    if routes_source_path != NATIVE_SERVER_ROUTES_SOURCE_PATH {
        anyhow::bail!(
            "native server plan routes_source must be {NATIVE_SERVER_ROUTES_SOURCE_PATH}"
        );
    }
    verify_native_server_routes_source(&dir.join(routes_source_path), artifact)
}

pub(crate) fn verify_native_server_plan_router_source(
    dir: &Path,
    plan: &serde_json::Value,
) -> anyhow::Result<()> {
    let router_source_path = json_str(plan, "router_source", "native server plan")?;
    if router_source_path != NATIVE_SERVER_ROUTER_SOURCE_PATH {
        anyhow::bail!(
            "native server plan router_source must be {NATIVE_SERVER_ROUTER_SOURCE_PATH}"
        );
    }
    verify_native_server_router_source(&dir.join(router_source_path))
}

pub(crate) fn verify_native_server_plan_handlers_source(
    dir: &Path,
    plan: &serde_json::Value,
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let handlers_source_path = json_str(plan, "handlers_source", "native server plan")?;
    if handlers_source_path != NATIVE_SERVER_HANDLERS_SOURCE_PATH {
        anyhow::bail!(
            "native server plan handlers_source must be {NATIVE_SERVER_HANDLERS_SOURCE_PATH}"
        );
    }
    verify_native_server_handlers_source(&dir.join(handlers_source_path), artifact)
}

pub(crate) fn verify_native_server_plan_runtime_image(
    plan: &serde_json::Value,
) -> anyhow::Result<()> {
    if json_str(plan, "runtime_image_plan", "native server plan")? != NATIVE_RUNTIME_IMAGE_PLAN_PATH
    {
        anyhow::bail!(
            "native server plan runtime_image_plan must be {NATIVE_RUNTIME_IMAGE_PLAN_PATH}"
        );
    }
    Ok(())
}

pub(crate) fn verify_native_runtime_image_plan_value(
    plan: &serde_json::Value,
    artifact_path: &str,
    native_plan_path: &str,
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    if plan
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(
            orv_compiler::NATIVE_RUNTIME_IMAGE_PLAN_ARTIFACT_VERSION,
        ))
    {
        anyhow::bail!(
            "native runtime image plan schema_version must be {}",
            orv_compiler::NATIVE_RUNTIME_IMAGE_PLAN_ARTIFACT_VERSION
        );
    }
    if json_str(plan, "kind", "native runtime image plan")? != "native_runtime_image_plan" {
        anyhow::bail!("native runtime image plan kind must be native_runtime_image_plan");
    }
    let direct_http = orv_compiler::native_server_direct_http_capable(artifact);
    let expected_status = native_runtime_image_plan_status(direct_http);
    if json_str(plan, "status", "native runtime image plan")? != expected_status {
        anyhow::bail!("native runtime image plan status must be {expected_status}");
    }
    if json_str(plan, "artifact", "native runtime image plan")? != artifact_path {
        anyhow::bail!("native runtime image plan artifact must be {artifact_path}");
    }
    if json_str(plan, "native_plan", "native runtime image plan")? != native_plan_path {
        anyhow::bail!("native runtime image plan native_plan must be {native_plan_path}");
    }
    if json_str(plan, "runtime", "native runtime image plan")? != artifact.runtime {
        anyhow::bail!("native runtime image plan runtime does not match runtime artifact");
    }
    if plan.get("runtime_features") != Some(&serde_json::to_value(&artifact.runtime_features)?) {
        anyhow::bail!("native runtime image plan runtime_features do not match runtime artifact");
    }
    if json_str(plan, "reference_image", "native runtime image plan")?
        != ORV_REFERENCE_RUNTIME_IMAGE
    {
        anyhow::bail!(
            "native runtime image plan reference_image must be {ORV_REFERENCE_RUNTIME_IMAGE}"
        );
    }
    let target = plan
        .get("target")
        .ok_or_else(|| anyhow::anyhow!("native runtime image plan target must be an object"))?;
    if json_str(target, "kind", "native runtime image plan target")? != "oci_image" {
        anyhow::bail!("native runtime image plan target kind must be oci_image");
    }
    if json_str(target, "image", "native runtime image plan target")? != NATIVE_RUNTIME_IMAGE_NAME {
        anyhow::bail!("native runtime image plan target image must be {NATIVE_RUNTIME_IMAGE_NAME}");
    }
    if json_str(target, "binary", "native runtime image plan target")? != NATIVE_SERVER_BINARY_PATH
    {
        anyhow::bail!(
            "native runtime image plan target binary must be {NATIVE_SERVER_BINARY_PATH}"
        );
    }
    if json_str(target, "protocol", "native runtime image plan target")? != "http1" {
        anyhow::bail!("native runtime image plan target protocol must be http1");
    }
    let blocked_by = plan
        .get("blocked_by")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("native runtime image plan blocked_by must be an array"))?;
    if direct_http
        && blocked_by
            .iter()
            .any(|item| item.as_str() == Some("native-codegen"))
    {
        anyhow::bail!(
            "native runtime image plan direct_http must not be blocked by native-codegen"
        );
    }
    if !direct_http
        && !blocked_by
            .iter()
            .any(|item| item.as_str() == Some("native-codegen"))
    {
        anyhow::bail!("native runtime image plan blocked_by must include native-codegen");
    }
    if direct_http
        && blocked_by
            .iter()
            .any(|item| item.as_str() == Some("native-runtime-image"))
    {
        anyhow::bail!(
            "native runtime image plan direct_http must not be blocked by native-runtime-image"
        );
    }
    if !direct_http
        && !blocked_by
            .iter()
            .any(|item| item.as_str() == Some("native-runtime-image"))
    {
        anyhow::bail!("native runtime image plan blocked_by must include native-runtime-image");
    }
    if json_str(plan, "dockerfile", "native runtime image plan")?
        != NATIVE_RUNTIME_IMAGE_DOCKERFILE_PATH
    {
        anyhow::bail!(
            "native runtime image plan dockerfile must be {NATIVE_RUNTIME_IMAGE_DOCKERFILE_PATH}"
        );
    }
    if plan.pointer("/commands/build")
        != Some(&serde_json::json!([
            "docker",
            "build",
            "-f",
            NATIVE_RUNTIME_IMAGE_DOCKERFILE_PATH,
            "-t",
            NATIVE_RUNTIME_IMAGE_NAME,
            "."
        ]))
    {
        anyhow::bail!("native runtime image plan build command must match generated Dockerfile");
    }
    verify_deploy_listen_value(
        plan.get("listen"),
        artifact.listen.as_ref(),
        "native runtime image plan",
    )?;
    let artifact_routes = serde_json::to_value(&artifact.routes)?;
    if plan.get("routes") != Some(&artifact_routes) {
        anyhow::bail!("native runtime image plan routes do not match runtime artifact");
    }
    Ok(())
}

pub(crate) fn verify_native_runtime_image_dockerfile(target: &Path) -> anyhow::Result<()> {
    if !target.is_file() {
        anyhow::bail!(
            "missing native runtime image Dockerfile: {}",
            target.display()
        );
    }
    let source = std::fs::read_to_string(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    for expected in [
        "FROM rust:",
        "cargo build --manifest-path /work/server/native/Cargo.toml --release",
        "COPY . /app",
        "COPY --from=build /work/server/native/target/release/orv-native-server /app/server/app",
        "ENV ORV_BUILD_DIR=/app",
        "ENTRYPOINT [\"/app/server/app\"]",
    ] {
        if !source.contains(expected) {
            anyhow::bail!("native runtime image Dockerfile must contain {expected}");
        }
    }
    Ok(())
}

pub(crate) fn verify_native_server_launcher_package(target: &Path) -> anyhow::Result<()> {
    if !target.is_file() {
        anyhow::bail!(
            "missing native server launcher package: {}",
            target.display()
        );
    }
    let source = std::fs::read_to_string(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    let manifest = toml::from_str::<toml::Value>(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", target.display()))?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| anyhow::anyhow!("native server launcher package must have [package]"))?;
    if package.get("name").and_then(toml::Value::as_str) != Some("orv-native-server") {
        anyhow::bail!("native server launcher package name must be orv-native-server");
    }
    if package.get("edition").and_then(toml::Value::as_str) != Some("2021") {
        anyhow::bail!("native server launcher package edition must be 2021");
    }
    if package.get("publish").and_then(toml::Value::as_bool) != Some(false) {
        anyhow::bail!("native server launcher package publish must be false");
    }
    if manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .is_some_and(|dependencies| !dependencies.is_empty())
    {
        anyhow::bail!("native server launcher package must not declare dependencies");
    }
    let bins = manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("native server launcher package must declare [[bin]]"))?;
    let bin = bins
        .iter()
        .find_map(toml::Value::as_table)
        .ok_or_else(|| anyhow::anyhow!("native server launcher package bin must be a table"))?;
    if bin.get("name").and_then(toml::Value::as_str) != Some("orv-native-server") {
        anyhow::bail!("native server launcher package bin name must be orv-native-server");
    }
    if bin.get("path").and_then(toml::Value::as_str) != Some("main.rs") {
        anyhow::bail!("native server launcher package bin path must be main.rs");
    }
    Ok(())
}

pub(crate) fn verify_native_server_launcher_source(
    target: &Path,
    artifact_path: &str,
    native_plan_path: &str,
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    if !target.is_file() {
        anyhow::bail!(
            "missing native server launcher source: {}",
            target.display()
        );
    }
    let source = std::fs::read_to_string(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    let artifact_line = format!(r#"const ORV_SERVER_ARTIFACT: &str = "{artifact_path}";"#);
    if !source.contains(&artifact_line) {
        anyhow::bail!("native server launcher source must reference {artifact_path}");
    }
    let plan_line = format!(r#"const ORV_NATIVE_SERVER_PLAN: &str = "{native_plan_path}";"#);
    if !source.contains(&plan_line) {
        anyhow::bail!("native server launcher source must reference {native_plan_path}");
    }
    if !source.contains("build_dir.join(ORV_NATIVE_SERVER_PLAN)")
        || !source.contains("native_plan.is_file()")
    {
        anyhow::bail!("native server launcher source must validate native server plan");
    }
    if !source.contains("fn orv_build_dir() -> std::path::PathBuf")
        || !source.contains(r#"std::env::var_os("ORV_BUILD_DIR")"#)
        || !source.contains("std::env::current_exe()")
        || !source.contains("path.parent()?.parent()?.parent()?.parent()?.parent()")
    {
        anyhow::bail!("native server launcher source must infer build dir from executable path");
    }
    if !source.contains("build_dir.join(ORV_SERVER_ARTIFACT)")
        || !source.contains("artifact.is_file()")
    {
        anyhow::bail!("native server launcher source must validate server artifact");
    }
    let expected =
        orv_compiler::native_server_launcher_source(artifact_path, native_plan_path, artifact);
    if expected.contains("fn orv_native_serve() -> std::io::Result<()>") {
        if !source.contains("fn orv_native_serve() -> std::io::Result<()>")
            || !source.contains("std::net::TcpListener::bind(orv_native_listen_address())")
        {
            anyhow::bail!("native server launcher source must serve HTTP directly");
        }
        if !source.contains("router::orv_native_dispatch_with_request(")
            || !source.contains("request.body")
        {
            anyhow::bail!("native server launcher source must dispatch through generated router");
        }
        if !source.contains("fn orv_native_http_response(") {
            anyhow::bail!("native server launcher source must serialize native HTTP responses");
        }
        if source.contains(r#"Command::new("orv")"#) || source.contains(r#".arg("run-artifact")"#) {
            anyhow::bail!(
                "native server launcher source must not shell through `orv run-artifact`"
            );
        }
    } else {
        if !source.contains("fn orv_native_reference_bridge(")
            || !source.contains(r#"Command::new("orv")"#)
            || !source.contains(r#".arg("run-artifact")"#)
        {
            anyhow::bail!("native server launcher source must fall back to `orv run-artifact`");
        }
        if !source.contains("std::env::args_os().skip(1)") {
            anyhow::bail!("native server launcher source must forward process arguments");
        }
    }
    if !source.contains("mod routes;") || !source.contains("routes::ORV_NATIVE_ROUTE_COUNT") {
        anyhow::bail!("native server launcher source must link generated routes source");
    }
    if !source.contains(r#"routes::orv_native_match_route("__orv_probe__", "__orv_probe__")"#) {
        anyhow::bail!("native server launcher source must link generated route matcher");
    }
    if !source.contains("mod router;") || !source.contains("router::ORV_NATIVE_HANDLER_COUNT") {
        anyhow::bail!("native server launcher source must link generated router source");
    }
    if !source.contains(r#"router::orv_native_dispatch("__orv_probe__", "__orv_probe__")"#) {
        anyhow::bail!("native server launcher source must link generated router dispatch");
    }
    if !source.contains("mod handlers;") || !source.contains("handlers::ORV_NATIVE_HANDLER_COUNT") {
        anyhow::bail!("native server launcher source must link generated handlers source");
    }
    if source != expected {
        anyhow::bail!("native server launcher source must match generated source");
    }
    Ok(())
}

pub(crate) fn verify_native_server_routes_source(
    target: &Path,
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    if !target.is_file() {
        anyhow::bail!("missing native server routes source: {}", target.display());
    }
    let source = std::fs::read_to_string(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    let expected = orv_compiler::native_server_routes_source(artifact);
    if source != expected {
        anyhow::bail!("native server routes source must match server runtime artifact");
    }
    Ok(())
}

pub(crate) fn verify_native_server_router_source(target: &Path) -> anyhow::Result<()> {
    if !target.is_file() {
        anyhow::bail!("missing native server router source: {}", target.display());
    }
    let source = std::fs::read_to_string(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    let expected = orv_compiler::native_server_router_source();
    if source != expected {
        anyhow::bail!("native server router source must match generated source");
    }
    Ok(())
}

pub(crate) fn verify_native_server_handlers_source(
    target: &Path,
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    if !target.is_file() {
        anyhow::bail!(
            "missing native server handlers source: {}",
            target.display()
        );
    }
    let source = std::fs::read_to_string(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    let expected = orv_compiler::native_server_handlers_source(artifact);
    if source != expected {
        anyhow::bail!("native server handlers source must match generated source");
    }
    Ok(())
}

pub(crate) fn verify_static_page_target(
    bundle: &serde_json::Value,
    target: &Path,
) -> anyhow::Result<()> {
    let runtime_features = bundle
        .get("runtime_features")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("static_page runtime_features must be an array"))?;
    if !runtime_features.is_empty() {
        anyhow::bail!("static_page bundle must be zero-runtime");
    }
    let html = std::fs::read_to_string(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    let trimmed = html.trim_start();
    if trimmed.is_empty() {
        anyhow::bail!("static_page bundle is empty: {}", target.display());
    }
    if !(trimmed.starts_with("<html") || trimmed.starts_with("<!doctype")) {
        anyhow::bail!("static_page bundle is not html: {}", target.display());
    }
    Ok(())
}

pub(crate) fn verify_client_page_target(
    bundle: &serde_json::Value,
    target: &Path,
) -> anyhow::Result<()> {
    let runtime_features = bundle
        .get("runtime_features")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("client_page runtime_features must be an array"))?;
    if !runtime_features
        .iter()
        .any(|feature| feature == "client_wasm")
    {
        anyhow::bail!("client_page bundle must declare client_wasm");
    }
    verify_client_page_file(target)
}

pub(crate) fn verify_client_manifest_target(
    dir: &Path,
    bundle: &serde_json::Value,
    target: &Path,
) -> anyhow::Result<()> {
    if json_str(bundle, "path", "client_manifest bundle")? != CLIENT_MANIFEST_PATH {
        anyhow::bail!("client_manifest bundle path must be {CLIENT_MANIFEST_PATH}");
    }
    let runtime_features = bundle
        .get("runtime_features")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("client_manifest runtime_features must be an array"))?;
    if !runtime_features
        .iter()
        .any(|feature| feature == "client_wasm")
    {
        anyhow::bail!("client_manifest bundle must declare client_wasm");
    }
    let manifest = read_json_value(target)?;
    verify_client_manifest_value(dir, &manifest)
}

pub(crate) fn verify_client_manifest_value(
    dir: &Path,
    manifest: &serde_json::Value,
) -> anyhow::Result<()> {
    if manifest
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("client_manifest schema_version must be 1");
    }
    if json_str(manifest, "kind", "client manifest")? != "orv.client.bundle" {
        anyhow::bail!("client_manifest kind must be orv.client.bundle");
    }
    verify_client_manifest_paths(dir, manifest)?;
    verify_client_manifest_source_binding(dir, manifest)?;
    verify_client_manifest_artifact_hashes(dir, manifest)?;
    verify_client_manifest_capabilities(dir, manifest)?;
    verify_client_manifest_wasm_hash(dir, manifest)?;
    verify_client_manifest_exports(manifest)?;
    verify_client_manifest_initial_render(dir, manifest)?;
    verify_client_blocker_details(manifest, "client_manifest")
}

pub(crate) fn verify_client_manifest_paths(
    dir: &Path,
    manifest: &serde_json::Value,
) -> anyhow::Result<()> {
    let reactive_plan = json_str(manifest, "reactive_plan", "client manifest")?;
    if reactive_plan != CLIENT_REACTIVE_PLAN_PATH || !dir.join(reactive_plan).is_file() {
        anyhow::bail!("client_manifest reactive_plan must be {CLIENT_REACTIVE_PLAN_PATH}");
    }
    let page = json_str(manifest, "page", "client manifest")?;
    if page != CLIENT_PAGE_PATH || !dir.join(page).is_file() {
        anyhow::bail!("client_manifest page must be {CLIENT_PAGE_PATH}");
    }
    let loader = json_str(manifest, "loader", "client manifest")?;
    if loader != CLIENT_JS_PATH || !dir.join(loader).is_file() {
        anyhow::bail!("client_manifest loader must be {CLIENT_JS_PATH}");
    }
    let wasm = json_str(manifest, "wasm", "client manifest")?;
    if wasm != CLIENT_WASM_PATH || !dir.join(wasm).is_file() {
        anyhow::bail!("client_manifest wasm must be {CLIENT_WASM_PATH}");
    }
    Ok(())
}

pub(crate) fn verify_client_manifest_source_binding(
    dir: &Path,
    manifest: &serde_json::Value,
) -> anyhow::Result<()> {
    if json_str(manifest, "source_bundle", "client manifest")? != SOURCE_BUNDLE_PATH {
        anyhow::bail!("client_manifest source_bundle must be {SOURCE_BUNDLE_PATH}");
    }
    let source_bundle = read_json_value(&dir.join(SOURCE_BUNDLE_PATH))?;
    let expected_hash = stable_json_hash(&source_bundle)?;
    if json_str(manifest, "source_bundle_hash", "client manifest")? != expected_hash {
        anyhow::bail!("client_manifest source_bundle_hash does not match source bundle");
    }
    if manifest.get("entry") != source_bundle.get("entry") {
        anyhow::bail!("client_manifest entry does not match source bundle");
    }
    if !manifest
        .get("runtime_features")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|features| features.iter().any(|feature| feature == "client_wasm"))
    {
        anyhow::bail!("client_manifest runtime_features must include client_wasm");
    }
    Ok(())
}

pub(crate) fn verify_client_manifest_wasm_hash(
    dir: &Path,
    manifest: &serde_json::Value,
) -> anyhow::Result<()> {
    let wasm = json_str(manifest, "wasm", "client manifest")?;
    let expected_hash = file_content_hash(&dir.join(wasm))?;
    if json_str(manifest, "wasm_hash", "client manifest")? != expected_hash {
        anyhow::bail!("client_manifest wasm_hash does not match wasm bundle");
    }
    Ok(())
}

pub(crate) fn verify_client_manifest_artifact_hashes(
    dir: &Path,
    manifest: &serde_json::Value,
) -> anyhow::Result<()> {
    let loader = json_str(manifest, "loader", "client manifest")?;
    let expected_loader_hash = file_content_hash(&dir.join(loader))?;
    if json_str(manifest, "loader_hash", "client manifest")? != expected_loader_hash {
        anyhow::bail!("client_manifest loader_hash does not match loader");
    }
    let reactive_plan = json_str(manifest, "reactive_plan", "client manifest")?;
    let reactive_plan = read_json_value(&dir.join(reactive_plan))?;
    let expected_reactive_plan_hash = stable_json_hash(&reactive_plan)?;
    if json_str(manifest, "reactive_plan_hash", "client manifest")? != expected_reactive_plan_hash {
        anyhow::bail!("client_manifest reactive_plan_hash does not match reactive plan");
    }
    Ok(())
}

pub(crate) fn verify_client_manifest_capabilities(
    dir: &Path,
    manifest: &serde_json::Value,
) -> anyhow::Result<()> {
    let reactive_plan = json_str(manifest, "reactive_plan", "client manifest")?;
    let reactive_plan = read_json_value(&dir.join(reactive_plan))?;
    verify_client_reactive_plan_value(dir, &reactive_plan)?;
    let expected = client_bundle_capabilities_json(&reactive_plan);
    if manifest.get("capabilities") != Some(&expected) {
        anyhow::bail!("client_manifest capabilities do not match reactive plan");
    }
    Ok(())
}

pub(crate) fn verify_client_manifest_exports(manifest: &serde_json::Value) -> anyhow::Result<()> {
    let exports = manifest
        .get("exports")
        .ok_or_else(|| anyhow::anyhow!("client_manifest exports must be an object"))?;
    if json_str(exports, "start", "client manifest exports")? != CLIENT_WASM_START_EXPORT
        || json_str(exports, "render_ptr", "client manifest exports")?
            != CLIENT_WASM_RENDER_PTR_EXPORT
        || json_str(exports, "render_len", "client manifest exports")?
            != CLIENT_WASM_RENDER_LEN_EXPORT
        || json_str(exports, "memory", "client manifest exports")? != CLIENT_WASM_MEMORY_EXPORT
    {
        anyhow::bail!("client_manifest exports do not match client WASM ABI");
    }
    Ok(())
}

pub(crate) fn verify_client_manifest_initial_render(
    dir: &Path,
    manifest: &serde_json::Value,
) -> anyhow::Result<()> {
    let manifest_initial_render = manifest
        .get("initial_render")
        .ok_or_else(|| anyhow::anyhow!("client_manifest initial_render must be an object"))?;
    let wasm = json_str(manifest, "wasm", "client manifest")?;
    let wasm_metadata = client_wasm_metadata_value(&dir.join(wasm))?;
    let wasm_initial_render = wasm_metadata
        .get("initial_render")
        .ok_or_else(|| anyhow::anyhow!("client_wasm ORV metadata missing initial_render"))?;
    for field in ["content_type", "encoding", "html_hash", "byte_length"] {
        if manifest_initial_render.get(field) != wasm_initial_render.get(field) {
            anyhow::bail!("client_manifest initial_render does not match client WASM metadata");
        }
    }
    Ok(())
}

pub(crate) fn verify_client_reactive_plan_target(
    dir: &Path,
    bundle: &serde_json::Value,
    target: &Path,
) -> anyhow::Result<()> {
    if json_str(bundle, "path", "client_reactive_plan bundle")? != CLIENT_REACTIVE_PLAN_PATH {
        anyhow::bail!("client_reactive_plan bundle path must be {CLIENT_REACTIVE_PLAN_PATH}");
    }
    let runtime_features = bundle
        .get("runtime_features")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("client_reactive_plan runtime_features must be an array"))?;
    if !runtime_features
        .iter()
        .any(|feature| feature == "client_wasm")
    {
        anyhow::bail!("client_reactive_plan bundle must declare client_wasm");
    }
    let plan = read_json_value(target)?;
    verify_client_reactive_plan_value(dir, &plan)
}

pub(crate) fn verify_client_reactive_plan_value(
    dir: &Path,
    plan: &serde_json::Value,
) -> anyhow::Result<()> {
    if plan
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("client_reactive_plan schema_version must be 1");
    }
    if json_str(plan, "kind", "client reactive plan")? != "orv.client.reactive_plan" {
        anyhow::bail!("client_reactive_plan kind must be orv.client.reactive_plan");
    }
    if json_str(plan, "source_bundle", "client reactive plan")? != SOURCE_BUNDLE_PATH {
        anyhow::bail!("client_reactive_plan source_bundle must be {SOURCE_BUNDLE_PATH}");
    }
    let source_bundle = read_json_value(&dir.join(SOURCE_BUNDLE_PATH))?;
    let expected_hash = stable_json_hash(&source_bundle)?;
    if json_str(plan, "source_bundle_hash", "client reactive plan")? != expected_hash {
        anyhow::bail!("client_reactive_plan source_bundle_hash does not match source bundle");
    }
    if plan.get("entry") != source_bundle.get("entry") {
        anyhow::bail!("client_reactive_plan entry does not match source bundle");
    }
    let signals = plan
        .get("signals")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("client_reactive_plan signals must be an array"))?;
    if !signals.iter().all(|signal| {
        signal
            .get("name")
            .and_then(serde_json::Value::as_str)
            .is_some()
            && signal
                .get("origin_id")
                .and_then(serde_json::Value::as_str)
                .is_some()
            && signal
                .get("state_key")
                .and_then(serde_json::Value::as_str)
                .is_some()
            && signal
                .get("initial_value")
                .and_then(|value| value.get("kind"))
                .and_then(serde_json::Value::as_str)
                .is_some()
    }) {
        anyhow::bail!("client_reactive_plan signals must be an array of source-backed signals");
    }
    let bindings = plan
        .get("bindings")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("client_reactive_plan bindings must be an array"))?;
    verify_client_reactive_plan_initial_render_binding(dir, bindings)?;
    if !signals.iter().all(|signal| {
        let origin_id = signal
            .get("origin_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let state_key = signal
            .get("state_key")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        bindings.iter().any(|binding| {
            binding.get("kind").and_then(serde_json::Value::as_str) == Some("signal_state")
                && binding.get("target").and_then(serde_json::Value::as_str) == Some(CLIENT_JS_PATH)
                && binding.get("source").and_then(serde_json::Value::as_str) == Some(origin_id)
                && binding.get("state_key").and_then(serde_json::Value::as_str) == Some(state_key)
        })
    }) {
        anyhow::bail!("client_reactive_plan signal_state binding is missing");
    }
    if !client_reactive_plan_signal_text_bindings_are_valid(signals, bindings) {
        anyhow::bail!("client_reactive_plan signal_text binding is invalid");
    }
    if !client_reactive_plan_signal_attr_bindings_are_valid(signals, bindings) {
        anyhow::bail!("client_reactive_plan signal_attr binding is invalid");
    }
    if !client_reactive_plan_signal_event_bindings_are_valid(signals, bindings) {
        anyhow::bail!("client_reactive_plan signal_event binding is invalid");
    }
    if !plan
        .get("blocked_by")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item == "reactive-dom-diff"))
    {
        anyhow::bail!("client_reactive_plan blocked_by must include reactive-dom-diff");
    }
    verify_client_blocker_details(plan, "client_reactive_plan")?;
    Ok(())
}

pub(crate) fn verify_client_blocker_details(
    value: &serde_json::Value,
    context: &str,
) -> anyhow::Result<()> {
    let blocked_by = value
        .get("blocked_by")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("{context} blocked_by must be an array"))?;
    let blockers = value
        .get("blockers")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("{context} blockers must be an array"))?;
    for blocked in blocked_by {
        let Some(id) = blocked.as_str() else {
            anyhow::bail!("{context} blocked_by entries must be strings");
        };
        if !blockers.iter().any(|blocker| {
            blocker.get("id").and_then(serde_json::Value::as_str) == Some(id)
                && blocker
                    .get("artifact")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|artifact| !artifact.is_empty())
                && blocker
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|reason| !reason.is_empty())
        }) {
            anyhow::bail!("{context} blockers must describe blocked_by entry {id}");
        }
    }
    Ok(())
}

pub(crate) fn verify_client_page_file(target: &Path) -> anyhow::Result<()> {
    let html = std::fs::read_to_string(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    let trimmed = html.trim_start();
    if trimmed.is_empty() {
        anyhow::bail!("client_page bundle is empty: {}", target.display());
    }
    if !(trimmed.starts_with("<html") || trimmed.starts_with("<!doctype")) {
        anyhow::bail!("client_page bundle is not html: {}", target.display());
    }
    if !html.contains("data-orv-client=\"wasm\"") {
        anyhow::bail!("client_page bundle does not declare wasm bootstrap");
    }
    if !html.contains("type=\"module\"") || !html.contains("client/app.js") {
        anyhow::bail!("client_page bundle does not load client/app.js");
    }
    Ok(())
}

pub(crate) fn verify_client_reactive_plan_initial_render_binding(
    dir: &Path,
    bindings: &[serde_json::Value],
) -> anyhow::Result<()> {
    let binding = bindings
        .iter()
        .find(|binding| {
            binding.get("kind").and_then(serde_json::Value::as_str) == Some("initial_render")
                && binding.get("target").and_then(serde_json::Value::as_str)
                    == Some(CLIENT_PAGE_PATH)
                && binding.get("source").and_then(serde_json::Value::as_str)
                    == Some(CLIENT_WASM_PATH)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "client_reactive_plan initial_render binding must target {CLIENT_PAGE_PATH} from {CLIENT_WASM_PATH}"
            )
        })?;
    let manifest = read_json_value(&dir.join(CLIENT_MANIFEST_PATH))?;
    let initial_render = manifest
        .get("initial_render")
        .ok_or_else(|| anyhow::anyhow!("client_manifest initial_render must be an object"))?;
    for field in ["html_hash", "byte_length"] {
        if binding.get(field) != initial_render.get(field) {
            anyhow::bail!(
                "client_reactive_plan initial_render binding does not match client manifest"
            );
        }
    }
    Ok(())
}

pub(crate) fn client_reactive_plan_signal_text_bindings_are_valid(
    signals: &[serde_json::Value],
    bindings: &[serde_json::Value],
) -> bool {
    bindings
        .iter()
        .filter(|binding| {
            binding.get("kind").and_then(serde_json::Value::as_str) == Some("signal_text")
        })
        .all(|binding| client_reactive_plan_signal_text_binding_is_valid(signals, binding))
}

pub(crate) fn client_reactive_plan_signal_text_binding_is_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    let origin_id = binding
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let state_key = binding
        .get("state_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    binding.get("target").and_then(serde_json::Value::as_str) == Some(CLIENT_PAGE_PATH)
        && binding
            .get("selector")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|selector| !selector.is_empty())
        && signals.iter().any(|signal| {
            signal.get("origin_id").and_then(serde_json::Value::as_str) == Some(origin_id)
                && signal.get("state_key").and_then(serde_json::Value::as_str) == Some(state_key)
        })
        && client_signal_text_state_keys_are_valid(signals, binding)
        && client_signal_text_sources_are_valid(signals, binding)
        && client_signal_text_template_is_valid(signals, binding)
        && client_signal_text_condition_is_valid(signals, binding)
}

pub(crate) fn client_signal_text_binding_state_keys(
    binding: &serde_json::Value,
) -> Option<Vec<&str>> {
    if let Some(state_keys) = binding.get("state_keys") {
        let state_keys = state_keys.as_array()?;
        if state_keys.is_empty() {
            return None;
        }
        return state_keys
            .iter()
            .map(serde_json::Value::as_str)
            .collect::<Option<Vec<_>>>();
    }
    binding
        .get("state_key")
        .and_then(serde_json::Value::as_str)
        .map(|state_key| vec![state_key])
}

pub(crate) fn client_signal_text_state_keys_are_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    let Some(state_keys) = client_signal_text_binding_state_keys(binding) else {
        return false;
    };
    let binding_state_key = binding
        .get("state_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    state_keys.contains(&binding_state_key)
        && state_keys.iter().all(|state_key| {
            signals.iter().any(|signal| {
                signal.get("state_key").and_then(serde_json::Value::as_str) == Some(*state_key)
            })
        })
}

pub(crate) fn client_signal_text_sources_are_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    let Some(sources) = binding.get("sources") else {
        return true;
    };
    let Some(sources) = sources.as_array() else {
        return false;
    };
    let Some(state_keys) = client_signal_text_binding_state_keys(binding) else {
        return false;
    };
    !sources.is_empty()
        && sources.iter().all(|source| {
            let origin_id = source
                .get("source")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let state_key = source
                .get("state_key")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            state_keys.contains(&state_key)
                && signals.iter().any(|signal| {
                    signal.get("origin_id").and_then(serde_json::Value::as_str) == Some(origin_id)
                        && signal.get("state_key").and_then(serde_json::Value::as_str)
                            == Some(state_key)
                })
        })
}

pub(crate) fn client_signal_text_template_is_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    let Some(template) = binding.get("text_template") else {
        return true;
    };
    let Some(segments) = template.as_array() else {
        return false;
    };
    let Some(state_keys) = client_signal_text_binding_state_keys(binding) else {
        return false;
    };
    !segments.is_empty()
        && segments.iter().all(|segment| {
            match segment.get("kind").and_then(serde_json::Value::as_str) {
                Some("text") => segment
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .is_some(),
                Some("signal") => {
                    let state_key = segment
                        .get("state_key")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    state_keys.contains(&state_key)
                        && signals.iter().any(|signal| {
                            signal.get("state_key").and_then(serde_json::Value::as_str)
                                == Some(state_key)
                        })
                }
                _ => false,
            }
        })
}

pub(crate) fn client_signal_text_condition_is_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    let Some(condition) = binding.get("text_condition") else {
        return true;
    };
    client_signal_condition_binding_is_valid(signals, binding, condition)
}

pub(crate) fn client_reactive_plan_signal_attr_bindings_are_valid(
    signals: &[serde_json::Value],
    bindings: &[serde_json::Value],
) -> bool {
    bindings
        .iter()
        .filter(|binding| {
            binding.get("kind").and_then(serde_json::Value::as_str) == Some("signal_attr")
        })
        .all(|binding| client_reactive_plan_signal_attr_binding_is_valid(signals, binding))
}

pub(crate) fn client_reactive_plan_signal_attr_binding_is_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    let origin_id = binding
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let state_key = binding
        .get("state_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    binding.get("target").and_then(serde_json::Value::as_str) == Some(CLIENT_PAGE_PATH)
        && binding
            .get("selector")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|selector| !selector.is_empty())
        && binding
            .get("attr")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|attr| !attr.is_empty())
        && signals.iter().any(|signal| {
            signal.get("origin_id").and_then(serde_json::Value::as_str) == Some(origin_id)
                && signal.get("state_key").and_then(serde_json::Value::as_str) == Some(state_key)
        })
        && client_signal_attr_state_keys_are_valid(signals, binding)
        && client_signal_attr_sources_are_valid(signals, binding)
        && client_signal_attr_template_is_valid(signals, binding)
        && client_signal_attr_condition_is_valid(signals, binding)
}

pub(crate) fn client_signal_attr_state_keys_are_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    client_signal_text_state_keys_are_valid(signals, binding)
}

pub(crate) fn client_signal_attr_sources_are_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    client_signal_text_sources_are_valid(signals, binding)
}

pub(crate) fn client_signal_attr_template_is_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    let Some(template) = binding.get("attr_template") else {
        return true;
    };
    let Some(segments) = template.as_array() else {
        return false;
    };
    let Some(state_keys) = client_signal_text_binding_state_keys(binding) else {
        return false;
    };
    !segments.is_empty()
        && segments.iter().all(|segment| {
            match segment.get("kind").and_then(serde_json::Value::as_str) {
                Some("text") => segment
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .is_some(),
                Some("signal") => {
                    let state_key = segment
                        .get("state_key")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    state_keys.contains(&state_key)
                        && signals.iter().any(|signal| {
                            signal.get("state_key").and_then(serde_json::Value::as_str)
                                == Some(state_key)
                        })
                }
                _ => false,
            }
        })
}

pub(crate) fn client_signal_attr_condition_is_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    let Some(condition) = binding.get("attr_condition") else {
        return true;
    };
    client_signal_condition_binding_is_valid(signals, binding, condition)
}

pub(crate) fn client_signal_condition_binding_is_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
    condition: &serde_json::Value,
) -> bool {
    let state_key = condition
        .get("state_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let binding_state_key = binding
        .get("state_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    state_key == binding_state_key
        && condition
            .get("truthy")
            .and_then(serde_json::Value::as_str)
            .is_some()
        && condition
            .get("falsy")
            .and_then(serde_json::Value::as_str)
            .is_some()
        && client_signal_attr_condition_comparison_is_valid(condition)
        && signals.iter().any(|signal| {
            signal.get("state_key").and_then(serde_json::Value::as_str) == Some(state_key)
        })
}

pub(crate) fn client_signal_attr_condition_comparison_is_valid(
    condition: &serde_json::Value,
) -> bool {
    match (condition.get("op"), condition.get("rhs")) {
        (None, None) => true,
        (Some(op), Some(rhs)) => {
            op.as_str()
                .is_some_and(|op| matches!(op, "eq" | "ne" | "lt" | "gt" | "le" | "ge"))
                && client_signal_condition_operand_is_valid(rhs)
        }
        _ => false,
    }
}

pub(crate) fn client_signal_condition_operand_is_valid(value: &serde_json::Value) -> bool {
    let Some(kind) = value.get("kind").and_then(serde_json::Value::as_str) else {
        return false;
    };
    match kind {
        "int" | "float" | "string" => value
            .get("value")
            .and_then(serde_json::Value::as_str)
            .is_some(),
        "bool" => value
            .get("value")
            .and_then(serde_json::Value::as_bool)
            .is_some(),
        _ => false,
    }
}

pub(crate) fn client_reactive_plan_signal_event_bindings_are_valid(
    signals: &[serde_json::Value],
    bindings: &[serde_json::Value],
) -> bool {
    bindings
        .iter()
        .filter(|binding| {
            binding.get("kind").and_then(serde_json::Value::as_str) == Some("signal_event")
        })
        .all(|binding| client_reactive_plan_signal_event_binding_is_valid(signals, binding))
}

pub(crate) fn client_reactive_plan_signal_event_binding_is_valid(
    signals: &[serde_json::Value],
    binding: &serde_json::Value,
) -> bool {
    let origin_id = binding
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let state_key = binding
        .get("state_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    binding.get("target").and_then(serde_json::Value::as_str) == Some(CLIENT_PAGE_PATH)
        && binding
            .get("selector")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|selector| !selector.is_empty())
        && binding
            .get("event")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|event| !event.is_empty())
        && client_signal_event_action_is_valid(binding.get("action"))
        && signals.iter().any(|signal| {
            signal.get("origin_id").and_then(serde_json::Value::as_str) == Some(origin_id)
                && signal.get("state_key").and_then(serde_json::Value::as_str) == Some(state_key)
        })
}

pub(crate) fn client_signal_event_action_is_valid(action: Option<&serde_json::Value>) -> bool {
    let Some(action) = action else {
        return false;
    };
    match action.get("kind").and_then(serde_json::Value::as_str) {
        Some(
            "assign_toggle"
            | "assign_event_target_value"
            | "assign_event_target_checked"
            | "assign_event_target_value_float"
            | "assign_event_target_value_int",
        ) => true,
        Some("assign" | "assign_add" | "assign_sub") => action
            .get("value")
            .and_then(|value| value.get("kind"))
            .and_then(serde_json::Value::as_str)
            .is_some(),
        _ => false,
    }
}

pub(crate) fn verify_client_js_target(target: &Path) -> anyhow::Result<()> {
    let source = std::fs::read_to_string(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    if !source.contains("ORV_CLIENT_BOOTSTRAP") {
        anyhow::bail!("client_js bundle does not declare ORV bootstrap metadata");
    }
    if !source.contains("sourceBundleUrl") || !source.contains("../source-bundle.json") {
        anyhow::bail!("client_js bundle does not reference source bundle metadata");
    }
    if !source.contains("sourceBundleHash") {
        anyhow::bail!("client_js bundle does not declare source bundle hash metadata");
    }
    if !source.contains("sourceFileCount") {
        anyhow::bail!("client_js bundle does not declare source bundle file count metadata");
    }
    if !source.contains("manifestUrl")
        || !source.contains("./manifest.json")
        || !source.contains("loadClientManifest")
        || !source.contains("client manifest fetch failed")
        || !source.contains("client manifest hash mismatch")
        || !source.contains("client manifest export mismatch")
        || !source.contains("validateWasmBundle")
        || !source.contains("client wasm hash mismatch")
    {
        anyhow::bail!("client_js bundle does not verify client manifest contract");
    }
    if !source.contains("reactivePlanUrl")
        || !source.contains("./reactive-plan.json")
        || !source.contains("loadReactivePlan")
        || !source.contains("embeddedReactivePlan")
        || !source.contains("embeddedReactivePlanHash")
        || !source.contains("loadEmbeddedReactivePlan")
        || !source.contains("validateReactivePlan")
        || !source.contains("client embedded reactive plan hash mismatch")
        || !source.contains("validateReactiveBindings")
        || !source.contains("client reactive plan fetch failed")
        || !source.contains("client reactive plan hash mismatch")
        || !source.contains("client reactive plan initial_render binding mismatch")
        || !source.contains("client reactive plan signal_state binding mismatch")
        || !source.contains("client reactive plan signal_text binding mismatch")
        || !source.contains("client reactive plan signal_attr binding mismatch")
        || !source.contains("client reactive plan signal_event binding mismatch")
        || !source.contains("renderSignalTextBinding")
        || !source.contains("text_template")
        || !source.contains("renderSignalTextCondition")
        || !source.contains("text_condition")
        || !source.contains("signalTextBindingStateKeys")
        || !source.contains("signalTextBindingCursorKey")
        || !source.contains("state_keys")
        || !source.contains("renderSignalAttrBinding")
        || !source.contains("attr_template")
        || !source.contains("signalAttrBindingStateKeys")
        || !source.contains("signalAttrBindingCursorKey")
        || !source.contains("renderSignalAttrCondition")
        || !source.contains("attr_condition")
        || !source.contains("compareSignalAttrCondition")
        || !source.contains("decodeSignalConditionOperand")
        || !source.contains("createReactiveState")
        || !source.contains("bindReactiveDom")
        || !source.contains("bindReactiveAttrs")
        || !source.contains("bindReactiveEvents")
        || !source.contains("applySignalAction")
        || !source.contains("assign_add")
        || !source.contains("assign_sub")
        || !source.contains("assign_toggle")
        || !source.contains("assign_event_target_value")
        || !source.contains("assign_event_target_checked")
        || !source.contains("assign_event_target_value_float")
        || !source.contains("assign_event_target_value_int")
        || !source.contains("setSignal")
        || !source.contains("orvReactiveSignals")
        || !source.contains("orvReactiveBindings")
        || !source.contains("orvReactiveDomBindings")
        || !source.contains("orvReactiveAttrBindings")
        || !source.contains("orvReactiveEventBindings")
        || !source.contains("orvReactiveStateHash")
        || !source.contains("__ORV_CLIENT_REACTIVE_STATE__")
        || !source.contains("__ORV_SET_SIGNAL__")
    {
        anyhow::bail!("client_js bundle does not verify client reactive plan contract");
    }
    if !source.contains("loadSourceBundle")
        || !source.contains("sourceFileCount")
        || !source.contains("fnv1a64")
        || !source.contains("source bundle hash mismatch")
    {
        anyhow::bail!("client_js bundle does not verify source bundle hash");
    }
    if !source.contains("app.wasm") {
        anyhow::bail!("client_js bundle does not reference app.wasm");
    }
    if !source.contains("WebAssembly.instantiate") {
        anyhow::bail!("client_js bundle does not instantiate wasm");
    }
    if !source.contains("readInitialRender")
        || !source.contains("orv_render_ptr")
        || !source.contains("orv_render_len")
        || !source.contains("TextDecoder")
        || !source.contains("#orv-root")
        || !source.contains("initialRenderMountHtml")
        || !source.contains("DOMParser")
        || !source.contains("root.innerHTML")
    {
        anyhow::bail!("client_js bundle does not decode initial render from wasm");
    }
    if !source.contains("instance.exports.orv_start()") {
        anyhow::bail!("client_js bundle does not call {CLIENT_WASM_START_EXPORT}");
    }
    if !source.contains("validateInitialRender")
        || !source.contains("initial_render")
        || !source.contains("html_hash")
        || !source.contains("client initial render hash mismatch")
        || !source.contains("client initial render byte length mismatch")
    {
        anyhow::bail!("client_js bundle does not verify initial render contract");
    }
    Ok(())
}

pub(crate) fn verify_client_wasm_target(dir: &Path, target: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    verify_client_wasm_bytes(dir, target, &bytes)
}

pub(crate) fn client_wasm_metadata_value(target: &Path) -> anyhow::Result<serde_json::Value> {
    let bytes = std::fs::read(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    client_wasm_metadata_value_from_bytes(&bytes)
}

pub(crate) fn client_wasm_metadata_value_from_bytes(
    bytes: &[u8],
) -> anyhow::Result<serde_json::Value> {
    let payload = client_wasm_custom_section_payload(bytes)?
        .ok_or_else(|| anyhow::anyhow!("client_wasm bundle does not declare ORV metadata"))?;
    let payload = std::str::from_utf8(payload)
        .map_err(|e| anyhow::anyhow!("client_wasm ORV metadata is not UTF-8: {e}"))?;
    serde_json::from_str(payload)
        .map_err(|e| anyhow::anyhow!("client_wasm ORV metadata is not JSON: {e}"))
}

pub(crate) fn verify_client_wasm_bytes(
    dir: &Path,
    target: &Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    if bytes.len() < WASM_MODULE_HEADER.len() {
        anyhow::bail!("client_wasm bundle is too small: {}", target.display());
    }
    if &bytes[..4] != b"\0asm" {
        anyhow::bail!("client_wasm bundle has invalid magic: {}", target.display());
    }
    if &bytes[4..8] != b"\x01\0\0\0" {
        anyhow::bail!(
            "client_wasm bundle has unsupported version: {}",
            target.display()
        );
    }
    let metadata = client_wasm_metadata_value_from_bytes(bytes)?;
    if metadata
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("client_wasm ORV metadata schema_version must be 1");
    }
    if metadata
        .get("source_bundle")
        .and_then(serde_json::Value::as_str)
        != Some(CLIENT_WASM_SOURCE_BUNDLE_PATH)
    {
        anyhow::bail!("client_wasm ORV metadata source_bundle is invalid");
    }
    let source_bundle = read_json_value(&dir.join("source-bundle.json"))?;
    let expected_source_bundle_hash = stable_json_hash(&source_bundle)?;
    if metadata
        .get("source_bundle_hash")
        .and_then(serde_json::Value::as_str)
        != Some(expected_source_bundle_hash.as_str())
    {
        anyhow::bail!("client_wasm ORV metadata source_bundle_hash is invalid");
    }
    if metadata.get("entry") != source_bundle.get("entry") {
        anyhow::bail!("client_wasm ORV metadata entry is invalid");
    }
    if !metadata
        .get("runtime_features")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|features| features.iter().any(|feature| feature == "client_wasm"))
    {
        anyhow::bail!("client_wasm ORV metadata must include client_wasm runtime feature");
    }
    let initial_render = metadata
        .get("initial_render")
        .ok_or_else(|| anyhow::anyhow!("client_wasm ORV metadata missing initial_render"))?;
    if initial_render
        .get("content_type")
        .and_then(serde_json::Value::as_str)
        != Some("text/html")
    {
        anyhow::bail!("client_wasm initial_render content_type is invalid");
    }
    if initial_render
        .get("encoding")
        .and_then(serde_json::Value::as_str)
        != Some("utf-8")
    {
        anyhow::bail!("client_wasm initial_render encoding is invalid");
    }
    if initial_render
        .get("html_hash")
        .and_then(serde_json::Value::as_str)
        .is_none_or(str::is_empty)
    {
        anyhow::bail!("client_wasm initial_render html_hash is required");
    }
    if initial_render
        .get("byte_length")
        .and_then(serde_json::Value::as_u64)
        .is_none()
    {
        anyhow::bail!("client_wasm initial_render byte_length is required");
    }
    if initial_render
        .get("ptr_export")
        .and_then(serde_json::Value::as_str)
        != Some(CLIENT_WASM_RENDER_PTR_EXPORT)
        || initial_render
            .get("len_export")
            .and_then(serde_json::Value::as_str)
            != Some(CLIENT_WASM_RENDER_LEN_EXPORT)
        || initial_render
            .get("memory_export")
            .and_then(serde_json::Value::as_str)
            != Some(CLIENT_WASM_MEMORY_EXPORT)
    {
        anyhow::bail!("client_wasm initial_render export metadata is invalid");
    }
    if client_wasm_export_index(bytes, CLIENT_WASM_START_EXPORT, 0)? != Some(0) {
        anyhow::bail!("client_wasm bundle must export `{CLIENT_WASM_START_EXPORT}` function 0");
    }
    if !client_wasm_exports_function(bytes, CLIENT_WASM_RENDER_PTR_EXPORT)?
        || !client_wasm_exports_function(bytes, CLIENT_WASM_RENDER_LEN_EXPORT)?
    {
        anyhow::bail!("client_wasm bundle must export initial render pointer and length");
    }
    if client_wasm_export_index(bytes, CLIENT_WASM_MEMORY_EXPORT, 2)? != Some(0) {
        anyhow::bail!("client_wasm bundle must export initial render memory 0");
    }
    verify_client_wasm_initial_render_data(bytes, initial_render)?;
    Ok(())
}

pub(crate) fn verify_dev_hmr_session_if_present(
    dir: &Path,
    plan: &serde_json::Value,
) -> anyhow::Result<()> {
    let session_path = dir.join("dev").join("session.json");
    if !session_path.is_file() {
        return Ok(());
    }
    let session = read_json_value(&session_path)?;
    if session
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("dev session schema_version must be 1");
    }
    if json_str(&session, "mode", "dev session")? != "hmr" {
        anyhow::bail!("dev session mode must be hmr");
    }
    if json_str(&session, "source_bundle", "dev session")? != "source-bundle.json" {
        anyhow::bail!("dev session source_bundle must be source-bundle.json");
    }
    let watch = session
        .get("watch")
        .ok_or_else(|| anyhow::anyhow!("dev session watch must be an object"))?;
    let session_sources = watch
        .get("sources")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("dev session watch.sources must be an array"))?;
    let session_targets = watch
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("dev session watch.targets must be an array"))?;
    let source_bundle = read_json_value(&dir.join("source-bundle.json"))?;
    let expected_sources = source_bundle
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("source-bundle.json files must be an array"))?;
    for source in expected_sources {
        let path = json_str(source, "path", "source bundle file")?;
        let content_hash = json_str(source, "content_hash", "source bundle file")?;
        if !session_sources.iter().any(|session_source| {
            session_source
                .get("path")
                .and_then(serde_json::Value::as_str)
                == Some(path)
                && session_source
                    .get("content_hash")
                    .and_then(serde_json::Value::as_str)
                    == Some(content_hash)
        }) {
            anyhow::bail!("dev session missing source {path}");
        }
    }
    let bundles = plan
        .get("bundles")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("bundle plan bundles must be an array"))?;
    for bundle in bundles {
        let kind = json_str(bundle, "kind", "bundle target")?;
        let path = json_str(bundle, "path", "bundle target")?;
        if !session_targets.iter().any(|session_target| {
            session_target
                .get("kind")
                .and_then(serde_json::Value::as_str)
                == Some(kind)
                && session_target
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    == Some(path)
        }) {
            anyhow::bail!("dev session missing bundle target {kind}:{path}");
        }
    }
    let reload = session
        .get("reload")
        .ok_or_else(|| anyhow::anyhow!("dev session reload must be an object"))?;
    let has_client_target = bundles.iter().any(|target| {
        target
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_client_bundle_kind)
    });
    let expected_strategy = if has_client_target {
        "hot-reload"
    } else {
        "full-reload"
    };
    if json_str(reload, "strategy", "dev session reload")? != expected_strategy {
        anyhow::bail!("dev session reload strategy must be {expected_strategy}");
    }
    if json_str(reload, "fallback", "dev session reload")? != "full-reload" {
        anyhow::bail!("dev session reload fallback must be full-reload");
    }
    Ok(())
}

pub(crate) fn verify_dev_hmr_transport_if_present(dir: &Path) -> anyhow::Result<()> {
    let transport_path = dir.join("dev").join("transport.json");
    if !transport_path.is_file() {
        return Ok(());
    }
    if !dir.join("dev").join("session.json").is_file() {
        anyhow::bail!("dev hmr transport requires dev/session.json");
    }
    let transport = read_json_value(&transport_path)?;
    if transport
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("dev hmr transport schema_version must be 1");
    }
    if json_str(&transport, "mode", "dev hmr transport")? != "hmr-transport" {
        anyhow::bail!("dev hmr transport mode must be hmr-transport");
    }
    if json_str(&transport, "source_bundle", "dev hmr transport")? != "source-bundle.json" {
        anyhow::bail!("dev hmr transport source_bundle must be source-bundle.json");
    }
    if json_str(&transport, "session", "dev hmr transport")? != "dev/session.json" {
        anyhow::bail!("dev hmr transport session must be dev/session.json");
    }
    let browser = transport
        .get("browser")
        .ok_or_else(|| anyhow::anyhow!("dev hmr transport browser must be an object"))?;
    if json_str(browser, "kind", "dev hmr transport browser")? != "event-source" {
        anyhow::bail!("dev hmr transport browser kind must be event-source");
    }
    if json_str(browser, "client", "dev hmr transport browser")? != "dev/hmr-client.js" {
        anyhow::bail!("dev hmr transport browser client must be dev/hmr-client.js");
    }
    if json_str(browser, "event_source", "dev hmr transport browser")? != "/__orv/hmr/events" {
        anyhow::bail!("dev hmr transport browser event_source must be /__orv/hmr/events");
    }
    if json_str(browser, "session", "dev hmr transport browser")? != "/__orv/hmr/session" {
        anyhow::bail!("dev hmr transport browser session must be /__orv/hmr/session");
    }
    let server = transport
        .get("server")
        .ok_or_else(|| anyhow::anyhow!("dev hmr transport server must be an object"))?;
    if json_str(server, "kind", "dev hmr transport server")? != "reference-dev" {
        anyhow::bail!("dev hmr transport server kind must be reference-dev");
    }
    if json_str(server, "events", "dev hmr transport server")? != "dev/events.json" {
        anyhow::bail!("dev hmr transport server events must be dev/events.json");
    }
    let client_path = dir.join("dev").join("hmr-client.js");
    let client = std::fs::read_to_string(&client_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", client_path.display()))?;
    if !client.contains("EventSource('/__orv/hmr/events')") {
        anyhow::bail!("dev hmr client must connect to /__orv/hmr/events");
    }
    if !client.contains("window.location.reload()") {
        anyhow::bail!("dev hmr client must support full reload fallback");
    }
    Ok(())
}

pub(crate) fn verify_dev_hmr_server_if_present(dir: &Path) -> anyhow::Result<()> {
    let server_path = dir.join("dev").join("server.json");
    if !server_path.is_file() {
        return Ok(());
    }
    if !dir.join("dev").join("session.json").is_file() {
        anyhow::bail!("dev hmr server requires dev/session.json");
    }
    if !dir.join("dev").join("events.json").is_file() {
        anyhow::bail!("dev hmr server requires dev/events.json");
    }
    let server = read_json_value(&server_path)?;
    if server
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("dev hmr server schema_version must be 1");
    }
    if json_str(&server, "mode", "dev hmr server")? != "hmr-server" {
        anyhow::bail!("dev hmr server mode must be hmr-server");
    }
    if json_str(&server, "protocol", "dev hmr server")? != "http1" {
        anyhow::bail!("dev hmr server protocol must be http1");
    }
    if json_str(&server, "session", "dev hmr server")? != "dev/session.json" {
        anyhow::bail!("dev hmr server session must be dev/session.json");
    }
    if json_str(&server, "events", "dev hmr server")? != "dev/events.json" {
        anyhow::bail!("dev hmr server events must be dev/events.json");
    }
    let address = json_str(&server, "address", "dev hmr server")?;
    address
        .parse::<SocketAddr>()
        .map_err(|e| anyhow::anyhow!("dev hmr server address must be a socket address: {e}"))?;
    let endpoints = server
        .get("endpoints")
        .ok_or_else(|| anyhow::anyhow!("dev hmr server endpoints must be an object"))?;
    if json_str(endpoints, "session", "dev hmr server endpoints")? != "/__orv/hmr/session" {
        anyhow::bail!("dev hmr server session endpoint must be /__orv/hmr/session");
    }
    if json_str(endpoints, "events", "dev hmr server endpoints")? != "/__orv/hmr/events" {
        anyhow::bail!("dev hmr server events endpoint must be /__orv/hmr/events");
    }
    Ok(())
}

pub(crate) fn verify_dev_watch_session_if_present(
    dir: &Path,
    plan: &serde_json::Value,
) -> anyhow::Result<()> {
    let session_path = dir.join("dev").join("watch.json");
    if !session_path.is_file() {
        return Ok(());
    }
    let session = read_json_value(&session_path)?;
    if session
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("dev watch session schema_version must be 1");
    }
    if json_str(&session, "mode", "dev watch session")? != "watch" {
        anyhow::bail!("dev watch session mode must be watch");
    }
    if json_str(&session, "source_bundle", "dev watch session")? != "source-bundle.json" {
        anyhow::bail!("dev watch session source_bundle must be source-bundle.json");
    }
    verify_dev_watch_set(dir, plan, &session, "dev watch session")?;
    let loop_config = session
        .get("loop")
        .ok_or_else(|| anyhow::anyhow!("dev watch session loop must be an object"))?;
    if json_str(loop_config, "strategy", "dev watch session loop")? != "poll" {
        anyhow::bail!("dev watch session loop strategy must be poll");
    }
    if json_str(loop_config, "run", "dev watch session loop")? != "build-verify-run" {
        anyhow::bail!("dev watch session loop run must be build-verify-run");
    }
    let hmr = loop_config
        .get("hmr")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| anyhow::anyhow!("dev watch session loop hmr must be a boolean"))?;
    let interval_ms = loop_config
        .get("interval_ms")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("dev watch session loop interval_ms must be a number"))?;
    if interval_ms == 0 {
        anyhow::bail!("dev watch session loop interval_ms must be positive");
    }
    let reload = session
        .get("reload")
        .ok_or_else(|| anyhow::anyhow!("dev watch session reload must be an object"))?;
    let expected_strategy = if hmr && bundle_plan_has_client_target(plan)? {
        "hot-reload"
    } else {
        "full-reload"
    };
    if json_str(reload, "strategy", "dev watch session reload")? != expected_strategy {
        anyhow::bail!("dev watch session reload strategy must be {expected_strategy}");
    }
    if json_str(reload, "fallback", "dev watch session reload")? != "full-reload" {
        anyhow::bail!("dev watch session reload fallback must be full-reload");
    }
    let transport = session
        .get("transport")
        .ok_or_else(|| anyhow::anyhow!("dev watch session transport must be an object"))?;
    if json_str(transport, "kind", "dev watch session transport")? != "manifest" {
        anyhow::bail!("dev watch session transport kind must be manifest");
    }
    if json_str(transport, "path", "dev watch session transport")? != "dev/watch.json" {
        anyhow::bail!("dev watch session transport path must be dev/watch.json");
    }
    Ok(())
}

pub(crate) fn verify_dev_watch_events_if_present(dir: &Path) -> anyhow::Result<()> {
    let events_path = dir.join("dev").join("events.json");
    if !events_path.is_file() {
        return Ok(());
    }
    let events = read_json_value(&events_path)?;
    if events
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("dev watch events schema_version must be 1");
    }
    if json_str(&events, "mode", "dev watch events")? != "watch-loop" {
        anyhow::bail!("dev watch events mode must be watch-loop");
    }
    if json_str(&events, "source_bundle", "dev watch events")? != "source-bundle.json" {
        anyhow::bail!("dev watch events source_bundle must be source-bundle.json");
    }
    let transport = events
        .get("transport")
        .ok_or_else(|| anyhow::anyhow!("dev watch events transport must be an object"))?;
    if json_str(transport, "kind", "dev watch events transport")? != "manifest" {
        anyhow::bail!("dev watch events transport kind must be manifest");
    }
    if json_str(transport, "path", "dev watch events transport")? != "dev/events.json" {
        anyhow::bail!("dev watch events transport path must be dev/events.json");
    }
    let loop_config = events
        .get("loop")
        .ok_or_else(|| anyhow::anyhow!("dev watch events loop must be an object"))?;
    if json_str(loop_config, "strategy", "dev watch events loop")? != "poll" {
        anyhow::bail!("dev watch events loop strategy must be poll");
    }
    if json_str(loop_config, "run", "dev watch events loop")? != "build-verify-run" {
        anyhow::bail!("dev watch events loop run must be build-verify-run");
    }
    let interval_ms = loop_config
        .get("interval_ms")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("dev watch events loop interval_ms must be a number"))?;
    if interval_ms == 0 {
        anyhow::bail!("dev watch events loop interval_ms must be positive");
    }
    let event_items = events
        .get("events")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("dev watch events events must be an array"))?;
    if event_items.is_empty() {
        anyhow::bail!("dev watch events must contain at least one event");
    }
    for event in event_items {
        if event
            .get("iteration")
            .and_then(serde_json::Value::as_u64)
            .is_none()
        {
            anyhow::bail!("dev watch event iteration must be a number");
        }
        let action = json_str(event, "action", "dev watch event")?;
        if !matches!(action, "build-verify-run" | "skip") {
            anyhow::bail!("dev watch event action must be build-verify-run or skip");
        }
        if json_str(event, "status", "dev watch event")? != "ok" {
            anyhow::bail!("dev watch event status must be ok");
        }
        if json_str(event, "watch", "dev watch event")? != "dev/watch.json" {
            anyhow::bail!("dev watch event watch must be dev/watch.json");
        }
    }
    Ok(())
}

pub(crate) fn bundle_plan_has_client_target(plan: &serde_json::Value) -> anyhow::Result<bool> {
    let bundles = plan
        .get("bundles")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("bundle plan bundles must be an array"))?;
    Ok(bundles.iter().any(|target| {
        target
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_client_bundle_kind)
    }))
}

pub(crate) fn verify_dev_watch_set(
    dir: &Path,
    plan: &serde_json::Value,
    session: &serde_json::Value,
    context: &str,
) -> anyhow::Result<()> {
    let watch = session
        .get("watch")
        .ok_or_else(|| anyhow::anyhow!("{context} watch must be an object"))?;
    let session_sources = watch
        .get("sources")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("{context} watch.sources must be an array"))?;
    let session_targets = watch
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("{context} watch.targets must be an array"))?;
    let source_bundle = read_json_value(&dir.join("source-bundle.json"))?;
    let expected_sources = source_bundle
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("source-bundle.json files must be an array"))?;
    for source in expected_sources {
        let path = json_str(source, "path", "source bundle file")?;
        let content_hash = json_str(source, "content_hash", "source bundle file")?;
        if !session_sources.iter().any(|session_source| {
            session_source
                .get("path")
                .and_then(serde_json::Value::as_str)
                == Some(path)
                && session_source
                    .get("content_hash")
                    .and_then(serde_json::Value::as_str)
                    == Some(content_hash)
        }) {
            anyhow::bail!("{context} missing source {path}");
        }
    }
    let bundles = plan
        .get("bundles")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("bundle plan bundles must be an array"))?;
    for bundle in bundles {
        let kind = json_str(bundle, "kind", "bundle target")?;
        let path = json_str(bundle, "path", "bundle target")?;
        if !session_targets.iter().any(|session_target| {
            session_target
                .get("kind")
                .and_then(serde_json::Value::as_str)
                == Some(kind)
                && session_target
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    == Some(path)
        }) {
            anyhow::bail!("{context} missing bundle target {kind}:{path}");
        }
    }
    Ok(())
}

pub(crate) fn client_wasm_exports_function(bytes: &[u8], name: &str) -> anyhow::Result<bool> {
    Ok(client_wasm_export_index(bytes, name, 0)?.is_some())
}

pub(crate) fn client_wasm_export_function_index(
    bytes: &[u8],
    name: &str,
) -> anyhow::Result<Option<u32>> {
    client_wasm_export_index(bytes, name, 0)
}

pub(crate) fn client_wasm_export_index(
    bytes: &[u8],
    name: &str,
    expected_kind: u8,
) -> anyhow::Result<Option<u32>> {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;
        let section_len = read_wasm_u32_leb(bytes, &mut offset, bytes.len())? as usize;
        let section_end = offset
            .checked_add(section_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid WASM section length"))?;
        if section_end > bytes.len() {
            anyhow::bail!("client_wasm bundle has invalid WASM section length");
        }
        if section_id == 7 {
            return wasm_export_section_index(bytes, offset, section_end, name, expected_kind);
        }
        offset = section_end;
    }
    Ok(None)
}

pub(crate) fn wasm_export_section_index(
    bytes: &[u8],
    mut offset: usize,
    section_end: usize,
    name: &str,
    expected_kind: u8,
) -> anyhow::Result<Option<u32>> {
    let export_count = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
    for _ in 0..export_count {
        let name_len = read_wasm_u32_leb(bytes, &mut offset, section_end)? as usize;
        let name_end = offset
            .checked_add(name_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid export name"))?;
        if name_end > section_end {
            anyhow::bail!("client_wasm bundle has invalid export name");
        }
        let export_name_matches = &bytes[offset..name_end] == name.as_bytes();
        offset = name_end;
        if offset >= section_end {
            anyhow::bail!("client_wasm bundle has truncated export descriptor");
        }
        let kind = bytes[offset];
        offset += 1;
        let index = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
        if export_name_matches && kind == expected_kind {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

pub(crate) fn client_wasm_custom_section_payload(bytes: &[u8]) -> anyhow::Result<Option<&[u8]>> {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;
        let section_len = read_wasm_u32_leb(bytes, &mut offset, bytes.len())? as usize;
        let section_end = offset
            .checked_add(section_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid WASM section length"))?;
        if section_end > bytes.len() {
            anyhow::bail!("client_wasm bundle has invalid WASM section length");
        }
        if section_id == 0 {
            let mut section_offset = offset;
            let name_len = read_wasm_u32_leb(bytes, &mut section_offset, section_end)? as usize;
            let name_end = section_offset.checked_add(name_len).ok_or_else(|| {
                anyhow::anyhow!("client_wasm bundle has invalid custom section name")
            })?;
            if name_end > section_end {
                anyhow::bail!("client_wasm bundle has invalid custom section name");
            }
            if &bytes[section_offset..name_end] == CLIENT_WASM_CUSTOM_SECTION_NAME.as_bytes() {
                return Ok(Some(&bytes[name_end..section_end]));
            }
        }
        offset = section_end;
    }
    Ok(None)
}

pub(crate) fn verify_client_wasm_initial_render_data(
    bytes: &[u8],
    initial_render: &serde_json::Value,
) -> anyhow::Result<()> {
    let expected_len = initial_render
        .get("byte_length")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("client_wasm initial_render byte_length is required"))?;
    let expected_hash = initial_render
        .get("html_hash")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("client_wasm initial_render html_hash is required"))?;
    let data = client_wasm_initial_render_data(bytes)?.unwrap_or(&[]);
    let actual_len = u64::try_from(data.len())
        .map_err(|_| anyhow::anyhow!("client_wasm initial_render byte_length is invalid"))?;
    if actual_len != expected_len {
        anyhow::bail!("client_wasm initial_render byte_length mismatch");
    }
    let actual_hash = format!("{:016x}", fnv1a64(data));
    if actual_hash != expected_hash {
        anyhow::bail!("client_wasm initial_render html_hash mismatch");
    }
    let expected_len_i32 = i32::try_from(expected_len)
        .map_err(|_| anyhow::anyhow!("client_wasm initial_render byte_length exceeds wasm i32"))?;
    let ptr = client_wasm_exported_i32_const(bytes, CLIENT_WASM_RENDER_PTR_EXPORT)?
        .ok_or_else(|| anyhow::anyhow!("client_wasm orv_render_ptr export body is missing"))?;
    if ptr != 0 {
        anyhow::bail!("client_wasm orv_render_ptr export must return initial render pointer");
    }
    let len = client_wasm_exported_i32_const(bytes, CLIENT_WASM_RENDER_LEN_EXPORT)?
        .ok_or_else(|| anyhow::anyhow!("client_wasm orv_render_len export body is missing"))?;
    if len != expected_len_i32 {
        anyhow::bail!("client_wasm orv_render_len export must return initial render byte_length");
    }
    Ok(())
}

pub(crate) fn client_wasm_initial_render_data(bytes: &[u8]) -> anyhow::Result<Option<&[u8]>> {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;
        let section_len = read_wasm_u32_leb(bytes, &mut offset, bytes.len())? as usize;
        let section_end = offset
            .checked_add(section_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid WASM section length"))?;
        if section_end > bytes.len() {
            anyhow::bail!("client_wasm bundle has invalid WASM section length");
        }
        if section_id == 11 {
            return wasm_initial_render_data_section(bytes, offset, section_end);
        }
        offset = section_end;
    }
    Ok(None)
}

pub(crate) fn wasm_initial_render_data_section(
    bytes: &[u8],
    mut offset: usize,
    section_end: usize,
) -> anyhow::Result<Option<&[u8]>> {
    let data_count = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
    for _ in 0..data_count {
        let flags = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
        if flags != 0 {
            anyhow::bail!("client_wasm initial_render data segment must target memory 0");
        }
        if offset >= section_end || bytes[offset] != 0x41 {
            anyhow::bail!("client_wasm initial_render data segment must use i32.const offset");
        }
        offset += 1;
        let memory_offset = read_wasm_i32_leb(bytes, &mut offset, section_end)?;
        if offset >= section_end || bytes[offset] != 0x0b {
            anyhow::bail!("client_wasm initial_render data segment offset is invalid");
        }
        offset += 1;
        let data_len = read_wasm_u32_leb(bytes, &mut offset, section_end)? as usize;
        let data_end = offset
            .checked_add(data_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm initial_render data segment is invalid"))?;
        if data_end > section_end {
            anyhow::bail!("client_wasm initial_render data segment is truncated");
        }
        if memory_offset == 0 {
            return Ok(Some(&bytes[offset..data_end]));
        }
        offset = data_end;
    }
    Ok(None)
}

pub(crate) fn client_wasm_exported_i32_const(
    bytes: &[u8],
    name: &str,
) -> anyhow::Result<Option<i32>> {
    let Some(function_index) = client_wasm_export_function_index(bytes, name)? else {
        return Ok(None);
    };
    let imported_function_count = client_wasm_imported_function_count(bytes)?;
    if function_index < imported_function_count {
        anyhow::bail!("client_wasm {name} export must not point at an imported function");
    }
    let code_index = function_index - imported_function_count;
    client_wasm_code_function_i32_const(bytes, code_index)
}

pub(crate) fn client_wasm_imported_function_count(bytes: &[u8]) -> anyhow::Result<u32> {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;
        let section_len = read_wasm_u32_leb(bytes, &mut offset, bytes.len())? as usize;
        let section_end = offset
            .checked_add(section_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid WASM section length"))?;
        if section_end > bytes.len() {
            anyhow::bail!("client_wasm bundle has invalid WASM section length");
        }
        if section_id == 2 {
            anyhow::bail!("client_wasm render exports must not depend on imported functions");
        }
        offset = section_end;
    }
    Ok(0)
}

pub(crate) fn client_wasm_code_function_i32_const(
    bytes: &[u8],
    target_index: u32,
) -> anyhow::Result<Option<i32>> {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;
        let section_len = read_wasm_u32_leb(bytes, &mut offset, bytes.len())? as usize;
        let section_end = offset
            .checked_add(section_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm bundle has invalid WASM section length"))?;
        if section_end > bytes.len() {
            anyhow::bail!("client_wasm bundle has invalid WASM section length");
        }
        if section_id == 10 {
            return wasm_code_section_i32_const(bytes, offset, section_end, target_index);
        }
        offset = section_end;
    }
    Ok(None)
}

pub(crate) fn wasm_code_section_i32_const(
    bytes: &[u8],
    mut offset: usize,
    section_end: usize,
    target_index: u32,
) -> anyhow::Result<Option<i32>> {
    let function_count = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
    for index in 0..function_count {
        let body_len = read_wasm_u32_leb(bytes, &mut offset, section_end)? as usize;
        let body_end = offset
            .checked_add(body_len)
            .ok_or_else(|| anyhow::anyhow!("client_wasm function body length is invalid"))?;
        if body_end > section_end {
            anyhow::bail!("client_wasm function body is truncated");
        }
        if index == target_index {
            return wasm_i32_const_body(bytes, offset, body_end).map(Some);
        }
        offset = body_end;
    }
    Ok(None)
}

pub(crate) fn wasm_i32_const_body(
    bytes: &[u8],
    mut offset: usize,
    body_end: usize,
) -> anyhow::Result<i32> {
    let local_decl_count = read_wasm_u32_leb(bytes, &mut offset, body_end)?;
    if local_decl_count != 0 {
        anyhow::bail!("client_wasm render export body must not declare locals");
    }
    if offset >= body_end || bytes[offset] != 0x41 {
        anyhow::bail!("client_wasm render export body must return i32.const");
    }
    offset += 1;
    let value = read_wasm_i32_leb(bytes, &mut offset, body_end)?;
    if offset >= body_end || bytes[offset] != 0x0b {
        anyhow::bail!("client_wasm render export body must end after i32.const");
    }
    offset += 1;
    if offset != body_end {
        anyhow::bail!("client_wasm render export body has trailing instructions");
    }
    Ok(value)
}

pub(crate) fn read_wasm_u32_leb(
    bytes: &[u8],
    offset: &mut usize,
    limit: usize,
) -> anyhow::Result<u32> {
    let mut value = 0u32;
    let mut shift = 0;
    for _ in 0..5 {
        if *offset >= limit {
            anyhow::bail!("client_wasm bundle has truncated LEB128 length");
        }
        let byte = bytes[*offset];
        *offset += 1;
        if shift == 28 && (byte & 0xf0) != 0 {
            anyhow::bail!("client_wasm bundle has invalid u32 LEB128 length");
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    anyhow::bail!("client_wasm bundle has invalid u32 LEB128 length")
}

pub(crate) fn read_wasm_i32_leb(
    bytes: &[u8],
    offset: &mut usize,
    limit: usize,
) -> anyhow::Result<i32> {
    let mut value = 0i32;
    let mut shift = 0;
    for _ in 0..5 {
        if *offset >= limit {
            anyhow::bail!("client_wasm bundle has truncated i32 LEB128");
        }
        let byte = bytes[*offset];
        *offset += 1;
        value |= i32::from(byte & 0x7f) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 32 && (byte & 0x40) != 0 {
                value |= !0 << shift;
            }
            return Ok(value);
        }
    }
    anyhow::bail!("client_wasm bundle has invalid i32 LEB128")
}

pub(crate) fn verify_deploy_manifest_if_present(
    dir: &Path,
    origin_map: &orv_compiler::OriginMap,
    source_bundle: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<()> {
    let deploy_manifest = dir.join("deploy").join("manifest.json");
    if !deploy_manifest.is_file() {
        return Ok(());
    }
    let deploy = read_json_value(&deploy_manifest)?;
    let version = deploy
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("deploy manifest schema_version must be an integer"))?;
    if version != 1 {
        anyhow::bail!("unsupported deploy manifest schema_version {version}");
    }
    if deploy.get("profile").and_then(serde_json::Value::as_str) != Some("prod") {
        anyhow::bail!("deploy manifest profile must be prod");
    }
    verify_deploy_source_bundle(dir, deploy.get("source_bundle"), source_bundle)?;
    verify_deploy_server_target(
        dir,
        deploy.get("server"),
        deploy.get("client"),
        origin_map,
        source_bundle,
    )?;
    verify_deploy_static_target(dir, deploy.get("static"))?;
    verify_deploy_client_target(dir, deploy.get("client"))
}

pub(crate) fn verify_deploy_source_bundle(
    dir: &Path,
    source_bundle: Option<&serde_json::Value>,
    expected: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<()> {
    let Some(path) = source_bundle.and_then(serde_json::Value::as_str) else {
        anyhow::bail!("deploy manifest source_bundle must be a string");
    };
    let target = dir.join(path);
    if !target.is_file() {
        anyhow::bail!("missing deploy source bundle: {}", target.display());
    }
    let artifact = read_source_bundle_artifact(&target)?;
    if &artifact != expected {
        anyhow::bail!("deploy manifest source_bundle does not match build source-bundle artifact");
    }
    Ok(())
}

pub(crate) fn verify_deploy_server_target(
    dir: &Path,
    server: Option<&serde_json::Value>,
    client: Option<&serde_json::Value>,
    origin_map: &orv_compiler::OriginMap,
    source_bundle: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<()> {
    let Some(server) = server.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let artifact_path = json_str(server, "artifact", "deploy server")?;
    let entrypoint = json_str(server, "entrypoint", "deploy server")?;
    let routes_artifact = json_str(server, "routes_artifact", "deploy server")?;
    let native_plan = json_str(server, "native_plan", "deploy server")?;
    let native_runtime_image_plan = json_str(server, "native_runtime_image_plan", "deploy server")?;
    let native_route_table_source = json_str(server, "native_routes_source", "deploy server")?;
    let native_dispatch_source = json_str(server, "native_router_source", "deploy server")?;
    let native_handlers_source = json_str(server, "native_handlers_source", "deploy server")?;
    let container = json_str(server, "container", "deploy server")?;
    let dockerfile = json_str(server, "dockerfile", "deploy server")?;
    let compose = json_str(server, "compose", "deploy server")?;
    let env_example = json_str(server, "env_example", "deploy server")?;
    let db_adapters = json_str(server, "db_adapters", "deploy server")?;
    let commerce_adapters = json_str(server, "commerce_adapters", "deploy server")?;
    let smoke_test = json_str(server, "smoke_test", "deploy server")?;
    if smoke_test != DEPLOY_SMOKE_TEST_PATH {
        anyhow::bail!("deploy server smoke_test must be {DEPLOY_SMOKE_TEST_PATH}");
    }
    let smoke_output = json_str(server, "smoke_output", "deploy server")?;
    if smoke_output != DEPLOY_SMOKE_OUTPUT_PATH {
        anyhow::bail!("deploy server smoke_output must be {DEPLOY_SMOKE_OUTPUT_PATH}");
    }
    let preflight = json_str(server, "preflight", "deploy server")?;
    if preflight != DEPLOY_PREFLIGHT_PATH {
        anyhow::bail!("deploy server preflight must be {DEPLOY_PREFLIGHT_PATH}");
    }
    let benchmark_evidence = json_str(server, "benchmark_evidence", "deploy server")?;
    if benchmark_evidence != DEPLOY_BENCHMARK_EVIDENCE_PATH {
        anyhow::bail!("deploy server benchmark_evidence must be {DEPLOY_BENCHMARK_EVIDENCE_PATH}");
    }
    let runbook = json_str(server, "runbook", "deploy server")?;
    let runtime_image = json_str(server, "runtime_image", "deploy server")?;
    if runtime_image != ORV_REFERENCE_RUNTIME_IMAGE {
        anyhow::bail!("deploy server runtime_image must be {ORV_REFERENCE_RUNTIME_IMAGE}");
    }
    verify_deploy_server_entrypoint(dir, entrypoint)?;
    let artifact = read_server_artifact(&dir.join(artifact_path))?;
    orv_compiler::verify_server_runtime_artifact(&artifact)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    verify_server_runtime_origin_contract(&artifact, origin_map)?;
    verify_server_runtime_source_bundle_contract(&artifact, source_bundle)?;
    validate_prod_server_listen(Some(&artifact))?;
    let persistence = server_artifact_deploy_persistence(&artifact)?;
    verify_deploy_routes_artifact(
        dir,
        routes_artifact,
        artifact_path,
        artifact.runtime.as_str(),
        &artifact,
    )?;
    verify_native_server_plan_artifact(
        dir,
        native_plan,
        artifact_path,
        SERVER_LAUNCH_PATH,
        &artifact,
    )?;
    verify_native_runtime_image_plan_artifact(
        dir,
        native_runtime_image_plan,
        artifact_path,
        native_plan,
        &artifact,
    )?;
    if native_route_table_source != NATIVE_SERVER_ROUTES_SOURCE_PATH {
        anyhow::bail!(
            "deploy server native_routes_source must be {NATIVE_SERVER_ROUTES_SOURCE_PATH}"
        );
    }
    verify_native_server_routes_source(&dir.join(native_route_table_source), &artifact)?;
    if native_dispatch_source != NATIVE_SERVER_ROUTER_SOURCE_PATH {
        anyhow::bail!(
            "deploy server native_router_source must be {NATIVE_SERVER_ROUTER_SOURCE_PATH}"
        );
    }
    verify_native_server_router_source(&dir.join(native_dispatch_source))?;
    if native_handlers_source != NATIVE_SERVER_HANDLERS_SOURCE_PATH {
        anyhow::bail!(
            "deploy server native_handlers_source must be {NATIVE_SERVER_HANDLERS_SOURCE_PATH}"
        );
    }
    verify_native_server_handlers_source(&dir.join(native_handlers_source), &artifact)?;
    verify_deploy_container_artifact(
        dir,
        container,
        dockerfile,
        &DeployServerContract {
            artifact_path,
            entrypoint,
            routes_artifact,
            runtime: artifact.runtime.as_str(),
            runtime_image,
            listen: artifact.listen.as_ref(),
        },
        &persistence,
    )?;
    verify_deploy_compose_artifact(
        dir,
        compose,
        dockerfile,
        runtime_image,
        artifact.listen.as_ref(),
        &persistence,
    )?;
    verify_deploy_env_example_artifact(dir, env_example, artifact.listen.as_ref(), &persistence)?;
    verify_deploy_db_adapters_artifact(dir, db_adapters, artifact_path, &persistence, origin_map)?;
    verify_deploy_commerce_adapters_artifact(
        dir,
        commerce_adapters,
        artifact_path,
        &persistence,
        origin_map,
    )?;
    verify_deploy_smoke_test_artifact(
        dir,
        smoke_test,
        artifact.listen.as_ref(),
        &artifact,
        &persistence,
        client,
    )?;
    let deploy_artifacts = DeployRunbookArtifacts {
        server_artifact: artifact_path,
        compose,
        env_example,
        db_adapters,
        commerce_adapters,
        smoke_test,
        smoke_output,
        preflight,
        benchmark_evidence,
        runbook,
        routes: routes_artifact,
    };
    verify_deploy_preflight_artifact(
        dir,
        preflight,
        &deploy_artifacts,
        &artifact,
        &persistence,
        client,
    )?;
    verify_deploy_benchmark_evidence_artifact(
        dir,
        benchmark_evidence,
        &deploy_artifacts,
        &artifact,
        &persistence,
        client,
    )?;
    verify_deploy_runbook_artifact(
        dir,
        runbook,
        &deploy_artifacts,
        &artifact,
        &persistence,
        client,
    )?;
    if server.get("runtime").and_then(serde_json::Value::as_str) != Some(artifact.runtime.as_str())
    {
        anyhow::bail!("deploy server runtime does not match runtime artifact");
    }
    if server.get("runtime_features") != Some(&serde_json::to_value(&artifact.runtime_features)?) {
        anyhow::bail!("deploy server runtime_features do not match runtime artifact");
    }
    verify_deploy_listen_value(
        server.get("listen"),
        artifact.listen.as_ref(),
        "deploy server",
    )?;
    if let Some(routes) = server.get("routes") {
        let artifact_routes = serde_json::to_value(&artifact.routes)?;
        if routes != &artifact_routes {
            anyhow::bail!("deploy server routes do not match runtime artifact");
        }
    }
    if server.get("persistence") != Some(&deploy_persistence_value(&persistence)) {
        anyhow::bail!("deploy server persistence does not match runtime artifact");
    }
    Ok(())
}

pub(crate) fn verify_deploy_server_entrypoint(dir: &Path, entrypoint: &str) -> anyhow::Result<()> {
    let entrypoint_path = dir.join(entrypoint);
    if !entrypoint_path.is_file() {
        anyhow::bail!(
            "missing deploy server entrypoint: {}",
            entrypoint_path.display()
        );
    }
    let script = std::fs::read_to_string(&entrypoint_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", entrypoint_path.display()))?;
    if !script.contains("orv run-artifact") {
        anyhow::bail!("deploy server entrypoint must run `orv run-artifact`");
    }
    Ok(())
}

pub(crate) fn verify_deploy_container_artifact(
    dir: &Path,
    path: &str,
    dockerfile_path: &str,
    contract: &DeployServerContract<'_>,
    persistence: &DeployPersistence,
) -> anyhow::Result<()> {
    let container_path = dir.join(path);
    if !container_path.is_file() {
        anyhow::bail!(
            "missing deploy container artifact: {}",
            container_path.display()
        );
    }
    let container = read_json_value(&container_path)?;
    if container
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("deploy container schema_version must be 1");
    }
    if json_str(&container, "kind", "deploy container")? != "reference-server-container" {
        anyhow::bail!("deploy container kind must be reference-server-container");
    }
    if json_str(&container, "artifact", "deploy container")? != contract.artifact_path {
        let artifact_path = contract.artifact_path;
        anyhow::bail!("deploy container artifact must be {artifact_path}");
    }
    if json_str(&container, "entrypoint", "deploy container")? != contract.entrypoint {
        let entrypoint = contract.entrypoint;
        anyhow::bail!("deploy container entrypoint must be {entrypoint}");
    }
    if json_str(&container, "routes_artifact", "deploy container")? != contract.routes_artifact {
        let routes_artifact = contract.routes_artifact;
        anyhow::bail!("deploy container routes_artifact must be {routes_artifact}");
    }
    if json_str(&container, "dockerfile", "deploy container")? != dockerfile_path {
        anyhow::bail!("deploy container dockerfile must be {dockerfile_path}");
    }
    if json_str(&container, "runtime", "deploy container")? != contract.runtime {
        anyhow::bail!("deploy container runtime does not match runtime artifact");
    }
    if json_str(&container, "runtime_image", "deploy container")? != contract.runtime_image {
        let runtime_image = contract.runtime_image;
        anyhow::bail!("deploy container runtime_image must be {runtime_image}");
    }
    if json_str(&container, "protocol", "deploy container")? != "http1" {
        anyhow::bail!("deploy container protocol must be http1");
    }
    verify_deploy_listen_value(container.get("listen"), contract.listen, "deploy container")?;
    if container.get("ports") != Some(&deploy_ports_value(contract.listen)) {
        anyhow::bail!("deploy container ports do not match runtime artifact");
    }
    let command = container
        .get("command")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("deploy container command must be an array"))?;
    if command.first().and_then(serde_json::Value::as_str) != Some("./deploy/server.sh") {
        anyhow::bail!("deploy container command must start with ./deploy/server.sh");
    }
    if container.get("persistence") != Some(&deploy_persistence_value(persistence)) {
        anyhow::bail!("deploy container persistence does not match runtime artifact");
    }
    verify_deploy_dockerfile(
        dir,
        dockerfile_path,
        contract.runtime_image,
        contract.listen,
    )
}

pub(crate) fn verify_deploy_compose_artifact(
    dir: &Path,
    path: &str,
    dockerfile_path: &str,
    runtime_image: &str,
    listen: Option<&orv_compiler::ServerListenArtifact>,
    persistence: &DeployPersistence,
) -> anyhow::Result<()> {
    let compose_path = dir.join(path);
    if !compose_path.is_file() {
        anyhow::bail!("missing deploy compose file: {}", compose_path.display());
    }
    let compose = std::fs::read_to_string(&compose_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", compose_path.display()))?;
    let dockerfile_line = format!("dockerfile: {dockerfile_path}");
    if !compose.contains(&dockerfile_line) {
        anyhow::bail!("deploy compose must use {dockerfile_path}");
    }
    let runtime_image_line = format!("ORV_RUNTIME_IMAGE: {runtime_image}");
    if !compose.contains(&runtime_image_line) {
        anyhow::bail!("deploy compose must set ORV_RUNTIME_IMAGE");
    }
    if let Some(port) = deploy_compose_port(listen) {
        if !compose.contains(&port.binding) {
            let display = port.display;
            anyhow::bail!("deploy compose must publish {display}");
        }
    }
    for environment in deploy_compose_environment_lines(listen, persistence) {
        if !compose.contains(&environment) {
            anyhow::bail!("deploy compose must configure {environment}");
        }
    }
    for volume in &persistence.volumes {
        if !compose.contains(&volume.compose_mount) {
            let mount = &volume.compose_mount;
            anyhow::bail!("deploy compose must mount persistent volume {mount}");
        }
    }
    Ok(())
}

pub(crate) fn verify_deploy_env_example_artifact(
    dir: &Path,
    path: &str,
    listen: Option<&orv_compiler::ServerListenArtifact>,
    persistence: &DeployPersistence,
) -> anyhow::Result<()> {
    let env_path = dir.join(path);
    if !env_path.is_file() {
        anyhow::bail!("missing deploy env example: {}", env_path.display());
    }
    let env_example = std::fs::read_to_string(&env_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", env_path.display()))?;
    for assignment in deploy_env_example_assignments(listen, persistence) {
        if !env_example.contains(&assignment) {
            anyhow::bail!("deploy env example must include {assignment}");
        }
    }
    Ok(())
}

pub(crate) fn verify_deploy_commerce_adapters_artifact(
    dir: &Path,
    path: &str,
    artifact_path: &str,
    persistence: &DeployPersistence,
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<()> {
    let adapters_path = dir.join(path);
    if !adapters_path.is_file() {
        anyhow::bail!(
            "missing deploy commerce adapters artifact: {}",
            adapters_path.display()
        );
    }
    let adapters = read_json_value(&adapters_path)?;
    if adapters
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("deploy commerce adapters schema_version must be 1");
    }
    if json_str(&adapters, "kind", "deploy commerce adapters")? != "orv.deploy.commerce_adapters" {
        anyhow::bail!("deploy commerce adapters kind must be orv.deploy.commerce_adapters");
    }
    if json_str(&adapters, "artifact", "deploy commerce adapters")? != artifact_path {
        anyhow::bail!("deploy commerce adapters artifact must be {artifact_path}");
    }
    if adapters.get("adapters")
        != Some(&serde_json::Value::Array(deploy_commerce_adapter_value(
            &persistence.commerce_adapters,
        )))
    {
        anyhow::bail!("deploy commerce adapters do not match runtime artifact persistence");
    }
    verify_deploy_commerce_adapter_source_origins(origin_map, &persistence.commerce_adapters)?;
    Ok(())
}

pub(crate) fn verify_deploy_db_adapters_artifact(
    dir: &Path,
    path: &str,
    artifact_path: &str,
    persistence: &DeployPersistence,
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<()> {
    let adapters_path = dir.join(path);
    if !adapters_path.is_file() {
        anyhow::bail!(
            "missing deploy DB adapters artifact: {}",
            adapters_path.display()
        );
    }
    let adapters = read_json_value(&adapters_path)?;
    if adapters
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("deploy DB adapters schema_version must be 1");
    }
    if json_str(&adapters, "kind", "deploy DB adapters")? != "orv.deploy.db_adapters" {
        anyhow::bail!("deploy DB adapters kind must be orv.deploy.db_adapters");
    }
    if json_str(&adapters, "artifact", "deploy DB adapters")? != artifact_path {
        anyhow::bail!("deploy DB adapters artifact must be {artifact_path}");
    }
    if adapters.get("adapters")
        != Some(&serde_json::Value::Array(deploy_db_adapter_value(
            &persistence.db_adapters,
        )))
    {
        anyhow::bail!("deploy DB adapters do not match runtime artifact persistence");
    }
    verify_deploy_db_adapter_source_origins(origin_map, &persistence.db_adapters)?;
    Ok(())
}

pub(crate) fn verify_deploy_db_adapter_source_origins(
    origin_map: &orv_compiler::OriginMap,
    adapters: &[DeployDbAdapter],
) -> anyhow::Result<()> {
    let entries_by_id = origin_entries_by_id(origin_map);
    for adapter in adapters {
        if adapter.source_origin_ids.is_empty() {
            let provider = &adapter.provider;
            anyhow::bail!("deploy DB adapter {provider} is missing source_origin_ids");
        }
        for origin_id in &adapter.source_origin_ids {
            verify_deploy_adapter_source_origin(
                &entries_by_id,
                origin_id,
                "deploy DB adapter",
                "@db.connect",
            )?;
        }
    }
    Ok(())
}

pub(crate) fn verify_deploy_commerce_adapter_source_origins(
    origin_map: &orv_compiler::OriginMap,
    adapters: &[DeployCommerceAdapter],
) -> anyhow::Result<()> {
    let entries_by_id = origin_entries_by_id(origin_map);
    for adapter in adapters {
        if adapter.source_origin_ids.is_empty() {
            let kind = &adapter.kind;
            anyhow::bail!("deploy commerce adapter {kind} is missing source_origin_ids");
        }
        let expected_call = match adapter.kind.as_str() {
            "payment" => "@payment.connect",
            "shipping" => "@shipping.connect",
            kind => {
                anyhow::bail!("deploy commerce adapter {kind} has unknown source kind");
            }
        };
        let context = format!("deploy commerce adapter {}", adapter.kind);
        for origin_id in &adapter.source_origin_ids {
            verify_deploy_adapter_source_origin(
                &entries_by_id,
                origin_id,
                &context,
                expected_call,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn origin_entries_by_id(
    origin_map: &orv_compiler::OriginMap,
) -> HashMap<&str, &orv_compiler::OriginEntry> {
    origin_map
        .entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect()
}

pub(crate) fn verify_deploy_adapter_source_origin(
    entries_by_id: &HashMap<&str, &orv_compiler::OriginEntry>,
    origin_id: &str,
    context: &str,
    expected_call: &str,
) -> anyhow::Result<()> {
    let Some(entry) = entries_by_id.get(origin_id).copied() else {
        anyhow::bail!("{context} source_origin_id `{origin_id}` not found in origin-map.json");
    };
    if entry.kind != "call" || entry.name != expected_call {
        anyhow::bail!(
            "{context} source_origin_id `{origin_id}` must reference origin-map call {expected_call}"
        );
    }
    Ok(())
}

pub(crate) fn verify_deploy_smoke_test_artifact(
    dir: &Path,
    path: &str,
    listen: Option<&orv_compiler::ServerListenArtifact>,
    artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
    client: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let smoke_path = dir.join(path);
    if !smoke_path.is_file() {
        anyhow::bail!("missing deploy smoke test: {}", smoke_path.display());
    }
    verify_executable_if_supported(&smoke_path, "deploy smoke test")?;
    verify_shell_syntax_if_supported(&smoke_path, "deploy smoke test")?;
    let smoke = std::fs::read_to_string(&smoke_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", smoke_path.display()))?;
    let base_url = deploy_smoke_base_url(listen);
    let base_assignment = format!(r#"BASE_URL="${{ORV_BASE_URL:-{base_url}}}""#);
    if !smoke.contains(&base_assignment) {
        anyhow::bail!("deploy smoke test must include {base_assignment}");
    }
    if !smoke.contains("command -v curl") || !smoke.contains("orv deploy smoke test requires curl")
    {
        anyhow::bail!("deploy smoke test must check curl availability");
    }
    if !smoke.contains(r#"ORV_SMOKE_OUTPUT="${ORV_SMOKE_OUTPUT:-deploy/smoke-output.txt}""#)
        || !smoke.contains(r#"> "$ORV_SMOKE_OUTPUT""#)
        || !smoke.contains("orv_smoke_write_output()")
        || !smoke.contains("\norv_smoke_write_output\n")
        || !smoke.contains("graph_contract=verified")
        || !smoke.contains("dap_summary=verified")
        || !smoke.contains("dap_source_bundle=verified")
        || !smoke.contains("server_routes=")
        || !smoke.contains("trace_stream_requested=%s")
    {
        anyhow::bail!("deploy smoke test must write deploy smoke output artifact");
    }
    if !smoke.contains(r#"ORV_BIN="${ORV_BIN:-orv}""#)
        || !smoke.contains("orv_smoke_reveal_contains()")
        || !smoke.contains("orv_smoke_editor_reveal_contains()")
        || !smoke.contains("orv_smoke_lsp_reveal_contains()")
        || !smoke.contains("orv_smoke_dap_summary_contains()")
        || !smoke.contains("editor reveal")
        || !smoke.contains("lsp reveal")
        || !smoke.contains("editor run-debug . --control next")
        || !smoke.contains("orv deploy smoke test requires orv")
    {
        anyhow::bail!(
            "deploy smoke test must verify source, editor, LSP, and DAP production surfaces with the ORV CLI"
        );
    }
    if !smoke.contains("orv_smoke_trace_stream()")
        || !smoke.contains("ORV_SMOKE_TRACE_STREAM")
        || !smoke.contains("editor trace-stream")
    {
        anyhow::bail!("deploy smoke test must optionally verify live trace stream");
    }
    if !smoke.contains("orv_smoke_graph_contract()")
        || !smoke.contains("\norv_smoke_graph_contract\n")
        || !smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap graph summary" '"graph_contract_count": 3'"#,
        )
        || !smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap source bundle summary" '"source_bundle_file_count": 1'"#,
        )
        || !smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap source bundle panel" '"source_bundle": {'"#,
        )
        || !smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap source bundle panel path" '"path": "./source-bundle.json"'"#,
        )
        || !smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap source bundle panel file count" '"fileCount": 1'"#,
        )
        || !smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap source bundle panel hash" '"hash":'"#,
        )
        || !smoke.contains(r#""$ORV_BIN" verify-build ."#)
        || !smoke.contains("source-bundle.json")
        || !smoke.contains("project-graph.json")
        || !smoke.contains("origin-map.json")
    {
        anyhow::bail!("deploy smoke test must verify the build graph contract");
    }
    if !smoke.contains("orv_smoke_dap_summary_capture()")
        || !smoke.contains("orv_smoke_dap_summary_cleanup()")
        || !smoke.contains("\norv_smoke_dap_summary_cleanup\n")
    {
        anyhow::bail!("deploy smoke test must cache and clean DAP production summary output");
    }
    if !smoke.contains(
        r#"orv_smoke_dap_summary_contains "dap smoke required markers" '"smoke_test_required_markers": ['"#,
    ) || !smoke.contains(
        r#"orv_smoke_dap_summary_contains "dap smoke summary required markers" '"required_markers": ['"#,
    ) || !smoke.contains(
        r#"orv_smoke_dap_summary_contains "dap smoke marker dap source bundle" '"dap_source_bundle"'"#,
    ) {
        anyhow::bail!(
            "deploy smoke test must verify smoke marker contract in DAP production context"
        );
    }
    if !smoke.contains("ORV_SMOKE_BUILD_DIR=") || !smoke.contains(r#"cd "$ORV_SMOKE_BUILD_DIR""#) {
        anyhow::bail!("deploy smoke test must run from its build directory");
    }
    if !smoke.contains("orv_smoke_curl()") || !smoke.contains("orv deploy smoke test failed: %s") {
        anyhow::bail!("deploy smoke test must label failed curl steps");
    }
    if !artifact.routes.is_empty()
        && (!smoke.contains("orv_smoke_origin_header()")
            || !smoke.contains("orv_smoke_curl_origin()")
            || !smoke.contains("expected_origin")
            || !smoke.contains("wrong x-orv-origin-id"))
    {
        anyhow::bail!("deploy smoke test must verify exact route origin headers");
    }
    let has_single_response_origin = artifact
        .routes
        .iter()
        .any(|route| deploy_smoke_unique_response_origin(route).is_some());
    if has_single_response_origin
        && (!smoke.contains("orv_smoke_response_origin_header()")
            || !smoke.contains("orv_smoke_curl_origin_response()")
            || !smoke.contains("expected_response_origin")
            || !smoke.contains("wrong x-orv-response-origin-id"))
    {
        anyhow::bail!("deploy smoke test must verify exact response origin headers");
    }
    for route in &artifact.routes {
        let assignment = format!(
            r#"{}="{}""#,
            deploy_smoke_origin_var_name(&route.method, &route.path),
            route.origin_id
        );
        if !smoke.contains(&assignment) {
            let method = &route.method;
            let path = &route.path;
            anyhow::bail!("deploy smoke test must declare expected origin for {method} {path}");
        }
        if let Some(response_origin_id) = deploy_smoke_unique_response_origin(route) {
            let assignment = format!(
                r#"{}="{}""#,
                deploy_smoke_response_origin_var_name(&route.method, &route.path),
                response_origin_id
            );
            if !smoke.contains(&assignment) {
                let method = &route.method;
                let path = &route.path;
                anyhow::bail!(
                    "deploy smoke test must declare expected response origin for {method} {path}"
                );
            }
        }
    }
    if !artifact.routes.is_empty()
        && (!smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap native target summary" '"native_server_target_count": 1'"#,
        ) || !smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap native route summary" '"native_server_route_count": 1'"#,
        ))
    {
        anyhow::bail!("deploy smoke test must check DAP native production summary counters");
    }
    if !artifact.routes.is_empty()
        && (!smoke.contains(r#"orv_smoke_reveal_contains "reveal smoke required markers" "#)
            || !smoke
                .contains(r#"orv_smoke_reveal_contains "reveal smoke summary required markers" "#)
            || !smoke
                .contains(r#"orv_smoke_reveal_contains "reveal smoke marker dap source bundle" "#)
            || !smoke.contains(
                r#"orv_smoke_editor_reveal_contains "editor reveal smoke required markers" "#,
            )
            || !smoke.contains(
                r#"orv_smoke_editor_reveal_contains "editor reveal smoke summary required markers" "#,
            )
            || !smoke.contains(
                r#"orv_smoke_editor_reveal_contains "editor reveal smoke marker dap source bundle" "#,
            )
            || !smoke.contains(r#"orv_smoke_lsp_reveal_contains "lsp reveal smoke required markers" "#)
            || !smoke.contains(
                r#"orv_smoke_lsp_reveal_contains "lsp reveal smoke summary required markers" "#,
            )
            || !smoke.contains(
                r#"orv_smoke_lsp_reveal_contains "lsp reveal smoke marker dap source bundle" "#,
            ))
    {
        anyhow::bail!("deploy smoke test must verify smoke marker contract across reveal surfaces");
    }
    if deploy_routes_include(artifact, "POST", "/checkout")
        && !smoke.contains("orv_smoke_cookie_from_headers()")
    {
        anyhow::bail!("deploy smoke test must extract cookies for protected shop routes");
    }
    if deploy_routes_include(artifact, "POST", "/checkout")
        && (!smoke.contains("orv_smoke_fetch()") || !smoke.contains("orv_smoke_body_contains()"))
    {
        anyhow::bail!("deploy smoke test must inspect shop response bodies");
    }
    verify_deploy_smoke_client_contract(&smoke, client)?;
    verify_deploy_smoke_db_adapter_contract(&smoke, persistence)?;
    if let Some(ready_path) = deploy_smoke_ready_path(artifact) {
        let ready_assignment = format!(r#"READY_PATH="{ready_path}""#);
        if !smoke.contains(&ready_assignment) {
            anyhow::bail!("deploy smoke test must include {ready_assignment}");
        }
        if !smoke.contains("for attempt in 1 2 3 4 5") || !smoke.contains("sleep 1") {
            anyhow::bail!("deploy smoke test must wait for server readiness");
        }
    }
    for route in artifact.routes.iter().filter(|route| {
        route.method == "GET"
            && !route.path.contains(':')
            && !route.path.starts_with("/admin")
            && route.path != "/account/sessions"
    }) {
        let origin_ref = deploy_smoke_origin_var_ref(&route.method, &route.path);
        let command = if deploy_smoke_unique_response_origin(route).is_some() {
            let response_origin_ref =
                deploy_smoke_response_origin_var_ref(&route.method, &route.path);
            format!(
                r#"orv_smoke_curl_origin_response "GET {}" "{}" "{}" "$BASE_URL{}""#,
                route.path, origin_ref, response_origin_ref, route.path
            )
        } else {
            format!(
                r#"orv_smoke_curl_origin "GET {}" "{}" "$BASE_URL{}""#,
                route.path, origin_ref, route.path
            )
        };
        if !smoke.contains(&command) {
            let method = &route.method;
            let path = &route.path;
            anyhow::bail!("deploy smoke test must cover {method} {path}");
        }
        if deploy_smoke_unique_response_origin(route).is_some() {
            let response_origin_ref =
                deploy_smoke_response_origin_var_ref(&route.method, &route.path);
            for required in [
                format!(
                    r#"orv_smoke_reveal_contains "reveal GET {} response source" "{}" '@respond'"#,
                    route.path, response_origin_ref
                ),
                format!(
                    r#"orv_smoke_reveal_contains "reveal GET {} response production" "{}" '"response_origin_dispatch": true'"#,
                    route.path, response_origin_ref
                ),
                format!(
                    r#"orv_smoke_editor_reveal_contains "editor reveal GET {} response source" "{}" '@respond'"#,
                    route.path, response_origin_ref
                ),
                format!(
                    r#"orv_smoke_editor_reveal_contains "editor reveal GET {} response production" "{}" '"response_origin_dispatch": true'"#,
                    route.path, response_origin_ref
                ),
                format!(
                    r#"orv_smoke_lsp_reveal_contains "lsp reveal GET {} response origin" "{}" '"name": "respond"'"#,
                    route.path, response_origin_ref
                ),
                format!(
                    r#"orv_smoke_lsp_reveal_contains "lsp reveal GET {} response production" "{}" '"response_origin_dispatch": true'"#,
                    route.path, response_origin_ref
                ),
            ] {
                if !smoke.contains(&required) {
                    let method = &route.method;
                    let path = &route.path;
                    anyhow::bail!(
                        "deploy smoke test must reveal response origin for {method} {path}"
                    );
                }
            }
        }
    }
    if deploy_routes_include(artifact, "POST", "/checkout") {
        for path in ["/products", "/members", "/cart/items"] {
            let origin_ref = deploy_smoke_origin_var_ref("POST", path);
            let command = format!(
                r#"orv_smoke_curl_origin "POST {path}" "{origin_ref}" -X POST "$BASE_URL{path}""#
            );
            if !smoke.contains(&command) {
                anyhow::bail!("deploy smoke test must cover POST {path}");
            }
        }
        let checkout_origin_ref = deploy_smoke_origin_var_ref("POST", "/checkout");
        let checkout_command = format!(
            r#"orv_smoke_fetch_origin "POST /checkout" "$SMOKE_CHECKOUT_BODY" "{checkout_origin_ref}" -X POST "$BASE_URL/checkout""#
        );
        if !smoke.contains(&checkout_command) {
            anyhow::bail!("deploy smoke test must cover POST /checkout with captured body");
        }
        if !smoke.contains(r#"SMOKE_SKU="orv-smoke-sku-${SMOKE_ID}""#) {
            anyhow::bail!("deploy smoke test must use unique smoke SKU");
        }
        if (!persistence.db_paths.is_empty() || !persistence.db_env.is_empty())
            && !smoke.contains(r#"ORV_SMOKE_DB_CONNECT_ORIGIN="ori_"#)
        {
            anyhow::bail!("deploy smoke test must declare a DB connect source origin");
        }
        if deploy_smoke_has_commerce_record(persistence, "payment", "data/payments.jsonl")
            && !smoke.contains(r#"ORV_SMOKE_PAYMENT_CONNECT_ORIGIN="ori_"#)
        {
            anyhow::bail!("deploy smoke test must declare a payment connect source origin");
        }
        if deploy_smoke_has_commerce_record(persistence, "shipping", "data/shipments.jsonl")
            && !smoke.contains(r#"ORV_SMOKE_SHIPPING_CONNECT_ORIGIN="ori_"#)
        {
            anyhow::bail!("deploy smoke test must declare a shipping connect source origin");
        }
        if !smoke.contains(r#"SMOKE_SKU_SECOND="orv-smoke-sku-${SMOKE_ID}-2""#)
            || !smoke.contains(r#"SMOKE_SKU_THIRD="orv-smoke-sku-${SMOKE_ID}-3""#)
        {
            anyhow::bail!("deploy smoke test must create three unique smoke SKUs");
        }
        if !smoke.contains(r#"SMOKE_HANDLE="orv-smoke-${SMOKE_ID}""#) {
            anyhow::bail!("deploy smoke test must use unique smoke member handle");
        }
        if !smoke.contains(
            "CSRF_COOKIE=\"$(orv_smoke_cookie_from_headers orv_csrf \"$SMOKE_HEADERS\")\"",
        ) || !smoke.contains(r#"-H "x-csrf-token: ${CSRF_TOKEN}""#)
        {
            anyhow::bail!("deploy smoke test must send reference CSRF cookie/token");
        }
        if deploy_routes_include(artifact, "GET", "/account/sessions") {
            let origin_ref = deploy_smoke_origin_var_ref("GET", "/account/sessions");
            let command = format!(
                r#"orv_smoke_curl_origin "GET /account/sessions" "{origin_ref}" -H "cookie: ${{MEMBER_SESSION_COOKIE}}" "$BASE_URL/account/sessions""#
            );
            if !smoke.contains(&command) {
                anyhow::bail!(
                    "deploy smoke test must cover GET /account/sessions with a session cookie"
                );
            }
        }
        for required in [
            r#"orv_smoke_body_contains "home title" "$SMOKE_HOME_BODY" 'Miol Shop'"#,
            r#"orv_smoke_body_contains "home copy" "$SMOKE_HOME_BODY" 'Catalog, member signup, payment capture, and shipment booking are ready.'"#,
            r#"orv_smoke_body_contains "home theme surface" "$SMOKE_HOME_BODY" 'background-color: #f8fafc'"#,
            r#"orv_smoke_body_contains "home theme typography" "$SMOKE_HOME_BODY" 'font-family: Inter, system-ui, sans-serif'"#,
            r#"orv_smoke_reveal_contains "reveal GET / source" "$ORV_SMOKE_ORIGIN_GET_ROOT" '@route GET /'"#,
            r#"orv_smoke_reveal_contains "reveal GET / production" "$ORV_SMOKE_ORIGIN_GET_ROOT" '"path": "/"'"#,
            r#"orv_smoke_editor_reveal_contains "editor reveal GET / source" "$ORV_SMOKE_ORIGIN_GET_ROOT" '@route GET /'"#,
            r#"orv_smoke_editor_reveal_contains "editor reveal GET / production" "$ORV_SMOKE_ORIGIN_GET_ROOT" '"path": "/"'"#,
            r#"orv_smoke_lsp_reveal_contains "lsp reveal GET / origin" "$ORV_SMOKE_ORIGIN_GET_ROOT" '"name": "GET /"'"#,
            r#"orv_smoke_lsp_reveal_contains "lsp reveal GET / production" "$ORV_SMOKE_ORIGIN_GET_ROOT" '"path": "/"'"#,
            r#"orv_smoke_body_contains "catalog smoke product" "$SMOKE_CATALOG_BODY" "$SMOKE_SKU""#,
            r#"orv_smoke_body_contains "catalog second smoke product" "$SMOKE_CATALOG_BODY" "$SMOKE_SKU_SECOND""#,
            r#"orv_smoke_body_contains "catalog third smoke product" "$SMOKE_CATALOG_BODY" "$SMOKE_SKU_THIRD""#,
            r#"orv_smoke_body_contains "cart smoke item" "$SMOKE_CART_BODY" "$SMOKE_SKU""#,
            r#"orv_smoke_body_contains "account smoke session" "$SMOKE_ACCOUNT_BODY" "$SMOKE_HANDLE""#,
            r#"orv_smoke_body_contains "checkout shipped order" "$SMOKE_CHECKOUT_BODY" '"status":"shipped"'"#,
            r#"orv_smoke_body_contains "checkout captured payment" "$SMOKE_CHECKOUT_BODY" '"status":"captured"'"#,
            r#"orv_smoke_body_contains "checkout shipment tracking" "$SMOKE_CHECKOUT_BODY" 'TRK-LOCAL'"#,
            r#"orv_smoke_body_contains "admin catalog smoke product" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_SKU""#,
            r#"orv_smoke_body_contains "admin catalog second smoke product" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_SKU_SECOND""#,
            r#"orv_smoke_body_contains "admin catalog third smoke product" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_SKU_THIRD""#,
            r#"orv_smoke_body_contains "admin orders shipped" "$SMOKE_ADMIN_ORDERS_BODY" 'shipped'"#,
            r#"orv_smoke_body_contains "admin payments captured" "$SMOKE_ADMIN_PAYMENTS_BODY" 'captured'"#,
            r#"orv_smoke_body_contains "admin shipments tracking" "$SMOKE_ADMIN_SHIPMENTS_BODY" 'TRK-LOCAL'"#,
            r#"orv_smoke_body_contains "admin audit checkout" "$SMOKE_ADMIN_AUDIT_BODY" 'checkout.complete'"#,
        ] {
            if !smoke.contains(required) {
                anyhow::bail!("deploy smoke test must include {required}");
            }
        }
        if !persistence.db_paths.is_empty() || !persistence.db_env.is_empty() {
            for required in [
                r#"orv_smoke_reveal_contains "reveal DB source" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '@db.connect'"#,
                r#"orv_smoke_reveal_contains "reveal DB preflight" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight"'"#,
                r#"orv_smoke_reveal_contains "reveal DB smoke summary" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"smoke_test_summary"'"#,
                r#"orv_smoke_reveal_contains "reveal DB smoke summary count" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight_smoke_summary_missing_count"'"#,
                r#"orv_smoke_reveal_contains "reveal DB sqlite path" "$ORV_SMOKE_DB_CONNECT_ORIGIN" 'sqlite://data/shop.sqlite'"#,
                r#"orv_smoke_editor_reveal_contains "editor reveal DB source" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '@db.connect'"#,
                r#"orv_smoke_editor_reveal_contains "editor reveal DB preflight" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight"'"#,
                r#"orv_smoke_editor_reveal_contains "editor reveal DB smoke summary" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"smoke_test_summary"'"#,
                r#"orv_smoke_editor_reveal_contains "editor reveal DB smoke summary count" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight_smoke_summary_missing_count"'"#,
                r#"orv_smoke_lsp_reveal_contains "lsp reveal DB origin" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '@db.connect'"#,
                r#"orv_smoke_lsp_reveal_contains "lsp reveal DB preflight" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight"'"#,
                r#"orv_smoke_lsp_reveal_contains "lsp reveal DB smoke summary" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"smoke_test_summary"'"#,
                r#"orv_smoke_lsp_reveal_contains "lsp reveal DB smoke summary count" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight_smoke_summary_missing_count"'"#,
            ] {
                if !smoke.contains(required) {
                    anyhow::bail!("deploy smoke test must include {required}");
                }
            }
        }
        if deploy_smoke_has_commerce_record(persistence, "payment", "data/payments.jsonl") {
            for required in [
                r#"orv_smoke_reveal_contains "reveal payment source" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '@payment.connect'"#,
                r#"orv_smoke_reveal_contains "reveal payment match" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '"matched": true'"#,
                r#"orv_smoke_reveal_contains "reveal payment record path" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" 'file://data/payments.jsonl'"#,
                r#"orv_smoke_reveal_contains "reveal payment request kind" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" 'payment.capture'"#,
                r#"orv_smoke_editor_reveal_contains "editor reveal payment source" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '@payment.connect'"#,
                r#"orv_smoke_editor_reveal_contains "editor reveal payment match" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '"matched": true'"#,
                r#"orv_smoke_lsp_reveal_contains "lsp reveal payment origin" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '@payment.connect'"#,
                r#"orv_smoke_lsp_reveal_contains "lsp reveal payment match" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '"matched": true'"#,
            ] {
                if !smoke.contains(required) {
                    anyhow::bail!("deploy smoke test must include {required}");
                }
            }
        }
        if deploy_smoke_has_commerce_record(persistence, "shipping", "data/shipments.jsonl") {
            for required in [
                r#"orv_smoke_reveal_contains "reveal shipping source" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '@shipping.connect'"#,
                r#"orv_smoke_reveal_contains "reveal shipping match" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '"matched": true'"#,
                r#"orv_smoke_reveal_contains "reveal shipping record path" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" 'file://data/shipments.jsonl'"#,
                r#"orv_smoke_reveal_contains "reveal shipping request kind" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" 'shipping.booking'"#,
                r#"orv_smoke_editor_reveal_contains "editor reveal shipping source" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '@shipping.connect'"#,
                r#"orv_smoke_editor_reveal_contains "editor reveal shipping match" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '"matched": true'"#,
                r#"orv_smoke_lsp_reveal_contains "lsp reveal shipping origin" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '@shipping.connect'"#,
                r#"orv_smoke_lsp_reveal_contains "lsp reveal shipping match" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '"matched": true'"#,
            ] {
                if !smoke.contains(required) {
                    anyhow::bail!("deploy smoke test must include {required}");
                }
            }
        }
        for route in artifact.routes.iter().filter(|route| {
            route.method == "GET" && !route.path.contains(':') && route.path.starts_with("/admin")
        }) {
            let origin_ref = deploy_smoke_origin_var_ref(&route.method, &route.path);
            let command = format!(
                r#"orv_smoke_curl_origin "GET {}" "{}" -H "cookie: ${{ADMIN_SESSION_COOKIE}}; ${{ADMIN_ROLE_COOKIE}}" "$BASE_URL{}""#,
                route.path, origin_ref, route.path
            );
            if !smoke.contains(&command) {
                let path = &route.path;
                anyhow::bail!("deploy smoke test must cover GET {path} with an admin role cookie");
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_deploy_smoke_db_adapter_contract(
    smoke: &str,
    persistence: &DeployPersistence,
) -> anyhow::Result<()> {
    if persistence.db_adapters.is_empty() {
        return Ok(());
    }
    if !smoke.contains(r#"orv_smoke_file "deploy/db-adapters.json""#)
        || !smoke.contains(
            r#"orv_smoke_grep "db adapter bridge contract" "deploy/db-adapters.json" '"contract": "http-json-v1"'"#,
        )
        || !smoke.contains("orv_smoke_db_bridge_schema()")
    {
        anyhow::bail!("deploy smoke test must check DB adapter bridge contract");
    }
    for adapter in &persistence.db_adapters {
        let Some(endpoint_env) = adapter
            .bridge_env
            .iter()
            .find(|env| env.purpose == "bridge_endpoint")
        else {
            continue;
        };
        let Some(endpoint) = &adapter.endpoint else {
            continue;
        };
        let auth_env = adapter
            .bridge_env
            .iter()
            .find(|env| env.purpose == "bridge_auth_token")
            .map(|env| env.env.as_str())
            .unwrap_or("");
        let endpoint_expr = format!("${{{}:-${{ORV_DB_ADAPTER_ENDPOINT:-}}}}", endpoint_env.env);
        let auth_expr = format!("${{{auth_env}:-${{ORV_DB_ADAPTER_AUTH_TOKEN:-}}}}");
        let command = format!(
            r#"orv_smoke_db_bridge_schema "{} bridge" "{}" "{}" "{}" "{}""#,
            adapter.provider, endpoint_expr, adapter.provider, endpoint, auth_expr
        );
        if !smoke.contains(&command) {
            let provider = &adapter.provider;
            anyhow::bail!("deploy smoke test must probe DB bridge endpoint for {provider}");
        }
    }
    Ok(())
}

pub(crate) fn verify_deploy_smoke_client_contract(
    smoke: &str,
    client: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let Some(client) = client.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    if !smoke.contains("orv_smoke_file()") || !smoke.contains("orv_smoke_grep()") {
        anyhow::bail!("deploy smoke test must include client file contract helpers");
    }
    if !smoke.contains(r#"ORV_SMOKE_CLIENT_ORIGIN="ori_"#) {
        anyhow::bail!("deploy smoke test must declare a client reveal origin");
    }
    for key in ["manifest", "reactive_plan", "page", "loader", "wasm"] {
        let path = json_str(client, key, "deploy client")?;
        let command = format!(r#"orv_smoke_file "{path}""#);
        if !smoke.contains(&command) {
            anyhow::bail!("deploy smoke test must check client {key} {path}");
        }
    }
    let reactive_plan = json_str(client, "reactive_plan", "deploy client")?;
    let page = json_str(client, "page", "deploy client")?;
    let loader = json_str(client, "loader", "deploy client")?;
    let manifest = json_str(client, "manifest", "deploy client")?;
    for required in [
        format!(r#"orv_smoke_grep "client page marker" "{page}" 'data-orv-client="wasm"'"#),
        format!(r#"orv_smoke_grep "client loader reference" "{page}" 'app.js'"#),
        format!(
            r#"orv_smoke_grep "client manifest reactive plan path" "{manifest}" '"reactive_plan": "{reactive_plan}"'"#
        ),
        format!("client_manifest={manifest}"),
        format!("client_reactive_plan={reactive_plan}"),
        format!("client_page={page}"),
        format!("client_loader={loader}"),
        format!("client_wasm={}", json_str(client, "wasm", "deploy client")?),
        format!(
            r#"orv_smoke_grep "client manifest reactive plan hash" "{manifest}" '"reactive_plan_hash"'"#
        ),
        format!(r#"orv_smoke_grep "client manifest loader hash" "{manifest}" '"loader_hash"'"#),
        format!(r#"orv_smoke_grep "client manifest wasm hash" "{manifest}" '"wasm_hash"'"#),
        format!(
            r#"orv_smoke_grep "client manifest source bundle" "{manifest}" '"source_bundle": "source-bundle.json"'"#
        ),
        format!(
            r#"orv_smoke_grep "client manifest runtime" "{manifest}" '"runtime": "client_wasm"'"#
        ),
        format!(r#"orv_smoke_grep "client manifest capabilities" "{manifest}" '"capabilities"'"#),
        format!(
            r#"orv_smoke_grep "client manifest capability surfaces" "{manifest}" '"surfaces"'"#
        ),
        format!(r#"orv_smoke_grep "client manifest event actions" "{manifest}" '"event_actions"'"#),
        format!(
            r#"orv_smoke_grep "client reactive plan kind" "{reactive_plan}" '"kind": "orv.client.reactive_plan"'"#
        ),
        format!(
            r#"orv_smoke_grep "client reactive plan source bundle" "{reactive_plan}" '"source_bundle": "source-bundle.json"'"#
        ),
        format!(
            r#"orv_smoke_grep "client reactive plan blocked_by" "{reactive_plan}" '"blocked_by"'"#
        ),
        format!(r#"orv_smoke_grep "client loader bootstrap" "{loader}" 'ORV_CLIENT_BOOTSTRAP'"#),
        format!(
            r#"orv_smoke_grep "client loader embedded reactive plan" "{loader}" 'embeddedReactivePlan'"#
        ),
        format!(
            r#"orv_smoke_grep "client loader embedded reactive plan hash" "{loader}" 'embeddedReactivePlanHash'"#
        ),
        format!(
            r#"orv_smoke_grep "client loader source bundle hash" "{loader}" 'sourceBundleHash'"#
        ),
        format!(r#"orv_smoke_grep "client loader wasm reference" "{loader}" 'app.wasm'"#),
        format!(r#"orv_smoke_grep "client loader signal setter" "{loader}" '__ORV_SET_SIGNAL__'"#),
        r#"orv_smoke_reveal_contains "reveal client target summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_target_count": 5'"#.to_string(),
        r#"orv_smoke_reveal_contains "reveal client manifest summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_manifest_count": 1'"#.to_string(),
        r#"orv_smoke_reveal_contains "reveal client capability summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_capability_surface_count"'"#.to_string(),
        format!(
            r#"orv_smoke_reveal_contains "reveal client manifest target" "$ORV_SMOKE_CLIENT_ORIGIN" '"path": "{manifest}"'"#
        ),
        r#"orv_smoke_editor_reveal_contains "editor reveal client target summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_target_count": 5'"#.to_string(),
        r#"orv_smoke_editor_reveal_contains "editor reveal client manifest summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_manifest_count": 1'"#.to_string(),
        r#"orv_smoke_editor_reveal_contains "editor reveal client capability summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_capability_surface_count"'"#.to_string(),
        r#"orv_smoke_lsp_reveal_contains "lsp reveal client target summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_target_count": 5'"#.to_string(),
        r#"orv_smoke_lsp_reveal_contains "lsp reveal client manifest summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_manifest_count": 1'"#.to_string(),
        r#"orv_smoke_lsp_reveal_contains "lsp reveal client capability summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_capability_surface_count"'"#.to_string(),
        r#"orv_smoke_dap_summary_contains "dap client target summary" '"client_target_count": 5'"#.to_string(),
        r#"orv_smoke_dap_summary_contains "dap client manifest summary" '"client_manifest_count": 1'"#.to_string(),
        r#"orv_smoke_dap_summary_contains "dap client capability summary" '"client_capability_surface_count"'"#.to_string(),
    ] {
        if !smoke.contains(&required) {
            anyhow::bail!("deploy smoke test must include {required}");
        }
    }
    Ok(())
}

pub(crate) fn verify_deploy_preflight_artifact(
    dir: &Path,
    path: &str,
    artifacts: &DeployRunbookArtifacts<'_>,
    artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
    client: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let preflight_path = dir.join(path);
    if !preflight_path.is_file() {
        anyhow::bail!(
            "missing deploy preflight artifact: {}",
            preflight_path.display()
        );
    }
    let preflight = read_json_value(&preflight_path)?;
    if preflight
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("deploy preflight schema_version must be 1");
    }
    if json_str(&preflight, "kind", "deploy preflight")? != "orv.deploy.preflight" {
        anyhow::bail!("deploy preflight kind must be orv.deploy.preflight");
    }
    verify_json_pointer_str(
        &preflight,
        "/commands/verify_build",
        "orv verify-build .",
        "deploy preflight verify_build command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/env_check",
        "orv deploy-env-check .",
        "deploy preflight env_check command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/run_build",
        "orv run-build .",
        "deploy preflight run_build command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/smoke_test",
        &format!("./{}", artifacts.smoke_test),
        "deploy preflight smoke_test command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/editor_run_debug",
        "orv editor run-debug . --control next",
        "deploy preflight editor_run_debug command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/benchmark_report",
        "orv benchmark-report .",
        "deploy preflight benchmark_report command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/benchmark_report_require_pass",
        "orv benchmark-report . --require-pass",
        "deploy preflight benchmark_report_require_pass command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/compose_up",
        &format!("docker compose -f {} up --build -d", artifacts.compose),
        "deploy preflight compose_up command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/trace",
        "./deploy/server.sh --trace deploy/request-trace.json",
        "deploy preflight trace command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/trace_run_build",
        "orv run-build . --trace deploy/request-trace.json",
        "deploy preflight trace_run_build command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/editor_trace",
        "orv editor trace . --trace deploy/request-trace.json",
        "deploy preflight editor_trace command",
    )?;
    verify_json_pointer_str(
        &preflight,
        "/commands/trace_stream_smoke",
        "ORV_SMOKE_TRACE_STREAM=1 ./deploy/smoke-test.sh",
        "deploy preflight trace_stream_smoke command",
    )?;
    for (key, expected) in [
        ("server", artifacts.server_artifact),
        ("routes", artifacts.routes),
        ("source_bundle", SOURCE_BUNDLE_PATH),
        ("project_graph", "project-graph.json"),
        ("origin_map", "origin-map.json"),
        ("build_manifest", "build-manifest.json"),
        ("bundle_plan", "bundle-plan.json"),
        ("env_example", artifacts.env_example),
        ("db_adapters", artifacts.db_adapters),
        ("commerce_adapters", artifacts.commerce_adapters),
        ("smoke_test", artifacts.smoke_test),
        ("smoke_output", artifacts.smoke_output),
        ("preflight", artifacts.preflight),
        ("benchmark_evidence", artifacts.benchmark_evidence),
        ("runbook", artifacts.runbook),
    ] {
        let pointer = format!("/artifacts/{key}");
        verify_json_pointer_str(
            &preflight,
            &pointer,
            expected,
            &format!("deploy preflight artifact {key}"),
        )?;
    }
    if preflight.get("smoke_output_contract")
        != Some(&deploy_smoke_output_contract_value(artifacts))
    {
        anyhow::bail!("deploy preflight smoke_output_contract must match smoke output contract");
    }
    if preflight.get("runtime").and_then(serde_json::Value::as_str)
        != Some(artifact.runtime.as_str())
    {
        anyhow::bail!("deploy preflight runtime does not match runtime artifact");
    }
    if preflight.get("runtime_features") != Some(&serde_json::to_value(&artifact.runtime_features)?)
    {
        anyhow::bail!("deploy preflight runtime_features do not match runtime artifact");
    }
    if preflight.get("security_features")
        != Some(&serde_json::to_value(deploy_security_runtime_features(
            &artifact.runtime_features,
        ))?)
    {
        anyhow::bail!("deploy preflight security_features do not match runtime artifact");
    }
    if preflight.get("listen") != Some(&serde_json::to_value(&artifact.listen)?) {
        anyhow::bail!("deploy preflight listen does not match runtime artifact");
    }
    if preflight.get("routes") != Some(&serde_json::to_value(&artifact.routes)?) {
        anyhow::bail!("deploy preflight routes do not match runtime artifact");
    }
    if preflight.get("persistence") != Some(&deploy_persistence_value(persistence)) {
        anyhow::bail!("deploy preflight persistence does not match runtime artifact");
    }
    let expected_required_env =
        deploy_preflight_env_values(artifact.listen.as_ref(), persistence, true);
    if preflight.get("required_env") != Some(&expected_required_env) {
        anyhow::bail!("deploy preflight required_env does not match runtime artifact");
    }
    let expected_optional_env =
        deploy_preflight_env_values(artifact.listen.as_ref(), persistence, false);
    if preflight.get("optional_env") != Some(&expected_optional_env) {
        anyhow::bail!("deploy preflight optional_env does not match runtime artifact");
    }
    if preflight.get("client") != Some(&deploy_preflight_client_value(client)) {
        anyhow::bail!("deploy preflight client does not match deploy manifest");
    }
    if preflight.get("benchmark") != Some(&deploy_preflight_benchmark_value()) {
        anyhow::bail!("deploy preflight benchmark does not match 5-hour shop contract");
    }
    Ok(())
}

pub(crate) fn verify_deploy_benchmark_evidence_artifact(
    dir: &Path,
    path: &str,
    artifacts: &DeployRunbookArtifacts<'_>,
    artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
    client: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let evidence_path = dir.join(path);
    if !evidence_path.is_file() {
        anyhow::bail!(
            "missing deploy benchmark evidence artifact: {}",
            evidence_path.display()
        );
    }
    let evidence = read_json_value(&evidence_path)?;
    if evidence
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("deploy benchmark evidence schema_version must be 1");
    }
    if json_str(&evidence, "kind", "deploy benchmark evidence")? != "orv.benchmark.shop_5h.evidence"
    {
        anyhow::bail!("deploy benchmark evidence kind must be orv.benchmark.shop_5h.evidence");
    }
    verify_json_pointer_str(
        &evidence,
        "/preflight",
        artifacts.preflight,
        "deploy benchmark evidence preflight",
    )?;
    let expected_preflight =
        deploy_preflight_artifact_value(artifacts, artifact, persistence, client);
    let expected_preflight_hash = stable_json_hash(&expected_preflight)?;
    verify_json_pointer_str(
        &evidence,
        "/preflight_hash",
        &expected_preflight_hash,
        "deploy benchmark evidence preflight_hash",
    )?;
    if evidence.get("benchmark") != Some(&deploy_preflight_benchmark_value()) {
        anyhow::bail!("deploy benchmark evidence benchmark does not match 5-hour shop contract");
    }
    if evidence.get("commands") != Some(&deploy_preflight_commands_value(artifacts)) {
        anyhow::bail!("deploy benchmark evidence commands do not match deploy preflight");
    }
    if evidence.get("artifacts") != Some(&deploy_preflight_artifacts_value(artifacts)) {
        anyhow::bail!("deploy benchmark evidence artifacts do not match deploy preflight");
    }
    if evidence.get("smoke_output_contract") != Some(&deploy_smoke_output_contract_value(artifacts))
    {
        anyhow::bail!(
            "deploy benchmark evidence smoke_output_contract must match smoke output contract"
        );
    }
    verify_deploy_benchmark_evidence_task_entries(&evidence)?;
    verify_deploy_benchmark_evidence_data(&evidence)?;
    if evidence
        .get("recording_status")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        anyhow::bail!("deploy benchmark evidence recording_status must be a string");
    }
    Ok(())
}

pub(crate) fn verify_deploy_benchmark_evidence_task_entries(
    evidence: &serde_json::Value,
) -> anyhow::Result<()> {
    let entries = evidence
        .get("task_entries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            anyhow::anyhow!("deploy benchmark evidence task_entries must be an array")
        })?;
    let expected = deploy_benchmark::evidence_task_entries_value();
    let expected_entries = expected
        .as_array()
        .expect("benchmark evidence task entries are an array");
    if entries.len() != expected_entries.len() {
        anyhow::bail!("deploy benchmark evidence task_entries do not match 5-hour time budget");
    }
    for (index, (entry, expected)) in entries.iter().zip(expected_entries.iter()).enumerate() {
        if entry.get("task") != expected.get("task")
            || entry.get("target_minutes") != expected.get("target_minutes")
        {
            anyhow::bail!("deploy benchmark evidence task_entries do not match 5-hour time budget");
        }
        if !entry
            .as_object()
            .is_some_and(|object| object.contains_key("elapsed_minutes"))
        {
            anyhow::bail!(
                "deploy benchmark evidence task_entries[{index}] must include elapsed_minutes"
            );
        }
        if !entry
            .get("elapsed_minutes")
            .is_some_and(json_null_or_number)
        {
            anyhow::bail!(
                "deploy benchmark evidence task_entries[{index}] elapsed_minutes must be null or a number"
            );
        }
        if entry
            .get("status")
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            anyhow::bail!(
                "deploy benchmark evidence task_entries[{index}] status must be a string"
            );
        }
        if entry
            .get("notes")
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            anyhow::bail!("deploy benchmark evidence task_entries[{index}] notes must be a string");
        }
    }
    Ok(())
}

pub(crate) fn verify_deploy_benchmark_evidence_data(
    evidence: &serde_json::Value,
) -> anyhow::Result<()> {
    let data = evidence
        .get("data")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("deploy benchmark evidence data must be an object"))?;
    for key in [
        "elapsed_time_per_task",
        "docs_help_lookups",
        "compiler_runtime_errors",
        "first_error_to_fix_minutes",
        "manual_config_edits",
        "smoke_test_output",
        "smoke_test_required_markers",
        "participant_notes",
    ] {
        if !data.contains_key(key) {
            anyhow::bail!("deploy benchmark evidence data must include {key}");
        }
    }
    if data
        .get("elapsed_time_per_task")
        .and_then(serde_json::Value::as_str)
        != Some("task_entries[*].elapsed_minutes")
    {
        anyhow::bail!(
            "deploy benchmark evidence data elapsed_time_per_task must reference task_entries"
        );
    }
    for key in ["docs_help_lookups", "compiler_runtime_errors"] {
        if !data.get(key).is_some_and(json_null_or_integer) {
            anyhow::bail!("deploy benchmark evidence data {key} must be null or an integer");
        }
    }
    if !data
        .get("first_error_to_fix_minutes")
        .is_some_and(json_null_or_number)
    {
        anyhow::bail!(
            "deploy benchmark evidence data first_error_to_fix_minutes must be null or a number"
        );
    }
    if !data
        .get("manual_config_edits")
        .is_some_and(serde_json::Value::is_array)
    {
        anyhow::bail!("deploy benchmark evidence data manual_config_edits must be an array");
    }
    if !data
        .get("smoke_test_output")
        .is_some_and(json_null_or_string)
    {
        anyhow::bail!("deploy benchmark evidence data smoke_test_output must be null or a string");
    }
    let expected_smoke_required_markers = deploy_benchmark::smoke_required_markers_value();
    if data.get("smoke_test_required_markers") != Some(&expected_smoke_required_markers) {
        anyhow::bail!(
            "deploy benchmark evidence data smoke_test_required_markers must match smoke output contract"
        );
    }
    if !data
        .get("participant_notes")
        .is_some_and(serde_json::Value::is_string)
    {
        anyhow::bail!("deploy benchmark evidence data participant_notes must be a string");
    }
    Ok(())
}

pub(crate) fn json_null_or_integer(value: &serde_json::Value) -> bool {
    value.is_null() || value.as_i64().is_some() || value.as_u64().is_some()
}

pub(crate) fn json_null_or_number(value: &serde_json::Value) -> bool {
    value.is_null() || value.as_f64().is_some()
}

pub(crate) fn json_null_or_string(value: &serde_json::Value) -> bool {
    value.is_null() || value.as_str().is_some()
}

pub(crate) fn verify_json_pointer_str(
    root: &serde_json::Value,
    pointer: &str,
    expected: &str,
    context: &str,
) -> anyhow::Result<()> {
    if root.pointer(pointer).and_then(serde_json::Value::as_str) != Some(expected) {
        anyhow::bail!("{context} must be {expected}");
    }
    Ok(())
}

pub(crate) fn deploy_env_check_with_lookup<F>(dir: &Path, mut lookup: F) -> anyhow::Result<()>
where
    F: FnMut(&str) -> Option<String>,
{
    let preflight_path = dir.join(DEPLOY_PREFLIGHT_PATH);
    if !preflight_path.is_file() {
        anyhow::bail!(
            "missing deploy preflight artifact: {}",
            preflight_path.display()
        );
    }
    let preflight = read_json_value(&preflight_path)?;
    if preflight
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("deploy preflight schema_version must be 1");
    }
    if json_str(&preflight, "kind", "deploy preflight")? != "orv.deploy.preflight" {
        anyhow::bail!("deploy preflight kind must be orv.deploy.preflight");
    }

    let db_adapters_path = dir.join("deploy").join("db-adapters.json");
    if !db_adapters_path.is_file() {
        anyhow::bail!(
            "missing deploy DB adapters artifact: {}",
            db_adapters_path.display()
        );
    }
    let db_adapters = read_json_value(&db_adapters_path)?;
    if db_adapters
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("deploy DB adapters schema_version must be 1");
    }
    if json_str(&db_adapters, "kind", "deploy DB adapters")? != "orv.deploy.db_adapters" {
        anyhow::bail!("deploy DB adapters kind must be orv.deploy.db_adapters");
    }

    let commerce_adapters_path = dir.join("deploy").join("commerce-adapters.json");
    if !commerce_adapters_path.is_file() {
        anyhow::bail!(
            "missing deploy commerce adapters artifact: {}",
            commerce_adapters_path.display()
        );
    }
    let commerce_adapters = read_json_value(&commerce_adapters_path)?;
    if commerce_adapters
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("deploy commerce adapters schema_version must be 1");
    }
    if json_str(&commerce_adapters, "kind", "deploy commerce adapters")?
        != "orv.deploy.commerce_adapters"
    {
        anyhow::bail!("deploy commerce adapters kind must be orv.deploy.commerce_adapters");
    }
    if db_adapters
        .get("adapters")
        .and_then(serde_json::Value::as_array)
        .is_none()
    {
        anyhow::bail!("deploy DB adapters must include adapters array");
    }
    if commerce_adapters
        .get("adapters")
        .and_then(serde_json::Value::as_array)
        .is_none()
    {
        anyhow::bail!("deploy commerce adapters must include adapters array");
    }
    let (missing, optional_missing) = deploy_env_check_preflight_missing(&preflight, &mut lookup)?;
    if !missing.is_empty() {
        anyhow::bail!(
            "missing required deploy env: {}; optional missing: {}",
            missing.join(", "),
            optional_missing.join(", ")
        );
    }
    Ok(())
}

pub(crate) fn deploy_env_check_preflight_missing<F>(
    preflight: &serde_json::Value,
    lookup: &mut F,
) -> anyhow::Result<(Vec<String>, Vec<String>)>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut missing = Vec::new();
    let mut optional_missing = Vec::new();
    let required_env = preflight
        .get("required_env")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("deploy preflight required_env must be an array"))?;
    for env in required_env {
        let variable = json_str(env, "env", "deploy preflight env")?;
        if lookup(variable)
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || deploy_preflight_env_has_db_bridge_fallback(env, &mut *lookup)
        {
            continue;
        }
        missing.push(deploy_preflight_env_label(env)?);
    }
    let optional_env = preflight
        .get("optional_env")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("deploy preflight optional_env must be an array"))?;
    for env in optional_env {
        let variable = json_str(env, "env", "deploy preflight env")?;
        if lookup(variable)
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || deploy_preflight_optional_env_has_db_bridge_fallback(env, &mut *lookup)
        {
            continue;
        }
        optional_missing.push(deploy_preflight_env_label(env)?);
    }
    Ok((missing, optional_missing))
}

pub(crate) fn deploy_preflight_env_has_db_bridge_fallback<F>(
    env: &serde_json::Value,
    lookup: &mut F,
) -> bool
where
    F: FnMut(&str) -> Option<String>,
{
    let kind = env.get("kind").and_then(serde_json::Value::as_str);
    let purpose = env.get("purpose").and_then(serde_json::Value::as_str);
    let variable = env.get("env").and_then(serde_json::Value::as_str);
    if kind == Some("db")
        && purpose == Some("bridge_endpoint")
        && variable != Some("ORV_DB_ADAPTER_ENDPOINT")
    {
        return lookup("ORV_DB_ADAPTER_ENDPOINT")
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    }
    false
}

pub(crate) fn deploy_preflight_optional_env_has_db_bridge_fallback<F>(
    env: &serde_json::Value,
    lookup: &mut F,
) -> bool
where
    F: FnMut(&str) -> Option<String>,
{
    let kind = env.get("kind").and_then(serde_json::Value::as_str);
    let purpose = env.get("purpose").and_then(serde_json::Value::as_str);
    let variable = env.get("env").and_then(serde_json::Value::as_str);
    if kind == Some("db")
        && purpose == Some("bridge_auth_token")
        && variable != Some("ORV_DB_ADAPTER_AUTH_TOKEN")
    {
        return lookup("ORV_DB_ADAPTER_AUTH_TOKEN")
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    }
    false
}

pub(crate) fn deploy_preflight_env_label(env: &serde_json::Value) -> anyhow::Result<String> {
    let kind = json_str(env, "kind", "deploy preflight env")?;
    let variable = json_str(env, "env", "deploy preflight env")?;
    if let Some(provider) = env.get("provider").and_then(serde_json::Value::as_str) {
        return Ok(format!("{kind} {provider} {variable}"));
    }
    Ok(format!("{kind} {variable}"))
}

pub(crate) struct DeployRunbookArtifacts<'a> {
    pub(crate) server_artifact: &'a str,
    pub(crate) compose: &'a str,
    pub(crate) env_example: &'a str,
    pub(crate) db_adapters: &'a str,
    pub(crate) commerce_adapters: &'a str,
    pub(crate) smoke_test: &'a str,
    pub(crate) smoke_output: &'a str,
    pub(crate) preflight: &'a str,
    pub(crate) benchmark_evidence: &'a str,
    pub(crate) runbook: &'a str,
    pub(crate) routes: &'a str,
}

pub(crate) fn verify_deploy_runbook_artifact(
    dir: &Path,
    path: &str,
    artifacts: &DeployRunbookArtifacts<'_>,
    artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
    client: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let runbook_path = dir.join(path);
    if !runbook_path.is_file() {
        anyhow::bail!("missing deploy runbook: {}", runbook_path.display());
    }
    let runbook = std::fs::read_to_string(&runbook_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", runbook_path.display()))?;
    let compose_command = format!("docker compose -f {} up --build -d", artifacts.compose);
    if !runbook.contains(&compose_command) {
        anyhow::bail!("deploy runbook must include compose launch command");
    }
    if !runbook.contains(artifacts.routes) {
        let routes_artifact = artifacts.routes;
        anyhow::bail!("deploy runbook must reference {routes_artifact}");
    }
    if !runbook.contains(artifacts.env_example) {
        let env_example_path = artifacts.env_example;
        anyhow::bail!("deploy runbook must reference {env_example_path}");
    }
    if !runbook.contains(artifacts.db_adapters) {
        let db_adapters_path = artifacts.db_adapters;
        anyhow::bail!("deploy runbook must reference {db_adapters_path}");
    }
    if !runbook.contains(artifacts.commerce_adapters) {
        let commerce_adapters_path = artifacts.commerce_adapters;
        anyhow::bail!("deploy runbook must reference {commerce_adapters_path}");
    }
    if !runbook.contains(artifacts.smoke_test) {
        let smoke_test_path = artifacts.smoke_test;
        anyhow::bail!("deploy runbook must reference {smoke_test_path}");
    }
    if !runbook.contains(artifacts.smoke_output) {
        let smoke_output_path = artifacts.smoke_output;
        anyhow::bail!("deploy runbook must reference {smoke_output_path}");
    }
    if !runbook.contains(artifacts.preflight) {
        let preflight_path = artifacts.preflight;
        anyhow::bail!("deploy runbook must reference {preflight_path}");
    }
    if !runbook.contains(artifacts.benchmark_evidence) {
        let benchmark_evidence_path = artifacts.benchmark_evidence;
        anyhow::bail!("deploy runbook must reference {benchmark_evidence_path}");
    }
    let smoke_command = format!("./{}", artifacts.smoke_test);
    if !runbook.contains(&smoke_command) {
        anyhow::bail!("deploy runbook must document deploy smoke test command");
    }
    if !runbook.contains("## Benchmark Evidence") {
        anyhow::bail!("deploy runbook must document benchmark evidence capture");
    }
    if !runbook.contains("## Smoke Output Markers") {
        anyhow::bail!("deploy runbook must document smoke output markers");
    }
    for marker in deploy_benchmark::SMOKE_REQUIRED_MARKERS {
        let marker_line = format!("- `{marker}`");
        if !runbook.contains(&marker_line) {
            anyhow::bail!("deploy runbook must document smoke output marker {marker}");
        }
    }
    if !runbook.contains("orv benchmark-report .") {
        anyhow::bail!("deploy runbook must document benchmark report command");
    }
    if !runbook.contains("orv editor run-debug . --control next") {
        anyhow::bail!("deploy runbook must document DAP production summary command");
    }
    if !runbook.contains("orv benchmark-report . --require-pass") {
        anyhow::bail!("deploy runbook must document benchmark report require-pass command");
    }
    if !runbook.contains("./deploy/server.sh --trace deploy/request-trace.json") {
        anyhow::bail!("deploy runbook must document request trace capture command");
    }
    if !runbook.contains("orv editor trace . --trace deploy/request-trace.json") {
        anyhow::bail!("deploy runbook must document editor trace navigation command");
    }
    if !runbook.contains("ORV_SMOKE_TRACE_STREAM=1 ./deploy/smoke-test.sh") {
        anyhow::bail!("deploy runbook must document trace stream smoke command");
    }
    if !runbook.contains("orv deploy-env-check .") {
        anyhow::bail!("deploy runbook must document deploy env preflight command");
    }
    if !runbook.contains("orv verify-build .") {
        anyhow::bail!("deploy runbook must document build verification preflight command");
    }
    if !runbook.contains("cargo build --manifest-path server/native/Cargo.toml --release") {
        anyhow::bail!("deploy runbook must document native launcher build command");
    }
    if !runbook.contains("ORV_BUILD_DIR=. ./server/native/target/release/orv-native-server") {
        anyhow::bail!("deploy runbook must document native launcher run command");
    }
    if !runbook.contains("docker build -f server/native/Dockerfile -t orv-native-server:latest .") {
        anyhow::bail!("deploy runbook must document native runtime image build command");
    }
    if !runbook.contains("ORV_BUILD_DIR is an explicit override") {
        anyhow::bail!("deploy runbook must document native launcher build-dir inference");
    }
    if !runbook.contains("/__orv/trace/events") {
        anyhow::bail!("deploy runbook must document live trace event stream endpoint");
    }
    verify_deploy_runbook_client_section(&runbook, client)?;
    for path in &persistence.wal_paths {
        if !runbook.contains(path) {
            anyhow::bail!("deploy runbook must document persistent WAL path {path}");
        }
    }
    for path in &persistence.db_paths {
        if !runbook.contains(path) {
            anyhow::bail!("deploy runbook must document persistent DB path {path}");
        }
    }
    for endpoint in &persistence.db_endpoints {
        if !runbook.contains(endpoint) {
            anyhow::bail!("deploy runbook must document DB endpoint {endpoint}");
        }
    }
    for env in &persistence.db_env {
        if !runbook.contains(&env.env) {
            let variable = &env.env;
            anyhow::bail!("deploy runbook must document DB adapter env {variable}");
        }
        if let Some(default) = &env.default {
            if !runbook.contains(default) {
                anyhow::bail!("deploy runbook must document DB adapter env default {default}");
            }
        }
    }
    for path in &persistence.record_paths {
        if !runbook.contains(path) {
            anyhow::bail!("deploy runbook must document commerce record path {path}");
        }
    }
    for endpoint in &persistence.commerce_endpoints {
        if !runbook.contains(endpoint) {
            anyhow::bail!("deploy runbook must document commerce endpoint {endpoint}");
        }
    }
    for env in &persistence.commerce_env {
        if !runbook.contains(&env.env) {
            let variable = &env.env;
            anyhow::bail!("deploy runbook must document commerce endpoint env {variable}");
        }
        if let Some(default) = &env.default {
            if !runbook.contains(default) {
                anyhow::bail!(
                    "deploy runbook must document commerce endpoint env default {default}"
                );
            }
        }
    }
    for adapter in &persistence.commerce_adapters {
        let Some(provider) = &adapter.provider else {
            continue;
        };
        for env in &adapter.provider_env {
            let required = if env.required { "required" } else { "optional" };
            let line = format!(
                "- Commerce provider env: {} {provider} {} {required} {}",
                adapter.kind, env.env, env.purpose
            );
            if !runbook.contains(&line) {
                anyhow::bail!("deploy runbook must document {line}");
            }
        }
    }
    for volume in &persistence.volumes {
        if !runbook.contains(&volume.compose_mount) {
            let mount = &volume.compose_mount;
            anyhow::bail!("deploy runbook must document persistent volume {mount}");
        }
    }
    if let Some(port) = deploy_runbook_port_assignment(artifact.listen.as_ref()) {
        if !runbook.contains(&port) {
            anyhow::bail!("deploy runbook must document {port}");
        }
    }
    for route in &artifact.routes {
        let route_line = format!("- {} {}", route.method, route.path);
        if !runbook.contains(&route_line) {
            let method = &route.method;
            let path = &route.path;
            anyhow::bail!("deploy runbook must list route {method} {path}");
        }
    }
    Ok(())
}

pub(crate) fn verify_deploy_runbook_client_section(
    runbook: &str,
    client: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let Some(client) = client.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    for key in ["manifest", "reactive_plan", "page", "loader", "wasm"] {
        let path = json_str(client, key, "deploy client")?;
        if !runbook.contains(path) {
            anyhow::bail!("deploy runbook must document client {key} {path}");
        }
    }
    let runtime = client
        .pointer("/capabilities/runtime")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("client_wasm");
    if !runbook.contains(runtime) {
        anyhow::bail!("deploy runbook must document client runtime {runtime}");
    }
    for surface in client
        .pointer("/capabilities/surfaces")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
    {
        if !runbook.contains(surface) {
            anyhow::bail!("deploy runbook must document client capability surface {surface}");
        }
    }
    for blocker in client
        .get("blockers")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        for key in ["id", "artifact"] {
            let value = json_str(blocker, key, "deploy client blocker")?;
            if !runbook.contains(value) {
                anyhow::bail!("deploy runbook must document client blocker {value}");
            }
        }
    }
    Ok(())
}

pub(crate) struct DeployServerContract<'a> {
    pub(crate) artifact_path: &'a str,
    pub(crate) entrypoint: &'a str,
    pub(crate) routes_artifact: &'a str,
    pub(crate) runtime: &'a str,
    pub(crate) runtime_image: &'a str,
    pub(crate) listen: Option<&'a orv_compiler::ServerListenArtifact>,
}

pub(crate) fn verify_deploy_listen_value(
    actual: Option<&serde_json::Value>,
    expected: Option<&orv_compiler::ServerListenArtifact>,
    label: &str,
) -> anyhow::Result<()> {
    let expected = serde_json::to_value(expected)?;
    if actual != Some(&expected) {
        anyhow::bail!("{label} listen does not match runtime artifact");
    }
    Ok(())
}

pub(crate) fn verify_deploy_dockerfile(
    dir: &Path,
    path: &str,
    runtime_image: &str,
    listen: Option<&orv_compiler::ServerListenArtifact>,
) -> anyhow::Result<()> {
    let dockerfile_path = dir.join(path);
    if !dockerfile_path.is_file() {
        anyhow::bail!("missing deploy Dockerfile: {}", dockerfile_path.display());
    }
    let dockerfile = std::fs::read_to_string(&dockerfile_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", dockerfile_path.display()))?;
    let expected_runtime_image = format!("ARG ORV_RUNTIME_IMAGE={runtime_image}");
    if !dockerfile.contains(&expected_runtime_image) {
        anyhow::bail!("deploy Dockerfile must declare {expected_runtime_image}");
    }
    if !dockerfile.contains("FROM ${ORV_RUNTIME_IMAGE}") {
        anyhow::bail!("deploy Dockerfile must use ORV_RUNTIME_IMAGE");
    }
    if !dockerfile.contains("COPY . /app") {
        anyhow::bail!("deploy Dockerfile must copy build output into /app");
    }
    if let Some(port) = deploy_exposed_port(listen) {
        let expected = format!("EXPOSE {port}");
        if !dockerfile.contains(&expected) {
            anyhow::bail!("deploy Dockerfile must expose {port}");
        }
    }
    if !dockerfile.contains(r#"ENTRYPOINT ["./deploy/server.sh"]"#) {
        anyhow::bail!("deploy Dockerfile must run ./deploy/server.sh");
    }
    Ok(())
}

pub(crate) fn verify_deploy_routes_artifact(
    dir: &Path,
    path: &str,
    artifact_path: &str,
    runtime: &str,
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let routes_path = dir.join(path);
    if !routes_path.is_file() {
        anyhow::bail!("missing deploy routes artifact: {}", routes_path.display());
    }
    let routes = read_json_value(&routes_path)?;
    if routes
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("deploy routes schema_version must be 1");
    }
    if json_str(&routes, "artifact", "deploy routes")? != artifact_path {
        anyhow::bail!("deploy routes artifact must be {artifact_path}");
    }
    if json_str(&routes, "runtime", "deploy routes")? != runtime {
        anyhow::bail!("deploy routes runtime does not match runtime artifact");
    }
    if json_str(&routes, "protocol", "deploy routes")? != "http1" {
        anyhow::bail!("deploy routes protocol must be http1");
    }
    let expected_routes = serde_json::to_value(&artifact.routes)?;
    if routes.get("routes") != Some(&expected_routes) {
        anyhow::bail!("deploy routes do not match runtime artifact");
    }
    Ok(())
}

pub(crate) fn verify_deploy_static_target(
    dir: &Path,
    static_target: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let plan = read_json_value(&dir.join("bundle-plan.json"))?;
    let bundles = plan
        .get("bundles")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("bundle plan bundles must be an array"))?;
    let static_bundle = bundles.iter().find(|bundle| {
        bundle.get("kind").and_then(serde_json::Value::as_str) == Some("static_page")
    });
    let Some(static_target) = static_target.filter(|value| !value.is_null()) else {
        if static_bundle.is_some() {
            anyhow::bail!("deploy static target missing for bundle static_page");
        }
        return Ok(());
    };
    let path = json_str(static_target, "path", "deploy static")?;
    let Some(static_bundle) = static_bundle else {
        anyhow::bail!("deploy static target exists without bundle static_page target");
    };
    if json_str(static_bundle, "path", "bundle target")? != path {
        anyhow::bail!("deploy static path does not match bundle static_page target");
    }
    let target = dir.join(path);
    if !target.is_file() {
        anyhow::bail!("missing deploy static target: {}", target.display());
    }
    let runtime_features = static_target
        .get("runtime_features")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("deploy static runtime_features must be an array"))?;
    if !runtime_features.is_empty() {
        anyhow::bail!("deploy static target must be zero-runtime");
    }
    verify_static_page_target(static_bundle, &target)
}

pub(crate) fn verify_deploy_client_target(
    dir: &Path,
    client: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let Some(client) = client.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let runtime_features = client
        .get("runtime_features")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("deploy client runtime_features must be an array"))?;
    if !runtime_features
        .iter()
        .any(|feature| feature == "client_wasm")
    {
        anyhow::bail!("deploy client target must declare client_wasm");
    }
    let manifest = json_str(client, "manifest", "deploy client")?;
    let manifest_target = dir.join(manifest);
    if !manifest_target.is_file() {
        anyhow::bail!(
            "missing deploy client manifest: {}",
            manifest_target.display()
        );
    }
    let manifest_value = read_json_value(&manifest_target)?;
    verify_client_manifest_value(dir, &manifest_value)?;
    let reactive_plan = json_str(client, "reactive_plan", "deploy client")?;
    if manifest_value
        .get("reactive_plan")
        .and_then(serde_json::Value::as_str)
        != Some(reactive_plan)
    {
        anyhow::bail!("deploy client reactive_plan does not match client manifest");
    }
    let reactive_plan_target = dir.join(reactive_plan);
    if !reactive_plan_target.is_file() {
        anyhow::bail!(
            "missing deploy client reactive plan: {}",
            reactive_plan_target.display()
        );
    }
    let reactive_plan_value = read_json_value(&reactive_plan_target)?;
    verify_client_reactive_plan_value(dir, &reactive_plan_value)?;
    if client.get("capabilities") != manifest_value.get("capabilities") {
        anyhow::bail!("deploy client capabilities do not match client manifest");
    }
    if client.get("blocked_by") != manifest_value.get("blocked_by") {
        anyhow::bail!("deploy client blocked_by does not match client manifest");
    }
    if client.get("blockers") != manifest_value.get("blockers") {
        anyhow::bail!("deploy client blockers do not match client manifest");
    }
    let page = json_str(client, "page", "deploy client")?;
    let page_target = dir.join(page);
    if !page_target.is_file() {
        anyhow::bail!("missing deploy client page: {}", page_target.display());
    }
    verify_client_page_file(&page_target)?;
    let loader = json_str(client, "loader", "deploy client")?;
    let loader_target = dir.join(loader);
    if !loader_target.is_file() {
        anyhow::bail!("missing deploy client loader: {}", loader_target.display());
    }
    verify_client_js_target(&loader_target)?;
    let wasm = json_str(client, "wasm", "deploy client")?;
    let wasm_target = dir.join(wasm);
    if !wasm_target.is_file() {
        anyhow::bail!("missing deploy client wasm: {}", wasm_target.display());
    }
    verify_client_wasm_target(dir, &wasm_target)
}

pub(crate) fn read_json_value(path: &Path) -> anyhow::Result<serde_json::Value> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))
}

pub(crate) fn reveal_origin_json(dir: &Path, origin_id: &str) -> anyhow::Result<serde_json::Value> {
    let origin_map = read_origin_map(dir)?;
    let entry = origin_map
        .entries
        .iter()
        .find(|entry| entry.id == origin_id)
        .ok_or_else(|| anyhow::anyhow!("origin id `{origin_id}` not found"))?;
    let graph = read_json_value(&dir.join("project-graph.json"))?;
    let file_paths = graph_file_paths(&graph);
    let server_artifacts = read_server_artifacts(dir)?;
    let source_bundle = read_source_bundle_if_present(dir)?;
    let mut production = serde_json::json!({
        "graph_contract": editor_production_graph_contract_targets(dir)?,
        "routes": reveal_routes(origin_id, &origin_map, &server_artifacts),
        "native_server": reveal_native_server_targets(dir, origin_id, &origin_map)?,
        "preflight": reveal_preflight_targets(dir)?,
        "static": reveal_static_targets(dir, origin_id, &origin_map)?,
        "db_adapters": reveal_db_adapter_targets_for_origin(dir, origin_id, &origin_map)?,
        "commerce_adapters": reveal_commerce_adapter_targets_for_origin(dir, origin_id, &origin_map)?,
        "client": reveal_client_targets(dir, origin_id, entry, &origin_map)?,
    });
    let summary = production_summary_json(&production);
    production
        .as_object_mut()
        .expect("reveal production payload is object")
        .insert("summary".to_string(), summary);
    Ok(serde_json::json!({
        "schema_version": 1,
        "origin": entry,
        "source": reveal_source(entry, &file_paths, &server_artifacts, source_bundle.as_ref()),
        "project_graph": reveal_project_graph_node(&graph, origin_id),
        "production": production,
    }))
}

pub(crate) fn read_origin_map(dir: &Path) -> anyhow::Result<orv_compiler::OriginMap> {
    serde_json::from_value(read_json_value(&dir.join("origin-map.json"))?)
        .map_err(|e| anyhow::anyhow!("failed to parse origin-map.json: {e}"))
}

pub(crate) fn read_server_artifacts(
    dir: &Path,
) -> anyhow::Result<Vec<(String, orv_compiler::ServerRuntimeArtifact)>> {
    let plan = read_json_value(&dir.join("bundle-plan.json"))?;
    let mut artifacts = Vec::new();
    let Some(bundles) = plan.get("bundles").and_then(serde_json::Value::as_array) else {
        return Ok(artifacts);
    };
    for bundle in bundles {
        if bundle.get("kind").and_then(serde_json::Value::as_str) != Some("server_runtime") {
            continue;
        }
        let path = json_str(bundle, "path", "bundle target")?;
        let artifact = read_server_artifact(&dir.join(path))?;
        artifacts.push((path.to_string(), artifact));
    }
    Ok(artifacts)
}

pub(crate) fn read_source_bundle_if_present(
    dir: &Path,
) -> anyhow::Result<Option<orv_compiler::SourceBundleArtifact>> {
    let path = dir.join("source-bundle.json");
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(read_source_bundle_artifact(&path)?))
}

pub(crate) fn read_source_bundle_artifact(
    path: &Path,
) -> anyhow::Result<orv_compiler::SourceBundleArtifact> {
    let artifact: orv_compiler::SourceBundleArtifact =
        serde_json::from_value(read_json_value(path)?)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
    orv_compiler::verify_source_bundle_artifact(&artifact)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    Ok(artifact)
}

pub(crate) fn graph_file_paths(graph: &serde_json::Value) -> HashMap<u32, String> {
    let mut paths = HashMap::new();
    let Some(nodes) = graph.get("nodes").and_then(serde_json::Value::as_array) else {
        return paths;
    };
    for node in nodes {
        if node.get("kind").and_then(serde_json::Value::as_str) != Some("file") {
            continue;
        }
        let Some(file) = node.get("file").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let Some(path) = node.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if let Ok(file) = u32::try_from(file) {
            paths.insert(file, path.to_string());
        }
    }
    paths
}

pub(crate) fn reveal_source(
    entry: &orv_compiler::OriginEntry,
    file_paths: &HashMap<u32, String>,
    server_artifacts: &[(String, orv_compiler::ServerRuntimeArtifact)],
    source_bundle: Option<&orv_compiler::SourceBundleArtifact>,
) -> serde_json::Value {
    let mut path = file_paths.get(&entry.span.file).cloned();
    let mut source = None;
    if let Ok(file_index) = usize::try_from(entry.span.file) {
        for (_, artifact) in server_artifacts {
            if let Some(file) = artifact.source_bundle.files.get(file_index) {
                path = Some(file.path.clone());
                source = Some(file.source.clone());
                break;
            }
        }
        if source.is_none() {
            if let Some(file) = source_bundle.and_then(|bundle| bundle.files.get(file_index)) {
                path = Some(file.path.clone());
                source = Some(file.source.clone());
            }
        }
    }
    if source.is_none() {
        if let Some(path) = &path {
            source = std::fs::read_to_string(path).ok();
        }
    }
    let snippet = source.as_deref().and_then(|source| {
        byte_snippet(source, entry.span.start, entry.span.end).map(ToString::to_string)
    });
    serde_json::json!({
        "file": entry.span.file,
        "path": path,
        "start": entry.span.start,
        "end": entry.span.end,
        "snippet": snippet,
        "content": source,
    })
}

pub(crate) fn byte_snippet(source: &str, start: u32, end: u32) -> Option<&str> {
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    source.get(start..end)
}

pub(crate) fn reveal_project_graph_node(
    graph: &serde_json::Value,
    origin_id: &str,
) -> serde_json::Value {
    let Some(nodes) = graph.get("nodes").and_then(serde_json::Value::as_array) else {
        return serde_json::Value::Null;
    };
    let Some(links) = graph
        .get("semantic")
        .and_then(|semantic| semantic.get("origin_links"))
        .and_then(serde_json::Value::as_array)
    else {
        return serde_json::Value::Null;
    };
    let Some(link) = links
        .iter()
        .find(|link| link.get("origin_id").and_then(serde_json::Value::as_str) == Some(origin_id))
    else {
        return serde_json::Value::Null;
    };
    let Some(node_id) = link.get("node_id") else {
        return serde_json::Value::Null;
    };
    nodes
        .iter()
        .find(|node| node.get("id") == Some(node_id))
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn reveal_routes(
    origin_id: &str,
    origin_map: &orv_compiler::OriginMap,
    server_artifacts: &[(String, orv_compiler::ServerRuntimeArtifact)],
) -> Vec<serde_json::Value> {
    let mut routes = Vec::new();
    for (artifact_path, artifact) in server_artifacts {
        for route in artifact.routes.iter().filter(|route| {
            route.origin_id == origin_id || origin_contains(origin_map, &route.origin_id, origin_id)
        }) {
            routes.push(serde_json::json!({
                "artifact": artifact_path,
                "method": route.method,
                "path": route.path,
                "origin_id": route.origin_id,
                "match": if route.origin_id == origin_id { "direct" } else { "contains" },
                "matched_origin_id": origin_id,
                "policies": route.policies,
            }));
        }
    }
    routes
}

pub(crate) fn reveal_native_server_targets(
    dir: &Path,
    origin_id: &str,
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let plan = read_json_value(&dir.join("bundle-plan.json"))?;
    let Some(bundles) = plan.get("bundles").and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut targets = Vec::new();
    for bundle in bundles {
        if bundle.get("kind").and_then(serde_json::Value::as_str) != Some("native_server_plan") {
            continue;
        }
        let path = json_str(bundle, "path", "bundle target")?;
        let target_path = dir.join(path);
        if !target_path.is_file() {
            continue;
        }
        let native_plan = read_json_value(&target_path)?;
        let matching_routes = native_plan
            .get("routes")
            .and_then(serde_json::Value::as_array)
            .map(|routes| {
                routes
                    .iter()
                    .filter(|route| {
                        route.get("origin_id").and_then(serde_json::Value::as_str)
                            == Some(origin_id)
                            || route
                                .get("origin_id")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|route_origin_id| {
                                    origin_contains(origin_map, route_origin_id, origin_id)
                                })
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if matching_routes.is_empty() {
            continue;
        }
        targets.push(native_server_production_target_json(
            dir,
            path,
            &native_plan,
            serde_json::json!(matching_routes),
        )?);
    }
    Ok(targets)
}

pub(crate) fn origin_contains(
    origin_map: &orv_compiler::OriginMap,
    ancestor_id: &str,
    descendant_id: &str,
) -> bool {
    if ancestor_id == descendant_id {
        return true;
    }
    let mut stack = vec![ancestor_id];
    let mut seen = HashSet::<&str>::new();
    while let Some(current) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }
        for edge in origin_map
            .edges
            .iter()
            .filter(|edge| edge.kind == "contains" && edge.from == current)
        {
            if edge.to == descendant_id {
                return true;
            }
            stack.push(edge.to.as_str());
        }
    }
    false
}

pub(crate) fn origin_is_html_projection_origin(
    origin_map: &orv_compiler::OriginMap,
    origin_id: &str,
) -> bool {
    origin_map.entries.iter().any(|entry| {
        entry.id == origin_id
            && entry.kind == "domain"
            && matches!(entry.name.as_str(), "html" | "out")
    }) || origin_map.entries.iter().any(|entry| {
        entry.kind == "domain"
            && entry.name == "html"
            && origin_contains(origin_map, &entry.id, origin_id)
    })
}

pub(crate) fn reveal_native_server_routes_source(
    dir: &Path,
    native_plan: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let Some(path) = native_plan
        .get("routes_source")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(serde_json::Value::Null);
    };
    let target_path = dir.join(path);
    if !target_path.is_file() {
        return Ok(serde_json::json!({
            "path": path,
            "exists": false,
        }));
    }
    let source = std::fs::read_to_string(&target_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target_path.display()))?;
    Ok(serde_json::json!({
        "path": path,
        "exists": true,
        "route_count": source.matches("OrvNativeRoute { method:").count(),
    }))
}

pub(crate) fn reveal_native_server_router_source(
    dir: &Path,
    native_plan: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let Some(path) = native_plan
        .get("router_source")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(serde_json::Value::Null);
    };
    let target_path = dir.join(path);
    if !target_path.is_file() {
        return Ok(serde_json::json!({
            "path": path,
            "exists": false,
        }));
    }
    let source = std::fs::read_to_string(&target_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target_path.display()))?;
    Ok(serde_json::json!({
        "path": path,
        "exists": true,
        "dispatch": source.contains("pub fn orv_native_dispatch("),
        "handler_count_contract": source.contains("ORV_NATIVE_HANDLER_COUNT"),
        "response_origin_dispatch": source.contains("pub response_origin_id: Option<&'static str>")
            && source.contains("response_origin_id: response.response_origin_id"),
    }))
}

pub(crate) fn reveal_native_server_handlers_source(
    dir: &Path,
    native_plan: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let Some(path) = native_plan
        .get("handlers_source")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(serde_json::Value::Null);
    };
    let target_path = dir.join(path);
    if !target_path.is_file() {
        return Ok(serde_json::json!({
            "path": path,
            "exists": false,
        }));
    }
    let source = std::fs::read_to_string(&target_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target_path.display()))?;
    Ok(serde_json::json!({
        "path": path,
        "exists": true,
        "handler_count_contract": source.contains("ORV_NATIVE_HANDLER_COUNT"),
        "body_lowering_placeholder": source.contains("native route body lowering pending"),
        "response_origin_dispatch": source.contains("pub response_origin_id: Option<&'static str>")
            && (source.contains("response_origin_id: route_match.route.response_origin_ids.first().copied()")
                || source.contains("response_origin_id: Some(")),
    }))
}

pub(crate) fn reveal_native_runtime_image_plan(
    dir: &Path,
    native_plan: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let Some(path) = native_plan
        .get("runtime_image_plan")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(serde_json::Value::Null);
    };
    let target_path = dir.join(path);
    if !target_path.is_file() {
        return Ok(serde_json::json!({
            "path": path,
            "exists": false,
        }));
    }
    let image_plan = read_json_value(&target_path)?;
    Ok(serde_json::json!({
        "path": path,
        "exists": true,
        "kind": image_plan
            .get("kind")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "status": image_plan
            .get("status")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "artifact": image_plan
            .get("artifact")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "reference_image": image_plan
            .get("reference_image")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "target": image_plan
            .get("target")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "runtime_features": image_plan
            .get("runtime_features")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "blocked_by": image_plan
            .get("blocked_by")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    }))
}

pub(crate) fn reveal_commerce_adapter_targets(
    dir: &Path,
) -> anyhow::Result<Vec<serde_json::Value>> {
    reveal_commerce_adapter_targets_impl(dir, None, None)
}

pub(crate) fn reveal_commerce_adapter_targets_for_origin(
    dir: &Path,
    origin_id: &str,
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<Vec<serde_json::Value>> {
    reveal_commerce_adapter_targets_impl(dir, Some(origin_id), Some(origin_map))
}

pub(crate) fn reveal_commerce_adapter_targets_impl(
    dir: &Path,
    origin_id: Option<&str>,
    origin_map: Option<&orv_compiler::OriginMap>,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let deploy_manifest_path = dir.join("deploy").join("manifest.json");
    if !deploy_manifest_path.is_file() {
        return Ok(Vec::new());
    }
    let deploy = read_json_value(&deploy_manifest_path)?;
    let Some(path) = deploy
        .get("server")
        .and_then(|server| server.get("commerce_adapters"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(Vec::new());
    };
    let target_path = dir.join(path);
    if !target_path.is_file() {
        return Ok(vec![serde_json::json!({
            "kind": "commerce_adapters",
            "path": path,
            "exists": false,
        })]);
    }
    let artifact = read_json_value(&target_path)?;
    let adapters = artifact
        .get("adapters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let matched_adapters = origin_id
        .zip(origin_map)
        .map(|(origin_id, origin_map)| {
            reveal_adapter_origin_matches(&adapters, origin_id, origin_map)
        })
        .unwrap_or_default();
    Ok(vec![serde_json::json!({
        "kind": "commerce_adapters",
        "path": path,
        "exists": true,
        "selected_origin_id": origin_id,
        "matched": !matched_adapters.is_empty(),
        "matched_adapter_count": matched_adapters.len(),
        "artifact": artifact
            .get("artifact")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "adapters": adapters,
        "source_reveal_commands": adapter_source_reveal_commands(dir, &adapters),
        "matched_adapters": matched_adapters,
    })])
}

pub(crate) fn reveal_db_adapter_targets(dir: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    reveal_db_adapter_targets_impl(dir, None, None)
}

pub(crate) fn reveal_db_adapter_targets_for_origin(
    dir: &Path,
    origin_id: &str,
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<Vec<serde_json::Value>> {
    reveal_db_adapter_targets_impl(dir, Some(origin_id), Some(origin_map))
}

pub(crate) fn reveal_db_adapter_targets_impl(
    dir: &Path,
    origin_id: Option<&str>,
    origin_map: Option<&orv_compiler::OriginMap>,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let deploy_manifest_path = dir.join("deploy").join("manifest.json");
    if !deploy_manifest_path.is_file() {
        return Ok(Vec::new());
    }
    let deploy = read_json_value(&deploy_manifest_path)?;
    let Some(path) = deploy
        .get("server")
        .and_then(|server| server.get("db_adapters"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(Vec::new());
    };
    let target_path = dir.join(path);
    if !target_path.is_file() {
        return Ok(vec![serde_json::json!({
            "kind": "db_adapters",
            "path": path,
            "exists": false,
        })]);
    }
    let artifact = read_json_value(&target_path)?;
    let adapters = artifact
        .get("adapters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let matched_adapters = origin_id
        .zip(origin_map)
        .map(|(origin_id, origin_map)| {
            reveal_adapter_origin_matches(&adapters, origin_id, origin_map)
        })
        .unwrap_or_default();
    Ok(vec![serde_json::json!({
        "kind": "db_adapters",
        "path": path,
        "exists": true,
        "selected_origin_id": origin_id,
        "matched": !matched_adapters.is_empty(),
        "matched_adapter_count": matched_adapters.len(),
        "artifact": artifact
            .get("artifact")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "adapters": adapters,
        "source_reveal_commands": adapter_source_reveal_commands(dir, &adapters),
        "matched_adapters": matched_adapters,
    })])
}

pub(crate) fn adapter_source_reveal_commands(
    dir: &Path,
    adapters: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let Some(adapters) = adapters.as_array() else {
        return Vec::new();
    };
    let build_dir = dir.display().to_string();
    adapters
        .iter()
        .enumerate()
        .flat_map(|(index, adapter)| {
            let build_dir = build_dir.clone();
            adapter_source_origin_ids(adapter)
                .into_iter()
                .map(move |origin_id| {
                    let command = editor_reveal_command_json(&build_dir, Some(&origin_id));
                    serde_json::json!({
                        "adapter_index": index,
                        "kind": adapter
                            .get("kind")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "provider": adapter
                            .get("provider")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "env": adapter
                            .get("env")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "endpoint": adapter
                            .get("endpoint")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "record_path": adapter
                            .get("record_path")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "source_origin_id": origin_id,
                        "command": command,
                    })
                })
        })
        .collect()
}

pub(crate) fn reveal_adapter_origin_matches(
    adapters: &serde_json::Value,
    origin_id: &str,
    origin_map: &orv_compiler::OriginMap,
) -> Vec<serde_json::Value> {
    let Some(adapters) = adapters.as_array() else {
        return Vec::new();
    };
    adapters
        .iter()
        .filter_map(|adapter| {
            let source_origin_ids = adapter_source_origin_ids(adapter);
            let match_kind = if source_origin_ids.iter().any(|source| source == origin_id) {
                "direct"
            } else if source_origin_ids
                .iter()
                .any(|source| origin_contains(origin_map, source, origin_id))
            {
                "source_contains_selected"
            } else if source_origin_ids
                .iter()
                .any(|source| origin_contains(origin_map, origin_id, source))
            {
                "selected_contains_source"
            } else {
                return None;
            };
            let mut value = adapter.clone();
            if let Some(adapter) = value.as_object_mut() {
                adapter.insert("match".to_string(), serde_json::json!(match_kind));
                adapter.insert(
                    "matched_origin_id".to_string(),
                    serde_json::json!(origin_id),
                );
            }
            Some(value)
        })
        .collect()
}

pub(crate) fn adapter_source_origin_ids(adapter: &serde_json::Value) -> Vec<String> {
    let mut ids = adapter
        .get("source_origin_ids")
        .and_then(serde_json::Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(id) = adapter
        .get("source_origin_id")
        .and_then(serde_json::Value::as_str)
    {
        ids.push(id.to_string());
    }
    normalize_source_origin_ids(&mut ids);
    ids
}

pub(crate) fn reveal_preflight_targets(dir: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    let deploy_manifest_path = dir.join("deploy").join("manifest.json");
    if !deploy_manifest_path.is_file() {
        return Ok(Vec::new());
    }
    let deploy = read_json_value(&deploy_manifest_path)?;
    let Some(path) = deploy
        .get("server")
        .and_then(|server| server.get("preflight"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(Vec::new());
    };
    let target_path = dir.join(path);
    if !target_path.is_file() {
        return Ok(vec![serde_json::json!({
            "kind": "preflight",
            "path": path,
            "exists": false,
        })]);
    }
    let artifact = read_json_value(&target_path)?;
    let benchmark_evidence = reveal_benchmark_evidence_summary(dir, &artifact)?;
    Ok(vec![serde_json::json!({
        "kind": "preflight",
        "path": path,
        "exists": true,
        "artifact": artifact
            .get("artifact")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "commands": artifact
            .get("commands")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        "artifacts": artifact
            .get("artifacts")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        "smoke_output_contract": artifact
            .get("smoke_output_contract")
            .cloned()
            .unwrap_or_else(|| {
                artifact
                    .pointer("/artifacts/smoke_output")
                    .and_then(serde_json::Value::as_str)
                    .map_or(serde_json::Value::Null, smoke_output_contract_value)
            }),
        "benchmark": artifact
            .get("benchmark")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "benchmark_evidence": benchmark_evidence,
        "runtime_features": artifact
            .get("runtime_features")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "security_features": artifact
            .get("security_features")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "listen": artifact
            .get("listen")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "routes": artifact
            .get("routes")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "required_env": artifact
            .get("required_env")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "optional_env": artifact
            .get("optional_env")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    })])
}

pub(crate) fn reveal_benchmark_evidence_summary(
    dir: &Path,
    preflight: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let Some(path) = preflight
        .pointer("/artifacts/benchmark_evidence")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(serde_json::Value::Null);
    };
    let target_path = dir.join(path);
    if !target_path.is_file() {
        return Ok(serde_json::json!({
            "path": path,
            "exists": false,
        }));
    }
    let evidence = read_json_value(&target_path)?;
    let task_count = evidence
        .get("task_entries")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let data_keys = evidence
        .get("data")
        .and_then(serde_json::Value::as_object)
        .map(|data| data.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let max_elapsed_minutes = preflight
        .pointer("/benchmark/max_elapsed_minutes")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(300.0);
    let task_report = benchmark_report_tasks(&evidence, max_elapsed_minutes)?;
    let smoke_output_rel = preflight
        .pointer("/artifacts/smoke_output")
        .and_then(serde_json::Value::as_str);
    let data_report = benchmark_report_data(&evidence, Some(dir), smoke_output_rel)?;
    let report_status =
        benchmark_report_status_summary(&task_report, &data_report, max_elapsed_minutes);
    Ok(serde_json::json!({
        "path": path,
        "exists": true,
        "kind": evidence
            .get("kind")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "preflight": evidence
            .get("preflight")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "preflight_hash": evidence
            .get("preflight_hash")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "recording_status": evidence
            .get("recording_status")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "report_status": report_status.status,
        "max_elapsed_minutes": max_elapsed_minutes,
        "total_elapsed_minutes": report_status
            .total_elapsed_minutes
            .map_or(serde_json::Value::Null, serde_json::Value::from),
        "time_over_limit": report_status.time_over_limit,
        "task_count": task_count,
        "recorded_task_count": task_report
            .get("recorded_task_count")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "missing_task_count": report_status.missing_task_count,
        "failed_task_count": report_status.failed_task_count,
        "missing_data_count": report_status.missing_data_count,
        "missing_data": data_report
            .get("missing_data")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "smoke_test_output_source": data_report
            .get("smoke_test_output_source")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "smoke_test_required_markers": data_report
            .get("smoke_test_required_markers")
            .cloned()
            .unwrap_or_else(deploy_benchmark::smoke_required_markers_value),
        "smoke_test_summary": data_report
            .get("smoke_test_summary")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "data_keys": data_keys,
    }))
}

pub(crate) fn reveal_static_targets(
    dir: &Path,
    origin_id: &str,
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<Vec<serde_json::Value>> {
    if !origin_is_html_projection_origin(origin_map, origin_id) {
        return Ok(Vec::new());
    }
    let plan = read_json_value(&dir.join("bundle-plan.json"))?;
    let Some(bundles) = plan.get("bundles").and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut targets = Vec::new();
    for bundle in bundles.iter().filter(|bundle| {
        bundle.get("kind").and_then(serde_json::Value::as_str) == Some("static_page")
    }) {
        let path = json_str(bundle, "path", "bundle target")?;
        let target_path = dir.join(path);
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

pub(crate) fn reveal_client_targets(
    dir: &Path,
    origin_id: &str,
    entry: &orv_compiler::OriginEntry,
    origin_map: &orv_compiler::OriginMap,
) -> anyhow::Result<Vec<serde_json::Value>> {
    if !matches!(entry.kind.as_str(), "signal" | "await")
        && !origin_is_html_projection_origin(origin_map, origin_id)
    {
        return Ok(Vec::new());
    }
    reveal_client_bundle_targets(dir)
}

pub(crate) fn reveal_client_bundle_targets(dir: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    let plan = read_json_value(&dir.join("bundle-plan.json"))?;
    let Some(bundles) = plan.get("bundles").and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut targets = Vec::new();
    for bundle in bundles {
        let kind = bundle
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !is_client_bundle_kind(kind) {
            continue;
        }
        let path = json_str(bundle, "path", "bundle target")?;
        let mut target = serde_json::json!({
            "kind": kind,
            "path": path,
            "exists": dir.join(path).is_file(),
            "runtime_features": bundle
                .get("runtime_features")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        });
        if kind == "client_manifest" {
            add_client_manifest_reveal_fields(dir, path, &mut target)?;
        } else if kind == "client_reactive_plan" {
            add_client_reactive_plan_reveal_fields(dir, path, &mut target)?;
        }
        targets.push(target);
    }
    Ok(targets)
}

pub(crate) fn is_client_bundle_kind(kind: &str) -> bool {
    matches!(
        kind,
        "client_manifest" | "client_reactive_plan" | "client_page" | "client_js" | "client_wasm"
    )
}

pub(crate) fn add_client_manifest_reveal_fields(
    dir: &Path,
    path: &str,
    target: &mut serde_json::Value,
) -> anyhow::Result<()> {
    let manifest_path = dir.join(path);
    if !manifest_path.is_file() {
        return Ok(());
    }
    let manifest = read_json_value(&manifest_path)?;
    target["source_bundle"] = manifest
        .get("source_bundle")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    target["source_bundle_hash"] = manifest
        .get("source_bundle_hash")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    target["wasm_hash"] = manifest
        .get("wasm_hash")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    target["exports"] = manifest
        .get("exports")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    target["capabilities"] = manifest
        .get("capabilities")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    target["blocked_by"] = manifest
        .get("blocked_by")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    target["blockers"] = manifest
        .get("blockers")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    Ok(())
}

pub(crate) fn add_client_reactive_plan_reveal_fields(
    dir: &Path,
    path: &str,
    target: &mut serde_json::Value,
) -> anyhow::Result<()> {
    let plan_path = dir.join(path);
    if !plan_path.is_file() {
        return Ok(());
    }
    let plan = read_json_value(&plan_path)?;
    target["source_bundle"] = plan
        .get("source_bundle")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    target["source_bundle_hash"] = plan
        .get("source_bundle_hash")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    target["signal_count"] = plan
        .get("signals")
        .and_then(serde_json::Value::as_array)
        .map_or_else(
            || serde_json::json!(0),
            |signals| serde_json::json!(signals.len()),
        );
    target["blocked_by"] = plan
        .get("blocked_by")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    target["blockers"] = plan
        .get("blockers")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    Ok(())
}

pub(crate) fn json_str<'a>(
    value: &'a serde_json::Value,
    key: &str,
    context: &str,
) -> anyhow::Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("{context} field `{key}` must be a string"))
}

pub(crate) fn json_optional_str<'a>(
    value: &'a serde_json::Value,
    key: &str,
    context: &str,
) -> anyhow::Result<Option<&'a str>> {
    let Some(value) = value.get(key) else {
        return Ok(None);
    };
    value
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("{context} field `{key}` must be a non-empty string"))
}

pub(crate) fn json_u32(value: &serde_json::Value, key: &str, context: &str) -> anyhow::Result<u32> {
    let raw = value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("{context} field `{key}` must be an integer"))?;
    u32::try_from(raw).map_err(|_| anyhow::anyhow!("{context} field `{key}` is too large"))
}

pub(crate) fn cmd_verify_artifact(path: &Path) -> anyhow::Result<()> {
    let artifact = read_server_artifact(path)?;
    orv_compiler::verify_server_runtime_artifact(&artifact)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    println!(
        "artifact: {} verified (routes={}, sources={})",
        path.display(),
        artifact.routes.len(),
        artifact.source_bundle.files.len()
    );
    Ok(())
}

pub(crate) fn cmd_check_artifact(path: &Path) -> anyhow::Result<()> {
    let artifact = read_server_artifact(path)?;
    orv_compiler::verify_server_runtime_artifact(&artifact)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    let lowered = lower_artifact_entry(&artifact)?;
    println!(
        "artifact: {} checked (routes={}, sources={}, items={})",
        path.display(),
        artifact.routes.len(),
        artifact.source_bundle.files.len(),
        lowered.program.items.len()
    );
    Ok(())
}

pub(crate) fn cmd_check_build(dir: &Path) -> anyhow::Result<()> {
    verify_build_dir(dir)?;
    let source_bundle = read_source_bundle_artifact(&dir.join("source-bundle.json"))?;
    let lowered = lower_source_bundle_entry(&source_bundle)?;
    println!(
        "build: {} checked (sources={}, items={})",
        dir.display(),
        source_bundle.files.len(),
        lowered.program.items.len()
    );
    Ok(())
}

pub(crate) fn cmd_lock(path: &Path, check: bool) -> anyhow::Result<()> {
    let manifest = project_manifest_path(path)?;
    let lock = project_lock_json(&manifest)?;
    let lock_path = manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("orv.lock");
    if check {
        let existing = read_json_value(&lock_path)?;
        if existing != lock {
            anyhow::bail!("orv.lock is out of date; run `orv lock`");
        }
        println!("lock: {} verified", lock_path.display());
    } else {
        write_json_atomic(&lock_path, &lock)?;
        println!("lock: wrote {}", lock_path.display());
    }
    Ok(())
}

pub(crate) fn cmd_fetch(path: &Path, out: &Path) -> anyhow::Result<()> {
    let manifest = project_manifest_path(path)?;
    let root = manifest.parent().unwrap_or_else(|| Path::new("."));
    let lock_path = root.join("orv.lock");
    let lock = read_json_value(&lock_path)?;
    let expected = project_lock_json(&manifest)?;
    if lock != expected {
        anyhow::bail!("orv.lock is out of date; run `orv lock` before `orv fetch`");
    }

    fetch_lock_dependencies(root, out, &lock, "orv.lock")?;
    println!("fetch: wrote {}", out.display());
    Ok(())
}

pub(crate) fn fetch_lock_dependencies(
    root: &Path,
    out: &Path,
    lock: &serde_json::Value,
    lockfile: &str,
) -> anyhow::Result<serde_json::Value> {
    let mut packages = Vec::new();
    for key in ["dependencies", "dev_dependencies"] {
        let entries = lock
            .get(key)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("orv.lock field `{key}` must be an array"))?;
        for entry in entries {
            packages.push(fetch_dependency_package(root, out, entry)?);
        }
    }

    let manifest = serde_json::json!({
        "schema_version": 1,
        "kind": "orv.dependencies",
        "root": root.display().to_string(),
        "lockfile": lockfile,
        "stats": {
            "package_count": packages.len(),
        },
        "packages": packages,
    });
    write_json(&out.join("deps-manifest.json"), &manifest)?;
    Ok(manifest)
}

pub(crate) fn fetch_dependency_package(
    root: &Path,
    out: &Path,
    dependency: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let name = json_str(dependency, "name", "lock dependency")?;
    let section = json_str(dependency, "section", "lock dependency")?;
    let source = json_str(dependency, "source", "lock dependency")?;
    let version = json_str(dependency, "version", "lock dependency")?;
    let checksum = json_str(dependency, "checksum", "lock dependency")?;
    let fetched = match source {
        "path" => FetchedDependency::ProjectRoot(path_dependency_project_root(root, dependency)?),
        "registry" => registry_dependency_source(root, dependency)?,
        other => anyhow::bail!("unsupported dependency source `{other}`"),
    };
    let resolved_url;
    let resolved_path;
    let source_bundle = match fetched {
        FetchedDependency::ProjectRoot(package_root) => {
            let entry = project_entry_path(&package_root)?;
            let loaded = orv_project::load_project(&entry).map_err(|e| anyhow::anyhow!("{e}"))?;
            report_diagnostics(&loaded.diagnostics, &loaded.files)?;
            resolved_path = Some(package_root.display().to_string());
            resolved_url = None;
            orv_compiler::source_bundle_artifact(
                entry.display().to_string(),
                loaded
                    .files
                    .iter()
                    .map(|file| (file.path.display().to_string(), file.source.clone())),
            )
        }
        FetchedDependency::SourceBundle { url, artifact } => {
            resolved_path = None;
            resolved_url = Some(url);
            artifact
        }
    };
    orv_compiler::verify_source_bundle_artifact(&source_bundle)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    let package_dir = format!(
        "packages/{}/{}",
        dependency_cache_component(section),
        dependency_cache_component(name)
    );
    let source_bundle_path = format!("{package_dir}/source-bundle.json");
    write_json(
        &out.join(&source_bundle_path),
        &serde_json::to_value(&source_bundle)?,
    )?;
    let source_entry = source_bundle.entry.clone();
    let source_file_count = source_bundle.files.len();

    let mut package = serde_json::json!({
        "name": name,
        "section": section,
        "source": source,
        "version": version,
        "checksum": checksum,
        "entry": source_entry,
        "source_bundle": source_bundle_path,
        "source_file_count": source_file_count,
        "verified": true,
    });
    if let Some(path) = resolved_path {
        package["resolved_path"] = serde_json::json!(path);
    }
    if let Some(url) = resolved_url {
        package["resolved_url"] = serde_json::json!(url);
    }
    if source == "path" {
        package["path"] = serde_json::json!(json_str(dependency, "path", "path dependency")?);
    } else {
        package["registry"] =
            serde_json::json!(json_str(dependency, "registry", "registry dependency")?);
        if let Some(auth_token_env) =
            json_optional_str(dependency, "auth_token_env", "registry dependency")?
        {
            package["auth_token_env"] = serde_json::json!(auth_token_env);
        }
    }
    Ok(package)
}

pub(crate) fn path_dependency_project_root(
    root: &Path,
    dependency: &serde_json::Value,
) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(json_str(dependency, "path", "path dependency")?);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(root.join(path))
    }
}

pub(crate) enum FetchedDependency {
    ProjectRoot(PathBuf),
    SourceBundle {
        url: String,
        artifact: orv_compiler::SourceBundleArtifact,
    },
}

pub(crate) fn registry_dependency_source(
    root: &Path,
    dependency: &serde_json::Value,
) -> anyhow::Result<FetchedDependency> {
    let registry = json_str(dependency, "registry", "registry dependency")?;
    if registry.starts_with("http://") || registry.starts_with("https://") {
        let url = registry_source_bundle_url(
            registry,
            json_str(dependency, "name", "registry dependency")?,
            json_str(dependency, "version", "registry dependency")?,
        );
        let artifact = download_registry_source_bundle(
            &url,
            json_optional_str(dependency, "auth_token_env", "registry dependency")?,
        )?;
        return Ok(FetchedDependency::SourceBundle { url, artifact });
    }
    if registry == "registry.orv.dev" {
        anyhow::bail!(
            "remote registry download requires an explicit http://, https://, or file:// registry"
        );
    }
    let registry_root = registry.strip_prefix("file://").map_or_else(
        || {
            let path = PathBuf::from(registry);
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        },
        PathBuf::from,
    );
    Ok(FetchedDependency::ProjectRoot(
        registry_root
            .join(json_str(dependency, "name", "registry dependency")?)
            .join(json_str(dependency, "version", "registry dependency")?),
    ))
}

pub(crate) fn registry_source_bundle_url(registry: &str, name: &str, version: &str) -> String {
    format!(
        "{}/{}/{}/source-bundle.json",
        registry.trim_end_matches('/'),
        name,
        version
    )
}

pub(crate) fn download_registry_source_bundle(
    url: &str,
    auth_token_env: Option<&str>,
) -> anyhow::Result<orv_compiler::SourceBundleArtifact> {
    let body = registry_get_string_with_auth(url, auth_token_env)?;
    let artifact: orv_compiler::SourceBundleArtifact = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("failed to parse registry source bundle {url}: {e}"))?;
    orv_compiler::verify_source_bundle_artifact(&artifact)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    Ok(artifact)
}

pub(crate) fn registry_get_string_with_auth(
    url: &str,
    auth_token_env: Option<&str>,
) -> anyhow::Result<String> {
    if url.starts_with("https://") {
        return https_get_string_with_auth(url, auth_token_env);
    }
    http_get_string_with_auth(url, auth_token_env)
}

pub(crate) fn https_get_string_with_auth(
    url: &str,
    auth_token_env: Option<&str>,
) -> anyhow::Result<String> {
    let mut request = ureq::get(url);
    if let Some(authorization) = registry_authorization_header(auth_token_env)? {
        request = request.header("Authorization", &authorization);
    }
    let mut response = request
        .call()
        .map_err(|e| anyhow::anyhow!("registry request {url} failed: {e}"))?;
    response
        .body_mut()
        .read_to_string()
        .map_err(|e| anyhow::anyhow!("failed to read registry response {url}: {e}"))
}

pub(crate) fn http_get_string_with_auth(
    url: &str,
    auth_token_env: Option<&str>,
) -> anyhow::Result<String> {
    let (host, port, path) = parse_http_url(url)?;
    let mut stream = std::net::TcpStream::connect((host.as_str(), port))
        .map_err(|e| anyhow::anyhow!("failed to connect to registry {host}:{port}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| anyhow::anyhow!("failed to configure registry read timeout: {e}"))?;
    let authorization = registry_authorization_header(auth_token_env)?;
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n");
    if let Some(authorization) = authorization {
        request.push_str("Authorization: ");
        request.push_str(&authorization);
        request.push_str("\r\n");
    }
    request.push_str("Connection: close\r\n\r\n");
    std::io::Write::write_all(&mut stream, request.as_bytes())
        .map_err(|e| anyhow::anyhow!("failed to send registry request {url}: {e}"))?;
    let mut response = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut response)
        .map_err(|e| anyhow::anyhow!("failed to read registry response {url}: {e}"))?;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("registry response missing HTTP header terminator"))?;
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|e| anyhow::anyhow!("registry response headers are not UTF-8: {e}"))?;
    let status = headers.lines().next().unwrap_or_default();
    if !status.starts_with("HTTP/1.1 200") && !status.starts_with("HTTP/1.0 200") {
        anyhow::bail!("registry request {url} failed with {status}");
    }
    String::from_utf8(response[header_end + 4..].to_vec())
        .map_err(|e| anyhow::anyhow!("registry response body is not UTF-8: {e}"))
}

pub(crate) fn registry_authorization_header(
    auth_token_env: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(auth_token_env) = auth_token_env else {
        return Ok(None);
    };
    let token = std::env::var(auth_token_env)
        .map_err(|_| anyhow::anyhow!("registry auth token env `{auth_token_env}` is not set"))?;
    if token.trim().is_empty() {
        anyhow::bail!("registry auth token env `{auth_token_env}` must not be empty");
    }
    if token.contains('\r') || token.contains('\n') {
        anyhow::bail!("registry auth token env `{auth_token_env}` must not contain newlines");
    }
    Ok(Some(format!("Bearer {token}")))
}

pub(crate) fn parse_http_url(url: &str) -> anyhow::Result<(String, u16, String)> {
    let Some(rest) = url.strip_prefix("http://") else {
        anyhow::bail!("registry URL must start with http://");
    };
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, "/"), |(authority, path)| {
            (authority, path.strip_prefix('/').unwrap_or(path))
        });
    if authority.is_empty() {
        anyhow::bail!("registry URL host must not be empty");
    }
    let (host, port) = if let Some((host, port)) = authority.rsplit_once(':') {
        let port = port
            .parse::<u16>()
            .map_err(|e| anyhow::anyhow!("registry URL port must be a u16: {e}"))?;
        (host.to_string(), port)
    } else {
        (authority.to_string(), 80)
    };
    if host.is_empty() {
        anyhow::bail!("registry URL host must not be empty");
    }
    Ok((host, port, format!("/{}", path.trim_start_matches('/'))))
}

pub(crate) fn dependency_cache_component(value: &str) -> String {
    let component = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if component.is_empty() {
        "package".to_string()
    } else {
        component
    }
}

pub(crate) fn cmd_add_dependency(
    path: &Path,
    name: &str,
    version: Option<&str>,
    dev: bool,
    dependency_path: Option<&Path>,
    registry: Option<&str>,
) -> anyhow::Result<()> {
    let manifest_path = project_manifest_path(path)?;
    let mut manifest = read_toml_manifest(&manifest_path)?;
    add_dependency_to_manifest(&mut manifest, name, version, dev, dependency_path, registry)?;
    write_toml_manifest_atomic(&manifest_path, &manifest)?;
    cmd_lock(&manifest_path, false)?;
    println!("dependency: added {} to {}", name, dependency_section(dev));
    Ok(())
}

pub(crate) fn cmd_remove_dependency(path: &Path, name: &str, dev: bool) -> anyhow::Result<()> {
    let manifest_path = project_manifest_path(path)?;
    let mut manifest = read_toml_manifest(&manifest_path)?;
    remove_dependency_from_manifest(&mut manifest, name, dev)?;
    write_toml_manifest_atomic(&manifest_path, &manifest)?;
    cmd_lock(&manifest_path, false)?;
    println!(
        "dependency: removed {} from {}",
        name,
        dependency_section(dev)
    );
    Ok(())
}

pub(crate) fn cmd_run_artifact(path: &Path, trace: Option<&Path>) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    run_artifact_with_writer_with_trace(path, trace, &mut stdout)
}

pub(crate) fn cmd_run_build(dir: &Path, trace: Option<&Path>) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    run_build_with_writer_with_trace(dir, trace, &mut stdout)
}

#[derive(Clone, Copy)]
pub(crate) struct DevOptions {
    pub(crate) hmr: bool,
    pub(crate) watch: bool,
    pub(crate) loop_mode: DevLoopMode,
    pub(crate) serve: Option<DevServeOptions>,
}

#[derive(Clone, Copy)]
pub(crate) struct DevServeOptions {
    pub(crate) port: u16,
    pub(crate) iterations: Option<u64>,
    pub(crate) interval_ms: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum DevLoopMode {
    Once,
    WatchLoop {
        iterations: Option<u64>,
        interval_ms: u64,
    },
}

pub(crate) fn cmd_dev(path: &Path, out: &Path, options: DevOptions) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    if let Some(serve) = options.serve {
        return dev_hmr_serve_with_writer(
            path,
            out,
            serve.port,
            serve.iterations,
            serve.interval_ms,
            &mut stdout,
        );
    }
    if let DevLoopMode::WatchLoop {
        iterations,
        interval_ms,
    } = options.loop_mode
    {
        return dev_watch_loop_with_writer(
            path,
            out,
            options.hmr,
            iterations,
            interval_ms,
            &mut stdout,
        );
    }
    if options.hmr {
        dev_with_writer_with_options(path, out, true, options.watch, &mut stdout)
    } else if options.watch {
        dev_with_writer_with_options(path, out, false, true, &mut stdout)
    } else {
        dev_with_writer(path, out, &mut stdout)
    }
}

pub(crate) struct DevHmrServer {
    pub(crate) addr: SocketAddr,
    pub(crate) shutdown: Arc<AtomicBool>,
    pub(crate) handle: Option<JoinHandle<()>>,
}

impl DevHmrServer {
    pub(crate) const fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for DevHmrServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub(crate) fn dev_hmr_serve_with_writer<W: std::io::Write>(
    path: &Path,
    out: &Path,
    port: u16,
    iterations: Option<u64>,
    interval_ms: u64,
    writer: &mut W,
) -> anyhow::Result<()> {
    validate_dev_loop_options(iterations, interval_ms)?;
    let mut events = Vec::new();
    let mut previous_signature: Option<String> = None;
    let mut server: Option<DevHmrServer> = None;
    let mut iteration = 0_u64;

    loop {
        iteration = iteration.saturating_add(1);
        let reason = dev_watch_loop_reason(out, previous_signature.as_deref())?;
        if reason == "unchanged" {
            events.push(dev_watch_loop_event(iteration, reason, "skip", "ok", None));
        } else {
            dev_with_writer_with_options(path, out, true, true, writer)?;
            let signature = dev_watch_current_source_signature(out)?;
            events.push(dev_watch_loop_event(
                iteration,
                reason,
                "build-verify-run",
                "ok",
                Some(&signature),
            ));
            previous_signature = Some(signature);
        }
        write_dev_watch_events(out, true, interval_ms, &events)?;

        if server.is_none() {
            let spawned = spawn_dev_hmr_server(out, port)?;
            writeln!(writer, "\n[orv dev] hmr server http://{}", spawned.addr())?;
            server = Some(spawned);
        }
        if iterations.is_some_and(|limit| iteration >= limit) {
            break;
        }
        std::thread::sleep(Duration::from_millis(interval_ms));
    }
    drop(server);
    Ok(())
}

pub(crate) fn spawn_dev_hmr_server(out: &Path, port: u16) -> anyhow::Result<DevHmrServer> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| anyhow::anyhow!("failed to bind HMR dev server: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| anyhow::anyhow!("failed to configure HMR dev server: {e}"))?;
    let addr = listener
        .local_addr()
        .map_err(|e| anyhow::anyhow!("failed to read HMR dev server address: {e}"))?;
    write_dev_hmr_server_manifest(out, addr)?;

    let root = out.to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let worker_shutdown = Arc::clone(&shutdown);
    let handle =
        std::thread::spawn(move || dev_hmr_server_loop(&listener, &root, &worker_shutdown));
    Ok(DevHmrServer {
        addr,
        shutdown,
        handle: Some(handle),
    })
}

pub(crate) fn write_dev_hmr_server_manifest(out: &Path, addr: SocketAddr) -> anyhow::Result<()> {
    let server = serde_json::json!({
        "schema_version": 1,
        "mode": "hmr-server",
        "protocol": "http1",
        "address": addr.to_string(),
        "source_bundle": "source-bundle.json",
        "session": "dev/session.json",
        "events": "dev/events.json",
        "endpoints": {
            "session": "/__orv/hmr/session",
            "events": "/__orv/hmr/events",
        },
    });
    write_json(&out.join("dev").join("server.json"), &server)
}

pub(crate) fn dev_hmr_server_loop(
    listener: &std::net::TcpListener,
    out: &Path,
    shutdown: &AtomicBool,
) {
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = handle_dev_hmr_connection(stream, out);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
}

pub(crate) fn handle_dev_hmr_connection(
    mut stream: std::net::TcpStream,
    out: &Path,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") && request.len() < 8192 {
        let read = std::io::Read::read(&mut stream, &mut buffer)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
    }
    let path = dev_hmr_request_path(&request).unwrap_or("/");
    let response = dev_hmr_http_response(out, path)
        .unwrap_or_else(|err| dev_hmr_text_response("500 Internal Server Error", &err.to_string()));
    std::io::Write::write_all(&mut stream, &response)
}

pub(crate) fn dev_hmr_request_path(request: &[u8]) -> Option<&str> {
    let request = std::str::from_utf8(request).ok()?;
    let line = request.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?;
    (method == "GET").then_some(path)
}

pub(crate) fn dev_hmr_http_response(out: &Path, path: &str) -> anyhow::Result<Vec<u8>> {
    match path {
        "/__orv/hmr/session" => {
            let body = std::fs::read_to_string(out.join("dev").join("session.json"))?;
            Ok(dev_hmr_response(
                "200 OK",
                "application/json",
                "no-cache",
                &body,
            ))
        }
        "/__orv/hmr/events" => {
            let events = read_json_value(&out.join("dev").join("events.json"))?;
            let body = dev_hmr_sse_body(&events);
            Ok(dev_hmr_response(
                "200 OK",
                "text/event-stream",
                "no-cache",
                &body,
            ))
        }
        _ => Ok(dev_hmr_text_response("404 Not Found", "not found")),
    }
}

pub(crate) fn dev_hmr_sse_body(events: &serde_json::Value) -> String {
    let mut body = String::new();
    if let Some(items) = events.get("events").and_then(serde_json::Value::as_array) {
        for event in items {
            let data = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
            let _ = write!(body, "event: message\ndata: {data}\n\n");
            if event.get("action").and_then(serde_json::Value::as_str) == Some("build-verify-run") {
                let _ = write!(body, "event: orv:reload\ndata: {data}\n\n");
            }
        }
    }
    body
}

pub(crate) fn dev_hmr_text_response(status: &str, body: &str) -> Vec<u8> {
    dev_hmr_response(status, "text/plain; charset=utf-8", "no-cache", body)
}

pub(crate) fn dev_hmr_response(
    status: &str,
    content_type: &str,
    cache_control: &str,
    body: &str,
) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nCache-Control: {cache_control}\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

pub(crate) fn dev_watch_loop_with_writer<W: std::io::Write>(
    path: &Path,
    out: &Path,
    hmr: bool,
    iterations: Option<u64>,
    interval_ms: u64,
    writer: &mut W,
) -> anyhow::Result<()> {
    validate_dev_loop_options(iterations, interval_ms)?;

    let mut events = Vec::new();
    let mut previous_signature: Option<String> = None;
    let mut iteration = 0_u64;
    loop {
        iteration = iteration.saturating_add(1);
        let reason = dev_watch_loop_reason(out, previous_signature.as_deref())?;
        if reason == "unchanged" {
            events.push(dev_watch_loop_event(iteration, reason, "skip", "ok", None));
        } else {
            dev_with_writer_with_options(path, out, hmr, true, writer)?;
            let signature = dev_watch_current_source_signature(out)?;
            events.push(dev_watch_loop_event(
                iteration,
                reason,
                "build-verify-run",
                "ok",
                Some(&signature),
            ));
            previous_signature = Some(signature);
        }
        write_dev_watch_events(out, hmr, interval_ms, &events)?;

        if iterations.is_some_and(|limit| iteration >= limit) {
            break;
        }
        std::thread::sleep(Duration::from_millis(interval_ms));
    }
    Ok(())
}

pub(crate) fn validate_dev_loop_options(
    iterations: Option<u64>,
    interval_ms: u64,
) -> anyhow::Result<()> {
    if interval_ms == 0 {
        anyhow::bail!("watch loop interval_ms must be positive");
    }
    if iterations == Some(0) {
        anyhow::bail!("watch loop iterations must be positive");
    }
    Ok(())
}

pub(crate) fn dev_watch_loop_reason(
    out: &Path,
    previous_signature: Option<&str>,
) -> anyhow::Result<&'static str> {
    let Some(signature) = previous_signature else {
        return Ok("initial");
    };
    let current = dev_watch_current_source_signature(out)?;
    if current == signature {
        Ok("unchanged")
    } else {
        Ok("changed")
    }
}

pub(crate) fn dev_watch_loop_event(
    iteration: u64,
    reason: &str,
    action: &str,
    status: &str,
    source_signature: Option<&str>,
) -> serde_json::Value {
    let mut event = serde_json::json!({
        "iteration": iteration,
        "reason": reason,
        "action": action,
        "status": status,
        "watch": "dev/watch.json",
    });
    if let Some(signature) = source_signature {
        event["source_signature"] = serde_json::json!(signature);
    }
    event
}

pub(crate) fn write_dev_watch_events(
    out: &Path,
    hmr: bool,
    interval_ms: u64,
    events: &[serde_json::Value],
) -> anyhow::Result<()> {
    let value = serde_json::json!({
        "schema_version": 1,
        "mode": "watch-loop",
        "source_bundle": "source-bundle.json",
        "loop": {
            "strategy": "poll",
            "interval_ms": interval_ms,
            "run": "build-verify-run",
            "hmr": hmr,
        },
        "transport": {
            "kind": "manifest",
            "path": "dev/events.json",
        },
        "events": events,
    });
    write_json(&out.join("dev").join("events.json"), &value)
}

pub(crate) fn dev_watch_current_source_signature(out: &Path) -> anyhow::Result<String> {
    let session = read_json_value(&out.join("dev").join("watch.json"))?;
    let sources = session
        .pointer("/watch/sources")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("dev watch session watch.sources must be an array"))?;
    let mut current = Vec::with_capacity(sources.len());
    for source in sources {
        let path = json_str(source, "path", "dev watch source")?;
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("failed to read watched source {path}: {e}"))?;
        current.push(serde_json::json!({
            "path": path,
            "content_hash": format!("fnv1a64:{:016x}", fnv1a64(&bytes)),
        }));
    }
    stable_json_hash(&serde_json::Value::Array(current))
}

pub(crate) fn dev_with_writer<W: std::io::Write>(
    path: &Path,
    out: &Path,
    writer: &mut W,
) -> anyhow::Result<()> {
    dev_with_writer_with_options(path, out, false, false, writer)
}

pub(crate) fn dev_with_writer_with_options<W: std::io::Write>(
    path: &Path,
    out: &Path,
    hmr: bool,
    watch: bool,
    writer: &mut W,
) -> anyhow::Result<()> {
    cmd_build(path, out)?;
    verify_build_dir(out)?;
    if hmr {
        write_dev_hmr_session(out)?;
        write_dev_hmr_transport(out)?;
    }
    if watch {
        write_dev_watch_session(out, hmr)?;
    }
    run_build_with_writer(out, writer)
}

pub(crate) fn write_dev_hmr_session(out: &Path) -> anyhow::Result<()> {
    let (sources, targets, has_client_target) = dev_session_inputs(out)?;
    let session = serde_json::json!({
        "schema_version": 1,
        "mode": "hmr",
        "source_bundle": "source-bundle.json",
        "watch": {
            "sources": sources,
            "targets": targets,
        },
        "reload": {
            "strategy": if has_client_target { "hot-reload" } else { "full-reload" },
            "fallback": "full-reload",
            "state": if has_client_target { "preserve-sig-state-when-compatible" } else { "stateless" },
        },
    });
    write_json(&out.join("dev").join("session.json"), &session)
}

pub(crate) fn write_dev_hmr_transport(out: &Path) -> anyhow::Result<()> {
    let transport = serde_json::json!({
        "schema_version": 1,
        "mode": "hmr-transport",
        "source_bundle": "source-bundle.json",
        "session": "dev/session.json",
        "browser": {
            "kind": "event-source",
            "client": "dev/hmr-client.js",
            "event_source": "/__orv/hmr/events",
            "session": "/__orv/hmr/session",
            "reload_event": "orv:reload",
        },
        "server": {
            "kind": "reference-dev",
            "events": "dev/events.json",
            "session": "dev/session.json",
        },
    });
    write_json(&out.join("dev").join("transport.json"), &transport)?;
    write_text(&out.join("dev").join("hmr-client.js"), DEV_HMR_CLIENT_JS)
}

pub(crate) const DEV_HMR_CLIENT_JS: &str = r"(function () {
  if (!('EventSource' in window)) {
    return;
  }
  var source = new EventSource('/__orv/hmr/events');
  source.addEventListener('message', function (event) {
    var payload = {};
    try {
      payload = JSON.parse(event.data || '{}');
    } catch (_) {
      payload = {};
    }
    window.dispatchEvent(new CustomEvent('orv:hmr', { detail: payload }));
    if (payload.action === 'build-verify-run' || payload.action === 'reload') {
      window.location.reload();
    }
  });
  source.addEventListener('orv:reload', function () {
    window.location.reload();
  });
}());
";

pub(crate) fn write_dev_watch_session(out: &Path, hmr: bool) -> anyhow::Result<()> {
    let (sources, targets, has_client_target) = dev_session_inputs(out)?;
    let session = serde_json::json!({
        "schema_version": 1,
        "mode": "watch",
        "source_bundle": "source-bundle.json",
        "watch": {
            "sources": sources,
            "targets": targets,
        },
        "loop": {
            "strategy": "poll",
            "interval_ms": 500,
            "run": "build-verify-run",
            "hmr": hmr,
        },
        "reload": {
            "strategy": if hmr && has_client_target { "hot-reload" } else { "full-reload" },
            "fallback": "full-reload",
            "state": if hmr && has_client_target { "preserve-sig-state-when-compatible" } else { "stateless" },
        },
        "transport": {
            "kind": "manifest",
            "path": "dev/watch.json",
        },
    });
    write_json(&out.join("dev").join("watch.json"), &session)
}

pub(crate) fn dev_session_inputs(
    out: &Path,
) -> anyhow::Result<(Vec<serde_json::Value>, Vec<serde_json::Value>, bool)> {
    let source_bundle = read_json_value(&out.join("source-bundle.json"))?;
    let bundle_plan = read_json_value(&out.join("bundle-plan.json"))?;
    let sources = source_bundle
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("source-bundle.json files must be an array"))?
        .iter()
        .map(|source| {
            Ok(serde_json::json!({
                "path": json_string_field(source, "path", "source bundle file")?,
                "content_hash": json_string_field(source, "content_hash", "source bundle file")?,
            }))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let targets = bundle_plan
        .get("bundles")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("bundle-plan.json bundles must be an array"))?
        .iter()
        .map(|target| {
            let runtime_features = target
                .get("runtime_features")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    anyhow::anyhow!("bundle target runtime_features must be an array")
                })?;
            Ok(serde_json::json!({
                "kind": json_string_field(target, "kind", "bundle target")?,
                "path": json_string_field(target, "path", "bundle target")?,
                "runtime_features": runtime_features,
            }))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let has_client_target = targets.iter().any(|target| {
        target
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_client_bundle_kind)
    });
    Ok((sources, targets, has_client_target))
}

pub(crate) fn json_string_field<'a>(
    value: &'a serde_json::Value,
    field: &str,
    context: &str,
) -> anyhow::Result<&'a str> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("{context} {field} must be a string"))
}

pub(crate) fn run_build_with_writer<W: std::io::Write>(
    dir: &Path,
    writer: &mut W,
) -> anyhow::Result<()> {
    run_build_with_writer_with_trace(dir, None, writer)
}

pub(crate) fn run_build_with_writer_with_trace<W: std::io::Write>(
    dir: &Path,
    trace: Option<&Path>,
    writer: &mut W,
) -> anyhow::Result<()> {
    let build_dir = dir
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("failed to resolve build dir {}: {e}", dir.display()))?;
    let plan_path = build_dir.join("bundle-plan.json");
    if plan_path.is_file() {
        let plan = read_json_value(&plan_path)?;
        if let Some(launcher) = bundle_target_path(&plan, "server_launcher")? {
            let launch_path = build_dir.join(launcher);
            verify_server_launcher_target(&build_dir, &launch_path)?;
            let launch = read_server_launch_artifact(&launch_path)?;
            return run_artifact_with_writer_with_build_dir(
                &build_dir.join(launch.artifact),
                &build_dir,
                trace,
                writer,
            );
        }
        return run_static_build_with_writer(&build_dir, writer);
    }
    let launch_path = build_dir.join("server").join("launch.json");
    if launch_path.is_file() {
        verify_server_launcher_target(&build_dir, &launch_path)?;
        let launch = read_server_launch_artifact(&launch_path)?;
        return run_artifact_with_writer_with_build_dir(
            &build_dir.join(launch.artifact),
            &build_dir,
            trace,
            writer,
        );
    }
    run_static_build_with_writer(&build_dir, writer)
}

pub(crate) fn bundle_target_path(
    plan: &serde_json::Value,
    kind: &str,
) -> anyhow::Result<Option<String>> {
    let bundles = plan
        .get("bundles")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("bundle plan bundles must be an array"))?;
    for bundle in bundles {
        if bundle.get("kind").and_then(serde_json::Value::as_str) == Some(kind) {
            return Ok(Some(json_str(bundle, "path", "bundle target")?.to_string()));
        }
    }
    Ok(None)
}

pub(crate) fn run_static_build_with_writer<W: std::io::Write>(
    dir: &Path,
    writer: &mut W,
) -> anyhow::Result<()> {
    let plan = read_json_value(&dir.join("bundle-plan.json"))?;
    let bundles = plan
        .get("bundles")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("bundle plan bundles must be an array"))?;
    if let Some(bundle) = bundles.iter().find(|bundle| {
        bundle.get("kind").and_then(serde_json::Value::as_str) == Some("static_page")
    }) {
        let path = json_str(bundle, "path", "bundle target")?;
        let target = dir.join(path);
        verify_static_page_target(bundle, &target)?;
        let html = std::fs::read_to_string(&target)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
        writer.write_all(html.as_bytes())?;
        return Ok(());
    }
    let bundle = bundles
        .iter()
        .find(|bundle| {
            bundle.get("kind").and_then(serde_json::Value::as_str) == Some("client_page")
        })
        .ok_or_else(|| anyhow::anyhow!("build has no server launcher or page target"))?;
    let path = json_str(bundle, "path", "bundle target")?;
    let target = dir.join(path);
    verify_client_page_target(bundle, &target)?;
    let html = std::fs::read_to_string(&target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    writer.write_all(html.as_bytes())?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn run_artifact_with_writer<W: std::io::Write>(
    path: &Path,
    writer: &mut W,
) -> anyhow::Result<()> {
    run_artifact_with_writer_with_trace(path, None, writer)
}

pub(crate) fn run_artifact_with_writer_with_trace<W: std::io::Write>(
    path: &Path,
    trace: Option<&Path>,
    writer: &mut W,
) -> anyhow::Result<()> {
    let options = orv_runtime::RuntimeOptions {
        request_trace_path: trace.map(Path::to_path_buf),
        ..orv_runtime::RuntimeOptions::default()
    };
    run_artifact_with_writer_with_options(path, writer, options)
}

pub(crate) fn run_artifact_with_writer_with_build_dir<W: std::io::Write>(
    path: &Path,
    build_dir: &Path,
    trace: Option<&Path>,
    writer: &mut W,
) -> anyhow::Result<()> {
    let options = orv_runtime::RuntimeOptions {
        request_trace_path: trace.map(|path| build_runtime_path(build_dir, path)),
        working_dir: Some(build_dir.to_path_buf()),
    };
    run_artifact_with_writer_with_options(path, writer, options)
}

pub(crate) fn build_runtime_path(build_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        build_dir.join(path)
    }
}

pub(crate) fn run_artifact_with_writer_with_options<W: std::io::Write>(
    path: &Path,
    writer: &mut W,
    options: orv_runtime::RuntimeOptions,
) -> anyhow::Result<()> {
    let artifact = read_server_artifact(path)?;
    orv_compiler::verify_server_runtime_artifact(&artifact)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    let lowered = lower_artifact_entry(&artifact)?;
    orv_runtime::run_with_writer_with_options(&lowered.program, writer, options)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

pub(crate) fn read_server_artifact(
    path: &Path,
) -> anyhow::Result<orv_compiler::ServerRuntimeArtifact> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))
}

pub(crate) fn read_server_launch_artifact(
    path: &Path,
) -> anyhow::Result<orv_compiler::ServerLaunchArtifact> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))
}

pub(crate) fn lower_artifact_entry(
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<orv_analyzer::LowerResult> {
    let entry = artifact_entry_path(artifact)?;
    let loaded = orv_project::load_project_from_sources(
        &entry,
        artifact
            .source_bundle
            .files
            .iter()
            .map(|file| (PathBuf::from(&file.path), file.source.clone())),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    report_diagnostics(&loaded.diagnostics, &loaded.files)?;
    let resolved = orv_resolve::resolve(&loaded.program);
    report_diagnostics(&resolved.diagnostics, &loaded.files)?;
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    report_diagnostics(&lowered.diagnostics, &loaded.files)?;
    Ok(lowered)
}

pub(crate) fn lower_source_bundle_entry(
    artifact: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<orv_analyzer::LowerResult> {
    let loaded = load_project_from_source_bundle_artifact(artifact)?;
    report_diagnostics(&loaded.diagnostics, &loaded.files)?;
    let resolved = orv_resolve::resolve(&loaded.program);
    report_diagnostics(&resolved.diagnostics, &loaded.files)?;
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    report_diagnostics(&lowered.diagnostics, &loaded.files)?;
    Ok(lowered)
}

pub(crate) fn load_project_from_source_bundle_artifact(
    artifact: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<orv_project::LoadedProject> {
    let entry = source_bundle_entry_path(artifact)?;
    orv_project::load_project_from_sources(
        &entry,
        artifact
            .files
            .iter()
            .map(|file| (PathBuf::from(&file.path), file.source.clone())),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

pub(crate) fn artifact_entry_path(
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<PathBuf> {
    let entry = normalized_artifact_path(&artifact.entry);
    if let Some(file) = artifact.source_bundle.files.iter().find(|file| {
        let path = normalized_artifact_path(&file.path);
        path == entry || path.ends_with(&entry)
    }) {
        return Ok(PathBuf::from(&file.path));
    }
    if artifact.source_bundle.files.len() == 1 {
        return Ok(PathBuf::from(&artifact.source_bundle.files[0].path));
    }
    anyhow::bail!("entry source `{}` not found in artifact", artifact.entry)
}

pub(crate) fn source_bundle_entry_path(
    artifact: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<PathBuf> {
    let entry = normalized_artifact_path(&artifact.entry);
    if let Some(file) = artifact.files.iter().find(|file| {
        let path = normalized_artifact_path(&file.path);
        path == entry || path.ends_with(&entry)
    }) {
        return Ok(PathBuf::from(&file.path));
    }
    if artifact.files.len() == 1 {
        return Ok(PathBuf::from(&artifact.files[0].path));
    }
    anyhow::bail!(
        "entry source `{}` not found in source bundle",
        artifact.entry
    )
}

pub(crate) fn normalized_artifact_path(path: &str) -> String {
    path.replace('\\', "/")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuildProfile {
    Development,
    Production,
}

impl BuildProfile {
    pub(crate) const fn from_prod_flag(prod: bool) -> Self {
        if prod {
            Self::Production
        } else {
            Self::Development
        }
    }

    const fn is_production(self) -> bool {
        matches!(self, Self::Production)
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Development => "dev",
            Self::Production => "prod",
        }
    }
}

pub(crate) fn cmd_build(path: &Path, out: &Path) -> anyhow::Result<()> {
    cmd_build_with_profile(path, out, BuildProfile::Development)
}

pub(crate) fn cmd_build_with_profile(
    path: &Path,
    out: &Path,
    profile: BuildProfile,
) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let loaded = orv_project::load_project(&entry).map_err(|e| anyhow::anyhow!("{e}"))?;
    report_diagnostics(&loaded.diagnostics, &loaded.files)?;
    let resolved = orv_resolve::resolve(&loaded.program);
    report_diagnostics(&resolved.diagnostics, &loaded.files)?;
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    report_diagnostics(&lowered.diagnostics, &loaded.files)?;
    let origin_map = orv_compiler::origin_map(&lowered.program);
    let graph = project_graph_json(&loaded.graph, &origin_map);
    let manifest = orv_compiler::build_manifest(entry.display().to_string(), &origin_map);
    let bundle_plan = orv_compiler::bundle_plan(&manifest);
    let client_manifest_path = bundle_output_path(&bundle_plan, "client_manifest");
    let client_reactive_plan_path = bundle_output_path(&bundle_plan, "client_reactive_plan");
    let client_page_path = bundle_output_path(&bundle_plan, "client_page");
    let client_js_path = bundle_output_path(&bundle_plan, "client_js");
    let client_wasm_path = bundle_output_path(&bundle_plan, "client_wasm");
    let static_page = bundle_plan
        .bundles
        .iter()
        .find(|bundle| bundle.kind == "static_page")
        .map(|bundle| {
            render_static_page(&lowered).map(|html| (PathBuf::from(bundle.path.clone()), html))
        })
        .transpose()?;
    let static_page_path = static_page
        .as_ref()
        .map(|(path, _)| normalized_artifact_path(&path.display().to_string()));
    let server_artifact_path = SERVER_ARTIFACT_PATH;
    let server_launch_path = SERVER_LAUNCH_PATH;
    let native_server_plan_path = NATIVE_SERVER_PLAN_PATH;
    let native_runtime_image_plan_path = NATIVE_RUNTIME_IMAGE_PLAN_PATH;
    let native_runtime_image_dockerfile_path = NATIVE_RUNTIME_IMAGE_DOCKERFILE_PATH;
    let native_server_source_path = NATIVE_SERVER_SOURCE_PATH;
    let native_server_routes_source_path = NATIVE_SERVER_ROUTES_SOURCE_PATH;
    let native_server_router_source_path = NATIVE_SERVER_ROUTER_SOURCE_PATH;
    let native_server_handlers_source_path = NATIVE_SERVER_HANDLERS_SOURCE_PATH;
    let native_server_package_path = NATIVE_SERVER_PACKAGE_PATH;
    let source_bundle = orv_compiler::source_bundle_artifact(
        entry.display().to_string(),
        loaded
            .files
            .iter()
            .map(|file| (file.path.display().to_string(), file.source.clone())),
    );
    let server_artifact = manifest.capabilities.has_server.then(|| {
        orv_compiler::server_runtime_artifact_with_program(
            &manifest,
            &origin_map,
            &lowered.program,
            loaded
                .files
                .iter()
                .map(|file| (file.path.display().to_string(), file.source.clone())),
        )
    });
    if profile.is_production() {
        validate_prod_server_listen(server_artifact.as_ref())?;
    }

    std::fs::create_dir_all(out)
        .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", out.display()))?;
    write_json(
        &out.join("build-manifest.json"),
        &serde_json::to_value(&manifest)?,
    )?;
    write_json(
        &out.join("bundle-plan.json"),
        &serde_json::to_value(&bundle_plan)?,
    )?;
    write_json(
        &out.join("origin-map.json"),
        &serde_json::to_value(&origin_map)?,
    )?;
    write_json(&out.join("project-graph.json"), &graph)?;
    let source_bundle_value = serde_json::to_value(&source_bundle)?;
    let source_bundle_hash = stable_json_hash(&source_bundle_value)?;
    write_json(&out.join("source-bundle.json"), &source_bundle_value)?;
    let client_initial_render = if manifest.capabilities.client_wasm {
        Some(render_static_page(&lowered)?)
    } else {
        None
    };
    if let Some(server_artifact) = &server_artifact {
        write_json(
            &out.join(server_artifact_path),
            &serde_json::to_value(server_artifact)?,
        )?;
        let launch = orv_compiler::server_launch_artifact(server_artifact_path, server_artifact);
        write_json(
            &out.join(server_launch_path),
            &serde_json::to_value(launch)?,
        )?;
        let native_server_paths = NativeServerPlanPaths {
            plan: native_server_plan_path,
            artifact: server_artifact_path,
            launcher: server_launch_path,
            source: native_server_source_path,
            routes_source: native_server_routes_source_path,
            router_source: native_server_router_source_path,
            handlers_source: native_server_handlers_source_path,
            package: native_server_package_path,
            runtime_image_plan: native_runtime_image_plan_path,
        };
        write_native_server_plan_artifact(out, &native_server_paths, server_artifact)?;
        write_native_runtime_image_plan_artifact(
            out,
            native_runtime_image_plan_path,
            native_runtime_image_dockerfile_path,
            server_artifact_path,
            native_server_plan_path,
            server_artifact,
        )?;
        write_native_runtime_image_dockerfile(out, native_runtime_image_dockerfile_path)?;
        write_native_server_launcher_source(
            out,
            native_server_source_path,
            server_artifact_path,
            native_server_plan_path,
            server_artifact,
        )?;
        write_native_server_routes_source(out, native_server_routes_source_path, server_artifact)?;
        write_native_server_router_source(out, native_server_router_source_path)?;
        write_native_server_handlers_source(
            out,
            native_server_handlers_source_path,
            server_artifact,
        )?;
        write_native_server_launcher_package(out, native_server_package_path)?;
    }
    if let Some((path, html)) = static_page {
        write_text(&out.join(path), &html)?;
    }
    let client_source_binding = ClientSourceBinding {
        source_bundle: &source_bundle,
        source_bundle_hash: &source_bundle_hash,
        origin_map: &origin_map,
        program: &lowered.program,
        initial_render: client_initial_render.as_deref().unwrap_or(""),
    };
    let client_bundle_targets = ClientBundleTargets {
        manifest: client_manifest_path.as_deref(),
        reactive_plan: client_reactive_plan_path.as_deref(),
        page: client_page_path.as_deref(),
        js: client_js_path.as_deref(),
        wasm: client_wasm_path.as_deref(),
    };
    write_client_bundle_artifacts(
        out,
        &entry,
        manifest.capabilities.client_wasm,
        &client_source_binding,
        &client_bundle_targets,
    )?;
    if profile.is_production() {
        write_prod_deploy_artifacts(
            out,
            &entry,
            &manifest,
            &origin_map,
            server_artifact.as_ref(),
            ProdBuildTargets {
                static_page: static_page_path.as_deref(),
                client_manifest: client_manifest_path.as_deref(),
                client_reactive_plan: client_reactive_plan_path.as_deref(),
                client_page: client_page_path.as_deref(),
                client_js: client_js_path.as_deref(),
                client_wasm: client_wasm_path.as_deref(),
                server_artifact: server_artifact_path,
                native_server_plan: native_server_plan_path,
                native_runtime_image_plan: native_runtime_image_plan_path,
                native_server_routes_source: native_server_routes_source_path,
                native_server_router_source: native_server_router_source_path,
                native_server_handlers_source: native_server_handlers_source_path,
            },
        )?;
    }
    println!("build: wrote {}", out.display());
    Ok(())
}

pub(crate) fn write_client_bundle_artifacts(
    out: &Path,
    entry: &Path,
    enabled: bool,
    binding: &ClientSourceBinding<'_>,
    targets: &ClientBundleTargets<'_>,
) -> anyhow::Result<()> {
    if !enabled {
        return Ok(());
    }
    let page_path = targets
        .page
        .ok_or_else(|| anyhow::anyhow!("missing client_page bundle target"))?;
    let manifest_path = targets
        .manifest
        .ok_or_else(|| anyhow::anyhow!("missing client_manifest bundle target"))?;
    let reactive_plan_path = targets
        .reactive_plan
        .ok_or_else(|| anyhow::anyhow!("missing client_reactive_plan bundle target"))?;
    let js_path = targets
        .js
        .ok_or_else(|| anyhow::anyhow!("missing client_js bundle target"))?;
    let wasm_path = targets
        .wasm
        .ok_or_else(|| anyhow::anyhow!("missing client_wasm bundle target"))?;
    write_client_wasm_bundle(
        &out.join(wasm_path),
        binding.source_bundle,
        binding.source_bundle_hash,
        binding.initial_render,
    )?;
    write_client_js_loader(&out.join(js_path), entry, binding)?;
    let loader_src = relative_bundle_path(page_path, js_path);
    write_client_page_shell(&out.join(page_path), entry, &loader_src)?;
    write_client_reactive_plan(out, reactive_plan_path, entry, binding)?;
    write_client_bundle_manifest(out, manifest_path, entry, binding, targets)
}

pub(crate) struct ClientSourceBinding<'a> {
    pub(crate) source_bundle: &'a orv_compiler::SourceBundleArtifact,
    pub(crate) source_bundle_hash: &'a str,
    pub(crate) origin_map: &'a orv_compiler::OriginMap,
    pub(crate) program: &'a orv_hir::HirProgram,
    pub(crate) initial_render: &'a str,
}

pub(crate) struct ClientBundleTargets<'a> {
    pub(crate) manifest: Option<&'a str>,
    pub(crate) reactive_plan: Option<&'a str>,
    pub(crate) page: Option<&'a str>,
    pub(crate) js: Option<&'a str>,
    pub(crate) wasm: Option<&'a str>,
}

pub(crate) fn write_client_reactive_plan(
    out: &Path,
    path: &str,
    entry: &Path,
    binding: &ClientSourceBinding<'_>,
) -> anyhow::Result<()> {
    let plan = client_reactive_plan_json(entry, binding);
    write_json(&out.join(path), &plan)
}

pub(crate) fn client_reactive_plan_json(
    entry: &Path,
    binding: &ClientSourceBinding<'_>,
) -> serde_json::Value {
    let signals = client_reactive_plan_signals(binding);
    let mut bindings = vec![serde_json::json!({
        "kind": "initial_render",
        "target": CLIENT_PAGE_PATH,
        "source": CLIENT_WASM_PATH,
        "html_hash": format!("{:016x}", fnv1a64(binding.initial_render.as_bytes())),
        "byte_length": binding.initial_render.len(),
    })];
    bindings.extend(signals.iter().map(|signal| {
        serde_json::json!({
            "kind": "signal_state",
            "target": CLIENT_JS_PATH,
            "source": signal["origin_id"].clone(),
            "state_key": signal["state_key"].clone(),
        })
    }));
    bindings.extend(client_reactive_dom_bindings(binding));
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.client.reactive_plan",
        "entry": entry.display().to_string(),
        "source_bundle": SOURCE_BUNDLE_PATH,
        "source_bundle_hash": binding.source_bundle_hash,
        "runtime_features": ["client_wasm"],
        "signals": signals,
        "bindings": bindings,
        "blocked_by": ["reactive-dom-diff"],
        "blockers": client_reactive_plan_blockers_json(),
    })
}

pub(crate) fn client_reactive_plan_blockers_json() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "id": "reactive-dom-diff",
        "artifact": CLIENT_REACTIVE_PLAN_PATH,
        "reason": "full DOM diff codegen is not emitted yet",
    })]
}

pub(crate) fn client_reactive_plan_signals(
    binding: &ClientSourceBinding<'_>,
) -> Vec<serde_json::Value> {
    let initial_values = client_signal_initial_values(binding.program);
    binding
        .origin_map
        .entries
        .iter()
        .filter(|entry| entry.kind == "signal")
        .map(|signal| {
            serde_json::json!({
                "name": &signal.name,
                "origin_id": &signal.id,
                "state_key": &signal.name,
                "initial_value": initial_values
                    .get(&signal.id)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"kind": "dynamic"})),
                "span": {
                    "file": signal.span.file,
                    "start": signal.span.start,
                    "end": signal.span.end,
                },
            })
        })
        .collect()
}

#[derive(Clone, Debug)]
pub(crate) struct ClientSignalDomSource {
    pub(crate) origin_id: String,
    pub(crate) state_key: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ClientSignalTextBinding {
    pub(crate) origin_id: String,
    pub(crate) state_key: String,
    pub(crate) text_template: Option<Vec<serde_json::Value>>,
    pub(crate) text_condition: Option<serde_json::Value>,
    pub(crate) signal_sources: Vec<ClientSignalDomSource>,
}

#[derive(Clone, Debug)]
pub(crate) struct ClientSignalAttrBinding {
    pub(crate) origin_id: String,
    pub(crate) state_key: String,
    pub(crate) attr_template: Option<Vec<serde_json::Value>>,
    pub(crate) attr_condition: Option<serde_json::Value>,
    pub(crate) signal_sources: Vec<ClientSignalDomSource>,
}

pub(crate) fn client_reactive_dom_bindings(
    binding: &ClientSourceBinding<'_>,
) -> Vec<serde_json::Value> {
    let signals = client_signal_dom_sources(binding.program);
    let mut bindings = Vec::new();
    for stmt in &binding.program.items {
        collect_client_dom_bindings_stmt(stmt, false, &signals, &mut bindings);
    }
    bindings
}

pub(crate) fn client_signal_dom_sources(
    program: &orv_hir::HirProgram,
) -> HashMap<orv_hir::NameId, ClientSignalDomSource> {
    program
        .items
        .iter()
        .filter_map(|stmt| {
            let orv_hir::HirStmt::Let(stmt) = stmt else {
                return None;
            };
            (stmt.kind == orv_hir::HirLetKind::Signal).then(|| {
                (
                    stmt.name.id,
                    ClientSignalDomSource {
                        origin_id: orv_hir::origin_id("signal", &stmt.name.name, stmt.span),
                        state_key: stmt.name.name.clone(),
                    },
                )
            })
        })
        .collect()
}

pub(crate) fn collect_client_dom_bindings_stmt(
    stmt: &orv_hir::HirStmt,
    inside_html: bool,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
    out: &mut Vec<serde_json::Value>,
) {
    match stmt {
        orv_hir::HirStmt::Let(stmt) => {
            collect_client_dom_bindings_expr(&stmt.init, inside_html, signals, out);
        }
        orv_hir::HirStmt::Const(stmt) => {
            collect_client_dom_bindings_expr(&stmt.init, inside_html, signals, out);
        }
        orv_hir::HirStmt::Function(stmt) => {
            collect_client_dom_bindings_function_body(&stmt.body, inside_html, signals, out);
        }
        orv_hir::HirStmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_client_dom_bindings_expr(value, inside_html, signals, out);
            }
        }
        orv_hir::HirStmt::Expr(expr) => {
            collect_client_dom_bindings_expr(expr, inside_html, signals, out);
        }
        orv_hir::HirStmt::Struct(_)
        | orv_hir::HirStmt::Enum(_)
        | orv_hir::HirStmt::TypeAlias(_)
        | orv_hir::HirStmt::Import(_) => {}
    }
}

pub(crate) fn collect_client_dom_bindings_function_body(
    body: &orv_hir::HirFunctionBody,
    inside_html: bool,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
    out: &mut Vec<serde_json::Value>,
) {
    match body {
        orv_hir::HirFunctionBody::Block(block) => {
            collect_client_dom_bindings_block(block, inside_html, signals, out);
        }
        orv_hir::HirFunctionBody::Expr(expr) => {
            collect_client_dom_bindings_expr(expr, inside_html, signals, out);
        }
    }
}

pub(crate) fn collect_client_dom_bindings_block(
    block: &orv_hir::HirBlock,
    inside_html: bool,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
    out: &mut Vec<serde_json::Value>,
) {
    for stmt in &block.stmts {
        collect_client_dom_bindings_stmt(stmt, inside_html, signals, out);
    }
}

pub(crate) fn collect_client_dom_bindings_expr(
    expr: &orv_hir::HirExpr,
    inside_html: bool,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
    out: &mut Vec<serde_json::Value>,
) {
    match &expr.kind {
        orv_hir::HirExprKind::Html(block) => {
            collect_client_dom_bindings_block(block, true, signals, out);
        }
        orv_hir::HirExprKind::Domain { name, args, .. } => {
            if inside_html {
                collect_client_dom_bindings_for_tag(name, args, signals, out);
                collect_client_attr_bindings_for_tag(name, args, signals, out);
                collect_client_event_bindings_for_tag(name, args, signals, out);
            }
            for arg in args {
                collect_client_dom_bindings_expr(arg, inside_html, signals, out);
            }
        }
        orv_hir::HirExprKind::Block(block) => {
            collect_client_dom_bindings_block(block, inside_html, signals, out);
        }
        orv_hir::HirExprKind::Out(inner)
        | orv_hir::HirExprKind::Unary { expr: inner, .. }
        | orv_hir::HirExprKind::Paren(inner)
        | orv_hir::HirExprKind::Throw(inner)
        | orv_hir::HirExprKind::Await(inner)
        | orv_hir::HirExprKind::Cast { expr: inner, .. } => {
            collect_client_dom_bindings_expr(inner, inside_html, signals, out);
        }
        orv_hir::HirExprKind::Binary { lhs, rhs, .. }
        | orv_hir::HirExprKind::Range {
            start: lhs,
            end: rhs,
            ..
        } => {
            collect_client_dom_bindings_expr(lhs, inside_html, signals, out);
            collect_client_dom_bindings_expr(rhs, inside_html, signals, out);
        }
        orv_hir::HirExprKind::String(segments) => {
            for segment in segments {
                if let orv_hir::HirStringSegment::Interp(expr) = segment {
                    collect_client_dom_bindings_expr(expr, inside_html, signals, out);
                }
            }
        }
        orv_hir::HirExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            collect_client_dom_bindings_expr(cond, inside_html, signals, out);
            collect_client_dom_bindings_block(then, inside_html, signals, out);
            if let Some(else_branch) = else_branch {
                collect_client_dom_bindings_expr(else_branch, inside_html, signals, out);
            }
        }
        _ => collect_client_dom_bindings_nested_expr(expr, inside_html, signals, out),
    }
}

pub(crate) fn collect_client_dom_bindings_nested_expr(
    expr: &orv_hir::HirExpr,
    inside_html: bool,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
    out: &mut Vec<serde_json::Value>,
) {
    match &expr.kind {
        orv_hir::HirExprKind::Assign { value, .. } => {
            collect_client_dom_bindings_expr(value, inside_html, signals, out);
        }
        orv_hir::HirExprKind::AssignField { object, value, .. } => {
            collect_client_dom_bindings_expr(object, inside_html, signals, out);
            collect_client_dom_bindings_expr(value, inside_html, signals, out);
        }
        orv_hir::HirExprKind::AssignIndex {
            object,
            index,
            value,
        } => {
            collect_client_dom_bindings_expr(object, inside_html, signals, out);
            collect_client_dom_bindings_expr(index, inside_html, signals, out);
            collect_client_dom_bindings_expr(value, inside_html, signals, out);
        }
        orv_hir::HirExprKind::Call { callee, args } => {
            collect_client_dom_bindings_expr(callee, inside_html, signals, out);
            for arg in args {
                collect_client_dom_bindings_expr(arg, inside_html, signals, out);
            }
        }
        orv_hir::HirExprKind::For { iter, body, .. } => {
            collect_client_dom_bindings_expr(iter, inside_html, signals, out);
            collect_client_dom_bindings_block(body, inside_html, signals, out);
        }
        orv_hir::HirExprKind::While { cond, body } => {
            collect_client_dom_bindings_expr(cond, inside_html, signals, out);
            collect_client_dom_bindings_block(body, inside_html, signals, out);
        }
        orv_hir::HirExprKind::Array(items) | orv_hir::HirExprKind::Tuple(items) => {
            for item in items {
                collect_client_dom_bindings_expr(item, inside_html, signals, out);
            }
        }
        orv_hir::HirExprKind::Object(fields) | orv_hir::HirExprKind::TypedObject { fields, .. } => {
            for field in fields {
                collect_client_dom_bindings_expr(&field.value, inside_html, signals, out);
            }
        }
        orv_hir::HirExprKind::Index { target, index } => {
            collect_client_dom_bindings_expr(target, inside_html, signals, out);
            collect_client_dom_bindings_expr(index, inside_html, signals, out);
        }
        orv_hir::HirExprKind::Slice { target, start, end } => {
            collect_client_dom_bindings_expr(target, inside_html, signals, out);
            if let Some(start) = start {
                collect_client_dom_bindings_expr(start, inside_html, signals, out);
            }
            if let Some(end) = end {
                collect_client_dom_bindings_expr(end, inside_html, signals, out);
            }
        }
        orv_hir::HirExprKind::Field { target, .. }
        | orv_hir::HirExprKind::OptionalField { target, .. } => {
            collect_client_dom_bindings_expr(target, inside_html, signals, out);
        }
        orv_hir::HirExprKind::Lambda { body, .. } => {
            collect_client_dom_bindings_function_body(body, inside_html, signals, out);
        }
        orv_hir::HirExprKind::Try { try_block, catch } => {
            collect_client_dom_bindings_block(try_block, inside_html, signals, out);
            if let Some(catch) = catch {
                collect_client_dom_bindings_block(&catch.body, inside_html, signals, out);
            }
        }
        orv_hir::HirExprKind::Route { handler, .. } => {
            collect_client_dom_bindings_block(handler, inside_html, signals, out);
        }
        orv_hir::HirExprKind::Respond { status, payload } => {
            collect_client_dom_bindings_expr(status, inside_html, signals, out);
            collect_client_dom_bindings_expr(payload, inside_html, signals, out);
        }
        orv_hir::HirExprKind::Server {
            listen,
            routes,
            body_stmts,
        } => {
            if let Some(listen) = listen {
                collect_client_dom_bindings_expr(listen, inside_html, signals, out);
            }
            for route in routes {
                collect_client_dom_bindings_expr(route, inside_html, signals, out);
            }
            for stmt in body_stmts {
                collect_client_dom_bindings_stmt(stmt, inside_html, signals, out);
            }
        }
        _ => {}
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ClientSignalEventAction {
    pub(crate) origin_id: String,
    pub(crate) state_key: String,
    pub(crate) action: serde_json::Value,
}

pub(crate) fn collect_client_event_bindings_for_tag(
    tag: &str,
    args: &[orv_hir::HirExpr],
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
    out: &mut Vec<serde_json::Value>,
) {
    for_each_client_tag_attr_assignment(args, |target, value| {
        let Some(event) = client_event_attr_name(&target.name) else {
            return;
        };
        let Some(action) = client_signal_event_action(value, signals) else {
            return;
        };
        out.push(serde_json::json!({
            "kind": "signal_event",
            "target": CLIENT_PAGE_PATH,
            "source": action.origin_id,
            "state_key": action.state_key,
            "selector": tag,
            "event": event,
            "action": action.action,
            "span": {
                "file": value.span.file.index(),
                "start": value.span.range.start,
                "end": value.span.range.end,
            },
        }));
    });
}

pub(crate) fn client_event_attr_name(name: &str) -> Option<String> {
    let rest = name.strip_prefix("on")?;
    let mut chars = rest.chars();
    let first = chars.next()?;
    first
        .is_ascii_uppercase()
        .then(|| format!("{}{}", first.to_ascii_lowercase(), chars.as_str()))
}

pub(crate) fn client_signal_event_action(
    expr: &orv_hir::HirExpr,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
) -> Option<ClientSignalEventAction> {
    if let orv_hir::HirExprKind::Block(block) = &expr.kind {
        let [orv_hir::HirStmt::Expr(expr)] = block.stmts.as_slice() else {
            return None;
        };
        return client_signal_event_action(expr, signals);
    }
    if let orv_hir::HirExprKind::Lambda { body, .. } = &expr.kind {
        return client_signal_event_action_from_function_body(body, signals);
    }
    let orv_hir::HirExprKind::Assign { target, value } = &expr.kind else {
        return None;
    };
    let signal = signals.get(&target.id)?;
    let action = client_signal_assignment_action(target.id, value);
    Some(ClientSignalEventAction {
        origin_id: signal.origin_id.clone(),
        state_key: signal.state_key.clone(),
        action,
    })
}

pub(crate) fn client_signal_event_action_from_function_body(
    body: &orv_hir::HirFunctionBody,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
) -> Option<ClientSignalEventAction> {
    match body {
        orv_hir::HirFunctionBody::Expr(expr) => client_signal_event_action(expr, signals),
        orv_hir::HirFunctionBody::Block(block) => {
            let [orv_hir::HirStmt::Expr(expr)] = block.stmts.as_slice() else {
                return None;
            };
            client_signal_event_action(expr, signals)
        }
    }
}

pub(crate) fn client_signal_assignment_action(
    target: orv_hir::NameId,
    value: &orv_hir::HirExpr,
) -> serde_json::Value {
    if client_expr_is_event_target_value(value) {
        return serde_json::json!({
            "kind": "assign_event_target_value",
        });
    }
    if client_expr_is_event_target_checked(value) {
        return serde_json::json!({
            "kind": "assign_event_target_checked",
        });
    }
    if let Some(kind) = client_event_target_value_conversion_action(value) {
        return serde_json::json!({
            "kind": kind,
        });
    }
    if let orv_hir::HirExprKind::Unary {
        op: orv_hir::UnaryOp::Not,
        expr,
    } = &value.kind
    {
        if client_expr_is_ident(expr, target) {
            return serde_json::json!({
                "kind": "assign_toggle",
            });
        }
    }
    if let orv_hir::HirExprKind::Binary { op, lhs, rhs } = &value.kind {
        if client_expr_is_ident(lhs, target) {
            let kind = match op {
                orv_hir::BinaryOp::Add => Some("assign_add"),
                orv_hir::BinaryOp::Sub => Some("assign_sub"),
                _ => None,
            };
            if let Some(kind) = kind {
                return serde_json::json!({
                    "kind": kind,
                    "value": client_signal_initial_value_json(rhs),
                });
            }
        }
    }
    serde_json::json!({
        "kind": "assign",
        "value": client_signal_initial_value_json(value),
    })
}

pub(crate) fn client_expr_is_ident(expr: &orv_hir::HirExpr, id: orv_hir::NameId) -> bool {
    matches!(&expr.kind, orv_hir::HirExprKind::Ident(ident) if ident.id == id)
}

pub(crate) fn client_expr_is_event_target_value(expr: &orv_hir::HirExpr) -> bool {
    client_expr_is_event_target_field(expr, "value")
}

pub(crate) fn client_expr_is_event_target_checked(expr: &orv_hir::HirExpr) -> bool {
    client_expr_is_event_target_field(expr, "checked")
}

pub(crate) fn client_expr_is_event_target_field(
    expr: &orv_hir::HirExpr,
    expected_field: &str,
) -> bool {
    let orv_hir::HirExprKind::Field {
        target,
        field: value,
        ..
    } = &expr.kind
    else {
        return false;
    };
    if value != expected_field {
        return false;
    }
    let orv_hir::HirExprKind::Field {
        target: event,
        field,
        ..
    } = &target.kind
    else {
        return false;
    };
    field == "target" && matches!(event.kind, orv_hir::HirExprKind::Ident(_))
}

pub(crate) fn client_event_target_value_conversion_action(
    expr: &orv_hir::HirExpr,
) -> Option<&'static str> {
    let orv_hir::HirExprKind::Call { callee, args } = &expr.kind else {
        return None;
    };
    let [arg] = args.as_slice() else {
        return None;
    };
    if !client_expr_is_event_target_value(arg) {
        return None;
    }
    let orv_hir::HirExprKind::Field { target, field, .. } = &callee.kind else {
        return None;
    };
    if field != "from" {
        return None;
    }
    match &target.kind {
        orv_hir::HirExprKind::TypeName(name)
        | orv_hir::HirExprKind::Ident(orv_hir::HirIdent { name, .. }) => match name.as_str() {
            "float" => Some("assign_event_target_value_float"),
            "int" => Some("assign_event_target_value_int"),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn collect_client_attr_bindings_for_tag(
    tag: &str,
    args: &[orv_hir::HirExpr],
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
    out: &mut Vec<serde_json::Value>,
) {
    for_each_client_tag_attr_assignment(args, |target, value| {
        if client_event_attr_name(&target.name).is_some() {
            return;
        }
        let Some(binding) = client_signal_attr_binding(value, signals) else {
            return;
        };
        let mut attr_binding = serde_json::json!({
            "kind": "signal_attr",
            "target": CLIENT_PAGE_PATH,
            "source": binding.origin_id,
            "state_key": binding.state_key,
            "selector": tag,
            "attr": &target.name,
            "span": {
                "file": value.span.file.index(),
                "start": value.span.range.start,
                "end": value.span.range.end,
            },
        });
        if binding.signal_sources.len() > 1 {
            attr_binding
                .as_object_mut()
                .expect("signal attr binding is an object")
                .insert(
                    "state_keys".to_string(),
                    serde_json::Value::Array(
                        binding
                            .signal_sources
                            .iter()
                            .map(|source| serde_json::json!(&source.state_key))
                            .collect(),
                    ),
                );
            attr_binding
                .as_object_mut()
                .expect("signal attr binding is an object")
                .insert(
                    "sources".to_string(),
                    serde_json::Value::Array(
                        binding
                            .signal_sources
                            .iter()
                            .map(|source| {
                                serde_json::json!({
                                    "source": &source.origin_id,
                                    "state_key": &source.state_key,
                                })
                            })
                            .collect(),
                    ),
                );
        }
        if let Some(attr_template) = binding.attr_template {
            attr_binding
                .as_object_mut()
                .expect("signal attr binding is an object")
                .insert(
                    "attr_template".to_string(),
                    serde_json::Value::Array(attr_template),
                );
        }
        if let Some(attr_condition) = binding.attr_condition {
            attr_binding
                .as_object_mut()
                .expect("signal attr binding is an object")
                .insert("attr_condition".to_string(), attr_condition);
        }
        out.push(attr_binding);
    });
}

pub(crate) fn for_each_client_tag_attr_assignment<'a>(
    args: &'a [orv_hir::HirExpr],
    mut visit: impl FnMut(&'a orv_hir::HirIdent, &'a orv_hir::HirExpr),
) {
    for arg in args {
        match &arg.kind {
            orv_hir::HirExprKind::Assign { target, value } => visit(target, value),
            orv_hir::HirExprKind::Block(block) => {
                for stmt in &block.stmts {
                    let orv_hir::HirStmt::Expr(expr) = stmt else {
                        break;
                    };
                    let orv_hir::HirExprKind::Assign { target, value } = &expr.kind else {
                        break;
                    };
                    visit(target, value);
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn collect_client_dom_bindings_for_tag(
    tag: &str,
    args: &[orv_hir::HirExpr],
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
    out: &mut Vec<serde_json::Value>,
) {
    for arg in args {
        let Some(binding) = client_signal_text_binding(arg, signals) else {
            continue;
        };
        let mut value = serde_json::json!({
            "kind": "signal_text",
            "target": CLIENT_PAGE_PATH,
            "source": binding.origin_id,
            "state_key": binding.state_key,
            "selector": tag,
            "span": {
                "file": arg.span.file.index(),
                "start": arg.span.range.start,
                "end": arg.span.range.end,
            },
        });
        if binding.signal_sources.len() > 1 {
            value
                .as_object_mut()
                .expect("signal text binding is an object")
                .insert(
                    "state_keys".to_string(),
                    serde_json::Value::Array(
                        binding
                            .signal_sources
                            .iter()
                            .map(|source| serde_json::json!(&source.state_key))
                            .collect(),
                    ),
                );
            value
                .as_object_mut()
                .expect("signal text binding is an object")
                .insert(
                    "sources".to_string(),
                    serde_json::Value::Array(
                        binding
                            .signal_sources
                            .iter()
                            .map(|source| {
                                serde_json::json!({
                                    "source": &source.origin_id,
                                    "state_key": &source.state_key,
                                })
                            })
                            .collect(),
                    ),
                );
        }
        if let Some(text_template) = binding.text_template {
            value
                .as_object_mut()
                .expect("signal text binding is an object")
                .insert(
                    "text_template".to_string(),
                    serde_json::Value::Array(text_template),
                );
        }
        if let Some(text_condition) = binding.text_condition {
            value
                .as_object_mut()
                .expect("signal text binding is an object")
                .insert("text_condition".to_string(), text_condition);
        }
        out.push(value);
    }
}

pub(crate) fn client_signal_text_binding(
    expr: &orv_hir::HirExpr,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
) -> Option<ClientSignalTextBinding> {
    match &expr.kind {
        orv_hir::HirExprKind::Ident(ident) => {
            let signal = signals.get(&ident.id)?;
            Some(ClientSignalTextBinding {
                origin_id: signal.origin_id.clone(),
                state_key: signal.state_key.clone(),
                text_template: None,
                text_condition: None,
                signal_sources: vec![signal.clone()],
            })
        }
        orv_hir::HirExprKind::String(segments) => {
            client_signal_text_template_binding(segments, signals)
        }
        orv_hir::HirExprKind::If { .. } => client_signal_text_condition_binding(expr, signals),
        orv_hir::HirExprKind::Block(block) => {
            let [orv_hir::HirStmt::Expr(expr)] = block.stmts.as_slice() else {
                return None;
            };
            client_signal_text_binding(expr, signals)
        }
        _ => None,
    }
}

pub(crate) fn client_signal_text_template_binding(
    segments: &[orv_hir::HirStringSegment],
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
) -> Option<ClientSignalTextBinding> {
    let mut sources: Vec<ClientSignalDomSource> = Vec::new();
    let mut text_template = Vec::new();
    for segment in segments {
        match segment {
            orv_hir::HirStringSegment::Str(text) => {
                if !text.is_empty() {
                    text_template.push(serde_json::json!({
                        "kind": "text",
                        "value": text,
                    }));
                }
            }
            orv_hir::HirStringSegment::Interp(expr) => {
                let orv_hir::HirExprKind::Ident(ident) = &expr.kind else {
                    return None;
                };
                let signal = signals.get(&ident.id)?;
                if !sources
                    .iter()
                    .any(|source| source.state_key == signal.state_key)
                {
                    sources.push(signal.clone());
                }
                text_template.push(serde_json::json!({
                    "kind": "signal",
                    "state_key": &signal.state_key,
                }));
            }
        }
    }
    let signal = sources.first()?;
    Some(ClientSignalTextBinding {
        origin_id: signal.origin_id.clone(),
        state_key: signal.state_key.clone(),
        text_template: Some(text_template),
        text_condition: None,
        signal_sources: sources,
    })
}

pub(crate) fn client_signal_text_condition_binding(
    expr: &orv_hir::HirExpr,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
) -> Option<ClientSignalTextBinding> {
    let orv_hir::HirExprKind::If {
        cond,
        then,
        else_branch,
    } = &expr.kind
    else {
        return None;
    };
    let (signal, mut text_condition) = client_signal_condition_json(cond, signals)?;
    let truthy = client_plain_string_block(then)?;
    let falsy = client_plain_string_expr(else_branch.as_deref()?)?;
    let condition = text_condition.as_object_mut()?;
    condition.insert("truthy".to_string(), serde_json::json!(truthy));
    condition.insert("falsy".to_string(), serde_json::json!(falsy));
    Some(ClientSignalTextBinding {
        origin_id: signal.origin_id.clone(),
        state_key: signal.state_key.clone(),
        text_template: None,
        text_condition: Some(text_condition),
        signal_sources: vec![signal.clone()],
    })
}

pub(crate) fn client_signal_attr_binding(
    expr: &orv_hir::HirExpr,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
) -> Option<ClientSignalAttrBinding> {
    if let Some(binding) = client_signal_attr_condition_binding(expr, signals) {
        return Some(binding);
    }
    if let Some(binding) = client_signal_text_binding(expr, signals) {
        return Some(ClientSignalAttrBinding {
            origin_id: binding.origin_id,
            state_key: binding.state_key,
            attr_template: binding.text_template,
            attr_condition: None,
            signal_sources: binding.signal_sources,
        });
    }
    None
}

pub(crate) fn client_signal_attr_condition_binding(
    expr: &orv_hir::HirExpr,
    signals: &HashMap<orv_hir::NameId, ClientSignalDomSource>,
) -> Option<ClientSignalAttrBinding> {
    let orv_hir::HirExprKind::If {
        cond,
        then,
        else_branch,
    } = &expr.kind
    else {
        return None;
    };
    let (signal, mut attr_condition) = client_signal_condition_json(cond, signals)?;
    let truthy = client_plain_string_block(then)?;
    let falsy = client_plain_string_expr(else_branch.as_deref()?)?;
    let condition = attr_condition.as_object_mut()?;
    condition.insert("truthy".to_string(), serde_json::json!(truthy));
    condition.insert("falsy".to_string(), serde_json::json!(falsy));
    Some(ClientSignalAttrBinding {
        origin_id: signal.origin_id.clone(),
        state_key: signal.state_key.clone(),
        attr_template: None,
        attr_condition: Some(attr_condition),
        signal_sources: vec![signal.clone()],
    })
}

pub(crate) fn client_signal_condition_json<'a>(
    expr: &orv_hir::HirExpr,
    signals: &'a HashMap<orv_hir::NameId, ClientSignalDomSource>,
) -> Option<(&'a ClientSignalDomSource, serde_json::Value)> {
    match &expr.kind {
        orv_hir::HirExprKind::Ident(ident) => {
            let signal = signals.get(&ident.id)?;
            Some((
                signal,
                serde_json::json!({
                    "state_key": &signal.state_key,
                }),
            ))
        }
        orv_hir::HirExprKind::Binary { op, lhs, rhs } => {
            client_signal_comparison_condition_json(*op, lhs, rhs, signals)
        }
        orv_hir::HirExprKind::Paren(inner) => client_signal_condition_json(inner, signals),
        _ => None,
    }
}

pub(crate) fn client_signal_comparison_condition_json<'a>(
    op: orv_hir::BinaryOp,
    lhs: &orv_hir::HirExpr,
    rhs: &orv_hir::HirExpr,
    signals: &'a HashMap<orv_hir::NameId, ClientSignalDomSource>,
) -> Option<(&'a ClientSignalDomSource, serde_json::Value)> {
    if let Some(signal) = client_signal_condition_ident(lhs, signals) {
        let op = client_signal_comparison_op(op)?;
        let rhs = client_signal_condition_operand_json(rhs)?;
        return Some((
            signal,
            serde_json::json!({
                "state_key": &signal.state_key,
                "op": op,
                "rhs": rhs,
            }),
        ));
    }
    let signal = client_signal_condition_ident(rhs, signals)?;
    let op = client_signal_inverted_comparison_op(op)?;
    let rhs = client_signal_condition_operand_json(lhs)?;
    Some((
        signal,
        serde_json::json!({
            "state_key": &signal.state_key,
            "op": op,
            "rhs": rhs,
        }),
    ))
}

pub(crate) fn client_signal_condition_ident<'a>(
    expr: &orv_hir::HirExpr,
    signals: &'a HashMap<orv_hir::NameId, ClientSignalDomSource>,
) -> Option<&'a ClientSignalDomSource> {
    match &expr.kind {
        orv_hir::HirExprKind::Ident(ident) => signals.get(&ident.id),
        orv_hir::HirExprKind::Paren(inner) => client_signal_condition_ident(inner, signals),
        _ => None,
    }
}

pub(crate) fn client_signal_condition_operand_json(
    expr: &orv_hir::HirExpr,
) -> Option<serde_json::Value> {
    match &expr.kind {
        orv_hir::HirExprKind::Integer(value) => Some(serde_json::json!({
            "kind": "int",
            "value": value,
        })),
        orv_hir::HirExprKind::Float(value) => Some(serde_json::json!({
            "kind": "float",
            "value": value,
        })),
        orv_hir::HirExprKind::String(segments)
            if segments
                .iter()
                .all(|segment| matches!(segment, orv_hir::HirStringSegment::Str(_))) =>
        {
            let value = segments
                .iter()
                .map(|segment| match segment {
                    orv_hir::HirStringSegment::Str(value) => value.as_str(),
                    orv_hir::HirStringSegment::Interp(_) => "",
                })
                .collect::<String>();
            Some(serde_json::json!({
                "kind": "string",
                "value": value,
            }))
        }
        orv_hir::HirExprKind::True => Some(serde_json::json!({
            "kind": "bool",
            "value": true,
        })),
        orv_hir::HirExprKind::False => Some(serde_json::json!({
            "kind": "bool",
            "value": false,
        })),
        orv_hir::HirExprKind::Paren(inner) => client_signal_condition_operand_json(inner),
        _ => None,
    }
}

pub(crate) fn client_signal_comparison_op(op: orv_hir::BinaryOp) -> Option<&'static str> {
    match op {
        orv_hir::BinaryOp::Eq => Some("eq"),
        orv_hir::BinaryOp::Ne => Some("ne"),
        orv_hir::BinaryOp::Lt => Some("lt"),
        orv_hir::BinaryOp::Gt => Some("gt"),
        orv_hir::BinaryOp::Le => Some("le"),
        orv_hir::BinaryOp::Ge => Some("ge"),
        _ => None,
    }
}

pub(crate) fn client_signal_inverted_comparison_op(op: orv_hir::BinaryOp) -> Option<&'static str> {
    match op {
        orv_hir::BinaryOp::Eq => Some("eq"),
        orv_hir::BinaryOp::Ne => Some("ne"),
        orv_hir::BinaryOp::Lt => Some("gt"),
        orv_hir::BinaryOp::Gt => Some("lt"),
        orv_hir::BinaryOp::Le => Some("ge"),
        orv_hir::BinaryOp::Ge => Some("le"),
        _ => None,
    }
}

pub(crate) fn client_plain_string_block(block: &orv_hir::HirBlock) -> Option<String> {
    let [orv_hir::HirStmt::Expr(expr)] = block.stmts.as_slice() else {
        return None;
    };
    client_plain_string_expr(expr)
}

pub(crate) fn client_plain_string_expr(expr: &orv_hir::HirExpr) -> Option<String> {
    match &expr.kind {
        orv_hir::HirExprKind::String(segments)
            if segments
                .iter()
                .all(|segment| matches!(segment, orv_hir::HirStringSegment::Str(_))) =>
        {
            Some(
                segments
                    .iter()
                    .map(|segment| match segment {
                        orv_hir::HirStringSegment::Str(text) => text.as_str(),
                        orv_hir::HirStringSegment::Interp(_) => "",
                    })
                    .collect(),
            )
        }
        orv_hir::HirExprKind::Block(block) => client_plain_string_block(block),
        _ => None,
    }
}

pub(crate) fn client_signal_initial_values(
    program: &orv_hir::HirProgram,
) -> HashMap<String, serde_json::Value> {
    program
        .items
        .iter()
        .filter_map(|stmt| {
            let orv_hir::HirStmt::Let(stmt) = stmt else {
                return None;
            };
            (stmt.kind == orv_hir::HirLetKind::Signal).then(|| {
                (
                    orv_hir::origin_id("signal", &stmt.name.name, stmt.span),
                    client_signal_initial_value_json(&stmt.init),
                )
            })
        })
        .collect()
}

pub(crate) fn client_signal_initial_value_json(expr: &orv_hir::HirExpr) -> serde_json::Value {
    match &expr.kind {
        orv_hir::HirExprKind::Integer(value) => {
            serde_json::json!({"kind": "int", "value": value})
        }
        orv_hir::HirExprKind::Float(value) => {
            serde_json::json!({"kind": "float", "value": value})
        }
        orv_hir::HirExprKind::String(segments)
            if segments
                .iter()
                .all(|segment| matches!(segment, orv_hir::HirStringSegment::Str(_))) =>
        {
            let value = segments
                .iter()
                .map(|segment| match segment {
                    orv_hir::HirStringSegment::Str(value) => value.as_str(),
                    orv_hir::HirStringSegment::Interp(_) => "",
                })
                .collect::<String>();
            serde_json::json!({"kind": "string", "value": value})
        }
        orv_hir::HirExprKind::True => serde_json::json!({"kind": "bool", "value": true}),
        orv_hir::HirExprKind::False => serde_json::json!({"kind": "bool", "value": false}),
        orv_hir::HirExprKind::Void => serde_json::json!({"kind": "void", "value": null}),
        _ => serde_json::json!({
            "kind": "dynamic",
            "span": {
                "file": expr.span.file.index(),
                "start": expr.span.range.start,
                "end": expr.span.range.end,
            },
        }),
    }
}

pub(crate) fn write_client_bundle_manifest(
    out: &Path,
    path: &str,
    entry: &Path,
    binding: &ClientSourceBinding<'_>,
    targets: &ClientBundleTargets<'_>,
) -> anyhow::Result<()> {
    let page = targets
        .page
        .ok_or_else(|| anyhow::anyhow!("missing client_page bundle target"))?;
    let loader = targets
        .js
        .ok_or_else(|| anyhow::anyhow!("missing client_js bundle target"))?;
    let wasm = targets
        .wasm
        .ok_or_else(|| anyhow::anyhow!("missing client_wasm bundle target"))?;
    let reactive_plan = targets
        .reactive_plan
        .ok_or_else(|| anyhow::anyhow!("missing client_reactive_plan bundle target"))?;
    let loader_hash = file_content_hash(&out.join(loader))?;
    let reactive_plan_value = read_json_value(&out.join(reactive_plan))?;
    let reactive_plan_hash = stable_json_hash(&reactive_plan_value)?;
    let wasm_hash = file_content_hash(&out.join(wasm))?;
    let manifest = serde_json::json!({
        "schema_version": 1,
        "kind": "orv.client.bundle",
        "entry": entry.display().to_string(),
        "reactive_plan": reactive_plan,
        "reactive_plan_hash": reactive_plan_hash,
        "page": page,
        "loader": loader,
        "loader_hash": loader_hash,
        "wasm": wasm,
        "wasm_hash": wasm_hash,
        "source_bundle": SOURCE_BUNDLE_PATH,
        "source_bundle_hash": binding.source_bundle_hash,
        "runtime_features": ["client_wasm"],
        "exports": {
            "start": CLIENT_WASM_START_EXPORT,
            "render_ptr": CLIENT_WASM_RENDER_PTR_EXPORT,
            "render_len": CLIENT_WASM_RENDER_LEN_EXPORT,
            "memory": CLIENT_WASM_MEMORY_EXPORT,
        },
        "initial_render": {
            "content_type": "text/html",
            "encoding": "utf-8",
            "html_hash": format!("{:016x}", fnv1a64(binding.initial_render.as_bytes())),
            "byte_length": binding.initial_render.len(),
        },
        "capabilities": client_bundle_capabilities_json(&reactive_plan_value),
        "blocked_by": ["dynamic-client-codegen", "reactive-dom-diff"],
        "blockers": client_manifest_blockers_json(),
    });
    write_json(&out.join(path), &manifest)
}

pub(crate) fn client_bundle_capabilities_json(
    reactive_plan: &serde_json::Value,
) -> serde_json::Value {
    let empty = Vec::new();
    let bindings = reactive_plan
        .get("bindings")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);
    let signals = reactive_plan
        .get("signals")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);
    let mut surfaces = BTreeSet::new();
    if client_binding_count(bindings, "initial_render") > 0 {
        surfaces.insert("wasm_initial_render");
    }
    surfaces.insert("embedded_reactive_plan");
    surfaces.insert("source_bundle_validation");
    if client_binding_count(bindings, "signal_state") > 0 {
        surfaces.insert("signal_state");
    }
    if client_binding_count(bindings, "signal_text") > 0 {
        surfaces.insert("signal_text");
    }
    if client_binding_has_field(bindings, "signal_text", "text_template") {
        surfaces.insert("signal_text_template");
    }
    if client_binding_has_field(bindings, "signal_text", "text_condition") {
        surfaces.insert("signal_text_condition");
    }
    if client_binding_count(bindings, "signal_attr") > 0 {
        surfaces.insert("signal_attr");
    }
    if client_binding_has_field(bindings, "signal_attr", "attr_template") {
        surfaces.insert("signal_attr_template");
    }
    if client_binding_has_field(bindings, "signal_attr", "attr_condition") {
        surfaces.insert("signal_attr_condition");
    }
    if client_binding_count(bindings, "signal_event") > 0 {
        surfaces.insert("signal_event");
    }
    serde_json::json!({
        "schema_version": 1,
        "runtime": "client_wasm",
        "source": CLIENT_REACTIVE_PLAN_PATH,
        "signals": signals.len(),
        "bindings": {
            "total": bindings.len(),
            "initial_render": client_binding_count(bindings, "initial_render"),
            "signal_state": client_binding_count(bindings, "signal_state"),
            "signal_text": client_binding_count(bindings, "signal_text"),
            "signal_attr": client_binding_count(bindings, "signal_attr"),
            "signal_event": client_binding_count(bindings, "signal_event"),
        },
        "surfaces": surfaces.into_iter().collect::<Vec<_>>(),
        "event_actions": client_event_action_kinds(bindings),
    })
}

pub(crate) fn client_binding_count(bindings: &[serde_json::Value], kind: &str) -> usize {
    bindings
        .iter()
        .filter(|binding| binding.get("kind").and_then(serde_json::Value::as_str) == Some(kind))
        .count()
}

pub(crate) fn client_binding_has_field(
    bindings: &[serde_json::Value],
    kind: &str,
    field: &str,
) -> bool {
    bindings.iter().any(|binding| {
        binding.get("kind").and_then(serde_json::Value::as_str) == Some(kind)
            && binding.get(field).is_some()
    })
}

pub(crate) fn client_event_action_kinds(bindings: &[serde_json::Value]) -> Vec<String> {
    let mut actions = BTreeSet::new();
    for binding in bindings {
        if binding.get("kind").and_then(serde_json::Value::as_str) != Some("signal_event") {
            continue;
        }
        if let Some(kind) = binding
            .pointer("/action/kind")
            .and_then(serde_json::Value::as_str)
        {
            actions.insert(kind.to_string());
        }
    }
    actions.into_iter().collect()
}

pub(crate) fn client_manifest_blockers_json() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "id": "dynamic-client-codegen",
            "artifact": CLIENT_JS_PATH,
            "reason": "optimized source-to-JS client codegen is not emitted yet",
        }),
        serde_json::json!({
            "id": "reactive-dom-diff",
            "artifact": CLIENT_REACTIVE_PLAN_PATH,
            "reason": "full DOM diff codegen is not emitted yet",
        }),
    ]
}

pub(crate) fn validate_prod_server_listen(
    server_artifact: Option<&orv_compiler::ServerRuntimeArtifact>,
) -> anyhow::Result<()> {
    let Some(server_artifact) = server_artifact else {
        return Ok(());
    };
    if server_artifact
        .listen
        .as_ref()
        .and_then(|listen| listen.port)
        == Some(0)
    {
        anyhow::bail!("prod server listen port must be 1..=65535; @listen 0 is test-only");
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeployPersistence {
    pub(crate) wal_paths: Vec<String>,
    pub(crate) db_paths: Vec<String>,
    pub(crate) db_endpoints: Vec<String>,
    pub(crate) db_env: Vec<DeployAdapterEnv>,
    pub(crate) db_adapters: Vec<DeployDbAdapter>,
    pub(crate) record_paths: Vec<String>,
    pub(crate) commerce_endpoints: Vec<String>,
    pub(crate) commerce_env: Vec<DeployAdapterEnv>,
    pub(crate) commerce_adapters: Vec<DeployCommerceAdapter>,
    pub(crate) volumes: Vec<DeployPersistenceVolume>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct DeployAdapterEnv {
    pub(crate) env: String,
    pub(crate) default: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct DeployDbAdapter {
    pub(crate) mode: String,
    pub(crate) provider: String,
    pub(crate) env: Option<String>,
    pub(crate) default: Option<String>,
    pub(crate) endpoint: Option<String>,
    pub(crate) adapter_status: String,
    pub(crate) bridge_env: Vec<DeployProviderEnv>,
    pub(crate) source_origin_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct DeployCommerceAdapter {
    pub(crate) kind: String,
    pub(crate) mode: String,
    pub(crate) provider: Option<String>,
    pub(crate) env: Option<String>,
    pub(crate) default: Option<String>,
    pub(crate) endpoint: Option<String>,
    pub(crate) record_path: Option<String>,
    pub(crate) provider_env: Vec<DeployProviderEnv>,
    pub(crate) source_origin_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct DeployProviderEnv {
    pub(crate) env: String,
    pub(crate) required: bool,
    pub(crate) purpose: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct DeployPreflightEnv {
    pub(crate) kind: String,
    pub(crate) env: String,
    pub(crate) required: bool,
    pub(crate) purpose: String,
    pub(crate) default: Option<String>,
    pub(crate) provider: Option<String>,
}

#[derive(Default)]
pub(crate) struct DeployPersistenceAccumulator {
    pub(crate) wal_paths: Vec<String>,
    pub(crate) db_paths: Vec<String>,
    pub(crate) db_endpoints: Vec<String>,
    pub(crate) db_env: Vec<DeployAdapterEnv>,
    pub(crate) db_adapters: Vec<DeployDbAdapter>,
    pub(crate) record_paths: Vec<String>,
    pub(crate) commerce_endpoints: Vec<String>,
    pub(crate) commerce_env: Vec<DeployAdapterEnv>,
    pub(crate) commerce_adapters: Vec<DeployCommerceAdapter>,
}

impl DeployPersistenceAccumulator {
    fn into_persistence(mut self) -> DeployPersistence {
        self.wal_paths.sort();
        self.wal_paths.dedup();
        self.db_paths.sort();
        self.db_paths.dedup();
        self.db_endpoints.sort();
        self.db_endpoints.dedup();
        self.db_env.sort();
        self.db_env.dedup();
        self.db_adapters = merge_deploy_db_adapters(self.db_adapters);
        self.record_paths.sort();
        self.record_paths.dedup();
        self.commerce_endpoints.sort();
        self.commerce_endpoints.dedup();
        self.commerce_env.sort();
        self.commerce_env.dedup();
        self.commerce_adapters = merge_deploy_commerce_adapters(self.commerce_adapters);
        let mut persistent_paths = self.wal_paths.clone();
        persistent_paths.extend(self.db_paths.clone());
        persistent_paths.extend(self.record_paths.clone());
        persistent_paths.sort();
        persistent_paths.dedup();
        DeployPersistence {
            volumes: deploy_persistence_volumes(&persistent_paths),
            wal_paths: self.wal_paths,
            db_paths: self.db_paths,
            db_endpoints: self.db_endpoints,
            db_env: self.db_env,
            db_adapters: self.db_adapters,
            record_paths: self.record_paths,
            commerce_endpoints: self.commerce_endpoints,
            commerce_env: self.commerce_env,
            commerce_adapters: self.commerce_adapters,
        }
    }
}

pub(crate) fn merge_deploy_db_adapters(adapters: Vec<DeployDbAdapter>) -> Vec<DeployDbAdapter> {
    let mut merged = Vec::<DeployDbAdapter>::new();
    for mut adapter in adapters {
        normalize_source_origin_ids(&mut adapter.source_origin_ids);
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| same_db_adapter_contract(existing, &adapter))
        {
            existing.source_origin_ids.extend(adapter.source_origin_ids);
            normalize_source_origin_ids(&mut existing.source_origin_ids);
        } else {
            merged.push(adapter);
        }
    }
    merged.sort();
    merged
}

pub(crate) fn same_db_adapter_contract(a: &DeployDbAdapter, b: &DeployDbAdapter) -> bool {
    a.mode == b.mode
        && a.provider == b.provider
        && a.env == b.env
        && a.default == b.default
        && a.endpoint == b.endpoint
        && a.adapter_status == b.adapter_status
        && a.bridge_env == b.bridge_env
}

pub(crate) fn merge_deploy_commerce_adapters(
    adapters: Vec<DeployCommerceAdapter>,
) -> Vec<DeployCommerceAdapter> {
    let mut merged = Vec::<DeployCommerceAdapter>::new();
    for mut adapter in adapters {
        normalize_source_origin_ids(&mut adapter.source_origin_ids);
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| same_commerce_adapter_contract(existing, &adapter))
        {
            existing.source_origin_ids.extend(adapter.source_origin_ids);
            normalize_source_origin_ids(&mut existing.source_origin_ids);
        } else {
            merged.push(adapter);
        }
    }
    merged.sort();
    merged
}

pub(crate) fn same_commerce_adapter_contract(
    a: &DeployCommerceAdapter,
    b: &DeployCommerceAdapter,
) -> bool {
    a.kind == b.kind
        && a.mode == b.mode
        && a.provider == b.provider
        && a.env == b.env
        && a.default == b.default
        && a.endpoint == b.endpoint
        && a.record_path == b.record_path
        && a.provider_env == b.provider_env
}

pub(crate) fn normalize_source_origin_ids(source_origin_ids: &mut Vec<String>) {
    source_origin_ids.sort();
    source_origin_ids.dedup();
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeployPersistenceVolume {
    pub(crate) host: String,
    pub(crate) container: String,
    pub(crate) compose_mount: String,
}

pub(crate) fn server_artifact_deploy_persistence(
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<DeployPersistence> {
    let entry_path = artifact
        .source_bundle
        .files
        .first()
        .ok_or_else(|| anyhow::anyhow!("server artifact source bundle is empty"))?
        .path
        .clone();
    let loaded = orv_project::load_project_from_sources(
        Path::new(&entry_path),
        artifact
            .source_bundle
            .files
            .iter()
            .map(|file| (PathBuf::from(&file.path), file.source.clone())),
    )
    .map_err(|e| anyhow::anyhow!("failed to rehydrate deploy persistence sources: {e}"))?;
    if !loaded.diagnostics.is_empty() {
        anyhow::bail!("deploy persistence source reanalysis produced diagnostics");
    }
    let resolved = orv_resolve::resolve(&loaded.program);
    if !resolved.diagnostics.is_empty() {
        anyhow::bail!("deploy persistence resolve reanalysis produced diagnostics");
    }
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    if !lowered.diagnostics.is_empty() {
        anyhow::bail!("deploy persistence lowering reanalysis produced diagnostics");
    }
    let mut persistence = DeployPersistenceAccumulator::default();
    collect_program_persistence_paths(&lowered.program, &mut persistence);
    Ok(persistence.into_persistence())
}

pub(crate) fn deploy_persistence_value(persistence: &DeployPersistence) -> serde_json::Value {
    serde_json::json!({
        "wal_paths": persistence.wal_paths,
        "db_paths": persistence.db_paths,
        "db_endpoints": persistence.db_endpoints,
        "db_env": deploy_adapter_env_value(&persistence.db_env),
        "db_adapters": deploy_db_adapter_value(&persistence.db_adapters),
        "record_paths": persistence.record_paths,
        "commerce_endpoints": persistence.commerce_endpoints,
        "commerce_env": deploy_adapter_env_value(&persistence.commerce_env),
        "commerce_adapters": deploy_commerce_adapter_value(&persistence.commerce_adapters),
        "volumes": persistence.volumes.iter().map(|volume| {
            serde_json::json!({
                "host": volume.host,
                "container": volume.container,
                "compose_mount": volume.compose_mount,
            })
        }).collect::<Vec<_>>(),
    })
}

pub(crate) fn deploy_security_runtime_features(runtime_features: &[String]) -> Vec<String> {
    let mut features = runtime_features
        .iter()
        .filter(|feature| {
            matches!(
                feature.as_str(),
                "auth_roles" | "csrf_protection" | "rate_limit" | "session_cookies"
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    features.sort();
    features
}

pub(crate) fn deploy_preflight_env_values(
    listen: Option<&orv_compiler::ServerListenArtifact>,
    persistence: &DeployPersistence,
    required: bool,
) -> serde_json::Value {
    serde_json::Value::Array(
        deploy_preflight_env_contract(listen, persistence)
            .into_iter()
            .filter(|env| env.required == required)
            .map(|env| {
                serde_json::json!({
                    "kind": env.kind,
                    "env": env.env,
                    "required": env.required,
                    "purpose": env.purpose,
                    "default": env.default,
                    "provider": env.provider,
                })
            })
            .collect(),
    )
}

pub(crate) fn deploy_preflight_env_contract(
    listen: Option<&orv_compiler::ServerListenArtifact>,
    persistence: &DeployPersistence,
) -> Vec<DeployPreflightEnv> {
    let mut envs = BTreeSet::new();
    if let Some(env) = listen.and_then(|listen| listen.env.as_ref()) {
        envs.insert(DeployPreflightEnv {
            kind: "listen".to_string(),
            env: env.variable.clone(),
            required: env.default_port.is_none(),
            purpose: "port".to_string(),
            default: env.default_port.map(|port| port.to_string()),
            provider: None,
        });
    }
    for env in &persistence.db_env {
        envs.insert(DeployPreflightEnv {
            kind: "db".to_string(),
            env: env.env.clone(),
            required: env.default.is_none(),
            purpose: "adapter_url".to_string(),
            default: env.default.clone(),
            provider: None,
        });
    }
    for adapter in &persistence.db_adapters {
        for env in &adapter.bridge_env {
            envs.insert(DeployPreflightEnv {
                kind: "db".to_string(),
                env: env.env.clone(),
                required: env.required,
                purpose: env.purpose.clone(),
                default: None,
                provider: Some(adapter.provider.clone()),
            });
        }
    }
    for adapter in &persistence.commerce_adapters {
        if let Some(env) = &adapter.env {
            envs.insert(DeployPreflightEnv {
                kind: adapter.kind.clone(),
                env: env.clone(),
                required: adapter.default.is_none(),
                purpose: "adapter_url".to_string(),
                default: adapter.default.clone(),
                provider: adapter.provider.clone(),
            });
        }
        for env in &adapter.provider_env {
            envs.insert(DeployPreflightEnv {
                kind: adapter.kind.clone(),
                env: env.env.clone(),
                required: env.required,
                purpose: env.purpose.clone(),
                default: None,
                provider: adapter.provider.clone(),
            });
        }
    }
    envs.into_iter().collect()
}

pub(crate) fn deploy_db_adapter_value(adapters: &[DeployDbAdapter]) -> Vec<serde_json::Value> {
    adapters
        .iter()
        .map(|adapter| {
            serde_json::json!({
                "kind": "db",
                "mode": adapter.mode,
                "provider": adapter.provider,
                "env": adapter.env.as_deref(),
                "default": adapter.default.as_deref(),
                "endpoint": adapter.endpoint.as_deref(),
                "adapter_status": adapter.adapter_status,
                "source_origin_id": adapter.source_origin_ids.first().map(String::as_str),
                "source_origin_ids": adapter.source_origin_ids.clone(),
                "runtime": {
                    "status": adapter.adapter_status,
                    "query_methods": ["create", "find", "update", "delete", "transaction"],
                },
                "bridge": deploy_db_adapter_bridge_value(&adapter.bridge_env),
            })
        })
        .collect()
}

pub(crate) fn deploy_db_adapter_bridge_value(envs: &[DeployProviderEnv]) -> serde_json::Value {
    serde_json::json!({
        "contract": "http-json-v1",
        "method": "POST",
        "content_type": "application/json",
        "query_methods": [
            "create",
            "find",
            "findAll",
            "update",
            "delete",
            "upsert",
            "search",
            "count",
            "sum",
            "transaction",
            "schema",
        ],
        "body": {
            "kind": "orv.db.adapter",
            "contract": "http-json-v1",
            "provider": "adapter provider",
            "url": "adapter url",
            "method": "db method",
            "args": "runtime value array",
        },
        "retry": {
            "attempts": 3,
            "on": ["5xx", "connect_error", "read_error", "timeout"],
        },
        "env": deploy_provider_env_value(envs),
    })
}

pub(crate) fn deploy_adapter_env_value(envs: &[DeployAdapterEnv]) -> Vec<serde_json::Value> {
    envs.iter()
        .map(|env| {
            serde_json::json!({
                "env": env.env,
                "default": env.default.as_deref(),
            })
        })
        .collect()
}

pub(crate) fn deploy_commerce_adapter_value(
    adapters: &[DeployCommerceAdapter],
) -> Vec<serde_json::Value> {
    adapters
        .iter()
        .map(|adapter| {
            let mut value = serde_json::json!({
                "kind": adapter.kind,
                "mode": adapter.mode,
                "env": adapter.env.as_deref(),
                "default": adapter.default.as_deref(),
                "endpoint": adapter.endpoint.as_deref(),
                "record_path": adapter.record_path.as_deref(),
                "source_origin_id": adapter.source_origin_ids.first().map(String::as_str),
                "source_origin_ids": adapter.source_origin_ids.clone(),
                "request": deploy_commerce_adapter_request_value(&adapter.kind),
            });
            if let Some(provider) = &adapter.provider {
                value
                    .as_object_mut()
                    .expect("commerce adapter value is an object")
                    .insert(
                        "provider".to_string(),
                        serde_json::Value::String(provider.clone()),
                    );
            }
            if !adapter.provider_env.is_empty() {
                value
                    .as_object_mut()
                    .expect("commerce adapter value is an object")
                    .insert(
                        "provider_env".to_string(),
                        serde_json::Value::Array(deploy_provider_env_value(&adapter.provider_env)),
                    );
            }
            value
        })
        .collect()
}

pub(crate) fn deploy_provider_env_value(envs: &[DeployProviderEnv]) -> Vec<serde_json::Value> {
    envs.iter()
        .map(|env| {
            serde_json::json!({
                "env": env.env,
                "required": env.required,
                "purpose": env.purpose,
            })
        })
        .collect()
}

pub(crate) fn deploy_commerce_adapter_request_value(kind: &str) -> serde_json::Value {
    let (request_kind, payload) = match kind {
        "payment" => ("payment.capture", "payment capture payload"),
        "shipping" => ("shipping.booking", "shipping booking payload"),
        _ => ("commerce.request", "commerce payload"),
    };
    serde_json::json!({
        "method": "POST",
        "content_type": "application/json",
        "kind": request_kind,
        "body": {
            "kind": request_kind,
            "payload": payload,
        },
    })
}

pub(crate) fn deploy_persistence_volumes(wal_paths: &[String]) -> Vec<DeployPersistenceVolume> {
    let mut dirs = BTreeSet::new();
    for wal_path in wal_paths {
        if let Some(dir) = deploy_persistent_parent_dir(wal_path) {
            dirs.insert(dir);
        }
    }
    dirs.into_iter()
        .map(|host| {
            let container = format!("/app/{host}");
            DeployPersistenceVolume {
                compose_mount: format!("../{host}:{container}"),
                host,
                container,
            }
        })
        .collect()
}

pub(crate) fn deploy_persistent_parent_dir(path: &str) -> Option<String> {
    let path = Path::new(path);
    if path.is_absolute() {
        return None;
    }
    let parent = path.parent()?;
    if parent.as_os_str().is_empty()
        || parent
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(parent.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn deploy_compose_volumes(persistence: &DeployPersistence) -> String {
    if persistence.volumes.is_empty() {
        return String::new();
    }
    let mut out = String::from("    volumes:\n");
    for volume in &persistence.volumes {
        let _ = writeln!(out, "      - {}", volume.compose_mount);
    }
    out
}

pub(crate) fn deploy_runbook_persistence_section(persistence: &DeployPersistence) -> String {
    let has_db_bridge_env = persistence
        .db_adapters
        .iter()
        .any(|adapter| !adapter.bridge_env.is_empty());
    let has_provider_env = persistence
        .commerce_adapters
        .iter()
        .any(|adapter| !adapter.provider_env.is_empty());
    if persistence.wal_paths.is_empty()
        && persistence.db_paths.is_empty()
        && persistence.db_endpoints.is_empty()
        && persistence.db_env.is_empty()
        && persistence.record_paths.is_empty()
        && persistence.commerce_endpoints.is_empty()
        && persistence.commerce_env.is_empty()
        && !has_db_bridge_env
        && !has_provider_env
    {
        return String::new();
    }
    let mut out = String::from("## Persistent Data\n\n");
    for path in &persistence.wal_paths {
        let _ = writeln!(out, "- WAL: {path}");
    }
    for path in &persistence.db_paths {
        let _ = writeln!(out, "- DB: {path}");
    }
    for endpoint in &persistence.db_endpoints {
        let _ = writeln!(out, "- DB endpoint: {endpoint}");
    }
    for env in &persistence.db_env {
        match &env.default {
            Some(default) => {
                let _ = writeln!(out, "- DB adapter env: {} default {default}", env.env);
            }
            None => {
                let _ = writeln!(out, "- DB adapter env: {}", env.env);
            }
        }
    }
    for adapter in &persistence.db_adapters {
        for env in &adapter.bridge_env {
            let required = if env.required { "required" } else { "optional" };
            let _ = writeln!(
                out,
                "- DB bridge env: {} {} {required} {}",
                adapter.provider, env.env, env.purpose
            );
        }
    }
    for path in &persistence.record_paths {
        let _ = writeln!(out, "- Record log: {path}");
    }
    for endpoint in &persistence.commerce_endpoints {
        let _ = writeln!(out, "- Commerce endpoint: {endpoint}");
    }
    for env in &persistence.commerce_env {
        match &env.default {
            Some(default) => {
                let _ = writeln!(out, "- Commerce adapter env: {} default {default}", env.env);
            }
            None => {
                let _ = writeln!(out, "- Commerce adapter env: {}", env.env);
            }
        }
    }
    for adapter in &persistence.commerce_adapters {
        let Some(provider) = &adapter.provider else {
            continue;
        };
        for env in &adapter.provider_env {
            let required = if env.required { "required" } else { "optional" };
            let _ = writeln!(
                out,
                "- Commerce provider env: {} {provider} {} {required} {}",
                adapter.kind, env.env, env.purpose
            );
        }
    }
    for volume in &persistence.volumes {
        let _ = writeln!(out, "- Compose volume: {}", volume.compose_mount);
    }
    out.push('\n');
    out
}

pub(crate) fn collect_program_persistence_paths(
    program: &orv_hir::HirProgram,
    out: &mut DeployPersistenceAccumulator,
) {
    for stmt in &program.items {
        collect_stmt_persistence_paths(stmt, out);
    }
}

pub(crate) fn collect_stmt_persistence_paths(
    stmt: &orv_hir::HirStmt,
    out: &mut DeployPersistenceAccumulator,
) {
    match stmt {
        orv_hir::HirStmt::Let(stmt) => {
            collect_expr_persistence_paths(&stmt.init, out);
        }
        orv_hir::HirStmt::Const(stmt) => {
            collect_expr_persistence_paths(&stmt.init, out);
        }
        orv_hir::HirStmt::Function(stmt) => {
            collect_function_body_persistence_paths(&stmt.body, out);
        }
        orv_hir::HirStmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_persistence_paths(value, out);
            }
        }
        orv_hir::HirStmt::Expr(expr) => {
            collect_expr_persistence_paths(expr, out);
        }
        orv_hir::HirStmt::Struct(_)
        | orv_hir::HirStmt::Enum(_)
        | orv_hir::HirStmt::TypeAlias(_)
        | orv_hir::HirStmt::Import(_) => {}
    }
}

pub(crate) fn collect_block_persistence_paths(
    block: &orv_hir::HirBlock,
    out: &mut DeployPersistenceAccumulator,
) {
    for stmt in &block.stmts {
        collect_stmt_persistence_paths(stmt, out);
    }
}

pub(crate) fn collect_function_body_persistence_paths(
    body: &orv_hir::HirFunctionBody,
    out: &mut DeployPersistenceAccumulator,
) {
    match body {
        orv_hir::HirFunctionBody::Block(block) => {
            collect_block_persistence_paths(block, out);
        }
        orv_hir::HirFunctionBody::Expr(expr) => {
            collect_expr_persistence_paths(expr, out);
        }
    }
}

pub(crate) fn collect_expr_persistence_paths(
    expr: &orv_hir::HirExpr,
    out: &mut DeployPersistenceAccumulator,
) {
    use orv_hir::HirExprKind;

    if let HirExprKind::Call { callee, args } = &expr.kind {
        let call_name = hir_call_name(callee);
        if call_name == "@db.wal" {
            if let Some(path) = args.first().and_then(hir_static_string) {
                out.wal_paths.push(path);
            }
        } else if call_name == "@db.connect" {
            if let Some(arg) = args.first() {
                collect_db_adapter_persistence_arg(
                    arg,
                    hir_source_origin_id("call", &call_name, expr.span),
                    out,
                );
            }
        } else if matches!(call_name.as_str(), "@payment.connect" | "@shipping.connect") {
            if let Some(arg) = args.first() {
                let kind = if call_name == "@payment.connect" {
                    "payment"
                } else {
                    "shipping"
                };
                collect_commerce_adapter_persistence_arg(
                    kind,
                    arg,
                    hir_source_origin_id("call", &call_name, expr.span),
                    out,
                );
            }
        }
    }

    match &expr.kind {
        HirExprKind::Integer(_)
        | HirExprKind::Float(_)
        | HirExprKind::Regex { .. }
        | HirExprKind::True
        | HirExprKind::False
        | HirExprKind::Void
        | HirExprKind::TypeName(_)
        | HirExprKind::Ident(_)
        | HirExprKind::Break
        | HirExprKind::Continue => {}
        HirExprKind::String(segments) => {
            for segment in segments {
                if let orv_hir::HirStringSegment::Interp(expr) = segment {
                    collect_expr_persistence_paths(expr, out);
                }
            }
        }
        HirExprKind::Unary { expr, .. }
        | HirExprKind::Paren(expr)
        | HirExprKind::Out(expr)
        | HirExprKind::Throw(expr)
        | HirExprKind::Await(expr)
        | HirExprKind::Cast { expr, .. } => {
            collect_expr_persistence_paths(expr, out);
        }
        HirExprKind::Binary { lhs, rhs, .. } => {
            collect_expr_persistence_paths(lhs, out);
            collect_expr_persistence_paths(rhs, out);
        }
        HirExprKind::Html(block) | HirExprKind::Block(block) => {
            collect_block_persistence_paths(block, out);
        }
        HirExprKind::Route { handler, .. } => {
            collect_block_persistence_paths(handler, out);
        }
        HirExprKind::Respond { status, payload } => {
            collect_expr_persistence_paths(status, out);
            collect_expr_persistence_paths(payload, out);
        }
        HirExprKind::Server {
            listen,
            routes,
            body_stmts,
        } => {
            if let Some(listen) = listen {
                collect_expr_persistence_paths(listen, out);
            }
            for route in routes {
                collect_expr_persistence_paths(route, out);
            }
            for stmt in body_stmts {
                collect_stmt_persistence_paths(stmt, out);
            }
        }
        HirExprKind::Domain { args, .. } => {
            for arg in args {
                collect_expr_persistence_paths(arg, out);
            }
        }
        HirExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            collect_expr_persistence_paths(cond, out);
            collect_block_persistence_paths(then, out);
            if let Some(else_branch) = else_branch {
                collect_expr_persistence_paths(else_branch, out);
            }
        }
        HirExprKind::When { scrutinee, arms } => {
            collect_expr_persistence_paths(scrutinee, out);
            for arm in arms {
                collect_pattern_persistence_paths(&arm.pattern, out);
                collect_expr_persistence_paths(&arm.body, out);
            }
        }
        HirExprKind::Assign { value, .. } => {
            collect_expr_persistence_paths(value, out);
        }
        HirExprKind::AssignField { object, value, .. } => {
            collect_expr_persistence_paths(object, out);
            collect_expr_persistence_paths(value, out);
        }
        HirExprKind::AssignIndex {
            object,
            index,
            value,
        } => {
            collect_expr_persistence_paths(object, out);
            collect_expr_persistence_paths(index, out);
            collect_expr_persistence_paths(value, out);
        }
        HirExprKind::Call { callee, args } => {
            collect_expr_persistence_paths(callee, out);
            for arg in args {
                collect_expr_persistence_paths(arg, out);
            }
        }
        HirExprKind::For { iter, body, .. } => {
            collect_expr_persistence_paths(iter, out);
            collect_block_persistence_paths(body, out);
        }
        HirExprKind::While { cond, body } => {
            collect_expr_persistence_paths(cond, out);
            collect_block_persistence_paths(body, out);
        }
        HirExprKind::Range { start, end, .. } => {
            collect_expr_persistence_paths(start, out);
            collect_expr_persistence_paths(end, out);
        }
        HirExprKind::Array(items) | HirExprKind::Tuple(items) => {
            for item in items {
                collect_expr_persistence_paths(item, out);
            }
        }
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            for field in fields {
                collect_expr_persistence_paths(&field.value, out);
            }
        }
        HirExprKind::Index { target, index } => {
            collect_expr_persistence_paths(target, out);
            collect_expr_persistence_paths(index, out);
        }
        HirExprKind::Slice { target, start, end } => {
            collect_expr_persistence_paths(target, out);
            if let Some(start) = start {
                collect_expr_persistence_paths(start, out);
            }
            if let Some(end) = end {
                collect_expr_persistence_paths(end, out);
            }
        }
        HirExprKind::Field { target, .. } | HirExprKind::OptionalField { target, .. } => {
            collect_expr_persistence_paths(target, out);
        }
        HirExprKind::Lambda { body, .. } => {
            collect_function_body_persistence_paths(body, out);
        }
        HirExprKind::Try { try_block, catch } => {
            collect_block_persistence_paths(try_block, out);
            if let Some(catch) = catch {
                collect_block_persistence_paths(&catch.body, out);
            }
        }
    }
}

pub(crate) fn collect_pattern_persistence_paths(
    pattern: &orv_hir::HirPattern,
    out: &mut DeployPersistenceAccumulator,
) {
    match pattern {
        orv_hir::HirPattern::Literal(expr)
        | orv_hir::HirPattern::Guard(expr)
        | orv_hir::HirPattern::Not(expr)
        | orv_hir::HirPattern::Contains(expr) => {
            collect_expr_persistence_paths(expr, out);
        }
        orv_hir::HirPattern::Range { start, end, .. } => {
            collect_expr_persistence_paths(start, out);
            collect_expr_persistence_paths(end, out);
        }
        orv_hir::HirPattern::Wildcard => {}
    }
}

pub(crate) fn file_adapter_path(url: &str) -> Option<String> {
    let path = url.strip_prefix("file://")?;
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

pub(crate) fn sqlite_adapter_path(url: &str) -> Option<String> {
    let path = url.strip_prefix("sqlite://")?;
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

pub(crate) fn http_adapter_endpoint(url: &str) -> Option<String> {
    url.strip_prefix("http://")
        .filter(|target| !target.is_empty())
        .map(|_| url.to_string())
}

pub(crate) fn collect_db_adapter_persistence_arg(
    arg: &orv_hir::HirExpr,
    source_origin_id: Option<String>,
    out: &mut DeployPersistenceAccumulator,
) {
    if let Some(url) = hir_static_string(arg) {
        collect_db_adapter_url(&url, None, None, source_origin_id.clone(), out);
    }
    if let Some(env) = hir_env_configured_string(arg) {
        if let Some(default) = &env.default {
            collect_db_adapter_url(
                default,
                Some(env.env.clone()),
                Some(default.clone()),
                source_origin_id.clone(),
                out,
            );
        } else {
            out.db_adapters.push(DeployDbAdapter {
                mode: "env".to_string(),
                provider: "unknown".to_string(),
                env: Some(env.env.clone()),
                default: None,
                endpoint: None,
                adapter_status: "env_required".to_string(),
                bridge_env: Vec::new(),
                source_origin_ids: source_origin_id.clone().into_iter().collect(),
            });
        }
        out.db_env.push(env);
    }
}

pub(crate) fn collect_db_adapter_url(
    url: &str,
    env: Option<String>,
    default: Option<String>,
    source_origin_id: Option<String>,
    out: &mut DeployPersistenceAccumulator,
) {
    if let Some(path) = file_adapter_path(url) {
        out.wal_paths.push(path);
    }
    if let Some(path) = sqlite_adapter_path(url) {
        out.db_paths.push(path);
    }
    if let Some(provider) = external_db_adapter_provider(url) {
        out.db_endpoints.push(url.to_string());
        out.db_adapters.push(DeployDbAdapter {
            mode: "external".to_string(),
            provider: provider.to_string(),
            env,
            default,
            endpoint: Some(url.to_string()),
            adapter_status: "unsupported_runtime".to_string(),
            bridge_env: db_adapter_bridge_env(provider),
            source_origin_ids: source_origin_id.into_iter().collect(),
        });
    }
}

pub(crate) fn hir_source_origin_id(kind: &str, name: &str, span: Span) -> Option<String> {
    (span.file != FileId::DUMMY).then(|| orv_hir::origin_id(kind, name, span))
}

pub(crate) fn external_db_adapter_provider(url: &str) -> Option<&'static str> {
    if url
        .strip_prefix("postgres://")
        .is_some_and(|target| !target.is_empty())
    {
        return Some("postgres");
    }
    if url
        .strip_prefix("mysql://")
        .is_some_and(|target| !target.is_empty())
    {
        return Some("mysql");
    }
    None
}

pub(crate) fn db_adapter_bridge_env(provider: &str) -> Vec<DeployProviderEnv> {
    match provider {
        "postgres" => vec![
            deploy_provider_env("ORV_DB_ADAPTER_POSTGRES_ENDPOINT", true, "bridge_endpoint"),
            deploy_provider_env(
                "ORV_DB_ADAPTER_POSTGRES_AUTH_TOKEN",
                false,
                "bridge_auth_token",
            ),
            deploy_provider_env("ORV_DB_ADAPTER_ENDPOINT", false, "bridge_endpoint_fallback"),
            deploy_provider_env(
                "ORV_DB_ADAPTER_AUTH_TOKEN",
                false,
                "bridge_auth_token_fallback",
            ),
        ],
        "mysql" => vec![
            deploy_provider_env("ORV_DB_ADAPTER_MYSQL_ENDPOINT", true, "bridge_endpoint"),
            deploy_provider_env(
                "ORV_DB_ADAPTER_MYSQL_AUTH_TOKEN",
                false,
                "bridge_auth_token",
            ),
            deploy_provider_env("ORV_DB_ADAPTER_ENDPOINT", false, "bridge_endpoint_fallback"),
            deploy_provider_env(
                "ORV_DB_ADAPTER_AUTH_TOKEN",
                false,
                "bridge_auth_token_fallback",
            ),
        ],
        _ => Vec::new(),
    }
}

pub(crate) fn collect_commerce_adapter_persistence_arg(
    kind: &str,
    arg: &orv_hir::HirExpr,
    source_origin_id: Option<String>,
    out: &mut DeployPersistenceAccumulator,
) {
    if let Some(url) = hir_static_string(arg) {
        collect_commerce_adapter_url(kind, &url, None, None, source_origin_id.clone(), out);
    }
    if let Some(env) = hir_env_configured_string(arg) {
        if let Some(default) = &env.default {
            collect_commerce_adapter_url(
                kind,
                default,
                Some(env.env.clone()),
                Some(default.clone()),
                source_origin_id.clone(),
                out,
            );
        } else {
            out.commerce_adapters.push(DeployCommerceAdapter {
                kind: kind.to_string(),
                mode: "env".to_string(),
                provider: None,
                env: Some(env.env.clone()),
                default: None,
                endpoint: None,
                record_path: None,
                provider_env: Vec::new(),
                source_origin_ids: source_origin_id.clone().into_iter().collect(),
            });
        }
        out.commerce_env.push(env);
    }
}

pub(crate) fn collect_commerce_adapter_url(
    kind: &str,
    url: &str,
    env: Option<String>,
    default: Option<String>,
    source_origin_id: Option<String>,
    out: &mut DeployPersistenceAccumulator,
) {
    let mut mode = "local".to_string();
    let mut provider = commerce_provider(url, kind);
    let mut record_path = None;
    let mut endpoint = None;
    if let Some(path) = file_adapter_path(url) {
        mode = "file".to_string();
        provider = None;
        record_path = Some(path.clone());
        out.record_paths.push(path);
    }
    if let Some(http_endpoint) = http_adapter_endpoint(url) {
        mode = "http".to_string();
        provider = None;
        endpoint = Some(http_endpoint.clone());
        out.commerce_endpoints.push(http_endpoint);
    }
    if provider.is_some() {
        mode = "provider".to_string();
    }
    let provider_env = provider
        .as_deref()
        .map(|provider| commerce_provider_env_for_url(provider, url))
        .unwrap_or_default();
    out.commerce_adapters.push(DeployCommerceAdapter {
        kind: kind.to_string(),
        mode,
        provider,
        env,
        default,
        endpoint,
        record_path,
        provider_env,
        source_origin_ids: source_origin_id.into_iter().collect(),
    });
}

pub(crate) fn commerce_provider(url: &str, kind: &str) -> Option<String> {
    let (scheme, target) = url.split_once("://")?;
    if target.is_empty() {
        return None;
    }
    match (kind, scheme) {
        ("payment", "stripe") => Some("stripe".to_string()),
        ("shipping", "carrier") => Some("carrier".to_string()),
        _ => None,
    }
}

pub(crate) fn commerce_provider_env(provider: &str) -> Vec<DeployProviderEnv> {
    match provider {
        "stripe" => vec![
            deploy_provider_env("STRIPE_API_ENDPOINT", false, "api_endpoint"),
            deploy_provider_env("STRIPE_SECRET_KEY", true, "api_secret"),
            deploy_provider_env("STRIPE_WEBHOOK_SECRET", false, "webhook_signature"),
            deploy_provider_env(
                "STRIPE_WEBHOOK_SECRET_PREVIOUS",
                false,
                "webhook_signature_previous",
            ),
        ],
        "carrier" => vec![
            deploy_provider_env("CARRIER_API_ENDPOINT", false, "api_endpoint"),
            deploy_provider_env("CARRIER_API_KEY", true, "api_key"),
            deploy_provider_env("CARRIER_WEBHOOK_SECRET", false, "webhook_signature"),
        ],
        _ => Vec::new(),
    }
}

pub(crate) fn commerce_provider_env_for_url(provider: &str, url: &str) -> Vec<DeployProviderEnv> {
    if provider == "stripe" && url.starts_with("stripe://webhook") {
        return vec![
            deploy_provider_env("STRIPE_WEBHOOK_SECRET", false, "webhook_signature"),
            deploy_provider_env(
                "STRIPE_WEBHOOK_SECRET_PREVIOUS",
                false,
                "webhook_signature_previous",
            ),
        ];
    }
    commerce_provider_env(provider)
}

pub(crate) fn deploy_provider_env(env: &str, required: bool, purpose: &str) -> DeployProviderEnv {
    DeployProviderEnv {
        env: env.to_string(),
        required,
        purpose: purpose.to_string(),
    }
}

pub(crate) fn hir_env_configured_string(expr: &orv_hir::HirExpr) -> Option<DeployAdapterEnv> {
    match &expr.kind {
        orv_hir::HirExprKind::Paren(inner) => hir_env_configured_string(inner),
        orv_hir::HirExprKind::Binary {
            op: orv_hir::BinaryOp::Coalesce,
            lhs,
            rhs,
        } => {
            let env = hir_env_variable(lhs)?;
            Some(DeployAdapterEnv {
                env,
                default: hir_static_string(rhs),
            })
        }
        _ => hir_env_variable(expr).map(|env| DeployAdapterEnv { env, default: None }),
    }
}

pub(crate) fn hir_env_variable(expr: &orv_hir::HirExpr) -> Option<String> {
    match &expr.kind {
        orv_hir::HirExprKind::Paren(inner) => hir_env_variable(inner),
        orv_hir::HirExprKind::Field { target, field, .. } => match &target.kind {
            orv_hir::HirExprKind::Domain { name, args, .. } if name == "env" && args.is_empty() => {
                Some(field.clone())
            }
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn hir_static_string(expr: &orv_hir::HirExpr) -> Option<String> {
    if let orv_hir::HirExprKind::Paren(inner) = &expr.kind {
        return hir_static_string(inner);
    }
    let orv_hir::HirExprKind::String(segments) = &expr.kind else {
        return None;
    };
    let mut out = String::new();
    for segment in segments {
        match segment {
            orv_hir::HirStringSegment::Str(value) => out.push_str(value),
            orv_hir::HirStringSegment::Interp(_) => return None,
        }
    }
    Some(out)
}

pub(crate) fn hir_call_name(expr: &orv_hir::HirExpr) -> String {
    match &expr.kind {
        orv_hir::HirExprKind::Ident(ident) => ident.name.clone(),
        orv_hir::HirExprKind::Field { target, field, .. } => {
            format!("{}.{}", hir_call_name(target), field)
        }
        orv_hir::HirExprKind::OptionalField { target, field, .. } => {
            format!("{}?.{}", hir_call_name(target), field)
        }
        orv_hir::HirExprKind::Domain { name, .. } => format!("@{name}"),
        orv_hir::HirExprKind::TypeName(name) => name.clone(),
        _ => "<expr>".to_string(),
    }
}

pub(crate) fn deploy_ports_value(
    listen: Option<&orv_compiler::ServerListenArtifact>,
) -> serde_json::Value {
    let Some(listen) = listen else {
        return serde_json::json!([]);
    };
    if let Some(port) = listen.port.filter(|port| *port > 0) {
        return serde_json::json!([
            {
                "container": port,
                "protocol": "tcp",
            }
        ]);
    }
    let Some(env) = &listen.env else {
        return serde_json::json!([]);
    };
    let mut port = serde_json::json!({
        "env": env.variable.clone(),
        "protocol": "tcp",
    });
    if let Some(default_port) = env.default_port.filter(|port| *port > 0) {
        port["default"] = serde_json::json!(default_port);
    }
    serde_json::json!([port])
}

pub(crate) fn deploy_exposed_port(
    listen: Option<&orv_compiler::ServerListenArtifact>,
) -> Option<u16> {
    listen
        .and_then(|listen| {
            listen
                .port
                .or_else(|| listen.env.as_ref().and_then(|env| env.default_port))
        })
        .filter(|port| *port > 0)
}

pub(crate) struct DeployComposePort {
    pub(crate) binding: String,
    pub(crate) environment: String,
    pub(crate) display: String,
}

pub(crate) fn deploy_compose_port(
    listen: Option<&orv_compiler::ServerListenArtifact>,
) -> Option<DeployComposePort> {
    let listen = listen?;
    if let Some(port) = listen.port.filter(|port| *port > 0) {
        return Some(DeployComposePort {
            binding: format!("\"{port}:{port}\""),
            environment: format!("PORT: \"{port}\""),
            display: port.to_string(),
        });
    }
    let env = listen.env.as_ref()?;
    let variable = &env.variable;
    if let Some(default_port) = env.default_port.filter(|port| *port > 0) {
        return Some(DeployComposePort {
            binding: format!("\"${{{variable}:-{default_port}}}:{default_port}\""),
            environment: format!("PORT: \"${{{variable}:-{default_port}}}\""),
            display: default_port.to_string(),
        });
    }
    Some(DeployComposePort {
        binding: format!("\"${{{variable}}}:${{{variable}}}\""),
        environment: format!("PORT: \"${{{variable}}}\""),
        display: format!("${{{variable}}}"),
    })
}

pub(crate) fn deploy_compose_ports(listen: Option<&orv_compiler::ServerListenArtifact>) -> String {
    deploy_compose_port(listen)
        .map(|port| format!("    ports:\n      - {}\n", port.binding))
        .unwrap_or_default()
}

pub(crate) fn deploy_compose_environment_lines(
    listen: Option<&orv_compiler::ServerListenArtifact>,
    persistence: &DeployPersistence,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(port) = deploy_compose_port(listen) {
        lines.push(port.environment);
    }
    for env in &persistence.db_env {
        let variable = &env.env;
        let value = match &env.default {
            Some(default) => format!("{variable}: \"${{{variable}:-{default}}}\""),
            None => format!("{variable}: \"${{{variable}}}\""),
        };
        lines.push(value);
    }
    for env in deploy_db_bridge_envs(persistence) {
        let variable = &env.env;
        lines.push(format!("{variable}: \"${{{variable}}}\""));
    }
    for env in &persistence.commerce_env {
        let variable = &env.env;
        let value = match &env.default {
            Some(default) => format!("{variable}: \"${{{variable}:-{default}}}\""),
            None => format!("{variable}: \"${{{variable}}}\""),
        };
        lines.push(value);
    }
    for env in deploy_commerce_provider_envs(persistence) {
        let variable = &env.env;
        lines.push(format!("{variable}: \"${{{variable}}}\""));
    }
    lines
}

pub(crate) fn deploy_compose_environment(
    listen: Option<&orv_compiler::ServerListenArtifact>,
    persistence: &DeployPersistence,
) -> String {
    let lines = deploy_compose_environment_lines(listen, persistence);
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::from("    environment:\n");
    for line in lines {
        let _ = writeln!(out, "      {line}");
    }
    out
}

pub(crate) fn deploy_env_example_assignments(
    listen: Option<&orv_compiler::ServerListenArtifact>,
    persistence: &DeployPersistence,
) -> Vec<String> {
    let mut assignments = Vec::new();
    if let Some(port) = deploy_env_example_port_assignment(listen) {
        assignments.push(port);
    }
    assignments.extend(persistence.db_env.iter().map(deploy_adapter_env_assignment));
    assignments.extend(
        deploy_db_bridge_envs(persistence)
            .iter()
            .map(deploy_provider_env_assignment),
    );
    assignments.extend(
        persistence
            .commerce_env
            .iter()
            .map(deploy_adapter_env_assignment),
    );
    assignments.extend(
        deploy_commerce_provider_envs(persistence)
            .iter()
            .map(deploy_provider_env_assignment),
    );
    assignments
}

pub(crate) fn deploy_env_example_port_assignment(
    listen: Option<&orv_compiler::ServerListenArtifact>,
) -> Option<String> {
    let listen = listen?;
    if let Some(port) = listen.port.filter(|port| *port > 0) {
        return Some(format!("PORT={port}"));
    }
    let env = listen.env.as_ref()?;
    let value = env
        .default_port
        .filter(|port| *port > 0)
        .map_or_else(String::new, |port| port.to_string());
    Some(format!("{}={value}", env.variable))
}

pub(crate) fn deploy_adapter_env_assignment(env: &DeployAdapterEnv) -> String {
    match &env.default {
        Some(default) => format!("{}={default}", env.env),
        None => format!("{}=", env.env),
    }
}

pub(crate) fn deploy_provider_env_assignment(env: &DeployProviderEnv) -> String {
    format!("{}=", env.env)
}

pub(crate) fn deploy_db_bridge_envs(persistence: &DeployPersistence) -> Vec<DeployProviderEnv> {
    let mut envs = BTreeSet::new();
    for adapter in &persistence.db_adapters {
        for env in &adapter.bridge_env {
            envs.insert(env.clone());
        }
    }
    envs.into_iter().collect()
}

pub(crate) fn deploy_commerce_provider_envs(
    persistence: &DeployPersistence,
) -> Vec<DeployProviderEnv> {
    let mut envs = BTreeSet::new();
    for adapter in &persistence.commerce_adapters {
        for env in &adapter.provider_env {
            envs.insert(env.clone());
        }
    }
    envs.into_iter().collect()
}

pub(crate) fn deploy_runbook_port_assignment(
    listen: Option<&orv_compiler::ServerListenArtifact>,
) -> Option<String> {
    let listen = listen?;
    if let Some(port) = listen.port.filter(|port| *port > 0) {
        return Some(format!("PORT={port}"));
    }
    let env = listen.env.as_ref()?;
    let variable = &env.variable;
    if let Some(default_port) = env.default_port.filter(|port| *port > 0) {
        return Some(format!("PORT=${{{variable}:-{default_port}}}"));
    }
    Some(format!("PORT=${{{variable}}}"))
}

pub(crate) fn deploy_runbook_trace_events_url(
    listen: Option<&orv_compiler::ServerListenArtifact>,
) -> String {
    let port = deploy_listen_url_port(listen);
    format!("http://127.0.0.1:{port}/__orv/trace/events")
}

pub(crate) fn deploy_smoke_base_url(listen: Option<&orv_compiler::ServerListenArtifact>) -> String {
    let port = deploy_listen_url_port(listen);
    format!("http://127.0.0.1:{port}")
}

pub(crate) fn deploy_listen_url_port(
    listen: Option<&orv_compiler::ServerListenArtifact>,
) -> String {
    let port = listen
        .and_then(|listen| {
            listen
                .port
                .filter(|port| *port > 0)
                .map(|port| port.to_string())
                .or_else(|| {
                    listen.env.as_ref().map(|env| {
                        let variable = &env.variable;
                        env.default_port.filter(|port| *port > 0).map_or_else(
                            || format!("${{{variable}}}"),
                            |port| format!("${{{variable}:-{port}}}"),
                        )
                    })
                })
        })
        .unwrap_or_else(|| "8080".to_string());
    port
}

pub(crate) fn deploy_routes_include(
    artifact: &orv_compiler::ServerRuntimeArtifact,
    method: &str,
    path: &str,
) -> bool {
    artifact
        .routes
        .iter()
        .any(|route| route.method == method && route.path == path)
}

pub(crate) fn deploy_smoke_origin_var_name(method: &str, path: &str) -> String {
    let mut suffix = String::new();
    let mut wrote = false;
    for ch in path.trim_matches('/').chars() {
        if ch.is_ascii_alphanumeric() {
            suffix.push(ch.to_ascii_uppercase());
            wrote = true;
        } else if wrote && !suffix.ends_with('_') {
            suffix.push('_');
        }
    }
    while suffix.ends_with('_') {
        suffix.pop();
    }
    if !wrote {
        suffix.push_str("ROOT");
    }
    format!(
        "ORV_SMOKE_ORIGIN_{}_{}",
        method.to_ascii_uppercase(),
        suffix
    )
}

pub(crate) fn deploy_smoke_origin_var_ref(method: &str, path: &str) -> String {
    format!("${}", deploy_smoke_origin_var_name(method, path))
}

pub(crate) fn deploy_smoke_response_origin_var_name(method: &str, path: &str) -> String {
    deploy_smoke_origin_var_name(method, path).replacen(
        "ORV_SMOKE_ORIGIN_",
        "ORV_SMOKE_RESPONSE_ORIGIN_",
        1,
    )
}

pub(crate) fn deploy_smoke_response_origin_var_ref(method: &str, path: &str) -> String {
    format!("${}", deploy_smoke_response_origin_var_name(method, path))
}

pub(crate) fn deploy_smoke_unique_response_origin(
    route: &orv_compiler::ServerRouteArtifact,
) -> Option<&str> {
    match route.response_origin_ids.as_slice() {
        [origin_id] => Some(origin_id.as_str()),
        _ => None,
    }
}

pub(crate) fn deploy_smoke_has_commerce_record(
    persistence: &DeployPersistence,
    kind: &str,
    record_path: &str,
) -> bool {
    persistence
        .commerce_adapters
        .iter()
        .any(|adapter| adapter.kind == kind && adapter.record_path.as_deref() == Some(record_path))
}

pub(crate) fn deploy_smoke_commerce_record_origin(
    persistence: &DeployPersistence,
    kind: &str,
    record_path: &str,
) -> String {
    persistence
        .commerce_adapters
        .iter()
        .find(|adapter| adapter.kind == kind && adapter.record_path.as_deref() == Some(record_path))
        .and_then(|adapter| adapter.source_origin_ids.first())
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn deploy_smoke_ready_path(
    artifact: &orv_compiler::ServerRuntimeArtifact,
) -> Option<&str> {
    artifact
        .routes
        .iter()
        .find(|route| route.method == "GET" && route.path == "/health")
        .or_else(|| {
            artifact
                .routes
                .iter()
                .find(|route| route.method == "GET" && !route.path.contains(':'))
        })
        .map(|route| route.path.as_str())
}

pub(crate) fn deploy_smoke_client_reveal_origin(
    origin_map: &orv_compiler::OriginMap,
) -> Option<&str> {
    origin_map
        .entries
        .iter()
        .find(|entry| matches!(entry.kind.as_str(), "signal" | "await"))
        .or_else(|| {
            origin_map
                .entries
                .iter()
                .find(|entry| entry.kind == "domain" && entry.name == "html")
        })
        .map(|entry| entry.id.as_str())
}

pub(crate) fn bundle_output_path(plan: &orv_compiler::BundlePlan, kind: &str) -> Option<String> {
    plan.bundles
        .iter()
        .find(|bundle| bundle.kind == kind)
        .map(|bundle| normalized_artifact_path(&bundle.path))
}

pub(crate) const WASM_MODULE_HEADER: &[u8] = b"\0asm\x01\0\0\0";
pub(crate) const CLIENT_WASM_CUSTOM_SECTION_NAME: &str = "orv.client";
pub(crate) const SOURCE_BUNDLE_PATH: &str = "source-bundle.json";
pub(crate) const CLIENT_MANIFEST_PATH: &str = "client/manifest.json";
pub(crate) const CLIENT_REACTIVE_PLAN_PATH: &str = "client/reactive-plan.json";
pub(crate) const CLIENT_PAGE_PATH: &str = "pages/index.html";
pub(crate) const CLIENT_JS_PATH: &str = "client/app.js";
pub(crate) const CLIENT_WASM_PATH: &str = "client/app.wasm";
pub(crate) const CLIENT_WASM_SOURCE_BUNDLE_PATH: &str = "../source-bundle.json";
pub(crate) const ORV_REFERENCE_RUNTIME_IMAGE: &str = "ghcr.io/orv-lang/orv-reference:latest";
pub(crate) const DEPLOY_SMOKE_TEST_PATH: &str = "deploy/smoke-test.sh";
pub(crate) const DEPLOY_SMOKE_OUTPUT_PATH: &str = "deploy/smoke-output.txt";
pub(crate) const DEPLOY_PREFLIGHT_PATH: &str = "deploy/preflight.json";
pub(crate) const DEPLOY_BENCHMARK_EVIDENCE_PATH: &str = "deploy/benchmark-evidence.json";
pub(crate) const SERVER_ARTIFACT_PATH: &str = "server/app.orv-runtime.json";
pub(crate) const SERVER_LAUNCH_PATH: &str = "server/launch.json";
pub(crate) const NATIVE_SERVER_PLAN_PATH: &str = "server/native-server.json";
pub(crate) const NATIVE_RUNTIME_IMAGE_PLAN_PATH: &str = "server/runtime-image.json";
pub(crate) const NATIVE_RUNTIME_IMAGE_DOCKERFILE_PATH: &str = "server/native/Dockerfile";
pub(crate) const NATIVE_SERVER_SOURCE_PATH: &str = "server/native/main.rs";
pub(crate) const NATIVE_SERVER_ROUTES_SOURCE_PATH: &str = "server/native/routes.rs";
pub(crate) const NATIVE_SERVER_ROUTER_SOURCE_PATH: &str = "server/native/router.rs";
pub(crate) const NATIVE_SERVER_HANDLERS_SOURCE_PATH: &str = "server/native/handlers.rs";
pub(crate) const NATIVE_SERVER_PACKAGE_PATH: &str = "server/native/Cargo.toml";
pub(crate) const NATIVE_SERVER_BINARY_PATH: &str = "server/app";
pub(crate) const NATIVE_SERVER_LAUNCHER_BINARY_PATH: &str =
    "./server/native/target/release/orv-native-server";
pub(crate) const NATIVE_RUNTIME_IMAGE_NAME: &str = "orv-native-server:latest";
pub(crate) const CLIENT_WASM_START_EXPORT: &str = "orv_start";
pub(crate) const CLIENT_WASM_RENDER_PTR_EXPORT: &str = "orv_render_ptr";
pub(crate) const CLIENT_WASM_RENDER_LEN_EXPORT: &str = "orv_render_len";
pub(crate) const CLIENT_WASM_MEMORY_EXPORT: &str = "memory";
pub(crate) const CLIENT_JS_LOADER_TEMPLATE: &str = include_str!("client_loader_template.js");

pub(crate) fn write_client_wasm_bundle(
    path: &Path,
    source_bundle: &orv_compiler::SourceBundleArtifact,
    source_bundle_hash: &str,
    initial_render: &str,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(
        path,
        client_wasm_bundle_bytes(source_bundle, source_bundle_hash, initial_render)?,
    )
    .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))
}

pub(crate) fn client_wasm_bundle_bytes(
    source_bundle: &orv_compiler::SourceBundleArtifact,
    source_bundle_hash: &str,
    initial_render: &str,
) -> anyhow::Result<Vec<u8>> {
    let render_bytes = initial_render.as_bytes();
    let render_len = i32::try_from(render_bytes.len())
        .map_err(|_| anyhow::anyhow!("client initial render exceeds wasm32 i32 length"))?;
    let mut bytes = WASM_MODULE_HEADER.to_vec();
    let mut custom_section = Vec::new();
    push_wasm_len(&mut custom_section, CLIENT_WASM_CUSTOM_SECTION_NAME.len());
    custom_section.extend_from_slice(CLIENT_WASM_CUSTOM_SECTION_NAME.as_bytes());
    let payload = client_wasm_metadata_json(source_bundle, source_bundle_hash, initial_render);
    custom_section.extend_from_slice(payload.as_bytes());

    bytes.push(0);
    push_wasm_len(&mut bytes, custom_section.len());
    bytes.extend(custom_section);

    let mut type_section = Vec::new();
    push_wasm_u32_leb(&mut type_section, 2);
    type_section.push(0x60);
    push_wasm_u32_leb(&mut type_section, 0);
    push_wasm_u32_leb(&mut type_section, 0);
    type_section.push(0x60);
    push_wasm_u32_leb(&mut type_section, 0);
    push_wasm_u32_leb(&mut type_section, 1);
    type_section.push(0x7f);
    push_wasm_section(&mut bytes, 1, &type_section);

    let mut function_section = Vec::new();
    push_wasm_u32_leb(&mut function_section, 3);
    push_wasm_u32_leb(&mut function_section, 0);
    push_wasm_u32_leb(&mut function_section, 1);
    push_wasm_u32_leb(&mut function_section, 1);
    push_wasm_section(&mut bytes, 3, &function_section);

    let mut memory_section = Vec::new();
    push_wasm_u32_leb(&mut memory_section, 1);
    memory_section.push(0x00);
    push_wasm_u32_leb(&mut memory_section, wasm_min_pages(render_bytes.len())?);
    push_wasm_section(&mut bytes, 5, &memory_section);

    let mut export_section = Vec::new();
    push_wasm_u32_leb(&mut export_section, 4);
    push_wasm_len(&mut export_section, CLIENT_WASM_START_EXPORT.len());
    export_section.extend_from_slice(CLIENT_WASM_START_EXPORT.as_bytes());
    export_section.push(0);
    push_wasm_u32_leb(&mut export_section, 0);
    push_wasm_len(&mut export_section, CLIENT_WASM_RENDER_PTR_EXPORT.len());
    export_section.extend_from_slice(CLIENT_WASM_RENDER_PTR_EXPORT.as_bytes());
    export_section.push(0);
    push_wasm_u32_leb(&mut export_section, 1);
    push_wasm_len(&mut export_section, CLIENT_WASM_RENDER_LEN_EXPORT.len());
    export_section.extend_from_slice(CLIENT_WASM_RENDER_LEN_EXPORT.as_bytes());
    export_section.push(0);
    push_wasm_u32_leb(&mut export_section, 2);
    push_wasm_len(&mut export_section, CLIENT_WASM_MEMORY_EXPORT.len());
    export_section.extend_from_slice(CLIENT_WASM_MEMORY_EXPORT.as_bytes());
    export_section.push(2);
    push_wasm_u32_leb(&mut export_section, 0);
    push_wasm_section(&mut bytes, 7, &export_section);

    let mut code_section = Vec::new();
    push_wasm_u32_leb(&mut code_section, 3);
    push_wasm_u32_leb(&mut code_section, 2);
    push_wasm_u32_leb(&mut code_section, 0);
    code_section.push(0x0b);
    push_wasm_const_i32_function(&mut code_section, 0);
    push_wasm_const_i32_function(&mut code_section, render_len);
    push_wasm_section(&mut bytes, 10, &code_section);
    if !render_bytes.is_empty() {
        let mut data_section = Vec::new();
        push_wasm_u32_leb(&mut data_section, 1);
        data_section.push(0x00);
        data_section.push(0x41);
        push_wasm_u32_leb(&mut data_section, 0);
        data_section.push(0x0b);
        push_wasm_len(&mut data_section, render_bytes.len());
        data_section.extend_from_slice(render_bytes);
        push_wasm_section(&mut bytes, 11, &data_section);
    }
    Ok(bytes)
}

pub(crate) fn client_wasm_metadata_json(
    source_bundle: &orv_compiler::SourceBundleArtifact,
    source_bundle_hash: &str,
    initial_render: &str,
) -> String {
    serde_json::json!({
        "schema_version": 1,
        "runtime_features": ["client_wasm"],
        "source_bundle": CLIENT_WASM_SOURCE_BUNDLE_PATH,
        "source_bundle_hash": source_bundle_hash,
        "entry": &source_bundle.entry,
        "initial_render": {
            "content_type": "text/html",
            "encoding": "utf-8",
            "html_hash": format!("{:016x}", fnv1a64(initial_render.as_bytes())),
            "byte_length": initial_render.len(),
            "ptr_export": CLIENT_WASM_RENDER_PTR_EXPORT,
            "len_export": CLIENT_WASM_RENDER_LEN_EXPORT,
            "memory_export": CLIENT_WASM_MEMORY_EXPORT,
        },
    })
    .to_string()
}

pub(crate) fn wasm_min_pages(byte_len: usize) -> anyhow::Result<u32> {
    let pages = byte_len.div_ceil(65_536).max(1);
    u32::try_from(pages)
        .map_err(|_| anyhow::anyhow!("client initial render exceeds wasm32 memory page count"))
}

pub(crate) fn push_wasm_const_i32_function(out: &mut Vec<u8>, value: i32) {
    let mut body = Vec::new();
    push_wasm_u32_leb(&mut body, 0);
    body.push(0x41);
    push_wasm_i32_leb(&mut body, value);
    body.push(0x0b);
    push_wasm_len(out, body.len());
    out.extend(body);
}

pub(crate) fn push_wasm_section(out: &mut Vec<u8>, id: u8, section: &[u8]) {
    out.push(id);
    push_wasm_len(out, section.len());
    out.extend_from_slice(section);
}

pub(crate) fn push_wasm_len(out: &mut Vec<u8>, len: usize) {
    let len = u32::try_from(len).expect("WASM section length fits in u32");
    push_wasm_u32_leb(out, len);
}

pub(crate) fn push_wasm_u32_leb(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

pub(crate) fn push_wasm_i32_leb(out: &mut Vec<u8>, mut value: i32) {
    loop {
        let byte = (value as u8) & 0x7f;
        value >>= 7;
        let done = (value == 0 && (byte & 0x40) == 0) || (value == -1 && (byte & 0x40) != 0);
        if done {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

pub(crate) fn write_client_js_loader(
    path: &Path,
    entry: &Path,
    binding: &ClientSourceBinding<'_>,
) -> anyhow::Result<()> {
    let reactive_plan = client_reactive_plan_json(entry, binding);
    let reactive_plan_hash = stable_json_hash(&reactive_plan)?;
    let bootstrap = serde_json::to_string_pretty(&serde_json::json!({
        "schemaVersion": 1,
        "runtimeFeatures": ["client_wasm"],
        "manifestUrl": "./manifest.json",
        "reactivePlanUrl": "./reactive-plan.json",
        "wasmUrl": "./app.wasm",
        "manifestReactivePlan": CLIENT_REACTIVE_PLAN_PATH,
        "manifestWasm": CLIENT_WASM_PATH,
        "sourceBundleUrl": "../source-bundle.json",
        "manifestSourceBundle": SOURCE_BUNDLE_PATH,
        "sourceBundleHash": binding.source_bundle_hash,
        "sourceFileCount": binding.source_bundle.files.len(),
        "entry": &binding.source_bundle.entry,
        "embeddedReactivePlan": reactive_plan,
        "embeddedReactivePlanHash": reactive_plan_hash,
        "exports": {
            "start": CLIENT_WASM_START_EXPORT,
            "renderPtr": CLIENT_WASM_RENDER_PTR_EXPORT,
            "renderLen": CLIENT_WASM_RENDER_LEN_EXPORT,
            "memory": CLIENT_WASM_MEMORY_EXPORT,
        },
    }))?;
    let script = CLIENT_JS_LOADER_TEMPLATE.replace("__ORV_BOOTSTRAP__", &bootstrap);
    write_text(path, &script)
}

pub(crate) fn write_client_page_shell(
    path: &Path,
    entry: &Path,
    loader_src: &str,
) -> anyhow::Result<()> {
    let entry = html_attr_escape(&entry.display().to_string());
    let loader_src = html_attr_escape(loader_src);
    let html = format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="orv-runtime" content="client_wasm">
</head>
<body data-orv-client="wasm" data-orv-entry="{entry}">
<div id="orv-root"></div>
<script type="module" src="{loader_src}"></script>
</body>
</html>"#
    );
    write_text(path, &html)
}

pub(crate) struct NativeServerPlanPaths<'a> {
    pub(crate) plan: &'a str,
    pub(crate) artifact: &'a str,
    pub(crate) launcher: &'a str,
    pub(crate) source: &'a str,
    pub(crate) routes_source: &'a str,
    pub(crate) router_source: &'a str,
    pub(crate) handlers_source: &'a str,
    pub(crate) package: &'a str,
    pub(crate) runtime_image_plan: &'a str,
}

pub(crate) fn write_native_server_plan_artifact(
    out: &Path,
    paths: &NativeServerPlanPaths<'_>,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let direct_http = orv_compiler::native_server_direct_http_capable(server_artifact);
    let plan = orv_compiler::NativeServerPlanArtifact {
        schema_version: orv_compiler::NATIVE_SERVER_PLAN_ARTIFACT_VERSION,
        kind: "native_server_plan".to_string(),
        status: native_server_plan_status(direct_http).to_string(),
        runtime: server_artifact.runtime.clone(),
        runtime_features: server_artifact.runtime_features.clone(),
        artifact: paths.artifact.to_string(),
        launcher: paths.launcher.to_string(),
        source: paths.source.to_string(),
        routes_source: paths.routes_source.to_string(),
        router_source: paths.router_source.to_string(),
        handlers_source: paths.handlers_source.to_string(),
        package: paths.package.to_string(),
        runtime_image_plan: paths.runtime_image_plan.to_string(),
        target: orv_compiler::NativeServerTargetArtifact {
            kind: "server_binary".to_string(),
            path: NATIVE_SERVER_BINARY_PATH.to_string(),
            protocol: "http1".to_string(),
        },
        commands: orv_compiler::NativeServerCommands {
            build: vec![
                "cargo".to_string(),
                "build".to_string(),
                "--manifest-path".to_string(),
                paths.package.to_string(),
                "--release".to_string(),
            ],
            run: orv_compiler::NativeServerRunCommand {
                env: HashMap::from([("ORV_BUILD_DIR".to_string(), ".".to_string())]),
                command: vec![NATIVE_SERVER_LAUNCHER_BINARY_PATH.to_string()],
            },
        },
        blocked_by: native_server_plan_blockers(direct_http),
        listen: server_artifact.listen.clone(),
        routes: server_artifact.routes.clone(),
    };
    write_json(&out.join(paths.plan), &serde_json::to_value(plan)?)
}

pub(crate) fn write_native_runtime_image_plan_artifact(
    out: &Path,
    path: &str,
    dockerfile_path: &str,
    server_artifact_path: &str,
    native_server_plan_path: &str,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let direct_http = orv_compiler::native_server_direct_http_capable(server_artifact);
    let plan = orv_compiler::NativeRuntimeImagePlanArtifact {
        schema_version: orv_compiler::NATIVE_RUNTIME_IMAGE_PLAN_ARTIFACT_VERSION,
        kind: "native_runtime_image_plan".to_string(),
        status: native_runtime_image_plan_status(direct_http).to_string(),
        runtime: server_artifact.runtime.clone(),
        runtime_features: server_artifact.runtime_features.clone(),
        artifact: server_artifact_path.to_string(),
        native_plan: native_server_plan_path.to_string(),
        reference_image: ORV_REFERENCE_RUNTIME_IMAGE.to_string(),
        target: orv_compiler::NativeRuntimeImageTargetArtifact {
            kind: "oci_image".to_string(),
            image: NATIVE_RUNTIME_IMAGE_NAME.to_string(),
            binary: NATIVE_SERVER_BINARY_PATH.to_string(),
            protocol: "http1".to_string(),
        },
        dockerfile: dockerfile_path.to_string(),
        commands: orv_compiler::NativeRuntimeImageCommands {
            build: vec![
                "docker".to_string(),
                "build".to_string(),
                "-f".to_string(),
                dockerfile_path.to_string(),
                "-t".to_string(),
                NATIVE_RUNTIME_IMAGE_NAME.to_string(),
                ".".to_string(),
            ],
        },
        blocked_by: native_runtime_image_plan_blockers(direct_http),
        listen: server_artifact.listen.clone(),
        routes: server_artifact.routes.clone(),
    };
    write_json(&out.join(path), &serde_json::to_value(plan)?)
}

pub(crate) fn native_server_plan_status(direct_http: bool) -> &'static str {
    if direct_http {
        "direct_http"
    } else {
        "planned"
    }
}

pub(crate) fn native_runtime_image_plan_status(direct_http: bool) -> &'static str {
    if direct_http {
        "image_planned"
    } else {
        "planned"
    }
}

pub(crate) fn native_server_plan_blockers(direct_http: bool) -> Vec<String> {
    if direct_http {
        Vec::new()
    } else {
        vec![
            "native-codegen".to_string(),
            "native-runtime-image".to_string(),
        ]
    }
}

pub(crate) fn native_runtime_image_plan_blockers(direct_http: bool) -> Vec<String> {
    if direct_http {
        Vec::new()
    } else {
        vec![
            "native-codegen".to_string(),
            "native-runtime-image".to_string(),
        ]
    }
}

pub(crate) fn write_native_runtime_image_dockerfile(out: &Path, path: &str) -> anyhow::Result<()> {
    let dockerfile = r#"FROM rust:1-bookworm AS build
WORKDIR /work
COPY server/native /work/server/native
RUN cargo build --manifest-path /work/server/native/Cargo.toml --release

FROM debian:bookworm-slim
WORKDIR /app
COPY . /app
COPY --from=build /work/server/native/target/release/orv-native-server /app/server/app
ENV ORV_BUILD_DIR=/app
ENTRYPOINT ["/app/server/app"]
"#;
    write_text(&out.join(path), dockerfile)
}

pub(crate) fn write_native_server_launcher_source(
    out: &Path,
    path: &str,
    server_artifact_path: &str,
    native_server_plan_path: &str,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let source = orv_compiler::native_server_launcher_source(
        server_artifact_path,
        native_server_plan_path,
        server_artifact,
    );
    write_text(&out.join(path), &source)
}

pub(crate) fn write_native_server_routes_source(
    out: &Path,
    path: &str,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let source = orv_compiler::native_server_routes_source(server_artifact);
    write_text(&out.join(path), &source)
}

pub(crate) fn write_native_server_router_source(out: &Path, path: &str) -> anyhow::Result<()> {
    let source = orv_compiler::native_server_router_source();
    write_text(&out.join(path), &source)
}

pub(crate) fn write_native_server_handlers_source(
    out: &Path,
    path: &str,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let source = orv_compiler::native_server_handlers_source(server_artifact);
    write_text(&out.join(path), &source)
}

pub(crate) fn write_native_server_launcher_package(out: &Path, path: &str) -> anyhow::Result<()> {
    let manifest = r#"[package]
name = "orv-native-server"
version = "0.1.0"
edition = "2021"
publish = false

[[bin]]
name = "orv-native-server"
path = "main.rs"
"#;
    write_text(&out.join(path), manifest)
}

pub(crate) fn relative_bundle_path(from: &str, to: &str) -> String {
    let depth = from.split('/').count().saturating_sub(1);
    format!("{}{}", "../".repeat(depth), to)
}

pub(crate) fn html_attr_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Clone, Copy)]
pub(crate) struct ProdBuildTargets<'a> {
    pub(crate) static_page: Option<&'a str>,
    pub(crate) client_manifest: Option<&'a str>,
    pub(crate) client_reactive_plan: Option<&'a str>,
    pub(crate) client_page: Option<&'a str>,
    pub(crate) client_js: Option<&'a str>,
    pub(crate) client_wasm: Option<&'a str>,
    pub(crate) server_artifact: &'a str,
    pub(crate) native_server_plan: &'a str,
    pub(crate) native_runtime_image_plan: &'a str,
    pub(crate) native_server_routes_source: &'a str,
    pub(crate) native_server_router_source: &'a str,
    pub(crate) native_server_handlers_source: &'a str,
}

pub(crate) fn write_prod_deploy_artifacts(
    out: &Path,
    entry: &Path,
    manifest: &orv_compiler::BuildManifest,
    origin_map: &orv_compiler::OriginMap,
    server_artifact: Option<&orv_compiler::ServerRuntimeArtifact>,
    targets: ProdBuildTargets<'_>,
) -> anyhow::Result<()> {
    let client = prod_deploy_client_json(out, manifest.capabilities.client_wasm, targets)?;
    let server = if let Some(server_artifact) = server_artifact {
        let entrypoint = "deploy/server.sh";
        let routes_artifact = "deploy/routes.json";
        let container = "deploy/container.json";
        let dockerfile = "deploy/Dockerfile";
        let compose = "deploy/compose.yaml";
        let env_example = "deploy/env.example";
        let db_adapters = "deploy/db-adapters.json";
        let commerce_adapters = "deploy/commerce-adapters.json";
        let smoke_test = DEPLOY_SMOKE_TEST_PATH;
        let smoke_output = DEPLOY_SMOKE_OUTPUT_PATH;
        let preflight = DEPLOY_PREFLIGHT_PATH;
        let benchmark_evidence = DEPLOY_BENCHMARK_EVIDENCE_PATH;
        let runbook = "deploy/README.md";
        let persistence = server_artifact_deploy_persistence(server_artifact)?;
        write_prod_server_entrypoint(out, targets.server_artifact)?;
        write_prod_routes_artifact(out, targets.server_artifact, server_artifact)?;
        write_prod_container_artifacts(
            out,
            targets.server_artifact,
            entrypoint,
            routes_artifact,
            dockerfile,
            server_artifact,
            &persistence,
        )?;
        write_prod_compose_artifact(out, dockerfile, server_artifact, &persistence)?;
        write_prod_env_example_artifact(out, env_example, server_artifact, &persistence)?;
        write_prod_db_adapters_artifact(out, db_adapters, targets.server_artifact, &persistence)?;
        write_prod_commerce_adapters_artifact(
            out,
            commerce_adapters,
            targets.server_artifact,
            &persistence,
        )?;
        write_prod_smoke_test_artifact(
            out,
            smoke_test,
            server_artifact,
            origin_map,
            &persistence,
            &client,
        )?;
        let deploy_artifacts = DeployRunbookArtifacts {
            server_artifact: targets.server_artifact,
            compose,
            env_example,
            db_adapters,
            commerce_adapters,
            smoke_test,
            smoke_output,
            preflight,
            benchmark_evidence,
            runbook,
            routes: routes_artifact,
        };
        write_prod_preflight_artifact(
            out,
            preflight,
            &deploy_artifacts,
            server_artifact,
            &persistence,
            &client,
        )?;
        write_prod_benchmark_evidence_artifact(
            out,
            benchmark_evidence,
            &deploy_artifacts,
            server_artifact,
            &persistence,
            &client,
        )?;
        write_prod_deploy_runbook(
            out,
            &deploy_artifacts,
            server_artifact,
            &persistence,
            &client,
        )?;
        serde_json::json!({
            "runtime": server_artifact.runtime.clone(),
            "runtime_features": server_artifact.runtime_features.clone(),
            "artifact": targets.server_artifact,
            "entrypoint": entrypoint,
            "routes_artifact": routes_artifact,
            "native_plan": targets.native_server_plan,
            "native_runtime_image_plan": targets.native_runtime_image_plan,
            "native_routes_source": targets.native_server_routes_source,
            "native_router_source": targets.native_server_router_source,
            "native_handlers_source": targets.native_server_handlers_source,
            "container": container,
            "dockerfile": dockerfile,
            "compose": compose,
            "env_example": env_example,
            "db_adapters": db_adapters,
            "commerce_adapters": commerce_adapters,
            "smoke_test": smoke_test,
            "smoke_output": smoke_output,
            "preflight": preflight,
            "benchmark_evidence": benchmark_evidence,
            "runbook": runbook,
            "runtime_image": ORV_REFERENCE_RUNTIME_IMAGE,
            "protocol": "http1",
            "listen": server_artifact.listen.clone(),
            "routes": server_artifact.routes.clone(),
            "persistence": deploy_persistence_value(&persistence),
        })
    } else {
        serde_json::Value::Null
    };
    let static_target = targets.static_page.map_or(serde_json::Value::Null, |path| {
        serde_json::json!({
            "path": path,
            "runtime_features": [],
        })
    });
    let deploy = serde_json::json!({
        "schema_version": 1,
        "profile": "prod",
        "entry": entry.display().to_string(),
        "runtime": manifest.runtime.clone(),
        "runtime_features": manifest.capabilities.runtime_features.clone(),
        "source_bundle": "source-bundle.json",
        "server": server,
        "static": static_target,
        "client": client,
    });
    write_json(&out.join("deploy").join("manifest.json"), &deploy)
}

pub(crate) fn prod_deploy_client_json(
    out: &Path,
    enabled: bool,
    targets: ProdBuildTargets<'_>,
) -> anyhow::Result<serde_json::Value> {
    if !enabled {
        return Ok(serde_json::Value::Null);
    }
    let client_manifest = targets
        .client_manifest
        .ok_or_else(|| anyhow::anyhow!("missing client_manifest bundle target"))?;
    let client_manifest_value = read_json_value(&out.join(client_manifest))?;
    let reactive_plan = targets
        .client_reactive_plan
        .ok_or_else(|| anyhow::anyhow!("missing client_reactive_plan bundle target"))?;
    if json_str(&client_manifest_value, "reactive_plan", "client manifest")? != reactive_plan {
        anyhow::bail!("client manifest reactive_plan does not match bundle target");
    }
    Ok(serde_json::json!({
        "manifest": client_manifest,
        "reactive_plan": reactive_plan,
        "page": targets.client_page.ok_or_else(|| anyhow::anyhow!("missing client_page bundle target"))?,
        "loader": targets.client_js.ok_or_else(|| anyhow::anyhow!("missing client_js bundle target"))?,
        "wasm": targets.client_wasm.ok_or_else(|| anyhow::anyhow!("missing client_wasm bundle target"))?,
        "runtime_features": ["client_wasm"],
        "capabilities": client_manifest_value
            .get("capabilities")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "blocked_by": client_manifest_value
            .get("blocked_by")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "blockers": client_manifest_value
            .get("blockers")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    }))
}

pub(crate) fn write_prod_routes_artifact(
    out: &Path,
    server_artifact_path: &str,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let routes = serde_json::json!({
        "schema_version": 1,
        "artifact": server_artifact_path,
        "runtime": server_artifact.runtime.clone(),
        "protocol": "http1",
        "routes": server_artifact.routes.clone(),
    });
    write_json(&out.join("deploy").join("routes.json"), &routes)
}

pub(crate) fn write_prod_container_artifacts(
    out: &Path,
    server_artifact_path: &str,
    entrypoint: &str,
    routes_artifact: &str,
    dockerfile_path: &str,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
) -> anyhow::Result<()> {
    let container = serde_json::json!({
        "schema_version": 1,
        "kind": "reference-server-container",
        "dockerfile": dockerfile_path,
        "artifact": server_artifact_path,
        "entrypoint": entrypoint,
        "routes_artifact": routes_artifact,
        "runtime": server_artifact.runtime.clone(),
        "runtime_image": ORV_REFERENCE_RUNTIME_IMAGE,
        "protocol": "http1",
        "listen": server_artifact.listen.clone(),
        "ports": deploy_ports_value(server_artifact.listen.as_ref()),
        "command": ["./deploy/server.sh"],
        "persistence": deploy_persistence_value(persistence),
    });
    write_json(&out.join("deploy").join("container.json"), &container)?;
    let expose = deploy_exposed_port(server_artifact.listen.as_ref())
        .map(|port| format!("EXPOSE {port}\n"))
        .unwrap_or_default();
    let dockerfile = format!(
        r#"ARG ORV_RUNTIME_IMAGE={ORV_REFERENCE_RUNTIME_IMAGE}
FROM ${{ORV_RUNTIME_IMAGE}}
WORKDIR /app
COPY . /app
{expose}ENTRYPOINT ["./deploy/server.sh"]
"#
    );
    write_text(&out.join(dockerfile_path), &dockerfile)
}

pub(crate) fn write_prod_compose_artifact(
    out: &Path,
    dockerfile_path: &str,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
) -> anyhow::Result<()> {
    let ports = deploy_compose_ports(server_artifact.listen.as_ref());
    let environment = deploy_compose_environment(server_artifact.listen.as_ref(), persistence);
    let volumes = deploy_compose_volumes(persistence);
    let compose = format!(
        r#"services:
  orv-app:
    build:
      context: ..
      dockerfile: {dockerfile_path}
      args:
        ORV_RUNTIME_IMAGE: {ORV_REFERENCE_RUNTIME_IMAGE}
    image: orv-reference-app:latest
{ports}{environment}{volumes}"#
    );
    write_text(&out.join("deploy").join("compose.yaml"), &compose)
}

pub(crate) fn write_prod_env_example_artifact(
    out: &Path,
    path: &str,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
) -> anyhow::Result<()> {
    let mut env_example = String::from("# orv deploy environment\n");
    for assignment in deploy_env_example_assignments(server_artifact.listen.as_ref(), persistence) {
        let _ = writeln!(env_example, "{assignment}");
    }
    write_text(&out.join(path), &env_example)
}

pub(crate) fn write_prod_commerce_adapters_artifact(
    out: &Path,
    path: &str,
    server_artifact_path: &str,
    persistence: &DeployPersistence,
) -> anyhow::Result<()> {
    let artifact = serde_json::json!({
        "schema_version": 1,
        "kind": "orv.deploy.commerce_adapters",
        "artifact": server_artifact_path,
        "adapters": deploy_commerce_adapter_value(&persistence.commerce_adapters),
    });
    write_json(&out.join(path), &artifact)
}

pub(crate) fn write_prod_db_adapters_artifact(
    out: &Path,
    path: &str,
    server_artifact_path: &str,
    persistence: &DeployPersistence,
) -> anyhow::Result<()> {
    let artifact = serde_json::json!({
        "schema_version": 1,
        "kind": "orv.deploy.db_adapters",
        "artifact": server_artifact_path,
        "adapters": deploy_db_adapter_value(&persistence.db_adapters),
    });
    write_json(&out.join(path), &artifact)
}

pub(crate) fn write_prod_preflight_artifact(
    out: &Path,
    path: &str,
    artifacts: &DeployRunbookArtifacts<'_>,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
    client: &serde_json::Value,
) -> anyhow::Result<()> {
    let preflight =
        deploy_preflight_artifact_value(artifacts, server_artifact, persistence, Some(client));
    write_json(&out.join(path), &preflight)
}

pub(crate) fn write_prod_benchmark_evidence_artifact(
    out: &Path,
    path: &str,
    artifacts: &DeployRunbookArtifacts<'_>,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
    client: &serde_json::Value,
) -> anyhow::Result<()> {
    let evidence = deploy_benchmark_evidence_artifact_value(
        artifacts,
        server_artifact,
        persistence,
        Some(client),
    )?;
    write_json(&out.join(path), &evidence)
}

pub(crate) fn deploy_preflight_artifact_value(
    artifacts: &DeployRunbookArtifacts<'_>,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
    client: Option<&serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.deploy.preflight",
        "artifact": artifacts.server_artifact,
        "runtime": server_artifact.runtime.clone(),
        "runtime_features": server_artifact.runtime_features.clone(),
        "security_features": deploy_security_runtime_features(&server_artifact.runtime_features),
        "listen": server_artifact.listen.clone(),
        "routes": server_artifact.routes.clone(),
        "persistence": deploy_persistence_value(persistence),
        "required_env": deploy_preflight_env_values(server_artifact.listen.as_ref(), persistence, true),
        "optional_env": deploy_preflight_env_values(server_artifact.listen.as_ref(), persistence, false),
        "commands": deploy_preflight_commands_value(artifacts),
        "artifacts": deploy_preflight_artifacts_value(artifacts),
        "smoke_output_contract": deploy_smoke_output_contract_value(artifacts),
        "benchmark": deploy_preflight_benchmark_value(),
        "client": deploy_preflight_client_value(client),
    })
}

pub(crate) fn deploy_preflight_commands_value(
    artifacts: &DeployRunbookArtifacts<'_>,
) -> serde_json::Value {
    serde_json::json!({
        "verify_build": "orv verify-build .",
        "env_check": "orv deploy-env-check .",
        "run_build": "orv run-build .",
        "smoke_test": format!("./{}", artifacts.smoke_test),
        "editor_run_debug": "orv editor run-debug . --control next",
        "benchmark_report": "orv benchmark-report .",
        "benchmark_report_require_pass": "orv benchmark-report . --require-pass",
        "compose_up": format!("docker compose -f {} up --build -d", artifacts.compose),
        "trace": "./deploy/server.sh --trace deploy/request-trace.json",
        "trace_run_build": "orv run-build . --trace deploy/request-trace.json",
        "editor_trace": "orv editor trace . --trace deploy/request-trace.json",
        "trace_stream_smoke": format!("ORV_SMOKE_TRACE_STREAM=1 ./{}", artifacts.smoke_test),
    })
}

pub(crate) fn deploy_preflight_artifacts_value(
    artifacts: &DeployRunbookArtifacts<'_>,
) -> serde_json::Value {
    serde_json::json!({
        "server": artifacts.server_artifact,
        "routes": artifacts.routes,
        "source_bundle": SOURCE_BUNDLE_PATH,
        "project_graph": "project-graph.json",
        "origin_map": "origin-map.json",
        "build_manifest": "build-manifest.json",
        "bundle_plan": "bundle-plan.json",
        "env_example": artifacts.env_example,
        "db_adapters": artifacts.db_adapters,
        "commerce_adapters": artifacts.commerce_adapters,
        "smoke_test": artifacts.smoke_test,
        "smoke_output": artifacts.smoke_output,
        "preflight": artifacts.preflight,
        "benchmark_evidence": artifacts.benchmark_evidence,
        "runbook": artifacts.runbook,
    })
}

pub(crate) fn deploy_smoke_output_contract_value(
    artifacts: &DeployRunbookArtifacts<'_>,
) -> serde_json::Value {
    smoke_output_contract_value(artifacts.smoke_output)
}

pub(crate) fn smoke_output_contract_value(smoke_output: &str) -> serde_json::Value {
    serde_json::json!({
        "output": smoke_output,
        "required_markers": deploy_benchmark::smoke_required_markers_value(),
    })
}

pub(crate) fn deploy_benchmark_evidence_artifact_value(
    artifacts: &DeployRunbookArtifacts<'_>,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
    client: Option<&serde_json::Value>,
) -> anyhow::Result<serde_json::Value> {
    let preflight =
        deploy_preflight_artifact_value(artifacts, server_artifact, persistence, client);
    let preflight_hash = stable_json_hash(&preflight)?;
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.benchmark.shop_5h.evidence",
        "preflight": artifacts.preflight,
        "preflight_hash": preflight_hash,
        "benchmark": deploy_preflight_benchmark_value(),
        "commands": deploy_preflight_commands_value(artifacts),
        "artifacts": deploy_preflight_artifacts_value(artifacts),
        "smoke_output_contract": deploy_smoke_output_contract_value(artifacts),
        "recording_status": "not_recorded",
        "task_entries": deploy_benchmark::evidence_task_entries_value(),
        "data": deploy_benchmark::evidence_data_value(),
    }))
}

pub(crate) fn deploy_preflight_benchmark_value() -> serde_json::Value {
    deploy_benchmark::preflight_contract_value()
}

pub(crate) fn deploy_preflight_client_value(
    client: Option<&serde_json::Value>,
) -> serde_json::Value {
    let Some(client) = client.filter(|value| !value.is_null()) else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "manifest": client.get("manifest").and_then(serde_json::Value::as_str),
        "page": client.get("page").and_then(serde_json::Value::as_str),
        "loader": client.get("loader").and_then(serde_json::Value::as_str),
        "wasm": client.get("wasm").and_then(serde_json::Value::as_str),
        "runtime_features": client.get("runtime_features").cloned().unwrap_or_else(|| serde_json::json!([])),
        "capabilities": client.get("capabilities").cloned().unwrap_or(serde_json::Value::Null),
        "blocked_by": client.get("blocked_by").cloned().unwrap_or_else(|| serde_json::json!([])),
        "blockers": client.get("blockers").cloned().unwrap_or_else(|| serde_json::json!([])),
    })
}

pub(crate) fn write_prod_smoke_test_artifact(
    out: &Path,
    path: &str,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
    origin_map: &orv_compiler::OriginMap,
    persistence: &DeployPersistence,
    client: &serde_json::Value,
) -> anyhow::Result<()> {
    let mut script = format!(
        r#"#!/usr/bin/env sh
set -eu
ORV_SMOKE_SCRIPT_DIR=$(CDPATH= cd "$(dirname "$0")" && pwd)
ORV_SMOKE_BUILD_DIR=$(CDPATH= cd "$ORV_SMOKE_SCRIPT_DIR/.." && pwd)
cd "$ORV_SMOKE_BUILD_DIR"
BASE_URL="${{ORV_BASE_URL:-{}}}"
ORV_BIN="${{ORV_BIN:-orv}}"
ORV_SMOKE_OUTPUT="${{ORV_SMOKE_OUTPUT:-{}}}"
ORV_SMOKE_DAP_SUMMARY_OUTPUT=""

if ! command -v curl >/dev/null 2>&1; then
  printf 'orv deploy smoke test requires curl\n' >&2
  exit 127
fi

if ! command -v "$ORV_BIN" >/dev/null 2>&1; then
  printf 'orv deploy smoke test requires orv; set ORV_BIN to the local binary path\n' >&2
  exit 127
fi

orv_smoke_reveal_contains() {{
  label="$1"
  origin_id="$2"
  pattern="$3"
  output_path="$(mktemp)"
  if ! "$ORV_BIN" reveal . "$origin_id" > "$output_path"; then
    rm -f "$output_path"
    printf 'orv deploy smoke test failed: %s reveal command\n' "$label" >&2
    exit 1
  fi
  if ! grep -F "$pattern" "$output_path" >/dev/null; then
    rm -f "$output_path"
    printf 'orv deploy smoke test failed: %s reveal missing %s\n' "$label" "$pattern" >&2
    exit 1
  fi
  rm -f "$output_path"
}}

orv_smoke_editor_reveal_contains() {{
  label="$1"
  origin_id="$2"
  pattern="$3"
  output_path="$(mktemp)"
  if ! "$ORV_BIN" editor reveal . "$origin_id" > "$output_path"; then
    rm -f "$output_path"
    printf 'orv deploy smoke test failed: %s editor reveal command\n' "$label" >&2
    exit 1
  fi
  if ! grep -F "$pattern" "$output_path" >/dev/null; then
    rm -f "$output_path"
    printf 'orv deploy smoke test failed: %s editor reveal missing %s\n' "$label" "$pattern" >&2
    exit 1
  fi
  rm -f "$output_path"
}}

orv_smoke_lsp_reveal_contains() {{
  label="$1"
  origin_id="$2"
  pattern="$3"
  output_path="$(mktemp)"
  if ! "$ORV_BIN" lsp reveal . "$origin_id" > "$output_path"; then
    rm -f "$output_path"
    printf 'orv deploy smoke test failed: %s lsp reveal command\n' "$label" >&2
    exit 1
  fi
  if ! grep -F "$pattern" "$output_path" >/dev/null; then
    rm -f "$output_path"
    printf 'orv deploy smoke test failed: %s lsp reveal missing %s\n' "$label" "$pattern" >&2
    exit 1
  fi
  rm -f "$output_path"
}}

orv_smoke_dap_summary_capture() {{
  if [ -n "$ORV_SMOKE_DAP_SUMMARY_OUTPUT" ] && [ -f "$ORV_SMOKE_DAP_SUMMARY_OUTPUT" ]; then
    return 0
  fi
  output_path="$(mktemp)"
  if ! "$ORV_BIN" editor run-debug . --control next > "$output_path"; then
    rm -f "$output_path"
    printf 'orv deploy smoke test failed: DAP editor run-debug command\n' >&2
    exit 1
  fi
  ORV_SMOKE_DAP_SUMMARY_OUTPUT="$output_path"
}}

orv_smoke_dap_summary_contains() {{
  label="$1"
  pattern="$2"
  orv_smoke_dap_summary_capture
  if ! grep -F "$pattern" "$ORV_SMOKE_DAP_SUMMARY_OUTPUT" >/dev/null; then
    printf 'orv deploy smoke test failed: %s editor run-debug missing %s\n' "$label" "$pattern" >&2
    exit 1
  fi
}}

orv_smoke_dap_summary_cleanup() {{
  if [ -n "$ORV_SMOKE_DAP_SUMMARY_OUTPUT" ]; then
    rm -f "$ORV_SMOKE_DAP_SUMMARY_OUTPUT"
    ORV_SMOKE_DAP_SUMMARY_OUTPUT=""
  fi
}}

orv_smoke_trace_stream() {{
  if [ "${{ORV_SMOKE_TRACE_STREAM:-0}}" != "1" ]; then
    return 0
  fi
  events_path="${{ORV_SMOKE_TRACE_EVENTS:-deploy/trace-events.sse}}"
  output_path="$(mktemp)"
  rm -f "$events_path"
  if ! curl -fsS --max-time "${{ORV_SMOKE_TRACE_TIMEOUT:-2}}" "$BASE_URL/__orv/trace/events" > "$events_path" 2>/dev/null; then
    if ! grep -F 'event: orv:trace' "$events_path" >/dev/null 2>&1; then
      rm -f "$output_path"
      printf 'orv deploy smoke test failed: live trace stream unavailable; start server with --trace deploy/request-trace.json\n' >&2
      exit 1
    fi
  fi
  for pattern in 'event: orv:trace' 'orv.production.trace' 'event: orv:trace.frame'; do
    if ! grep -F "$pattern" "$events_path" >/dev/null; then
      rm -f "$output_path"
      printf 'orv deploy smoke test failed: live trace stream missing %s\n' "$pattern" >&2
      exit 1
    fi
  done
  if ! "$ORV_BIN" editor trace-stream . --events "$events_path" > "$output_path"; then
    rm -f "$output_path"
    printf 'orv deploy smoke test failed: editor trace-stream command\n' >&2
    exit 1
  fi
  for pattern in '"kind": "orv.editor.trace.stream"' '"strategy": "event-source-snapshot"' '"response_navigation"'; do
    if ! grep -F "$pattern" "$output_path" >/dev/null; then
      rm -f "$output_path"
      printf 'orv deploy smoke test failed: editor trace-stream missing %s\n' "$pattern" >&2
      exit 1
    fi
  done
  rm -f "$output_path"
}}

orv_smoke_curl() {{
  label="$1"
  shift
  if ! curl -fsS "$@" >/dev/null; then
    printf 'orv deploy smoke test failed: %s\n' "$label" >&2
    exit 1
  fi
}}

orv_smoke_origin_header() {{
  label="$1"
  headers_path="$2"
  expected_origin="$3"
  actual_origin="$(tr -d '\r' < "$headers_path" | awk '
    {{
      lower = tolower($0)
      if (index(lower, "x-orv-origin-id:") == 1) {{
        value = substr($0, index($0, ":") + 1)
        sub(/^[[:space:]]*/, "", value)
        sub(/[[:space:]]*$/, "", value)
        print value
        exit
      }}
    }}
  ')"
  if [ -z "$actual_origin" ]; then
    printf 'orv deploy smoke test failed: %s missing x-orv-origin-id\n' "$label" >&2
    exit 1
  fi
  if [ "$actual_origin" != "$expected_origin" ]; then
    printf 'orv deploy smoke test failed: %s wrong x-orv-origin-id expected %s got %s\n' "$label" "$expected_origin" "$actual_origin" >&2
    exit 1
  fi
}}

orv_smoke_response_origin_header() {{
  label="$1"
  headers_path="$2"
  expected_response_origin="$3"
  actual_response_origin="$(tr -d '\r' < "$headers_path" | awk '
    {{
      lower = tolower($0)
      if (index(lower, "x-orv-response-origin-id:") == 1) {{
        value = substr($0, index($0, ":") + 1)
        sub(/^[[:space:]]*/, "", value)
        sub(/[[:space:]]*$/, "", value)
        print value
        exit
      }}
    }}
  ')"
  if [ -z "$actual_response_origin" ]; then
    printf 'orv deploy smoke test failed: %s missing x-orv-response-origin-id\n' "$label" >&2
    exit 1
  fi
  if [ "$actual_response_origin" != "$expected_response_origin" ]; then
    printf 'orv deploy smoke test failed: %s wrong x-orv-response-origin-id expected %s got %s\n' "$label" "$expected_response_origin" "$actual_response_origin" >&2
    exit 1
  fi
}}

orv_smoke_curl_origin() {{
  label="$1"
  expected_origin="$2"
  shift 2
  orv_smoke_tmp_headers="$(mktemp)"
  if ! curl -fsS -D "$orv_smoke_tmp_headers" "$@" >/dev/null; then
    rm -f "$orv_smoke_tmp_headers"
    printf 'orv deploy smoke test failed: %s\n' "$label" >&2
    exit 1
  fi
  orv_smoke_origin_header "$label" "$orv_smoke_tmp_headers" "$expected_origin"
  rm -f "$orv_smoke_tmp_headers"
}}

orv_smoke_curl_origin_response() {{
  label="$1"
  expected_origin="$2"
  expected_response_origin="$3"
  shift 3
  orv_smoke_tmp_headers="$(mktemp)"
  if ! curl -fsS -D "$orv_smoke_tmp_headers" "$@" >/dev/null; then
    rm -f "$orv_smoke_tmp_headers"
    printf 'orv deploy smoke test failed: %s\n' "$label" >&2
    exit 1
  fi
  orv_smoke_origin_header "$label" "$orv_smoke_tmp_headers" "$expected_origin"
  orv_smoke_response_origin_header "$label" "$orv_smoke_tmp_headers" "$expected_response_origin"
  rm -f "$orv_smoke_tmp_headers"
}}

orv_smoke_curl_capture_origin() {{
  label="$1"
  headers_path="$2"
  expected_origin="$3"
  shift 3
  if ! curl -fsS -D "$headers_path" "$@" >/dev/null; then
    printf 'orv deploy smoke test failed: %s\n' "$label" >&2
    exit 1
  fi
  orv_smoke_origin_header "$label" "$headers_path" "$expected_origin"
}}

orv_smoke_fetch() {{
  label="$1"
  output_path="$2"
  shift 2
  if ! curl -fsS "$@" > "$output_path"; then
    printf 'orv deploy smoke test failed: %s\n' "$label" >&2
    exit 1
  fi
}}

orv_smoke_fetch_origin() {{
  label="$1"
  output_path="$2"
  expected_origin="$3"
  shift 3
  orv_smoke_tmp_headers="$(mktemp)"
  if ! curl -fsS -D "$orv_smoke_tmp_headers" "$@" > "$output_path"; then
    rm -f "$orv_smoke_tmp_headers"
    printf 'orv deploy smoke test failed: %s\n' "$label" >&2
    exit 1
  fi
  orv_smoke_origin_header "$label" "$orv_smoke_tmp_headers" "$expected_origin"
  rm -f "$orv_smoke_tmp_headers"
}}

orv_smoke_fetch_capture_origin() {{
  label="$1"
  output_path="$2"
  headers_path="$3"
  expected_origin="$4"
  shift 4
  if ! curl -fsS -D "$headers_path" "$@" > "$output_path"; then
    printf 'orv deploy smoke test failed: %s\n' "$label" >&2
    exit 1
  fi
  orv_smoke_origin_header "$label" "$headers_path" "$expected_origin"
}}

orv_smoke_body_contains() {{
  label="$1"
  body_path="$2"
  pattern="$3"
  if ! grep -F "$pattern" "$body_path" >/dev/null; then
    printf 'orv deploy smoke test failed: %s\n' "$label" >&2
    exit 1
  fi
}}

orv_smoke_file() {{
  path="$1"
  if [ ! -f "$path" ]; then
    printf 'orv deploy smoke test missing file: %s\n' "$path" >&2
    exit 1
  fi
}}

orv_smoke_grep() {{
  label="$1"
  path="$2"
  pattern="$3"
  if ! grep -F "$pattern" "$path" >/dev/null; then
    printf 'orv deploy smoke test failed: %s\n' "$label" >&2
    exit 1
  fi
}}

orv_smoke_graph_contract() {{
  for path in source-bundle.json project-graph.json origin-map.json build-manifest.json; do
    orv_smoke_file "$path"
  done
  orv_smoke_grep "source bundle schema" "source-bundle.json" '"schema_version": 1'
  orv_smoke_grep "source bundle files" "source-bundle.json" '"files"'
  orv_smoke_grep "project graph semantic origin map" "project-graph.json" '"origin_map"'
  orv_smoke_grep "project graph origin links" "project-graph.json" '"origin_links"'
  orv_smoke_grep "origin map entries" "origin-map.json" '"entries"'
  if ! "$ORV_BIN" verify-build . >/dev/null; then
    printf 'orv deploy smoke test failed: verify-build graph contract\n' >&2
    exit 1
  fi
}}

orv_smoke_db_bridge_schema() {{
  label="$1"
  endpoint="$2"
  provider="$3"
  adapter_url="$4"
  auth_token="$5"
  if [ -z "$endpoint" ]; then
    printf 'orv deploy smoke test failed: %s missing endpoint\n' "$label" >&2
    exit 1
  fi
  if [ -n "$auth_token" ]; then
    if ! curl -fsS -H 'content-type: application/json' -H 'accept: application/json' -H "authorization: Bearer ${{auth_token}}" --data "{{\"kind\":\"orv.db.adapter\",\"contract\":\"http-json-v1\",\"provider\":\"${{provider}}\",\"url\":\"${{adapter_url}}\",\"method\":\"schema\",\"args\":[]}}" "$endpoint" >/dev/null; then
      printf 'orv deploy smoke test failed: %s\n' "$label" >&2
      exit 1
    fi
    return 0
  fi
  if ! curl -fsS -H 'content-type: application/json' -H 'accept: application/json' --data "{{\"kind\":\"orv.db.adapter\",\"contract\":\"http-json-v1\",\"provider\":\"${{provider}}\",\"url\":\"${{adapter_url}}\",\"method\":\"schema\",\"args\":[]}}" "$endpoint" >/dev/null; then
    printf 'orv deploy smoke test failed: %s\n' "$label" >&2
    exit 1
  fi
}}

orv_smoke_cookie_from_headers() {{
  cookie_name="$1"
  headers_path="$2"
  tr -d '\r' < "$headers_path" | awk -v cookie_name="$cookie_name" '
    {{
      lower = tolower($0)
      if (index(lower, "set-cookie:") == 1) {{
        line = substr($0, length("set-cookie:") + 1)
        sub(/^[[:space:]]*/, "", line)
        split(line, parts, ";")
        split(parts[1], kv, "=")
        if (kv[1] == cookie_name) {{
          print parts[1]
          exit
        }}
      }}
    }}
  '
}}

"#,
        deploy_smoke_base_url(server_artifact.listen.as_ref()),
        DEPLOY_SMOKE_OUTPUT_PATH
    );
    for route in &server_artifact.routes {
        let _ = writeln!(
            script,
            r#"{}="{}""#,
            deploy_smoke_origin_var_name(&route.method, &route.path),
            route.origin_id
        );
        if let Some(response_origin_id) = deploy_smoke_unique_response_origin(route) {
            let _ = writeln!(
                script,
                r#"{}="{}""#,
                deploy_smoke_response_origin_var_name(&route.method, &route.path),
                response_origin_id
            );
        }
    }
    if !client.is_null() {
        let client_origin_id = deploy_smoke_client_reveal_origin(origin_map)
            .ok_or_else(|| anyhow::anyhow!("client bundle smoke requires a revealable origin"))?;
        let _ = writeln!(script, r#"ORV_SMOKE_CLIENT_ORIGIN="{client_origin_id}""#);
    }
    if !server_artifact.routes.is_empty() {
        script.push('\n');
    }
    script.push_str("orv_smoke_graph_contract\n");
    script.push_str(
        r#"orv_smoke_dap_summary_contains "dap graph summary" '"graph_contract_count": 3'
orv_smoke_dap_summary_contains "dap source bundle summary" '"source_bundle_file_count": 1'
orv_smoke_dap_summary_contains "dap source bundle panel" '"source_bundle": {'
orv_smoke_dap_summary_contains "dap source bundle panel path" '"path": "./source-bundle.json"'
orv_smoke_dap_summary_contains "dap source bundle panel file count" '"fileCount": 1'
orv_smoke_dap_summary_contains "dap source bundle panel hash" '"hash":'
orv_smoke_dap_summary_contains "dap smoke required markers" '"smoke_test_required_markers": ['
orv_smoke_dap_summary_contains "dap smoke summary required markers" '"required_markers": ['
orv_smoke_dap_summary_contains "dap smoke marker dap source bundle" '"dap_source_bundle"'
"#,
    );
    if !server_artifact.routes.is_empty() {
        script.push_str(
            r#"orv_smoke_dap_summary_contains "dap native target summary" '"native_server_target_count": 1'
orv_smoke_dap_summary_contains "dap native route summary" '"native_server_route_count": 1'
"#,
        );
    }
    if let Some(route) = server_artifact.routes.first() {
        let origin_ref = deploy_smoke_origin_var_ref(&route.method, &route.path);
        script.push_str(&deploy_smoke_reveal_marker_contract_section(&origin_ref));
    }
    script.push_str(&deploy_smoke_client_contract_section(client));
    script.push_str(&deploy_smoke_client_reveal_section(client));
    script.push_str("orv_smoke_dap_summary_cleanup\n");
    script.push_str(&deploy_smoke_db_adapter_contract_section(persistence));
    script.push_str(&deploy_smoke_output_function_section(
        server_artifact.routes.len(),
        client,
    ));
    if let Some(ready_path) = deploy_smoke_ready_path(server_artifact) {
        let _ = writeln!(
            script,
            r#"READY_PATH="{ready_path}"
for attempt in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do
  if curl -fsS "$BASE_URL$READY_PATH" >/dev/null; then
    break
  fi
  if [ "$attempt" = "30" ]; then
    printf 'orv deploy smoke test failed waiting for %s%s\n' "$BASE_URL" "$READY_PATH" >&2
    exit 1
  fi
  sleep 1
done
"#
        );
    }
    for route in server_artifact.routes.iter().filter(|route| {
        route.method == "GET"
            && !route.path.contains(':')
            && !route.path.starts_with("/admin")
            && route.path != "/account/sessions"
    }) {
        let origin_ref = deploy_smoke_origin_var_ref(&route.method, &route.path);
        if deploy_smoke_unique_response_origin(route).is_some() {
            let response_origin_ref =
                deploy_smoke_response_origin_var_ref(&route.method, &route.path);
            let _ = writeln!(
                script,
                r#"orv_smoke_curl_origin_response "GET {}" "{}" "{}" "$BASE_URL{}""#,
                route.path, origin_ref, response_origin_ref, route.path
            );
            let _ = writeln!(
                script,
                r#"orv_smoke_reveal_contains "reveal GET {} response source" "{}" '@respond'"#,
                route.path, response_origin_ref
            );
            let _ = writeln!(
                script,
                r#"orv_smoke_reveal_contains "reveal GET {} response production" "{}" '"response_origin_dispatch": true'"#,
                route.path, response_origin_ref
            );
            let _ = writeln!(
                script,
                r#"orv_smoke_editor_reveal_contains "editor reveal GET {} response source" "{}" '@respond'"#,
                route.path, response_origin_ref
            );
            let _ = writeln!(
                script,
                r#"orv_smoke_editor_reveal_contains "editor reveal GET {} response production" "{}" '"response_origin_dispatch": true'"#,
                route.path, response_origin_ref
            );
            let _ = writeln!(
                script,
                r#"orv_smoke_lsp_reveal_contains "lsp reveal GET {} response origin" "{}" '"name": "respond"'"#,
                route.path, response_origin_ref
            );
            let _ = writeln!(
                script,
                r#"orv_smoke_lsp_reveal_contains "lsp reveal GET {} response production" "{}" '"response_origin_dispatch": true'"#,
                route.path, response_origin_ref
            );
        } else {
            let _ = writeln!(
                script,
                r#"orv_smoke_curl_origin "GET {}" "{}" "$BASE_URL{}""#,
                route.path, origin_ref, route.path
            );
        }
        let _ = writeln!(
            script,
            r#"orv_smoke_reveal_contains "reveal GET {} route summary" "{}" '"route_target_count": 1'"#,
            route.path, origin_ref
        );
        let _ = writeln!(
            script,
            r#"orv_smoke_reveal_contains "reveal GET {} native target summary" "{}" '"native_server_target_count": 1'"#,
            route.path, origin_ref
        );
        let _ = writeln!(
            script,
            r#"orv_smoke_reveal_contains "reveal GET {} native route summary" "{}" '"native_server_route_count": 1'"#,
            route.path, origin_ref
        );
        let _ = writeln!(
            script,
            r#"orv_smoke_editor_reveal_contains "editor reveal GET {} native target summary" "{}" '"native_server_target_count": 1'"#,
            route.path, origin_ref
        );
        let _ = writeln!(
            script,
            r#"orv_smoke_editor_reveal_contains "editor reveal GET {} native route summary" "{}" '"native_server_route_count": 1'"#,
            route.path, origin_ref
        );
        let _ = writeln!(
            script,
            r#"orv_smoke_lsp_reveal_contains "lsp reveal GET {} native target summary" "{}" '"native_server_target_count": 1'"#,
            route.path, origin_ref
        );
        let _ = writeln!(
            script,
            r#"orv_smoke_lsp_reveal_contains "lsp reveal GET {} native route summary" "{}" '"native_server_route_count": 1'"#,
            route.path, origin_ref
        );
    }
    if deploy_routes_include(server_artifact, "POST", "/checkout") {
        let root_origin = deploy_smoke_origin_var_ref("GET", "/");
        let products_origin = deploy_smoke_origin_var_ref("POST", "/products");
        let members_origin = deploy_smoke_origin_var_ref("POST", "/members");
        let login_origin = deploy_smoke_origin_var_ref("POST", "/members/login");
        let account_origin = deploy_smoke_origin_var_ref("GET", "/account/sessions");
        let cart_items_origin = deploy_smoke_origin_var_ref("POST", "/cart/items");
        let catalog_origin = deploy_smoke_origin_var_ref("GET", "/catalog");
        let cart_origin = deploy_smoke_origin_var_ref("GET", "/cart");
        let checkout_origin = deploy_smoke_origin_var_ref("POST", "/checkout");
        let admin_origin = deploy_smoke_origin_var_ref("GET", "/admin");
        let admin_summary_origin = deploy_smoke_origin_var_ref("GET", "/admin/summary");
        let admin_catalog_origin = deploy_smoke_origin_var_ref("GET", "/admin/catalog");
        let admin_orders_origin = deploy_smoke_origin_var_ref("GET", "/admin/orders");
        let admin_payments_origin = deploy_smoke_origin_var_ref("GET", "/admin/payments");
        let admin_shipments_origin = deploy_smoke_origin_var_ref("GET", "/admin/shipments");
        let admin_webhooks_origin = deploy_smoke_origin_var_ref("GET", "/admin/webhooks");
        let admin_audit_origin = deploy_smoke_origin_var_ref("GET", "/admin/audit");
        let db_connect_origin = origin_map
            .entries
            .iter()
            .find(|entry| entry.kind == "call" && entry.name == "@db.connect")
            .map(|entry| entry.id.clone())
            .unwrap_or_default();
        let payment_connect_origin =
            deploy_smoke_commerce_record_origin(persistence, "payment", "data/payments.jsonl");
        let shipping_connect_origin =
            deploy_smoke_commerce_record_origin(persistence, "shipping", "data/shipments.jsonl");
        let shop_smoke = r#"
SMOKE_ID="${ORV_SMOKE_ID:-$(date +%s)}"
SMOKE_SKU="orv-smoke-sku-${SMOKE_ID}"
SMOKE_SKU_SECOND="orv-smoke-sku-${SMOKE_ID}-2"
SMOKE_SKU_THIRD="orv-smoke-sku-${SMOKE_ID}-3"
SMOKE_BADGE="orv-smoke-badge-${SMOKE_ID}"
SMOKE_BADGE_SECOND="orv-smoke-badge-${SMOKE_ID}-2"
SMOKE_BADGE_THIRD="orv-smoke-badge-${SMOKE_ID}-3"
SMOKE_HANDLE="orv-smoke-${SMOKE_ID}"
SMOKE_EMAIL="${SMOKE_HANDLE}@example.invalid"
SMOKE_PASSWORD="orv-smoke-password-${SMOKE_ID}"
ORV_SMOKE_DB_CONNECT_ORIGIN="__DB_CONNECT_ORIGIN__"
ORV_SMOKE_PAYMENT_CONNECT_ORIGIN="__PAYMENT_CONNECT_ORIGIN__"
ORV_SMOKE_SHIPPING_CONNECT_ORIGIN="__SHIPPING_CONNECT_ORIGIN__"
SMOKE_HEADERS="$(mktemp)"
SMOKE_MEMBER_HEADERS="$(mktemp)"
SMOKE_ADMIN_HEADERS="$(mktemp)"
SMOKE_HOME_BODY="$(mktemp)"
SMOKE_CATALOG_BODY="$(mktemp)"
SMOKE_CART_BODY="$(mktemp)"
SMOKE_ACCOUNT_BODY="$(mktemp)"
SMOKE_CHECKOUT_BODY="$(mktemp)"
SMOKE_ADMIN_BODY="$(mktemp)"
SMOKE_ADMIN_SUMMARY_BODY="$(mktemp)"
SMOKE_ADMIN_CATALOG_BODY="$(mktemp)"
SMOKE_ADMIN_ORDERS_BODY="$(mktemp)"
SMOKE_ADMIN_PAYMENTS_BODY="$(mktemp)"
SMOKE_ADMIN_SHIPMENTS_BODY="$(mktemp)"
SMOKE_ADMIN_WEBHOOKS_BODY="$(mktemp)"
SMOKE_ADMIN_AUDIT_BODY="$(mktemp)"
trap 'rm -f "$SMOKE_HEADERS" "$SMOKE_MEMBER_HEADERS" "$SMOKE_ADMIN_HEADERS" "$SMOKE_HOME_BODY" "$SMOKE_CATALOG_BODY" "$SMOKE_CART_BODY" "$SMOKE_ACCOUNT_BODY" "$SMOKE_CHECKOUT_BODY" "$SMOKE_ADMIN_BODY" "$SMOKE_ADMIN_SUMMARY_BODY" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_ADMIN_ORDERS_BODY" "$SMOKE_ADMIN_PAYMENTS_BODY" "$SMOKE_ADMIN_SHIPMENTS_BODY" "$SMOKE_ADMIN_WEBHOOKS_BODY" "$SMOKE_ADMIN_AUDIT_BODY"' EXIT

orv_smoke_fetch_capture_origin "GET / home" "$SMOKE_HOME_BODY" "$SMOKE_HEADERS" "__ROOT_ORIGIN__" "$BASE_URL/"
orv_smoke_body_contains "home title" "$SMOKE_HOME_BODY" 'Miol Shop'
orv_smoke_body_contains "home copy" "$SMOKE_HOME_BODY" 'Catalog, member signup, payment capture, and shipment booking are ready.'
orv_smoke_body_contains "home theme surface" "$SMOKE_HOME_BODY" 'background-color: #f8fafc'
orv_smoke_body_contains "home theme typography" "$SMOKE_HOME_BODY" 'font-family: Inter, system-ui, sans-serif'
orv_smoke_reveal_contains "reveal GET / source" "__ROOT_ORIGIN__" '@route GET /'
orv_smoke_reveal_contains "reveal GET / production" "__ROOT_ORIGIN__" '"path": "/"'
orv_smoke_editor_reveal_contains "editor reveal GET / source" "__ROOT_ORIGIN__" '@route GET /'
orv_smoke_editor_reveal_contains "editor reveal GET / production" "__ROOT_ORIGIN__" '"path": "/"'
orv_smoke_lsp_reveal_contains "lsp reveal GET / origin" "__ROOT_ORIGIN__" '"name": "GET /"'
orv_smoke_lsp_reveal_contains "lsp reveal GET / production" "__ROOT_ORIGIN__" '"path": "/"'
if [ -n "$ORV_SMOKE_DB_CONNECT_ORIGIN" ]; then
  orv_smoke_reveal_contains "reveal DB source" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '@db.connect'
  orv_smoke_reveal_contains "reveal DB preflight" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight"'
  orv_smoke_reveal_contains "reveal DB smoke summary" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"smoke_test_summary"'
  orv_smoke_reveal_contains "reveal DB smoke summary count" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight_smoke_summary_missing_count"'
  orv_smoke_reveal_contains "reveal DB sqlite path" "$ORV_SMOKE_DB_CONNECT_ORIGIN" 'sqlite://data/shop.sqlite'
  orv_smoke_editor_reveal_contains "editor reveal DB source" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '@db.connect'
  orv_smoke_editor_reveal_contains "editor reveal DB preflight" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight"'
  orv_smoke_editor_reveal_contains "editor reveal DB smoke summary" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"smoke_test_summary"'
  orv_smoke_editor_reveal_contains "editor reveal DB smoke summary count" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight_smoke_summary_missing_count"'
  orv_smoke_lsp_reveal_contains "lsp reveal DB origin" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '@db.connect'
  orv_smoke_lsp_reveal_contains "lsp reveal DB preflight" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight"'
  orv_smoke_lsp_reveal_contains "lsp reveal DB smoke summary" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"smoke_test_summary"'
  orv_smoke_lsp_reveal_contains "lsp reveal DB smoke summary count" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight_smoke_summary_missing_count"'
fi
if [ -n "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" ]; then
  orv_smoke_reveal_contains "reveal payment source" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '@payment.connect'
  orv_smoke_reveal_contains "reveal payment match" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '"matched": true'
  orv_smoke_reveal_contains "reveal payment record path" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" 'file://data/payments.jsonl'
  orv_smoke_reveal_contains "reveal payment request kind" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" 'payment.capture'
  orv_smoke_editor_reveal_contains "editor reveal payment source" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '@payment.connect'
  orv_smoke_editor_reveal_contains "editor reveal payment match" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '"matched": true'
  orv_smoke_lsp_reveal_contains "lsp reveal payment origin" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '@payment.connect'
  orv_smoke_lsp_reveal_contains "lsp reveal payment match" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '"matched": true'
fi
if [ -n "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" ]; then
  orv_smoke_reveal_contains "reveal shipping source" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '@shipping.connect'
  orv_smoke_reveal_contains "reveal shipping match" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '"matched": true'
  orv_smoke_reveal_contains "reveal shipping record path" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" 'file://data/shipments.jsonl'
  orv_smoke_reveal_contains "reveal shipping request kind" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" 'shipping.booking'
  orv_smoke_editor_reveal_contains "editor reveal shipping source" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '@shipping.connect'
  orv_smoke_editor_reveal_contains "editor reveal shipping match" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '"matched": true'
  orv_smoke_lsp_reveal_contains "lsp reveal shipping origin" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '@shipping.connect'
  orv_smoke_lsp_reveal_contains "lsp reveal shipping match" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '"matched": true'
fi
CSRF_COOKIE="$(orv_smoke_cookie_from_headers orv_csrf "$SMOKE_HEADERS")"
if [ -z "$CSRF_COOKIE" ]; then
  printf 'orv deploy smoke test failed: missing orv_csrf cookie\n' >&2
  exit 1
fi
CSRF_TOKEN="${CSRF_COOKIE#orv_csrf=}"

orv_smoke_curl_origin "POST /products" "__PRODUCTS_ORIGIN__" -X POST "$BASE_URL/products" -H 'content-type: application/json' -H "cookie: ${CSRF_COOKIE}" -H "x-csrf-token: ${CSRF_TOKEN}" --data "{\"sku\":\"${SMOKE_SKU}\",\"name\":\"ORV Smoke Product\",\"badge\":\"${SMOKE_BADGE}\",\"price\":1000,\"stock\":5}"
orv_smoke_curl_origin "POST /products second" "__PRODUCTS_ORIGIN__" -X POST "$BASE_URL/products" -H 'content-type: application/json' -H "cookie: ${CSRF_COOKIE}" -H "x-csrf-token: ${CSRF_TOKEN}" --data "{\"sku\":\"${SMOKE_SKU_SECOND}\",\"name\":\"ORV Smoke Product 2\",\"badge\":\"${SMOKE_BADGE_SECOND}\",\"price\":1200,\"stock\":4}"
orv_smoke_curl_origin "POST /products third" "__PRODUCTS_ORIGIN__" -X POST "$BASE_URL/products" -H 'content-type: application/json' -H "cookie: ${CSRF_COOKIE}" -H "x-csrf-token: ${CSRF_TOKEN}" --data "{\"sku\":\"${SMOKE_SKU_THIRD}\",\"name\":\"ORV Smoke Product 3\",\"badge\":\"${SMOKE_BADGE_THIRD}\",\"price\":1300,\"stock\":3}"
orv_smoke_curl_origin "POST /members" "__MEMBERS_ORIGIN__" -X POST "$BASE_URL/members" -H 'content-type: application/json' -H "cookie: ${CSRF_COOKIE}" -H "x-csrf-token: ${CSRF_TOKEN}" --data "{\"handle\":\"${SMOKE_HANDLE}\",\"name\":\"ORV Smoke Member\",\"email\":\"${SMOKE_EMAIL}\",\"password\":\"${SMOKE_PASSWORD}\"}"
orv_smoke_curl_capture_origin "POST /members/login smoke" "$SMOKE_MEMBER_HEADERS" "__LOGIN_ORIGIN__" -X POST "$BASE_URL/members/login" -H 'content-type: application/json' -H "cookie: ${CSRF_COOKIE}" -H "x-csrf-token: ${CSRF_TOKEN}" --data "{\"handle\":\"${SMOKE_HANDLE}\",\"email\":\"${SMOKE_EMAIL}\",\"password\":\"${SMOKE_PASSWORD}\"}"
MEMBER_SESSION_COOKIE="$(orv_smoke_cookie_from_headers orv_session "$SMOKE_MEMBER_HEADERS")"
if [ -z "$MEMBER_SESSION_COOKIE" ]; then
  printf 'orv deploy smoke test failed: missing member session cookie\n' >&2
  exit 1
fi
orv_smoke_curl_origin "GET /account/sessions" "__ACCOUNT_ORIGIN__" -H "cookie: ${MEMBER_SESSION_COOKIE}" "$BASE_URL/account/sessions"
orv_smoke_fetch_origin "GET /account/sessions content" "$SMOKE_ACCOUNT_BODY" "__ACCOUNT_ORIGIN__" -H "cookie: ${MEMBER_SESSION_COOKIE}" "$BASE_URL/account/sessions"
orv_smoke_body_contains "account smoke session" "$SMOKE_ACCOUNT_BODY" "$SMOKE_HANDLE"
orv_smoke_curl_origin "POST /cart/items" "__CART_ITEMS_ORIGIN__" -X POST "$BASE_URL/cart/items" -H 'content-type: application/json' -H "cookie: ${CSRF_COOKIE}" -H "x-csrf-token: ${CSRF_TOKEN}" --data "{\"handle\":\"${SMOKE_HANDLE}\",\"sku\":\"${SMOKE_SKU}\",\"quantity\":1}"
orv_smoke_fetch_origin "GET /catalog content" "$SMOKE_CATALOG_BODY" "__CATALOG_ORIGIN__" "$BASE_URL/catalog"
orv_smoke_body_contains "catalog smoke product" "$SMOKE_CATALOG_BODY" "$SMOKE_SKU"
orv_smoke_body_contains "catalog second smoke product" "$SMOKE_CATALOG_BODY" "$SMOKE_SKU_SECOND"
orv_smoke_body_contains "catalog third smoke product" "$SMOKE_CATALOG_BODY" "$SMOKE_SKU_THIRD"
orv_smoke_body_contains "catalog smoke product field" "$SMOKE_CATALOG_BODY" "$SMOKE_BADGE"
orv_smoke_body_contains "catalog second smoke product field" "$SMOKE_CATALOG_BODY" "$SMOKE_BADGE_SECOND"
orv_smoke_body_contains "catalog third smoke product field" "$SMOKE_CATALOG_BODY" "$SMOKE_BADGE_THIRD"
orv_smoke_fetch_origin "GET /cart content" "$SMOKE_CART_BODY" "__CART_ORIGIN__" "$BASE_URL/cart"
orv_smoke_body_contains "cart smoke item" "$SMOKE_CART_BODY" "$SMOKE_SKU"
orv_smoke_fetch_origin "POST /checkout" "$SMOKE_CHECKOUT_BODY" "__CHECKOUT_ORIGIN__" -X POST "$BASE_URL/checkout" -H 'content-type: application/json' -H "cookie: ${CSRF_COOKIE}" -H "x-csrf-token: ${CSRF_TOKEN}" --data "{\"handle\":\"${SMOKE_HANDLE}\",\"sku\":\"${SMOKE_SKU}\",\"quantity\":1,\"total\":1000,\"method\":\"card\",\"carrier\":\"post\",\"address\":\"ORV smoke address\"}"
orv_smoke_body_contains "checkout shipped order" "$SMOKE_CHECKOUT_BODY" '"status":"shipped"'
orv_smoke_body_contains "checkout captured payment" "$SMOKE_CHECKOUT_BODY" '"status":"captured"'
orv_smoke_body_contains "checkout shipment tracking" "$SMOKE_CHECKOUT_BODY" 'TRK-LOCAL'
orv_smoke_curl_capture_origin "POST /members/login admin" "$SMOKE_ADMIN_HEADERS" "__LOGIN_ORIGIN__" -X POST "$BASE_URL/members/login" -H 'content-type: application/json' -H "cookie: ${CSRF_COOKIE}" -H "x-csrf-token: ${CSRF_TOKEN}" --data "{\"handle\":\"admin\",\"email\":\"admin@example.test\",\"password\":\"admin-reference-password\"}"
ADMIN_SESSION_COOKIE="$(orv_smoke_cookie_from_headers orv_session "$SMOKE_ADMIN_HEADERS")"
ADMIN_ROLE_COOKIE="$(orv_smoke_cookie_from_headers orv_session_role "$SMOKE_ADMIN_HEADERS")"
if [ -z "$ADMIN_SESSION_COOKIE" ] || [ -z "$ADMIN_ROLE_COOKIE" ]; then
  printf 'orv deploy smoke test failed: missing admin session cookies\n' >&2
  exit 1
fi
orv_smoke_fetch_origin "GET /admin dashboard content" "$SMOKE_ADMIN_BODY" "__ADMIN_ORIGIN__" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin"
orv_smoke_body_contains "admin dashboard title" "$SMOKE_ADMIN_BODY" 'Miol Shop Admin'
orv_smoke_body_contains "admin dashboard summary link" "$SMOKE_ADMIN_BODY" '/admin/summary'
orv_smoke_body_contains "admin dashboard webhook link" "$SMOKE_ADMIN_BODY" '/admin/webhooks'
orv_smoke_body_contains "admin dashboard audit link" "$SMOKE_ADMIN_BODY" '/admin/audit'
orv_smoke_body_contains "admin dashboard sqlite storage" "$SMOKE_ADMIN_BODY" 'data/shop.sqlite'
orv_smoke_body_contains "admin dashboard payment storage" "$SMOKE_ADMIN_BODY" 'data/payments.jsonl'
orv_smoke_body_contains "admin dashboard shipment storage" "$SMOKE_ADMIN_BODY" 'data/shipments.jsonl'
orv_smoke_fetch_origin "GET /admin/summary content" "$SMOKE_ADMIN_SUMMARY_BODY" "__ADMIN_SUMMARY_ORIGIN__" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin/summary"
orv_smoke_body_contains "admin summary orders" "$SMOKE_ADMIN_SUMMARY_BODY" '"orders"'
orv_smoke_body_contains "admin summary payments" "$SMOKE_ADMIN_SUMMARY_BODY" '"payments"'
orv_smoke_body_contains "admin summary webhook events" "$SMOKE_ADMIN_SUMMARY_BODY" '"webhookEvents"'
orv_smoke_body_contains "admin summary audit events" "$SMOKE_ADMIN_SUMMARY_BODY" '"auditEvents"'
orv_smoke_fetch_origin "GET /admin/catalog content" "$SMOKE_ADMIN_CATALOG_BODY" "__ADMIN_CATALOG_ORIGIN__" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin/catalog"
orv_smoke_body_contains "admin catalog smoke product" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_SKU"
orv_smoke_body_contains "admin catalog second smoke product" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_SKU_SECOND"
orv_smoke_body_contains "admin catalog third smoke product" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_SKU_THIRD"
orv_smoke_body_contains "admin catalog smoke product field" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_BADGE"
orv_smoke_body_contains "admin catalog second smoke product field" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_BADGE_SECOND"
orv_smoke_body_contains "admin catalog third smoke product field" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_BADGE_THIRD"
orv_smoke_fetch_origin "GET /admin/orders content" "$SMOKE_ADMIN_ORDERS_BODY" "__ADMIN_ORDERS_ORIGIN__" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin/orders"
orv_smoke_body_contains "admin orders smoke member" "$SMOKE_ADMIN_ORDERS_BODY" "$SMOKE_HANDLE"
orv_smoke_body_contains "admin orders shipped" "$SMOKE_ADMIN_ORDERS_BODY" 'shipped'
orv_smoke_fetch_origin "GET /admin/payments content" "$SMOKE_ADMIN_PAYMENTS_BODY" "__ADMIN_PAYMENTS_ORIGIN__" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin/payments"
orv_smoke_body_contains "admin payments captured" "$SMOKE_ADMIN_PAYMENTS_BODY" 'captured'
orv_smoke_fetch_origin "GET /admin/shipments content" "$SMOKE_ADMIN_SHIPMENTS_BODY" "__ADMIN_SHIPMENTS_ORIGIN__" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin/shipments"
orv_smoke_body_contains "admin shipments tracking" "$SMOKE_ADMIN_SHIPMENTS_BODY" 'TRK-LOCAL'
orv_smoke_fetch_origin "GET /admin/webhooks content" "$SMOKE_ADMIN_WEBHOOKS_BODY" "__ADMIN_WEBHOOKS_ORIGIN__" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin/webhooks"
orv_smoke_body_contains "admin webhooks title" "$SMOKE_ADMIN_WEBHOOKS_BODY" 'Webhooks'
orv_smoke_fetch_origin "GET /admin/audit content" "$SMOKE_ADMIN_AUDIT_BODY" "__ADMIN_AUDIT_ORIGIN__" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin/audit"
orv_smoke_body_contains "admin audit checkout" "$SMOKE_ADMIN_AUDIT_BODY" 'checkout.complete'
orv_smoke_body_contains "admin audit payment" "$SMOKE_ADMIN_AUDIT_BODY" 'payment.capture'
orv_smoke_body_contains "admin audit shipment" "$SMOKE_ADMIN_AUDIT_BODY" 'shipment.book'
"#
        .replace("__ROOT_ORIGIN__", &root_origin)
        .replace("__DB_CONNECT_ORIGIN__", &db_connect_origin)
        .replace("__PAYMENT_CONNECT_ORIGIN__", &payment_connect_origin)
        .replace("__SHIPPING_CONNECT_ORIGIN__", &shipping_connect_origin)
        .replace("__PRODUCTS_ORIGIN__", &products_origin)
        .replace("__MEMBERS_ORIGIN__", &members_origin)
        .replace("__LOGIN_ORIGIN__", &login_origin)
        .replace("__ACCOUNT_ORIGIN__", &account_origin)
        .replace("__CART_ITEMS_ORIGIN__", &cart_items_origin)
        .replace("__CATALOG_ORIGIN__", &catalog_origin)
        .replace("__CART_ORIGIN__", &cart_origin)
        .replace("__CHECKOUT_ORIGIN__", &checkout_origin)
        .replace("__ADMIN_ORIGIN__", &admin_origin)
        .replace("__ADMIN_SUMMARY_ORIGIN__", &admin_summary_origin)
        .replace("__ADMIN_CATALOG_ORIGIN__", &admin_catalog_origin)
        .replace("__ADMIN_ORDERS_ORIGIN__", &admin_orders_origin)
        .replace("__ADMIN_PAYMENTS_ORIGIN__", &admin_payments_origin)
        .replace("__ADMIN_SHIPMENTS_ORIGIN__", &admin_shipments_origin)
        .replace("__ADMIN_WEBHOOKS_ORIGIN__", &admin_webhooks_origin)
        .replace("__ADMIN_AUDIT_ORIGIN__", &admin_audit_origin);
        script.push_str(&shop_smoke);
        for route in server_artifact.routes.iter().filter(|route| {
            route.method == "GET" && !route.path.contains(':') && route.path.starts_with("/admin")
        }) {
            let origin_ref = deploy_smoke_origin_var_ref(&route.method, &route.path);
            let _ = writeln!(
                script,
                r#"orv_smoke_curl_origin "GET {}" "{}" -H "cookie: ${{ADMIN_SESSION_COOKIE}}; ${{ADMIN_ROLE_COOKIE}}" "$BASE_URL{}""#,
                route.path, origin_ref, route.path
            );
        }
    }
    script.push_str("orv_smoke_trace_stream\n");
    script.push_str("orv_smoke_write_output\nprintf 'orv deploy smoke test passed\\n'\n");
    let target = out.join(path);
    write_text(&target, &script)?;
    set_executable_if_supported(&target)
}

pub(crate) fn deploy_smoke_output_function_section(
    route_count: usize,
    client: &serde_json::Value,
) -> String {
    let mut out = format!(
        r#"orv_smoke_write_output() {{
  {{
    printf 'orv deploy smoke test passed\n'
    printf 'build_dir=%s\n' "$ORV_SMOKE_BUILD_DIR"
    printf 'base_url=%s\n' "$BASE_URL"
    printf 'graph_contract=verified\n'
    printf 'dap_summary=verified\n'
    printf 'dap_source_bundle=verified\n'
    printf 'server_routes={route_count}\n'
    printf 'trace_stream_requested=%s\n' "${{ORV_SMOKE_TRACE_STREAM:-0}}"
"#,
    );
    if !client.is_null() {
        let manifest = json_str_or_empty(client, "manifest");
        let reactive_plan = json_str_or_empty(client, "reactive_plan");
        let page = json_str_or_empty(client, "page");
        let loader = json_str_or_empty(client, "loader");
        let wasm = json_str_or_empty(client, "wasm");
        for line in [
            format!("    printf 'client_manifest={manifest}\\n'\n"),
            format!("    printf 'client_reactive_plan={reactive_plan}\\n'\n"),
            format!("    printf 'client_page={page}\\n'\n"),
            format!("    printf 'client_loader={loader}\\n'\n"),
            format!("    printf 'client_wasm={wasm}\\n'\n"),
        ] {
            out.push_str(&line);
        }
    }
    out.push_str(
        r#"  } > "$ORV_SMOKE_OUTPUT"
}

"#,
    );
    out
}

pub(crate) fn deploy_smoke_reveal_marker_contract_section(origin_ref: &str) -> String {
    format!(
        r#"orv_smoke_reveal_contains "reveal smoke required markers" "{origin_ref}" '"smoke_test_required_markers": ['
orv_smoke_reveal_contains "reveal smoke summary required markers" "{origin_ref}" '"required_markers": ['
orv_smoke_reveal_contains "reveal smoke marker dap source bundle" "{origin_ref}" '"dap_source_bundle"'
orv_smoke_editor_reveal_contains "editor reveal smoke required markers" "{origin_ref}" '"smoke_test_required_markers": ['
orv_smoke_editor_reveal_contains "editor reveal smoke summary required markers" "{origin_ref}" '"required_markers": ['
orv_smoke_editor_reveal_contains "editor reveal smoke marker dap source bundle" "{origin_ref}" '"dap_source_bundle"'
orv_smoke_lsp_reveal_contains "lsp reveal smoke required markers" "{origin_ref}" '"smoke_test_required_markers": ['
orv_smoke_lsp_reveal_contains "lsp reveal smoke summary required markers" "{origin_ref}" '"required_markers": ['
orv_smoke_lsp_reveal_contains "lsp reveal smoke marker dap source bundle" "{origin_ref}" '"dap_source_bundle"'

"#
    )
}

pub(crate) fn deploy_smoke_client_contract_section(client: &serde_json::Value) -> String {
    if client.is_null() {
        return String::new();
    }
    let manifest = json_str_or_empty(client, "manifest");
    let reactive_plan = json_str_or_empty(client, "reactive_plan");
    let page = json_str_or_empty(client, "page");
    let loader = json_str_or_empty(client, "loader");
    let wasm = json_str_or_empty(client, "wasm");
    format!(
        r#"orv_smoke_file "{manifest}"
orv_smoke_file "{reactive_plan}"
orv_smoke_file "{page}"
orv_smoke_file "{loader}"
orv_smoke_file "{wasm}"
orv_smoke_grep "client page marker" "{page}" 'data-orv-client="wasm"'
orv_smoke_grep "client loader reference" "{page}" 'app.js'
orv_smoke_grep "client manifest reactive plan path" "{manifest}" '"reactive_plan": "{reactive_plan}"'
orv_smoke_grep "client manifest reactive plan hash" "{manifest}" '"reactive_plan_hash"'
orv_smoke_grep "client manifest loader hash" "{manifest}" '"loader_hash"'
orv_smoke_grep "client manifest wasm hash" "{manifest}" '"wasm_hash"'
orv_smoke_grep "client manifest source bundle" "{manifest}" '"source_bundle": "source-bundle.json"'
orv_smoke_grep "client manifest runtime" "{manifest}" '"runtime": "client_wasm"'
orv_smoke_grep "client manifest capabilities" "{manifest}" '"capabilities"'
orv_smoke_grep "client manifest capability surfaces" "{manifest}" '"surfaces"'
orv_smoke_grep "client manifest event actions" "{manifest}" '"event_actions"'
orv_smoke_grep "client reactive plan kind" "{reactive_plan}" '"kind": "orv.client.reactive_plan"'
orv_smoke_grep "client reactive plan source bundle" "{reactive_plan}" '"source_bundle": "source-bundle.json"'
orv_smoke_grep "client reactive plan blocked_by" "{reactive_plan}" '"blocked_by"'
orv_smoke_grep "client loader bootstrap" "{loader}" 'ORV_CLIENT_BOOTSTRAP'
orv_smoke_grep "client loader embedded reactive plan" "{loader}" 'embeddedReactivePlan'
orv_smoke_grep "client loader embedded reactive plan hash" "{loader}" 'embeddedReactivePlanHash'
orv_smoke_grep "client loader source bundle hash" "{loader}" 'sourceBundleHash'
orv_smoke_grep "client loader wasm reference" "{loader}" 'app.wasm'
orv_smoke_grep "client loader signal setter" "{loader}" '__ORV_SET_SIGNAL__'

"#
    )
}

pub(crate) fn deploy_smoke_client_reveal_section(client: &serde_json::Value) -> String {
    if client.is_null() {
        return String::new();
    }
    let manifest = json_str_or_empty(client, "manifest");
    format!(
        r#"orv_smoke_reveal_contains "reveal client target summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_target_count": 5'
orv_smoke_reveal_contains "reveal client manifest summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_manifest_count": 1'
orv_smoke_reveal_contains "reveal client capability summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_capability_surface_count"'
orv_smoke_reveal_contains "reveal client manifest target" "$ORV_SMOKE_CLIENT_ORIGIN" '"path": "{manifest}"'
orv_smoke_editor_reveal_contains "editor reveal client target summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_target_count": 5'
orv_smoke_editor_reveal_contains "editor reveal client manifest summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_manifest_count": 1'
orv_smoke_editor_reveal_contains "editor reveal client capability summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_capability_surface_count"'
orv_smoke_lsp_reveal_contains "lsp reveal client target summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_target_count": 5'
orv_smoke_lsp_reveal_contains "lsp reveal client manifest summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_manifest_count": 1'
orv_smoke_lsp_reveal_contains "lsp reveal client capability summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_capability_surface_count"'
orv_smoke_dap_summary_contains "dap client target summary" '"client_target_count": 5'
orv_smoke_dap_summary_contains "dap client manifest summary" '"client_manifest_count": 1'
orv_smoke_dap_summary_contains "dap client capability summary" '"client_capability_surface_count"'

"#
    )
}

pub(crate) fn deploy_smoke_db_adapter_contract_section(persistence: &DeployPersistence) -> String {
    if persistence.db_adapters.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        r#"orv_smoke_file "deploy/db-adapters.json"
orv_smoke_grep "db adapter artifact kind" "deploy/db-adapters.json" '"orv.deploy.db_adapters"'
orv_smoke_grep "db adapter bridge contract" "deploy/db-adapters.json" '"contract": "http-json-v1"'
orv_smoke_grep "db adapter bridge retry" "deploy/db-adapters.json" '"retry"'
"#,
    );
    for adapter in &persistence.db_adapters {
        let Some(endpoint_env) = adapter
            .bridge_env
            .iter()
            .find(|env| env.purpose == "bridge_endpoint")
        else {
            continue;
        };
        let Some(endpoint) = &adapter.endpoint else {
            continue;
        };
        let auth_env = adapter
            .bridge_env
            .iter()
            .find(|env| env.purpose == "bridge_auth_token")
            .map(|env| env.env.as_str())
            .unwrap_or("");
        let endpoint_expr = format!("${{{}:-${{ORV_DB_ADAPTER_ENDPOINT:-}}}}", endpoint_env.env);
        let auth_expr = format!("${{{auth_env}:-${{ORV_DB_ADAPTER_AUTH_TOKEN:-}}}}");
        let _ = writeln!(
            out,
            r#"orv_smoke_db_bridge_schema "{} bridge" "{}" "{}" "{}" "{}""#,
            adapter.provider, endpoint_expr, adapter.provider, endpoint, auth_expr
        );
    }
    out.push('\n');
    out
}

pub(crate) fn write_prod_deploy_runbook(
    out: &Path,
    artifacts: &DeployRunbookArtifacts<'_>,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
    persistence: &DeployPersistence,
    client: &serde_json::Value,
) -> anyhow::Result<()> {
    let compose_path = artifacts.compose;
    let env_example_path = artifacts.env_example;
    let db_adapters_path = artifacts.db_adapters;
    let commerce_adapters_path = artifacts.commerce_adapters;
    let smoke_test_path = artifacts.smoke_test;
    let smoke_output_path = artifacts.smoke_output;
    let preflight_path = artifacts.preflight;
    let benchmark_evidence_path = artifacts.benchmark_evidence;
    let routes_artifact = artifacts.routes;
    let port_prefix = deploy_runbook_port_assignment(server_artifact.listen.as_ref())
        .map(|port| format!("{port} "))
        .unwrap_or_default();
    let trace_events_url = deploy_runbook_trace_events_url(server_artifact.listen.as_ref());
    let routes = server_artifact
        .routes
        .iter()
        .map(|route| format!("- {} {}\n", route.method, route.path))
        .collect::<String>();
    let persistence_section = deploy_runbook_persistence_section(persistence);
    let client_section = deploy_runbook_client_section(client);
    let smoke_required_markers = deploy_benchmark::SMOKE_REQUIRED_MARKERS
        .iter()
        .map(|marker| format!("- `{marker}`\n"))
        .collect::<String>();
    let runbook = format!(
        r#"# orv deploy

## Run

```sh
{port_prefix}docker compose -f {compose_path} up --build -d
```

## Artifacts

- Compose: {compose_path}
- Env example: {env_example_path}
- DB adapters: {db_adapters_path}
- Commerce adapters: {commerce_adapters_path}
- Smoke test: {smoke_test_path}
- Smoke output: {smoke_output_path}
- Preflight: {preflight_path}
- Benchmark evidence: {benchmark_evidence_path}
- Routes: {routes_artifact}

## Native Launcher

```sh
cargo build --manifest-path server/native/Cargo.toml --release
ORV_BUILD_DIR=. ./server/native/target/release/orv-native-server
```

The generated launcher path can infer the build directory; ORV_BUILD_DIR is an explicit override.

## Native Runtime Image

```sh
docker build -f server/native/Dockerfile -t orv-native-server:latest .
```

## Request Trace

```sh
./deploy/server.sh --trace deploy/request-trace.json
curl -N {trace_events_url}
orv editor trace . --trace deploy/request-trace.json
ORV_SMOKE_TRACE_STREAM=1 ./{smoke_test_path}
```

## Deploy Preflight

```sh
orv verify-build .
orv deploy-env-check .
orv editor run-debug . --control next
orv benchmark-report .
```

## Smoke Test

```sh
./{smoke_test_path}
```

## Benchmark Evidence

Record human-run timing and observation data in `{benchmark_evidence_path}` after the preflight and smoke commands pass. The file keeps the 5-hour shop benchmark tasks, data-to-record fields, and preflight hash together so benchmark reports stay tied to the checked build contract.
The generated smoke test writes `{smoke_output_path}` on success, and `orv benchmark-report .` uses it when the evidence `smoke_test_output` field is still empty.

## Smoke Output Markers

The benchmark report requires these markers in `{smoke_output_path}`:

{smoke_required_markers}

```sh
orv benchmark-report . --require-pass
```

{client_section}
{persistence_section}
## Routes

{routes}"#
    );
    write_text(&out.join("deploy").join("README.md"), &runbook)
}

pub(crate) fn deploy_runbook_client_section(client: &serde_json::Value) -> String {
    if client.is_null() {
        return String::new();
    }
    let manifest = json_str_or_empty(client, "manifest");
    let reactive_plan = json_str_or_empty(client, "reactive_plan");
    let page = json_str_or_empty(client, "page");
    let loader = json_str_or_empty(client, "loader");
    let wasm = json_str_or_empty(client, "wasm");
    let runtime = client
        .pointer("/capabilities/runtime")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("client_wasm");
    let surfaces = client
        .pointer("/capabilities/surfaces")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| "none".to_string());
    let mut blockers = String::new();
    for blocker in client
        .get("blockers")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = json_str_or_empty(blocker, "id");
        let artifact = json_str_or_empty(blocker, "artifact");
        let reason = json_str_or_empty(blocker, "reason");
        let _ = writeln!(blockers, "- Client blocker: {id} {artifact} {reason}");
    }
    format!(
        r#"## Client Bundle

- Client manifest: {manifest}
- Client reactive plan: {reactive_plan}
- Client page: {page}
- Client loader: {loader}
- Client WASM: {wasm}
- Client runtime: {runtime}
- Client capability surfaces: {surfaces}
{blockers}
"#
    )
}

pub(crate) fn write_prod_server_entrypoint(
    out: &Path,
    server_artifact_path: &str,
) -> anyhow::Result<()> {
    let script = format!(
        r#"#!/usr/bin/env sh
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BUILD_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
exec orv run-artifact "$BUILD_DIR/{server_artifact_path}" "$@"
"#
    );
    let path = out.join("deploy").join("server.sh");
    write_text(&path, &script)?;
    set_executable_if_supported(&path)
}

#[cfg(unix)]
pub(crate) fn set_executable_if_supported(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .map_err(|e| anyhow::anyhow!("failed to stat {}: {e}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
        .map_err(|e| anyhow::anyhow!("failed to chmod {}: {e}", path.display()))
}

#[cfg(not(unix))]
pub(crate) fn set_executable_if_supported(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn verify_executable_if_supported(path: &Path, label: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::metadata(path)
        .map_err(|e| anyhow::anyhow!("failed to stat {}: {e}", path.display()))?
        .permissions();
    if permissions.mode() & 0o111 == 0 {
        anyhow::bail!("{label} must be executable: {}", path.display());
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn verify_executable_if_supported(_path: &Path, _label: &str) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn verify_shell_syntax_if_supported(path: &Path, label: &str) -> anyhow::Result<()> {
    let output = ProcessCommand::new("sh")
        .arg("-n")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run shell syntax check for {label}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{label} shell syntax invalid: {}", stderr.trim());
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn verify_shell_syntax_if_supported(_path: &Path, _label: &str) -> anyhow::Result<()> {
    Ok(())
}
