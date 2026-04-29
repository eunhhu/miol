//! orv CLI 프론트엔드 — `orv` 바이너리.
//!
//! MVP: `orv run <file>`로 `.orv` 파일을 tree-walking 인터프리터로 실행한다.
//! 이후 `orv build`, `orv check`, `orv dev` 등이 추가된다.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use orv_diagnostics::FileId;

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
    }
}

fn cmd_run(path: &Path) -> anyhow::Result<()> {
    // B3: entry 파일에서 시작해 import 를 따라 multi-file 을 하나의 Program 으로
    // 병합한다. import 가 없으면 entry 한 파일만 로드되므로 기존 동작과 동일.
    let loaded = orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    report_diagnostics(&loaded.diagnostics, path)?;

    let resolved = orv_resolve::resolve(&loaded.program);
    report_diagnostics(&resolved.diagnostics, path)?;

    // B5: 타입 진단도 보고. 에러면 실행 전에 중단.
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    report_diagnostics(&lowered.diagnostics, path)?;
    orv_runtime::run(&lowered.program).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn cmd_check(path: &Path) -> anyhow::Result<()> {
    // 파일을 읽어 파싱과 타입 검사만 수행하고 실행은 하지 않는다.
    let loaded = orv_project::load_project(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    report_diagnostics(&loaded.diagnostics, path)?;

    let resolved = orv_resolve::resolve(&loaded.program);
    report_diagnostics(&resolved.diagnostics, path)?;

    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    report_diagnostics(&lowered.diagnostics, path)?;

    println!("check: {} passed", path.display());
    Ok(())
}

fn cmd_dump(path: &PathBuf) -> anyhow::Result<()> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let file_id = FileId(0);
    let lx = orv_syntax::lex(&source, file_id);
    report_diagnostics(&lx.diagnostics, path)?;
    let pr = orv_syntax::parse_with_newlines(lx.tokens, file_id, lx.newlines);
    report_diagnostics(&pr.diagnostics, path)?;
    println!("{:#?}", pr.program);
    Ok(())
}

fn report_diagnostics(diags: &[orv_diagnostics::Diagnostic], path: &Path) -> anyhow::Result<()> {
    if diags.is_empty() {
        return Ok(());
    }
    let file_name = path.display().to_string();
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let files = codespan_reporting::files::SimpleFile::new(&file_name, &source);

    for d in diags {
        let mut labels = Vec::new();
        if let Some(lbl) = &d.primary {
            let start = lbl.span.range.start as usize;
            let end = lbl.span.range.end as usize;
            labels.push(
                codespan_reporting::diagnostic::Label::primary((), start..end)
                    .with_message(&lbl.message),
            );
        }
        for sec in &d.secondary {
            let start = sec.span.range.start as usize;
            let end = sec.span.range.end as usize;
            labels.push(
                codespan_reporting::diagnostic::Label::secondary((), start..end)
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
        for note in &d.notes {
            diag = diag.with_notes(vec![note.clone()]);
        }
        let config = codespan_reporting::term::Config::default();
        let mut writer = codespan_reporting::term::termcolor::StandardStream::stderr(
            codespan_reporting::term::termcolor::ColorChoice::Auto,
        );
        codespan_reporting::term::emit(&mut writer, &config, &files, &diag).ok();
    }
    if diags
        .iter()
        .any(|d| matches!(d.severity, orv_diagnostics::Severity::Error))
    {
        anyhow::bail!("aborting due to previous errors");
    }
    Ok(())
}
