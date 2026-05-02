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
//! `orv verify-build <dir>`은 build manifest/plan target 을 검증한다.
//! `orv verify-artifact <file>`은 server runtime artifact 를 검증하고,
//! `orv check-artifact <file>`은 source bundle 을 재분석하며,
//! `orv check-build <dir>`은 build source bundle 을 재분석하며,
//! `orv run-artifact <file>`은 source bundle 을 재수화해 reference runtime 으로 실행한다.
//! `orv run-build <dir>`은 `server/launch.json` 의 reference runner 계약을 실행한다.
//! `orv reveal <dir> <origin-id>`는 build artifact 에서 origin id 를 원본
//! `.orv` span 과 production descriptor 로 되짚는다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use codespan_reporting::files::SimpleFiles;
use codespan_reporting::term::termcolor::WriteColor;
use orv_diagnostics::{FileId, Span};
use orv_project::{ProjectEdgeKind, ProjectGraph, ProjectNodeId, ProjectNodeKind, SourceFile};
use orv_syntax::ast::{
    BinaryOp as AstBinaryOp, Block, ConstraintValue, Expr, ExprKind, FunctionBody, Program, Stmt,
    StringSegment, TypeConstraint, TypeRef, TypeRefKind, UnaryOp as AstUnaryOp,
};

#[derive(Parser)]
#[command(name = "orv", about = "orv language toolchain", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 주어진 `.orv` 파일을 tree-walking 인터프리터로 실행한다 (MVP).
    Run {
        /// 실행할 소스 파일 경로.
        file: PathBuf,
    },
    /// 파싱 및 타입 검사만 수행하고 실행하지 않는다.
    Check {
        /// 검사할 소스 파일 경로.
        file: PathBuf,
    },
    /// 파싱 결과(AST)를 디버그 출력한다.
    Dump {
        /// 대상 파일 경로.
        file: PathBuf,
    },
    /// HIR 기반 origin map을 JSON으로 출력한다.
    Origins {
        /// 대상 파일 경로.
        file: PathBuf,
    },
    /// AST 기반 `ProjectGraph` v1과 HIR origin map을 JSON으로 출력한다.
    Graph {
        /// 대상 파일 경로.
        file: PathBuf,
    },
    /// 빌드 artifact 디렉터리를 생성한다.
    Build {
        /// 대상 파일 경로.
        file: PathBuf,
        /// artifact 출력 디렉터리.
        #[arg(long, short = 'o')]
        out: PathBuf,
        /// 배포용 production profile 산출물을 함께 생성한다.
        #[arg(long)]
        prod: bool,
    },
    /// build artifact 디렉터리의 manifest/plan 산출물을 검증한다.
    VerifyBuild {
        /// 검증할 build artifact 디렉터리.
        dir: PathBuf,
    },
    /// server runtime artifact를 검증한다.
    VerifyArtifact {
        /// 검증할 artifact JSON 경로.
        file: PathBuf,
    },
    /// server runtime artifact source bundle을 재분석한다.
    CheckArtifact {
        /// 검사할 artifact JSON 경로.
        file: PathBuf,
    },
    /// build artifact source bundle을 재분석한다.
    CheckBuild {
        /// 검사할 build artifact 디렉터리.
        dir: PathBuf,
    },
    /// server runtime artifact source bundle을 재수화하고 실행한다.
    RunArtifact {
        /// 실행할 artifact JSON 경로.
        file: PathBuf,
    },
    /// build artifact 디렉터리의 server launcher를 실행한다.
    RunBuild {
        /// 실행할 build artifact 디렉터리.
        dir: PathBuf,
    },
    /// build artifact를 생성/검증한 뒤 reference dev runtime으로 실행한다.
    Dev {
        /// 실행할 소스 파일, orv.toml, 또는 프로젝트 디렉터리.
        #[arg(default_value = ".")]
        file: PathBuf,
        /// dev artifact 출력 디렉터리.
        #[arg(long, short = 'o', default_value = "target/orv-dev")]
        out: PathBuf,
        /// HMR dev session artifact를 출력한다.
        #[arg(long)]
        hmr: bool,
        /// watch dev session artifact를 출력한다.
        #[arg(long)]
        watch: bool,
    },
    /// build artifact 디렉터리에서 origin id를 원본 코드/production descriptor로 reveal한다.
    Reveal {
        /// 검사할 build artifact 디렉터리.
        dir: PathBuf,
        /// reveal 할 origin id.
        origin_id: String,
    },
    /// 새 orv 프로젝트를 생성한다.
    Init {
        /// 생성할 프로젝트 디렉터리.
        dir: PathBuf,
        /// 프로젝트 이름.
        #[arg(long)]
        name: Option<String>,
        /// 생성할 starter template.
        #[arg(long, value_enum, default_value_t = InitTemplate::Basic)]
        template: InitTemplate,
    },
    /// orv 테스트를 실행한다.
    Test {
        /// 테스트를 찾을 파일 또는 디렉터리.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// 이름에 이 문자열을 포함하는 테스트만 선택한다.
        #[arg(long)]
        filter: Option<String>,
        /// 테스트를 실행하지 않고 발견된 테스트 목록만 JSON으로 출력한다.
        #[arg(long)]
        list: bool,
    },
    /// DB schema migration helper commands.
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Editor/LSP helper commands.
    Lsp {
        #[command(subcommand)]
        command: LspCommand,
    },
    /// Debug Adapter Protocol helper commands.
    Dap {
        #[command(subcommand)]
        command: DapCommand,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum InitTemplate {
    Basic,
    Shop,
}

#[derive(Subcommand)]
enum DbCommand {
    /// 현재 struct schema와 적용된 schema snapshot의 migration dry-run plan을 출력한다.
    Plan {
        /// 대상 소스 파일 경로.
        file: PathBuf,
        /// 마지막 적용 schema snapshot JSON 경로.
        #[arg(long)]
        applied: Option<PathBuf>,
    },
    /// 현재 struct schema와 적용된 schema snapshot이 일치하는지 검증한다.
    Verify {
        /// 대상 소스 파일 경로.
        file: PathBuf,
        /// 검증할 적용 schema snapshot JSON 경로.
        #[arg(long)]
        schema: PathBuf,
    },
    /// 현재 struct schema snapshot을 적용된 schema 파일로 저장한다.
    Apply {
        /// 대상 소스 파일 경로.
        file: PathBuf,
        /// 갱신할 적용 schema snapshot JSON 경로.
        #[arg(long)]
        schema: PathBuf,
        /// migration apply 이력을 기록할 JSON 경로.
        #[arg(long)]
        history: Option<PathBuf>,
    },
    /// 현재 struct schema snapshot을 migration workflow로 적용한다.
    Migrate {
        /// 대상 소스 파일 경로.
        file: PathBuf,
        /// 갱신할 적용 schema snapshot JSON 경로.
        #[arg(long)]
        schema: PathBuf,
        /// migration apply 이력을 기록할 JSON 경로.
        #[arg(long)]
        history: Option<PathBuf>,
        /// 함께 변환할 @db.save JSON data snapshot 경로.
        #[arg(long)]
        data: Option<PathBuf>,
    },
    /// 마지막 적용 전 schema snapshot으로 되돌린다.
    Rollback {
        /// 되돌릴 적용 schema snapshot JSON 경로.
        #[arg(long)]
        schema: PathBuf,
        /// 함께 되돌릴 @db.save JSON data snapshot 경로.
        #[arg(long)]
        data: Option<PathBuf>,
    },
    /// @db.save JSON data snapshot을 local backup artifact로 저장한다.
    Backup {
        /// 백업할 @db.save JSON data snapshot 경로.
        #[arg(long)]
        data: PathBuf,
        /// 쓸 backup artifact JSON 경로.
        #[arg(long)]
        out: PathBuf,
    },
    /// local backup artifact에서 @db.save JSON data snapshot을 복원한다.
    Restore {
        /// 읽을 backup artifact JSON 경로.
        #[arg(long)]
        backup: PathBuf,
        /// 복원할 @db.save JSON data snapshot 경로.
        #[arg(long)]
        data: PathBuf,
    },
    /// JSONL WAL을 재생해 @db.save JSON data snapshot으로 복구한다.
    Recover {
        /// 읽을 @db.wal JSONL 경로.
        #[arg(long)]
        wal: Option<PathBuf>,
        /// 읽을 WAL archive manifest JSON 경로.
        #[arg(long)]
        archive: Option<PathBuf>,
        /// 쓸 @db.save JSON data snapshot 경로.
        #[arg(long)]
        out: PathBuf,
        /// 처음 N개 complete WAL record까지만 재생한다.
        #[arg(long)]
        until_record: Option<usize>,
        /// 이 unix millisecond timestamp 이하 WAL record까지만 재생한다.
        #[arg(long)]
        until_unix_ms: Option<u64>,
        /// 이 RFC3339 timestamp 이하 WAL record까지만 재생한다.
        #[arg(long)]
        until_time: Option<String>,
    },
    /// JSONL WAL archive manifest artifact를 작성한다.
    Archive {
        /// 읽을 @db.wal JSONL 경로.
        #[arg(long)]
        wal: PathBuf,
        /// 쓸 WAL archive manifest JSON 경로.
        #[arg(long)]
        out: PathBuf,
        /// WAL/archive manifest를 복사할 archive target URI. 현재 file:// target을 지원한다.
        #[arg(long)]
        target: Option<String>,
    },
    /// migration history JSON을 하나의 squashed action artifact로 압축한다.
    Squash {
        /// 읽을 migration history JSON 경로.
        #[arg(long)]
        history: PathBuf,
        /// 쓸 squashed migration JSON 경로.
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum LspCommand {
    /// 현재 파일의 LSP bootstrap snapshot JSON을 출력한다.
    Snapshot {
        /// 대상 소스 파일 경로.
        file: PathBuf,
    },
    /// build artifact origin id를 LSP location JSON으로 reveal한다.
    Reveal {
        /// 검사할 build artifact 디렉터리.
        dir: PathBuf,
        /// reveal 할 origin id.
        origin_id: String,
    },
    /// stdin/stdout JSON-RPC LSP server bootstrap을 실행한다.
    Serve {
        /// stdin/stdout transport를 사용한다.
        #[arg(long)]
        stdio: bool,
    },
}

#[derive(Subcommand)]
enum DapCommand {
    /// stdin/stdout Debug Adapter Protocol server bootstrap을 실행한다.
    Serve {
        /// stdin/stdout transport를 사용한다.
        #[arg(long)]
        stdio: bool,
    },
}

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
        Command::Graph { file } => match cmd_graph(&file) {
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
        Command::RunArtifact { file } => match cmd_run_artifact(&file) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::RunBuild { dir } => match cmd_run_build(&dir) {
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
        } => match cmd_dev(&file, &out, hmr, watch) {
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
            DbCommand::Restore { backup, data } => match cmd_db_restore(&backup, &data) {
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
            DbCommand::Squash { history, out } => match cmd_db_squash(&history, &out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
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

fn cmd_graph(path: &Path) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let value = project_graph_json_for_path(&entry)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn cmd_lsp_snapshot(path: &Path) -> anyhow::Result<()> {
    let entry = project_entry_path(path)?;
    let value = lsp_snapshot_json(&entry)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn cmd_lsp_reveal(dir: &Path, origin_id: &str) -> anyhow::Result<()> {
    let value = lsp_reveal_json(dir, origin_id)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn cmd_lsp_serve(use_stdio: bool) -> anyhow::Result<()> {
    if !use_stdio {
        anyhow::bail!("lsp serve currently requires --stdio");
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    lsp_serve_stdio_stream(&mut reader, &mut writer)
}

fn cmd_dap_serve(use_stdio: bool) -> anyhow::Result<()> {
    if !use_stdio {
        anyhow::bail!("dap serve currently requires --stdio");
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    dap_serve_stdio_stream(&mut reader, &mut writer)
}

fn cmd_reveal(dir: &Path, origin_id: &str) -> anyhow::Result<()> {
    let value = reveal_origin_json(dir, origin_id)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

const BASIC_INIT_TEMPLATE_SOURCE: &str =
    "@html { @body { @h1 \"Hello from orv\" @p \"Edit src/main.orv\" } }\n";
const SHOP_INIT_TEMPLATE_SOURCE: &str = include_str!("../../../fixtures/e2e/shopping_mall.orv");

fn cmd_init(dir: &Path, name: Option<&str>, template: InitTemplate) -> anyhow::Result<()> {
    let project_name = name
        .map(str::to_string)
        .or_else(|| {
            dir.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_string)
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "orv-app".to_string());
    let src = dir.join("src");
    std::fs::create_dir_all(&src)
        .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", src.display()))?;
    write_new_text_file(
        &dir.join("orv.toml"),
        &format!(
            "[project]\nname = \"{}\"\nversion = \"0.1.0\"\nentry = \"src/main.orv\"\n",
            escape_toml_string(&project_name)
        ),
    )?;
    let entry_source = match template {
        InitTemplate::Basic => BASIC_INIT_TEMPLATE_SOURCE,
        InitTemplate::Shop => SHOP_INIT_TEMPLATE_SOURCE,
    };
    write_new_text_file(&src.join("main.orv"), entry_source)?;
    if template == InitTemplate::Shop {
        write_new_text_file(&dir.join("README.md"), &shop_init_readme(&project_name))?;
    }
    println!("init: {} created", dir.display());
    Ok(())
}

fn shop_init_readme(project_name: &str) -> String {
    format!(
        "# {project_name}\n\
\n\
Generated ORV shop starter.\n\
\n\
## Verify\n\
\n\
```sh\n\
orv check .\n\
orv build . --prod --out dist\n\
orv verify-build dist\n\
```\n\
\n\
## Run\n\
\n\
```sh\n\
orv run-build dist\n\
```\n\
\n\
## Deploy artifacts\n\
\n\
- `deploy/manifest.json`\n\
- `deploy/container.json`\n\
- `deploy/Dockerfile`\n\
- `deploy/routes.json`\n\
- `deploy/server.sh`\n\
\n\
## Routes\n\
\n\
- `GET /health`\n\
- `POST /products`\n\
- `GET /products`\n\
- `GET /products/:sku`\n\
- `POST /members`\n\
- `GET /members/:handle`\n\
- `POST /orders`\n\
- `GET /orders/:customer`\n\
- `POST /payments`\n\
- `POST /shipments`\n\
- `GET /shipments/:orderId`\n"
    )
}

#[derive(Debug)]
struct OrvTestSummary {
    selected: usize,
    passed: usize,
    failed: usize,
    files: Vec<PathBuf>,
}

#[derive(Debug)]
struct OrvTestCase {
    file: PathBuf,
    name: String,
}

fn cmd_test(path: &Path, filter: Option<&str>, list: bool) -> anyhow::Result<()> {
    if list {
        let value = orv_test_list_json(path, filter)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    let summary = orv_test_summary(path, filter)?;
    println!("test: {} passed", summary.passed);
    Ok(())
}

fn orv_test_list_json(path: &Path, filter: Option<&str>) -> anyhow::Result<serde_json::Value> {
    let tests = orv_test_cases(path, filter)?
        .into_iter()
        .map(|case| {
            serde_json::json!({
                "path": case.file.display().to_string(),
                "name": case.name,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "schema_version": 1,
        "tests": tests,
    }))
}

fn orv_test_cases(path: &Path, filter: Option<&str>) -> anyhow::Result<Vec<OrvTestCase>> {
    let files = orv_test_candidate_files(path)?;
    let mut cases = Vec::new();
    for file in files {
        let source = std::fs::read_to_string(&file)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", file.display()))?;
        for name in orv_test_names(&source) {
            if filter.is_none_or(|filter| name.contains(filter)) {
                cases.push(OrvTestCase {
                    file: file.clone(),
                    name,
                });
            }
        }
    }
    Ok(cases)
}

fn orv_test_summary(path: &Path, filter: Option<&str>) -> anyhow::Result<OrvTestSummary> {
    let files = orv_test_candidate_files(path)?;
    let mut summary = OrvTestSummary {
        selected: 0,
        passed: 0,
        failed: 0,
        files: Vec::new(),
    };
    for file in files {
        let source = std::fs::read_to_string(&file)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", file.display()))?;
        let names = orv_test_names(&source);
        let selected = names
            .iter()
            .filter(|name| filter.is_none_or(|filter| name.contains(filter)))
            .count();
        if selected == 0 {
            continue;
        }
        summary.selected += selected;
        summary.files.push(file.clone());
        let lowered = load_checked_hir(&file)?;
        let mut output = Vec::new();
        if let Err(err) = orv_runtime::run_with_writer(&lowered.program, &mut output) {
            summary.failed += selected;
            anyhow::bail!("test: {} failed: {err}", file.display());
        }
        summary.passed += selected;
    }
    Ok(summary)
}

fn orv_test_candidate_files(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if path.is_dir() {
        let mut files = Vec::new();
        collect_orv_files(path, &mut files)?;
        files.sort();
        return Ok(files);
    }
    let file = project_entry_path(path)?;
    if is_orv_file(&file) {
        return Ok(vec![file]);
    }
    anyhow::bail!("test path must be a .orv file, orv.toml, or directory")
}

fn collect_orv_files(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", dir.display()))?
    {
        let entry = entry.map_err(|e| anyhow::anyhow!("failed to read dir entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_orv_files(&path, out)?;
        } else if is_orv_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_orv_file(path: &Path) -> bool {
    path.extension().and_then(std::ffi::OsStr::to_str) == Some("orv")
}

fn orv_test_names(source: &str) -> Vec<String> {
    let lexed = orv_syntax::lex(source, FileId(0));
    let mut names = Vec::new();
    for window in lexed.tokens.windows(2) {
        let [head, tail] = window else {
            continue;
        };
        if matches!(&head.kind, orv_syntax::TokenKind::Ident(name) if name == "test") {
            if let orv_syntax::TokenKind::String(name) = &tail.kind {
                names.push(name.clone());
            }
        }
    }
    names
}

fn cmd_db_plan(path: &Path, applied: Option<&Path>) -> anyhow::Result<()> {
    let value = db_plan_json(path, applied)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn cmd_db_verify(path: &Path, schema: &Path) -> anyhow::Result<()> {
    let plan = db_plan_json(path, Some(schema))?;
    let actions = plan
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("db plan actions must be an array"))?;
    if !actions.is_empty() {
        anyhow::bail!("db schema drift: {} action(s)", actions.len());
    }
    println!("db schema: {} verified", schema.display());
    Ok(())
}

fn cmd_db_apply(path: &Path, schema: &Path) -> anyhow::Result<()> {
    cmd_db_apply_with_history(path, schema, None)
}

fn cmd_db_migrate(path: &Path, schema: &Path, history: Option<&Path>) -> anyhow::Result<()> {
    cmd_db_apply_with_history(path, schema, history)
}

fn cmd_db_migrate_with_data(
    path: &Path,
    schema: &Path,
    history: Option<&Path>,
    data: Option<&Path>,
) -> anyhow::Result<()> {
    cmd_db_apply_with_data(path, schema, history, data)
}

fn cmd_db_apply_with_history(
    path: &Path,
    schema: &Path,
    history: Option<&Path>,
) -> anyhow::Result<()> {
    cmd_db_apply_with_data(path, schema, history, None)
}

fn cmd_db_apply_with_data(
    path: &Path,
    schema: &Path,
    history: Option<&Path>,
    data: Option<&Path>,
) -> anyhow::Result<()> {
    let snapshot = current_db_schema_snapshot(path)?;
    let previous = if schema.is_file() {
        read_json_value(schema)?
    } else {
        empty_db_schema_snapshot()
    };
    let actions = db_schema_diff_actions(&previous, &snapshot);
    let migrated_data = if let Some(data) = data {
        Some(migrated_db_data_snapshot(data, &actions)?)
    } else {
        None
    };
    backup_schema_for_rollback(schema)?;
    if let Some(data) = data {
        backup_json_for_rollback(data)?;
    }
    write_json_atomic(schema, &snapshot)?;
    if let (Some(data), Some(migrated_data)) = (data, migrated_data.as_ref()) {
        write_json_atomic(data, migrated_data)?;
        println!("db data: {} migrated", data.display());
    }
    if let Some(history) = history {
        append_db_history(history, path, &snapshot, actions)?;
    }
    println!("db schema: {} applied", schema.display());
    Ok(())
}

fn cmd_db_rollback(schema: &Path) -> anyhow::Result<()> {
    cmd_db_rollback_with_data(schema, None)
}

fn cmd_db_rollback_with_data(schema: &Path, data: Option<&Path>) -> anyhow::Result<()> {
    let rollback = rollback_schema_path(schema);
    if !rollback.is_file() {
        anyhow::bail!("no rollback schema snapshot at {}", rollback.display());
    }
    let snapshot = read_json_value(&rollback)?;
    let data_snapshot = if let Some(data) = data {
        let rollback = rollback_schema_path(data);
        if !rollback.is_file() {
            anyhow::bail!("no rollback data snapshot at {}", rollback.display());
        }
        let snapshot = read_json_value(&rollback)?;
        Some((data, rollback, snapshot))
    } else {
        None
    };
    write_json_atomic(schema, &snapshot)?;
    std::fs::remove_file(&rollback)
        .map_err(|e| anyhow::anyhow!("failed to remove {}: {e}", rollback.display()))?;
    if let Some((data, rollback, snapshot)) = data_snapshot {
        write_json_atomic(data, &snapshot)?;
        std::fs::remove_file(&rollback)
            .map_err(|e| anyhow::anyhow!("failed to remove {}: {e}", rollback.display()))?;
        println!("db data: {} rolled back", data.display());
    }
    println!("db schema: {} rolled back", schema.display());
    Ok(())
}

fn backup_schema_for_rollback(schema: &Path) -> anyhow::Result<()> {
    backup_json_for_rollback(schema)
}

fn backup_json_for_rollback(path: &Path) -> anyhow::Result<()> {
    if path.is_file() {
        let current = read_json_value(path)?;
        write_json_atomic(&rollback_schema_path(path), &current)?;
    }
    Ok(())
}

fn cmd_db_backup(data: &Path, out: &Path) -> anyhow::Result<()> {
    let snapshot = read_json_value(data)?;
    validate_db_data_snapshot(&snapshot)?;
    let backup = serde_json::json!({
        "schema_version": 1,
        "source": data.display().to_string(),
        "data_hash": stable_json_hash(&snapshot)?,
        "data": snapshot,
    });
    write_json_atomic(out, &backup)?;
    println!("db backup: {} written", out.display());
    Ok(())
}

fn cmd_db_restore(backup: &Path, data: &Path) -> anyhow::Result<()> {
    let backup = read_json_value(backup)?;
    let version = backup
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("db backup schema_version must be an integer"))?;
    if version != 1 {
        anyhow::bail!("unsupported db backup schema_version {version}");
    }
    let snapshot = backup
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("db backup data snapshot is missing"))?;
    validate_db_data_snapshot(snapshot)?;
    backup_json_for_rollback(data)?;
    write_json_atomic(data, snapshot)?;
    println!("db data: {} restored", data.display());
    Ok(())
}

fn cmd_db_recover(
    wal: &Path,
    out: &Path,
    until_record: Option<usize>,
    until_unix_ms: Option<u64>,
    until_time: Option<&str>,
) -> anyhow::Result<()> {
    let cutoff_count = usize::from(until_record.is_some())
        + usize::from(until_unix_ms.is_some())
        + usize::from(until_time.is_some());
    if cutoff_count > 1 {
        anyhow::bail!(
            "db recover accepts only one of --until-record, --until-unix-ms, or --until-time"
        );
    }
    let until_time_unix_ms = until_time.map(parse_db_recover_time_unix_ms).transpose()?;
    let timestamp_limit = until_unix_ms.or(until_time_unix_ms);
    let db = if timestamp_limit.is_some() {
        orv_runtime::db::InMemoryDb::load_wal_until_unix_ms(wal, timestamp_limit)
    } else {
        orv_runtime::db::InMemoryDb::load_wal_until_record(wal, until_record)
    }
    .map_err(|e| anyhow::anyhow!("db wal recover failed: {e}"))?;
    let snapshot = db.snapshot_json();
    validate_db_data_snapshot(&snapshot)?;
    backup_json_for_rollback(out)?;
    write_json_atomic(out, &snapshot)?;
    match (until_record, until_unix_ms, until_time) {
        (Some(limit), None, None) => println!(
            "db recover: {} written from {} through record {}",
            out.display(),
            wal.display(),
            limit
        ),
        (None, Some(limit), None) => println!(
            "db recover: {} written from {} through unix ms {}",
            out.display(),
            wal.display(),
            limit
        ),
        (None, None, Some(limit)) => println!(
            "db recover: {} written from {} through time {}",
            out.display(),
            wal.display(),
            limit
        ),
        (None, None, None) => println!(
            "db recover: {} written from {}",
            out.display(),
            wal.display()
        ),
        _ => unreachable!("validated mutually exclusive recover limits"),
    }
    Ok(())
}

fn cmd_db_recover_from_inputs(
    wal: Option<&Path>,
    archive: Option<&Path>,
    out: &Path,
    until_record: Option<usize>,
    until_unix_ms: Option<u64>,
    until_time: Option<&str>,
) -> anyhow::Result<()> {
    match (wal, archive) {
        (Some(wal), None) => cmd_db_recover(wal, out, until_record, until_unix_ms, until_time),
        (None, Some(archive)) => {
            let wal = db_archive_manifest_wal_path(archive)?;
            cmd_db_recover(&wal, out, until_record, until_unix_ms, until_time)
        }
        (Some(_), Some(_)) => anyhow::bail!("db recover accepts only one of --wal or --archive"),
        (None, None) => anyhow::bail!("db recover requires --wal or --archive"),
    }
}

fn cmd_db_archive(wal: &Path, out: &Path, target: Option<&str>) -> anyhow::Result<()> {
    let mut manifest = db_wal_archive_manifest(wal)?;
    let archive_target = target
        .map(|target| db_archive_file_target(target, wal, out))
        .transpose()?;
    if let Some(target) = &archive_target {
        manifest["target"] = db_archive_file_target_json(target);
    }
    write_json_atomic(out, &manifest)?;
    if let Some(target) = &archive_target {
        copy_db_archive_to_file_target(wal, out, target)?;
    }
    println!(
        "db archive: {} written from {}",
        out.display(),
        wal.display()
    );
    Ok(())
}

fn db_wal_archive_manifest(wal: &Path) -> anyhow::Result<serde_json::Value> {
    let source = std::fs::read_to_string(wal)
        .map_err(|e| anyhow::anyhow!("failed to read WAL {}: {e}", wal.display()))?;
    let lines = source.lines().collect::<Vec<_>>();
    let has_complete_tail = source.ends_with('\n');
    let mut records = Vec::new();
    let mut first_ts_unix_ms = None;
    let mut last_ts_unix_ms = None;
    for (line_index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: serde_json::Value = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(source)
                if line_index + 1 == lines.len() && !has_complete_tail && source.is_eof() =>
            {
                break;
            }
            Err(source) => {
                return Err(anyhow::anyhow!(
                    "failed to parse WAL {} line {}: {source}",
                    wal.display(),
                    line_index + 1
                ));
            }
        };
        let record_number = records.len() + 1;
        let timestamp = record.get("ts_unix_ms").and_then(serde_json::Value::as_u64);
        if let Some(timestamp) = timestamp {
            first_ts_unix_ms.get_or_insert(timestamp);
            last_ts_unix_ms = Some(timestamp);
        }
        let mut item = serde_json::Map::new();
        item.insert(
            "record".to_string(),
            serde_json::Value::from(u64::try_from(record_number).unwrap_or(u64::MAX)),
        );
        if let Some(timestamp) = timestamp {
            item.insert("ts_unix_ms".to_string(), serde_json::Value::from(timestamp));
        }
        records.push(serde_json::Value::Object(item));
    }
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.db.wal_archive",
        "wal": {
            "path": wal.display().to_string(),
            "hash": format!("fnv1a64:{:016x}", fnv1a64(source.as_bytes())),
            "byte_count": source.len(),
            "record_count": records.len(),
            "first_ts_unix_ms": first_ts_unix_ms,
            "last_ts_unix_ms": last_ts_unix_ms,
        },
        "records": records,
    }))
}

fn db_archive_manifest_wal_path(archive: &Path) -> anyhow::Result<PathBuf> {
    let manifest = read_json_value(archive)?;
    if manifest
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("db archive schema_version must be 1");
    }
    if manifest.get("kind").and_then(serde_json::Value::as_str) != Some("orv.db.wal_archive") {
        anyhow::bail!("db archive kind must be orv.db.wal_archive");
    }
    if let Some(target_path) = manifest
        .pointer("/target/wal/path")
        .and_then(serde_json::Value::as_str)
    {
        let wal_path = lsp_file_uri_path(target_path)?;
        verify_db_archive_wal(&manifest, &wal_path)?;
        return Ok(wal_path);
    }
    let wal_path = manifest
        .pointer("/wal/path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("db archive wal.path must be a string"))?;
    let wal_path = PathBuf::from(wal_path);
    verify_db_archive_wal(&manifest, &wal_path)?;
    Ok(wal_path)
}

fn verify_db_archive_wal(manifest: &serde_json::Value, wal: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(wal)
        .map_err(|e| anyhow::anyhow!("failed to read WAL {}: {e}", wal.display()))?;
    let expected_hash = manifest
        .pointer("/wal/hash")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("db archive wal.hash must be a string"))?;
    let actual_hash = format!("fnv1a64:{:016x}", fnv1a64(&bytes));
    if actual_hash != expected_hash {
        anyhow::bail!("db archive WAL hash mismatch for {}", wal.display());
    }
    let expected_bytes = manifest
        .pointer("/wal/byte_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("db archive wal.byte_count must be a number"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != expected_bytes {
        anyhow::bail!("db archive WAL byte count mismatch for {}", wal.display());
    }
    Ok(())
}

struct DbArchiveFileTarget {
    uri: String,
    wal_path: PathBuf,
    manifest_path: PathBuf,
}

fn db_archive_file_target(
    target: &str,
    wal: &Path,
    manifest: &Path,
) -> anyhow::Result<DbArchiveFileTarget> {
    if !target.starts_with("file://") {
        anyhow::bail!("unsupported db archive target `{target}`");
    }
    let target_dir = lsp_file_uri_path(target)?;
    let wal_name = wal
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("WAL path must include a file name"))?;
    let manifest_name = manifest
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("archive manifest path must include a file name"))?;
    Ok(DbArchiveFileTarget {
        uri: target.to_string(),
        wal_path: target_dir.join(wal_name),
        manifest_path: target_dir.join(manifest_name),
    })
}

fn db_archive_file_target_json(target: &DbArchiveFileTarget) -> serde_json::Value {
    serde_json::json!({
        "kind": "file",
        "uri": target.uri.clone(),
        "wal": {
            "path": lsp_file_uri_for_path(&target.wal_path),
        },
        "manifest": {
            "path": lsp_file_uri_for_path(&target.manifest_path),
        },
    })
}

fn copy_db_archive_to_file_target(
    wal: &Path,
    manifest: &Path,
    target: &DbArchiveFileTarget,
) -> anyhow::Result<()> {
    if let Some(parent) = target.wal_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!("failed to create archive target {}: {e}", parent.display())
        })?;
    }
    std::fs::copy(wal, &target.wal_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to copy WAL to archive target {}: {e}",
            target.wal_path.display()
        )
    })?;
    std::fs::copy(manifest, &target.manifest_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to copy archive manifest to target {}: {e}",
            target.manifest_path.display()
        )
    })?;
    Ok(())
}

fn parse_db_recover_time_unix_ms(input: &str) -> anyhow::Result<u64> {
    let bytes = input.as_bytes();
    if bytes.len() < 20 {
        anyhow::bail!("--until-time must be an RFC3339 timestamp like 2026-05-02T12:00:00Z");
    }
    expect_time_byte(bytes, 4, b'-')?;
    expect_time_byte(bytes, 7, b'-')?;
    if !matches!(bytes.get(10), Some(b'T' | b't')) {
        anyhow::bail!("--until-time must separate date and time with `T`");
    }
    expect_time_byte(bytes, 13, b':')?;
    expect_time_byte(bytes, 16, b':')?;

    let year = i64::from(parse_time_digits(bytes, 0, 4, "year")?);
    let month = parse_time_digits(bytes, 5, 7, "month")?;
    let day = parse_time_digits(bytes, 8, 10, "day")?;
    let hour = parse_time_digits(bytes, 11, 13, "hour")?;
    let minute = parse_time_digits(bytes, 14, 16, "minute")?;
    let second = parse_time_digits(bytes, 17, 19, "second")?;
    validate_recover_time_parts(year, month, day, hour, minute, second)?;

    let mut index = 19usize;
    let mut millisecond = 0u32;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(|byte| byte.is_ascii_digit()) {
            if index - fraction_start < 3 {
                millisecond = millisecond
                    .saturating_mul(10)
                    .saturating_add(u32::from(bytes[index] - b'0'));
            }
            index += 1;
        }
        let fraction_digits = index.saturating_sub(fraction_start);
        if fraction_digits == 0 {
            anyhow::bail!("--until-time fractional seconds must contain digits");
        }
        for _ in fraction_digits..3 {
            millisecond = millisecond.saturating_mul(10);
        }
    }

    let offset_seconds = parse_recover_time_offset(bytes, index)?;
    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(i64::from(hour) * 3_600))
        .and_then(|value| value.checked_add(i64::from(minute) * 60))
        .and_then(|value| value.checked_add(i64::from(second)))
        .and_then(|value| value.checked_sub(offset_seconds))
        .ok_or_else(|| anyhow::anyhow!("--until-time is out of supported range"))?;
    let unix_ms = seconds
        .checked_mul(1_000)
        .and_then(|value| value.checked_add(i64::from(millisecond)))
        .ok_or_else(|| anyhow::anyhow!("--until-time is out of supported range"))?;
    if unix_ms < 0 {
        anyhow::bail!("--until-time must not be before the Unix epoch");
    }
    Ok(u64::try_from(unix_ms).unwrap_or(u64::MAX))
}

fn parse_time_digits(bytes: &[u8], start: usize, end: usize, label: &str) -> anyhow::Result<u32> {
    let Some(slice) = bytes.get(start..end) else {
        anyhow::bail!("--until-time is missing {label}");
    };
    let mut value = 0u32;
    for byte in slice {
        if !byte.is_ascii_digit() {
            anyhow::bail!("--until-time has invalid {label}");
        }
        value = value
            .saturating_mul(10)
            .saturating_add(u32::from(byte - b'0'));
    }
    Ok(value)
}

fn expect_time_byte(bytes: &[u8], index: usize, expected: u8) -> anyhow::Result<()> {
    if bytes.get(index) != Some(&expected) {
        anyhow::bail!("--until-time must be an RFC3339 timestamp like 2026-05-02T12:00:00Z");
    }
    Ok(())
}

fn validate_recover_time_parts(
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> anyhow::Result<()> {
    if !(1..=12).contains(&month) {
        anyhow::bail!("--until-time month is out of range");
    }
    if day == 0 || day > days_in_month(year, month) {
        anyhow::bail!("--until-time day is out of range");
    }
    if hour > 23 || minute > 59 || second > 59 {
        anyhow::bail!("--until-time clock is out of range");
    }
    Ok(())
}

fn parse_recover_time_offset(bytes: &[u8], index: usize) -> anyhow::Result<i64> {
    match bytes.get(index) {
        Some(b'Z' | b'z') if index + 1 == bytes.len() => Ok(0),
        Some(sign @ (b'+' | b'-')) => {
            if index + 6 != bytes.len() {
                anyhow::bail!("--until-time timezone offset must use HH:MM");
            }
            expect_time_byte(bytes, index + 3, b':')?;
            let hour = parse_time_digits(bytes, index + 1, index + 3, "timezone hour")?;
            let minute = parse_time_digits(bytes, index + 4, index + 6, "timezone minute")?;
            if hour > 23 || minute > 59 {
                anyhow::bail!("--until-time timezone offset is out of range");
            }
            let offset = i64::from(hour) * 3_600 + i64::from(minute) * 60;
            if *sign == b'+' {
                Ok(offset)
            } else {
                Ok(-offset)
            }
        }
        _ => anyhow::bail!("--until-time must end with `Z` or a timezone offset"),
    }
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

const fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn cmd_db_squash(history: &Path, out: &Path) -> anyhow::Result<()> {
    let history_value = read_json_value(history)?;
    let entries = history_value
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("db history entries must be an array"))?;
    let mut actions = Vec::new();
    for entry in entries {
        let entry_actions = entry
            .get("actions")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("db history entry actions must be an array"))?;
        actions.extend(entry_actions.iter().cloned());
    }
    let schema_hash = entries
        .last()
        .and_then(|entry| entry.get("schema_hash"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let squashed = serde_json::json!({
        "schema_version": 1,
        "source_history": history.display().to_string(),
        "entries": entries.len(),
        "schema_hash": schema_hash,
        "actions": actions,
    });
    write_json_atomic(out, &squashed)?;
    println!("db squash: {} written", out.display());
    Ok(())
}

fn validate_db_data_snapshot(snapshot: &serde_json::Value) -> anyhow::Result<()> {
    snapshot
        .get("tables")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("db data snapshot tables must be an object"))?;
    Ok(())
}

fn migrated_db_data_snapshot(
    data: &Path,
    actions: &[serde_json::Value],
) -> anyhow::Result<serde_json::Value> {
    let mut snapshot = read_json_value(data)?;
    validate_db_data_snapshot(&snapshot)?;
    let tables = snapshot
        .get_mut("tables")
        .and_then(serde_json::Value::as_object_mut)
        .expect("validated db data tables");
    for action in actions {
        let Some(kind) = action.get("kind").and_then(serde_json::Value::as_str) else {
            continue;
        };
        match kind {
            "create_struct" => {
                let struct_name = required_action_string(action, "struct")?;
                tables
                    .entry(struct_name.to_string())
                    .or_insert_with(|| serde_json::json!({ "next_id": 1, "rows": [] }));
            }
            "drop_struct" => {
                let struct_name = required_action_string(action, "struct")?;
                tables.remove(struct_name);
            }
            "add_field" => {
                let struct_name = required_action_string(action, "struct")?;
                let field_name = required_action_string(action, "field")?;
                if let Some(rows) = db_data_rows_mut(tables, struct_name)? {
                    for row in rows {
                        let row = row.as_object_mut().ok_or_else(|| {
                            anyhow::anyhow!("db data row in {struct_name} must be an object")
                        })?;
                        row.entry(field_name.to_string())
                            .or_insert(serde_json::Value::Null);
                    }
                }
            }
            "drop_field" => {
                let struct_name = required_action_string(action, "struct")?;
                let field_name = required_action_string(action, "field")?;
                if let Some(rows) = db_data_rows_mut(tables, struct_name)? {
                    for row in rows {
                        let row = row.as_object_mut().ok_or_else(|| {
                            anyhow::anyhow!("db data row in {struct_name} must be an object")
                        })?;
                        row.remove(field_name);
                    }
                }
            }
            "change_field" => {}
            _ => {}
        }
    }
    Ok(snapshot)
}

fn required_action_string<'a>(action: &'a serde_json::Value, key: &str) -> anyhow::Result<&'a str> {
    action
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("db migration action missing string `{key}`"))
}

fn db_data_rows_mut<'a>(
    tables: &'a mut serde_json::Map<String, serde_json::Value>,
    struct_name: &str,
) -> anyhow::Result<Option<&'a mut Vec<serde_json::Value>>> {
    let Some(table) = tables.get_mut(struct_name) else {
        return Ok(None);
    };
    let rows = table
        .get_mut("rows")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("db data table {struct_name} rows must be an array"))?;
    Ok(Some(rows))
}

fn append_db_history(
    history: &Path,
    source: &Path,
    schema: &serde_json::Value,
    actions: Vec<serde_json::Value>,
) -> anyhow::Result<()> {
    let mut value = if history.is_file() {
        read_json_value(history)?
    } else {
        serde_json::json!({
            "schema_version": 1,
            "entries": [],
        })
    };
    let entries = value
        .get_mut("entries")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("db history entries must be an array"))?;
    entries.push(serde_json::json!({
        "source": source.display().to_string(),
        "schema_hash": stable_json_hash(schema)?,
        "actions": actions,
    }));
    write_json_atomic(history, &value)
}

fn db_plan_json(path: &Path, applied: Option<&Path>) -> anyhow::Result<serde_json::Value> {
    let current_schema = current_db_schema_snapshot(path)?;
    let applied_schema = if let Some(applied) = applied {
        read_json_value(applied)?
    } else {
        empty_db_schema_snapshot()
    };
    let actions = db_schema_diff_actions(&applied_schema, &current_schema);
    Ok(serde_json::json!({
        "schema_version": 1,
        "current_schema": current_schema,
        "actions": actions,
    }))
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

fn lsp_snapshot_json(path: &Path) -> anyhow::Result<serde_json::Value> {
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

fn lsp_reveal_json(dir: &Path, origin_id: &str) -> anyhow::Result<serde_json::Value> {
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
fn lsp_jsonrpc_response(request: &serde_json::Value) -> serde_json::Value {
    LspSession::default().jsonrpc_response(request)
}

#[derive(Default)]
struct LspSession {
    open_documents: HashMap<PathBuf, String>,
    workspace_root: Option<PathBuf>,
}

impl LspSession {
    fn message_response(&mut self, request: &serde_json::Value) -> Option<serde_json::Value> {
        if request.get("id").is_none() {
            self.handle_notification(request);
            return None;
        }
        Some(self.jsonrpc_response(request))
    }

    fn jsonrpc_response(&mut self, request: &serde_json::Value) -> serde_json::Value {
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
            Some("textDocument/documentSymbol") => match self.document_symbol_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/codeLens") => match self.code_lens_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/codeAction") => match self.code_action_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/documentLink") => match self.document_link_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/foldingRange") => match self.folding_range_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/selectionRange") => match self.selection_range_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/semanticTokens/full") => {
                match self.semantic_tokens_result(request) {
                    Ok(result) => lsp_jsonrpc_result(&id, &result),
                    Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
                }
            }
            Some("textDocument/diagnostic") => {
                match self.text_document_diagnostic_result(request) {
                    Ok(result) => lsp_jsonrpc_result(&id, &result),
                    Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
                }
            }
            Some("workspace/diagnostic") => match self.workspace_diagnostic_result() {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("workspace/executeCommand") => match self.execute_command_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/definition") => match self.definition_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/references") => match self.references_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/documentHighlight") => match self.document_highlight_result(request)
            {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/prepareRename") => match self.prepare_rename_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/rename") => match self.rename_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/hover") => match self.hover_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("textDocument/completion") => match self.completion_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some("workspace/symbol") => match self.workspace_symbol_result(request) {
                Ok(result) => lsp_jsonrpc_result(&id, &result),
                Err(err) => lsp_jsonrpc_error(&id, -32602, &err.to_string()),
            },
            Some(method) => lsp_jsonrpc_method_not_found(&id, method),
            None => lsp_jsonrpc_error(&id, -32600, "invalid request"),
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
                    "textDocumentSync": 1,
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
                    "referencesProvider": true,
                    "documentHighlightProvider": true,
                    "renameProvider": {
                        "prepareProvider": true,
                    },
                    "hoverProvider": true,
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

    fn references_result(&self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let uri = lsp_text_document_uri(request)?;
        let path = lsp_file_uri_path(uri)?;
        let position = lsp_text_document_position(request)?;
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::Value::Array(Vec::new()));
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some(name) = identifier_at_byte(&file.source, byte) else {
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
        let Some(name) = identifier_at_byte(&file.source, byte) else {
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
        let Some((start, end, name)) = identifier_span_at_byte(&file.source, byte) else {
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
        if !lsp_valid_identifier_name(new_name) {
            return Err(anyhow::anyhow!("newName must be a valid identifier"));
        }
        let loaded = self.loaded_project_for_path(&path)?;
        let Some(file) = lsp_source_file_for_path(&loaded.files, &path) else {
            return Ok(serde_json::json!({ "changes": {} }));
        };
        let byte = lsp_position_to_byte(&file.source, position);
        let Some((_, _, name)) = identifier_span_at_byte(&file.source, byte) else {
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
        let Some(name) = identifier_at_byte(&file.source, byte) else {
            return Ok(serde_json::Value::Null);
        };
        let Some(node) = lsp_definition_node(&loaded.graph, name) else {
            return Ok(serde_json::Value::Null);
        };
        Ok(lsp_hover_json(node, &loaded.files))
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
        Ok(serde_json::json!({
            "isIncomplete": false,
            "items": lsp_completion_items_json(&loaded.graph),
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

    fn handle_notification(&mut self, request: &serde_json::Value) {
        match request.get("method").and_then(serde_json::Value::as_str) {
            Some("textDocument/didOpen") => self.handle_did_open(request),
            Some("textDocument/didChange") => self.handle_did_change(request),
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
fn dap_protocol_response(request: &serde_json::Value) -> serde_json::Value {
    DapSession::default()
        .message_response(request)
        .expect("DAP response")
}

#[derive(Default)]
struct DapSession {
    next_seq: u64,
    launched: Option<DapLaunchState>,
    breakpoints: HashMap<PathBuf, Vec<DapBreakpoint>>,
    function_breakpoints: Vec<DapFunctionBreakpoint>,
    data_breakpoints: Vec<DapDataBreakpoint>,
    pending_events: Vec<DapPendingEvent>,
}

struct DapLaunchState {
    path: PathBuf,
    uri: String,
    name: String,
    node_count: usize,
    diagnostic_count: usize,
    stopped_line: u64,
    stopped_reason: String,
    executable_lines: Vec<u64>,
    runtime: DapRuntimeState,
    sources: Vec<DapSourceInfo>,
    files: Vec<SourceFile>,
    frames: Vec<DapFrameState>,
    current_frame_index: usize,
    live_requested: bool,
    live: Option<DapLiveState>,
    long_running: bool,
    async_runtime: Option<DapAsyncRuntimeState>,
}

struct DapPendingEvent {
    event: String,
    body: serde_json::Value,
}

struct DapLiveState {
    stepper: orv_runtime::DebugStepper<Vec<u8>>,
}

enum DapLiveAdvance {
    Frame { index: usize, output: String },
    Skipped,
    Done,
    Error { message: String },
}

#[derive(Clone)]
struct DapSourceInfo {
    reference: u64,
    name: String,
    path: PathBuf,
    uri: String,
}

#[derive(Clone)]
struct DapBreakpoint {
    id: u64,
    line: u64,
    verified: bool,
    condition: Option<String>,
    hit_condition: Option<String>,
    message: Option<String>,
}

#[derive(Clone)]
struct DapFunctionBreakpoint {
    id: u64,
    name: String,
    verified: bool,
    message: Option<String>,
}

#[derive(Clone)]
struct DapDataBreakpoint {
    id: u64,
    data_id: String,
    verified: bool,
    message: Option<String>,
}

#[derive(Clone)]
struct DapRuntimeState {
    status: String,
    stdout: String,
    error: String,
}

#[derive(Clone)]
struct DapAsyncRuntimeState {
    kind: String,
    state: String,
    resume_count: u64,
    pause_count: u64,
}

impl DapAsyncRuntimeState {
    fn server() -> Self {
        Self {
            kind: "server".to_string(),
            state: "paused".to_string(),
            resume_count: 0,
            pause_count: 0,
        }
    }
}

#[derive(Clone)]
struct DapVariable {
    name: String,
    value: String,
    value_type: String,
    line: u64,
    variables_reference: u64,
}

#[derive(Clone)]
struct DapFrameState {
    source: DapSourceInfo,
    line: u64,
    locals: Vec<DapVariable>,
    stack: Vec<DapStackFrameState>,
    output: String,
}

#[derive(Clone)]
struct DapStackFrameState {
    name: String,
    source: DapSourceInfo,
    line: u64,
}

#[derive(Clone, Debug, PartialEq)]
enum DapDebugValue {
    Int(i64),
    Float(f64),
    String(String),
    Regex { pattern: String, flags: String },
    Bool(bool),
    Void,
    Array(Vec<Self>),
    Tuple(Vec<Self>),
    Object(Vec<(String, Self)>),
}

impl DapDebugValue {
    fn display_value(&self) -> String {
        match self {
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::String(value) => {
                serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
            }
            Self::Regex { pattern, flags } => format!("r\"{pattern}\"{flags}"),
            Self::Bool(value) => value.to_string(),
            Self::Void => "void".to_string(),
            Self::Array(items) => {
                let items = items
                    .iter()
                    .map(Self::display_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{items}]")
            }
            Self::Tuple(items) => {
                let items = items
                    .iter()
                    .map(Self::display_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({items})")
            }
            Self::Object(fields) => {
                let fields = fields
                    .iter()
                    .map(|(name, value)| format!("{name}: {}", value.display_value()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{ {fields} }}")
            }
        }
    }

    fn value_type(&self) -> String {
        match self {
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::Regex { .. } => "regex",
            Self::Bool(_) => "bool",
            Self::Void => "void",
            Self::Array(_) => "array",
            Self::Tuple(_) => "tuple",
            Self::Object(_) => "object",
        }
        .to_string()
    }
}

impl DapSession {
    fn message_response(&mut self, request: &serde_json::Value) -> Option<serde_json::Value> {
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
            "restart" => self.restart_result(request),
            "configurationDone" => self.configuration_done_result(),
            "setExceptionBreakpoints" => Ok(dap_set_exception_breakpoints_result(request)),
            "setBreakpoints" => self.set_breakpoints_result(request),
            "setFunctionBreakpoints" => self.set_function_breakpoints_result(request),
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
            "stackTrace" => self.stack_trace_result(),
            "scopes" => self.scopes_result(),
            "variables" => self.variables_result(request),
            "setVariable" => self.set_variable_result(request),
            "evaluate" => self.evaluate_result(request),
            "setExpression" => self.set_expression_result(request),
            "completions" => self.completions_result(request),
            "exceptionInfo" => self.exception_info_result(),
            "loadedSources" => self.loaded_sources_result(),
            "modules" => self.modules_result(request),
            "source" => self.source_result(request),
            "continue" => self.continue_result(),
            "reverseContinue" => self.reverse_continue_result(),
            "goto" => self.goto_result(request),
            "stepBack" => self.step_back_result(),
            "restartFrame" => self.restart_frame_result(request),
            "next" => self.next_result(),
            "stepInTargets" => self.step_in_targets_result(request),
            "stepIn" => self.step_in_result(request),
            "stepOut" => self.step_out_result(),
            "pause" => self.pause_result(),
            "terminateThreads" => self.terminate_threads_result(request),
            "disconnect" | "terminate" => {
                self.queue_event("terminated", serde_json::json!({}));
                self.launched = None;
                Ok(serde_json::json!({}))
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

    fn launch_result(&mut self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let path = dap_program_path(request)?;
        let loaded = orv_project::load_project(&path).map_err(|e| anyhow::anyhow!("{e}"))?;
        let file = lsp_source_file_for_path(&loaded.files, &path)
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
                dap_source_info(&file.path, u64::try_from(index + 1).unwrap_or(u64::MAX))
            })
            .collect();
        let live_requested = dap_launch_live(request);
        let (runtime, mut frames, live, long_running) = dap_launch_runtime_state(
            &lowered,
            diagnostic_count,
            &loaded.files,
            &sources,
            live_requested,
        );
        let async_runtime = long_running.then(DapAsyncRuntimeState::server);
        let mut executable_lines = if frames.is_empty() {
            dap_verified_breakpoint_lines(&entry_path).unwrap_or_else(|_| vec![1])
        } else {
            frames.iter().map(|frame| frame.line).collect::<Vec<_>>()
        };
        if executable_lines.is_empty() {
            executable_lines.push(1);
        }
        executable_lines.sort_unstable();
        executable_lines.dedup();
        let current_frame_index = self.first_verified_breakpoint_frame(&frames).unwrap_or(0);
        let stopped_line = frames
            .get(current_frame_index)
            .map_or(executable_lines[0], |frame| frame.line);
        let stopped_reason = if matches!(runtime.status.as_str(), "diagnostics" | "error") {
            "exception".to_string()
        } else if let Some(reason) = self.breakpoint_frame_reason(&frames, current_frame_index) {
            reason.to_string()
        } else {
            "entry".to_string()
        };
        self.launched = Some(DapLaunchState {
            path: entry_path.clone(),
            uri: entry_uri.clone(),
            name: entry_name.clone(),
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
            "diagnostics": diagnostic_count,
            "runtime": dap_runtime_json(&runtime, async_runtime.as_ref()),
        }))
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
        let path = request
            .pointer("/arguments/program")
            .and_then(serde_json::Value::as_str)
            .map(dap_path_from_protocol_string)
            .transpose()?
            .or_else(|| self.launched.as_ref().map(|launched| launched.path.clone()))
            .ok_or_else(|| anyhow::anyhow!("launch is required before restart"))?;
        let restart_request = serde_json::json!({
            "arguments": {
                "program": path.display().to_string(),
                "live": live_requested,
            },
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
        let content = std::fs::read_to_string(&source.path).map_err(|e| {
            anyhow::anyhow!("failed to read source `{}`: {e}", source.path.display())
        })?;
        Ok(serde_json::json!({
            "content": content,
            "mimeType": "text/x-orv",
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
        self.function_breakpoints = breakpoints.clone();
        Ok(serde_json::json!({
            "breakpoints": breakpoints
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
                .collect::<Vec<_>>(),
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
        self.data_breakpoints = breakpoints.clone();
        Ok(serde_json::json!({
            "breakpoints": breakpoints
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
                .collect::<Vec<_>>(),
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

    fn stack_trace_result(&self) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before stackTrace"))?;
        let frames = dap_stack_frames_json(launched);
        let total_frames = frames.len();
        Ok(serde_json::json!({
            "stackFrames": frames,
            "totalFrames": total_frames,
        }))
    }

    fn scopes_result(&self) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before scopes"))?;
        let (source_name, source_path, source_uri, _) = dap_current_source_and_line(launched);
        Ok(serde_json::json!({
            "scopes": [
                {
                    "name": "Project",
                    "variablesReference": 1,
                    "expensive": false,
                    "source": {
                        "name": source_name,
                        "path": source_path,
                        "sourceReference": 0,
                        "uri": source_uri,
                    },
                },
                {
                    "name": "Locals",
                    "variablesReference": 2,
                    "expensive": false,
                    "source": {
                        "name": source_name,
                        "path": source_path,
                        "sourceReference": 0,
                        "uri": source_uri,
                    },
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
            return Ok(serde_json::json!({
                "variables": dap_current_locals(launched)
                    .iter()
                    .map(dap_variable_json)
                    .collect::<Vec<_>>(),
            }));
        }
        if variables_reference != 1 {
            anyhow::bail!("unknown variablesReference {variables_reference}");
        }
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
            variables.extend([
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
            ]);
        }
        Ok(serde_json::json!({
            "variables": variables,
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

    fn exception_info_result(&self) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before exceptionInfo"))?;
        Ok(dap_exception_info_json(&launched.runtime))
    }

    fn continue_result(&mut self) -> anyhow::Result<serde_json::Value> {
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

    fn reverse_continue_result(&mut self) -> anyhow::Result<serde_json::Value> {
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

    fn step_back_result(&mut self) -> anyhow::Result<serde_json::Value> {
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
        if frame_id != 1 {
            anyhow::bail!("restartFrame currently supports current ORV frameId 1");
        }
        let target_frame = {
            let launched = self
                .launched
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("launch is required before restartFrame"))?;
            dap_restart_frame_target_index(launched)
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

    fn next_result(&mut self) -> anyhow::Result<serde_json::Value> {
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

    fn step_out_result(&mut self) -> anyhow::Result<serde_json::Value> {
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
        if frame_id != 1 {
            anyhow::bail!("stepInTargets currently supports current ORV frameId 1");
        }
        let launched = self
            .launched
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("launch is required before stepInTargets"))?;
        Ok(serde_json::json!({
            "targets": dap_step_in_targets_json(launched),
        }))
    }

    fn step_in_result(&mut self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
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
        self.queue_event(
            "continued",
            serde_json::json!({
                "threadId": 1,
                "allThreadsContinued": false,
            }),
        );
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before continue"))?;
        launched.runtime.status = "running".to_string();
        if let Some(async_runtime) = launched.async_runtime.as_mut() {
            if async_runtime.state != "running" {
                async_runtime.resume_count = async_runtime.resume_count.saturating_add(1);
            }
            async_runtime.state = "running".to_string();
        }
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

    fn pause_result(&mut self) -> anyhow::Result<serde_json::Value> {
        let launched = self
            .launched
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("launch is required before debug control"))?;
        if launched.long_running {
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
                .skip(start)
                .take(end.saturating_sub(start).saturating_add(1))
                .map(|frame| frame.output.clone())
                .collect()
        });
        for output in outputs {
            self.queue_stdout_output(&output);
        }
    }

    fn queue_stdout_output(&mut self, output: &str) {
        if output.is_empty() {
            return;
        }
        self.queue_event(
            "output",
            serde_json::json!({
                "category": "stdout",
                "output": output,
            }),
        );
    }

    fn drain_pending_events(&mut self) -> Vec<serde_json::Value> {
        std::mem::take(&mut self.pending_events)
            .into_iter()
            .map(|event| {
                dap_event_response(self.next_response_seq(), event.event.as_str(), &event.body)
            })
            .collect()
    }

    fn first_verified_breakpoint_line(&self, path: &Path) -> Option<u64> {
        let normalized = dap_normalize_path(path);
        self.breakpoints.get(&normalized).and_then(|breakpoints| {
            breakpoints
                .iter()
                .find(|breakpoint| breakpoint.verified)
                .map(|breakpoint| breakpoint.line)
        })
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

fn dap_runtime_state(
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

fn dap_launch_runtime_state(
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

fn dap_live_runtime_state(
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

fn dap_program_has_long_running_runtime(program: &orv_hir::HirProgram) -> bool {
    program.items.iter().any(dap_stmt_has_long_running_runtime)
}

const fn dap_stmt_has_long_running_runtime(stmt: &orv_hir::HirStmt) -> bool {
    match stmt {
        orv_hir::HirStmt::Expr(expr) => dap_expr_has_long_running_runtime(expr),
        _ => false,
    }
}

const fn dap_expr_has_long_running_runtime(expr: &orv_hir::HirExpr) -> bool {
    matches!(expr.kind, orv_hir::HirExprKind::Server { .. })
}

fn dap_long_running_runtime_state(
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

fn dap_long_running_frame(
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

fn dap_runtime_json(
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
        });
    }
    value
}

fn dap_runtime_frames(
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

fn dap_source_for_span(
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

fn dap_runtime_variable(variable: &orv_runtime::DebugVariable, line: u64) -> DapVariable {
    let (value, value_type) = dap_runtime_value_display(&variable.value);
    DapVariable {
        name: variable.name.clone(),
        value,
        value_type,
        line,
        variables_reference: 0,
    }
}

fn dap_runtime_value_display(value: &orv_runtime::Value) -> (String, String) {
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

fn dap_current_source_and_line(launched: &DapLaunchState) -> (String, String, String, u64) {
    if let Some(frame) = launched.frames.get(launched.current_frame_index) {
        return (
            frame.source.name.clone(),
            frame.source.path.display().to_string(),
            frame.source.uri.clone(),
            frame.line,
        );
    }
    (
        launched.name.clone(),
        launched.path.display().to_string(),
        launched.uri.clone(),
        launched.stopped_line,
    )
}

fn dap_stack_frames_json(launched: &DapLaunchState) -> Vec<serde_json::Value> {
    let current_frame = launched.frames.get(launched.current_frame_index);
    let (source_name, source_path, source_uri, line) = dap_current_source_and_line(launched);
    let current_name = current_frame
        .and_then(|frame| frame.stack.last())
        .map_or_else(|| "orv entry".to_string(), |frame| frame.name.clone());
    let mut frames = vec![dap_stack_frame_json(
        1,
        &current_name,
        &source_name,
        &source_path,
        &source_uri,
        line,
    )];
    if let Some(current_frame) = current_frame {
        for (index, stack_frame) in current_frame.stack.iter().rev().skip(1).enumerate() {
            frames.push(dap_stack_frame_json(
                u64::try_from(index + 2).unwrap_or(u64::MAX),
                &stack_frame.name,
                &stack_frame.source.name,
                &stack_frame.source.path.display().to_string(),
                &stack_frame.source.uri,
                stack_frame.line,
            ));
        }
        if !current_frame.stack.is_empty() {
            frames.push(dap_stack_frame_json(
                u64::try_from(frames.len() + 1).unwrap_or(u64::MAX),
                "orv entry",
                &launched.name,
                &launched.path.display().to_string(),
                &launched.uri,
                1,
            ));
        }
    }
    frames
}

fn dap_stack_frame_json(
    id: u64,
    name: &str,
    source_name: &str,
    source_path: &str,
    source_uri: &str,
    line: u64,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "source": {
            "name": source_name,
            "path": source_path,
            "sourceReference": 0,
            "uri": source_uri,
        },
        "line": line,
        "column": 1,
    })
}

fn dap_step_in_target_id(frame_index: usize) -> u64 {
    u64::try_from(frame_index.saturating_add(1)).unwrap_or(u64::MAX)
}

fn dap_step_in_target_indices(launched: &DapLaunchState) -> Vec<usize> {
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

fn dap_step_in_targets_json(launched: &DapLaunchState) -> Vec<serde_json::Value> {
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
                "source": {
                    "name": call_frame.source.name,
                    "path": call_frame.source.path.display().to_string(),
                    "sourceReference": 0,
                    "uri": call_frame.source.uri,
                },
            }))
        })
        .collect()
}

fn dap_restart_frame_target_index(launched: &DapLaunchState) -> Option<usize> {
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

fn dap_current_locals(launched: &DapLaunchState) -> &[DapVariable] {
    launched
        .frames
        .get(launched.current_frame_index)
        .map_or(&[], |frame| frame.locals.as_slice())
}

fn dap_data_breakpoint_local_name(data_id: &str) -> Option<&str> {
    data_id
        .strip_prefix("local:")
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn dap_frame_local_value<'a>(frame: &'a DapFrameState, name: &str) -> Option<&'a str> {
    frame
        .locals
        .iter()
        .find(|local| local.name == name)
        .map(|local| local.value.as_str())
}

fn dap_breakpoint_condition_matches(frame: &DapFrameState, condition: Option<&str>) -> bool {
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

fn dap_compare_breakpoint_condition(
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

fn dap_compare_condition_numbers(left: &str, op: &str, right: &str) -> Option<bool> {
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

fn dap_normalize_condition_literal(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        let decoded = serde_json::from_str::<String>(trimmed)
            .unwrap_or_else(|_| trimmed.trim_matches('"').to_string());
        return serde_json::to_string(&decoded).unwrap_or(decoded);
    }
    trimmed.to_string()
}

fn dap_condition_value_truthy(value: &str) -> bool {
    !matches!(value, "" | "false" | "0" | "0.0" | "void" | "\"\"")
}

fn dap_hit_condition_matches(condition: &str, hit_count: usize) -> bool {
    let condition = condition.trim();
    if let Some(modulo) = condition
        .strip_prefix('%')
        .and_then(|value| value.trim_start_matches('=').trim().parse::<usize>().ok())
    {
        return modulo > 0 && hit_count.is_multiple_of(modulo);
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

fn dap_set_exception_breakpoints_result(request: &serde_json::Value) -> serde_json::Value {
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

fn dap_exception_info_json(runtime: &DapRuntimeState) -> serde_json::Value {
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

fn dap_variable_json(variable: &DapVariable) -> serde_json::Value {
    serde_json::json!({
        "name": variable.name,
        "value": variable.value,
        "type": variable.value_type,
        "variablesReference": variable.variables_reference,
    })
}

fn dap_set_value_json(variable: &DapVariable) -> serde_json::Value {
    serde_json::json!({
        "value": variable.value,
        "type": variable.value_type,
        "variablesReference": variable.variables_reference,
    })
}

fn dap_local_variables(program: &Program, files: &[SourceFile]) -> Vec<DapVariable> {
    let mut locals = Vec::new();
    let mut env = HashMap::new();
    for stmt in &program.items {
        let local = match stmt {
            Stmt::Let(stmt) => dap_local_variable(
                &stmt.name.name,
                stmt.ty.as_ref(),
                &stmt.init,
                stmt.span,
                files,
                &env,
            ),
            Stmt::Const(stmt) => dap_local_variable(
                &stmt.name.name,
                stmt.ty.as_ref(),
                &stmt.init,
                stmt.span,
                files,
                &env,
            ),
            _ => None,
        };
        if let Some((variable, value)) = local {
            env.insert(variable.name.clone(), value);
            locals.push(variable);
        }
    }
    locals
}

fn dap_local_variable(
    name: &str,
    ty: Option<&TypeRef>,
    init: &Expr,
    span: Span,
    files: &[SourceFile],
    env: &HashMap<String, DapDebugValue>,
) -> Option<(DapVariable, DapDebugValue)> {
    let value = dap_expr_debug_value(init, env)?;
    let line = dap_span_line(span, files)?;
    let variable = DapVariable {
        name: name.to_string(),
        value: value.display_value(),
        value_type: ty.map_or_else(|| value.value_type(), type_ref_string),
        line,
        variables_reference: 0,
    };
    Some((variable, value))
}

fn dap_span_line(span: Span, files: &[SourceFile]) -> Option<u64> {
    let file = files.iter().find(|file| file.id == span.file)?;
    let start = byte_position(&file.source, span.range.start);
    Some(u64::try_from(start.0 + 1).unwrap_or(u64::MAX))
}

fn dap_expr_debug_value(
    expr: &Expr,
    env: &HashMap<String, DapDebugValue>,
) -> Option<DapDebugValue> {
    match &expr.kind {
        ExprKind::Integer(value) => value.parse::<i64>().ok().map(DapDebugValue::Int),
        ExprKind::Float(value) => value.parse::<f64>().ok().map(DapDebugValue::Float),
        ExprKind::String(segments) => {
            dap_string_debug_value(segments, env).map(DapDebugValue::String)
        }
        ExprKind::Regex { pattern, flags } => Some(DapDebugValue::Regex {
            pattern: pattern.clone(),
            flags: flags.clone(),
        }),
        ExprKind::True => Some(DapDebugValue::Bool(true)),
        ExprKind::False => Some(DapDebugValue::Bool(false)),
        ExprKind::Void => Some(DapDebugValue::Void),
        ExprKind::Ident(ident) => env.get(&ident.name).cloned(),
        ExprKind::Paren(inner) => dap_expr_debug_value(inner, env),
        ExprKind::Unary { op, expr } => {
            let value = dap_expr_debug_value(expr, env)?;
            dap_apply_debug_unary(*op, value)
        }
        ExprKind::Binary { op, lhs, rhs } => {
            let lhs = dap_expr_debug_value(lhs, env)?;
            let rhs = dap_expr_debug_value(rhs, env)?;
            dap_apply_debug_binary(*op, lhs, rhs)
        }
        ExprKind::Array(items) => items
            .iter()
            .map(|item| dap_expr_debug_value(item, env))
            .collect::<Option<Vec<_>>>()
            .map(DapDebugValue::Array),
        ExprKind::Tuple(items) => items
            .iter()
            .map(|item| dap_expr_debug_value(item, env))
            .collect::<Option<Vec<_>>>()
            .map(DapDebugValue::Tuple),
        ExprKind::Object(fields) => fields
            .iter()
            .filter(|field| !field.is_spread)
            .map(|field| {
                Some((
                    field.name.name.clone(),
                    dap_expr_debug_value(&field.value, env)?,
                ))
            })
            .collect::<Option<Vec<_>>>()
            .map(DapDebugValue::Object),
        _ => None,
    }
}

fn dap_string_debug_value(
    segments: &[StringSegment],
    env: &HashMap<String, DapDebugValue>,
) -> Option<String> {
    let mut value = String::new();
    for segment in segments {
        match segment {
            StringSegment::Str(text) => value.push_str(text),
            StringSegment::Interp(expr) => {
                value.push_str(&dap_expr_debug_value(expr, env)?.display_value());
            }
        }
    }
    Some(value)
}

fn dap_apply_debug_unary(op: AstUnaryOp, value: DapDebugValue) -> Option<DapDebugValue> {
    match (op, value) {
        (AstUnaryOp::Not, DapDebugValue::Bool(value)) => Some(DapDebugValue::Bool(!value)),
        (AstUnaryOp::Neg, DapDebugValue::Int(value)) => Some(DapDebugValue::Int(-value)),
        (AstUnaryOp::Neg, DapDebugValue::Float(value)) => Some(DapDebugValue::Float(-value)),
        _ => None,
    }
}

fn dap_apply_debug_binary(
    op: AstBinaryOp,
    lhs: DapDebugValue,
    rhs: DapDebugValue,
) -> Option<DapDebugValue> {
    match op {
        AstBinaryOp::Add => dap_debug_add(lhs, rhs),
        AstBinaryOp::Sub => dap_debug_numeric(
            lhs,
            rhs,
            |left, right| left - right,
            |left, right| left - right,
        ),
        AstBinaryOp::Mul => dap_debug_numeric(
            lhs,
            rhs,
            |left, right| left * right,
            |left, right| left * right,
        ),
        AstBinaryOp::Div => dap_debug_numeric(
            lhs,
            rhs,
            |left, right| left / right,
            |left, right| left / right,
        ),
        AstBinaryOp::Rem => dap_debug_numeric(
            lhs,
            rhs,
            |left, right| left % right,
            |left, right| left % right,
        ),
        AstBinaryOp::Eq => Some(DapDebugValue::Bool(lhs == rhs)),
        AstBinaryOp::Ne => Some(DapDebugValue::Bool(lhs != rhs)),
        AstBinaryOp::And => match (lhs, rhs) {
            (DapDebugValue::Bool(left), DapDebugValue::Bool(right)) => {
                Some(DapDebugValue::Bool(left && right))
            }
            _ => None,
        },
        AstBinaryOp::Or => match (lhs, rhs) {
            (DapDebugValue::Bool(left), DapDebugValue::Bool(right)) => {
                Some(DapDebugValue::Bool(left || right))
            }
            _ => None,
        },
        AstBinaryOp::Coalesce => {
            if lhs == DapDebugValue::Void {
                Some(rhs)
            } else {
                Some(lhs)
            }
        }
        _ => None,
    }
}

fn dap_debug_add(lhs: DapDebugValue, rhs: DapDebugValue) -> Option<DapDebugValue> {
    match (lhs, rhs) {
        (DapDebugValue::Int(left), DapDebugValue::Int(right)) => {
            Some(DapDebugValue::Int(left + right))
        }
        (DapDebugValue::Float(left), DapDebugValue::Float(right)) => {
            Some(DapDebugValue::Float(left + right))
        }
        (DapDebugValue::String(left), DapDebugValue::String(right)) => {
            Some(DapDebugValue::String(format!("{left}{right}")))
        }
        _ => None,
    }
}

fn dap_debug_numeric(
    lhs: DapDebugValue,
    rhs: DapDebugValue,
    int_op: impl FnOnce(i64, i64) -> i64,
    float_op: impl FnOnce(f64, f64) -> f64,
) -> Option<DapDebugValue> {
    match (lhs, rhs) {
        (DapDebugValue::Int(left), DapDebugValue::Int(right)) => {
            Some(DapDebugValue::Int(int_op(left, right)))
        }
        (DapDebugValue::Float(left), DapDebugValue::Float(right)) => {
            Some(DapDebugValue::Float(float_op(left, right)))
        }
        _ => None,
    }
}

fn dap_evaluate_project_value(
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
        "runtimeKind" => launched
            .async_runtime
            .as_ref()
            .map(|runtime| (runtime.kind.clone(), "string".to_string())),
        "runtimeAsyncState" => launched
            .async_runtime
            .as_ref()
            .map(|runtime| (runtime.state.clone(), "string".to_string())),
        "runtimeResumeCount" => launched
            .async_runtime
            .as_ref()
            .map(|runtime| (runtime.resume_count.to_string(), "usize".to_string())),
        "runtimePauseCount" => launched
            .async_runtime
            .as_ref()
            .map(|runtime| (runtime.pause_count.to_string(), "usize".to_string())),
        _ => None,
    }
}

fn dap_completion_targets_json(launched: &DapLaunchState, prefix: &str) -> Vec<serde_json::Value> {
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

fn dap_breakpoint_locations_json(
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

fn dap_verified_breakpoint_lines(path: &Path) -> anyhow::Result<Vec<u64>> {
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

fn dap_collect_stmt_breakpoint_lines(
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

fn dap_collect_function_body_breakpoint_lines(
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

fn dap_collect_block_breakpoint_lines(
    block: &Block,
    file_id: FileId,
    files: &[SourceFile],
    lines: &mut Vec<u64>,
) {
    for stmt in &block.stmts {
        dap_collect_stmt_breakpoint_lines(stmt, file_id, files, lines);
    }
}

fn dap_collect_expr_breakpoint_lines(
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

fn dap_push_span_line(span: Span, file_id: FileId, files: &[SourceFile], lines: &mut Vec<u64>) {
    if span.file != file_id {
        return;
    }
    let Some(file) = files.iter().find(|file| file.id == span.file) else {
        return;
    };
    let start = byte_position(&file.source, span.range.start);
    lines.push(u64::try_from(start.0 + 1).unwrap_or(u64::MAX));
}

fn dap_following_executable_line(lines: &[u64], current: u64) -> Option<u64> {
    lines.iter().copied().find(|line| *line > current)
}

fn dap_source_info(path: &Path, reference: u64) -> DapSourceInfo {
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("source.orv")
        .to_string();
    DapSourceInfo {
        reference,
        name,
        path: path.to_path_buf(),
        uri: lsp_file_uri_for_path(path),
    }
}

fn dap_source_json(source: &DapSourceInfo) -> serde_json::Value {
    serde_json::json!({
        "name": source.name,
        "path": source.path.display().to_string(),
        "sourceReference": source.reference,
        "uri": source.uri,
    })
}

fn dap_module_json(source: &DapSourceInfo) -> serde_json::Value {
    serde_json::json!({
        "id": source.reference,
        "name": source.name,
        "path": source.path.display().to_string(),
        "isUserCode": true,
        "symbolStatus": "loaded",
    })
}

fn dap_goto_target_json(source: &DapSourceInfo, line: u64) -> serde_json::Value {
    serde_json::json!({
        "id": dap_goto_target_id(source.reference, line),
        "label": format!("{}:{line}", source.name),
        "line": line,
        "column": 1,
    })
}

const fn dap_goto_target_id(source_reference: u64, line: u64) -> u64 {
    source_reference
        .saturating_mul(1_000_000)
        .saturating_add(line)
}

fn dap_launch_live(request: &serde_json::Value) -> bool {
    request
        .pointer("/arguments/live")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn dap_program_path(request: &serde_json::Value) -> anyhow::Result<PathBuf> {
    let program = request
        .pointer("/arguments/program")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("launch.arguments.program must be a path or file URI"))?;
    dap_path_from_protocol_string(program)
}

fn dap_source_path(request: &serde_json::Value) -> anyhow::Result<PathBuf> {
    let path = request
        .pointer("/arguments/source/path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("source.path must be a path or file URI"))?;
    dap_path_from_protocol_string(path)
}

fn dap_source_reference(request: &serde_json::Value) -> Option<u64> {
    request
        .pointer("/arguments/sourceReference")
        .and_then(serde_json::Value::as_u64)
        .filter(|reference| *reference > 0)
}

fn dap_breakpoint_source_path(
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

fn dap_path_from_protocol_string(path: &str) -> anyhow::Result<PathBuf> {
    if path.starts_with("file://") {
        lsp_file_uri_path(path)
    } else {
        Ok(PathBuf::from(path))
    }
}

fn dap_normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn dap_success_response(
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

fn dap_error_response(
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

fn dap_event_response(seq: u64, event: &str, body: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "type": "event",
        "event": event,
        "body": body,
    })
}

fn lsp_text_document_uri(request: &serde_json::Value) -> anyhow::Result<&str> {
    request
        .pointer("/params/textDocument/uri")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("textDocument.uri must be a file URI"))
}

fn lsp_text_document_position(request: &serde_json::Value) -> anyhow::Result<(usize, usize)> {
    let position = request
        .pointer("/params/position")
        .ok_or_else(|| anyhow::anyhow!("position must be an object"))?;
    lsp_position_value(position)
}

fn lsp_request_range(
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

fn lsp_position_value(value: &serde_json::Value) -> anyhow::Result<(usize, usize)> {
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

fn lsp_diagnostics_for_loaded_project(
    loaded: &orv_project::LoadedProject,
) -> Vec<serde_json::Value> {
    let diagnostics = lsp_project_diagnostics(loaded);
    lsp_diagnostics_json(&diagnostics, &loaded.files)
}

fn lsp_project_diagnostics(
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

fn lsp_workspace_diagnostic_items_json(
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

fn lsp_source_file_for_path<'a>(files: &'a [SourceFile], path: &Path) -> Option<&'a SourceFile> {
    let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    files
        .iter()
        .find(|file| file.path == path || file.path == normalized)
}

fn lsp_definition_node<'a>(
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

fn lsp_location_json(node: &orv_project::ProjectNode, files: &[SourceFile]) -> serde_json::Value {
    let uri = files.iter().find(|file| file.id == node.file).map_or_else(
        || "file://<unknown>".to_string(),
        |file| lsp_file_uri_for_path(&file.path),
    );
    serde_json::json!({
        "uri": uri,
        "range": lsp_range_json(node.span, files),
    })
}

fn lsp_hover_json(node: &orv_project::ProjectNode, files: &[SourceFile]) -> serde_json::Value {
    let kind = lsp_symbol_kind(node.kind).unwrap_or("Symbol");
    serde_json::json!({
        "contents": {
            "kind": "markdown",
            "value": format!("**{kind}** `{}`", node.name),
        },
        "range": lsp_range_json(node.span, files),
    })
}

fn lsp_file_uri_for_path(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn lsp_position_to_byte(source: &str, position: (usize, usize)) -> usize {
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

fn identifier_at_byte(source: &str, byte: usize) -> Option<&str> {
    identifier_span_at_byte(source, byte).map(|(_, _, name)| name)
}

fn identifier_span_at_byte(source: &str, byte: usize) -> Option<(usize, usize, &str)> {
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

fn lsp_reference_locations_json(files: &[SourceFile], name: &str) -> Vec<serde_json::Value> {
    files
        .iter()
        .flat_map(|file| {
            identifier_occurrences(&file.source, name)
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

fn identifier_occurrences(source: &str, name: &str) -> Vec<(usize, usize)> {
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

const fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn lsp_valid_identifier_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_') && bytes.all(is_identifier_byte)
}

fn lsp_file_uri_path(uri: &str) -> anyhow::Result<PathBuf> {
    let raw_path = uri
        .strip_prefix("file://")
        .ok_or_else(|| anyhow::anyhow!("textDocument.uri must use file://"))?;
    Ok(PathBuf::from(percent_decode_uri_path(raw_path)?))
}

fn percent_decode_uri_path(raw: &str) -> anyhow::Result<String> {
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

const fn uri_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn lsp_jsonrpc_result(id: &serde_json::Value, result: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn lsp_jsonrpc_method_not_found(id: &serde_json::Value, method: &str) -> serde_json::Value {
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

fn lsp_jsonrpc_error(id: &serde_json::Value, code: i32, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    })
}

fn lsp_diagnostics_json(
    diagnostics: &[orv_diagnostics::Diagnostic],
    files: &[SourceFile],
) -> Vec<serde_json::Value> {
    diagnostics
        .iter()
        .map(|diagnostic| lsp_diagnostic_json(diagnostic, files))
        .collect()
}

fn lsp_diagnostics_json_for_file(
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

fn lsp_diagnostic_json(
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

fn lsp_diagnostic_span(diagnostic: &orv_diagnostics::Diagnostic) -> Span {
    diagnostic
        .primary
        .as_ref()
        .map(|label| label.span)
        .or_else(|| diagnostic.secondary.first().map(|label| label.span))
        .unwrap_or(Span::DUMMY)
}

fn lsp_diagnostic_file_id(diagnostic: &orv_diagnostics::Diagnostic) -> Option<FileId> {
    diagnostic
        .primary
        .as_ref()
        .map(|label| label.span.file)
        .or_else(|| diagnostic.secondary.first().map(|label| label.span.file))
}

fn lsp_document_symbols_json(graph: &ProjectGraph, files: &[SourceFile]) -> Vec<serde_json::Value> {
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

fn lsp_document_symbols_protocol_json(
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

fn lsp_code_lenses_json(
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

fn lsp_code_actions_json(
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
        .map(|diagnostic| {
            let diagnostic_json = lsp_diagnostic_json(diagnostic, &loaded.files);
            let range = diagnostic_json
                .get("range")
                .cloned()
                .unwrap_or_else(|| lsp_range_for_source(&file.source, start, end));
            serde_json::json!({
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
            })
        })
        .collect()
}

fn lsp_execute_reveal_diagnostic_json(request: &serde_json::Value) -> serde_json::Value {
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

const fn lsp_span_overlaps_range(span: Span, start: u32, end: u32) -> bool {
    span.range.start <= end && start <= span.range.end
}

fn lsp_document_links_json(
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

fn lsp_folding_ranges_json(
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

fn lsp_folding_range_json(span: Span, files: &[SourceFile]) -> Option<serde_json::Value> {
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

fn lsp_selection_range_json(
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

const fn lsp_selectable_node_kind(kind: ProjectNodeKind) -> bool {
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
struct LspSemanticToken {
    line: usize,
    character: usize,
    length: usize,
    token_type: u32,
    modifiers: u32,
}

fn lsp_semantic_tokens_json(
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

fn lsp_node_name_span(source: &str, node: &orv_project::ProjectNode) -> Option<(u32, u32)> {
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

const fn lsp_semantic_token_type(kind: ProjectNodeKind) -> Option<u32> {
    match kind {
        ProjectNodeKind::Domain => Some(0),
        ProjectNodeKind::Struct | ProjectNodeKind::Enum | ProjectNodeKind::TypeAlias => Some(1),
        ProjectNodeKind::Function | ProjectNodeKind::Define => Some(2),
        ProjectNodeKind::File | ProjectNodeKind::Import => None,
    }
}

fn lsp_completion_items_json(graph: &ProjectGraph) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    for node in &graph.nodes {
        let Some(kind) = lsp_completion_item_kind_code(node.kind) else {
            continue;
        };
        if items.iter().any(|item: &serde_json::Value| {
            item.get("label").and_then(serde_json::Value::as_str) == Some(node.name.as_str())
                && item.get("kind").and_then(serde_json::Value::as_u64) == Some(u64::from(kind))
        }) {
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

fn lsp_workspace_symbols_json(
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

const fn lsp_severity(severity: orv_diagnostics::Severity) -> u8 {
    match severity {
        orv_diagnostics::Severity::Error => 1,
        orv_diagnostics::Severity::Warning => 2,
        orv_diagnostics::Severity::Note => 3,
        orv_diagnostics::Severity::Help => 4,
    }
}

const fn lsp_symbol_kind(kind: ProjectNodeKind) -> Option<&'static str> {
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

const fn lsp_symbol_kind_code(kind: ProjectNodeKind) -> Option<u8> {
    match kind {
        ProjectNodeKind::Struct | ProjectNodeKind::TypeAlias => Some(23),
        ProjectNodeKind::Enum => Some(10),
        ProjectNodeKind::Function | ProjectNodeKind::Define => Some(12),
        ProjectNodeKind::Domain => Some(24),
        ProjectNodeKind::File | ProjectNodeKind::Import => None,
    }
}

const fn lsp_completion_item_kind_code(kind: ProjectNodeKind) -> Option<u8> {
    match kind {
        ProjectNodeKind::Struct | ProjectNodeKind::TypeAlias => Some(22),
        ProjectNodeKind::Enum => Some(13),
        ProjectNodeKind::Function | ProjectNodeKind::Define => Some(3),
        ProjectNodeKind::Domain => Some(23),
        ProjectNodeKind::File | ProjectNodeKind::Import => None,
    }
}

fn lsp_range_json(span: Span, files: &[SourceFile]) -> serde_json::Value {
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

fn lsp_range_for_source(source: &str, start: u32, end: u32) -> serde_json::Value {
    lsp_range_from_positions(byte_position(source, start), byte_position(source, end))
}

fn lsp_range_from_positions(start: (usize, usize), end: (usize, usize)) -> serde_json::Value {
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

fn cmd_verify_build(dir: &Path) -> anyhow::Result<()> {
    verify_build_dir(dir)?;
    println!("build: {} verified", dir.display());
    Ok(())
}

fn verify_build_dir(dir: &Path) -> anyhow::Result<()> {
    let manifest = read_json_value(&dir.join("build-manifest.json"))?;
    let plan = read_json_value(&dir.join("bundle-plan.json"))?;
    verify_bundle_targets(dir, &plan)?;
    verify_manifest_artifacts(dir, &manifest)?;
    verify_deploy_manifest_if_present(dir)?;
    verify_dev_hmr_session_if_present(dir, &plan)?;
    verify_dev_watch_session_if_present(dir, &plan)
}

fn verify_manifest_artifacts(dir: &Path, manifest: &serde_json::Value) -> anyhow::Result<()> {
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

fn verify_bundle_targets(dir: &Path, plan: &serde_json::Value) -> anyhow::Result<()> {
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
            }
            "server_launcher" => verify_server_launcher_target(dir, &target)?,
            "static_page" => verify_static_page_target(bundle, &target)?,
            "client_page" => verify_client_page_target(bundle, &target)?,
            "client_js" => verify_client_js_target(&target)?,
            "client_wasm" => verify_client_wasm_target(&target)?,
            _ => {}
        }
    }
    Ok(())
}

fn verify_server_launcher_target(dir: &Path, target: &Path) -> anyhow::Result<()> {
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

fn verify_static_page_target(bundle: &serde_json::Value, target: &Path) -> anyhow::Result<()> {
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

fn verify_client_page_target(bundle: &serde_json::Value, target: &Path) -> anyhow::Result<()> {
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

fn verify_client_page_file(target: &Path) -> anyhow::Result<()> {
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

fn verify_client_js_target(target: &Path) -> anyhow::Result<()> {
    let source = std::fs::read_to_string(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
    if !source.contains("ORV_CLIENT_BOOTSTRAP") {
        anyhow::bail!("client_js bundle does not declare ORV bootstrap metadata");
    }
    if !source.contains("sourceBundleUrl") || !source.contains("../source-bundle.json") {
        anyhow::bail!("client_js bundle does not reference source bundle metadata");
    }
    if !source.contains("app.wasm") {
        anyhow::bail!("client_js bundle does not reference app.wasm");
    }
    if !source.contains("WebAssembly.instantiate") {
        anyhow::bail!("client_js bundle does not instantiate wasm");
    }
    if !source.contains(CLIENT_WASM_START_EXPORT) {
        anyhow::bail!("client_js bundle does not call {CLIENT_WASM_START_EXPORT}");
    }
    Ok(())
}

fn verify_client_wasm_target(target: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(target)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", target.display()))?;
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
    let payload = client_wasm_custom_section_payload(&bytes)?
        .ok_or_else(|| anyhow::anyhow!("client_wasm bundle does not declare ORV metadata"))?;
    let payload = std::str::from_utf8(payload)
        .map_err(|e| anyhow::anyhow!("client_wasm ORV metadata is not UTF-8: {e}"))?;
    let metadata: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| anyhow::anyhow!("client_wasm ORV metadata is not JSON: {e}"))?;
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
    if !metadata
        .get("runtime_features")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|features| features.iter().any(|feature| feature == "client_wasm"))
    {
        anyhow::bail!("client_wasm ORV metadata must include client_wasm runtime feature");
    }
    if !client_wasm_exports_function(&bytes, CLIENT_WASM_START_EXPORT)? {
        anyhow::bail!("client_wasm bundle must export `{CLIENT_WASM_START_EXPORT}`");
    }
    Ok(())
}

fn verify_dev_hmr_session_if_present(dir: &Path, plan: &serde_json::Value) -> anyhow::Result<()> {
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
        matches!(
            target.get("kind").and_then(serde_json::Value::as_str),
            Some("client_page" | "client_js" | "client_wasm")
        )
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

fn verify_dev_watch_session_if_present(dir: &Path, plan: &serde_json::Value) -> anyhow::Result<()> {
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

fn bundle_plan_has_client_target(plan: &serde_json::Value) -> anyhow::Result<bool> {
    let bundles = plan
        .get("bundles")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("bundle plan bundles must be an array"))?;
    Ok(bundles.iter().any(|target| {
        matches!(
            target.get("kind").and_then(serde_json::Value::as_str),
            Some("client_page" | "client_js" | "client_wasm")
        )
    }))
}

fn verify_dev_watch_set(
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

fn client_wasm_exports_function(bytes: &[u8], name: &str) -> anyhow::Result<bool> {
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
        if section_id == 7 && wasm_export_section_has_function(bytes, offset, section_end, name)? {
            return Ok(true);
        }
        offset = section_end;
    }
    Ok(false)
}

fn wasm_export_section_has_function(
    bytes: &[u8],
    mut offset: usize,
    section_end: usize,
    name: &str,
) -> anyhow::Result<bool> {
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
        let _index = read_wasm_u32_leb(bytes, &mut offset, section_end)?;
        if export_name_matches && kind == 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn client_wasm_custom_section_payload(bytes: &[u8]) -> anyhow::Result<Option<&[u8]>> {
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

fn read_wasm_u32_leb(bytes: &[u8], offset: &mut usize, limit: usize) -> anyhow::Result<u32> {
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

fn verify_deploy_manifest_if_present(dir: &Path) -> anyhow::Result<()> {
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
    verify_deploy_source_bundle(dir, deploy.get("source_bundle"))?;
    verify_deploy_server_target(dir, deploy.get("server"))?;
    verify_deploy_static_target(dir, deploy.get("static"))?;
    verify_deploy_client_target(dir, deploy.get("client"))
}

fn verify_deploy_source_bundle(
    dir: &Path,
    source_bundle: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let Some(path) = source_bundle.and_then(serde_json::Value::as_str) else {
        anyhow::bail!("deploy manifest source_bundle must be a string");
    };
    let target = dir.join(path);
    if !target.is_file() {
        anyhow::bail!("missing deploy source bundle: {}", target.display());
    }
    read_source_bundle_artifact(&target)?;
    Ok(())
}

fn verify_deploy_server_target(
    dir: &Path,
    server: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let Some(server) = server.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let artifact_path = json_str(server, "artifact", "deploy server")?;
    let entrypoint = json_str(server, "entrypoint", "deploy server")?;
    let routes_artifact = json_str(server, "routes_artifact", "deploy server")?;
    let container = json_str(server, "container", "deploy server")?;
    let dockerfile = json_str(server, "dockerfile", "deploy server")?;
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
    let artifact = read_server_artifact(&dir.join(artifact_path))?;
    orv_compiler::verify_server_runtime_artifact(&artifact)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    verify_deploy_routes_artifact(
        dir,
        routes_artifact,
        artifact_path,
        artifact.runtime.as_str(),
        &artifact,
    )?;
    verify_deploy_container_artifact(
        dir,
        container,
        dockerfile,
        artifact_path,
        entrypoint,
        routes_artifact,
        artifact.runtime.as_str(),
    )?;
    if server.get("runtime").and_then(serde_json::Value::as_str) != Some(artifact.runtime.as_str())
    {
        anyhow::bail!("deploy server runtime does not match runtime artifact");
    }
    if let Some(routes) = server.get("routes") {
        let artifact_routes = serde_json::to_value(&artifact.routes)?;
        if routes != &artifact_routes {
            anyhow::bail!("deploy server routes do not match runtime artifact");
        }
    }
    Ok(())
}

fn verify_deploy_container_artifact(
    dir: &Path,
    path: &str,
    dockerfile_path: &str,
    artifact_path: &str,
    entrypoint: &str,
    routes_artifact: &str,
    runtime: &str,
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
    if json_str(&container, "artifact", "deploy container")? != artifact_path {
        anyhow::bail!("deploy container artifact must be {artifact_path}");
    }
    if json_str(&container, "entrypoint", "deploy container")? != entrypoint {
        anyhow::bail!("deploy container entrypoint must be {entrypoint}");
    }
    if json_str(&container, "routes_artifact", "deploy container")? != routes_artifact {
        anyhow::bail!("deploy container routes_artifact must be {routes_artifact}");
    }
    if json_str(&container, "dockerfile", "deploy container")? != dockerfile_path {
        anyhow::bail!("deploy container dockerfile must be {dockerfile_path}");
    }
    if json_str(&container, "runtime", "deploy container")? != runtime {
        anyhow::bail!("deploy container runtime does not match runtime artifact");
    }
    if json_str(&container, "protocol", "deploy container")? != "http1" {
        anyhow::bail!("deploy container protocol must be http1");
    }
    let command = container
        .get("command")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("deploy container command must be an array"))?;
    if command.first().and_then(serde_json::Value::as_str) != Some("./deploy/server.sh") {
        anyhow::bail!("deploy container command must start with ./deploy/server.sh");
    }
    verify_deploy_dockerfile(dir, dockerfile_path)
}

fn verify_deploy_dockerfile(dir: &Path, path: &str) -> anyhow::Result<()> {
    let dockerfile_path = dir.join(path);
    if !dockerfile_path.is_file() {
        anyhow::bail!("missing deploy Dockerfile: {}", dockerfile_path.display());
    }
    let dockerfile = std::fs::read_to_string(&dockerfile_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", dockerfile_path.display()))?;
    if !dockerfile.contains("FROM ${ORV_RUNTIME_IMAGE}") {
        anyhow::bail!("deploy Dockerfile must use ORV_RUNTIME_IMAGE");
    }
    if !dockerfile.contains("COPY . /app") {
        anyhow::bail!("deploy Dockerfile must copy build output into /app");
    }
    if !dockerfile.contains(r#"ENTRYPOINT ["./deploy/server.sh"]"#) {
        anyhow::bail!("deploy Dockerfile must run ./deploy/server.sh");
    }
    Ok(())
}

fn verify_deploy_routes_artifact(
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

fn verify_deploy_static_target(
    dir: &Path,
    static_target: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    let Some(static_target) = static_target.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let path = json_str(static_target, "path", "deploy static")?;
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
    Ok(())
}

fn verify_deploy_client_target(
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
    verify_client_wasm_target(&wasm_target)
}

fn read_json_value(path: &Path) -> anyhow::Result<serde_json::Value> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))
}

fn reveal_origin_json(dir: &Path, origin_id: &str) -> anyhow::Result<serde_json::Value> {
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
    Ok(serde_json::json!({
        "schema_version": 1,
        "origin": entry,
        "source": reveal_source(entry, &file_paths, &server_artifacts, source_bundle.as_ref()),
        "project_graph": reveal_project_graph_node(&graph, origin_id),
        "production": {
            "routes": reveal_routes(origin_id, &server_artifacts),
            "client": reveal_client_targets(dir, entry)?,
        },
    }))
}

fn read_origin_map(dir: &Path) -> anyhow::Result<orv_compiler::OriginMap> {
    serde_json::from_value(read_json_value(&dir.join("origin-map.json"))?)
        .map_err(|e| anyhow::anyhow!("failed to parse origin-map.json: {e}"))
}

fn read_server_artifacts(
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

fn read_source_bundle_if_present(
    dir: &Path,
) -> anyhow::Result<Option<orv_compiler::SourceBundleArtifact>> {
    let path = dir.join("source-bundle.json");
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(read_source_bundle_artifact(&path)?))
}

fn read_source_bundle_artifact(path: &Path) -> anyhow::Result<orv_compiler::SourceBundleArtifact> {
    let artifact: orv_compiler::SourceBundleArtifact =
        serde_json::from_value(read_json_value(path)?)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
    orv_compiler::verify_source_bundle_artifact(&artifact)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    Ok(artifact)
}

fn graph_file_paths(graph: &serde_json::Value) -> HashMap<u32, String> {
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

fn reveal_source(
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

fn byte_snippet(source: &str, start: u32, end: u32) -> Option<&str> {
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    source.get(start..end)
}

fn reveal_project_graph_node(graph: &serde_json::Value, origin_id: &str) -> serde_json::Value {
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

fn reveal_routes(
    origin_id: &str,
    server_artifacts: &[(String, orv_compiler::ServerRuntimeArtifact)],
) -> Vec<serde_json::Value> {
    let mut routes = Vec::new();
    for (artifact_path, artifact) in server_artifacts {
        for route in artifact
            .routes
            .iter()
            .filter(|route| route.origin_id == origin_id)
        {
            routes.push(serde_json::json!({
                "artifact": artifact_path,
                "method": route.method,
                "path": route.path,
                "origin_id": route.origin_id,
            }));
        }
    }
    routes
}

fn reveal_client_targets(
    dir: &Path,
    entry: &orv_compiler::OriginEntry,
) -> anyhow::Result<Vec<serde_json::Value>> {
    if !matches!(entry.kind.as_str(), "signal" | "await") {
        return Ok(Vec::new());
    }
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
        if !matches!(kind, "client_page" | "client_js" | "client_wasm") {
            continue;
        }
        let path = json_str(bundle, "path", "bundle target")?;
        targets.push(serde_json::json!({
            "kind": kind,
            "path": path,
            "exists": dir.join(path).is_file(),
            "runtime_features": bundle
                .get("runtime_features")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        }));
    }
    Ok(targets)
}

fn json_str<'a>(value: &'a serde_json::Value, key: &str, context: &str) -> anyhow::Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("{context} field `{key}` must be a string"))
}

fn json_u32(value: &serde_json::Value, key: &str, context: &str) -> anyhow::Result<u32> {
    let raw = value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("{context} field `{key}` must be an integer"))?;
    u32::try_from(raw).map_err(|_| anyhow::anyhow!("{context} field `{key}` is too large"))
}

fn cmd_verify_artifact(path: &Path) -> anyhow::Result<()> {
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

fn cmd_check_artifact(path: &Path) -> anyhow::Result<()> {
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

fn cmd_check_build(dir: &Path) -> anyhow::Result<()> {
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

fn cmd_run_artifact(path: &Path) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    run_artifact_with_writer(path, &mut stdout)
}

fn cmd_run_build(dir: &Path) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    run_build_with_writer(dir, &mut stdout)
}

fn cmd_dev(path: &Path, out: &Path, hmr: bool, watch: bool) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    if hmr {
        dev_with_writer_with_options(path, out, true, watch, &mut stdout)
    } else if watch {
        dev_with_writer_with_options(path, out, false, true, &mut stdout)
    } else {
        dev_with_writer(path, out, &mut stdout)
    }
}

fn dev_with_writer<W: std::io::Write>(
    path: &Path,
    out: &Path,
    writer: &mut W,
) -> anyhow::Result<()> {
    dev_with_writer_with_options(path, out, false, false, writer)
}

fn dev_with_writer_with_options<W: std::io::Write>(
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
    }
    if watch {
        write_dev_watch_session(out, hmr)?;
    }
    run_build_with_writer(out, writer)
}

fn write_dev_hmr_session(out: &Path) -> anyhow::Result<()> {
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

fn write_dev_watch_session(out: &Path, hmr: bool) -> anyhow::Result<()> {
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

fn dev_session_inputs(
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
        matches!(
            target.get("kind").and_then(serde_json::Value::as_str),
            Some("client_page" | "client_js" | "client_wasm")
        )
    });
    Ok((sources, targets, has_client_target))
}

fn json_string_field<'a>(
    value: &'a serde_json::Value,
    field: &str,
    context: &str,
) -> anyhow::Result<&'a str> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("{context} {field} must be a string"))
}

fn run_build_with_writer<W: std::io::Write>(dir: &Path, writer: &mut W) -> anyhow::Result<()> {
    let plan_path = dir.join("bundle-plan.json");
    if plan_path.is_file() {
        let plan = read_json_value(&plan_path)?;
        if let Some(launcher) = bundle_target_path(&plan, "server_launcher")? {
            let launch_path = dir.join(launcher);
            verify_server_launcher_target(dir, &launch_path)?;
            let launch = read_server_launch_artifact(&launch_path)?;
            return run_artifact_with_writer(&dir.join(launch.artifact), writer);
        }
        return run_static_build_with_writer(dir, writer);
    }
    let launch_path = dir.join("server").join("launch.json");
    if launch_path.is_file() {
        verify_server_launcher_target(dir, &launch_path)?;
        let launch = read_server_launch_artifact(&launch_path)?;
        return run_artifact_with_writer(&dir.join(launch.artifact), writer);
    }
    run_static_build_with_writer(dir, writer)
}

fn bundle_target_path(plan: &serde_json::Value, kind: &str) -> anyhow::Result<Option<String>> {
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

fn run_static_build_with_writer<W: std::io::Write>(
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

fn run_artifact_with_writer<W: std::io::Write>(path: &Path, writer: &mut W) -> anyhow::Result<()> {
    let artifact = read_server_artifact(path)?;
    orv_compiler::verify_server_runtime_artifact(&artifact)
        .map_err(|errors| anyhow::anyhow!("{}", errors.join("; ")))?;
    let lowered = lower_artifact_entry(&artifact)?;
    orv_runtime::run_with_writer(&lowered.program, writer).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn lsp_serve_stdio_stream<R, W>(reader: &mut R, writer: &mut W) -> anyhow::Result<()>
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

fn dap_serve_stdio_stream<R, W>(reader: &mut R, writer: &mut W) -> anyhow::Result<()>
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
fn lsp_stdio_response(input: &str) -> anyhow::Result<String> {
    let mut reader = std::io::Cursor::new(input.as_bytes());
    let mut writer = Vec::new();
    lsp_serve_stdio_stream(&mut reader, &mut writer)?;
    String::from_utf8(writer).map_err(|e| anyhow::anyhow!("invalid utf-8 LSP response: {e}"))
}

#[cfg(test)]
fn dap_stdio_response(input: &str) -> anyhow::Result<String> {
    let mut reader = std::io::Cursor::new(input.as_bytes());
    let mut writer = Vec::new();
    dap_serve_stdio_stream(&mut reader, &mut writer)?;
    String::from_utf8(writer).map_err(|e| anyhow::anyhow!("invalid utf-8 DAP response: {e}"))
}

fn read_lsp_content_length<R: std::io::BufRead>(reader: &mut R) -> anyhow::Result<Option<usize>> {
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

fn write_lsp_response_frame<W: std::io::Write>(
    writer: &mut W,
    response: &serde_json::Value,
) -> anyhow::Result<()> {
    let body = serde_json::to_string(response)?;
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    Ok(())
}

fn read_server_artifact(path: &Path) -> anyhow::Result<orv_compiler::ServerRuntimeArtifact> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))
}

fn read_server_launch_artifact(path: &Path) -> anyhow::Result<orv_compiler::ServerLaunchArtifact> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))
}

fn lower_artifact_entry(
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

fn lower_source_bundle_entry(
    artifact: &orv_compiler::SourceBundleArtifact,
) -> anyhow::Result<orv_analyzer::LowerResult> {
    let entry = source_bundle_entry_path(artifact)?;
    let loaded = orv_project::load_project_from_sources(
        &entry,
        artifact
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

fn artifact_entry_path(artifact: &orv_compiler::ServerRuntimeArtifact) -> anyhow::Result<PathBuf> {
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

fn source_bundle_entry_path(
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

fn normalized_artifact_path(path: &str) -> String {
    path.replace('\\', "/")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BuildProfile {
    Development,
    Production,
}

impl BuildProfile {
    const fn from_prod_flag(prod: bool) -> Self {
        if prod {
            Self::Production
        } else {
            Self::Development
        }
    }

    const fn is_production(self) -> bool {
        matches!(self, Self::Production)
    }
}

fn cmd_build(path: &Path, out: &Path) -> anyhow::Result<()> {
    cmd_build_with_profile(path, out, BuildProfile::Development)
}

fn cmd_build_with_profile(path: &Path, out: &Path, profile: BuildProfile) -> anyhow::Result<()> {
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
    let server_artifact_path = "server/app.orv-runtime.json";
    let server_launch_path = "server/launch.json";
    let source_bundle = orv_compiler::source_bundle_artifact(
        entry.display().to_string(),
        loaded
            .files
            .iter()
            .map(|file| (file.path.display().to_string(), file.source.clone())),
    );
    let server_artifact = manifest.capabilities.has_server.then(|| {
        orv_compiler::server_runtime_artifact(
            &manifest,
            &origin_map,
            loaded
                .files
                .iter()
                .map(|file| (file.path.display().to_string(), file.source.clone())),
        )
    });

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
        &serde_json::to_value(origin_map)?,
    )?;
    write_json(&out.join("project-graph.json"), &graph)?;
    write_json(
        &out.join("source-bundle.json"),
        &serde_json::to_value(&source_bundle)?,
    )?;
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
    }
    if let Some((path, html)) = static_page {
        write_text(&out.join(path), &html)?;
    }
    if manifest.capabilities.client_wasm {
        let page_path = required_bundle_output_path(&client_page_path, "client_page")?;
        let js_path = required_bundle_output_path(&client_js_path, "client_js")?;
        let wasm_path = required_bundle_output_path(&client_wasm_path, "client_wasm")?;
        write_client_wasm_placeholder(&out.join(wasm_path))?;
        write_client_js_loader(&out.join(js_path))?;
        let loader_src = relative_bundle_path(page_path, js_path);
        write_client_page_shell(&out.join(page_path), &entry, &loader_src)?;
    }
    if profile.is_production() {
        write_prod_deploy_artifacts(
            out,
            &entry,
            &manifest,
            server_artifact.as_ref(),
            ProdBuildTargets {
                static_page_path: static_page_path.as_deref(),
                client_page_path: client_page_path.as_deref(),
                client_js_path: client_js_path.as_deref(),
                client_wasm_path: client_wasm_path.as_deref(),
                server_artifact_path,
            },
        )?;
    }
    println!("build: wrote {}", out.display());
    Ok(())
}

fn bundle_output_path(plan: &orv_compiler::BundlePlan, kind: &str) -> Option<String> {
    plan.bundles
        .iter()
        .find(|bundle| bundle.kind == kind)
        .map(|bundle| normalized_artifact_path(&bundle.path))
}

fn required_bundle_output_path<'a>(
    path: &'a Option<String>,
    kind: &str,
) -> anyhow::Result<&'a str> {
    path.as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing {kind} bundle target"))
}

const WASM_MODULE_HEADER: &[u8] = b"\0asm\x01\0\0\0";
const CLIENT_WASM_CUSTOM_SECTION_NAME: &str = "orv.client";
const CLIENT_WASM_SOURCE_BUNDLE_PATH: &str = "../source-bundle.json";
const CLIENT_WASM_START_EXPORT: &str = "orv_start";
const CLIENT_WASM_CUSTOM_SECTION_PAYLOAD: &str = r#"{"schema_version":1,"runtime_features":["client_wasm"],"source_bundle":"../source-bundle.json"}"#;

fn write_client_wasm_placeholder(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, client_wasm_placeholder_bytes())
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))
}

fn client_wasm_placeholder_bytes() -> Vec<u8> {
    let mut bytes = WASM_MODULE_HEADER.to_vec();
    let mut custom_section = Vec::new();
    push_wasm_len(&mut custom_section, CLIENT_WASM_CUSTOM_SECTION_NAME.len());
    custom_section.extend_from_slice(CLIENT_WASM_CUSTOM_SECTION_NAME.as_bytes());
    custom_section.extend_from_slice(CLIENT_WASM_CUSTOM_SECTION_PAYLOAD.as_bytes());

    bytes.push(0);
    push_wasm_len(&mut bytes, custom_section.len());
    bytes.extend(custom_section);

    let mut type_section = Vec::new();
    push_wasm_u32_leb(&mut type_section, 1);
    type_section.push(0x60);
    push_wasm_u32_leb(&mut type_section, 0);
    push_wasm_u32_leb(&mut type_section, 0);
    push_wasm_section(&mut bytes, 1, &type_section);

    let mut function_section = Vec::new();
    push_wasm_u32_leb(&mut function_section, 1);
    push_wasm_u32_leb(&mut function_section, 0);
    push_wasm_section(&mut bytes, 3, &function_section);

    let mut export_section = Vec::new();
    push_wasm_u32_leb(&mut export_section, 1);
    push_wasm_len(&mut export_section, CLIENT_WASM_START_EXPORT.len());
    export_section.extend_from_slice(CLIENT_WASM_START_EXPORT.as_bytes());
    export_section.push(0);
    push_wasm_u32_leb(&mut export_section, 0);
    push_wasm_section(&mut bytes, 7, &export_section);

    let mut code_section = Vec::new();
    push_wasm_u32_leb(&mut code_section, 1);
    push_wasm_u32_leb(&mut code_section, 2);
    push_wasm_u32_leb(&mut code_section, 0);
    code_section.push(0x0b);
    push_wasm_section(&mut bytes, 10, &code_section);
    bytes
}

fn push_wasm_section(out: &mut Vec<u8>, id: u8, section: &[u8]) {
    out.push(id);
    push_wasm_len(out, section.len());
    out.extend_from_slice(section);
}

fn push_wasm_len(out: &mut Vec<u8>, len: usize) {
    let len = u32::try_from(len).expect("WASM section length fits in u32");
    push_wasm_u32_leb(out, len);
}

fn push_wasm_u32_leb(out: &mut Vec<u8>, mut value: u32) {
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

fn write_client_js_loader(path: &Path) -> anyhow::Result<()> {
    let script = r#"export const ORV_CLIENT_BOOTSTRAP = Object.freeze({
  schemaVersion: 1,
  runtimeFeatures: ["client_wasm"],
  wasmUrl: "./app.wasm",
  sourceBundleUrl: "../source-bundle.json",
});

const wasmUrl = new URL(ORV_CLIENT_BOOTSTRAP.wasmUrl, import.meta.url);
const sourceBundleUrl = new URL(ORV_CLIENT_BOOTSTRAP.sourceBundleUrl, import.meta.url);
const root = document.querySelector('[data-orv-client="wasm"]');

async function main() {
  const response = await fetch(wasmUrl);
  const bytes = await response.arrayBuffer();
  const { instance } = await WebAssembly.instantiate(bytes, {});
  if (typeof instance.exports.orv_start === "function") {
    instance.exports.orv_start();
  }
  if (root) {
    root.dataset.orvStatus = "ready";
    root.dataset.orvSourceBundle = sourceBundleUrl.href;
  }
}

main().catch((error) => {
  console.error("orv client bootstrap failed", error);
  if (root) {
    root.dataset.orvStatus = "error";
  }
});
"#;
    write_text(path, script)
}

fn write_client_page_shell(path: &Path, entry: &Path, loader_src: &str) -> anyhow::Result<()> {
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

fn relative_bundle_path(from: &str, to: &str) -> String {
    let depth = from.split('/').count().saturating_sub(1);
    format!("{}{}", "../".repeat(depth), to)
}

fn html_attr_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

struct ProdBuildTargets<'a> {
    static_page_path: Option<&'a str>,
    client_page_path: Option<&'a str>,
    client_js_path: Option<&'a str>,
    client_wasm_path: Option<&'a str>,
    server_artifact_path: &'a str,
}

fn write_prod_deploy_artifacts(
    out: &Path,
    entry: &Path,
    manifest: &orv_compiler::BuildManifest,
    server_artifact: Option<&orv_compiler::ServerRuntimeArtifact>,
    targets: ProdBuildTargets<'_>,
) -> anyhow::Result<()> {
    let server = if let Some(server_artifact) = server_artifact {
        let entrypoint = "deploy/server.sh";
        let routes_artifact = "deploy/routes.json";
        let container = "deploy/container.json";
        let dockerfile = "deploy/Dockerfile";
        write_prod_server_entrypoint(out, targets.server_artifact_path)?;
        write_prod_routes_artifact(out, targets.server_artifact_path, server_artifact)?;
        write_prod_container_artifacts(
            out,
            targets.server_artifact_path,
            entrypoint,
            routes_artifact,
            dockerfile,
            server_artifact,
        )?;
        serde_json::json!({
            "runtime": server_artifact.runtime.clone(),
            "artifact": targets.server_artifact_path,
            "entrypoint": entrypoint,
            "routes_artifact": routes_artifact,
            "container": container,
            "dockerfile": dockerfile,
            "protocol": "http1",
            "routes": server_artifact.routes.clone(),
        })
    } else {
        serde_json::Value::Null
    };
    let static_target = targets
        .static_page_path
        .map_or(serde_json::Value::Null, |path| {
            serde_json::json!({
                "path": path,
                "runtime_features": [],
            })
        });
    let client = if manifest.capabilities.client_wasm {
        serde_json::json!({
            "page": targets.client_page_path.ok_or_else(|| anyhow::anyhow!("missing client_page bundle target"))?,
            "loader": targets.client_js_path.ok_or_else(|| anyhow::anyhow!("missing client_js bundle target"))?,
            "wasm": targets.client_wasm_path.ok_or_else(|| anyhow::anyhow!("missing client_wasm bundle target"))?,
            "runtime_features": ["client_wasm"],
        })
    } else {
        serde_json::Value::Null
    };
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

fn write_prod_routes_artifact(
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

fn write_prod_container_artifacts(
    out: &Path,
    server_artifact_path: &str,
    entrypoint: &str,
    routes_artifact: &str,
    dockerfile_path: &str,
    server_artifact: &orv_compiler::ServerRuntimeArtifact,
) -> anyhow::Result<()> {
    let container = serde_json::json!({
        "schema_version": 1,
        "kind": "reference-server-container",
        "dockerfile": dockerfile_path,
        "artifact": server_artifact_path,
        "entrypoint": entrypoint,
        "routes_artifact": routes_artifact,
        "runtime": server_artifact.runtime.clone(),
        "protocol": "http1",
        "command": ["./deploy/server.sh"],
    });
    write_json(&out.join("deploy").join("container.json"), &container)?;
    write_text(
        &out.join(dockerfile_path),
        r#"ARG ORV_RUNTIME_IMAGE=ghcr.io/orv-lang/orv-reference:latest
FROM ${ORV_RUNTIME_IMAGE}
WORKDIR /app
COPY . /app
ENTRYPOINT ["./deploy/server.sh"]
"#,
    )
}

fn write_prod_server_entrypoint(out: &Path, server_artifact_path: &str) -> anyhow::Result<()> {
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
fn set_executable_if_supported(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .map_err(|e| anyhow::anyhow!("failed to stat {}: {e}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
        .map_err(|e| anyhow::anyhow!("failed to chmod {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_executable_if_supported(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

fn render_static_page(lowered: &orv_analyzer::LowerResult) -> anyhow::Result<String> {
    let mut out = Vec::new();
    orv_runtime::run_with_writer(&lowered.program, &mut out).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut html = String::from_utf8(out).map_err(|e| anyhow::anyhow!("html is not utf-8: {e}"))?;
    if html.ends_with('\n') {
        html.pop();
        if html.ends_with('\r') {
            html.pop();
        }
    }
    Ok(html)
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
mod tests {
    use super::*;

    fn workspace_path(parts: &[&str]) -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("../..");
        for part in parts {
            path.push(part);
        }
        path
    }

    fn orv_files_under(parts: &[&str]) -> Vec<PathBuf> {
        let root = workspace_path(parts);
        let mut files = Vec::new();
        collect_orv_files(&root, &mut files);
        files.sort();
        files
    }

    fn collect_orv_files(root: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(root)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", root.display()))
        {
            let path = entry.expect("fixture dir entry").path();
            if path.is_dir() {
                collect_orv_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "orv") {
                out.push(path);
            }
        }
    }

    fn temp_output_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after unix epoch")
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("orv-cli-{name}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn json_routes_include(routes: &serde_json::Value, method: &str, path: &str) -> bool {
        routes.as_array().is_some_and(|routes| {
            routes
                .iter()
                .any(|route| route["method"] == method && route["path"] == path)
        })
    }

    fn protocol_frames(output: &str) -> Vec<serde_json::Value> {
        let mut offset = 0;
        let mut frames = Vec::new();
        while offset < output.len() {
            let tail = &output[offset..];
            let (headers, _) = tail
                .split_once("\r\n\r\n")
                .expect("content-length response frame");
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .expect("content length header");
            let body_start = offset + headers.len() + "\r\n\r\n".len();
            let body_end = body_start + content_length;
            let body = output.get(body_start..body_end).expect("complete body");
            frames.push(serde_json::from_str(body).expect("response json"));
            offset = body_end;
        }
        frames
    }

    fn protocol_request_frame(body: &serde_json::Value) -> String {
        let body = body.to_string();
        format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
    }

    #[test]
    fn check_accepts_all_e2e_fixtures() {
        let files = orv_files_under(&["fixtures", "e2e"]);
        assert!(!files.is_empty(), "expected e2e fixtures");
        for file in files {
            cmd_check(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        }
    }

    #[test]
    fn check_accepts_plan_and_default_fixtures() {
        let mut files = orv_files_under(&["fixtures", "plan"]);
        files.push(workspace_path(&["fixtures", "default-syntax.orv"]));
        assert!(!files.is_empty(), "expected plan fixtures");
        for file in files {
            cmd_check(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        }
    }

    #[test]
    fn check_accepts_orv_toml_project_entry() {
        let dir = temp_output_dir("project-manifest-check");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).expect("create src dir");
        let entry = src.join("main.orv");
        std::fs::write(&entry, "@out \"manifest check\"\n").expect("write entry");
        let manifest = dir.join("orv.toml");
        std::fs::write(
            &manifest,
            r#"[project]
name = "manifest-demo"
entry = "src/main.orv"
"#,
        )
        .expect("write manifest");

        cmd_check(&manifest).expect("manifest check");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn graph_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "graph", "fixtures/e2e/hello.orv"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn init_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "init", "target/new-shop", "--name", "new-shop"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn test_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "test", "src/models", "--filter", "user"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn test_list_flag_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "test", "--list", "src/models"]);
        let cli = match parsed {
            Ok(cli) => cli,
            Err(err) => panic!("{}", err.render()),
        };
        match cli.command {
            Command::Test { path, filter, list } => {
                assert_eq!(path, PathBuf::from("src/models"));
                assert_eq!(filter, None);
                assert!(list);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn test_list_json_discovers_filtered_tests_without_running_them() {
        let dir = temp_output_dir("test-runner-list");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("checkout_test.orv");
        std::fs::write(
            &source,
            r#"test "checkout shows cart" {
  assert true
}

test "checkout failing runtime body" {
  assert false
}
"#,
        )
        .expect("write test source");

        let value = orv_test_list_json(&dir, Some("shows")).expect("test list");
        let tests = value["tests"].as_array().expect("tests array");

        assert_eq!(value["schema_version"], 1);
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0]["name"], "checkout shows cart");
        assert_eq!(tests[0]["path"], source.display().to_string());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_summary_discovers_and_runs_matching_tests() {
        let dir = temp_output_dir("test-runner-pass");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("math_test.orv");
        std::fs::write(
            &source,
            r#"test "math adds" {
  assert 1 + 2 == 3
}
"#,
        )
        .expect("write test source");

        let summary = orv_test_summary(&dir, Some("math")).expect("test summary");

        assert_eq!(summary.selected, 1);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 0);
        assert!(summary.files.iter().any(|file| file == &source));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_summary_reports_runtime_failures() {
        let dir = temp_output_dir("test-runner-fail");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("math_test.orv");
        std::fs::write(
            &source,
            r#"test "math fails" {
  assert 1 + 2 == 4
}
"#,
        )
        .expect("write test source");

        let err = orv_test_summary(&dir, None).expect_err("failing test should fail");

        assert!(err.to_string().contains("math_test.orv"));
        assert!(err.to_string().contains("assertion failed"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn init_writes_project_manifest_and_entry() {
        let dir = temp_output_dir("init-project");

        cmd_init(&dir, Some("starter-shop"), InitTemplate::Basic).expect("init project");

        let manifest = dir.join("orv.toml");
        let entry = dir.join("src").join("main.orv");
        assert!(manifest.is_file(), "missing {}", manifest.display());
        assert!(entry.is_file(), "missing {}", entry.display());
        let manifest_text = std::fs::read_to_string(&manifest).expect("manifest text");
        assert!(manifest_text.contains("name = \"starter-shop\""));
        assert!(manifest_text.contains("entry = \"src/main.orv\""));
        cmd_check(&manifest).expect("check manifest project");
        cmd_check(&dir).expect("check project directory");
        let out = dir.join("dist");
        cmd_build(&dir, &out).expect("build project directory");
        assert!(out.join("pages").join("index.html").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn init_accepts_shop_template_flag() {
        let parsed = Cli::try_parse_from(["orv", "init", "target/new-shop", "--template", "shop"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn init_shop_template_scaffolds_shopping_routes() {
        let dir = temp_output_dir("init-shop-template");

        cmd_init(&dir, Some("starter-shop"), InitTemplate::Shop).expect("init shop project");

        let entry = dir.join("src").join("main.orv");
        let source = std::fs::read_to_string(&entry).expect("entry source");
        assert!(source.contains("@route POST /members"));
        assert!(source.contains("@route POST /payments"));
        assert!(source.contains("@route POST /shipments"));
        cmd_check(&dir).expect("check shop project");
        let out = dir.join("dist");
        cmd_build_with_profile(&dir, &out, BuildProfile::Production).expect("build shop project");
        assert!(out.join("server").join("app.orv-runtime.json").is_file());
        assert!(out.join("deploy").join("manifest.json").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn init_shop_template_writes_deploy_guide() {
        let dir = temp_output_dir("init-shop-guide");

        cmd_init(&dir, Some("starter-shop"), InitTemplate::Shop).expect("init shop project");

        let guide = std::fs::read_to_string(dir.join("README.md")).expect("shop README");
        assert!(guide.contains("starter-shop"));
        assert!(guide.contains("orv check ."));
        assert!(guide.contains("orv build . --prod --out dist"));
        assert!(guide.contains("orv verify-build dist"));
        assert!(guide.contains("POST /members"));
        assert!(guide.contains("POST /payments"));
        assert!(guide.contains("POST /shipments"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn init_shop_template_prod_artifacts_keep_full_service_routes() {
        let dir = temp_output_dir("init-shop-prod-routes");

        cmd_init(&dir, Some("starter-shop"), InitTemplate::Shop).expect("init shop project");
        let out = dir.join("dist");
        cmd_build_with_profile(&dir, &out, BuildProfile::Production).expect("build shop project");

        let runtime =
            read_json_value(&out.join("server").join("app.orv-runtime.json")).expect("runtime");
        let deploy = read_json_value(&out.join("deploy").join("manifest.json")).expect("deploy");
        for (method, path) in [
            ("POST", "/members"),
            ("POST", "/payments"),
            ("POST", "/shipments"),
        ] {
            assert!(json_routes_include(&runtime["routes"], method, path));
            assert!(json_routes_include(
                &deploy["server"]["routes"],
                method,
                path
            ));
        }
        cmd_verify_build(&out).expect("verify shop prod build");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_snapshot_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "lsp", "snapshot", "fixtures/e2e/hello.orv"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn lsp_reveal_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "lsp",
            "reveal",
            "target/orv-build-test",
            "route:GET_/ping:abc123",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn lsp_serve_stdio_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "lsp", "serve", "--stdio"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn dap_serve_stdio_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "dap", "serve", "--stdio"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn build_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "build",
            "fixtures/e2e/hello.orv",
            "--out",
            "target/orv-build-test",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn db_plan_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "db", "plan", "fixtures/e2e/hello.orv"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn db_apply_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "db",
            "apply",
            "fixtures/e2e/hello.orv",
            "--schema",
            "target/orv-db-schema.json",
            "--history",
            "target/orv-db-history.json",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn db_migrate_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "db",
            "migrate",
            "fixtures/e2e/hello.orv",
            "--schema",
            "target/orv-db-schema.json",
            "--history",
            "target/orv-db-history.json",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn db_rollback_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "db",
            "rollback",
            "--schema",
            "target/orv-db-schema.json",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn db_plan_reports_added_nullable_field_from_applied_snapshot() {
        let dir = temp_output_dir("db-plan");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
        )
        .expect("write source");
        let applied = dir.join("applied-schema.json");
        std::fs::write(
            &applied,
            r#"{
  "schema_version": 1,
  "structs": {
    "User": {
      "fields": {
        "id": { "type": "int", "optional": false },
        "email": { "type": "string", "optional": false }
      }
    }
  }
}"#,
        )
        .expect("write applied schema");

        let plan = db_plan_json(&source, Some(&applied)).expect("db plan");

        let actions = plan["actions"].as_array().expect("actions array");
        assert!(actions.iter().any(|action| {
            action["kind"] == "add_field"
                && action["struct"] == "User"
                && action["field"] == "avatar"
                && action["type"] == "string?"
                && action["optional"] == true
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_snapshot_includes_diagnostics_graph_and_document_symbols() {
        let dir = temp_output_dir("lsp-snapshot");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User {
  id: int
}

function greet(user: User): string -> "hello"
"#,
        )
        .expect("write source");

        let snapshot = lsp_snapshot_json(&source).expect("lsp snapshot");

        assert_eq!(snapshot["schema_version"], 1);
        assert_eq!(
            snapshot["diagnostics"]
                .as_array()
                .expect("diagnostics")
                .len(),
            0
        );
        assert!(snapshot["project_graph"]["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .any(|node| node["kind"] == "struct" && node["name"] == "User"));
        let symbols = snapshot["document_symbols"]
            .as_array()
            .expect("document symbols");
        let user = symbols
            .iter()
            .find(|symbol| symbol["name"] == "User")
            .expect("User symbol");
        assert_eq!(user["kind"], "Struct");
        assert_eq!(user["range"]["start"]["line"], 0);
        assert!(symbols
            .iter()
            .any(|symbol| symbol["name"] == "greet" && symbol["kind"] == "Function"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_initialize_returns_server_capabilities() {
        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "initialize",
            "params": {},
        }));

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 7);
        assert_eq!(response["result"]["serverInfo"]["name"], "orv-lsp");
        assert_eq!(response["result"]["capabilities"]["textDocumentSync"], 1);
        assert_eq!(
            response["result"]["capabilities"]["documentSymbolProvider"],
            true
        );
        assert_eq!(
            response["result"]["capabilities"]["documentLinkProvider"]["resolveProvider"],
            false
        );
        assert_eq!(
            response["result"]["capabilities"]["foldingRangeProvider"],
            true
        );
        assert_eq!(
            response["result"]["capabilities"]["selectionRangeProvider"],
            true
        );
        assert_eq!(
            response["result"]["capabilities"]["definitionProvider"],
            true
        );
        assert_eq!(
            response["result"]["capabilities"]["referencesProvider"],
            true
        );
        assert_eq!(
            response["result"]["capabilities"]["documentHighlightProvider"],
            true
        );
        assert_eq!(
            response["result"]["capabilities"]["semanticTokensProvider"]["full"],
            true
        );
        assert_eq!(
            response["result"]["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"][1],
            "type"
        );
        assert_eq!(
            response["result"]["capabilities"]["codeLensProvider"]["resolveProvider"],
            false
        );
        assert_eq!(
            response["result"]["capabilities"]["codeActionProvider"]["codeActionKinds"][0],
            "quickfix"
        );
        assert_eq!(
            response["result"]["capabilities"]["executeCommandProvider"]["commands"][0],
            "orv.revealSourceNode"
        );
        assert_eq!(
            response["result"]["capabilities"]["renameProvider"]["prepareProvider"],
            true
        );
        assert_eq!(
            response["result"]["capabilities"]["workspaceSymbolProvider"],
            true
        );
        assert_eq!(response["result"]["capabilities"]["hoverProvider"], true);
        assert_eq!(
            response["result"]["capabilities"]["completionProvider"]["triggerCharacters"][0],
            "@"
        );
        assert_eq!(
            response["result"]["capabilities"]["diagnosticProvider"]["workspaceDiagnostics"],
            true
        );
    }

    #[test]
    fn lsp_shutdown_returns_null_result() {
        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "shutdown",
        }));

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 8);
        assert!(response.get("error").is_none());
        assert!(response
            .get("result")
            .is_some_and(serde_json::Value::is_null));
    }

    #[test]
    fn lsp_unknown_method_returns_method_not_found_with_method_name() {
        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": "request-9",
            "method": "workspace/configuration",
        }));

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], "request-9");
        assert_eq!(response["error"]["code"], -32601);
        assert_eq!(
            response["error"]["data"]["method"],
            "workspace/configuration"
        );
    }

    #[test]
    fn lsp_stdio_serves_content_length_initialize_frame() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "initialize",
            "params": {},
        })
        .to_string();
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        let output = lsp_stdio_response(&input).expect("stdio response");
        let (_, response_body) = output
            .split_once("\r\n\r\n")
            .expect("content-length response frame");
        let response: serde_json::Value =
            serde_json::from_str(response_body).expect("response json");

        assert!(output.starts_with("Content-Length: "));
        assert_eq!(response["id"], 10);
        assert_eq!(response["result"]["serverInfo"]["name"], "orv-lsp");
    }

    #[test]
    fn lsp_stdio_ignores_notifications_without_id() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {},
        })
        .to_string();
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        let output = lsp_stdio_response(&input).expect("stdio response");

        assert_eq!(output, "");
    }

    #[test]
    fn dap_initialize_returns_debug_capabilities() {
        let response = dap_protocol_response(&serde_json::json!({
            "seq": 1,
            "type": "request",
            "command": "initialize",
            "arguments": {},
        }));

        assert_eq!(response["type"], "response");
        assert_eq!(response["request_seq"], 1);
        assert_eq!(response["command"], "initialize");
        assert_eq!(response["success"], true);
        assert_eq!(response["body"]["supportsConfigurationDoneRequest"], true);
        assert_eq!(response["body"]["supportsTerminateRequest"], true);
        assert_eq!(response["body"]["supportsTerminateThreadsRequest"], true);
        assert_eq!(response["body"]["supportsLoadedSourcesRequest"], true);
        assert_eq!(response["body"]["supportsEvaluateForHovers"], true);
        assert_eq!(response["body"]["supportsCompletionsRequest"], true);
        assert_eq!(response["body"]["supportsBreakpointLocationsRequest"], true);
        assert_eq!(response["body"]["supportsConditionalBreakpoints"], true);
        assert_eq!(response["body"]["supportsHitConditionalBreakpoints"], true);
        assert_eq!(response["body"]["supportsFunctionBreakpoints"], true);
        assert_eq!(response["body"]["supportsDataBreakpoints"], true);
        assert_eq!(response["body"]["supportsExceptionInfoRequest"], true);
        assert_eq!(response["body"]["supportsRestartRequest"], true);
        assert_eq!(response["body"]["supportsSetVariable"], true);
        assert_eq!(response["body"]["supportsSetExpression"], true);
        assert_eq!(response["body"]["supportsModulesRequest"], true);
        assert_eq!(response["body"]["supportsGotoTargetsRequest"], true);
        assert_eq!(response["body"]["supportsStepBack"], true);
        assert_eq!(response["body"]["supportsStepInTargetsRequest"], true);
        assert_eq!(response["body"]["supportsRestartFrame"], true);
        assert_eq!(response["body"]["supportsPauseRequest"], true);
    }

    #[test]
    fn dap_set_exception_breakpoints_accepts_orv_filters() {
        let mut session = DapSession::default();

        let response = session
            .message_response(&serde_json::json!({
                "seq": 67,
                "type": "request",
                "command": "setExceptionBreakpoints",
                "arguments": {
                    "filters": ["orv.diagnostics", "orv.runtime"],
                },
            }))
            .expect("setExceptionBreakpoints response");

        assert_eq!(response["success"], true, "{response}");
        assert_eq!(response["command"], "setExceptionBreakpoints");
        assert_eq!(
            response["body"]["breakpoints"]
                .as_array()
                .expect("breakpoints")
                .len(),
            2
        );
        assert_eq!(response["body"]["breakpoints"][0]["verified"], true);
        assert_eq!(
            response["body"]["breakpoints"][0]["filter"],
            "orv.diagnostics"
        );
        assert_eq!(response["body"]["breakpoints"][1]["verified"], true);
        assert_eq!(response["body"]["breakpoints"][1]["filter"], "orv.runtime");
    }

    #[test]
    fn dap_set_breakpoints_accepts_loaded_source_reference() {
        let dir = temp_output_dir("dap-set-breakpoints-source-ref");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let answer: int = 42\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 7,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let response = session
            .message_response(&serde_json::json!({
                "seq": 8,
                "type": "request",
                "command": "setBreakpoints",
                "arguments": {
                    "source": {
                        "sourceReference": 1,
                    },
                    "breakpoints": [
                        {
                            "line": 1,
                        },
                    ],
                },
            }))
            .expect("setBreakpoints response");

        assert_eq!(response["success"], true, "{response}");
        assert_eq!(response["body"]["breakpoints"][0]["verified"], true);
        assert_eq!(response["body"]["breakpoints"][0]["line"], 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_stdio_serves_content_length_initialize_frame() {
        let body = serde_json::json!({
            "seq": 1,
            "type": "request",
            "command": "initialize",
            "arguments": {},
        })
        .to_string();
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        let output = dap_stdio_response(&input).expect("stdio response");
        let frames = protocol_frames(&output);
        let response = &frames[0];

        assert!(output.starts_with("Content-Length: "));
        assert_eq!(response["type"], "response");
        assert_eq!(response["command"], "initialize");
        assert_eq!(response["success"], true);
    }

    #[test]
    fn dap_stdio_emits_initialized_event_after_initialize() {
        let body = serde_json::json!({
            "seq": 1,
            "type": "request",
            "command": "initialize",
            "arguments": {},
        })
        .to_string();
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        let output = dap_stdio_response(&input).expect("stdio response");
        let frames = protocol_frames(&output);

        assert_eq!(frames.len(), 2, "{output}");
        assert_eq!(frames[0]["type"], "response");
        assert_eq!(frames[0]["command"], "initialize");
        assert_eq!(frames[1]["type"], "event");
        assert_eq!(frames[1]["event"], "initialized");
    }

    #[test]
    fn dap_stdio_emits_stopped_event_after_configuration_done() {
        let dir = temp_output_dir("dap-stopped-event");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let answer: int = 42\n").expect("write source");
        let input = [
            protocol_request_frame(&serde_json::json!({
                "seq": 1,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            })),
            protocol_request_frame(&serde_json::json!({
                "seq": 2,
                "type": "request",
                "command": "configurationDone",
                "arguments": {},
            })),
        ]
        .join("");

        let output = dap_stdio_response(&input).expect("stdio response");
        let frames = protocol_frames(&output);
        let stopped = frames
            .iter()
            .find(|frame| frame["type"] == "event" && frame["event"] == "stopped")
            .expect("stopped event");

        assert_eq!(stopped["body"]["reason"], "entry");
        assert_eq!(stopped["body"]["threadId"], 1);
        assert_eq!(stopped["body"]["allThreadsStopped"], false);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_stdio_emits_continued_and_terminated_events_after_continue() {
        let dir = temp_output_dir("dap-continue-events");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let answer: int = 42\n").expect("write source");
        let input = [
            protocol_request_frame(&serde_json::json!({
                "seq": 1,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            })),
            protocol_request_frame(&serde_json::json!({
                "seq": 2,
                "type": "request",
                "command": "continue",
                "arguments": {
                    "threadId": 1,
                },
            })),
        ]
        .join("");

        let output = dap_stdio_response(&input).expect("stdio response");
        let frames = protocol_frames(&output);
        let continued = frames
            .iter()
            .find(|frame| frame["type"] == "event" && frame["event"] == "continued")
            .expect("continued event");
        let terminated = frames
            .iter()
            .find(|frame| frame["type"] == "event" && frame["event"] == "terminated")
            .expect("terminated event");

        assert_eq!(continued["body"]["threadId"], 1);
        assert_eq!(continued["body"]["allThreadsContinued"], false);
        assert_eq!(terminated["body"], serde_json::json!({}));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_continue_terminates_session_state() {
        let dir = temp_output_dir("dap-continue-terminates-state");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let answer: int = 42\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 71,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let continue_response = session
            .message_response(&serde_json::json!({
                "seq": 72,
                "type": "request",
                "command": "continue",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("continue response");
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 73,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(continue_response["success"], true, "{continue_response}");
        assert_eq!(stack["success"], false, "{stack}");
        assert!(stack["message"]
            .as_str()
            .is_some_and(|message| message.contains("launch is required")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_continue_stops_at_next_verified_breakpoint_frame() {
        let dir = temp_output_dir("dap-continue-breakpoint-frame");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            "let first: int = 1\nlet middle: int = 2\nlet last: int = 3\n",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 158,
                "type": "request",
                "command": "setBreakpoints",
                "arguments": {
                    "source": {
                        "path": source.display().to_string(),
                    },
                    "breakpoints": [
                        { "line": 1 },
                        { "line": 3 },
                    ],
                },
            }))
            .expect("breakpoints response");
        session
            .message_response(&serde_json::json!({
                "seq": 159,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let first_stack = session
            .message_response(&serde_json::json!({
                "seq": 160,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("first stack response");
        session
            .message_response(&serde_json::json!({
                "seq": 161,
                "type": "request",
                "command": "continue",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("continue response");
        let events = session.drain_pending_events();
        let second_stack = session
            .message_response(&serde_json::json!({
                "seq": 162,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("second stack response");

        assert_eq!(first_stack["body"]["stackFrames"][0]["line"], 1);
        assert_eq!(second_stack["body"]["stackFrames"][0]["line"], 3);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "continued"
                && event["body"]["threadId"] == 1
        }));
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "breakpoint"
                && event["body"]["threadId"] == 1
        }));
        assert!(!events
            .iter()
            .any(|event| event["type"] == "event" && event["event"] == "terminated"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_conditional_breakpoint_skips_false_condition_frame() {
        let dir = temp_output_dir("dap-conditional-breakpoint");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            "let mut total: int = 1\ntotal = total + 4\ntotal = total + 4\n",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 204,
                "type": "request",
                "command": "setBreakpoints",
                "arguments": {
                    "source": {
                        "path": source.display().to_string(),
                    },
                    "breakpoints": [
                        {
                            "line": 2,
                            "condition": "total == 9",
                        },
                        {
                            "line": 3,
                            "condition": "total == 9",
                        },
                    ],
                },
            }))
            .expect("setBreakpoints response");
        session
            .message_response(&serde_json::json!({
                "seq": 205,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 206,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(stack["success"], true, "{stack}");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 3);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_hit_condition_breakpoint_stops_on_requested_hit() {
        let dir = temp_output_dir("dap-hit-condition-breakpoint");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"function bump(value: int): int -> {
  let result: int = value + 1
  result
}
let first: int = bump(0)
let second: int = bump(1)
",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 207,
                "type": "request",
                "command": "setBreakpoints",
                "arguments": {
                    "source": {
                        "path": source.display().to_string(),
                    },
                    "breakpoints": [
                        {
                            "line": 2,
                            "hitCondition": "2",
                        },
                    ],
                },
            }))
            .expect("setBreakpoints response");
        session
            .message_response(&serde_json::json!({
                "seq": 208,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let locals = session
            .message_response(&serde_json::json!({
                "seq": 209,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": 2,
                },
            }))
            .expect("locals response");

        let vars = locals["body"]["variables"].as_array().expect("locals");
        assert!(
            vars.iter()
                .any(|var| var["name"] == "result" && var["value"] == "2"),
            "{locals}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_reverse_continue_stops_at_previous_verified_breakpoint_frame() {
        let dir = temp_output_dir("dap-reverse-continue");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            "let first: int = 1\nlet middle: int = 2\nlet last: int = 3\n",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 181,
                "type": "request",
                "command": "setBreakpoints",
                "arguments": {
                    "source": {
                        "path": source.display().to_string(),
                    },
                    "breakpoints": [
                        { "line": 1 },
                        { "line": 3 },
                    ],
                },
            }))
            .expect("breakpoints response");
        session
            .message_response(&serde_json::json!({
                "seq": 182,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 183,
                "type": "request",
                "command": "continue",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("continue response");
        let _ = session.drain_pending_events();
        let reverse = session
            .message_response(&serde_json::json!({
                "seq": 184,
                "type": "request",
                "command": "reverseContinue",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("reverseContinue response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 185,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(reverse["success"], true, "{reverse}");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 1);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "breakpoint"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_function_breakpoint_stops_inside_named_function() {
        let dir = temp_output_dir("dap-function-breakpoint");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
        )
        .expect("write source");
        let mut session = DapSession::default();

        let breakpoints = session
            .message_response(&serde_json::json!({
                "seq": 190,
                "type": "request",
                "command": "setFunctionBreakpoints",
                "arguments": {
                    "breakpoints": [
                        { "name": "add" },
                    ],
                },
            }))
            .expect("setFunctionBreakpoints response");
        session
            .message_response(&serde_json::json!({
                "seq": 191,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 192,
                "type": "request",
                "command": "configurationDone",
                "arguments": {},
            }))
            .expect("configurationDone response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 193,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(breakpoints["success"], true, "{breakpoints}");
        assert_eq!(breakpoints["body"]["breakpoints"][0]["verified"], true);
        assert_eq!(stack["success"], true, "{stack}");
        assert_eq!(stack["body"]["stackFrames"][0]["name"], "add");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "function breakpoint"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_continue_stops_at_next_function_breakpoint_frame() {
        let dir = temp_output_dir("dap-continue-function-breakpoint");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"let first: int = 1
function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 194,
                "type": "request",
                "command": "setBreakpoints",
                "arguments": {
                    "source": {
                        "path": source.display().to_string(),
                    },
                    "breakpoints": [
                        { "line": 1 },
                    ],
                },
            }))
            .expect("setBreakpoints response");
        session
            .message_response(&serde_json::json!({
                "seq": 195,
                "type": "request",
                "command": "setFunctionBreakpoints",
                "arguments": {
                    "breakpoints": [
                        { "name": "add" },
                    ],
                },
            }))
            .expect("setFunctionBreakpoints response");
        session
            .message_response(&serde_json::json!({
                "seq": 196,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 197,
                "type": "request",
                "command": "continue",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("continue response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 198,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(stack["success"], true, "{stack}");
        assert_eq!(stack["body"]["stackFrames"][0]["name"], "add");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 3);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "function breakpoint"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_data_breakpoint_stops_when_local_changes() {
        let dir = temp_output_dir("dap-data-breakpoint");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let mut total: int = 1\ntotal = total + 4\n")
            .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 199,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let info = session
            .message_response(&serde_json::json!({
                "seq": 200,
                "type": "request",
                "command": "dataBreakpointInfo",
                "arguments": {
                    "variablesReference": 2,
                    "name": "total",
                },
            }))
            .expect("dataBreakpointInfo response");
        let data_id = info["body"]["dataId"].as_str().expect("data id");
        let set_data = session
            .message_response(&serde_json::json!({
                "seq": 201,
                "type": "request",
                "command": "setDataBreakpoints",
                "arguments": {
                    "breakpoints": [
                        {
                            "dataId": data_id,
                            "accessType": "write",
                        },
                    ],
                },
            }))
            .expect("setDataBreakpoints response");
        session
            .message_response(&serde_json::json!({
                "seq": 202,
                "type": "request",
                "command": "continue",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("continue response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 203,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(info["success"], true, "{info}");
        assert_eq!(info["body"]["dataId"], "local:total");
        assert_eq!(set_data["success"], true, "{set_data}");
        assert_eq!(set_data["body"]["breakpoints"][0]["verified"], true);
        assert_eq!(stack["success"], true, "{stack}");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "data breakpoint"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_stdio_emits_output_event_for_reference_stdout_after_launch() {
        let dir = temp_output_dir("dap-output-event");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "@out \"debug-ready\"\n").expect("write source");
        let input = protocol_request_frame(&serde_json::json!({
            "seq": 55,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }));

        let output = dap_stdio_response(&input).expect("stdio response");
        let frames = protocol_frames(&output);
        let output_event = frames
            .iter()
            .find(|frame| frame["type"] == "event" && frame["event"] == "output")
            .expect("output event");

        assert_eq!(output_event["body"]["category"], "stdout");
        assert_eq!(output_event["body"]["output"], "debug-ready\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_stdio_emits_stderr_output_event_for_runtime_error_after_launch() {
        let dir = temp_output_dir("dap-error-output-event");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "throw \"panic!\"\n").expect("write source");
        let input = protocol_request_frame(&serde_json::json!({
            "seq": 56,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }));

        let output = dap_stdio_response(&input).expect("stdio response");
        let frames = protocol_frames(&output);
        let output_event = frames
            .iter()
            .find(|frame| frame["type"] == "event" && frame["event"] == "output")
            .expect("output event");

        assert_eq!(frames[0]["body"]["runtime"]["status"], "error");
        assert_eq!(output_event["body"]["category"], "stderr");
        assert!(output_event["body"]["output"]
            .as_str()
            .is_some_and(|output| output.contains("panic!")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_next_advances_to_next_executable_line_and_queues_stopped_event() {
        let dir = temp_output_dir("dap-next-line");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\n\nlet second: int = 2\n")
            .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 48,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let first_stack = session
            .message_response(&serde_json::json!({
                "seq": 49,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("first stack response");
        let next = session
            .message_response(&serde_json::json!({
                "seq": 50,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let events = session.drain_pending_events();
        let second_stack = session
            .message_response(&serde_json::json!({
                "seq": 51,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("second stack response");

        assert_eq!(first_stack["body"]["stackFrames"][0]["line"], 1);
        assert_eq!(next["success"], true, "{next}");
        assert_eq!(next["body"], serde_json::json!({}));
        assert_eq!(second_stack["body"]["stackFrames"][0]["line"], 3);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "step"
                && event["body"]["threadId"] == 1
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_live_launch_defers_output_until_next_step() {
        let dir = temp_output_dir("dap-live-launch");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\n@out \"second\"\n").expect("write source");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 208,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                    "live": true,
                },
            }))
            .expect("launch response");
        let launch_events = session.drain_pending_events();
        let first_stack = session
            .message_response(&serde_json::json!({
                "seq": 209,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("first stack response");
        let next = session
            .message_response(&serde_json::json!({
                "seq": 210,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let next_events = session.drain_pending_events();

        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(launch["body"]["runtime"]["status"], "running");
        assert_eq!(launch["body"]["runtime"]["stdout"], "");
        assert!(launch_events
            .iter()
            .all(|event| { event["event"] != "output" || event["body"]["output"] != "second\n" }));
        assert_eq!(first_stack["body"]["stackFrames"][0]["line"], 1);
        assert_eq!(next["success"], true, "{next}");
        assert!(next_events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "output"
                && event["body"]["category"] == "stdout"
                && event["body"]["output"] == "second\n"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_live_continue_stops_at_breakpoint_before_program_end() {
        let dir = temp_output_dir("dap-live-continue-breakpoint");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            "let first: int = 1\n@out \"middle\"\nlet third: int = 3\nlet done: int = 4\n",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 211,
                "type": "request",
                "command": "setBreakpoints",
                "arguments": {
                    "source": {
                        "path": source.display().to_string(),
                    },
                    "breakpoints": [
                        {
                            "line": 3,
                        },
                    ],
                },
            }))
            .expect("setBreakpoints response");
        let launch = session
            .message_response(&serde_json::json!({
                "seq": 212,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                    "live": true,
                },
            }))
            .expect("launch response");
        let _ = session.drain_pending_events();
        let continue_response = session
            .message_response(&serde_json::json!({
                "seq": 213,
                "type": "request",
                "command": "continue",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("continue response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 214,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(launch["body"]["runtime"]["status"], "running");
        assert_eq!(continue_response["success"], true, "{continue_response}");
        assert_eq!(stack["success"], true, "{stack}");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 3);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "breakpoint"
        }));
        assert!(events.iter().all(|event| event["event"] != "terminated"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_live_step_in_rejects_target_id() {
        let dir = temp_output_dir("dap-live-step-in-target");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 218,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                    "live": true,
                },
            }))
            .expect("launch response");
        let step_in = session
            .message_response(&serde_json::json!({
                "seq": 219,
                "type": "request",
                "command": "stepIn",
                "arguments": {
                    "threadId": 1,
                    "targetId": 1_000_000,
                },
            }))
            .expect("stepIn response");
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 220,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(step_in["success"], false, "{step_in}");
        assert!(step_in["message"]
            .as_str()
            .is_some_and(|message| message.contains("targetId is unavailable in live debug mode")));
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_long_running_server_state_uses_server_frame_without_runtime() {
        let dir = temp_output_dir("dap-long-running-server-state");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"@server {
  @listen 0
  @route GET /ping { @respond 200 { ok: true } }
}
",
        )
        .expect("write source");
        let loaded = orv_project::load_project(&source).expect("load project");
        let resolved = orv_resolve::resolve(&loaded.program);
        let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
        let sources = loaded
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                dap_source_info(&file.path, u64::try_from(index + 1).unwrap_or(u64::MAX))
            })
            .collect::<Vec<_>>();

        let (runtime, frames) =
            dap_long_running_runtime_state(&lowered.program, &loaded.files, &sources);

        assert!(dap_program_has_long_running_runtime(&lowered.program));
        assert_eq!(runtime.status, "paused");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].line, 1);
        assert_eq!(frames[0].stack[0].name, "server runtime");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_launch_server_program_reports_paused_long_running_runtime() {
        let dir = temp_output_dir("dap-server-long-running-launch");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"@server {
  @listen 0
  @route GET /ping { @respond 200 { ok: true } }
}
",
        )
        .expect("write source");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 221,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 222,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(launch["body"]["runtime"]["status"], "paused");
        assert_eq!(stack["success"], true, "{stack}");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 1);
        assert_eq!(stack["body"]["stackFrames"][0]["name"], "server runtime");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_long_running_continue_and_pause_queue_events() {
        let dir = temp_output_dir("dap-server-long-running-pause");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"@server {
  @listen 0
  @route GET /ping { @respond 200 { ok: true } }
}
",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 223,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let _ = session.drain_pending_events();
        let continue_response = session
            .message_response(&serde_json::json!({
                "seq": 224,
                "type": "request",
                "command": "continue",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("continue response");
        let continue_events = session.drain_pending_events();
        let pause = session
            .message_response(&serde_json::json!({
                "seq": 225,
                "type": "request",
                "command": "pause",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("pause response");
        let pause_events = session.drain_pending_events();

        assert_eq!(continue_response["success"], true, "{continue_response}");
        assert!(continue_events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "continued"
                && event["body"]["threadId"] == 1
        }));
        assert_eq!(pause["success"], true, "{pause}");
        assert!(pause_events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "pause"
                && event["body"]["threadId"] == 1
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_long_running_exposes_async_pause_resume_state() {
        let dir = temp_output_dir("dap-server-async-state");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            "@server { @listen 0 @route GET /ping { @respond 200 { ok: true } } }\n",
        )
        .expect("write source");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 226,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 227,
                "type": "request",
                "command": "continue",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("continue response");
        session
            .message_response(&serde_json::json!({
                "seq": 228,
                "type": "request",
                "command": "pause",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("pause response");
        let variables = session
            .message_response(&serde_json::json!({
                "seq": 229,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": 1,
                },
            }))
            .expect("variables response");
        let async_state = session
            .message_response(&serde_json::json!({
                "seq": 230,
                "type": "request",
                "command": "evaluate",
                "arguments": {
                    "expression": "runtimeAsyncState",
                },
            }))
            .expect("evaluate response");
        let completions = session
            .message_response(&serde_json::json!({
                "seq": 231,
                "type": "request",
                "command": "completions",
                "arguments": {
                    "text": "runtimeA",
                    "column": 9,
                    "line": 1,
                },
            }))
            .expect("completions response");

        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(launch["body"]["runtime"]["async"]["kind"], "server");
        assert_eq!(launch["body"]["runtime"]["async"]["state"], "paused");
        assert!(variables["body"]["variables"]
            .as_array()
            .expect("variables")
            .iter()
            .any(
                |variable| variable["name"] == "runtimeAsyncState" && variable["value"] == "paused"
            ));
        assert!(variables["body"]["variables"]
            .as_array()
            .expect("variables")
            .iter()
            .any(|variable| variable["name"] == "runtimeResumeCount" && variable["value"] == "1"));
        assert!(variables["body"]["variables"]
            .as_array()
            .expect("variables")
            .iter()
            .any(|variable| variable["name"] == "runtimePauseCount" && variable["value"] == "1"));
        assert_eq!(async_state["success"], true, "{async_state}");
        assert_eq!(async_state["body"]["result"], "paused");
        assert!(completions["body"]["targets"]
            .as_array()
            .expect("completion targets")
            .iter()
            .any(|target| target["label"] == "runtimeAsyncState" && target["type"] == "property"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_step_back_moves_to_previous_runtime_frame() {
        let dir = temp_output_dir("dap-step-back");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 186,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 187,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let _ = session.drain_pending_events();
        let step_back = session
            .message_response(&serde_json::json!({
                "seq": 188,
                "type": "request",
                "command": "stepBack",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stepBack response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 189,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(step_back["success"], true, "{step_back}");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 1);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "step"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_step_out_leaves_current_function_frame() {
        let dir = temp_output_dir("dap-step-out");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
let done: int = total
",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 190,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 191,
                "type": "request",
                "command": "stepIn",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stepIn response");
        let inside_stack = session
            .message_response(&serde_json::json!({
                "seq": 192,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("inside stack response");
        let step_out = session
            .message_response(&serde_json::json!({
                "seq": 193,
                "type": "request",
                "command": "stepOut",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stepOut response");
        let events = session.drain_pending_events();
        let outside_stack = session
            .message_response(&serde_json::json!({
                "seq": 194,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("outside stack response");

        assert_eq!(inside_stack["body"]["stackFrames"][0]["name"], "add");
        assert_eq!(inside_stack["body"]["stackFrames"][0]["line"], 2);
        assert_eq!(step_out["success"], true, "{step_out}");
        assert_eq!(outside_stack["body"]["stackFrames"][0]["name"], "orv entry");
        assert_eq!(outside_stack["body"]["stackFrames"][0]["line"], 5);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "step"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_next_steps_over_function_call_frames() {
        let dir = temp_output_dir("dap-next-step-over");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
let done: int = total
",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 195,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let next = session
            .message_response(&serde_json::json!({
                "seq": 196,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 197,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(next["success"], true, "{next}");
        assert_eq!(stack["body"]["stackFrames"][0]["name"], "orv entry");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 5);
        assert_eq!(stack["body"]["totalFrames"], 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_step_in_targets_enter_selected_function_frame() {
        let dir = temp_output_dir("dap-step-in-targets");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 198,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let targets = session
            .message_response(&serde_json::json!({
                "seq": 199,
                "type": "request",
                "command": "stepInTargets",
                "arguments": {
                    "frameId": 1,
                },
            }))
            .expect("stepInTargets response");
        let target_id = targets["body"]["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .find(|target| target["label"] == "add")
            .and_then(|target| target["id"].as_u64())
            .expect("add target id");
        let step_in = session
            .message_response(&serde_json::json!({
                "seq": 200,
                "type": "request",
                "command": "stepIn",
                "arguments": {
                    "threadId": 1,
                    "targetId": target_id,
                },
            }))
            .expect("stepIn response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 201,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(targets["success"], true, "{targets}");
        assert_eq!(step_in["success"], true, "{step_in}");
        assert_eq!(stack["body"]["stackFrames"][0]["name"], "add");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "step"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_restart_frame_rewinds_current_function_frame() {
        let dir = temp_output_dir("dap-restart-frame");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 202,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 203,
                "type": "request",
                "command": "stepIn",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("first stepIn response");
        session
            .message_response(&serde_json::json!({
                "seq": 204,
                "type": "request",
                "command": "stepIn",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("second stepIn response");
        let before = session
            .message_response(&serde_json::json!({
                "seq": 205,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("before stack response");
        let restart_frame = session
            .message_response(&serde_json::json!({
                "seq": 206,
                "type": "request",
                "command": "restartFrame",
                "arguments": {
                    "frameId": 1,
                },
            }))
            .expect("restartFrame response");
        let events = session.drain_pending_events();
        let after = session
            .message_response(&serde_json::json!({
                "seq": 207,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("after stack response");

        assert_eq!(before["body"]["stackFrames"][0]["name"], "add");
        assert_eq!(before["body"]["stackFrames"][0]["line"], 3);
        assert_eq!(restart_frame["success"], true, "{restart_frame}");
        assert_eq!(after["body"]["stackFrames"][0]["name"], "add");
        assert_eq!(after["body"]["stackFrames"][0]["line"], 2);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "restart"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_next_queues_output_for_reached_runtime_frame() {
        let dir = temp_output_dir("dap-next-output-frame");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\n@out \"second\"\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 166,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        assert!(session.drain_pending_events().is_empty());
        session
            .message_response(&serde_json::json!({
                "seq": 167,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let events = session.drain_pending_events();

        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "output"
                && event["body"]["category"] == "stdout"
                && event["body"]["output"] == "second\n"
        }));
        assert!(events
            .iter()
            .any(|event| event["type"] == "event" && event["event"] == "stopped"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_stack_trace_names_runtime_function_frame() {
        let dir = temp_output_dir("dap-function-stack-frame");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 163,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 164,
                "type": "request",
                "command": "stepIn",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stepIn response");
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 165,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(stack["success"], true, "{stack}");
        assert_eq!(stack["body"]["stackFrames"][0]["name"], "add");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
        assert_eq!(stack["body"]["stackFrames"][1]["name"], "orv entry");
        assert_eq!(stack["body"]["totalFrames"], 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_next_after_last_executable_line_terminates_session() {
        let dir = temp_output_dir("dap-next-terminate");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let only: int = 1\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 68,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let next = session
            .message_response(&serde_json::json!({
                "seq": 69,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 70,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(next["success"], true, "{next}");
        assert!(events
            .iter()
            .any(|event| { event["type"] == "event" && event["event"] == "terminated" }));
        assert_eq!(stack["success"], false, "{stack}");
        assert!(stack["message"]
            .as_str()
            .is_some_and(|message| message.contains("launch is required")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_pause_keeps_current_line_and_queues_pause_stopped_event() {
        let dir = temp_output_dir("dap-pause-event");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let answer: int = 42\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 52,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let pause = session
            .message_response(&serde_json::json!({
                "seq": 53,
                "type": "request",
                "command": "pause",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("pause response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 54,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(pause["success"], true, "{pause}");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 1);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "pause"
                && event["body"]["threadId"] == 1
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_terminate_threads_clears_launch_and_queues_terminated_event() {
        let dir = temp_output_dir("dap-terminate-threads");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let answer: int = 42\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 183,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let terminate_threads = session
            .message_response(&serde_json::json!({
                "seq": 184,
                "type": "request",
                "command": "terminateThreads",
                "arguments": {
                    "threadIds": [1],
                },
            }))
            .expect("terminateThreads response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 185,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(terminate_threads["success"], true, "{terminate_threads}");
        assert!(events
            .iter()
            .any(|event| { event["type"] == "event" && event["event"] == "terminated" }));
        assert_eq!(stack["success"], false, "{stack}");
        assert!(stack["message"]
            .as_str()
            .is_some_and(|message| message.contains("launch is required")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_restart_reloads_current_program_and_resets_stopped_line() {
        let dir = temp_output_dir("dap-restart");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 78,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 79,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let moved_stack = session
            .message_response(&serde_json::json!({
                "seq": 80,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("moved stack response");
        let restart = session
            .message_response(&serde_json::json!({
                "seq": 81,
                "type": "request",
                "command": "restart",
                "arguments": {},
            }))
            .expect("restart response");
        let restarted_stack = session
            .message_response(&serde_json::json!({
                "seq": 82,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("restarted stack response");

        assert_eq!(moved_stack["body"]["stackFrames"][0]["line"], 2);
        assert_eq!(restart["success"], true, "{restart}");
        assert_eq!(restarted_stack["body"]["stackFrames"][0]["line"], 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_restart_preserves_live_launch_mode() {
        let dir = temp_output_dir("dap-restart-live");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\n@out \"after\"\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 215,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                    "live": true,
                },
            }))
            .expect("launch response");
        let _ = session.drain_pending_events();
        let restart = session
            .message_response(&serde_json::json!({
                "seq": 216,
                "type": "request",
                "command": "restart",
                "arguments": {},
            }))
            .expect("restart response");
        let restart_events = session.drain_pending_events();
        let restarted_stack = session
            .message_response(&serde_json::json!({
                "seq": 217,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("restarted stack response");

        assert_eq!(restart["success"], true, "{restart}");
        assert_eq!(restart["body"]["runtime"]["status"], "running");
        assert_eq!(restart["body"]["runtime"]["stdout"], "");
        assert_eq!(restarted_stack["body"]["stackFrames"][0]["line"], 1);
        assert!(restart_events
            .iter()
            .all(|event| { event["event"] != "output" || event["body"]["output"] != "after\n" }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_launch_threads_and_stacktrace_use_entry_source() {
        let dir = temp_output_dir("dap-launch");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let answer: int = 42\n").expect("write source");
        let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 2,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let threads = session
            .message_response(&serde_json::json!({
                "seq": 3,
                "type": "request",
                "command": "threads",
            }))
            .expect("threads response");
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 4,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(launch["body"]["projectGraphNodes"], 1);
        assert_eq!(threads["body"]["threads"][0]["id"], 1);
        assert_eq!(stack["success"], true, "{stack}");
        assert_eq!(stack["body"]["totalFrames"], 1);
        let frame = &stack["body"]["stackFrames"][0];
        assert_eq!(frame["id"], 1);
        assert_eq!(frame["line"], 1);
        assert_eq!(frame["column"], 1);
        assert_eq!(
            frame["source"]["path"],
            canonical_source.display().to_string()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_loaded_sources_returns_project_files_after_launch() {
        let dir = temp_output_dir("dap-loaded-sources");
        let models = dir.join("models");
        std::fs::create_dir_all(&models).expect("create models dir");
        let source = dir.join("app.orv");
        let imported = models.join("user.orv");
        std::fs::write(
            &source,
            "import models.user.User\nlet u: User = { id: 1 }\n",
        )
        .expect("write source");
        std::fs::write(&imported, "pub struct User { id: int }\n").expect("write imported");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 30,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let loaded = session
            .message_response(&serde_json::json!({
                "seq": 31,
                "type": "request",
                "command": "loadedSources",
                "arguments": {},
            }))
            .expect("loadedSources response");

        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(loaded["success"], true, "{loaded}");
        let sources = loaded["body"]["sources"].as_array().expect("sources");
        assert!(sources
            .iter()
            .any(|item| item["name"] == "app.orv" && item["path"].as_str().is_some()));
        assert!(sources
            .iter()
            .any(|item| item["name"] == "user.orv" && item["path"].as_str().is_some()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_modules_returns_project_sources_after_launch() {
        let dir = temp_output_dir("dap-modules");
        let models = dir.join("models");
        std::fs::create_dir_all(&models).expect("create models dir");
        let source = dir.join("app.orv");
        let imported = models.join("user.orv");
        std::fs::write(
            &source,
            "import models.user.User\nlet u: User = { id: 1 }\n",
        )
        .expect("write source");
        std::fs::write(&imported, "pub struct User { id: int }\n").expect("write imported");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 175,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let modules = session
            .message_response(&serde_json::json!({
                "seq": 176,
                "type": "request",
                "command": "modules",
                "arguments": {
                    "startModule": 0,
                    "moduleCount": 1,
                },
            }))
            .expect("modules response");

        assert_eq!(modules["success"], true, "{modules}");
        assert_eq!(modules["body"]["totalModules"], 2);
        let items = modules["body"]["modules"].as_array().expect("modules");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "app.orv");
        assert_eq!(items[0]["id"], 1);
        assert_eq!(items[0]["isUserCode"], true);
        assert!(items[0]["path"].as_str().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_breakpoint_locations_return_project_graph_lines() {
        let dir = temp_output_dir("dap-breakpoint-locations");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User { id: int }

function greet(user: User): string -> "hello"
"#,
        )
        .expect("write source");
        let mut session = DapSession::default();

        let response = session
            .message_response(&serde_json::json!({
                "seq": 51,
                "type": "request",
                "command": "breakpointLocations",
                "arguments": {
                    "source": {
                        "path": format!("file://{}", source.display()),
                    },
                    "line": 1,
                    "endLine": 3,
                },
            }))
            .expect("breakpointLocations response");

        assert_eq!(response["success"], true, "{response}");
        let breakpoints = response["body"]["breakpoints"]
            .as_array()
            .expect("breakpoint locations");
        assert!(breakpoints
            .iter()
            .any(|breakpoint| breakpoint["line"] == 1 && breakpoint["column"] == 1));
        assert!(breakpoints
            .iter()
            .any(|breakpoint| breakpoint["line"] == 3 && breakpoint["column"] == 1));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_goto_targets_and_goto_move_to_executable_frame() {
        let dir = temp_output_dir("dap-goto");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\n\nlet third: int = 3\n")
            .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 177,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let targets = session
            .message_response(&serde_json::json!({
                "seq": 178,
                "type": "request",
                "command": "gotoTargets",
                "arguments": {
                    "source": {
                        "path": source.display().to_string(),
                    },
                    "line": 1,
                    "endLine": 3,
                },
            }))
            .expect("gotoTargets response");
        assert_eq!(targets["success"], true, "{targets}");
        let target_id = targets["body"]["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .find(|target| target["line"] == 3)
            .and_then(|target| target["id"].as_u64())
            .expect("line 3 target");
        let goto = session
            .message_response(&serde_json::json!({
                "seq": 179,
                "type": "request",
                "command": "goto",
                "arguments": {
                    "threadId": 1,
                    "targetId": target_id,
                },
            }))
            .expect("goto response");
        let events = session.drain_pending_events();
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 180,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        let target_lines = targets["body"]["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .map(|target| target["line"].as_u64().expect("line"))
            .collect::<Vec<_>>();
        assert_eq!(target_lines, vec![1, 3]);
        assert_eq!(goto["success"], true, "{goto}");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 3);
        assert!(events.iter().any(|event| {
            event["type"] == "event"
                && event["event"] == "stopped"
                && event["body"]["reason"] == "goto"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_exception_info_returns_launch_runtime_status() {
        let dir = temp_output_dir("dap-exception-info");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let bad: int = \"wrong\"\n").expect("write source");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 52,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let exception = session
            .message_response(&serde_json::json!({
                "seq": 53,
                "type": "request",
                "command": "exceptionInfo",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("exceptionInfo response");

        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(launch["body"]["runtime"]["status"], "diagnostics");
        assert_eq!(exception["success"], true, "{exception}");
        assert_eq!(exception["body"]["exceptionId"], "orv.diagnostics");
        assert_eq!(exception["body"]["description"], "diagnostics present");
        assert_eq!(exception["body"]["breakMode"], "always");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_source_returns_loaded_file_content_after_launch() {
        let dir = temp_output_dir("dap-source");
        let models = dir.join("models");
        std::fs::create_dir_all(&models).expect("create models dir");
        let source = dir.join("app.orv");
        let imported = models.join("user.orv");
        let imported_source = "pub struct User { id: int }\n";
        std::fs::write(
            &source,
            "import models.user.User\nlet u: User = { id: 1 }\n",
        )
        .expect("write source");
        std::fs::write(&imported, imported_source).expect("write imported");
        let canonical_imported = std::fs::canonicalize(&imported).expect("canonical imported");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 32,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let source_response = session
            .message_response(&serde_json::json!({
                "seq": 33,
                "type": "request",
                "command": "source",
                "arguments": {
                    "source": {
                        "path": canonical_imported.display().to_string(),
                    },
                },
            }))
            .expect("source response");

        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(source_response["success"], true, "{source_response}");
        assert_eq!(source_response["body"]["content"], imported_source);
        assert_eq!(source_response["body"]["mimeType"], "text/x-orv");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_source_returns_content_by_loaded_source_reference() {
        let dir = temp_output_dir("dap-source-reference");
        let models = dir.join("models");
        std::fs::create_dir_all(&models).expect("create models dir");
        let source = dir.join("app.orv");
        let imported = models.join("user.orv");
        let imported_source = "pub struct User { id: int }\n";
        std::fs::write(
            &source,
            "import models.user.User\nlet u: User = { id: 1 }\n",
        )
        .expect("write source");
        std::fs::write(&imported, imported_source).expect("write imported");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 34,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let loaded = session
            .message_response(&serde_json::json!({
                "seq": 35,
                "type": "request",
                "command": "loadedSources",
                "arguments": {},
            }))
            .expect("loadedSources response");
        let user_reference = loaded["body"]["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .find(|item| item["name"] == "user.orv")
            .and_then(|item| item["sourceReference"].as_u64())
            .expect("user source reference");
        let source_response = session
            .message_response(&serde_json::json!({
                "seq": 36,
                "type": "request",
                "command": "source",
                "arguments": {
                    "sourceReference": user_reference,
                },
            }))
            .expect("source response");

        assert_eq!(launch["success"], true, "{launch}");
        assert!(user_reference > 0);
        assert_eq!(source_response["success"], true, "{source_response}");
        assert_eq!(source_response["body"]["content"], imported_source);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_set_breakpoints_and_stacktrace_use_verified_breakpoint_line() {
        let dir = temp_output_dir("dap-breakpoints");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
        let mut session = DapSession::default();

        let breakpoints = session
            .message_response(&serde_json::json!({
                "seq": 5,
                "type": "request",
                "command": "setBreakpoints",
                "arguments": {
                    "source": {
                        "path": source.display().to_string(),
                    },
                    "breakpoints": [
                        { "line": 2 }
                    ],
                },
            }))
            .expect("breakpoints response");
        let launch = session
            .message_response(&serde_json::json!({
                "seq": 6,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let stack = session
            .message_response(&serde_json::json!({
                "seq": 7,
                "type": "request",
                "command": "stackTrace",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("stack response");

        assert_eq!(breakpoints["success"], true, "{breakpoints}");
        assert_eq!(breakpoints["body"]["breakpoints"][0]["verified"], true);
        assert_eq!(breakpoints["body"]["breakpoints"][0]["line"], 2);
        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_set_breakpoints_rejects_non_executable_lines() {
        let dir = temp_output_dir("dap-breakpoint-verify");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\n\nlet second: int = 2\n")
            .expect("write source");
        let mut session = DapSession::default();

        let breakpoints = session
            .message_response(&serde_json::json!({
                "seq": 47,
                "type": "request",
                "command": "setBreakpoints",
                "arguments": {
                    "source": {
                        "path": source.display().to_string(),
                    },
                    "breakpoints": [
                        { "line": 2 },
                        { "line": 3 }
                    ],
                },
            }))
            .expect("breakpoints response");

        assert_eq!(breakpoints["success"], true, "{breakpoints}");
        assert_eq!(breakpoints["body"]["breakpoints"][0]["verified"], false);
        assert_eq!(
            breakpoints["body"]["breakpoints"][0]["message"],
            "no executable ORV node on this line"
        );
        assert_eq!(breakpoints["body"]["breakpoints"][1]["verified"], true);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_scopes_and_variables_expose_project_launch_state() {
        let dir = temp_output_dir("dap-variables");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let answer: int = 42\n").expect("write source");
        let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 8,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let scopes = session
            .message_response(&serde_json::json!({
                "seq": 9,
                "type": "request",
                "command": "scopes",
                "arguments": {
                    "frameId": 1,
                },
            }))
            .expect("scopes response");
        let variables = session
            .message_response(&serde_json::json!({
                "seq": 10,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": 1,
                },
            }))
            .expect("variables response");

        assert_eq!(scopes["success"], true, "{scopes}");
        assert_eq!(scopes["body"]["scopes"][0]["name"], "Project");
        assert_eq!(scopes["body"]["scopes"][0]["variablesReference"], 1);
        let vars = variables["body"]["variables"]
            .as_array()
            .expect("variables");
        assert!(vars.iter().any(|var| {
            var["name"] == "entry" && var["value"] == canonical_source.display().to_string()
        }));
        assert!(vars
            .iter()
            .any(|var| var["name"] == "projectGraphNodes" && var["value"] == "1"));
        assert!(vars
            .iter()
            .any(|var| var["name"] == "diagnostics" && var["value"] == "0"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_variables_expose_top_level_locals() {
        let dir = temp_output_dir("dap-locals");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            "let answer: int = 42\nconst greeting = \"hello\"\nlet ready = true\n",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 41,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let scopes = session
            .message_response(&serde_json::json!({
                "seq": 42,
                "type": "request",
                "command": "scopes",
                "arguments": {
                    "frameId": 1,
                },
            }))
            .expect("scopes response");
        let locals_ref = scopes["body"]["scopes"]
            .as_array()
            .expect("scopes")
            .iter()
            .find(|scope| scope["name"] == "Locals")
            .and_then(|scope| scope["variablesReference"].as_u64())
            .expect("locals scope");
        session
            .message_response(&serde_json::json!({
                "seq": 43,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("first next response");
        session
            .message_response(&serde_json::json!({
                "seq": 44,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("second next response");
        let locals = session
            .message_response(&serde_json::json!({
                "seq": 45,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": locals_ref,
                },
            }))
            .expect("locals response");

        assert_eq!(locals_ref, 2);
        assert_eq!(locals["success"], true, "{locals}");
        let vars = locals["body"]["variables"].as_array().expect("locals");
        assert!(vars
            .iter()
            .any(|var| var["name"] == "answer" && var["value"] == "42" && var["type"] == "int"));
        assert!(vars.iter().any(|var| {
            var["name"] == "greeting" && var["value"] == "\"hello\"" && var["type"] == "string"
        }));
        assert!(vars
            .iter()
            .any(|var| var["name"] == "ready" && var["value"] == "true" && var["type"] == "bool"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_set_variable_updates_current_local_and_evaluate() {
        let dir = temp_output_dir("dap-set-variable");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let answer: int = 42\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 168,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let set_variable = session
            .message_response(&serde_json::json!({
                "seq": 169,
                "type": "request",
                "command": "setVariable",
                "arguments": {
                    "variablesReference": 2,
                    "name": "answer",
                    "value": "99",
                },
            }))
            .expect("setVariable response");
        let locals = session
            .message_response(&serde_json::json!({
                "seq": 170,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": 2,
                },
            }))
            .expect("locals response");
        let evaluate = session
            .message_response(&serde_json::json!({
                "seq": 171,
                "type": "request",
                "command": "evaluate",
                "arguments": {
                    "expression": "answer",
                    "context": "repl",
                },
            }))
            .expect("evaluate response");

        assert_eq!(set_variable["success"], true, "{set_variable}");
        assert_eq!(set_variable["body"]["value"], "99");
        assert_eq!(set_variable["body"]["type"], "int");
        let vars = locals["body"]["variables"].as_array().expect("locals");
        assert!(vars
            .iter()
            .any(|var| var["name"] == "answer" && var["value"] == "99" && var["type"] == "int"));
        assert_eq!(evaluate["body"]["result"], "99");
        assert_eq!(evaluate["body"]["type"], "int");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_set_expression_updates_current_local() {
        let dir = temp_output_dir("dap-set-expression");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let name = \"Ada\"\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 172,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let set_expression = session
            .message_response(&serde_json::json!({
                "seq": 173,
                "type": "request",
                "command": "setExpression",
                "arguments": {
                    "expression": "name",
                    "value": "\"Grace\"",
                    "frameId": 1,
                },
            }))
            .expect("setExpression response");
        let evaluate = session
            .message_response(&serde_json::json!({
                "seq": 174,
                "type": "request",
                "command": "evaluate",
                "arguments": {
                    "expression": "name",
                    "context": "repl",
                },
            }))
            .expect("evaluate response");

        assert_eq!(set_expression["success"], true, "{set_expression}");
        assert_eq!(set_expression["body"]["value"], "\"Grace\"");
        assert_eq!(set_expression["body"]["type"], "string");
        assert_eq!(evaluate["body"]["result"], "\"Grace\"");
        assert_eq!(evaluate["body"]["type"], "string");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_evaluate_and_completions_include_top_level_locals() {
        let dir = temp_output_dir("dap-local-evaluate");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let answer: int = 42\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 44,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let evaluate = session
            .message_response(&serde_json::json!({
                "seq": 45,
                "type": "request",
                "command": "evaluate",
                "arguments": {
                    "expression": "answer",
                    "context": "repl",
                },
            }))
            .expect("evaluate response");
        let completions = session
            .message_response(&serde_json::json!({
                "seq": 46,
                "type": "request",
                "command": "completions",
                "arguments": {
                    "text": "ans",
                    "column": 4,
                    "line": 1,
                },
            }))
            .expect("completions response");

        assert_eq!(evaluate["success"], true, "{evaluate}");
        assert_eq!(evaluate["body"]["result"], "42");
        assert_eq!(evaluate["body"]["type"], "int");
        let targets = completions["body"]["targets"]
            .as_array()
            .expect("completion targets");
        assert!(targets
            .iter()
            .any(|target| target["label"] == "answer" && target["type"] == "variable"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_locals_use_runtime_values_from_function_calls() {
        let dir = temp_output_dir("dap-runtime-call-locals");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            "function add(a: int, b: int): int -> a + b\nlet total: int = add(2, 3)\n",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 151,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 152,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let locals = session
            .message_response(&serde_json::json!({
                "seq": 153,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": 2,
                },
            }))
            .expect("locals response");
        let evaluate = session
            .message_response(&serde_json::json!({
                "seq": 154,
                "type": "request",
                "command": "evaluate",
                "arguments": {
                    "expression": "total",
                    "context": "repl",
                },
            }))
            .expect("evaluate response");

        let vars = locals["body"]["variables"].as_array().expect("locals");
        assert!(vars
            .iter()
            .any(|var| var["name"] == "total" && var["value"] == "5" && var["type"] == "int"));
        assert_eq!(evaluate["success"], true, "{evaluate}");
        assert_eq!(evaluate["body"]["result"], "5");
        assert_eq!(evaluate["body"]["type"], "int");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_locals_reflect_runtime_reassignment_after_step() {
        let dir = temp_output_dir("dap-runtime-assign-locals");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let mut total: int = 1\ntotal = total + 4\n")
            .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 155,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 156,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let locals = session
            .message_response(&serde_json::json!({
                "seq": 157,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": 2,
                },
            }))
            .expect("locals response");

        let vars = locals["body"]["variables"].as_array().expect("locals");
        assert!(vars
            .iter()
            .any(|var| { var["name"] == "total" && var["value"] == "5" && var["type"] == "int" }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_variables_include_reference_runtime_output() {
        let dir = temp_output_dir("dap-runtime-output");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "@out \"debug-ready\"\n").expect("write source");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 11,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let variables = session
            .message_response(&serde_json::json!({
                "seq": 12,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": 1,
                },
            }))
            .expect("variables response");

        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(launch["body"]["runtime"]["status"], "ok");
        assert_eq!(launch["body"]["runtime"]["stdout"], "debug-ready\n");
        let vars = variables["body"]["variables"]
            .as_array()
            .expect("variables");
        assert!(vars
            .iter()
            .any(|var| var["name"] == "runtimeStatus" && var["value"] == "ok"));
        assert!(vars
            .iter()
            .any(|var| var["name"] == "stdout" && var["value"] == "debug-ready\n"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_evaluate_returns_project_runtime_values() {
        let dir = temp_output_dir("dap-evaluate");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "@out \"eval-ready\"\n").expect("write source");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 37,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let evaluate = session
            .message_response(&serde_json::json!({
                "seq": 38,
                "type": "request",
                "command": "evaluate",
                "arguments": {
                    "expression": "stdout",
                    "context": "repl",
                },
            }))
            .expect("evaluate response");

        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(evaluate["success"], true, "{evaluate}");
        assert_eq!(evaluate["body"]["result"], "eval-ready\n");
        assert_eq!(evaluate["body"]["type"], "string");
        assert_eq!(evaluate["body"]["variablesReference"], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_completions_returns_evaluable_project_values() {
        let dir = temp_output_dir("dap-completions");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "@out \"complete-ready\"\n").expect("write source");
        let mut session = DapSession::default();

        let launch = session
            .message_response(&serde_json::json!({
                "seq": 39,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let completions = session
            .message_response(&serde_json::json!({
                "seq": 40,
                "type": "request",
                "command": "completions",
                "arguments": {
                    "text": "std",
                    "column": 4,
                    "line": 1,
                },
            }))
            .expect("completions response");

        assert_eq!(launch["success"], true, "{launch}");
        assert_eq!(completions["success"], true, "{completions}");
        let targets = completions["body"]["targets"]
            .as_array()
            .expect("completion targets");
        assert!(targets
            .iter()
            .any(|target| target["label"] == "stdout" && target["type"] == "property"));
        assert!(targets.iter().all(|target| target["label"]
            .as_str()
            .is_some_and(|label| label.starts_with("std"))));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_locals_follow_current_stopped_line() {
        let dir = temp_output_dir("dap-line-locals");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 57,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let scopes = session
            .message_response(&serde_json::json!({
                "seq": 58,
                "type": "request",
                "command": "scopes",
                "arguments": {
                    "frameId": 1,
                },
            }))
            .expect("scopes response");
        let locals_ref = scopes["body"]["scopes"]
            .as_array()
            .expect("scopes")
            .iter()
            .find(|scope| scope["name"] == "Locals")
            .and_then(|scope| scope["variablesReference"].as_u64())
            .expect("locals scope");
        let first_locals = session
            .message_response(&serde_json::json!({
                "seq": 59,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": locals_ref,
                },
            }))
            .expect("first locals response");
        session
            .message_response(&serde_json::json!({
                "seq": 60,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let second_locals = session
            .message_response(&serde_json::json!({
                "seq": 61,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": locals_ref,
                },
            }))
            .expect("second locals response");

        let first_vars = first_locals["body"]["variables"]
            .as_array()
            .expect("first locals");
        assert!(first_vars.iter().any(|var| var["name"] == "first"));
        assert!(!first_vars.iter().any(|var| var["name"] == "second"));
        let second_vars = second_locals["body"]["variables"]
            .as_array()
            .expect("second locals");
        assert!(second_vars.iter().any(|var| var["name"] == "first"));
        assert!(second_vars.iter().any(|var| var["name"] == "second"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_locals_evaluate_pure_top_level_expressions() {
        let dir = temp_output_dir("dap-expression-locals");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            "let base: int = 2\nlet doubled: int = base * 2 + 1\n",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 62,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 63,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let scopes = session
            .message_response(&serde_json::json!({
                "seq": 64,
                "type": "request",
                "command": "scopes",
                "arguments": {
                    "frameId": 1,
                },
            }))
            .expect("scopes response");
        let locals_ref = scopes["body"]["scopes"]
            .as_array()
            .expect("scopes")
            .iter()
            .find(|scope| scope["name"] == "Locals")
            .and_then(|scope| scope["variablesReference"].as_u64())
            .expect("locals scope");
        let locals = session
            .message_response(&serde_json::json!({
                "seq": 65,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": locals_ref,
                },
            }))
            .expect("locals response");
        let evaluate = session
            .message_response(&serde_json::json!({
                "seq": 66,
                "type": "request",
                "command": "evaluate",
                "arguments": {
                    "expression": "doubled",
                    "context": "repl",
                },
            }))
            .expect("evaluate response");

        let vars = locals["body"]["variables"].as_array().expect("locals");
        assert!(vars
            .iter()
            .any(|var| var["name"] == "doubled" && var["value"] == "5" && var["type"] == "int"));
        assert_eq!(evaluate["success"], true, "{evaluate}");
        assert_eq!(evaluate["body"]["result"], "5");
        assert_eq!(evaluate["body"]["type"], "int");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dap_locals_evaluate_array_and_object_initializers() {
        let dir = temp_output_dir("dap-compound-locals");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            "let xs = [1, 2, 3]\nlet user = { id: 1, name: \"Ada\" }\n",
        )
        .expect("write source");
        let mut session = DapSession::default();

        session
            .message_response(&serde_json::json!({
                "seq": 74,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        session
            .message_response(&serde_json::json!({
                "seq": 75,
                "type": "request",
                "command": "next",
                "arguments": {
                    "threadId": 1,
                },
            }))
            .expect("next response");
        let scopes = session
            .message_response(&serde_json::json!({
                "seq": 76,
                "type": "request",
                "command": "scopes",
                "arguments": {
                    "frameId": 1,
                },
            }))
            .expect("scopes response");
        let locals_ref = scopes["body"]["scopes"]
            .as_array()
            .expect("scopes")
            .iter()
            .find(|scope| scope["name"] == "Locals")
            .and_then(|scope| scope["variablesReference"].as_u64())
            .expect("locals scope");
        let locals = session
            .message_response(&serde_json::json!({
                "seq": 77,
                "type": "request",
                "command": "variables",
                "arguments": {
                    "variablesReference": locals_ref,
                },
            }))
            .expect("locals response");

        let vars = locals["body"]["variables"].as_array().expect("locals");
        assert!(vars.iter().any(|var| var["name"] == "xs"
            && var["value"] == "[1, 2, 3]"
            && var["type"] == "array"));
        assert!(vars.iter().any(|var| {
            var["name"] == "user"
                && var["value"] == "{ id: 1, name: \"Ada\" }"
                && var["type"] == "object"
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_stdio_document_symbol_returns_symbols_for_file_uri() {
        let dir = temp_output_dir("lsp-document-symbol");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User {
  id: int
}

function greet(user: User): string -> "hello"
"#,
        )
        .expect("write source");
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "textDocument/documentSymbol",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
            },
        })
        .to_string();
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        let output = lsp_stdio_response(&input).expect("stdio response");
        let (_, response_body) = output
            .split_once("\r\n\r\n")
            .expect("content-length response frame");
        let response: serde_json::Value =
            serde_json::from_str(response_body).expect("response json");
        let symbols = response["result"].as_array().expect("document symbols");

        assert_eq!(response["id"], 11);
        assert!(response.get("error").is_none());
        assert!(symbols
            .iter()
            .any(|symbol| symbol["name"] == "User" && symbol["kind"] == 23));
        assert!(symbols
            .iter()
            .any(|symbol| symbol["name"] == "greet" && symbol["kind"] == 12));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_document_symbol_accepts_percent_encoded_file_uri() {
        let dir = temp_output_dir("lsp-document-symbol-space");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app space.orv");
        std::fs::write(&source, "struct User { id: int }\n").expect("write source");
        let uri = format!("file://{}", source.display()).replace(' ', "%20");

        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 12,
            "method": "textDocument/documentSymbol",
            "params": {
                "textDocument": {
                    "uri": uri,
                },
            },
        }));

        assert!(response.get("error").is_none(), "{response}");
        assert!(response["result"]
            .as_array()
            .expect("document symbols")
            .iter()
            .any(|symbol| symbol["name"] == "User"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_text_document_diagnostic_returns_full_report_for_file_uri() {
        let dir = temp_output_dir("lsp-diagnostic");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let bad: int = \"wrong\"\n").expect("write source");
        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 13,
            "method": "textDocument/diagnostic",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
            },
        }));

        assert_eq!(response["id"], 13);
        assert!(response.get("error").is_none(), "{response}");
        assert_eq!(response["result"]["kind"], "full");
        let items = response["result"]["items"]
            .as_array()
            .expect("diagnostic items");
        assert!(items.iter().any(|item| {
            item["severity"] == 1
                && item["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("type mismatch"))
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_code_action_returns_reveal_action_for_diagnostic_range() {
        let dir = temp_output_dir("lsp-code-action");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "let bad: int = \"wrong\"\n").expect("write source");
        let canonical_source = std::fs::canonicalize(&source).expect("canonical source");

        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 32,
            "method": "textDocument/codeAction",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 25 },
                },
                "context": {
                    "diagnostics": [],
                },
            },
        }));

        assert_eq!(response["id"], 32);
        assert!(response.get("error").is_none(), "{response}");
        let actions = response["result"].as_array().expect("code actions");
        let action = actions
            .iter()
            .find(|action| {
                action["title"]
                    .as_str()
                    .is_some_and(|title| title.contains("type mismatch"))
            })
            .expect("diagnostic reveal action");
        assert_eq!(action["kind"], "quickfix");
        assert_eq!(action["command"]["command"], "orv.revealDiagnostic");
        assert_eq!(action["diagnostics"][0]["source"], "orv");
        assert_eq!(
            action["command"]["arguments"][0],
            format!("file://{}", canonical_source.display())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_document_link_returns_import_targets() {
        let dir = temp_output_dir("lsp-document-link");
        let models = dir.join("models");
        std::fs::create_dir_all(&models).expect("create models dir");
        let source = dir.join("app.orv");
        let imported = models.join("user.orv");
        std::fs::write(&source, "import models.user.User\nlet ok: int = 1\n")
            .expect("write source");
        std::fs::write(&imported, "pub struct User { id: int }\n").expect("write imported");
        let canonical_imported = std::fs::canonicalize(&imported).expect("canonical imported");

        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 24,
            "method": "textDocument/documentLink",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
            },
        }));

        assert_eq!(response["id"], 24);
        assert!(response.get("error").is_none(), "{response}");
        let links = response["result"].as_array().expect("document links");
        let link = links
            .iter()
            .find(|link| link["target"] == format!("file://{}", canonical_imported.display()))
            .expect("import document link");
        assert_eq!(link["range"]["start"]["line"], 0);
        assert_eq!(link["range"]["start"]["character"], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_folding_range_returns_multiline_declarations() {
        let dir = temp_output_dir("lsp-folding-range");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User {
  id: int
  email: string
}

function greet(user: User): string -> {
  "hello"
}
"#,
        )
        .expect("write source");

        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 25,
            "method": "textDocument/foldingRange",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
            },
        }));

        assert_eq!(response["id"], 25);
        assert!(response.get("error").is_none(), "{response}");
        let ranges = response["result"].as_array().expect("folding ranges");
        assert!(ranges.iter().any(|range| {
            range["startLine"] == 0 && range["endLine"].as_u64().is_some_and(|line| line >= 3)
        }));
        assert!(ranges.iter().any(|range| {
            range["startLine"] == 5 && range["endLine"].as_u64().is_some_and(|line| line >= 7)
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_selection_range_returns_structural_parent_range() {
        let dir = temp_output_dir("lsp-selection-range");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User {
  id: int
  email: string
}

function greet(user: User): string -> {
  "hello"
}
"#,
        )
        .expect("write source");

        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 26,
            "method": "textDocument/selectionRange",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
                "positions": [
                    {
                        "line": 1,
                        "character": 4,
                    },
                ],
            },
        }));

        assert_eq!(response["id"], 26);
        assert!(response.get("error").is_none(), "{response}");
        let selections = response["result"].as_array().expect("selection ranges");
        assert_eq!(selections.len(), 1);
        let selection = &selections[0];
        assert_eq!(selection["range"]["start"]["line"], 0);
        assert_eq!(selection["range"]["start"]["character"], 0);
        assert!(selection["range"]["end"]["line"]
            .as_u64()
            .is_some_and(|line| line >= 3));
        assert!(selection
            .get("parent")
            .is_none_or(serde_json::Value::is_null));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_prepare_rename_returns_identifier_range_and_placeholder() {
        let dir = temp_output_dir("lsp-prepare-rename");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(&source, "struct User { id: int }\n").expect("write source");

        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 27,
            "method": "textDocument/prepareRename",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
                "position": {
                    "line": 0,
                    "character": 8,
                },
            },
        }));

        assert_eq!(response["id"], 27);
        assert!(response.get("error").is_none(), "{response}");
        assert_eq!(response["result"]["placeholder"], "User");
        assert_eq!(response["result"]["range"]["start"]["line"], 0);
        assert_eq!(response["result"]["range"]["start"]["character"], 7);
        assert_eq!(response["result"]["range"]["end"]["character"], 11);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_rename_returns_workspace_edit_for_project_references() {
        let dir = temp_output_dir("lsp-rename");
        let models = dir.join("models");
        std::fs::create_dir_all(&models).expect("create models dir");
        let source = dir.join("app.orv");
        let imported = models.join("user.orv");
        std::fs::write(
            &source,
            "import models.user.User\nlet u: User = { id: 1 }\n",
        )
        .expect("write source");
        std::fs::write(&imported, "pub struct User { id: int }\n").expect("write imported");
        let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
        let canonical_imported = std::fs::canonicalize(&imported).expect("canonical imported");

        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 28,
            "method": "textDocument/rename",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
                "position": {
                    "line": 1,
                    "character": 8,
                },
                "newName": "Account",
            },
        }));

        assert_eq!(response["id"], 28);
        assert!(response.get("error").is_none(), "{response}");
        let changes = response["result"]["changes"].as_object().expect("changes");
        let source_uri = format!("file://{}", canonical_source.display());
        let imported_uri = format!("file://{}", canonical_imported.display());
        let source_edits = changes
            .get(&source_uri)
            .and_then(serde_json::Value::as_array)
            .expect("source edits");
        let imported_edits = changes
            .get(&imported_uri)
            .and_then(serde_json::Value::as_array)
            .expect("imported edits");
        assert!(
            source_edits
                .iter()
                .filter(|edit| edit["newText"] == "Account")
                .count()
                >= 2
        );
        assert!(imported_edits
            .iter()
            .any(|edit| edit["newText"] == "Account"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_document_highlight_returns_current_file_identifier_occurrences() {
        let dir = temp_output_dir("lsp-document-highlight");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"struct User { id: int }

let u: User = { id: 1 }
let v: User = u
",
        )
        .expect("write source");

        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 29,
            "method": "textDocument/documentHighlight",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
                "position": {
                    "line": 2,
                    "character": 8,
                },
            },
        }));

        assert_eq!(response["id"], 29);
        assert!(response.get("error").is_none(), "{response}");
        let highlights = response["result"].as_array().expect("highlights");
        assert_eq!(highlights.len(), 3);
        assert!(highlights
            .iter()
            .any(|highlight| highlight["range"]["start"]["line"] == 0));
        assert!(highlights
            .iter()
            .any(|highlight| highlight["range"]["start"]["line"] == 2));
        assert!(highlights
            .iter()
            .any(|highlight| highlight["range"]["start"]["line"] == 3));
        assert!(highlights.iter().all(|highlight| highlight["kind"] == 1));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_semantic_tokens_returns_project_graph_declaration_tokens() {
        let dir = temp_output_dir("lsp-semantic-tokens");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User { id: int }

function greet(user: User): string -> "hello"
"#,
        )
        .expect("write source");

        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 30,
            "method": "textDocument/semanticTokens/full",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
            },
        }));

        assert_eq!(response["id"], 30);
        assert!(response.get("error").is_none(), "{response}");
        let data = response["result"]["data"]
            .as_array()
            .expect("semantic token data");
        assert_eq!(data.len() % 5, 0);
        let tokens: Vec<Vec<u64>> = data
            .chunks(5)
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|value| value.as_u64().expect("semantic token integer"))
                    .collect()
            })
            .collect();
        assert!(tokens
            .iter()
            .any(|token| token.as_slice() == [0, 7, 4, 1, 1]));
        assert!(tokens
            .iter()
            .any(|token| token.as_slice() == [2, 9, 5, 2, 1]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_code_lens_returns_project_graph_reveal_commands() {
        let dir = temp_output_dir("lsp-code-lens");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User { id: int }

function greet(user: User): string -> "hello"
"#,
        )
        .expect("write source");

        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 31,
            "method": "textDocument/codeLens",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
            },
        }));

        assert_eq!(response["id"], 31);
        assert!(response.get("error").is_none(), "{response}");
        let lenses = response["result"].as_array().expect("code lenses");
        let user_lens = lenses
            .iter()
            .find(|lens| lens["command"]["arguments"][1] == "User")
            .expect("User code lens");
        assert_eq!(user_lens["range"]["start"]["line"], 0);
        assert_eq!(user_lens["command"]["command"], "orv.revealSourceNode");
        assert_eq!(user_lens["command"]["title"], "Reveal Struct User");
        assert!(lenses
            .iter()
            .any(|lens| lens["command"]["arguments"][1] == "greet"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_execute_command_reveals_project_graph_source_node() {
        let dir = temp_output_dir("lsp-execute-command");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).expect("create src dir");
        let source = src.join("main.orv");
        std::fs::write(
            dir.join("orv.toml"),
            r#"[project]
name = "execute-command"
entry = "src/main.orv"
"#,
        )
        .expect("write manifest");
        std::fs::write(&source, "struct User { id: int }\n").expect("write source");
        let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
        let mut session = LspSession::default();

        let initialize = session.jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 33,
            "method": "initialize",
            "params": {
                "rootUri": format!("file://{}", dir.display()),
            },
        }));
        let lenses = session.jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 34,
            "method": "textDocument/codeLens",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
            },
        }));
        let user_lens = lenses["result"]
            .as_array()
            .expect("code lenses")
            .iter()
            .find(|lens| lens["command"]["arguments"][1] == "User")
            .expect("User code lens")
            .clone();
        let execute = session.jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 35,
            "method": "workspace/executeCommand",
            "params": {
                "command": user_lens["command"]["command"],
                "arguments": user_lens["command"]["arguments"],
            },
        }));

        assert!(initialize.get("error").is_none(), "{initialize}");
        assert!(lenses.get("error").is_none(), "{lenses}");
        assert_eq!(execute["id"], 35);
        assert!(execute.get("error").is_none(), "{execute}");
        assert_eq!(execute["result"]["name"], "User");
        assert_eq!(execute["result"]["kind"], "Struct");
        assert_eq!(
            execute["result"]["source_node"],
            user_lens["command"]["arguments"][0]
        );
        assert_eq!(
            execute["result"]["location"]["uri"],
            format!("file://{}", canonical_source.display())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_workspace_diagnostic_returns_imported_file_diagnostics() {
        let dir = temp_output_dir("lsp-workspace-diagnostic");
        let src = dir.join("src");
        let models = src.join("models");
        std::fs::create_dir_all(&models).expect("create models dir");
        let entry = src.join("main.orv");
        let imported = models.join("user.orv");
        std::fs::write(
            dir.join("orv.toml"),
            r#"[project]
name = "workspace-diagnostic"
entry = "src/main.orv"
"#,
        )
        .expect("write manifest");
        std::fs::write(&entry, "import models.user.User\nlet ok: int = 1\n").expect("write entry");
        std::fs::write(
            &imported,
            "pub struct User { id: int }\nlet bad: int = \"wrong\"\n",
        )
        .expect("write imported");
        let canonical_imported = std::fs::canonicalize(&imported).expect("canonical imported");
        let mut session = LspSession::default();

        let initialize = session.jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 22,
            "method": "initialize",
            "params": {
                "rootUri": format!("file://{}", dir.display()),
            },
        }));
        let response = session.jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 23,
            "method": "workspace/diagnostic",
            "params": {
                "previousResultIds": [],
            },
        }));

        assert!(initialize.get("error").is_none(), "{initialize}");
        assert_eq!(response["id"], 23);
        assert!(response.get("error").is_none(), "{response}");
        let items = response["result"]["items"]
            .as_array()
            .expect("workspace diagnostic items");
        let imported_report = items
            .iter()
            .find(|item| item["uri"] == format!("file://{}", canonical_imported.display()))
            .expect("imported diagnostic report");
        let diagnostics = imported_report["items"]
            .as_array()
            .expect("imported diagnostics");
        assert!(diagnostics.iter().any(|item| {
            item["message"]
                .as_str()
                .is_some_and(|message| message.contains("type mismatch"))
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_definition_returns_symbol_declaration_location() {
        let dir = temp_output_dir("lsp-definition");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"struct User {
  id: int
}

let u: User = { id: 1 }
",
        )
        .expect("write source");
        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 16,
            "method": "textDocument/definition",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
                "position": {
                    "line": 4,
                    "character": 8,
                },
            },
        }));

        assert_eq!(response["id"], 16);
        assert!(response.get("error").is_none(), "{response}");
        let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
        assert_eq!(
            response["result"]["uri"],
            format!("file://{}", canonical_source.display())
        );
        assert_eq!(response["result"]["range"]["start"]["line"], 0);
        assert_eq!(response["result"]["range"]["start"]["character"], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_hover_returns_symbol_summary() {
        let dir = temp_output_dir("lsp-hover");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r"struct User {
  id: int
}

let u: User = { id: 1 }
",
        )
        .expect("write source");
        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 17,
            "method": "textDocument/hover",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
                "position": {
                    "line": 4,
                    "character": 8,
                },
            },
        }));

        assert_eq!(response["id"], 17);
        assert!(response.get("error").is_none(), "{response}");
        assert_eq!(response["result"]["contents"]["kind"], "markdown");
        assert_eq!(response["result"]["contents"]["value"], "**Struct** `User`");
        assert_eq!(response["result"]["range"]["start"]["line"], 0);
        assert_eq!(response["result"]["range"]["start"]["character"], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_references_returns_identifier_locations() {
        let dir = temp_output_dir("lsp-references");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User {
  id: int
}

function greet(user: User): string -> "hello"

let u: User = { id: 1 }
"#,
        )
        .expect("write source");
        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 19,
            "method": "textDocument/references",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
                "position": {
                    "line": 6,
                    "character": 8,
                },
            },
        }));

        assert_eq!(response["id"], 19);
        assert!(response.get("error").is_none(), "{response}");
        let locations = response["result"].as_array().expect("reference locations");
        assert!(locations.iter().any(|location| {
            location["range"]["start"]["line"] == 0 && location["range"]["start"]["character"] == 7
        }));
        assert!(locations.iter().any(|location| {
            location["range"]["start"]["line"] == 4 && location["range"]["start"]["character"] == 21
        }));
        assert!(locations.iter().any(|location| {
            location["range"]["start"]["line"] == 6 && location["range"]["start"]["character"] == 7
        }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_completion_returns_project_symbols() {
        let dir = temp_output_dir("lsp-completion");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User {
  id: int
}

function greet(user: User): string -> "hello"

@server {
  @route GET /ping {
    @respond 200 "ok"
  }
}
"#,
        )
        .expect("write source");
        let response = lsp_jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 18,
            "method": "textDocument/completion",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
                "position": {
                    "line": 5,
                    "character": 0,
                },
            },
        }));

        assert_eq!(response["id"], 18);
        assert!(response.get("error").is_none(), "{response}");
        assert_eq!(response["result"]["isIncomplete"], false);
        let items = response["result"]["items"]
            .as_array()
            .expect("completion items");
        assert!(items
            .iter()
            .any(|item| item["label"] == "User" && item["kind"] == 22));
        assert!(items
            .iter()
            .any(|item| item["label"] == "greet" && item["kind"] == 3));
        assert!(items
            .iter()
            .any(|item| item["label"] == "route" && item["kind"] == 23));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_workspace_symbol_returns_matching_project_symbols() {
        let dir = temp_output_dir("lsp-workspace-symbol");
        let src = dir.join("src");
        let models = src.join("models");
        std::fs::create_dir_all(&models).expect("create models dir");
        let entry = src.join("main.orv");
        let imported = models.join("user.orv");
        std::fs::write(
            dir.join("orv.toml"),
            r#"[project]
name = "workspace-symbol"
entry = "src/main.orv"
"#,
        )
        .expect("write manifest");
        std::fs::write(
            &entry,
            "import models.user.User\nfunction checkout(user: User): string -> \"ok\"\n",
        )
        .expect("write entry");
        std::fs::write(&imported, "pub struct User { id: int }\n").expect("write imported");
        let canonical_imported = std::fs::canonicalize(&imported).expect("canonical imported");
        let mut session = LspSession::default();

        let initialize = session.jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 20,
            "method": "initialize",
            "params": {
                "rootUri": format!("file://{}", dir.display()),
            },
        }));
        let response = session.jsonrpc_response(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 21,
            "method": "workspace/symbol",
            "params": {
                "query": "User",
            },
        }));

        assert!(initialize.get("error").is_none(), "{initialize}");
        assert_eq!(response["id"], 21);
        assert!(response.get("error").is_none(), "{response}");
        let symbols = response["result"].as_array().expect("workspace symbols");
        let user = symbols
            .iter()
            .find(|symbol| symbol["name"] == "User")
            .expect("User workspace symbol");
        assert_eq!(user["kind"], 23);
        assert_eq!(
            user["location"]["uri"],
            format!("file://{}", canonical_imported.display())
        );
        assert!(symbols.iter().all(|symbol| symbol["name"]
            .as_str()
            .is_some_and(|name| name.contains("User"))));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_stdio_document_symbol_uses_did_open_unsaved_content() {
        let dir = temp_output_dir("lsp-did-open-symbol");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("unsaved.orv");
        let uri = format!("file://{}", source.display());
        let did_open = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "orv",
                    "version": 1,
                    "text": "struct Draft { id: int }\n",
                },
            },
        })
        .to_string();
        let document_symbol = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 14,
            "method": "textDocument/documentSymbol",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
            },
        })
        .to_string();
        let input = format!(
            "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
            did_open.len(),
            did_open,
            document_symbol.len(),
            document_symbol
        );

        let output = lsp_stdio_response(&input).expect("stdio response");
        let (_, response_body) = output
            .split_once("\r\n\r\n")
            .expect("content-length response frame");
        let response: serde_json::Value =
            serde_json::from_str(response_body).expect("response json");

        assert_eq!(response["id"], 14);
        assert!(response.get("error").is_none(), "{response}");
        assert!(response["result"]
            .as_array()
            .expect("document symbols")
            .iter()
            .any(|symbol| symbol["name"] == "Draft"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_stdio_document_symbol_uses_did_change_unsaved_content() {
        let dir = temp_output_dir("lsp-did-change-symbol");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("unsaved.orv");
        let uri = format!("file://{}", source.display());
        let did_open = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "orv",
                    "version": 1,
                    "text": "struct Draft { id: int }\n",
                },
            },
        })
        .to_string();
        let did_change = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                    "version": 2,
                },
                "contentChanges": [
                    { "text": "struct Changed { id: int }\n" }
                ],
            },
        })
        .to_string();
        let document_symbol = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 15,
            "method": "textDocument/documentSymbol",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source.display()),
                },
            },
        })
        .to_string();
        let input = format!(
            "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
            did_open.len(),
            did_open,
            did_change.len(),
            did_change,
            document_symbol.len(),
            document_symbol
        );

        let output = lsp_stdio_response(&input).expect("stdio response");
        let (_, response_body) = output
            .split_once("\r\n\r\n")
            .expect("content-length response frame");
        let response: serde_json::Value =
            serde_json::from_str(response_body).expect("response json");
        let symbols = response["result"].as_array().expect("document symbols");

        assert_eq!(response["id"], 15);
        assert!(response.get("error").is_none(), "{response}");
        assert!(symbols.iter().any(|symbol| symbol["name"] == "Changed"));
        assert!(!symbols.iter().any(|symbol| symbol["name"] == "Draft"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn db_apply_writes_current_schema_snapshot() {
        let dir = temp_output_dir("db-apply");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User {
  id: int
  email: string
}"#,
        )
        .expect("write source");
        let schema = dir.join("schema.json");

        cmd_db_apply(&source, &schema).expect("apply schema");

        let written = read_json_value(&schema).expect("read schema");
        assert_eq!(written["schema_version"], 1);
        assert_eq!(
            written["structs"]["User"]["fields"]["email"]["type"],
            "string"
        );
        let plan = db_plan_json(&source, Some(&schema)).expect("db plan after apply");
        assert_eq!(plan["actions"].as_array().expect("actions").len(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn db_apply_appends_migration_history_when_requested() {
        let dir = temp_output_dir("db-history");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let first_source = dir.join("first.orv");
        std::fs::write(
            &first_source,
            r#"struct User {
  id: int
  email: string
}"#,
        )
        .expect("write first source");
        let second_source = dir.join("second.orv");
        std::fs::write(
            &second_source,
            r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
        )
        .expect("write second source");
        let schema = dir.join("schema.json");
        let history = dir.join("history.json");

        cmd_db_apply_with_history(&first_source, &schema, Some(&history))
            .expect("apply first schema");
        cmd_db_apply_with_history(&second_source, &schema, Some(&history))
            .expect("apply second schema");

        let history = read_json_value(&history).expect("read history");
        assert_eq!(history["schema_version"], 1);
        let entries = history["entries"].as_array().expect("history entries");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["actions"].as_array().expect("actions").len(), 1);
        assert!(entries[1]["actions"]
            .as_array()
            .expect("actions")
            .iter()
            .any(|action| action["kind"] == "add_field" && action["field"] == "avatar"));
        assert_ne!(entries[0]["schema_hash"], entries[1]["schema_hash"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn db_migrate_applies_schema_and_history() {
        let dir = temp_output_dir("db-migrate");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct Order {
  id: int
  total: int
}"#,
        )
        .expect("write source");
        let schema = dir.join("schema.json");
        let history = dir.join("history.json");

        cmd_db_migrate(&source, &schema, Some(&history)).expect("migrate schema");

        let written = read_json_value(&schema).expect("read schema");
        assert_eq!(
            written["structs"]["Order"]["fields"]["total"]["type"],
            "int"
        );
        let history = read_json_value(&history).expect("read history");
        assert_eq!(
            history["entries"]
                .as_array()
                .expect("history entries")
                .len(),
            1
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn db_rollback_restores_previous_schema_snapshot() {
        let dir = temp_output_dir("db-rollback");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let original_source = dir.join("original.orv");
        std::fs::write(
            &original_source,
            r#"struct User {
  id: int
  email: string
}"#,
        )
        .expect("write original source");
        let changed_source = dir.join("changed.orv");
        std::fs::write(
            &changed_source,
            r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
        )
        .expect("write changed source");
        let schema = dir.join("schema.json");

        cmd_db_apply(&original_source, &schema).expect("apply original schema");
        cmd_db_apply(&changed_source, &schema).expect("apply changed schema");
        assert!(
            read_json_value(&schema).expect("read changed schema")["structs"]["User"]["fields"]
                .as_object()
                .expect("fields")
                .contains_key("avatar")
        );

        cmd_db_rollback(&schema).expect("rollback schema");

        let restored = read_json_value(&schema).expect("read restored schema");
        assert!(!restored["structs"]["User"]["fields"]
            .as_object()
            .expect("fields")
            .contains_key("avatar"));
        let plan = db_plan_json(&original_source, Some(&schema)).expect("plan after rollback");
        assert_eq!(plan["actions"].as_array().expect("actions").len(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn db_verify_accepts_current_schema_snapshot() {
        let dir = temp_output_dir("db-verify-current");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("app.orv");
        std::fs::write(
            &source,
            r#"struct User {
  id: int
  email: string
}"#,
        )
        .expect("write source");
        let schema = dir.join("schema.json");

        cmd_db_apply(&source, &schema).expect("apply schema");

        cmd_db_verify(&source, &schema).expect("verify current schema");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn db_verify_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "db",
            "verify",
            "fixtures/e2e/hello.orv",
            "--schema",
            "target/schema.json",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn db_verify_rejects_schema_drift() {
        let dir = temp_output_dir("db-verify-drift");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let original = dir.join("original.orv");
        std::fs::write(
            &original,
            r#"struct User {
  id: int
  email: string
}"#,
        )
        .expect("write original");
        let changed = dir.join("changed.orv");
        std::fs::write(
            &changed,
            r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
        )
        .expect("write changed");
        let schema = dir.join("schema.json");

        cmd_db_apply(&original, &schema).expect("apply schema");

        let err = cmd_db_verify(&changed, &schema).expect_err("schema drift");
        assert!(
            err.to_string().contains("db schema drift: 1 action(s)"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn db_squash_writes_compacted_history_actions() {
        let dir = temp_output_dir("db-squash");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let first_source = dir.join("first.orv");
        std::fs::write(
            &first_source,
            r#"struct User {
  id: int
  email: string
}"#,
        )
        .expect("write first");
        let second_source = dir.join("second.orv");
        std::fs::write(
            &second_source,
            r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
        )
        .expect("write second");
        let schema = dir.join("schema.json");
        let history = dir.join("history.json");
        let squashed = dir.join("squashed.json");

        cmd_db_apply_with_history(&first_source, &schema, Some(&history))
            .expect("apply first schema");
        cmd_db_apply_with_history(&second_source, &schema, Some(&history))
            .expect("apply second schema");

        cmd_db_squash(&history, &squashed).expect("squash history");

        let value = read_json_value(&squashed).expect("read squashed");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["entries"], 2);
        assert!(value["schema_hash"].as_str().expect("schema hash").len() >= 16);
        assert!(value["actions"]
            .as_array()
            .expect("actions")
            .iter()
            .any(|action| action["kind"] == "add_field" && action["field"] == "avatar"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn db_squash_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "db",
            "squash",
            "--history",
            "target/history.json",
            "--out",
            "target/squashed.json",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn db_recover_archive_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "db",
            "recover",
            "--archive",
            "target/archive.json",
            "--out",
            "target/data.json",
            "--until-record",
            "1",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn db_recover_archive_rejects_wal_hash_mismatch() {
        let dir = temp_output_dir("db-recover-archive-hash");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let wal = dir.join("db.wal.jsonl");
        let archive = dir.join("archive.json");
        let target_dir = dir.join("archive-target");
        let out = dir.join("data.json");
        let mut db = orv_runtime::db::InMemoryDb::load_wal(&wal).expect("open wal");
        db.create_logged(
            "users",
            vec![(
                "name".to_string(),
                orv_runtime::Value::Str("Ada".to_string()),
            )],
        )
        .expect("create user");
        cmd_db_archive(
            &wal,
            &archive,
            Some(&format!("file://{}", target_dir.display())),
        )
        .expect("archive wal");
        let archived_wal = db_archive_manifest_wal_path(&archive).expect("archive wal path");
        let tampered = std::fs::read_to_string(&archived_wal)
            .expect("read archived wal")
            .replace("Ada", "Eve");
        std::fs::write(&archived_wal, tampered).expect("tamper archived wal");

        let err = cmd_db_recover_from_inputs(None, Some(&archive), &out, None, None, None)
            .expect_err("tampered archive recover");

        assert!(err.to_string().contains("db archive WAL hash mismatch"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn db_recover_archive_uses_archived_wal_target() {
        let dir = temp_output_dir("db-recover-archive-target");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let wal = dir.join("db.wal.jsonl");
        let archive = dir.join("archive.json");
        let target_dir = dir.join("archive-target");
        let out = dir.join("data.json");
        let mut db = orv_runtime::db::InMemoryDb::load_wal(&wal).expect("open wal");
        db.create_logged(
            "users",
            vec![(
                "name".to_string(),
                orv_runtime::Value::Str("Ada".to_string()),
            )],
        )
        .expect("create first user");
        db.create_logged(
            "users",
            vec![(
                "name".to_string(),
                orv_runtime::Value::Str("Grace".to_string()),
            )],
        )
        .expect("create second user");
        cmd_db_archive(
            &wal,
            &archive,
            Some(&format!("file://{}", target_dir.display())),
        )
        .expect("archive wal");
        std::fs::remove_file(&wal).expect("remove original wal");

        cmd_db_recover_from_inputs(None, Some(&archive), &out, Some(1), None, None)
            .expect("recover from archive");

        let snapshot = read_json_value(&out).expect("snapshot");
        let rows = snapshot["tables"]["users"]["rows"]
            .as_array()
            .expect("users rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "Ada");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verify_artifact_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "verify-artifact",
            "target/orv-build-test/server/app.orv-runtime.json",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn check_artifact_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "check-artifact",
            "target/orv-build-test/server/app.orv-runtime.json",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn check_build_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "check-build", "target/orv-build-test"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn run_artifact_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "run-artifact",
            "target/orv-build-test/server/app.orv-runtime.json",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn run_build_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "run-build", "target/orv-build-test"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn dev_subcommand_is_accepted() {
        let parsed =
            Cli::try_parse_from(["orv", "dev", "src/main.orv", "--out", "target/orv-dev-test"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn dev_hmr_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "dev", "src/main.orv", "--hmr"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn dev_watch_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "dev", "src/main.orv", "--watch"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn reveal_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "reveal",
            "target/orv-build-test",
            "route:GET_/ping:abc123",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn verify_build_subcommand_is_accepted() {
        let parsed = Cli::try_parse_from(["orv", "verify-build", "target/orv-build-test"]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn build_prod_subcommand_flag_is_accepted() {
        let parsed = Cli::try_parse_from([
            "orv",
            "build",
            "fixtures/e2e/hello.orv",
            "--out",
            "target/orv-prod-build-test",
            "--prod",
        ]);
        if let Err(err) = parsed {
            panic!("{}", err.render());
        }
    }

    #[test]
    fn build_writes_manifest_origin_map_and_project_graph() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let out = temp_output_dir("build-artifacts");

        cmd_build(&path, &out).expect("build artifacts");

        let manifest_path = out.join("build-manifest.json");
        let origin_map_path = out.join("origin-map.json");
        let bundle_plan_path = out.join("bundle-plan.json");
        let server_artifact_path = out.join("server").join("app.orv-runtime.json");
        let server_launch_path = out.join("server").join("launch.json");
        let graph_path = out.join("project-graph.json");
        let source_bundle_path = out.join("source-bundle.json");
        assert!(
            manifest_path.is_file(),
            "missing {}",
            manifest_path.display()
        );
        assert!(
            origin_map_path.is_file(),
            "missing {}",
            origin_map_path.display()
        );
        assert!(
            bundle_plan_path.is_file(),
            "missing {}",
            bundle_plan_path.display()
        );
        assert!(
            server_artifact_path.is_file(),
            "missing {}",
            server_artifact_path.display()
        );
        assert!(
            server_launch_path.is_file(),
            "missing {}",
            server_launch_path.display()
        );
        assert!(graph_path.is_file(), "missing {}", graph_path.display());
        assert!(
            source_bundle_path.is_file(),
            "missing {}",
            source_bundle_path.display()
        );

        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest["schema_version"], 1);
        assert_eq!(manifest["entry"], path.display().to_string());
        assert_eq!(manifest["runtime"], "reference-interpreter");
        let runtime_features = manifest["capabilities"]["runtime_features"]
            .as_array()
            .expect("runtime features array");
        assert!(runtime_features
            .iter()
            .any(|feature| feature == "http_server"));
        assert!(runtime_features.iter().any(|feature| feature == "router"));
        assert!(manifest["artifacts"]
            .as_array()
            .expect("artifacts array")
            .iter()
            .any(|artifact| artifact["kind"] == "origin_map"
                && artifact["path"] == "origin-map.json"));
        assert!(manifest["artifacts"]
            .as_array()
            .expect("artifacts array")
            .iter()
            .any(|artifact| artifact["kind"] == "bundle_plan"
                && artifact["path"] == "bundle-plan.json"));
        assert!(manifest["artifacts"]
            .as_array()
            .expect("artifacts array")
            .iter()
            .any(|artifact| artifact["kind"] == "project_graph"
                && artifact["path"] == "project-graph.json"));
        assert!(manifest["artifacts"]
            .as_array()
            .expect("artifacts array")
            .iter()
            .any(|artifact| artifact["kind"] == "source_bundle"
                && artifact["path"] == "source-bundle.json"));
        let source_bundle: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&source_bundle_path).expect("source bundle"),
        )
        .expect("source bundle json");
        assert_eq!(source_bundle["schema_version"], 1);
        assert!(source_bundle["files"]
            .as_array()
            .expect("source files")
            .iter()
            .any(|file| file["source"]
                .as_str()
                .is_some_and(|source| source.contains("@route GET /ping"))));
        let plan: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&bundle_plan_path).expect("plan"))
                .expect("bundle plan json");
        assert_eq!(plan["schema_version"], 1);
        assert!(plan["bundles"]
            .as_array()
            .expect("bundles array")
            .iter()
            .any(|bundle| bundle["kind"] == "server_runtime"
                && bundle["path"] == "server/app.orv-runtime.json"));
        assert!(plan["bundles"]
            .as_array()
            .expect("bundles array")
            .iter()
            .any(|bundle| bundle["kind"] == "server_launcher"
                && bundle["path"] == "server/launch.json"));
        let server_artifact: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&server_artifact_path).expect("server artifact"),
        )
        .expect("server artifact json");
        assert_eq!(server_artifact["schema_version"], 1);
        assert_eq!(server_artifact["runtime"], "reference-interpreter");
        assert_eq!(server_artifact["listen"]["port"], 0);
        assert!(server_artifact["listen"]["origin_id"]
            .as_str()
            .is_some_and(|origin| origin.starts_with("ori_")));
        assert!(server_artifact["routes"]
            .as_array()
            .expect("routes array")
            .iter()
            .any(|route| route["method"] == "GET" && route["path"] == "/ping"));
        assert!(server_artifact["source_bundle"]["files"]
            .as_array()
            .expect("source bundle files")
            .iter()
            .any(|file| file["source"]
                .as_str()
                .is_some_and(|source| source.contains("@route GET /ping"))
                && file["content_hash"]
                    .as_str()
                    .is_some_and(|hash| hash.starts_with("fnv1a64:"))));
        let launch: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&server_launch_path).expect("server launch artifact"),
        )
        .expect("server launch json");
        assert_eq!(launch["schema_version"], 1);
        assert_eq!(launch["runtime"], "reference-interpreter");
        assert_eq!(launch["artifact"], "server/app.orv-runtime.json");
        assert_eq!(launch["protocol"], "http1");
        assert_eq!(launch["listen"], server_artifact["listen"]);
        assert_eq!(launch["command"][0], "orv");
        assert_eq!(launch["command"][1], "run-artifact");
        assert_eq!(launch["command"][2], "server/app.orv-runtime.json");
        assert!(launch["routes"]
            .as_array()
            .expect("launch routes")
            .iter()
            .any(|route| route["method"] == "GET" && route["path"] == "/ping"));

        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn build_prod_writes_deploy_manifest_and_server_entrypoint() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let out = temp_output_dir("build-prod-artifacts");

        cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

        let deploy_manifest_path = out.join("deploy").join("manifest.json");
        let deploy_container_path = out.join("deploy").join("container.json");
        let deploy_dockerfile_path = out.join("deploy").join("Dockerfile");
        let deploy_routes_path = out.join("deploy").join("routes.json");
        let server_entrypoint_path = out.join("deploy").join("server.sh");
        assert!(
            deploy_manifest_path.is_file(),
            "missing {}",
            deploy_manifest_path.display()
        );
        assert!(
            deploy_container_path.is_file(),
            "missing {}",
            deploy_container_path.display()
        );
        assert!(
            deploy_dockerfile_path.is_file(),
            "missing {}",
            deploy_dockerfile_path.display()
        );
        assert!(
            deploy_routes_path.is_file(),
            "missing {}",
            deploy_routes_path.display()
        );
        assert!(
            server_entrypoint_path.is_file(),
            "missing {}",
            server_entrypoint_path.display()
        );
        let deploy = read_json_value(&deploy_manifest_path).expect("deploy manifest");
        assert_eq!(deploy["schema_version"], 1);
        assert_eq!(deploy["profile"], "prod");
        assert_eq!(deploy["entry"], path.display().to_string());
        assert_eq!(deploy["source_bundle"], "source-bundle.json");
        assert_eq!(deploy["server"]["artifact"], "server/app.orv-runtime.json");
        assert_eq!(deploy["server"]["entrypoint"], "deploy/server.sh");
        assert_eq!(deploy["server"]["container"], "deploy/container.json");
        assert_eq!(deploy["server"]["dockerfile"], "deploy/Dockerfile");
        assert!(deploy["server"]["routes"]
            .as_array()
            .expect("server routes")
            .iter()
            .any(|route| route["method"] == "GET" && route["path"] == "/ping"));
        assert_eq!(deploy["server"]["routes_artifact"], "deploy/routes.json");
        let container = read_json_value(&deploy_container_path).expect("deploy container");
        assert_eq!(container["schema_version"], 1);
        assert_eq!(container["kind"], "reference-server-container");
        assert_eq!(container["artifact"], "server/app.orv-runtime.json");
        assert_eq!(container["entrypoint"], "deploy/server.sh");
        assert_eq!(container["routes_artifact"], "deploy/routes.json");
        assert_eq!(container["dockerfile"], "deploy/Dockerfile");
        assert_eq!(container["runtime"], "reference-interpreter");
        assert_eq!(container["protocol"], "http1");
        assert_eq!(container["command"][0], "./deploy/server.sh");
        let dockerfile = std::fs::read_to_string(&deploy_dockerfile_path).expect("Dockerfile");
        assert!(dockerfile.contains("FROM ${ORV_RUNTIME_IMAGE}"));
        assert!(dockerfile.contains("COPY . /app"));
        assert!(dockerfile.contains(r#"ENTRYPOINT ["./deploy/server.sh"]"#));
        let routes = read_json_value(&deploy_routes_path).expect("deploy routes");
        assert_eq!(routes["schema_version"], 1);
        assert_eq!(routes["artifact"], "server/app.orv-runtime.json");
        assert!(json_routes_include(&routes["routes"], "GET", "/ping"));
        let script = std::fs::read_to_string(&server_entrypoint_path).expect("server entrypoint");
        assert!(script.contains("orv run-artifact"));

        cmd_verify_build(&out).expect("verify prod build");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_rejects_deploy_routes_mismatch() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let out = temp_output_dir("deploy-routes-mismatch");

        cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
        let routes_path = out.join("deploy").join("routes.json");
        let mut routes = read_json_value(&routes_path).expect("routes");
        routes["routes"][0]["path"] = serde_json::json!("/wrong");
        write_json(&routes_path, &routes).expect("write corrupt routes");

        let err = cmd_verify_build(&out).expect_err("routes mismatch");

        assert!(err
            .to_string()
            .contains("deploy routes do not match runtime artifact"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_rejects_deploy_container_mismatch() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let out = temp_output_dir("deploy-container-mismatch");

        cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
        let container_path = out.join("deploy").join("container.json");
        let mut container = read_json_value(&container_path).expect("container");
        container["artifact"] = serde_json::json!("server/wrong.orv-runtime.json");
        write_json(&container_path, &container).expect("write corrupt container");

        let err = cmd_verify_build(&out).expect_err("container mismatch");

        assert!(err
            .to_string()
            .contains("deploy container artifact must be server/app.orv-runtime.json"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_rejects_server_launcher_listen_mismatch() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let out = temp_output_dir("server-launch-listen-mismatch");

        cmd_build(&path, &out).expect("build");
        let launch_path = out.join("server").join("launch.json");
        let mut launch = read_json_value(&launch_path).expect("launch");
        launch["listen"]["port"] = serde_json::json!(1234);
        write_json(&launch_path, &launch).expect("write corrupt launch");

        let err = cmd_verify_build(&out).expect_err("listen mismatch");

        assert!(err
            .to_string()
            .contains("server launcher listen does not match runtime artifact"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn build_accepts_orv_toml_project_entry() {
        let dir = temp_output_dir("project-manifest-build");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).expect("create src dir");
        let entry = src.join("main.orv");
        std::fs::write(&entry, "@html { \"Manifest page\" }\n").expect("write entry");
        let manifest = dir.join("orv.toml");
        std::fs::write(
            &manifest,
            r#"[project]
name = "manifest-build"
entry = "src/main.orv"
"#,
        )
        .expect("write manifest");
        let out = dir.join("dist");

        cmd_build(&manifest, &out).expect("manifest build");

        let build_manifest = read_json_value(&out.join("build-manifest.json")).expect("manifest");
        assert_eq!(build_manifest["entry"], entry.display().to_string());
        assert!(
            out.join("pages").join("index.html").is_file(),
            "missing static page"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn build_writes_static_html_page_for_html_only_entry() {
        let out = temp_output_dir("build-static-page");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            r#"@out @html { @body { @h1 "Home" @p "zero runtime" } }"#,
        )
        .expect("write entry");
        let build_out = out.join("dist");

        cmd_build(&entry, &build_out).expect("build artifacts");

        let page = build_out.join("pages").join("index.html");
        let html = std::fs::read_to_string(&page).expect("static page");
        assert_eq!(
            html,
            "<html><body><h1>Home</h1><p>zero runtime</p></body></html>"
        );
        let plan: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(build_out.join("bundle-plan.json")).expect("plan"),
        )
        .expect("bundle plan json");
        let static_bundle = plan["bundles"]
            .as_array()
            .expect("bundles array")
            .iter()
            .find(|bundle| bundle["kind"] == "static_page")
            .expect("static page bundle");
        assert_eq!(static_bundle["path"], "pages/index.html");
        assert_eq!(
            static_bundle["runtime_features"]
                .as_array()
                .expect("runtime features")
                .len(),
            0
        );
        assert!(!plan["bundles"]
            .as_array()
            .expect("bundles array")
            .iter()
            .any(|bundle| bundle["kind"] == "server_runtime"));

        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn build_writes_client_wasm_for_signal_html_entry() {
        let out = temp_output_dir("build-client-wasm");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            "let sig count: int = 0\n@out @html { @body { @p count } }",
        )
        .expect("write entry");
        let build_out = out.join("dist");

        cmd_build(&entry, &build_out).expect("build artifacts");

        let manifest = read_json_value(&build_out.join("build-manifest.json")).expect("manifest");
        assert_eq!(manifest["capabilities"]["client_wasm"], true);
        assert!(manifest["capabilities"]["runtime_features"]
            .as_array()
            .expect("runtime features")
            .iter()
            .any(|feature| feature == "client_wasm"));
        let plan = read_json_value(&build_out.join("bundle-plan.json")).expect("plan");
        assert!(plan["bundles"]
            .as_array()
            .expect("bundles")
            .iter()
            .any(|bundle| bundle["kind"] == "client_wasm" && bundle["path"] == "client/app.wasm"));
        assert!(plan["bundles"]
            .as_array()
            .expect("bundles")
            .iter()
            .any(|bundle| bundle["kind"] == "client_js" && bundle["path"] == "client/app.js"));
        assert!(plan["bundles"]
            .as_array()
            .expect("bundles")
            .iter()
            .any(|bundle| bundle["kind"] == "client_page" && bundle["path"] == "pages/index.html"));
        assert!(!plan["bundles"]
            .as_array()
            .expect("bundles")
            .iter()
            .any(|bundle| bundle["kind"] == "static_page"));
        let wasm = std::fs::read(build_out.join("client").join("app.wasm")).expect("client wasm");
        assert_eq!(&wasm[..4], b"\0asm");
        let wasm_text = String::from_utf8_lossy(&wasm);
        assert!(wasm_text.contains("orv.client"));
        assert!(wasm_text.contains("source_bundle"));
        assert!(wasm_text.contains("orv_start"));
        let loader =
            std::fs::read_to_string(build_out.join("client").join("app.js")).expect("client js");
        assert!(loader.contains("ORV_CLIENT_BOOTSTRAP"));
        assert!(loader.contains("sourceBundleUrl"));
        assert!(loader.contains("../source-bundle.json"));
        assert!(loader.contains("runtimeFeatures"));
        assert!(loader.contains("WebAssembly.instantiate"));
        assert!(loader.contains("orv_start"));
        assert!(loader.contains("app.wasm"));
        let page = std::fs::read_to_string(build_out.join("pages").join("index.html"))
            .expect("client page");
        assert!(page.contains("data-orv-client=\"wasm\""));
        assert!(page.contains("type=\"module\""));
        assert!(page.contains("../client/app.js"));
        cmd_verify_build(&build_out).expect("verify build");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn build_prod_records_client_bootstrap_targets() {
        let out = temp_output_dir("build-prod-client");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            r#"let sig count: int = 0
@out @html { @body { @p count } }"#,
        )
        .expect("write entry");
        let build_out = out.join("dist");

        cmd_build_with_profile(&entry, &build_out, BuildProfile::Production).expect("build prod");

        let deploy =
            read_json_value(&build_out.join("deploy").join("manifest.json")).expect("deploy");
        assert_eq!(deploy["client"]["page"], "pages/index.html");
        assert_eq!(deploy["client"]["loader"], "client/app.js");
        assert_eq!(deploy["client"]["wasm"], "client/app.wasm");
        assert!(deploy["client"]["runtime_features"]
            .as_array()
            .expect("runtime features")
            .iter()
            .any(|feature| feature == "client_wasm"));
        cmd_verify_build(&build_out).expect("verify prod build");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_accepts_static_page_output() {
        let out = temp_output_dir("verify-build-static");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(&entry, r#"@out @html { @body { @h1 "Home" } }"#).expect("write entry");
        let build_out = out.join("dist");

        cmd_build(&entry, &build_out).expect("build artifacts");

        cmd_verify_build(&build_out).expect("verify build");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_rejects_invalid_dev_hmr_session_manifest() {
        let out = temp_output_dir("verify-build-dev-hmr-session");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            "let sig count: int = 0\n@out @html { @body { @p count } }",
        )
        .expect("write entry");
        let build_out = out.join("dist");
        let mut stdout = Vec::new();

        dev_with_writer_with_options(&entry, &build_out, true, false, &mut stdout)
            .expect("dev hmr");
        let session_path = build_out.join("dev").join("session.json");
        let mut session = read_json_value(&session_path).expect("dev session");
        session["watch"]["targets"] = serde_json::Value::Array(
            session["watch"]["targets"]
                .as_array()
                .expect("targets")
                .iter()
                .filter(|target| target["kind"] != "client_wasm")
                .cloned()
                .collect(),
        );
        write_json(&session_path, &session).expect("write corrupt dev session");

        let err = cmd_verify_build(&build_out).expect_err("invalid dev hmr session");

        assert!(err
            .to_string()
            .contains("dev session missing bundle target client_wasm:client/app.wasm"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_rejects_invalid_dev_watch_session_manifest() {
        let out = temp_output_dir("verify-build-dev-watch-session");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(&entry, "@out @html { @body { @h1 \"Watch\" } }").expect("write entry");
        let build_out = out.join("dist");
        let mut stdout = Vec::new();

        dev_with_writer_with_options(&entry, &build_out, false, true, &mut stdout)
            .expect("dev watch");
        let session_path = build_out.join("dev").join("watch.json");
        let mut session = read_json_value(&session_path).expect("dev watch session");
        session["loop"]["interval_ms"] = serde_json::json!(0);
        write_json(&session_path, &session).expect("write corrupt dev watch session");

        let err = cmd_verify_build(&build_out).expect_err("invalid dev watch session");

        assert!(err
            .to_string()
            .contains("dev watch session loop interval_ms must be positive"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_rejects_invalid_dev_watch_transport_path() {
        let out = temp_output_dir("verify-build-dev-watch-transport");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(&entry, "@out @html { @body { @h1 \"Watch\" } }").expect("write entry");
        let build_out = out.join("dist");
        let mut stdout = Vec::new();

        dev_with_writer_with_options(&entry, &build_out, false, true, &mut stdout)
            .expect("dev watch");
        let session_path = build_out.join("dev").join("watch.json");
        let mut session = read_json_value(&session_path).expect("dev watch session");
        session["transport"]["path"] = serde_json::json!("tmp/watch.json");
        write_json(&session_path, &session).expect("write corrupt dev watch session");

        let err = cmd_verify_build(&build_out).expect_err("invalid dev watch transport");

        assert!(err
            .to_string()
            .contains("dev watch session transport path must be dev/watch.json"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_rejects_client_wasm_without_orv_custom_section() {
        let out = temp_output_dir("verify-build-client-wasm-section");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            r#"let sig count: int = 0
@out @html { @body { @p count } }"#,
        )
        .expect("write entry");
        let build_out = out.join("dist");

        cmd_build(&entry, &build_out).expect("build artifacts");
        let mut wasm = WASM_MODULE_HEADER.to_vec();
        let mut custom_section = Vec::new();
        push_wasm_len(&mut custom_section, "not.orv".len());
        custom_section.extend_from_slice(b"not.orv");
        custom_section.extend_from_slice(br#"{"note":"orv.client source_bundle"}"#);
        wasm.push(0);
        push_wasm_len(&mut wasm, custom_section.len());
        wasm.extend(custom_section);
        std::fs::write(build_out.join("client").join("app.wasm"), wasm).expect("rewrite wasm");

        let err = cmd_verify_build(&build_out).expect_err("invalid client wasm");

        assert!(
            err.to_string().contains("ORV metadata"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_rejects_client_wasm_without_start_export() {
        let out = temp_output_dir("verify-build-client-wasm-export");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            "let sig count: int = 0\n@out @html { @body { @p count } }",
        )
        .expect("write entry");
        let build_out = out.join("dist");

        cmd_build(&entry, &build_out).expect("build artifacts");
        let mut wasm = WASM_MODULE_HEADER.to_vec();
        let mut custom_section = Vec::new();
        push_wasm_len(&mut custom_section, CLIENT_WASM_CUSTOM_SECTION_NAME.len());
        custom_section.extend_from_slice(CLIENT_WASM_CUSTOM_SECTION_NAME.as_bytes());
        custom_section.extend_from_slice(CLIENT_WASM_CUSTOM_SECTION_PAYLOAD.as_bytes());
        push_wasm_section(&mut wasm, 0, &custom_section);
        std::fs::write(build_out.join("client").join("app.wasm"), wasm).expect("rewrite wasm");

        let err = cmd_verify_build(&build_out).expect_err("invalid client wasm");

        assert!(
            err.to_string().contains("orv_start"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_rejects_client_js_without_start_call() {
        let out = temp_output_dir("verify-build-client-js-start");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            "let sig count: int = 0\n@out @html { @body { @p count } }",
        )
        .expect("write entry");
        let build_out = out.join("dist");

        cmd_build(&entry, &build_out).expect("build artifacts");
        let loader_path = build_out.join("client").join("app.js");
        let loader = std::fs::read_to_string(&loader_path)
            .expect("client loader")
            .replace(
                r#"  if (typeof instance.exports.orv_start === "function") {
    instance.exports.orv_start();
  }
"#,
                "",
            );
        std::fs::write(&loader_path, loader).expect("rewrite loader");

        let err = cmd_verify_build(&build_out).expect_err("invalid client loader");

        assert!(
            err.to_string().contains("orv_start"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_build_rejects_missing_static_page_output() {
        let out = temp_output_dir("verify-build-missing-static");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(&entry, r#"@out @html { @body { @h1 "Home" } }"#).expect("write entry");
        let build_out = out.join("dist");

        cmd_build(&entry, &build_out).expect("build artifacts");
        std::fs::remove_file(build_out.join("pages").join("index.html")).expect("remove page");

        let err = cmd_verify_build(&build_out).expect_err("missing static page");

        assert!(err
            .to_string()
            .contains("missing bundle target static_page"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn reveal_origin_links_build_artifact_back_to_source_and_route() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let out = temp_output_dir("reveal-origin");

        cmd_build(&path, &out).expect("build artifacts");
        let origin_map: orv_compiler::OriginMap = serde_json::from_str(
            &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
        )
        .expect("origin map json");
        let route = origin_map
            .entries
            .iter()
            .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
            .expect("route origin");

        let reveal = reveal_origin_json(&out, &route.id).expect("reveal origin");

        assert_eq!(reveal["schema_version"], 1);
        assert_eq!(reveal["origin"]["id"], route.id);
        assert_eq!(reveal["origin"]["kind"], "route");
        assert_eq!(reveal["origin"]["name"], "GET /ping");
        let canonical_path = std::fs::canonicalize(&path).expect("canonical entry path");
        assert_eq!(
            reveal["source"]["path"],
            canonical_path.display().to_string()
        );
        assert!(reveal["source"]["snippet"]
            .as_str()
            .is_some_and(|snippet| snippet.contains("@route GET /ping")));
        assert_eq!(reveal["project_graph"]["kind"], "domain");
        assert_eq!(reveal["project_graph"]["name"], "route");
        assert!(reveal["production"]["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .any(|route| route["method"] == "GET" && route["path"] == "/ping"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn reveal_origin_links_client_signal_to_client_bundle_targets() {
        let out = temp_output_dir("reveal-client-origin");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            r#"let sig count: int = 0
@out @html { @body { @p count } }"#,
        )
        .expect("write entry");
        let build_out = out.join("dist");

        cmd_build(&entry, &build_out).expect("build artifacts");
        let origin_map: orv_compiler::OriginMap = serde_json::from_str(
            &std::fs::read_to_string(build_out.join("origin-map.json")).expect("origin map"),
        )
        .expect("origin map json");
        let signal = origin_map
            .entries
            .iter()
            .find(|entry| entry.kind == "signal" && entry.name == "count")
            .expect("signal origin");

        let reveal = reveal_origin_json(&build_out, &signal.id).expect("reveal origin");

        assert_eq!(reveal["origin"]["kind"], "signal");
        assert!(reveal["source"]["snippet"]
            .as_str()
            .is_some_and(|snippet| snippet.contains("let sig count")));
        let client = reveal["production"]["client"]
            .as_array()
            .expect("client targets");
        assert!(client
            .iter()
            .any(|target| target["kind"] == "client_page" && target["path"] == "pages/index.html"));
        assert!(client
            .iter()
            .any(|target| target["kind"] == "client_js" && target["path"] == "client/app.js"));
        assert!(client
            .iter()
            .any(|target| target["kind"] == "client_wasm" && target["path"] == "client/app.wasm"));
        assert!(reveal["production"]["routes"]
            .as_array()
            .expect("routes")
            .is_empty());
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn reveal_origin_uses_build_source_bundle_when_original_client_source_is_missing() {
        let out = temp_output_dir("reveal-client-source-bundle");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            r#"let sig count: int = 0
@out @html { @body { @p count } }"#,
        )
        .expect("write entry");
        let build_out = out.join("dist");

        cmd_build(&entry, &build_out).expect("build artifacts");
        let origin_map: orv_compiler::OriginMap = serde_json::from_str(
            &std::fs::read_to_string(build_out.join("origin-map.json")).expect("origin map"),
        )
        .expect("origin map json");
        let signal = origin_map
            .entries
            .iter()
            .find(|entry| entry.kind == "signal" && entry.name == "count")
            .expect("signal origin");
        std::fs::remove_file(&entry).expect("remove original source");

        let reveal = reveal_origin_json(&build_out, &signal.id).expect("reveal origin");

        assert!(reveal["source"]["snippet"]
            .as_str()
            .is_some_and(|snippet| snippet.contains("let sig count")));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn lsp_reveal_returns_location_for_build_origin() {
        let dir = temp_output_dir("lsp-reveal");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("app.orv");
        std::fs::write(
            &path,
            r#"@server {
  @listen 0
  @route GET /ping {
    @respond 200 { ok: true }
  }
}"#,
        )
        .expect("write source");
        let out = dir.join("dist");

        cmd_build(&path, &out).expect("build artifacts");
        let origin_map: orv_compiler::OriginMap = serde_json::from_str(
            &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
        )
        .expect("origin map json");
        let route = origin_map
            .entries
            .iter()
            .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
            .expect("route origin");

        let reveal = lsp_reveal_json(&out, &route.id).expect("lsp reveal");

        assert_eq!(reveal["schema_version"], 1);
        assert_eq!(reveal["origin"]["id"], route.id);
        let canonical_path = std::fs::canonicalize(&path).expect("canonical source path");
        assert_eq!(
            reveal["location"]["uri"],
            canonical_path.display().to_string()
        );
        assert_eq!(reveal["location"]["range"]["start"]["line"], 2);
        assert_eq!(reveal["location"]["range"]["start"]["character"], 2);
        assert!(reveal["production"]["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .any(|route| route["method"] == "GET" && route["path"] == "/ping"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lsp_reveal_uses_build_source_bundle_when_original_source_is_missing() {
        let dir = temp_output_dir("lsp-reveal-source-bundle");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("page.orv");
        std::fs::write(
            &path,
            r#"let sig count: int = 0
@out @html { @body { @p count } }"#,
        )
        .expect("write source");
        let out = dir.join("dist");

        cmd_build(&path, &out).expect("build artifacts");
        let origin_map: orv_compiler::OriginMap = serde_json::from_str(
            &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
        )
        .expect("origin map json");
        let signal = origin_map
            .entries
            .iter()
            .find(|entry| entry.kind == "signal" && entry.name == "count")
            .expect("signal origin");
        std::fs::remove_file(&path).expect("remove source");

        let reveal = lsp_reveal_json(&out, &signal.id).expect("lsp reveal");

        assert_eq!(reveal["origin"]["kind"], "signal");
        assert_eq!(reveal["location"]["range"]["start"]["line"], 0);
        assert!(reveal["production"]["client"]
            .as_array()
            .expect("client targets")
            .iter()
            .any(|target| target["kind"] == "client_wasm"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn check_build_reanalyzes_source_bundle_without_original_source() {
        let dir = temp_output_dir("check-build-source-bundle");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("page.orv");
        std::fs::write(
            &path,
            r#"let sig count: int = 0
@out @html { @body { @p count } }"#,
        )
        .expect("write source");
        let out = dir.join("dist");

        cmd_build(&path, &out).expect("build artifacts");
        std::fs::remove_file(&path).expect("remove source");

        cmd_check_build(&out).expect("check build");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn run_build_executes_server_launch_artifact_relative_to_build_dir() {
        let out = temp_output_dir("run-build");
        let artifact = out.join("server").join("app.orv-runtime.json");
        write_reference_artifact(&artifact, "artifact.orv", r#"@out "build ok""#);
        let launch = orv_compiler::ServerLaunchArtifact {
            schema_version: orv_compiler::SERVER_LAUNCH_ARTIFACT_VERSION,
            runtime: "reference-interpreter".to_string(),
            artifact: "server/app.orv-runtime.json".to_string(),
            command: vec![
                "orv".to_string(),
                "run-artifact".to_string(),
                "server/app.orv-runtime.json".to_string(),
            ],
            protocol: "http1".to_string(),
            routes: Vec::new(),
            listen: None,
        };
        write_json(
            &out.join("server").join("launch.json"),
            &serde_json::to_value(launch).expect("launch value"),
        )
        .expect("write launch");
        let mut stdout = Vec::new();

        run_build_with_writer(&out, &mut stdout).expect("run build");

        assert_eq!(
            String::from_utf8(stdout).expect("stdout utf-8"),
            "build ok\n"
        );
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn run_build_prints_zero_runtime_static_page() {
        let out = temp_output_dir("run-build-static");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(&entry, r#"@out @html { @body { @h1 "Static" } }"#).expect("write entry");
        let build_out = out.join("dist");
        cmd_build(&entry, &build_out).expect("build artifacts");
        let mut stdout = Vec::new();

        run_build_with_writer(&build_out, &mut stdout).expect("run build");

        assert_eq!(
            String::from_utf8(stdout).expect("stdout utf-8"),
            "<html><body><h1>Static</h1></body></html>"
        );
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn run_build_prints_client_page_shell() {
        let out = temp_output_dir("run-build-client-page");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            r#"let sig count: int = 0
@out @html { @body { @p count } }"#,
        )
        .expect("write entry");
        let build_out = out.join("dist");
        cmd_build(&entry, &build_out).expect("build artifacts");
        let mut stdout = Vec::new();

        run_build_with_writer(&build_out, &mut stdout).expect("run build");

        let html = String::from_utf8(stdout).expect("stdout utf-8");
        assert!(html.contains("data-orv-client=\"wasm\""));
        assert!(html.contains("../client/app.js"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn run_build_uses_bundle_plan_instead_of_stale_server_launcher() {
        let out = temp_output_dir("run-build-static-stale-server");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(&entry, r#"@out @html { @body { @h1 "Fresh" } }"#).expect("write entry");
        let build_out = out.join("dist");
        cmd_build(&entry, &build_out).expect("build artifacts");
        let stale_launch = build_out.join("server").join("launch.json");
        if let Some(parent) = stale_launch.parent() {
            std::fs::create_dir_all(parent).expect("create stale server dir");
        }
        std::fs::write(&stale_launch, "{ stale").expect("write stale launch");
        let mut stdout = Vec::new();

        run_build_with_writer(&build_out, &mut stdout).expect("run build");

        assert_eq!(
            String::from_utf8(stdout).expect("stdout utf-8"),
            "<html><body><h1>Fresh</h1></body></html>"
        );
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn dev_builds_verifies_and_runs_static_page() {
        let out = temp_output_dir("dev-static");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(&entry, r#"@out @html { @body { @h1 "Dev" } }"#).expect("write entry");
        let build_out = out.join("dist");
        let mut stdout = Vec::new();

        dev_with_writer(&entry, &build_out, &mut stdout).expect("dev");

        assert!(build_out.join("build-manifest.json").is_file());
        assert!(build_out.join("bundle-plan.json").is_file());
        assert_eq!(
            String::from_utf8(stdout).expect("stdout utf-8"),
            "<html><body><h1>Dev</h1></body></html>"
        );
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn dev_hmr_writes_session_manifest_for_client_page() {
        let out = temp_output_dir("dev-hmr-session");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(
            &entry,
            "let sig count: int = 0\n@out @html { @body { @p count } }",
        )
        .expect("write entry");
        let build_out = out.join("dist");
        let mut stdout = Vec::new();
        let canonical_entry = std::fs::canonicalize(&entry).expect("canonical entry");

        dev_with_writer_with_options(&entry, &build_out, true, false, &mut stdout)
            .expect("dev hmr");

        let session =
            read_json_value(&build_out.join("dev").join("session.json")).expect("dev session");
        assert_eq!(session["schema_version"], 1);
        assert_eq!(session["mode"], "hmr");
        assert_eq!(session["source_bundle"], "source-bundle.json");
        assert_eq!(session["reload"]["strategy"], "hot-reload");
        assert_eq!(session["reload"]["fallback"], "full-reload");
        assert!(session["watch"]["sources"]
            .as_array()
            .expect("watch sources")
            .iter()
            .any(|source| {
                source["path"] == canonical_entry.display().to_string()
                    && source["content_hash"]
                        .as_str()
                        .is_some_and(|hash| hash.starts_with("fnv1a64:"))
            }));
        assert!(session["watch"]["targets"]
            .as_array()
            .expect("watch targets")
            .iter()
            .any(|target| {
                target["kind"] == "client_wasm"
                    && target["path"] == "client/app.wasm"
                    && target["runtime_features"]
                        .as_array()
                        .expect("runtime features")
                        .iter()
                        .any(|feature| feature == "client_wasm")
            }));
        cmd_verify_build(&build_out).expect("verify dev hmr build");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn dev_watch_writes_watch_session_manifest() {
        let out = temp_output_dir("dev-watch-session");
        std::fs::create_dir_all(&out).expect("create temp root");
        let entry = out.join("page.orv");
        std::fs::write(&entry, "@out @html { @body { @h1 \"Watch\" } }").expect("write entry");
        let build_out = out.join("dist");
        let mut stdout = Vec::new();
        let canonical_entry = std::fs::canonicalize(&entry).expect("canonical entry");

        dev_with_writer_with_options(&entry, &build_out, false, true, &mut stdout)
            .expect("dev watch");

        let watch =
            read_json_value(&build_out.join("dev").join("watch.json")).expect("watch session");
        assert_eq!(watch["schema_version"], 1);
        assert_eq!(watch["mode"], "watch");
        assert_eq!(watch["source_bundle"], "source-bundle.json");
        assert_eq!(watch["loop"]["strategy"], "poll");
        assert_eq!(watch["loop"]["run"], "build-verify-run");
        assert_eq!(watch["reload"]["strategy"], "full-reload");
        assert!(watch["watch"]["sources"]
            .as_array()
            .expect("watch sources")
            .iter()
            .any(|source| {
                source["path"] == canonical_entry.display().to_string()
                    && source["content_hash"]
                        .as_str()
                        .is_some_and(|hash| hash.starts_with("fnv1a64:"))
            }));
        assert!(watch["watch"]["targets"]
            .as_array()
            .expect("watch targets")
            .iter()
            .any(|target| target["kind"] == "static_page" && target["path"] == "pages/index.html"));
        cmd_verify_build(&build_out).expect("verify dev watch build");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn verify_artifact_accepts_generated_server_runtime_artifact() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let out = temp_output_dir("verify-artifact");

        cmd_build(&path, &out).expect("build artifacts");
        let artifact = out.join("server").join("app.orv-runtime.json");

        cmd_verify_artifact(&artifact).expect("verify artifact");

        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn check_artifact_rehydrates_generated_server_runtime_artifact() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let out = temp_output_dir("check-artifact");

        cmd_build(&path, &out).expect("build artifacts");
        let artifact = out.join("server").join("app.orv-runtime.json");

        cmd_check_artifact(&artifact).expect("check artifact");

        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn run_artifact_rehydrates_and_runs_source_bundle() {
        let out = temp_output_dir("run-artifact");
        let artifact = out.join("app.orv-runtime.json");
        write_reference_artifact(&artifact, "artifact.orv", r#"@out "artifact ok""#);
        let mut stdout = Vec::new();

        run_artifact_with_writer(&artifact, &mut stdout).expect("run artifact");

        assert_eq!(
            String::from_utf8(stdout).expect("stdout utf-8"),
            "artifact ok\n"
        );
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn run_artifact_rehydrates_imported_source_bundle() {
        let out = temp_output_dir("run-artifact-import");
        let artifact = out.join("app.orv-runtime.json");
        write_reference_artifact_with_sources(
            &artifact,
            "main.orv",
            [
                (
                    "main.orv",
                    "import models.user.User\nlet u: User = { name: \"Ada\" }\n@out u.name",
                ),
                ("models/user.orv", "pub struct User { name: string }"),
            ],
        );
        let mut stdout = Vec::new();

        run_artifact_with_writer(&artifact, &mut stdout).expect("run artifact");

        assert_eq!(String::from_utf8(stdout).expect("stdout utf-8"), "Ada\n");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn run_artifact_rejects_corrupt_source_bundle() {
        let out = temp_output_dir("run-artifact-corrupt");
        let artifact_path = out.join("app.orv-runtime.json");
        write_reference_artifact(&artifact_path, "artifact.orv", r#"@out "artifact ok""#);
        let mut artifact: orv_compiler::ServerRuntimeArtifact =
            serde_json::from_str(&std::fs::read_to_string(&artifact_path).expect("artifact json"))
                .expect("artifact");
        artifact.source_bundle.files[0].source = r#"@out "tampered""#.to_string();
        write_json(
            &artifact_path,
            &serde_json::to_value(artifact).expect("artifact value"),
        )
        .expect("write artifact");
        let mut stdout = Vec::new();

        let err = run_artifact_with_writer(&artifact_path, &mut stdout).expect_err("hash mismatch");

        assert!(err.to_string().contains("content hash mismatch"));
        assert!(stdout.is_empty());
        let _ = std::fs::remove_dir_all(&out);
    }

    fn write_reference_artifact(path: &Path, entry: &str, source: &str) {
        write_reference_artifact_with_sources(path, entry, [(entry, source)]);
    }

    fn write_reference_artifact_with_sources<'a>(
        path: &Path,
        entry: &str,
        sources: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) {
        let manifest = orv_compiler::BuildManifest {
            schema_version: orv_compiler::BUILD_MANIFEST_VERSION,
            entry: entry.to_string(),
            runtime: "reference-interpreter".to_string(),
            artifacts: Vec::new(),
            capabilities: orv_compiler::BuildCapabilities {
                has_server: false,
                server_routes: 0,
                client_wasm: false,
                runtime_features: vec!["console_io".to_string()],
            },
        };
        let origin_map = orv_compiler::OriginMap {
            version: orv_compiler::ORIGIN_MAP_VERSION,
            entries: Vec::new(),
            edges: Vec::new(),
        };
        let artifact = orv_compiler::server_runtime_artifact(&manifest, &origin_map, sources);
        write_json(
            path,
            &serde_json::to_value(artifact).expect("artifact value"),
        )
        .expect("write artifact");
    }

    #[test]
    fn graph_json_for_path_outputs_schema_nodes_and_edges() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let value = project_graph_json_for_path(&path).expect("graph json");

        assert_eq!(value["schema_version"], 1);
        let nodes = value["nodes"].as_array().expect("nodes array");
        let edges = value["edges"].as_array().expect("edges array");
        assert!(nodes.iter().any(|node| node["kind"] == "file"));
        assert!(nodes.iter().any(|node| node["kind"] == "domain"));
        assert!(edges.iter().any(|edge| edge["kind"] == "contains"));
        assert_eq!(value["stats"]["node_count"], nodes.len());
        assert_eq!(value["stats"]["edge_count"], edges.len());
        assert_eq!(value["stats"]["file_count"], 1);
        assert!(
            value["stats"]["max_semantic_contains_depth"]
                .as_u64()
                .expect("semantic depth")
                >= 2
        );
    }

    #[test]
    fn graph_json_for_path_includes_semantic_origin_map() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let value = project_graph_json_for_path(&path).expect("graph json");
        let entries = value["semantic"]["origin_map"]["entries"]
            .as_array()
            .expect("origin entries array");

        assert!(entries
            .iter()
            .any(|entry| entry["kind"] == "route" && entry["name"] == "GET /ping"));
        assert!(entries
            .iter()
            .any(|entry| entry["kind"] == "domain" && entry["name"] == "respond"));
    }

    #[test]
    fn graph_json_links_semantic_origins_to_ast_nodes() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let value = project_graph_json_for_path(&path).expect("graph json");
        let nodes = value["nodes"].as_array().expect("nodes array");
        let route_node = nodes
            .iter()
            .find(|node| node["kind"] == "domain" && node["name"] == "route")
            .expect("route AST node");
        let route_origin = value["semantic"]["origin_map"]["entries"]
            .as_array()
            .expect("origin entries array")
            .iter()
            .find(|entry| entry["kind"] == "route" && entry["name"] == "GET /ping")
            .expect("route origin");
        let links = value["semantic"]["origin_links"]
            .as_array()
            .expect("origin links array");

        assert!(links.iter().any(|link| {
            link["kind"] == "source_node"
                && link["origin_id"] == route_origin["id"]
                && link["node_id"] == route_node["id"]
        }));
    }

    #[test]
    fn graph_json_includes_semantic_origin_edges() {
        let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
        let value = project_graph_json_for_path(&path).expect("graph json");
        let entries = value["semantic"]["origin_map"]["entries"]
            .as_array()
            .expect("origin entries array");
        let server = entries
            .iter()
            .find(|entry| entry["kind"] == "domain" && entry["name"] == "server")
            .expect("server origin");
        let route = entries
            .iter()
            .find(|entry| entry["kind"] == "route" && entry["name"] == "GET /ping")
            .expect("route origin");
        let respond = entries
            .iter()
            .find(|entry| entry["kind"] == "domain" && entry["name"] == "respond")
            .expect("respond origin");
        let edges = value["semantic"]["origin_edges"]
            .as_array()
            .expect("origin edges array");

        assert!(edges.iter().any(|edge| {
            edge["kind"] == "contains" && edge["from"] == server["id"] && edge["to"] == route["id"]
        }));
        assert!(edges.iter().any(|edge| {
            edge["kind"] == "contains" && edge["from"] == route["id"] && edge["to"] == respond["id"]
        }));
    }

    #[test]
    fn graph_json_exposes_call_edges_from_origin_map() {
        let path = workspace_path(&["fixtures", "plan", "01-basics.orv"]);
        let value = project_graph_json_for_path(&path).expect("graph json");
        let edges = value["semantic"]["origin_edges"]
            .as_array()
            .expect("origin edges array");

        assert!(edges.iter().any(|edge| edge["kind"] == "calls"));
    }

    #[test]
    fn rendered_diagnostics_use_span_file_source() {
        let files = vec![
            orv_project::SourceFile {
                id: FileId(0),
                path: PathBuf::from("main.orv"),
                source: "import models.user.User\nlet u: User = { name: \"ok\" }\n".to_string(),
            },
            orv_project::SourceFile {
                id: FileId(1),
                path: PathBuf::from("models/user.orv"),
                source: "pub struct User { name: string }\nlet bad: int = \"wrong\"\n".to_string(),
            },
        ];
        let start =
            u32::try_from(files[1].source.find("\"wrong\"").unwrap()).expect("offset fits u32");
        let len = u32::try_from("\"wrong\"".len()).expect("length fits u32");
        let diag = orv_diagnostics::Diagnostic::error(
            "type mismatch: `bad` annotated as `int` but value has type `string`",
        )
        .with_primary(
            orv_diagnostics::Span::new(
                FileId(1),
                orv_diagnostics::ByteRange::new(start, start + len),
            ),
            "value has type `string`",
        );

        let rendered = render_diagnostics_for_test(&[diag], &files);
        assert!(rendered.contains("models/user.orv"), "{rendered}");
        assert!(rendered.contains("let bad: int = \"wrong\""), "{rendered}");
        assert!(
            !rendered.contains("let u: User = { name: \"ok\" }"),
            "{rendered}"
        );
    }
}
