//! orv CLI 프론트엔드 — `orv` 바이너리.
//!
//! MVP: `orv run <file>`로 `.orv` 파일을 tree-walking 인터프리터로 실행한다.
//! source-entry 명령은 `orv.toml` 의 `[project].entry`와 프로젝트 디렉터리
//! 입력도 허용한다. `orv init <dir>`은 최소 프로젝트 scaffold 를 만든다.
//! `orv origins <file>`은 HIR 기반 origin map JSON을 출력한다. `orv graph
//! <file>`은 AST 기반 `ProjectGraph` v1과 HIR origin map JSON을 출력하고,
//! `orv build <file-or-orv.toml> --out <dir>`은 초기 build artifact directory 를 생성한다.
//! `--prod`는 같은 artifact에 deploy manifest, route inventory, reference container
//! contract, reference server entrypoint를 추가한다.
//! `orv lock [dir-or-orv.toml]`은 프로젝트 의존성 metadata를 `orv.lock`으로 고정한다.
//! `orv fetch [dir-or-orv.toml]`는 lockfile dependency source-bundle cache 를 생성한다.
//! `orv add/remove`은 `orv.toml` dependency section 과 lockfile 을 함께 갱신한다.
//! `orv verify-build <dir>`은 build manifest/plan target 을 검증한다.
//! `orv deploy-env-check <dir>`은 production deploy credential env 를 검증한다.
//! `orv benchmark-report <dir>`은 deploy benchmark evidence 를 JSON report 로 요약한다.
//! `orv verify-artifact <file>`은 server runtime artifact 를 검증하고,
//! `orv check-artifact <file>`은 source bundle 을 재분석하며,
//! `orv check-build <dir>`은 build source bundle 을 재분석하며,
//! `orv run-artifact <file>`은 source bundle 을 재수화해 reference runtime 으로 실행한다.
//! `orv run-build <dir>`은 `server/launch.json` 의 reference runner 계약을 실행한다.
//! `orv reveal <dir> <origin-id>`는 build artifact 에서 origin id 를 원본
//! `.orv` span 과 production descriptor 로 되짚는다.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::format_collect,
    clippy::literal_string_with_formatting_args,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value,
    clippy::needless_raw_string_hashes,
    clippy::option_if_let_else,
    clippy::redundant_clone,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines,
    clippy::unnecessary_wraps,
    clippy::unreadable_literal,
    clippy::unused_self,
    clippy::wildcard_imports
)]

use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, ExitCode, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::JoinHandle;
use std::time::Duration;

use clap::Parser;
use codespan_reporting::files::SimpleFiles;
use codespan_reporting::term::termcolor::WriteColor;
use orv_diagnostics::{ByteRange, FileId, Span};
use orv_project::{ProjectEdgeKind, ProjectGraph, ProjectNodeId, ProjectNodeKind, SourceFile};
use orv_syntax::ast::{
    Block, ConstraintValue, Expr, ExprKind, FunctionBody, FunctionStmt, Program, Stmt,
    TypeConstraint, TypeRef, TypeRefKind,
};

const EDITOR_DEBUG_SESSION_RUNNER_PATH: &str = "debug/session-runner.json";
const EDITOR_DEBUG_SESSION_RESULT_PATH: &str = "debug/session-result.json";
const EDITOR_DEBUG_SESSION_RESULT_HTML_PATH: &str = "debug/session-result.html";
const EDITOR_RUNTIME_PANEL_HTML_PATH: &str = "runtime/panel.html";
const EDITOR_PRODUCTION_PANEL_HTML_PATH: &str = "production/panel.html";
const EDITOR_NATIVE_HOST_MANIFEST_PATH: &str = "native-host.json";
const EDITOR_TRACE_STREAM_EVENTS_PATH: &str = "trace/events.sse";
const EDITOR_TRACE_PANEL_HTML_PATH: &str = "trace/panel.html";

mod deploy_benchmark;
pub(crate) mod editor_lsp_dap;
mod graph_view;
mod init;
mod orv_test;

use editor_lsp_dap::*;
use graph_view::{project_graph_view_svg, write_project_graph_view};
use init::cmd_init;
use orv_test::{cmd_test, orv_test_source_bundle};
#[cfg(test)]
use orv_test::{orv_test_list_json, orv_test_summary};

mod args;
pub(crate) mod build_deploy;
use args::{
    Cli, Command, DapCommand, DbCommand, EditorCommand, EditorDebugBreakpoint, EditorDebugControl,
    EditorDebugDataBreakpointInfoRequest, EditorDebugDataBreakpointSetRequest, InitTemplate,
    LspCommand, WorkspaceCommand,
};
use build_deploy::*;

mod db;
#[cfg(test)]
use db::{
    cmd_db_apply, cmd_db_migrate, cmd_db_rollback, db_archive_manifest_wal_path, db_plan_json,
};
use db::{
    cmd_db_apply_with_history, cmd_db_archive, cmd_db_backup, cmd_db_crash_matrix,
    cmd_db_migrate_with_data, cmd_db_plan, cmd_db_recover_from_inputs, cmd_db_restore_from_inputs,
    cmd_db_rollback_with_data, cmd_db_squash, cmd_db_verify, sha256_hex,
};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Run { file } => match cmd_run(&file) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Check { file } => match cmd_check(&file) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Dump { file } => match cmd_dump(&file) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Origins { file } => match cmd_origins(&file) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Graph { file, view, out } => match cmd_graph(&file, view, &out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Build { file, out, prod } => {
            match cmd_build_with_profile(&file, &out, BuildProfile::from_prod_flag(prod)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::VerifyBuild { dir } => match cmd_verify_build(&dir) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::DeployEnvCheck { dir } => match cmd_deploy_env_check(&dir) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::BenchmarkReport { dir, require_pass } => {
            match cmd_benchmark_report(&dir, require_pass) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::VerifyArtifact { file } => match cmd_verify_artifact(&file) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::CheckArtifact { file } => match cmd_check_artifact(&file) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::CheckBuild { dir } => match cmd_check_build(&dir) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Lock { dir, check } => match cmd_lock(&dir, check) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Fetch { dir, out } => match cmd_fetch(&dir, &out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Add {
            pkg,
            version,
            manifest,
            dev,
            path,
            registry,
        } => match cmd_add_dependency(
            &manifest,
            &pkg,
            version.as_deref(),
            dev,
            path.as_deref(),
            registry.as_deref(),
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Remove { pkg, manifest, dev } => {
            match cmd_remove_dependency(&manifest, &pkg, dev) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::RunArtifact { file, trace } => match cmd_run_artifact(&file, trace.as_deref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::RunBuild { dir, trace } => match cmd_run_build(&dir, trace.as_deref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Dev {
            file,
            out,
            hmr,
            watch,
            watch_loop,
            serve,
            serve_port,
            watch_iterations,
            watch_interval_ms,
        } => match cmd_dev(
            &file,
            &out,
            DevOptions {
                hmr,
                watch,
                loop_mode: if watch_loop {
                    DevLoopMode::WatchLoop {
                        iterations: watch_iterations,
                        interval_ms: watch_interval_ms,
                    }
                } else {
                    DevLoopMode::Once
                },
                serve: serve.then_some(DevServeOptions {
                    port: serve_port,
                    iterations: watch_iterations,
                    interval_ms: watch_interval_ms,
                }),
            },
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Reveal { dir, origin_id } => match cmd_reveal(&dir, &origin_id) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Init {
            dir,
            name,
            template,
        } => match cmd_init(&dir, name.as_deref(), template) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Workspace { command } => match command {
            WorkspaceCommand::New {
                member,
                root,
                name,
                template,
            } => match cmd_workspace_new(&root, &member, name.as_deref(), template) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            WorkspaceCommand::Graph { root, view, out } => {
                match cmd_workspace_graph(&root, out.as_deref(), view) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            WorkspaceCommand::Lock { root, out } => match cmd_workspace_lock(&root, &out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            WorkspaceCommand::Fetch { root, out } => match cmd_workspace_fetch(&root, &out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            WorkspaceCommand::Build {
                root,
                out,
                prod,
                incremental,
            } => match cmd_workspace_build(
                &root,
                &out,
                BuildProfile::from_prod_flag(prod),
                incremental,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
        },
        Command::Test { path, filter, list } => match cmd_test(&path, filter.as_deref(), list) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Db { command } => match command {
            DbCommand::Plan { file, applied } => match cmd_db_plan(&file, applied.as_deref()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            DbCommand::Verify { file, schema } => match cmd_db_verify(&file, &schema) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            DbCommand::Apply {
                file,
                schema,
                history,
            } => match cmd_db_apply_with_history(&file, &schema, history.as_deref()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            DbCommand::Migrate {
                file,
                schema,
                history,
                data,
            } => {
                match cmd_db_migrate_with_data(&file, &schema, history.as_deref(), data.as_deref())
                {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            DbCommand::Rollback { schema, data } => {
                match cmd_db_rollback_with_data(&schema, data.as_deref()) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            DbCommand::Backup { data, out } => match cmd_db_backup(&data, &out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            DbCommand::Restore {
                backup,
                wal,
                archive,
                data,
                at,
            } => match cmd_db_restore_from_inputs(
                backup.as_deref(),
                wal.as_deref(),
                archive.as_deref(),
                at.as_deref(),
                &data,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            DbCommand::Recover {
                wal,
                archive,
                out,
                until_record,
                until_unix_ms,
                until_time,
            } => match cmd_db_recover_from_inputs(
                wal.as_deref(),
                archive.as_deref(),
                &out,
                until_record,
                until_unix_ms,
                until_time.as_deref(),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            DbCommand::Archive { wal, out, target } => {
                match cmd_db_archive(&wal, &out, target.as_deref()) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            DbCommand::CrashMatrix { out } => match cmd_db_crash_matrix(&out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            DbCommand::Squash { history, out } => match cmd_db_squash(&history, &out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
        },
        Command::Editor { command } => match command {
            EditorCommand::Snapshot { file } => match cmd_editor_snapshot(&file) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            EditorCommand::Reveal { dir, origin_id } => match cmd_editor_reveal(&dir, &origin_id) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            EditorCommand::Runtime { file } => match cmd_editor_runtime(&file) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            EditorCommand::Debug {
                file,
                breakpoints,
                function_breakpoints,
                data_breakpoints,
                exception_filters,
                controls,
                watch_expressions,
            } => match cmd_editor_debug(
                &file,
                &controls,
                &breakpoints,
                &function_breakpoints,
                &data_breakpoints,
                &exception_filters,
                &watch_expressions,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            EditorCommand::RunDebug {
                state,
                breakpoints,
                function_breakpoints,
                data_breakpoints,
                exception_filters,
                controls,
                watch_expressions,
            } => match cmd_editor_run_debug(
                &state,
                &controls,
                &breakpoints,
                &function_breakpoints,
                &data_breakpoints,
                &exception_filters,
                &watch_expressions,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            EditorCommand::Export {
                file,
                out,
                build,
                trace,
            } => match if build.is_none() && trace.is_none() {
                cmd_editor_export(&file, &out)
            } else {
                cmd_editor_export_with_options(&file, &out, build.as_deref(), trace.as_deref())
            } {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            EditorCommand::Trace { dir, trace } => match cmd_editor_trace(&dir, &trace) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            EditorCommand::TraceStream { dir, events } => {
                match cmd_editor_trace_stream(&dir, &events) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
        },
        Command::Lsp { command } => match command {
            LspCommand::Snapshot { file } => match cmd_lsp_snapshot(&file) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            LspCommand::Reveal { dir, origin_id } => match cmd_lsp_reveal(&dir, &origin_id) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            LspCommand::Serve { stdio } => match cmd_lsp_serve(stdio) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
        },
        Command::Dap { command } => match command {
            DapCommand::Serve { stdio } => match cmd_dap_serve(stdio) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
        },
    }
}

fn cmd_run(path: &Path) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let lowered = load_checked_hir(&entry)?;
    orv_runtime::run(&lowered.program).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn cmd_check(path: &Path) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let _lowered = load_checked_hir(&entry)?;
    println!("check: {} passed", entry.display());
    Ok(())
}

fn cmd_origins(path: &Path) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let lowered = load_checked_hir(&entry)?;
    let origins = orv_compiler::origin_map(&lowered.program);
    println!("{}", serde_json::to_string_pretty(&origins)?);
    Ok(())
}

fn cmd_graph(path: &Path, view: bool, out: &Path) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let value = project_graph_json_for_path(&entry)?;
    if view {
        write_project_graph_view(out, &value)?;
        println!("graph view: {}", out.join("index.html").display());
        return Ok(());
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn cmd_reveal(dir: &Path, origin_id: &str) -> anyhow::Result<()> {
    let value = reveal_origin_json(dir, origin_id)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn cmd_workspace_new(
    root: &Path,
    member: &Path,
    name: Option<&str>,
    template: InitTemplate,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(root)
        .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", root.display()))?;
    let root_manifest = root.join("orv.toml");
    let mut manifest = if root_manifest.is_file() {
        read_toml_manifest(&root_manifest)?
    } else {
        toml::Value::Table(toml::map::Map::new())
    };
    let member_path = workspace_member_string(member)?;
    add_workspace_member_to_manifest(&mut manifest, member)?;

    let project_name = name.map_or_else(|| workspace_member_project_name(member), str::to_string);
    cmd_init(&root.join(&member_path), Some(&project_name), template)?;
    write_toml_manifest_atomic(&root_manifest, &manifest)?;
    println!("workspace: added {member_path}");
    Ok(())
}

fn cmd_workspace_graph(root: &Path, out: Option<&Path>, view: bool) -> anyhow::Result<()> {
    let graph = workspace_graph_json(root)?;
    if view {
        let default_out = PathBuf::from("target/orv-workspace-graph-view");
        let out = out.unwrap_or(&default_out);
        write_workspace_graph_view(out, &graph)?;
        println!("workspace graph view: {}", out.join("index.html").display());
        return Ok(());
    }
    if let Some(out) = out {
        let path = out.join("workspace-graph.json");
        write_json(&path, &graph)?;
        println!("workspace graph: wrote {}", path.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&graph)?);
    }
    Ok(())
}

fn write_workspace_graph_view(out: &Path, graph: &serde_json::Value) -> anyhow::Result<()> {
    std::fs::create_dir_all(out)
        .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", out.display()))?;
    write_json(&out.join("workspace-graph.json"), graph)?;
    write_text(&out.join("index.html"), &workspace_graph_view_html(graph))
}

fn workspace_graph_view_html(graph: &serde_json::Value) -> String {
    let stats = graph.get("stats").unwrap_or(&serde_json::Value::Null);
    let member_count = json_usize_field(stats, "member_count");
    let edge_count = json_usize_field(stats, "edge_count");
    let members = graph
        .get("members")
        .and_then(serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let edges = graph
        .get("edges")
        .and_then(serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut html = workspace_graph_view_head(member_count, edge_count);
    html.push_str(&workspace_graph_view_svg(members, edges));
    html.push_str("</section>");
    html.push_str("<section class=\"filters\"><label>Search<input id=\"workspace-search\" type=\"search\" autocomplete=\"off\"></label></section>");
    html.push_str(&workspace_graph_member_rows(members));
    html.push_str(&workspace_graph_edge_rows(edges));
    html.push_str("<script>function filterWorkspaceGraphRows(){const query=(document.getElementById('workspace-search')?.value||'').toLowerCase();for(const row of document.querySelectorAll('[data-workspace-member-row],[data-workspace-edge-row]')){row.hidden=!!query&&!row.textContent.toLowerCase().includes(query);}}document.getElementById('workspace-search')?.addEventListener('input',filterWorkspaceGraphRows);filterWorkspaceGraphRows();</script></main></body></html>");
    html
}

fn workspace_graph_view_head(member_count: usize, edge_count: usize) -> String {
    let mut html = String::new();
    html.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    html.push_str("<title>ORV Workspace Graph</title><style>");
    html.push_str("body{margin:0;background:#f7f7f3;color:#242424;font:14px/1.45 -apple-system,BlinkMacSystemFont,Segoe UI,sans-serif}main{max-width:1160px;margin:0 auto;padding:24px}header{display:flex;justify-content:space-between;gap:16px;align-items:flex-end;border-bottom:1px solid #d7d7cf;padding-bottom:14px}h1{font-size:24px;margin:0}p{margin:6px 0 0;color:#555}.stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:8px;margin:18px 0}.stat{border:1px solid #d7d7cf;background:#fff;padding:10px;border-radius:6px}.stat b{display:block;font-size:20px}.graph{overflow:auto;border:1px solid #d7d7cf;background:#fff;border-radius:6px}.filters{display:flex;flex-wrap:wrap;gap:10px;margin:18px 0}.filters label{display:grid;gap:4px;color:#555}.filters input{min-width:220px;border:1px solid #d7d7cf;background:#fff;padding:7px;font:inherit}svg{display:block;min-width:900px}.edge{stroke:#b8b8ad;stroke-width:1.5}.dep{stroke:#c2410c}.node-label{font-size:12px;fill:#242424}.node-kind{font-size:10px;fill:#555}table{width:100%;border-collapse:collapse;margin-top:18px;background:#fff;border:1px solid #d7d7cf}th,td{padding:8px;border-bottom:1px solid #e5e5df;text-align:left}th{font-size:12px;text-transform:uppercase;color:#555}</style>");
    html.push_str("</head><body data-member-count=\"");
    html.push_str(&member_count.to_string());
    html.push_str("\" data-edge-count=\"");
    html.push_str(&edge_count.to_string());
    html.push_str("\"><main><header><div><h1>ORV Workspace Graph</h1><p>Workspace-scale member graph view generated by <code>orv workspace graph --view</code>.</p></div><a href=\"workspace-graph.json\">workspace-graph.json</a></header><section class=\"stats\"><div class=\"stat\"><span>Members</span><b>");
    html.push_str(&member_count.to_string());
    html.push_str("</b></div><div class=\"stat\"><span>Edges</span><b>");
    html.push_str(&edge_count.to_string());
    html.push_str("</b></div></section><section class=\"graph\">");
    html
}

fn workspace_graph_member_rows(members: &[serde_json::Value]) -> String {
    let mut html = String::from(
        "<table><thead><tr><th>Member</th><th>Name</th><th>Version</th><th>Entry</th></tr></thead><tbody>",
    );
    for member in members {
        html.push_str("<tr data-workspace-member-row><td>");
        html.push_str(&html_escape_text(json_str_or_empty(member, "path")));
        html.push_str("</td><td>");
        html.push_str(&html_escape_text(json_str_or_empty(member, "name")));
        html.push_str("</td><td>");
        html.push_str(&html_escape_text(json_str_or_empty(member, "version")));
        html.push_str("</td><td>");
        html.push_str(&html_escape_text(json_str_or_empty(member, "entry")));
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");
    html
}

fn workspace_graph_edge_rows(edges: &[serde_json::Value]) -> String {
    let mut html = String::from(
        "<table><thead><tr><th>Kind</th><th>From</th><th>To</th><th>Package</th></tr></thead><tbody>",
    );
    for edge in edges {
        html.push_str("<tr data-workspace-edge-row><td>");
        html.push_str(&html_escape_text(json_str_or_empty(edge, "kind")));
        html.push_str("</td><td>");
        html.push_str(&html_escape_text(json_str_or_empty(edge, "from")));
        html.push_str("</td><td>");
        html.push_str(&html_escape_text(json_str_or_empty(edge, "to")));
        html.push_str("</td><td>");
        html.push_str(&html_escape_text(json_str_or_empty(edge, "package")));
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");
    html
}

fn workspace_graph_view_svg(members: &[serde_json::Value], edges: &[serde_json::Value]) -> String {
    let row_gap = 72_i64;
    let height = i64::try_from(members.len())
        .unwrap_or(i64::MAX / row_gap)
        .saturating_mul(row_gap)
        .saturating_add(120)
        .max(220);
    let mut positions = HashMap::from([("workspace".to_string(), (90_i64, 60_i64))]);
    for (index, member) in members.iter().enumerate() {
        let row = i64::try_from(index).unwrap_or(i64::MAX / row_gap);
        positions.insert(
            json_str_or_empty(member, "path").to_string(),
            (360, row.saturating_mul(row_gap).saturating_add(60)),
        );
    }
    let mut svg = workspace_graph_svg_open(height);
    svg.push_str(&workspace_graph_svg_edges(edges, &positions));
    svg.push_str("<g><circle cx=\"90\" cy=\"60\" r=\"17\" fill=\"#2563eb\"/><text class=\"node-label\" x=\"116\" y=\"58\">workspace</text><text class=\"node-kind\" x=\"116\" y=\"73\">root</text></g>");
    for member in members {
        let path = json_str_or_empty(member, "path");
        let Some((x, y)) = positions.get(path) else {
            continue;
        };
        svg.push_str("<g><circle cx=\"");
        svg.push_str(&x.to_string());
        svg.push_str("\" cy=\"");
        svg.push_str(&y.to_string());
        svg.push_str("\" r=\"16\" fill=\"#0f766e\"/><text class=\"node-label\" x=\"");
        svg.push_str(&(x + 25).to_string());
        svg.push_str("\" y=\"");
        svg.push_str(&(y - 2).to_string());
        svg.push_str("\">");
        svg.push_str(&html_escape_text(path));
        svg.push_str("</text><text class=\"node-kind\" x=\"");
        svg.push_str(&(x + 25).to_string());
        svg.push_str("\" y=\"");
        svg.push_str(&(y + 13).to_string());
        svg.push_str("\">");
        svg.push_str(&html_escape_text(json_str_or_empty(member, "name")));
        svg.push_str("</text></g>");
    }
    svg.push_str("</svg>");
    svg
}

fn workspace_graph_svg_open(height: i64) -> String {
    let mut svg = String::new();
    svg.push_str("<svg role=\"img\" aria-label=\"ORV workspace graph\" viewBox=\"0 0 920 ");
    svg.push_str(&height.to_string());
    svg.push_str("\" height=\"");
    svg.push_str(&height.to_string());
    svg.push_str("\" xmlns=\"http://www.w3.org/2000/svg\"><defs><marker id=\"workspace-arrow\" markerWidth=\"8\" markerHeight=\"8\" refX=\"7\" refY=\"4\" orient=\"auto\"><path d=\"M0,0 L8,4 L0,8 Z\" fill=\"#b8b8ad\"/></marker></defs>");
    svg
}

fn workspace_graph_svg_edges(
    edges: &[serde_json::Value],
    positions: &HashMap<String, (i64, i64)>,
) -> String {
    let mut svg = String::new();
    for edge in edges {
        let Some((x1, y1)) = positions.get(json_str_or_empty(edge, "from")) else {
            continue;
        };
        let Some((x2, y2)) = positions.get(json_str_or_empty(edge, "to")) else {
            continue;
        };
        let class = if json_str_or_empty(edge, "kind") == "path_dependency" {
            "edge dep"
        } else {
            "edge"
        };
        svg.push_str("<line class=\"");
        svg.push_str(class);
        svg.push_str("\" marker-end=\"url(#workspace-arrow)\" x1=\"");
        svg.push_str(&x1.to_string());
        svg.push_str("\" y1=\"");
        svg.push_str(&y1.to_string());
        svg.push_str("\" x2=\"");
        svg.push_str(&x2.to_string());
        svg.push_str("\" y2=\"");
        svg.push_str(&y2.to_string());
        svg.push_str("\"><title>");
        svg.push_str(&html_escape_text(json_str_or_empty(edge, "kind")));
        svg.push_str("</title></line>");
    }
    svg
}

fn cmd_workspace_lock(root: &Path, out: &Path) -> anyhow::Result<()> {
    let graph = workspace_graph_json(root)?;
    write_json(&out.join("workspace-graph.json"), &graph)?;

    let lock_order = workspace_build_order(&graph)?;
    let dependency_edges = workspace_path_dependency_edges_from_graph(&graph)?;
    let members = graph
        .get("members")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("workspace graph members must be an array"))?;
    let member_lookup = members
        .iter()
        .map(|member| {
            Ok((
                json_str(member, "path", "workspace member")?.to_string(),
                member,
            ))
        })
        .collect::<anyhow::Result<HashMap<_, _>>>()?;

    let mut member_locks = Vec::with_capacity(lock_order.len());
    let mut package_count = 0usize;
    for member_path in &lock_order {
        let member = member_lookup
            .get(member_path)
            .ok_or_else(|| anyhow::anyhow!("workspace lock member `{member_path}` not found"))?;
        let member_path =
            workspace_member_string(Path::new(json_str(member, "path", "workspace member")?))?;
        let lock = project_lock_json(&root.join(&member_path).join("orv.toml"))?;
        let dependencies = lock
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let dev_dependencies = lock
            .get("dev_dependencies")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        package_count += dependencies.len() + dev_dependencies.len();
        let lockfile = format!("members/{member_path}/orv.lock");
        write_json(&out.join(&lockfile), &lock)?;
        member_locks.push(serde_json::json!({
            "path": member_path,
            "name": json_str(member, "name", "workspace member")?,
            "entry": json_str(member, "entry", "workspace member")?,
            "lockfile": lockfile,
            "project": lock.get("project").cloned().unwrap_or(serde_json::Value::Null),
            "dependencies": dependencies,
            "dev_dependencies": dev_dependencies,
        }));
    }

    let manifest = serde_json::json!({
        "schema_version": 1,
        "kind": "orv.workspace.lock",
        "root": root.display().to_string(),
        "workspace_graph": "workspace-graph.json",
        "stats": {
            "member_count": member_locks.len(),
            "dependency_edge_count": dependency_edges.len(),
            "package_count": package_count,
        },
        "lock_order": lock_order,
        "members": member_locks,
        "dependency_edges": dependency_edges,
    });
    write_json(&out.join("workspace-lock.json"), &manifest)?;
    println!("workspace lock: wrote {}", out.display());
    Ok(())
}

fn cmd_workspace_fetch(root: &Path, out: &Path) -> anyhow::Result<()> {
    cmd_workspace_lock(root, out)?;
    let workspace_lock = read_json_value(&out.join("workspace-lock.json"))?;
    let lock_order = workspace_lock
        .get("lock_order")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("workspace-lock.json lock_order must be an array"))?
        .iter()
        .map(|member| {
            member
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("workspace lock_order entries must be strings"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let members = workspace_lock
        .get("members")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("workspace-lock.json members must be an array"))?;
    let member_lookup = members
        .iter()
        .map(|member| {
            Ok((
                json_str(member, "path", "workspace lock member")?.to_string(),
                member,
            ))
        })
        .collect::<anyhow::Result<HashMap<_, _>>>()?;

    let mut member_fetches = Vec::with_capacity(lock_order.len());
    let mut package_count = 0usize;
    for member_path in &lock_order {
        let member = member_lookup
            .get(member_path)
            .ok_or_else(|| anyhow::anyhow!("workspace fetch member `{member_path}` not found"))?;
        let member_path = workspace_member_string(Path::new(json_str(
            member,
            "path",
            "workspace lock member",
        )?))?;
        let lockfile = json_str(member, "lockfile", "workspace lock member")?;
        let lock = read_json_value(&out.join(lockfile))?;
        let deps_dir = format!("members/{member_path}/deps");
        let deps_manifest = fetch_lock_dependencies(
            &root.join(&member_path),
            &out.join(&deps_dir),
            &lock,
            "orv.lock",
        )?;
        let member_package_count = deps_manifest["stats"]["package_count"]
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or_default();
        package_count += member_package_count;
        member_fetches.push(serde_json::json!({
            "path": member_path,
            "lockfile": lockfile,
            "deps_manifest": format!("{deps_dir}/deps-manifest.json"),
            "package_count": member_package_count,
            "packages": deps_manifest.get("packages").cloned().unwrap_or_else(|| serde_json::json!([])),
        }));
    }

    let manifest = serde_json::json!({
        "schema_version": 1,
        "kind": "orv.workspace.dependencies",
        "root": root.display().to_string(),
        "workspace_graph": "workspace-graph.json",
        "workspace_lock": "workspace-lock.json",
        "stats": {
            "member_count": member_fetches.len(),
            "package_count": package_count,
        },
        "fetch_order": lock_order,
        "members": member_fetches,
    });
    write_json(&out.join("workspace-fetch.json"), &manifest)?;
    println!("workspace fetch: wrote {}", out.display());
    Ok(())
}

fn cmd_workspace_build(
    root: &Path,
    out: &Path,
    profile: BuildProfile,
    incremental: bool,
) -> anyhow::Result<()> {
    let _ = incremental;
    let graph = workspace_graph_json(root)?;
    let graph_path = out.join("workspace-graph.json");
    write_json(&graph_path, &graph)?;

    let build_order = workspace_build_order(&graph)?;
    let dependency_edges = workspace_path_dependency_edges_from_graph(&graph)?;
    let member_dependencies = workspace_member_dependency_map(&dependency_edges)?;
    let previous_manifest = if incremental {
        read_workspace_build_manifest(out)?
    } else {
        None
    };
    let members = graph
        .get("members")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("workspace graph members must be an array"))?;
    let member_lookup = members
        .iter()
        .map(|member| {
            Ok((
                json_str(member, "path", "workspace member")?.to_string(),
                member,
            ))
        })
        .collect::<anyhow::Result<HashMap<_, _>>>()?;
    let mut member_builds = Vec::with_capacity(members.len());
    let mut dirty_members = HashSet::new();
    let mut built_count = 0usize;
    let mut skipped_count = 0usize;
    for member_path in &build_order {
        let member = member_lookup
            .get(member_path)
            .ok_or_else(|| anyhow::anyhow!("workspace build member `{member_path}` not found"))?;
        let member_path =
            workspace_member_string(Path::new(json_str(member, "path", "workspace member")?))?;
        let name = json_str(member, "name", "workspace member")?;
        let entry = json_str(member, "entry", "workspace member")?;
        let input_hash = workspace_member_input_hash(root, member)?;
        let build_dir = format!("members/{member_path}");
        let member_out = out.join(&build_dir);
        let dependency_dirty = member_dependencies
            .get(&member_path)
            .is_some_and(|dependencies| dependencies.iter().any(|dep| dirty_members.contains(dep)));
        let skip = incremental
            && !dependency_dirty
            && workspace_previous_member_matches(
                previous_manifest.as_ref(),
                profile,
                &member_path,
                &build_dir,
                &input_hash,
            )
            && cmd_verify_build(&member_out).is_ok();
        let status = if skip {
            skipped_count += 1;
            "skipped"
        } else {
            cmd_build_with_profile(&root.join(entry), &member_out, profile)?;
            cmd_verify_build(&member_out)?;
            dirty_members.insert(member_path.clone());
            built_count += 1;
            "built"
        };
        member_builds.push(serde_json::json!({
            "path": member_path,
            "name": name,
            "entry": entry,
            "build_dir": build_dir,
            "manifest": format!("{build_dir}/build-manifest.json"),
            "input_hash": input_hash,
            "status": status,
            "verified": true,
        }));
    }

    let manifest = serde_json::json!({
        "schema_version": 1,
        "kind": "orv.workspace.build",
        "profile": profile.as_str(),
        "incremental": incremental,
        "root": root.display().to_string(),
        "workspace_graph": "workspace-graph.json",
        "stats": {
            "member_count": member_builds.len(),
            "dependency_edge_count": dependency_edges.len(),
            "built_count": built_count,
            "skipped_count": skipped_count,
        },
        "build_order": build_order,
        "members": member_builds,
        "dependency_edges": dependency_edges,
    });
    write_json(&out.join("workspace-build.json"), &manifest)?;
    println!("workspace build: wrote {}", out.display());
    Ok(())
}

fn read_workspace_build_manifest(out: &Path) -> anyhow::Result<Option<serde_json::Value>> {
    let path = out.join("workspace-build.json");
    if !path.is_file() {
        return Ok(None);
    }
    read_json_value(&path).map(Some)
}

fn workspace_path_dependency_edges_from_graph(
    graph: &serde_json::Value,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let edges = graph
        .get("edges")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("workspace graph edges must be an array"))?;
    Ok(edges
        .iter()
        .filter(|edge| {
            edge.get("kind").and_then(serde_json::Value::as_str) == Some("path_dependency")
        })
        .cloned()
        .collect())
}

fn workspace_member_dependency_map(
    edges: &[serde_json::Value],
) -> anyhow::Result<HashMap<String, Vec<String>>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for edge in edges {
        let dependent = json_str(edge, "from", "workspace edge")?.to_string();
        let dependency = json_str(edge, "to", "workspace edge")?.to_string();
        map.entry(dependent).or_default().push(dependency);
    }
    for dependencies in map.values_mut() {
        dependencies.sort();
        dependencies.dedup();
    }
    Ok(map)
}

fn workspace_member_input_hash(root: &Path, member: &serde_json::Value) -> anyhow::Result<String> {
    let entry = root.join(json_str(member, "entry", "workspace member")?);
    let loaded = orv_project::load_project(&entry).map_err(|e| anyhow::anyhow!("{e}"))?;
    report_diagnostics(&loaded.diagnostics, &loaded.files)?;
    let source_bundle = orv_compiler::source_bundle_artifact(
        entry.display().to_string(),
        loaded
            .files
            .iter()
            .map(|file| (file.path.display().to_string(), file.source.clone())),
    );
    let hash = stable_json_hash(&serde_json::to_value(&source_bundle)?)?;
    Ok(format!("fnv1a64:{hash}"))
}

fn workspace_previous_member_matches(
    previous_manifest: Option<&serde_json::Value>,
    profile: BuildProfile,
    member_path: &str,
    build_dir: &str,
    input_hash: &str,
) -> bool {
    let Some(manifest) = previous_manifest else {
        return false;
    };
    if manifest.get("profile").and_then(serde_json::Value::as_str) != Some(profile.as_str()) {
        return false;
    }
    manifest
        .get("members")
        .and_then(serde_json::Value::as_array)
        .and_then(|members| {
            members.iter().find(|member| {
                member.get("path").and_then(serde_json::Value::as_str) == Some(member_path)
            })
        })
        .is_some_and(|member| {
            member.get("build_dir").and_then(serde_json::Value::as_str) == Some(build_dir)
                && member.get("input_hash").and_then(serde_json::Value::as_str) == Some(input_hash)
        })
}

fn workspace_build_order(graph: &serde_json::Value) -> anyhow::Result<Vec<String>> {
    let members = graph
        .get("members")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("workspace graph members must be an array"))?;
    let mut indegree = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    for member in members {
        let path = json_str(member, "path", "workspace member")?.to_string();
        indegree.insert(path.clone(), 0usize);
        dependents.insert(path, Vec::new());
    }
    let edges = graph
        .get("edges")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("workspace graph edges must be an array"))?;
    for edge in edges {
        if edge.get("kind").and_then(serde_json::Value::as_str) != Some("path_dependency") {
            continue;
        }
        let dependent = json_str(edge, "from", "workspace edge")?;
        let dependency = json_str(edge, "to", "workspace edge")?;
        if !indegree.contains_key(dependent) || !indegree.contains_key(dependency) {
            anyhow::bail!(
                "workspace dependency edge references unknown member `{dependent}` -> `{dependency}`"
            );
        }
        *indegree
            .get_mut(dependent)
            .expect("dependent checked above") += 1;
        dependents
            .entry(dependency.to_string())
            .or_default()
            .push(dependent.to_string());
    }
    for edges in dependents.values_mut() {
        edges.sort();
        edges.dedup();
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(member, degree)| (*degree == 0).then_some(member.clone()))
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(indegree.len());
    while let Some(member) = ready.pop_first() {
        if let Some(edges) = dependents.get(&member) {
            for dependent in edges {
                let degree = indegree
                    .get_mut(dependent)
                    .expect("dependent came from workspace member");
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(dependent.clone());
                }
            }
        }
        order.push(member);
    }
    if order.len() != indegree.len() {
        anyhow::bail!("workspace dependency graph contains a cycle");
    }
    Ok(order)
}

fn project_entry_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_dir() {
        return project_manifest_entry_path(&path.join("orv.toml"));
    }
    if path.file_name().is_some_and(|name| name == "orv.toml") {
        return project_manifest_entry_path(path);
    }
    Ok(path.to_path_buf())
}

fn project_manifest_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_dir() {
        return Ok(path.join("orv.toml"));
    }
    if path.file_name().is_some_and(|name| name == "orv.toml") {
        return Ok(path.to_path_buf());
    }
    anyhow::bail!("lock path must be a project directory or orv.toml")
}

fn project_manifest_entry_path(manifest: &Path) -> anyhow::Result<PathBuf> {
    let source = std::fs::read_to_string(manifest)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", manifest.display()))?;
    let value = toml::from_str::<toml::Value>(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", manifest.display()))?;
    let entry = value
        .get("project")
        .and_then(|project| project.get("entry"))
        .and_then(toml::Value::as_str)
        .filter(|entry| !entry.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{} must define [project].entry", manifest.display()))?;
    let base = manifest.parent().unwrap_or_else(|| Path::new("."));
    Ok(base.join(entry))
}

fn project_lock_json(manifest: &Path) -> anyhow::Result<serde_json::Value> {
    let source = std::fs::read_to_string(manifest)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", manifest.display()))?;
    let value = toml::from_str::<toml::Value>(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", manifest.display()))?;
    let root = manifest.parent().unwrap_or_else(|| Path::new("."));
    let project = value
        .get("project")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| anyhow::anyhow!("{} must define [project]", manifest.display()))?;
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.lock",
        "project": {
            "name": toml_string(project, "name", "[project].name")?,
            "version": toml_string(project, "version", "[project].version")?,
            "entry": toml_string(project, "entry", "[project].entry")?,
        },
        "dependencies": lock_dependency_entries(root, &value, "dependencies")?,
        "dev_dependencies": lock_dependency_entries(root, &value, "dev-dependencies")?,
    }))
}

fn lock_dependency_entries(
    root: &Path,
    manifest: &toml::Value,
    section: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let Some(table) = manifest.get(section).and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };
    let mut entries = table
        .iter()
        .map(|(name, value)| lock_dependency_entry(root, section, name, value))
        .collect::<anyhow::Result<Vec<_>>>()?;
    entries.sort_by(|left, right| {
        json_str_or_empty(left, "name").cmp(json_str_or_empty(right, "name"))
    });
    Ok(entries)
}

fn lock_dependency_entry(
    root: &Path,
    section: &str,
    name: &str,
    value: &toml::Value,
) -> anyhow::Result<serde_json::Value> {
    let mut entry = match value {
        toml::Value::String(version) => registry_lock_dependency(root, section, name, version)?,
        toml::Value::Table(table) if table.contains_key("path") => {
            path_lock_dependency(section, name, table)?
        }
        toml::Value::Table(table) => registry_table_lock_dependency(root, section, name, table)?,
        _ => anyhow::bail!("{section}.{name} must be a version string or inline table"),
    };
    let checksum_input = entry.clone();
    entry["checksum"] =
        serde_json::json!(format!("fnv1a64:{}", stable_json_hash(&checksum_input)?));
    Ok(entry)
}

fn registry_table_lock_dependency(
    root: &Path,
    section: &str,
    name: &str,
    table: &toml::map::Map<String, toml::Value>,
) -> anyhow::Result<serde_json::Value> {
    let version = toml_string(table, "version", "dependency.version")?;
    let registry = table
        .get("registry")
        .and_then(toml::Value::as_str)
        .unwrap_or("registry.orv.dev");
    let auth_token_env =
        toml_optional_string(table, "auth_token_env", "dependency.auth_token_env")?;
    registry_lock_dependency_with_source(root, section, name, version, registry, auth_token_env)
}

fn registry_lock_dependency(
    root: &Path,
    section: &str,
    name: &str,
    version: &str,
) -> anyhow::Result<serde_json::Value> {
    registry_lock_dependency_with_source(root, section, name, version, "registry.orv.dev", None)
}

fn registry_lock_dependency_with_source(
    root: &Path,
    section: &str,
    name: &str,
    version: &str,
    registry: &str,
    auth_token_env: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    if version.trim().is_empty() {
        anyhow::bail!("{section}.{name} version must not be empty");
    }
    if registry.trim().is_empty() {
        anyhow::bail!("{section}.{name} registry must not be empty");
    }
    let resolved = resolve_registry_version(root, name, version, registry, auth_token_env)?;
    let resolved_changed = resolved != version;
    let mut entry = serde_json::json!({
        "name": name,
        "section": section,
        "source": "registry",
        "registry": registry,
        "version": resolved,
    });
    if resolved_changed {
        entry["requested_version"] = serde_json::json!(version);
    }
    if let Some(auth_token_env) = auth_token_env {
        entry["auth_token_env"] = serde_json::json!(auth_token_env);
    }
    Ok(entry)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SemverVersion {
    major: u64,
    minor: u64,
    patch: u64,
    pre_release: Vec<SemverPreReleaseId>,
    build: Option<String>,
    raw: String,
}

impl Ord for SemverVersion {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| compare_pre_release(&self.pre_release, &other.pre_release))
            .then_with(|| self.build.cmp(&other.build))
            .then_with(|| self.raw.cmp(&other.raw))
    }
}

impl PartialOrd for SemverVersion {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SemverPreReleaseId {
    Numeric(u64),
    AlphaNumeric(String),
}

fn compare_pre_release(left: &[SemverPreReleaseId], right: &[SemverPreReleaseId]) -> CmpOrdering {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => return CmpOrdering::Equal,
        (true, false) => return CmpOrdering::Greater,
        (false, true) => return CmpOrdering::Less,
        (false, false) => {}
    }
    for (left, right) in left.iter().zip(right) {
        let ordering = match (left, right) {
            (SemverPreReleaseId::Numeric(left), SemverPreReleaseId::Numeric(right)) => {
                left.cmp(right)
            }
            (SemverPreReleaseId::Numeric(_), SemverPreReleaseId::AlphaNumeric(_)) => {
                CmpOrdering::Less
            }
            (SemverPreReleaseId::AlphaNumeric(_), SemverPreReleaseId::Numeric(_)) => {
                CmpOrdering::Greater
            }
            (SemverPreReleaseId::AlphaNumeric(left), SemverPreReleaseId::AlphaNumeric(right)) => {
                left.cmp(right)
            }
        };
        if ordering != CmpOrdering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

fn resolve_registry_version(
    root: &Path,
    name: &str,
    requested: &str,
    registry: &str,
    auth_token_env: Option<&str>,
) -> anyhow::Result<String> {
    if parse_semver_version(requested).is_some() {
        return Ok(requested.to_string());
    }
    let versions = registry_index_versions(root, name, registry, auth_token_env)?;
    let resolved = versions
        .into_iter()
        .filter(|version| registry_version_matches(requested, version))
        .max()
        .ok_or_else(|| anyhow::anyhow!("no registry version for {name} matches `{requested}`"))?;
    Ok(resolved.raw)
}

fn registry_index_versions(
    root: &Path,
    name: &str,
    registry: &str,
    auth_token_env: Option<&str>,
) -> anyhow::Result<Vec<SemverVersion>> {
    let index = if registry.starts_with("http://") || registry.starts_with("https://") {
        read_json_from_registry_url(
            &format!("{}/{name}/index.json", registry.trim_end_matches('/')),
            auth_token_env,
        )?
    } else if registry == "registry.orv.dev" {
        anyhow::bail!("registry.orv.dev resolution is not implemented yet")
    } else {
        let root = registry.strip_prefix("file://").map_or_else(
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
        read_json_value(&root.join(name).join("index.json"))?
    };
    let versions = index
        .get("versions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("registry index versions must be an array"))?;
    versions
        .iter()
        .map(|version| {
            let raw = version
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("registry index versions must be strings"))?;
            parse_semver_version(raw).ok_or_else(|| {
                anyhow::anyhow!("registry version `{raw}` is not semver x.y.z[-pre][+build]")
            })
        })
        .collect()
}

fn read_json_from_registry_url(
    url: &str,
    auth_token_env: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let source = registry_get_string_with_auth(url, auth_token_env)?;
    serde_json::from_str(&source).map_err(|e| anyhow::anyhow!("failed to parse {url}: {e}"))
}

fn registry_version_matches(requested: &str, version: &SemverVersion) -> bool {
    if requested.contains("||") {
        return requested
            .split("||")
            .map(str::trim)
            .filter(|clause| !clause.is_empty())
            .any(|clause| registry_version_matches(clause, version));
    }
    if is_wildcard_segment(requested) {
        return true;
    }
    let Some(base) = requested.strip_prefix('^').and_then(parse_semver_version) else {
        if let Some(base) = requested.strip_prefix('~').and_then(parse_semver_version) {
            return version >= &base && version.major == base.major && version.minor == base.minor;
        }
        return wildcard_version_matches(requested, version)
            .or_else(|| comparator_range_matches(requested, version))
            .unwrap_or(false);
    };
    if version < &base {
        return false;
    }
    if base.major > 0 {
        version.major == base.major
    } else if base.minor > 0 {
        version.major == 0 && version.minor == base.minor
    } else {
        version.major == 0 && version.minor == 0 && version.patch == base.patch
    }
}

fn comparator_range_matches(requested: &str, version: &SemverVersion) -> Option<bool> {
    let tokens = requested.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    tokens
        .iter()
        .map(|token| comparator_matches(token, version))
        .collect::<Option<Vec<_>>>()
        .map(|matches| matches.into_iter().all(|matched| matched))
}

fn comparator_matches(token: &str, version: &SemverVersion) -> Option<bool> {
    let (operator, raw_version) = parse_comparator_token(token)?;
    let base = parse_semver_version(raw_version)?;
    Some(match operator {
        ComparatorOperator::Greater => version > &base,
        ComparatorOperator::GreaterEqual => version >= &base,
        ComparatorOperator::Less => version < &base,
        ComparatorOperator::LessEqual => version <= &base,
        ComparatorOperator::Equal => version == &base,
    })
}

fn parse_comparator_token(token: &str) -> Option<(ComparatorOperator, &str)> {
    for (prefix, operator) in [
        (">=", ComparatorOperator::GreaterEqual),
        ("<=", ComparatorOperator::LessEqual),
        (">", ComparatorOperator::Greater),
        ("<", ComparatorOperator::Less),
        ("=", ComparatorOperator::Equal),
    ] {
        if let Some(raw_version) = token.strip_prefix(prefix) {
            return Some((operator, raw_version));
        }
    }
    None
}

#[derive(Clone, Copy)]
enum ComparatorOperator {
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    Equal,
}

fn wildcard_version_matches(requested: &str, version: &SemverVersion) -> Option<bool> {
    let parts = requested.split('.').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let wildcard_at = parts.iter().position(|part| is_wildcard_segment(part))?;
    if !parts[wildcard_at..]
        .iter()
        .all(|part| is_wildcard_segment(part))
    {
        return None;
    }
    let numbers = parts[..wildcard_at]
        .iter()
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    match numbers.as_slice() {
        [major] => Some(version.major == *major),
        [major, minor] => Some(version.major == *major && version.minor == *minor),
        _ => None,
    }
}

fn is_wildcard_segment(segment: &str) -> bool {
    matches!(segment, "*" | "x" | "X")
}

fn parse_semver_version(version: &str) -> Option<SemverVersion> {
    let (without_build, build) = version
        .split_once('+')
        .map_or((version, None), |(without_build, build)| {
            (without_build, Some(build))
        });
    if build.is_some_and(|build| !is_valid_semver_identifier_list(build)) {
        return None;
    }
    let (core, pre_release) = without_build
        .split_once('-')
        .map_or((without_build, None), |(core, pre_release)| {
            (core, Some(pre_release))
        });
    let pre_release = match pre_release {
        Some(pre_release) => parse_pre_release_identifiers(pre_release)?,
        None => Vec::new(),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    parts.next().is_none().then(|| SemverVersion {
        major,
        minor,
        patch,
        pre_release,
        build: build.map(str::to_string),
        raw: version.to_string(),
    })
}

fn parse_pre_release_identifiers(raw: &str) -> Option<Vec<SemverPreReleaseId>> {
    if raw.is_empty() {
        return None;
    }
    raw.split('.')
        .map(|identifier| {
            if identifier.is_empty()
                || !identifier
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
            {
                return None;
            }
            identifier.parse::<u64>().map_or_else(
                |_| Some(SemverPreReleaseId::AlphaNumeric(identifier.to_string())),
                |number| Some(SemverPreReleaseId::Numeric(number)),
            )
        })
        .collect()
}

fn is_valid_semver_identifier_list(raw: &str) -> bool {
    !raw.is_empty()
        && raw.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
}

fn path_lock_dependency(
    section: &str,
    name: &str,
    table: &toml::map::Map<String, toml::Value>,
) -> anyhow::Result<serde_json::Value> {
    let path = toml_string(table, "path", "dependency.path")?;
    if path.trim().is_empty() {
        anyhow::bail!("{section}.{name} path must not be empty");
    }
    Ok(serde_json::json!({
        "name": name,
        "section": section,
        "source": "path",
        "path": path,
        "version": table.get("version").and_then(toml::Value::as_str).unwrap_or("0.0.0"),
    }))
}

fn toml_string<'a>(
    table: &'a toml::map::Map<String, toml::Value>,
    field: &str,
    context: &str,
) -> anyhow::Result<&'a str> {
    table
        .get(field)
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{context} must be a non-empty string"))
}

fn toml_optional_string<'a>(
    table: &'a toml::map::Map<String, toml::Value>,
    field: &str,
    context: &str,
) -> anyhow::Result<Option<&'a str>> {
    let Some(value) = table.get(field) else {
        return Ok(None);
    };
    let Some(value) = value.as_str().filter(|value| !value.trim().is_empty()) else {
        anyhow::bail!("{context} must be a non-empty string");
    };
    Ok(Some(value))
}

fn read_toml_manifest(manifest: &Path) -> anyhow::Result<toml::Value> {
    let source = std::fs::read_to_string(manifest)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", manifest.display()))?;
    toml::from_str::<toml::Value>(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", manifest.display()))
}

fn write_toml_manifest_atomic(manifest: &Path, value: &toml::Value) -> anyhow::Result<()> {
    let temp = atomic_temp_path(manifest);
    let source = toml::to_string_pretty(value)
        .map_err(|e| anyhow::anyhow!("failed to serialize {}: {e}", manifest.display()))?;
    std::fs::write(&temp, source)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", temp.display()))?;
    std::fs::rename(&temp, manifest).map_err(|e| {
        anyhow::anyhow!(
            "failed to replace {} with {}: {e}",
            manifest.display(),
            temp.display()
        )
    })
}

fn add_dependency_to_manifest(
    manifest: &mut toml::Value,
    name: &str,
    version: Option<&str>,
    dev: bool,
    path: Option<&Path>,
    registry: Option<&str>,
) -> anyhow::Result<()> {
    validate_dependency_name(name)?;
    let section = dependency_section(dev);
    let root = manifest
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("orv.toml root must be a table"))?;
    let dependencies = root
        .entry(section.to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("{section} must be a table"))?;
    dependencies.insert(
        name.to_string(),
        dependency_manifest_value(name, version, path, registry)?,
    );
    Ok(())
}

fn remove_dependency_from_manifest(
    manifest: &mut toml::Value,
    name: &str,
    dev: bool,
) -> anyhow::Result<()> {
    validate_dependency_name(name)?;
    let section = dependency_section(dev);
    let root = manifest
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("orv.toml root must be a table"))?;
    let dependencies = root
        .get_mut(section)
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("{section}.{name} is not present"))?;
    if dependencies.remove(name).is_none() {
        anyhow::bail!("{section}.{name} is not present");
    }
    if dependencies.is_empty() {
        root.remove(section);
    }
    Ok(())
}

fn dependency_manifest_value(
    name: &str,
    version: Option<&str>,
    path: Option<&Path>,
    registry: Option<&str>,
) -> anyhow::Result<toml::Value> {
    if let Some(path) = path {
        let mut table = toml::map::Map::new();
        table.insert(
            "path".to_string(),
            toml::Value::String(path.to_string_lossy().into_owned()),
        );
        table.insert(
            "version".to_string(),
            toml::Value::String(version.unwrap_or("0.0.0").to_string()),
        );
        return Ok(toml::Value::Table(table));
    }
    let version =
        version.ok_or_else(|| anyhow::anyhow!("{name} registry dependency requires a version"))?;
    if version.trim().is_empty() {
        anyhow::bail!("{name} registry dependency version must not be empty");
    }
    if let Some(registry) = registry {
        if registry.trim().is_empty() {
            anyhow::bail!("{name} registry must not be empty");
        }
        let mut table = toml::map::Map::new();
        table.insert(
            "version".to_string(),
            toml::Value::String(version.to_string()),
        );
        table.insert(
            "registry".to_string(),
            toml::Value::String(registry.to_string()),
        );
        return Ok(toml::Value::Table(table));
    }
    Ok(toml::Value::String(version.to_string()))
}

fn validate_dependency_name(name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("dependency name must not be empty");
    }
    if name.contains('@') {
        anyhow::bail!("dependency name must not include @; pass the version separately");
    }
    Ok(())
}

const fn dependency_section(dev: bool) -> &'static str {
    if dev {
        "dev-dependencies"
    } else {
        "dependencies"
    }
}

fn add_workspace_member_to_manifest(
    manifest: &mut toml::Value,
    member: &Path,
) -> anyhow::Result<String> {
    let member = workspace_member_string(member)?;
    let root = manifest
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("orv.toml root must be a table"))?;
    let workspace = root
        .entry("workspace".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("workspace must be a table"))?;
    workspace
        .entry("resolver".to_string())
        .or_insert_with(|| toml::Value::String("2".to_string()));
    let members = workspace
        .entry("members".to_string())
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("workspace.members must be an array"))?;
    if members.iter().any(|item| item.as_str() == Some(&member)) {
        return Ok(member);
    }
    members.push(toml::Value::String(member.clone()));
    members
        .sort_by(|left, right| toml_value_str_or_empty(left).cmp(toml_value_str_or_empty(right)));
    Ok(member)
}

fn workspace_member_string(member: &Path) -> anyhow::Result<String> {
    if member.is_absolute() {
        anyhow::bail!("workspace member path must be relative");
    }
    let member = member.to_string_lossy().replace('\\', "/");
    if member.trim().is_empty() || member.split('/').any(|segment| segment == "..") {
        anyhow::bail!("workspace member path must be a non-empty relative path");
    }
    Ok(member)
}

fn workspace_member_project_name(member: &Path) -> String {
    member
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("member")
        .to_string()
}

fn toml_value_str_or_empty(value: &toml::Value) -> &str {
    value.as_str().unwrap_or("")
}

fn workspace_graph_json(root: &Path) -> anyhow::Result<serde_json::Value> {
    let root_manifest = root.join("orv.toml");
    let manifest = read_toml_manifest(&root_manifest)?;
    let workspace = manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| anyhow::anyhow!("{} must define [workspace]", root_manifest.display()))?;
    let resolver = workspace
        .get("resolver")
        .and_then(toml::Value::as_str)
        .unwrap_or("1");
    let members = workspace_members(workspace)?;
    let mut member_values = Vec::new();
    let mut member_paths = HashSet::new();
    for member in &members {
        member_paths.insert(member.clone());
        member_values.push(workspace_member_graph_json(root, member)?);
    }
    let member_packages = workspace_member_package_map(&member_values)?;
    let edges = workspace_graph_edges(root, &members, &member_paths, &member_packages)?;
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.workspace.graph",
        "root": root.display().to_string(),
        "resolver": resolver,
        "stats": {
            "member_count": members.len(),
            "edge_count": edges.len(),
        },
        "members": member_values,
        "edges": edges,
    }))
}

struct WorkspaceMemberPackage {
    name: String,
    version: String,
}

fn workspace_member_package_map(
    members: &[serde_json::Value],
) -> anyhow::Result<HashMap<String, WorkspaceMemberPackage>> {
    members
        .iter()
        .map(|member| {
            Ok((
                json_str(member, "path", "workspace member")?.to_string(),
                WorkspaceMemberPackage {
                    name: json_str(member, "name", "workspace member")?.to_string(),
                    version: json_str(member, "version", "workspace member")?.to_string(),
                },
            ))
        })
        .collect()
}

fn workspace_members(
    workspace: &toml::map::Map<String, toml::Value>,
) -> anyhow::Result<Vec<String>> {
    let members = workspace
        .get("members")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("workspace.members must be an array"))?;
    let mut paths = members
        .iter()
        .map(|member| {
            let member = member
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("workspace.members entries must be strings"))?;
            workspace_member_string(Path::new(member))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn workspace_member_graph_json(root: &Path, member: &str) -> anyhow::Result<serde_json::Value> {
    let member_root = root.join(member);
    let manifest_path = member_root.join("orv.toml");
    let manifest = read_toml_manifest(&manifest_path)?;
    let project = manifest
        .get("project")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| anyhow::anyhow!("{} must define [project]", manifest_path.display()))?;
    let entry = project_manifest_entry_path(&manifest_path)?;
    let loaded = orv_project::load_project(&entry).map_err(|e| anyhow::anyhow!("{e}"))?;
    report_diagnostics(&loaded.diagnostics, &loaded.files)?;
    let resolved = orv_resolve::resolve(&loaded.program);
    report_diagnostics(&resolved.diagnostics, &loaded.files)?;
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    report_diagnostics(&lowered.diagnostics, &loaded.files)?;
    let origin_map = orv_compiler::origin_map(&lowered.program);
    Ok(serde_json::json!({
        "name": toml_string(project, "name", "[project].name")?,
        "version": project.get("version").and_then(toml::Value::as_str).unwrap_or("0.0.0"),
        "path": member,
        "manifest": format!("{member}/orv.toml"),
        "entry": workspace_relative_path(root, &entry),
        "files": loaded.files.iter().map(|file| workspace_relative_path(root, &file.path)).collect::<Vec<_>>(),
        "graph": project_graph_json(&loaded.graph, &origin_map),
        "dependencies": workspace_member_dependencies(&manifest),
    }))
}

fn workspace_member_dependencies(manifest: &toml::Value) -> Vec<serde_json::Value> {
    ["dependencies", "dev-dependencies"]
        .into_iter()
        .flat_map(|section| workspace_dependency_values(manifest, section))
        .collect()
}

fn workspace_dependency_values(manifest: &toml::Value, section: &str) -> Vec<serde_json::Value> {
    let Some(table) = manifest.get(section).and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    let mut dependencies = table
        .iter()
        .map(|(name, value)| {
            let mut dependency = serde_json::json!({
                "name": name,
                "section": section,
            });
            if let Some(version) = value.as_str().or_else(|| {
                value
                    .as_table()
                    .and_then(|table| table.get("version"))
                    .and_then(toml::Value::as_str)
            }) {
                dependency["version"] = serde_json::json!(version);
            }
            if let Some(table) = value.as_table() {
                if let Some(registry) = table.get("registry").and_then(toml::Value::as_str) {
                    dependency["registry"] = serde_json::json!(registry);
                }
            }
            if let Some(path) = value.as_table().and_then(|table| {
                table
                    .get("path")
                    .and_then(toml::Value::as_str)
                    .filter(|path| !path.trim().is_empty())
            }) {
                dependency["source"] = serde_json::json!("path");
                dependency["path"] = serde_json::json!(path);
            } else {
                dependency["source"] = serde_json::json!("registry");
            }
            dependency
        })
        .collect::<Vec<_>>();
    dependencies.sort_by(|left, right| {
        json_str_or_empty(left, "name").cmp(json_str_or_empty(right, "name"))
    });
    dependencies
}

fn workspace_graph_edges(
    root: &Path,
    members: &[String],
    member_paths: &HashSet<String>,
    member_packages: &HashMap<String, WorkspaceMemberPackage>,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut edges = members
        .iter()
        .map(|member| {
            serde_json::json!({
                "kind": "member",
                "from": "workspace",
                "to": member,
            })
        })
        .collect::<Vec<_>>();
    for member in members {
        let manifest = read_toml_manifest(&root.join(member).join("orv.toml"))?;
        edges.extend(workspace_path_dependency_edges(
            root,
            member,
            &manifest,
            member_paths,
            member_packages,
        )?);
    }
    Ok(edges)
}

fn workspace_path_dependency_edges(
    root: &Path,
    member: &str,
    manifest: &toml::Value,
    member_paths: &HashSet<String>,
    member_packages: &HashMap<String, WorkspaceMemberPackage>,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut edges = Vec::new();
    for section in ["dependencies", "dev-dependencies"] {
        let Some(dependencies) = manifest.get(section).and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, value) in dependencies {
            let Some(table) = value.as_table() else {
                continue;
            };
            let Some(path) = table
                .get("path")
                .and_then(toml::Value::as_str)
                .filter(|path| !path.trim().is_empty())
            else {
                continue;
            };
            let Some(target) = workspace_dependency_target(root, member, path) else {
                continue;
            };
            if !member_paths.contains(&target) {
                continue;
            }
            let target_package = member_packages.get(&target).ok_or_else(|| {
                anyhow::anyhow!("workspace member `{target}` has no package metadata")
            })?;
            let requested_version = table
                .get("version")
                .and_then(toml::Value::as_str)
                .filter(|version| !version.trim().is_empty());
            let mut edge = serde_json::json!({
                "kind": "path_dependency",
                "from": member,
                "to": target,
                "package": name,
                "section": section,
                "target_name": target_package.name,
                "target_version": target_package.version,
            });
            if let Some(requested_version) = requested_version {
                if !workspace_member_version_matches(requested_version, &target_package.version) {
                    anyhow::bail!(
                        "workspace dependency {member} -> {target} requests `{requested_version}` but target version is `{}`",
                        target_package.version
                    );
                }
                edge["requested_version"] = serde_json::json!(requested_version);
                edge["version_match"] = serde_json::json!(true);
            }
            edges.push(edge);
        }
    }
    Ok(edges)
}

fn workspace_dependency_target(root: &Path, member: &str, dependency: &str) -> Option<String> {
    let target = normalize_workspace_fs_path(&root.join(member).join(dependency));
    target
        .strip_prefix(normalize_workspace_fs_path(root))
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

fn workspace_member_version_matches(requested: &str, actual: &str) -> bool {
    let Some(actual) = parse_semver_version(actual) else {
        return requested == actual;
    };
    parse_semver_version(requested).map_or_else(
        || registry_version_matches(requested, &actual),
        |exact| actual == exact,
    )
}

fn workspace_relative_path(root: &Path, path: &Path) -> String {
    normalize_workspace_fs_path(path)
        .strip_prefix(normalize_workspace_fs_path(root))
        .map_or_else(
            |_| path.display().to_string(),
            |relative| relative.to_string_lossy().replace('\\', "/"),
        )
}

fn normalize_workspace_fs_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn write_new_text_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    if path.exists() {
        anyhow::bail!("refusing to overwrite {}", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, contents)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn byte_position(source: &str, byte: u32) -> (usize, usize) {
    let byte = usize::try_from(byte)
        .unwrap_or(source.len())
        .min(source.len());
    let prefix = source.get(..byte).unwrap_or(source);
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let character = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, tail)| tail)
        .chars()
        .count();
    (line, character)
}

fn current_db_schema_snapshot(path: &Path) -> anyhow::Result<serde_json::Value> {
    let entry = project_entry_path(path)?;
    let loaded = orv_project::load_project(&entry).map_err(|e| anyhow::anyhow!("{e}"))?;
    report_diagnostics(&loaded.diagnostics, &loaded.files)?;
    Ok(db_schema_snapshot_json(&loaded.program))
}

fn write_json_atomic(path: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
    }
    let temp = atomic_temp_path(path);
    let bytes = serde_json::to_vec_pretty(value)?;
    std::fs::write(&temp, bytes)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|e| {
        anyhow::anyhow!(
            "failed to replace {} with {}: {e}",
            path.display(),
            temp.display()
        )
    })
}

fn atomic_temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("schema.json");
    path.with_file_name(format!(".{file_name}.tmp"))
}

fn rollback_schema_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("schema.json");
    path.with_file_name(format!("{file_name}.rollback"))
}

fn stable_json_hash(value: &serde_json::Value) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(value)?;
    Ok(format!("{:016x}", fnv1a64(&bytes)))
}

fn file_content_hash(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    Ok(format!("{:016x}", fnv1a64(&bytes)))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x00000100000001b3);
    }
    hash
}

fn empty_db_schema_snapshot() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "structs": {},
    })
}

fn db_schema_snapshot_json(program: &Program) -> serde_json::Value {
    let mut structs = serde_json::Map::new();
    for item in &program.items {
        let Stmt::Struct(stmt) = item else {
            continue;
        };
        let mut fields = serde_json::Map::new();
        for field in &stmt.fields {
            fields.insert(
                field.name.name.clone(),
                serde_json::json!({
                    "type": type_ref_string(&field.ty),
                    "optional": type_ref_optional(&field.ty),
                    "span": span_json(field.span),
                }),
            );
        }
        structs.insert(
            stmt.name.name.clone(),
            serde_json::json!({
                "fields": fields,
                "span": span_json(stmt.span),
            }),
        );
    }
    serde_json::json!({
        "schema_version": 1,
        "structs": structs,
    })
}

fn db_schema_diff_actions(
    applied_schema: &serde_json::Value,
    current_schema: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let Some(current_structs) = current_schema
        .get("structs")
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };
    let empty = serde_json::Map::new();
    let applied_structs = applied_schema
        .get("structs")
        .and_then(serde_json::Value::as_object)
        .unwrap_or(&empty);
    let mut actions = Vec::new();
    for (struct_name, current_struct) in current_structs {
        let Some(applied_struct) = applied_structs.get(struct_name) else {
            actions.push(serde_json::json!({
                "kind": "create_struct",
                "struct": struct_name,
                "fields": schema_fields(current_struct).cloned().unwrap_or_default(),
            }));
            continue;
        };
        diff_schema_fields(struct_name, applied_struct, current_struct, &mut actions);
    }
    for struct_name in applied_structs.keys() {
        if !current_structs.contains_key(struct_name) {
            actions.push(serde_json::json!({
                "kind": "drop_struct",
                "struct": struct_name,
            }));
        }
    }
    actions
}

fn diff_schema_fields(
    struct_name: &str,
    applied_struct: &serde_json::Value,
    current_struct: &serde_json::Value,
    actions: &mut Vec<serde_json::Value>,
) {
    let empty = serde_json::Map::new();
    let applied_fields = schema_fields(applied_struct).unwrap_or(&empty);
    let current_fields = schema_fields(current_struct).unwrap_or(&empty);
    for (field_name, current_field) in current_fields {
        let Some(applied_field) = applied_fields.get(field_name) else {
            actions.push(schema_field_action(
                "add_field",
                struct_name,
                field_name,
                current_field,
            ));
            continue;
        };
        if applied_field.get("type") != current_field.get("type")
            || applied_field.get("optional") != current_field.get("optional")
        {
            let mut action =
                schema_field_action("change_field", struct_name, field_name, current_field);
            action["from"] = applied_field.clone();
            actions.push(action);
        }
    }
    for field_name in applied_fields.keys() {
        if !current_fields.contains_key(field_name) {
            actions.push(serde_json::json!({
                "kind": "drop_field",
                "struct": struct_name,
                "field": field_name,
            }));
        }
    }
}

fn schema_fields(value: &serde_json::Value) -> Option<&serde_json::Map<String, serde_json::Value>> {
    value.get("fields").and_then(serde_json::Value::as_object)
}

fn schema_field_action(
    kind: &str,
    struct_name: &str,
    field_name: &str,
    field: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "kind": kind,
        "struct": struct_name,
        "field": field_name,
        "type": field.get("type").cloned().unwrap_or(serde_json::Value::Null),
        "optional": field.get("optional").cloned().unwrap_or(serde_json::Value::Bool(false)),
    })
}

fn type_ref_string(ty: &TypeRef) -> String {
    let mut base = match &ty.kind {
        TypeRefKind::Named(id) => id.name.clone(),
        TypeRefKind::Nullable(inner) => format!("{}?", type_ref_string(inner)),
        TypeRefKind::Array(inner) => format!("{}[]", type_ref_string(inner)),
        TypeRefKind::Pattern(pattern) => format!("\"{pattern}\""),
        TypeRefKind::Union(items) => items
            .iter()
            .map(type_ref_string)
            .collect::<Vec<_>>()
            .join(" | "),
        TypeRefKind::InlineObject(fields) => {
            let fields = fields
                .iter()
                .map(|(name, ty)| format!("{}: {}", name.name, type_ref_string(ty)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{fields}}}")
        }
        TypeRefKind::Tuple(items) => {
            let items = items
                .iter()
                .map(type_ref_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("({items})")
        }
    };
    if !ty.constraints.is_empty() {
        base.push('(');
        base.push_str(
            &ty.constraints
                .iter()
                .map(type_constraint_string)
                .collect::<Vec<_>>()
                .join(", "),
        );
        base.push(')');
    }
    base
}

fn type_ref_optional(ty: &TypeRef) -> bool {
    matches!(ty.kind, TypeRefKind::Nullable(_))
}

fn type_constraint_string(constraint: &TypeConstraint) -> String {
    match constraint {
        TypeConstraint::Flag(name) => name.clone(),
        TypeConstraint::ExactInt(value) => value.to_string(),
        TypeConstraint::Range {
            start,
            end,
            inclusive,
        } => {
            let sep = if *inclusive { "..=" } else { ".." };
            format!(
                "{}{sep}{}",
                start.map_or_else(String::new, |value| value.to_string()),
                end.map_or_else(String::new, |value| value.to_string())
            )
        }
        TypeConstraint::KeyValue { key, value } => {
            format!("{key}={}", constraint_value_string(value))
        }
    }
}

fn constraint_value_string(value: &ConstraintValue) -> String {
    match value {
        ConstraintValue::Int(value) => value.to_string(),
        ConstraintValue::String(value) => format!("\"{value}\""),
        ConstraintValue::Bool(value) => value.to_string(),
        ConstraintValue::Ident(value) => value.clone(),
    }
}

fn render_static_page(lowered: &orv_analyzer::LowerResult) -> anyhow::Result<String> {
    let mut out = Vec::new();
    let program = static_page_render_program(&lowered.program);
    orv_runtime::run_with_writer(&program, &mut out).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut html = String::from_utf8(out).map_err(|e| anyhow::anyhow!("html is not utf-8: {e}"))?;
    if html.ends_with('\n') {
        html.pop();
        if html.ends_with('\r') {
            html.pop();
        }
    }
    Ok(html)
}

fn static_page_render_program(program: &orv_hir::HirProgram) -> orv_hir::HirProgram {
    orv_hir::HirProgram {
        items: program
            .items
            .iter()
            .filter(|stmt| !is_top_level_server_stmt(stmt))
            .cloned()
            .collect(),
        span: program.span,
    }
}

const fn is_top_level_server_stmt(stmt: &orv_hir::HirStmt) -> bool {
    matches!(
        stmt,
        orv_hir::HirStmt::Expr(orv_hir::HirExpr {
            kind: orv_hir::HirExprKind::Server { .. },
            ..
        })
    )
}

fn write_text(path: &Path, text: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, text)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))?;
    Ok(())
}

fn write_json(path: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    std::fs::write(path, bytes)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))?;
    Ok(())
}

fn project_graph_json_for_path(path: &Path) -> anyhow::Result<serde_json::Value> {
    let loaded = orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    report_diagnostics(&loaded.diagnostics, &loaded.files)?;
    let resolved = orv_resolve::resolve(&loaded.program);
    report_diagnostics(&resolved.diagnostics, &loaded.files)?;
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    report_diagnostics(&lowered.diagnostics, &loaded.files)?;
    let origin_map = orv_compiler::origin_map(&lowered.program);
    Ok(project_graph_json(&loaded.graph, &origin_map))
}

fn project_graph_json(
    graph: &ProjectGraph,
    origin_map: &orv_compiler::OriginMap,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "stats": project_graph_stats(graph, origin_map),
        "nodes": graph.nodes.iter().map(|node| {
            serde_json::json!({
                "id": node.id,
                "kind": node_kind(node.kind),
                "name": node.name,
                "file": node.file.0,
                "span": span_json(node.span),
            })
        }).collect::<Vec<_>>(),
        "edges": graph.edges.iter().map(|edge| {
            serde_json::json!({
                "from": edge.from,
                "to": edge.to,
                "kind": edge_kind(edge.kind),
            })
        }).collect::<Vec<_>>(),
        "semantic": {
            "origin_map": origin_map,
            "origin_edges": origin_edges(origin_map),
            "origin_links": origin_links(graph, origin_map),
        },
    })
}

fn project_graph_stats(
    graph: &ProjectGraph,
    origin_map: &orv_compiler::OriginMap,
) -> serde_json::Value {
    let file_count = graph
        .nodes
        .iter()
        .filter(|node| node.kind == ProjectNodeKind::File)
        .count();
    let import_count = graph
        .nodes
        .iter()
        .filter(|node| node.kind == ProjectNodeKind::Import)
        .count();
    let domain_count = graph
        .nodes
        .iter()
        .filter(|node| node.kind == ProjectNodeKind::Domain)
        .count();
    let declaration_count = graph
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind,
                ProjectNodeKind::Struct
                    | ProjectNodeKind::Enum
                    | ProjectNodeKind::TypeAlias
                    | ProjectNodeKind::Function
                    | ProjectNodeKind::Define
            )
        })
        .count();
    let semantic_call_edge_count = origin_map
        .edges
        .iter()
        .filter(|edge| edge.kind == "calls")
        .count();

    serde_json::json!({
        "node_count": graph.nodes.len(),
        "edge_count": graph.edges.len(),
        "file_count": file_count,
        "import_count": import_count,
        "declaration_count": declaration_count,
        "domain_count": domain_count,
        "max_source_contains_depth": max_project_contains_depth(graph),
        "semantic_origin_count": origin_map.entries.len(),
        "semantic_edge_count": origin_map.edges.len(),
        "semantic_call_edge_count": semantic_call_edge_count,
        "max_semantic_contains_depth": max_origin_contains_depth(origin_map),
    })
}

fn max_project_contains_depth(graph: &ProjectGraph) -> usize {
    let mut children: HashMap<ProjectNodeId, Vec<ProjectNodeId>> = HashMap::new();
    for edge in graph
        .edges
        .iter()
        .filter(|edge| edge.kind == ProjectEdgeKind::Contains)
    {
        children.entry(edge.from).or_default().push(edge.to);
    }
    let mut memo = HashMap::new();
    graph
        .nodes
        .iter()
        .map(|node| project_contains_depth(node.id, &children, &mut memo, &mut Vec::new()))
        .max()
        .unwrap_or(0)
}

fn project_contains_depth(
    node: ProjectNodeId,
    children: &HashMap<ProjectNodeId, Vec<ProjectNodeId>>,
    memo: &mut HashMap<ProjectNodeId, usize>,
    visiting: &mut Vec<ProjectNodeId>,
) -> usize {
    if let Some(depth) = memo.get(&node) {
        return *depth;
    }
    if visiting.contains(&node) {
        return 0;
    }
    visiting.push(node);
    let depth = children.get(&node).map_or(0, |child_nodes| {
        child_nodes
            .iter()
            .map(|child| 1 + project_contains_depth(*child, children, memo, visiting))
            .max()
            .unwrap_or(0)
    });
    visiting.pop();
    memo.insert(node, depth);
    depth
}

fn max_origin_contains_depth(origin_map: &orv_compiler::OriginMap) -> usize {
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in origin_map
        .edges
        .iter()
        .filter(|edge| edge.kind == "contains")
    {
        children
            .entry(edge.from.as_str())
            .or_default()
            .push(edge.to.as_str());
    }
    let mut memo = HashMap::new();
    origin_map
        .entries
        .iter()
        .map(|entry| origin_contains_depth(&entry.id, &children, &mut memo, &mut Vec::new()))
        .max()
        .unwrap_or(0)
}

fn origin_contains_depth<'a>(
    node: &'a str,
    children: &HashMap<&'a str, Vec<&'a str>>,
    memo: &mut HashMap<&'a str, usize>,
    visiting: &mut Vec<&'a str>,
) -> usize {
    if let Some(depth) = memo.get(node) {
        return *depth;
    }
    if visiting.contains(&node) {
        return 0;
    }
    visiting.push(node);
    let depth = children.get(node).map_or(0, |child_nodes| {
        child_nodes
            .iter()
            .map(|child| 1 + origin_contains_depth(child, children, memo, visiting))
            .max()
            .unwrap_or(0)
    });
    visiting.pop();
    memo.insert(node, depth);
    depth
}

fn origin_edges(origin_map: &orv_compiler::OriginMap) -> Vec<serde_json::Value> {
    origin_map
        .edges
        .iter()
        .map(|edge| {
            serde_json::json!({
                "kind": edge.kind,
                "from": edge.from,
                "to": edge.to,
            })
        })
        .collect()
}

fn origin_links(
    graph: &ProjectGraph,
    origin_map: &orv_compiler::OriginMap,
) -> Vec<serde_json::Value> {
    origin_map
        .entries
        .iter()
        .filter_map(|entry| {
            graph
                .nodes
                .iter()
                .find(|node| {
                    node.file.0 == entry.span.file
                        && node.span.range.start == entry.span.start
                        && node.span.range.end == entry.span.end
                })
                .map(|node| {
                    serde_json::json!({
                        "kind": "source_node",
                        "origin_id": entry.id,
                        "node_id": node.id,
                    })
                })
        })
        .collect()
}

fn span_json(span: Span) -> serde_json::Value {
    serde_json::json!({
        "file": span.file.0,
        "start": span.range.start,
        "end": span.range.end,
    })
}

const fn node_kind(kind: ProjectNodeKind) -> &'static str {
    match kind {
        ProjectNodeKind::File => "file",
        ProjectNodeKind::Import => "import",
        ProjectNodeKind::Struct => "struct",
        ProjectNodeKind::Enum => "enum",
        ProjectNodeKind::TypeAlias => "type_alias",
        ProjectNodeKind::Function => "function",
        ProjectNodeKind::Define => "define",
        ProjectNodeKind::Domain => "domain",
    }
}

const fn edge_kind(kind: ProjectEdgeKind) -> &'static str {
    match kind {
        ProjectEdgeKind::Contains => "contains",
        ProjectEdgeKind::Imports => "imports",
    }
}

fn load_checked_hir(path: &Path) -> anyhow::Result<orv_analyzer::LowerResult> {
    // B3: entry 파일에서 시작해 import 를 따라 multi-file 을 하나의 Program 으로
    // 병합한다. import 가 없으면 entry 한 파일만 로드되므로 기존 동작과 동일.
    let loaded = orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    lower_loaded_project(&loaded)
}

fn load_checked_hir_from_sources(
    path: &Path,
    source: &str,
) -> anyhow::Result<orv_analyzer::LowerResult> {
    let sources = orv_test_source_bundle(path, source)?;
    let loaded = orv_project::load_project_from_sources(path, sources)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    lower_loaded_project(&loaded)
}

fn lower_loaded_project(
    loaded: &orv_project::LoadedProject,
) -> anyhow::Result<orv_analyzer::LowerResult> {
    report_diagnostics(&loaded.diagnostics, &loaded.files)?;

    let resolved = orv_resolve::resolve(&loaded.program);
    report_diagnostics(&resolved.diagnostics, &loaded.files)?;

    // B5: 타입 진단도 보고. 에러면 실행 전에 중단.
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    report_diagnostics(&lowered.diagnostics, &loaded.files)?;
    Ok(lowered)
}

fn cmd_dump(path: &Path) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let source = std::fs::read_to_string(&entry)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", entry.display()))?;
    let file_id = FileId(0);
    let lx = orv_syntax::lex(&source, file_id);
    let files = vec![SourceFile {
        id: file_id,
        path: entry,
        source,
    }];
    report_diagnostics(&lx.diagnostics, &files)?;
    let pr = orv_syntax::parse_with_newlines(lx.tokens, file_id, lx.newlines);
    report_diagnostics(&pr.diagnostics, &files)?;
    println!("{:#?}", pr.program);
    Ok(())
}

fn report_diagnostics(
    diags: &[orv_diagnostics::Diagnostic],
    files: &[SourceFile],
) -> anyhow::Result<()> {
    if diags.is_empty() {
        return Ok(());
    }
    let mut writer = codespan_reporting::term::termcolor::StandardStream::stderr(
        codespan_reporting::term::termcolor::ColorChoice::Auto,
    );
    emit_diagnostics(diags, files, &mut writer)?;
    if diags
        .iter()
        .any(|d| matches!(d.severity, orv_diagnostics::Severity::Error))
    {
        anyhow::bail!("aborting due to previous errors");
    }
    Ok(())
}

fn emit_diagnostics<W: WriteColor>(
    diags: &[orv_diagnostics::Diagnostic],
    source_files: &[SourceFile],
    writer: &mut W,
) -> anyhow::Result<()> {
    let mut files = SimpleFiles::new();
    let mut ids = std::collections::HashMap::new();
    for source_file in source_files {
        let id = files.add(
            source_file.path.display().to_string(),
            source_file.source.clone(),
        );
        ids.insert(source_file.id, id);
    }
    let fallback = files.add("<unknown>".to_string(), String::new());

    for d in diags {
        let mut labels = Vec::new();
        if let Some(lbl) = &d.primary {
            let start = lbl.span.range.start as usize;
            let end = lbl.span.range.end as usize;
            labels.push(
                codespan_reporting::diagnostic::Label::primary(
                    file_id(&ids, lbl.span, fallback),
                    start..end,
                )
                .with_message(&lbl.message),
            );
        }
        for sec in &d.secondary {
            let start = sec.span.range.start as usize;
            let end = sec.span.range.end as usize;
            labels.push(
                codespan_reporting::diagnostic::Label::secondary(
                    file_id(&ids, sec.span, fallback),
                    start..end,
                )
                .with_message(&sec.message),
            );
        }
        let severity = match d.severity {
            orv_diagnostics::Severity::Error => codespan_reporting::diagnostic::Severity::Error,
            orv_diagnostics::Severity::Warning => codespan_reporting::diagnostic::Severity::Warning,
            orv_diagnostics::Severity::Note => codespan_reporting::diagnostic::Severity::Note,
            orv_diagnostics::Severity::Help => codespan_reporting::diagnostic::Severity::Help,
        };
        let mut diag = codespan_reporting::diagnostic::Diagnostic::new(severity)
            .with_message(&d.message)
            .with_labels(labels);
        if !d.notes.is_empty() {
            diag = diag.with_notes(d.notes.clone());
        }
        let config = codespan_reporting::term::Config::default();
        codespan_reporting::term::emit(writer, &config, &files, &diag)?;
    }
    Ok(())
}

fn file_id(ids: &std::collections::HashMap<FileId, usize>, span: Span, fallback: usize) -> usize {
    ids.get(&span.file).copied().unwrap_or(fallback)
}

#[cfg(test)]
fn render_diagnostics_for_test(
    diags: &[orv_diagnostics::Diagnostic],
    files: &[SourceFile],
) -> String {
    let mut out = Vec::new();
    {
        let mut writer = codespan_reporting::term::termcolor::NoColor::new(&mut out);
        emit_diagnostics(diags, files, &mut writer).expect("render diagnostics");
    }
    String::from_utf8(out).expect("diagnostics are utf-8")
}

#[cfg(test)]
mod tests;
