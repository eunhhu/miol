//! orv CLI 프론트엔드 — `orv` 바이너리.
//!
//! MVP: `orv run <file>`로 `.orv` 파일을 tree-walking 인터프리터로 실행한다.
//! 이후 `orv build`, `orv check`, `orv dev` 등이 추가된다.

use std::path::PathBuf;
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
        Command::Dump { file } => match cmd_dump(&file) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn cmd_run(path: &PathBuf) -> anyhow::Result<()> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let file_id = FileId(0);

    let lx = orv_syntax::lex(&source, file_id);
    report_diagnostics(&lx.diagnostics, path)?;

    let pr = orv_syntax::parse(lx.tokens, file_id);
    report_diagnostics(&pr.diagnostics, path)?;

    orv_runtime::run(&pr.program)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn cmd_dump(path: &PathBuf) -> anyhow::Result<()> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let file_id = FileId(0);
    let lx = orv_syntax::lex(&source, file_id);
    report_diagnostics(&lx.diagnostics, path)?;
    let pr = orv_syntax::parse(lx.tokens, file_id);
    report_diagnostics(&pr.diagnostics, path)?;
    println!("{:#?}", pr.program);
    Ok(())
}

fn report_diagnostics(
    diags: &[orv_diagnostics::Diagnostic],
    path: &PathBuf,
) -> anyhow::Result<()> {
    if diags.is_empty() {
        return Ok(());
    }
    for d in diags {
        let kind = match d.severity {
            orv_diagnostics::Severity::Error => "error",
            orv_diagnostics::Severity::Warning => "warning",
            orv_diagnostics::Severity::Note => "note",
            orv_diagnostics::Severity::Help => "help",
        };
        eprintln!("{kind}: {}", d.message);
        if let Some(lbl) = &d.primary {
            eprintln!(
                "  --> {}:{}..{}",
                path.display(),
                lbl.span.range.start,
                lbl.span.range.end
            );
        }
    }
    if diags
        .iter()
        .any(|d| matches!(d.severity, orv_diagnostics::Severity::Error))
    {
        anyhow::bail!("aborting due to previous errors");
    }
    Ok(())
}
