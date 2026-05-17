use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "orv", about = "orv language toolchain", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
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
        /// 정적 HTML graph view artifact를 작성한다.
        #[arg(long)]
        view: bool,
        /// graph view 출력 디렉터리.
        #[arg(long, short = 'o', default_value = "target/orv-graph-view")]
        out: PathBuf,
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
    /// production deploy artifact의 provider credential env 설정을 검사한다.
    DeployEnvCheck {
        /// 검사할 build artifact 디렉터리.
        dir: PathBuf,
    },
    /// deploy benchmark evidence를 검증하고 JSON report를 출력한다.
    BenchmarkReport {
        /// 검사할 build artifact 디렉터리.
        dir: PathBuf,
        /// report status가 passed가 아니면 실패 코드로 종료한다.
        #[arg(long)]
        require_pass: bool,
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
    /// orv.toml dependency metadata를 orv.lock artifact로 고정한다.
    Lock {
        /// orv.toml이 있는 프로젝트 디렉터리 또는 manifest 경로.
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// 기존 orv.lock이 최신인지 확인하고 쓰지는 않는다.
        #[arg(long)]
        check: bool,
    },
    /// `orv.lock` dependency source들을 local dependency artifact로 가져온다.
    Fetch {
        /// orv.toml/orv.lock이 있는 프로젝트 디렉터리 또는 manifest 경로.
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// dependency artifact 출력 디렉터리.
        #[arg(long, short = 'o', default_value = "target/orv-deps")]
        out: PathBuf,
    },
    /// `orv.toml`에 dependency를 추가하고 `orv.lock`을 갱신한다.
    Add {
        /// 추가할 package 이름.
        pkg: String,
        /// registry dependency version. path dependency에서는 생략 가능하다.
        version: Option<String>,
        /// `orv.toml`이 있는 프로젝트 디렉터리 또는 manifest 경로.
        #[arg(long, default_value = ".")]
        manifest: PathBuf,
        /// `[dev-dependencies]`에 추가한다.
        #[arg(long)]
        dev: bool,
        /// path dependency 경로.
        #[arg(long)]
        path: Option<PathBuf>,
        /// registry source override.
        #[arg(long)]
        registry: Option<String>,
    },
    /// `orv.toml`에서 dependency를 제거하고 `orv.lock`을 갱신한다.
    Remove {
        /// 제거할 package 이름.
        pkg: String,
        /// `orv.toml`이 있는 프로젝트 디렉터리 또는 manifest 경로.
        #[arg(long, default_value = ".")]
        manifest: PathBuf,
        /// `[dev-dependencies]`에서 제거한다.
        #[arg(long)]
        dev: bool,
    },
    /// server runtime artifact source bundle을 재수화하고 실행한다.
    RunArtifact {
        /// 실행할 artifact JSON 경로.
        file: PathBuf,
        /// graceful shutdown 때 쓸 production request trace JSON 경로.
        #[arg(long)]
        trace: Option<PathBuf>,
    },
    /// build artifact 디렉터리의 server launcher를 실행한다.
    RunBuild {
        /// 실행할 build artifact 디렉터리.
        dir: PathBuf,
        /// graceful shutdown 때 쓸 production request trace JSON 경로.
        #[arg(long)]
        trace: Option<PathBuf>,
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
        /// persistent watch loop를 실행한다.
        #[arg(long)]
        watch_loop: bool,
        /// reference HMR `EventSource` dev endpoint를 실행한다.
        #[arg(long)]
        serve: bool,
        /// HMR dev endpoint port. 0이면 OS가 빈 port를 고른다.
        #[arg(long, default_value_t = 0)]
        serve_port: u16,
        /// watch loop 반복 횟수. 생략하면 계속 실행한다.
        #[arg(long)]
        watch_iterations: Option<u64>,
        /// watch loop poll interval milliseconds.
        #[arg(long, default_value_t = 500)]
        watch_interval_ms: u64,
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
    /// Workspace helper commands.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
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
    /// First-party editor helper commands.
    Editor {
        #[command(subcommand)]
        command: EditorCommand,
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
pub enum InitTemplate {
    Basic,
    Shop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum EditorDebugControl {
    Continue,
    Pause,
    ReverseContinue,
    Next,
    StepBack,
    StepIn,
    StepInTargets,
    StepOut,
    RestartFrame,
    Restart,
    Terminate,
    TerminateThreads,
    Disconnect,
}

impl EditorDebugControl {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Continue => "Continue",
            Self::Pause => "Pause",
            Self::ReverseContinue => "Reverse Continue",
            Self::Next => "Next",
            Self::StepBack => "Step Back",
            Self::StepIn => "Step In",
            Self::StepInTargets => "Step In Targets",
            Self::StepOut => "Step Out",
            Self::RestartFrame => "Restart Frame",
            Self::Restart => "Restart",
            Self::Terminate => "Terminate",
            Self::TerminateThreads => "Terminate Threads",
            Self::Disconnect => "Disconnect",
        }
    }

    pub const fn cli_value(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Pause => "pause",
            Self::ReverseContinue => "reverse-continue",
            Self::Next => "next",
            Self::StepBack => "step-back",
            Self::StepIn => "step-in",
            Self::StepInTargets => "step-in-targets",
            Self::StepOut => "step-out",
            Self::RestartFrame => "restart-frame",
            Self::Restart => "restart",
            Self::Terminate => "terminate",
            Self::TerminateThreads => "terminate-threads",
            Self::Disconnect => "disconnect",
        }
    }

    pub fn request_json(self) -> serde_json::Value {
        match self {
            Self::Continue => serde_json::json!({
                "command": "continue",
                "arguments": {"threadId": 1},
            }),
            Self::Pause => serde_json::json!({
                "command": "pause",
                "arguments": {"threadId": 1},
            }),
            Self::ReverseContinue => serde_json::json!({
                "command": "reverseContinue",
                "arguments": {"threadId": 1},
            }),
            Self::Next => serde_json::json!({
                "command": "next",
                "arguments": {"threadId": 1},
            }),
            Self::StepBack => serde_json::json!({
                "command": "stepBack",
                "arguments": {"threadId": 1},
            }),
            Self::StepIn => serde_json::json!({
                "command": "stepIn",
                "arguments": {"threadId": 1},
            }),
            Self::StepInTargets => serde_json::json!({
                "command": "stepInTargets",
                "arguments": {"frameId": 1},
            }),
            Self::StepOut => serde_json::json!({
                "command": "stepOut",
                "arguments": {"threadId": 1},
            }),
            Self::RestartFrame => serde_json::json!({
                "command": "restartFrame",
                "arguments": {"frameId": 1},
            }),
            Self::Restart => serde_json::json!({
                "command": "restart",
                "arguments": {},
            }),
            Self::Terminate => serde_json::json!({
                "command": "terminate",
                "arguments": {},
            }),
            Self::TerminateThreads => serde_json::json!({
                "command": "terminateThreads",
                "arguments": {"threadIds": [1]},
            }),
            Self::Disconnect => serde_json::json!({
                "command": "disconnect",
                "arguments": {"terminateDebuggee": true},
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorDebugBreakpoint {
    pub path: PathBuf,
    pub line: u64,
}

pub type EditorDebugDataBreakpointInfoRequest = (u64, String, serde_json::Value);
pub type EditorDebugDataBreakpointSetRequest = (u64, Vec<String>, serde_json::Value);

fn parse_editor_debug_breakpoint(value: &str) -> Result<EditorDebugBreakpoint, String> {
    let (path, line) = value
        .rsplit_once(':')
        .ok_or_else(|| "breakpoint must be formatted as <path>:<line>".to_string())?;
    if path.is_empty() {
        return Err("breakpoint path must not be empty".to_string());
    }
    let line = line
        .parse::<u64>()
        .map_err(|_| "breakpoint line must be a positive integer".to_string())?;
    if line == 0 {
        return Err("breakpoint line must be greater than zero".to_string());
    }
    Ok(EditorDebugBreakpoint {
        path: PathBuf::from(path),
        line,
    })
}

fn parse_editor_debug_function_breakpoint(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("function breakpoint name must not be empty".to_string());
    }
    Ok(name.to_string())
}

fn parse_editor_debug_data_breakpoint(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("data breakpoint local name must not be empty".to_string());
    }
    Ok(name.to_string())
}

fn parse_editor_debug_exception_filter(value: &str) -> Result<String, String> {
    let filter = value.trim();
    if matches!(filter, "orv.diagnostics" | "orv.runtime") {
        Ok(filter.to_string())
    } else {
        Err("exception filter must be `orv.diagnostics` or `orv.runtime`".to_string())
    }
}

#[derive(Subcommand)]
pub enum WorkspaceCommand {
    /// 워크스페이스 member 프로젝트를 생성하고 root manifest에 등록한다.
    New {
        /// 생성할 member 경로.
        member: PathBuf,
        /// workspace root 디렉터리.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// member 프로젝트 이름.
        #[arg(long)]
        name: Option<String>,
        /// 생성할 starter template.
        #[arg(long, value_enum, default_value_t = InitTemplate::Basic)]
        template: InitTemplate,
    },
    /// Workspace member project graph들을 하나의 artifact로 출력한다.
    Graph {
        /// workspace root 디렉터리.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// 정적 HTML workspace graph view artifact를 작성한다.
        #[arg(long)]
        view: bool,
        /// workspace graph artifact 출력 디렉터리.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Workspace member lockfile들을 한 디렉터리 아래 생성한다.
    Lock {
        /// workspace root 디렉터리.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// workspace lock artifact 출력 디렉터리.
        #[arg(long, short = 'o', default_value = "target/orv-workspace-lock")]
        out: PathBuf,
    },
    /// Workspace member dependency cache들을 한 디렉터리 아래 생성한다.
    Fetch {
        /// workspace root 디렉터리.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// workspace dependency artifact 출력 디렉터리.
        #[arg(long, short = 'o', default_value = "target/orv-workspace-deps")]
        out: PathBuf,
    },
    /// Workspace member build artifact들을 한 디렉터리 아래 생성한다.
    Build {
        /// workspace root 디렉터리.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// workspace build artifact 출력 디렉터리.
        #[arg(long, short = 'o', default_value = "target/orv-workspace-build")]
        out: PathBuf,
        /// member build를 production profile로 생성한다.
        #[arg(long)]
        prod: bool,
        /// 이전 workspace build manifest와 source hash가 같으면 member build를 건너뛴다.
        #[arg(long)]
        incremental: bool,
    },
}

#[derive(Subcommand)]
pub enum EditorCommand {
    /// 현재 파일의 first-party editor bootstrap snapshot JSON을 출력한다.
    Snapshot {
        /// 대상 파일 경로.
        file: PathBuf,
    },
    /// build artifact origin id를 first-party editor navigation JSON으로 변환한다.
    Reveal {
        /// 검사할 build artifact 디렉터리.
        dir: PathBuf,
        /// reveal 할 origin id.
        origin_id: String,
    },
    /// 현재 파일의 first-party editor runtime inspection JSON을 출력한다.
    Runtime {
        /// 대상 파일 경로.
        file: PathBuf,
    },
    /// first-party editor DAP control transport smoke JSON을 출력한다.
    Debug {
        /// 대상 파일 경로.
        file: PathBuf,
        /// Apply a DAP source breakpoint before controls, formatted as `<path>:<line>`.
        #[arg(long = "breakpoint", value_parser = parse_editor_debug_breakpoint)]
        breakpoints: Vec<EditorDebugBreakpoint>,
        /// Apply a DAP function breakpoint before launch.
        #[arg(long = "function-breakpoint", value_parser = parse_editor_debug_function_breakpoint)]
        function_breakpoints: Vec<String>,
        /// Apply a DAP data breakpoint for a local variable before controls.
        #[arg(long = "data-breakpoint", value_parser = parse_editor_debug_data_breakpoint)]
        data_breakpoints: Vec<String>,
        /// Configure DAP exception filters before launch.
        #[arg(long = "exception-filter", value_parser = parse_editor_debug_exception_filter)]
        exception_filters: Vec<String>,
        /// 실행할 DAP control request. 여러 번 지정하면 같은 session 에서 순서대로 실행한다.
        #[arg(long = "control", value_enum)]
        controls: Vec<EditorDebugControl>,
        /// Evaluate a DAP watch expression after controls.
        #[arg(long = "watch-expression")]
        watch_expressions: Vec<String>,
    },
    /// exported editor state 의 debug session runner를 실행한다.
    RunDebug {
        /// `orv editor export`가 쓴 state.json 또는 debug/session-runner.json 경로.
        state: PathBuf,
        /// Apply a DAP source breakpoint before controls, formatted as `<path>:<line>`.
        #[arg(long = "breakpoint", value_parser = parse_editor_debug_breakpoint)]
        breakpoints: Vec<EditorDebugBreakpoint>,
        /// Apply a DAP function breakpoint before launch.
        #[arg(long = "function-breakpoint", value_parser = parse_editor_debug_function_breakpoint)]
        function_breakpoints: Vec<String>,
        /// Apply a DAP data breakpoint for a local variable before controls.
        #[arg(long = "data-breakpoint", value_parser = parse_editor_debug_data_breakpoint)]
        data_breakpoints: Vec<String>,
        /// Configure DAP exception filters before launch.
        #[arg(long = "exception-filter", value_parser = parse_editor_debug_exception_filter)]
        exception_filters: Vec<String>,
        /// 실행할 DAP control request. 여러 번 지정하면 같은 session 에서 순서대로 실행한다.
        #[arg(long = "control", value_enum)]
        controls: Vec<EditorDebugControl>,
        /// Evaluate a DAP watch expression after controls.
        #[arg(long = "watch-expression")]
        watch_expressions: Vec<String>,
    },
    /// first-party editor static UI artifact를 출력한다.
    Export {
        /// 대상 파일 경로.
        file: PathBuf,
        /// 출력 디렉터리.
        #[arg(long)]
        out: PathBuf,
        /// trace origin reveal에 사용할 build artifact 디렉터리.
        #[arg(long)]
        build: Option<PathBuf>,
        /// 함께 embed 할 production request trace JSON 경로.
        #[arg(long)]
        trace: Option<PathBuf>,
    },
    /// production request trace를 first-party editor navigation JSON으로 변환한다.
    Trace {
        /// 검사할 build artifact 디렉터리.
        dir: PathBuf,
        /// request frame trace JSON 경로.
        #[arg(long)]
        trace: PathBuf,
    },
    /// `EventSource` trace snapshot을 first-party editor trace JSON으로 변환한다.
    TraceStream {
        /// 검사할 build artifact 디렉터리.
        dir: PathBuf,
        /// `EventSource` snapshot body 경로.
        #[arg(long)]
        events: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum DbCommand {
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
    /// local backup artifact, raw WAL, 또는 WAL archive에서 @db.save JSON data snapshot을 복원한다.
    Restore {
        /// 읽을 backup artifact JSON 경로.
        #[arg(long)]
        backup: Option<PathBuf>,
        /// 읽을 @db.wal JSONL 경로.
        #[arg(long)]
        wal: Option<PathBuf>,
        /// 읽을 WAL archive manifest JSON 경로.
        #[arg(long)]
        archive: Option<PathBuf>,
        /// 복원할 @db.save JSON data snapshot 경로.
        #[arg(long)]
        data: PathBuf,
        /// raw WAL/archive에서 복구할 RFC3339 point-in-time timestamp.
        #[arg(long)]
        at: Option<String>,
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
    /// WAL crash/recovery matrix를 실행하고 JSON report를 쓴다.
    CrashMatrix {
        /// 쓸 crash matrix report JSON 경로.
        #[arg(long)]
        out: PathBuf,
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
pub enum LspCommand {
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
pub enum DapCommand {
    /// stdin/stdout Debug Adapter Protocol server bootstrap을 실행한다.
    Serve {
        /// stdin/stdout transport를 사용한다.
        #[arg(long)]
        stdio: bool,
    },
}
