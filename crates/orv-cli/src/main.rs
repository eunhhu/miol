use std::path::PathBuf;
use std::process;

use clap::Parser;
use orv_analyzer::dump_hir;
use orv_compiler::{FrontendFailure, load_path};
use orv_diagnostics::render_diagnostics;
use orv_runtime::{Request, compile_program, emit_build, execute_request, render_response};

#[derive(Parser)]
#[command(name = "orv", version, about = "Integrated Platform Development DSL")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Display version information
    Version,
    /// Check a source file for errors
    Check {
        /// Path to the .orv source file
        file: PathBuf,
    },
    /// Execute a request through the reference runtime
    Run {
        /// Path to the .orv source file
        file: PathBuf,
        /// HTTP method to execute
        #[arg(long, default_value = "GET")]
        method: String,
        /// Request path to execute
        #[arg(long, default_value = "/")]
        path: String,
    },
    /// Build a direct adapter binary and manifest
    Build {
        /// Path to the .orv source file
        file: PathBuf,
        /// Output directory for build artifacts
        #[arg(long, default_value = "dist")]
        output_dir: PathBuf,
    },
    /// Dump internal representations
    Dump {
        #[command(subcommand)]
        target: DumpTarget,
    },
}

#[derive(clap::Subcommand)]
enum DumpTarget {
    /// Dump source file metadata (file id, line count, spans)
    Source {
        /// Path to the .orv source file
        file: PathBuf,
    },
    /// Dump token stream for a source file
    Tokens {
        /// Path to the .orv source file
        file: PathBuf,
    },
    /// Dump AST for a source file
    Ast {
        /// Path to the .orv source file
        file: PathBuf,
    },
    /// Dump lowered HIR for a source file
    Hir {
        /// Path to the .orv source file
        file: PathBuf,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Version) | None => {
            println!("orv {}", orv_core::version());
        }
        Some(Commands::Check { file }) => {
            run_check(&file);
        }
        Some(Commands::Run { file, method, path }) => {
            run_runtime(&file, &method, &path);
        }
        Some(Commands::Build { file, output_dir }) => {
            run_build(&file, &output_dir);
        }
        Some(Commands::Dump { target }) => match target {
            DumpTarget::Source { file } => {
                run_dump_source(&file);
            }
            DumpTarget::Tokens { file } => {
                run_dump_tokens(&file);
            }
            DumpTarget::Ast { file } => {
                run_dump_ast(&file);
            }
            DumpTarget::Hir { file } => {
                run_dump_hir(&file);
            }
        },
    }
}

fn run_check(path: &PathBuf) {
    let analysis = analyze_or_exit(path);
    let name = analysis.source_map().name(analysis.file_id());
    println!(
        "check: {name} \u{2014} {} items, {} symbols, {} scopes, ok",
        analysis.module().items.len(),
        analysis.analysis().symbols.len(),
        analysis.analysis().scopes.len()
    );
}

fn run_runtime(path: &PathBuf, method: &str, request_path: &str) {
    let analysis = analyze_or_exit(path);
    let program = match compile_program(&analysis.analysis().hir) {
        Ok(program) => program,
        Err(error) => {
            eprintln!("runtime compile error: {error}");
            process::exit(1);
        }
    };
    let response = match execute_request(
        &program,
        &Request {
            method,
            path: request_path,
        },
    ) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("runtime execution error: {error}");
            process::exit(1);
        }
    };
    print!("{}", render_response(&response));
}

fn run_build(path: &PathBuf, output_dir: &PathBuf) {
    let analysis = analyze_or_exit(path);
    let program = match compile_program(&analysis.analysis().hir) {
        Ok(program) => program,
        Err(error) => {
            eprintln!("runtime compile error: {error}");
            process::exit(1);
        }
    };
    let artifacts = match emit_build(&program, output_dir) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            eprintln!("build error: {error}");
            process::exit(1);
        }
    };
    println!("build: {}", path.display());
    println!("adapter: direct-match");
    println!("manifest: {}", artifacts.manifest_path.display());
    println!("source: {}", artifacts.adapter_source_path.display());
    println!("binary: {}", artifacts.binary_path.display());
}

fn run_dump_tokens(path: &PathBuf) {
    let loaded = load_or_exit(path);
    let source_map = loaded.source_map();
    let (tokens, diags) = loaded.tokenize();

    if diags.has_errors() {
        render_diagnostics(source_map, &diags.into_vec());
        process::exit(1);
    }

    for token in &tokens {
        let span = token.span();
        let (_, line, col) = source_map.resolve(span);
        println!("{:>4}:{:<3} {:?}", line + 1, col, token.node());
    }
}

fn run_dump_ast(path: &PathBuf) {
    let parsed = match load_or_exit(path).parse() {
        Ok(parsed) => parsed,
        Err(failure) => render_failure_and_exit(failure),
    };
    println!("{}", parsed.dump_ast());
}

fn run_dump_source(path: &PathBuf) {
    let loaded = load_or_exit(path);
    let source_map = loaded.source_map();
    let name = source_map.name(loaded.file_id());
    let source = source_map.source(loaded.file_id());
    let line_count = source_map.line_index(loaded.file_id()).line_count();
    let byte_count = source.len();
    println!("file: {name}");
    println!("file_id: {}", loaded.file_id().raw());
    println!("bytes: {byte_count}");
    println!("lines: {line_count}");
}

fn run_dump_hir(path: &PathBuf) {
    let analysis = analyze_or_exit(path);
    println!("{}", dump_hir(&analysis.analysis().hir));
}

fn load_or_exit(path: &PathBuf) -> orv_compiler::LoadedUnit {
    match load_path(path) {
        Ok(loaded) => loaded,
        Err(failure) => render_failure_and_exit(failure),
    }
}

fn analyze_or_exit(path: &PathBuf) -> orv_compiler::AnalyzedUnit {
    let parsed = match load_or_exit(path).parse() {
        Ok(parsed) => parsed,
        Err(failure) => render_failure_and_exit(failure),
    };
    match parsed.analyze() {
        Ok(analysis) => analysis,
        Err(failure) => render_failure_and_exit(failure),
    }
}

fn render_failure_and_exit(failure: FrontendFailure) -> ! {
    let (source_map, diagnostics) = failure.into_parts();
    render_diagnostics(&source_map, &diagnostics.into_vec());
    process::exit(1);
}
