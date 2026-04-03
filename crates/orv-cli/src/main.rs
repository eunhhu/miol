use std::path::PathBuf;
use std::process;

use clap::Parser;
use orv_core::source::SourceLoader;
use orv_diagnostics::render_diagnostics;

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
        Some(Commands::Dump { target }) => match target {
            DumpTarget::Source { file } => {
                run_dump_source(&file);
            }
            DumpTarget::Tokens { file } => {
                run_dump_tokens(&file);
            }
        },
    }
}

fn load_source(path: &PathBuf) -> (SourceLoader, Option<orv_span::FileId>) {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
    let root = absolute.parent().unwrap_or_else(|| absolute.as_ref());
    let display_name = path.display().to_string();

    let mut loader = SourceLoader::new(root);
    let file_id = loader.load_absolute(&absolute, &display_name);
    (loader, file_id)
}

fn run_check(path: &PathBuf) {
    let (loader, file_id) = load_source(path);

    if let Some(id) = file_id {
        let source_map = loader.source_map();
        let name = source_map.name(id);
        let source = source_map.source(id);
        let line_count = source_map.line_index(id).line_count();
        let byte_count = source.len();
        println!("check: {name} \u{2014} {line_count} lines, {byte_count} bytes, ok");
    } else {
        let (source_map, diagnostics) = loader.into_parts();
        render_diagnostics(&source_map, &diagnostics.into_vec());
        process::exit(1);
    }
}

fn run_dump_tokens(path: &PathBuf) {
    let (loader, file_id) = load_source(path);

    if let Some(id) = file_id {
        let source_map = loader.source_map();
        let source = source_map.source(id);
        let lexer = orv_syntax::lexer::Lexer::new(source, id);
        let (tokens, diags) = lexer.tokenize();

        if diags.has_errors() {
            let diag_vec: Vec<_> = diags.into_vec();
            render_diagnostics(source_map, &diag_vec);
        }

        for token in &tokens {
            let span = token.span();
            let (_, line, col) = source_map.resolve(span);
            println!("{:>4}:{:<3} {:?}", line + 1, col, token.node());
        }
    } else {
        let (source_map, diagnostics) = loader.into_parts();
        render_diagnostics(&source_map, &diagnostics.into_vec());
        process::exit(1);
    }
}

fn run_dump_source(path: &PathBuf) {
    let (loader, file_id) = load_source(path);

    if let Some(id) = file_id {
        let source_map = loader.source_map();
        let name = source_map.name(id);
        let source = source_map.source(id);
        let line_count = source_map.line_index(id).line_count();
        let byte_count = source.len();
        println!("file: {name}");
        println!("file_id: {}", id.raw());
        println!("bytes: {byte_count}");
        println!("lines: {line_count}");
    } else {
        let (source_map, diagnostics) = loader.into_parts();
        render_diagnostics(&source_map, &diagnostics.into_vec());
        process::exit(1);
    }
}
