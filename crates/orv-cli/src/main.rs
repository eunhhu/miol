use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process;

use clap::{Parser, ValueEnum};
use orv_analyzer::dump_hir;
use orv_compiler::{FrontendFailure, dump_workspace_graph, load_path, load_project_graph};
use orv_diagnostics::render_diagnostics;
use orv_runtime::{
    AdapterKind, Request, RouteAction, compile_program, emit_build, execute_request,
    render_response,
};

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
        /// Build output kind
        #[arg(long, value_enum, default_value_t = BuildEmit::NativeAdapter)]
        emit: BuildEmit,
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
    /// Dump project graph for a source file
    ProjectGraph {
        /// Path to the .orv source file
        file: PathBuf,
    },
    /// Dump a stage-by-stage compile pipeline view
    Pipeline {
        /// Path to the .orv source file
        file: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum BuildEmit {
    NativeAdapter,
    ProjectGraph,
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
        Some(Commands::Build {
            file,
            output_dir,
            emit,
        }) => {
            run_build(&file, &output_dir, emit);
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
            DumpTarget::ProjectGraph { file } => {
                run_dump_project_graph(&file);
            }
            DumpTarget::Pipeline { file } => {
                run_dump_pipeline(&file);
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

fn run_build(path: &PathBuf, output_dir: &PathBuf, emit: BuildEmit) {
    let analysis = analyze_or_exit(path);
    let workspace_graph = match load_project_graph(path) {
        Ok(graph) => graph,
        Err(failure) => render_failure_and_exit(failure),
    };
    if emit == BuildEmit::ProjectGraph {
        if let Err(error) = fs::create_dir_all(output_dir) {
            eprintln!("build error: {error}");
            process::exit(1);
        }
        let output_path = output_dir.join("project-graph.json");
        if let Err(error) = write_project_graph(&workspace_graph, &output_path) {
            eprintln!("build error: {error}");
            process::exit(1);
        }
        println!("build: {}", path.display());
        println!("emit: project-graph");
        println!("output: {}", output_path.display());
        return;
    }

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
    let graph_path = output_dir.join("project-graph.json");
    if let Err(error) = write_project_graph(&workspace_graph, &graph_path) {
        eprintln!("build error: {error}");
        process::exit(1);
    }
    println!("build: {}", path.display());
    println!("adapter: direct-match");
    println!("manifest: {}", artifacts.manifest_path.display());
    println!("source: {}", artifacts.adapter_source_path.display());
    println!("binary: {}", artifacts.binary_path.display());
    println!("graph: {}", graph_path.display());
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

fn run_dump_project_graph(path: &PathBuf) {
    let graph = match load_project_graph(path) {
        Ok(graph) => graph,
        Err(failure) => render_failure_and_exit(failure),
    };
    println!("{}", dump_workspace_graph(&graph));
}

fn run_dump_pipeline(path: &PathBuf) {
    let loaded = load_or_exit(path);
    let source_name = loaded.source_name().to_owned();
    let source_len = loaded.source().len();
    let line_count = loaded
        .source_map()
        .line_index(loaded.file_id())
        .line_count();
    let (tokens, diagnostics) = loaded.tokenize();
    if diagnostics.has_errors() {
        render_diagnostics(loaded.source_map(), &diagnostics.into_vec());
        process::exit(1);
    }

    let token_count = tokens.len();
    let parsed = match loaded.parse() {
        Ok(parsed) => parsed,
        Err(failure) => render_failure_and_exit(failure),
    };
    let item_count = parsed.module().items.len();
    let analysis = match parsed.analyze() {
        Ok(analysis) => analysis,
        Err(failure) => render_failure_and_exit(failure),
    };

    let mut out = String::new();
    out.push_str("Compile Pipeline\n");
    out.push_str("================\n");
    out.push_str("1. Load     OK\n");
    out.push_str(&format!("   file: {source_name}\n"));
    out.push_str(&format!("   bytes: {source_len}\n"));
    out.push_str(&format!("   lines: {line_count}\n"));
    out.push_str("2. Lex      OK\n");
    out.push_str(&format!("   tokens: {token_count}\n"));
    out.push_str("3. Parse    OK\n");
    out.push_str(&format!("   items: {item_count}\n"));
    out.push_str("4. Analyze  OK\n");
    out.push_str(&format!(
        "   symbols: {}\n",
        analysis.analysis().symbols.len()
    ));
    out.push_str(&format!(
        "   scopes: {}\n",
        analysis.analysis().scopes.len()
    ));
    let graph = match load_project_graph(path) {
        Ok(graph) => graph,
        Err(failure) => render_failure_and_exit(failure),
    };
    out.push_str("5. Graph    OK\n");
    out.push_str(&format!("   modules: {}\n", graph.modules.len()));
    out.push_str(&format!(
        "   pages: {}\n",
        graph
            .modules
            .iter()
            .map(|module| module.pages.len())
            .sum::<usize>()
    ));
    out.push_str(&format!(
        "   signals: {}\n",
        graph
            .modules
            .iter()
            .map(|module| module.signals.len())
            .sum::<usize>()
    ));
    out.push_str(&format!(
        "   routes: {}\n",
        graph
            .modules
            .iter()
            .map(|module| module.routes.len())
            .sum::<usize>()
    ));
    out.push_str(&format!(
        "   fetches: {}\n",
        graph
            .modules
            .iter()
            .map(|module| module.fetches.len())
            .sum::<usize>()
    ));

    match compile_program(&analysis.analysis().hir) {
        Ok(program) => {
            out.push_str("6. Runtime  OK\n");
            out.push_str(&format!(
                "   adapter: {}\n",
                match program.adapter {
                    AdapterKind::DirectMatch => "direct-match",
                }
            ));
            out.push_str(&format!("   listen: {}\n", program.server.listen));
            out.push_str(&format!("   routes: {}\n", program.server.routes.len()));
            for route in &program.server.routes {
                out.push_str(&format!(
                    "   - {} {} -> {}\n",
                    route.method,
                    route.path,
                    describe_route_action(&route.action)
                ));
            }
            out.push_str("7. Build    READY\n");
            out.push_str("   backend: direct native adapter via rustc -O\n");
            out.push_str(
                "   outputs: program.json, direct_adapter.rs, orv-app, project-graph.json\n",
            );
        }
        Err(error) => {
            out.push_str("6. Runtime  SKIPPED\n");
            out.push_str(&format!("   reason: {error}\n"));
            out.push_str("7. Build    SKIPPED\n");
            out.push_str("   reason: runtime program could not be lowered\n");
        }
    }

    print!("{out}");
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

fn describe_route_action(action: &RouteAction) -> &'static str {
    match action {
        RouteAction::JsonResponse { .. } => "@response json",
        RouteAction::StaticServe { .. } => "@serve static",
        RouteAction::HtmlServe { .. } => "@serve html",
    }
}

fn write_project_graph(
    graph: &orv_compiler::WorkspaceGraph,
    output_path: &Path,
) -> Result<(), std::io::Error> {
    let json = serde_json::to_vec_pretty(graph)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    fs::write(output_path, json)
}
