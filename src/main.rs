// SPDX-License-Identifier: MIT

mod analysis;
mod graph;
mod render;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command as ProcessCommand};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};

use analysis::{analyze_project, internal_dependencies, InternalDependency};
use graph::{top_level_graph, TopLevelGraph};
use render::render_top_level_html;

fn main() {
    let result = match Cli::parse().command {
        Command::Analyse { rust_file, output } => analyze_project(&rust_file)
            .map(|analysis| internal_dependencies(&analysis.usages, &analysis.module_paths))
            .and_then(|dependencies| {
                write_internal_dependencies_json(&dependencies, output.as_deref())
            }),
        Command::Visualise { json_file, output } => read_internal_dependencies_json(&json_file)
            .map(|dependencies| top_level_graph(&dependencies))
            .and_then(|graph| write_or_open_top_level_html(&graph, output.as_deref())),
    };

    if let Err(error) = result {
        eprintln!("{error}");
        process::exit(1);
    }
}

#[derive(Parser)]
#[command(
    author,
    version,
    about = "Analyse and visualise Rust module dependencies"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyse a Rust file and output internal module dependency JSON.
    Analyse {
        /// Rust entry file to analyse.
        rust_file: PathBuf,

        /// Write JSON output to this file instead of stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Visualise internal module dependency JSON as an HTML page.
    Visualise {
        /// Internal dependency JSON file produced by `analyse`.
        json_file: PathBuf,

        /// Write HTML output to this file instead of opening a temporary HTML page.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn write_or_open_top_level_html(
    graph: &TopLevelGraph,
    output: Option<&Path>,
) -> Result<(), String> {
    let html = render_top_level_html(&graph);
    if let Some(output) = output {
        return write_text_output(Some(output), &html);
    }

    let output = temporary_html_path();
    write_text_output(Some(&output), &html)?;
    open_html_file(&output)
}

fn temporary_html_path() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("dep-analysis-{}-{timestamp}.html", process::id()))
}

fn open_html_file(path: &Path) -> Result<(), String> {
    let status = ProcessCommand::new("open")
        .arg(path)
        .status()
        .map_err(|error| format!("failed to run open {}: {error}", path.display()))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("open {} exited with {status}", path.display()))
    }
}

fn write_internal_dependencies_json(
    dependencies: &[InternalDependency],
    output: Option<&Path>,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(dependencies)
        .map_err(|error| format!("failed to serialize internal dependency json: {error}"))?;
    write_text_output(output, &format!("{json}\n"))
}

fn write_text_output(output: Option<&Path>, contents: &str) -> Result<(), String> {
    if let Some(output) = output {
        fs::write(output, contents)
            .map_err(|error| format!("failed to write {}: {error}", output.display()))
    } else {
        let stdout = io::stdout();
        let mut stdout = stdout.lock();
        stdout
            .write_all(contents.as_bytes())
            .and_then(|_| stdout.flush())
            .map_err(|error| format!("failed to write stdout: {error}"))
    }
}

fn read_internal_dependencies_json(path: &Path) -> Result<Vec<InternalDependency>, String> {
    let json = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&json).map_err(|error| {
        format!(
            "failed to parse {} as internal dependency json: {error}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::analysis::{
        analyze_project, dependency_usages, dependency_usages_for_file, internal_dependencies,
        FileLocalUsage, InternalDependency, Usage,
    };
    use crate::graph::{top_level_graph, TopLevelEdge};
    use crate::render::render_top_level_html;

    fn usages(source: &str) -> Vec<FileLocalUsage> {
        dependency_usages(source).unwrap()
    }

    #[test]
    fn collects_use_tree_symbols() {
        let source = r#"
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot as tokio_oneshot};
use crate::local::Thing;
use self::module::Local;
use super::other::LocalToo;
use uuid::*;
"#;

        assert_eq!(
            usages(source),
            [
                usage(3, "serde", "Deserialize"),
                usage(3, "serde", "Serialize"),
                usage(4, "tokio::sync", "mpsc"),
                usage(4, "tokio::sync", "oneshot"),
                usage(5, "crate::local", "Thing"),
                usage(6, "self::module", "Local"),
                usage(7, "super::other", "LocalToo"),
                usage(8, "uuid", "*"),
            ]
        );
    }

    #[test]
    fn collects_qualified_path_symbols() {
        let source = r#"
extern crate anyhow;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let value = serde_json::from_str::<Vec<String>>("{}")?;
    tracing::info!(?value);
    let _ = <String as anyhow::Context<()>>::context(String::new(), "missing");
    std::mem::drop(value);
    Ok(())
}
"#;

        assert_eq!(
            usages(source),
            [
                usage(2, "extern crate", "anyhow"),
                usage(4, "tokio", "main"),
                usage(5, "anyhow", "Result"),
                usage(6, "serde_json", "from_str"),
                usage(7, "tracing", "info!"),
                usage(8, "anyhow", "Context"),
                usage(8, "anyhow::Context", "context"),
            ]
        );
    }

    #[test]
    fn reports_the_first_line_for_duplicate_usages() {
        let source = r#"
fn first() {
    tracing::info!("first");
}

fn second() {
    tracing::info!("second");
}
"#;

        assert_eq!(usages(source), [usage(3, "tracing", "info!")]);
    }

    #[test]
    fn ignores_comments_strings_and_prelude_roots() {
        let source = r##"
// fake::dependency
/* also_fake::dependency */
const TEXT: &str = "quoted::dependency";
const RAW: &str = r#"raw::dependency"#;
fn main() {
    let value = real_dep::call();
    std::mem::drop(value);
    Ok::<(), anyhow::Error>(())
}
"##;

        assert_eq!(
            usages(source),
            [usage(7, "real_dep", "call"), usage(9, "anyhow", "Error")]
        );
    }

    #[test]
    fn recurses_into_referenced_module_files() {
        let dir = unique_test_dir();
        fs::create_dir_all(dir.join("child")).unwrap();
        fs::write(
            dir.join("lib.rs"),
            r#"
use root_dep::Root;
mod child;
"#,
        )
        .unwrap();
        fs::write(
            dir.join("child.rs"),
            r#"
use child_dep::Child;
mod grand;
"#,
        )
        .unwrap();
        fs::write(
            dir.join("child").join("grand.rs"),
            r#"
use grand_dep::Grand;
"#,
        )
        .unwrap();

        let root = dir.join("lib.rs").canonicalize().unwrap();
        let child = dir.join("child.rs").canonicalize().unwrap();
        let grand = dir.join("child").join("grand.rs").canonicalize().unwrap();

        assert_eq!(
            dependency_usages_for_file(&root).unwrap(),
            [
                file_usage(&root, ["crate"], 2, "root_dep", "Root"),
                file_usage(&child, ["crate", "child"], 2, "child_dep", "Child"),
                file_usage(&grand, ["crate", "child", "grand"], 2, "grand_dep", "Grand"),
            ]
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recurses_into_mods_declared_inside_inline_modules() {
        let dir = unique_test_dir();
        fs::create_dir_all(dir.join("outer")).unwrap();
        fs::write(
            dir.join("lib.rs"),
            r#"
mod outer {
    mod inner;
}
"#,
        )
        .unwrap();
        fs::write(
            dir.join("outer").join("inner.rs"),
            r#"
use inner_dep::Inner;
"#,
        )
        .unwrap();

        let root = dir.join("lib.rs").canonicalize().unwrap();
        let inner = dir.join("outer").join("inner.rs").canonicalize().unwrap();

        assert_eq!(
            dependency_usages_for_file(&root).unwrap(),
            [file_usage(
                &inner,
                ["crate", "outer", "inner"],
                2,
                "inner_dep",
                "Inner"
            )]
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resolves_raw_identifier_module_filenames() {
        let dir = unique_test_dir();
        fs::write(
            dir.join("lib.rs"),
            r#"
mod r#type;
"#,
        )
        .unwrap();
        fs::write(
            dir.join("type.rs"),
            r#"
use type_dep::TypeDep;
"#,
        )
        .unwrap();

        let root = dir.join("lib.rs").canonicalize().unwrap();
        let type_file = dir.join("type.rs").canonicalize().unwrap();

        assert_eq!(
            dependency_usages_for_file(&root).unwrap(),
            [file_usage(
                &type_file,
                ["crate", "type"],
                2,
                "type_dep",
                "TypeDep"
            )]
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn internal_dependencies_resolve_known_modules() {
        let dir = unique_test_dir();
        fs::create_dir_all(dir.join("a")).unwrap();
        fs::write(
            dir.join("lib.rs"),
            r#"
mod a;
mod b;
"#,
        )
        .unwrap();
        fs::write(
            dir.join("a.rs"),
            r#"
use crate::b::Thing;
use self::nested::NestedThing;
mod nested;
"#,
        )
        .unwrap();
        fs::write(dir.join("b.rs"), "").unwrap();
        fs::write(dir.join("a").join("nested.rs"), "").unwrap();

        let root = dir.join("lib.rs").canonicalize().unwrap();
        let a = dir.join("a.rs").canonicalize().unwrap();
        let analysis = analyze_project(&root).unwrap();

        assert_eq!(
            internal_dependencies(&analysis.usages, &analysis.module_paths),
            [
                internal_dep(&a, 2, "crate::a", "crate::b"),
                internal_dep(&a, 3, "crate::a", "crate::a::nested"),
            ]
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn internal_dependencies_use_inline_module_names() {
        let dir = unique_test_dir();
        fs::create_dir_all(dir.join("outer")).unwrap();
        fs::write(
            dir.join("lib.rs"),
            r#"
mod b;
mod outer {
    mod inner;

    fn f() {
        self::inner::Thing::new();
        crate::b::Thing::new();
    }
}
"#,
        )
        .unwrap();
        fs::write(dir.join("b.rs"), "").unwrap();
        fs::write(dir.join("outer").join("inner.rs"), "").unwrap();

        let root = dir.join("lib.rs").canonicalize().unwrap();
        let analysis = analyze_project(&root).unwrap();

        assert_eq!(
            internal_dependencies(&analysis.usages, &analysis.module_paths),
            [
                internal_dep(&root, 7, "crate::outer", "crate::outer::inner"),
                internal_dep(&root, 8, "crate::outer", "crate::b"),
            ]
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn top_level_graph_collapses_submodule_edges() {
        let file = PathBuf::from("/repo/src/a.rs");
        let graph = top_level_graph(&[
            internal_dep(&file, 1, "crate::a", "crate::b"),
            internal_dep(&file, 2, "crate::a::nested", "crate::b::deep"),
            internal_dep(&file, 3, "crate::a", "crate::a::nested"),
            internal_dep(&file, 4, "crate", "crate::b"),
        ]);

        assert_eq!(graph.modules, ["a", "b"]);
        assert_eq!(
            graph.edges,
            [TopLevelEdge {
                from: "a".to_owned(),
                to: "b".to_owned(),
                count: 2
            }]
        );

        let html = render_top_level_html(&graph);
        assert!(html.contains("<svg"));
        assert!(html.contains(">a<"));
        assert!(html.contains(">b<"));
        assert!(html.contains(r#""id":"a","incoming":0,"outgoing":2"#));
        assert!(html.contains(r#""id":"b","incoming":2,"outgoing":0"#));
        assert!(!html.contains("crate::a"));
        assert!(html.contains(r#""marker-end": "url(#arrow)""#));
        assert!(html.contains("markerWidth=\"10\""));
        assert!(html.contains("M 2 3.5 L 17 7 L 2 10.5 Z"));
        assert!(html.contains("installDrag(group, node);"));
        assert!(html.contains("requestAnimationFrame(tick);"));
        assert!(html.contains("node.fixed = true;"));
        assert!(html.contains("node.dragging = false;"));
        assert!(html.contains("function toggleSelection(node)"));
        assert!(html.contains("function spreadFocus(node)"));
        assert!(html.contains("function spreadAllVisible()"));
        assert!(html.contains("function releasePreviousPinnedNode(nextNode)"));
        assert!(html.contains("function pinNode(node)"));
        assert!(html.contains("let activeDragNode = null;"));
        assert!(html.contains("let localRelaxNode = null;"));
        assert!(html.contains("function taperLocalRelax()"));
        assert!(html.contains("function applyLocalDragForces(draggedNode)"));
        assert!(html.contains("function applyLocalCollisionForces()"));
        assert!(html.contains("function startLocalRelax(node)"));
        assert!(html.contains("localRelaxUntil = performance.now() + 1000;"));
        assert!(html.contains("function easeInOutCubic(progress)"));
        assert!(html.contains("if (localForceNode && !selectedNode)"));
        assert!(html.contains("function edgeLabelPoint(edge)"));
        assert!(html.contains("const offset = hasReverse ? 18 : 0;"));
        assert!(html.contains("function focusedNeighbors(node)"));
        assert!(html.contains("function placeFocusGroup(items"));
        assert!(html.contains("function animateFocusLayout(focusNodes, keepFixed)"));
        assert!(html.contains("function easeOutCubic(progress)"));
        assert!(html.contains("function relaxFocusTargets(focusNodes, anchor)"));
        assert!(html.contains("function nudgeFocusTarget(node, dx, dy)"));
        assert!(html.contains("function settleSimulation()"));
        assert!(html.contains("if (!clearingSelection) reheat();"));
        assert!(html.contains("if (!keepFixed) settleSimulation();"));
        assert!(html.contains("setFocusTarget(node, centerX, centerY);"));
        assert!(html.contains("requestAnimationFrame(frame);"));
        assert!(html.contains("group.classList.toggle(\"selected\""));
        assert!(html.contains("group.classList.toggle(\"hidden\""));
        assert!(html.contains("Outgoing from selected"));
        assert!(html.contains("arrow-outgoing"));
        assert!(html.contains("arrow-incoming"));
        assert!(html.contains("arrow-bidirectional"));
        assert!(html.contains("stroke-width: 8px;"));
        assert!(html.contains("function selectedEdgeKind(edge)"));
        assert!(html.contains("function selectedEdgeKindFor(edge, node)"));
        assert!(html.contains("return \"bidirectional\";"));
    }

    fn usage(line: usize, origin: &str, symbol: &str) -> FileLocalUsage {
        FileLocalUsage {
            module_path_suffix: Vec::new(),
            line,
            origin: origin.to_owned(),
            symbol: symbol.to_owned(),
        }
    }

    fn file_usage<const N: usize>(
        file: &PathBuf,
        module_path: [&str; N],
        line: usize,
        origin: &str,
        symbol: &str,
    ) -> Usage {
        Usage {
            file: file.to_owned(),
            module_path: module_path.into_iter().map(ToOwned::to_owned).collect(),
            line,
            origin: origin.to_owned(),
            symbol: symbol.to_owned(),
        }
    }

    fn internal_dep(
        file: &PathBuf,
        line: usize,
        from_module: &str,
        to_module: &str,
    ) -> InternalDependency {
        InternalDependency {
            file: file.to_owned(),
            line,
            from_module: from_module.to_owned(),
            to_module: to_module.to_owned(),
        }
    }

    fn unique_test_dir() -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "dep-analysis-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
