use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command as ProcessCommand};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use proc_macro2::{Ident, Span};
use serde::{Deserialize, Serialize};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, ExprPath, File, ItemExternCrate, ItemMod, ItemUse, LitStr, Macro, Path as SynPath,
    QSelf, TypePath, UseTree,
};

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

fn top_level_graph(dependencies: &[InternalDependency]) -> TopLevelGraph {
    let mut modules = BTreeSet::new();
    let mut edges = BTreeMap::<(String, String), usize>::new();

    for dependency in dependencies {
        let Some(from) = top_level_module(&dependency.from_module) else {
            continue;
        };
        let Some(to) = top_level_module(&dependency.to_module) else {
            continue;
        };
        if from == to {
            continue;
        }

        modules.insert(from.clone());
        modules.insert(to.clone());
        *edges.entry((from, to)).or_default() += 1;
    }

    TopLevelGraph {
        modules: modules.into_iter().collect(),
        edges: edges
            .into_iter()
            .map(|((from, to), count)| TopLevelEdge { from, to, count })
            .collect(),
    }
}

fn top_level_module(module: &str) -> Option<String> {
    let mut parts = module.split("::");
    if parts.next()? != "crate" {
        return None;
    }
    Some(parts.next()?.to_owned())
}

#[derive(Debug, Deserialize, Serialize)]
struct TopLevelGraph {
    modules: Vec<String>,
    edges: Vec<TopLevelEdge>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct TopLevelEdge {
    from: String,
    to: String,
    count: usize,
}

fn render_top_level_html(graph: &TopLevelGraph) -> String {
    let width = 1400.0;
    let height = 900.0;
    let center_x = width / 2.0;
    let center_y = 390.0;
    let radius_x = 520.0;
    let radius_y = 285.0;
    let box_width = 150.0;
    let box_height = 64.0;
    let module_counts = module_edge_counts(graph);
    let nodes = graph
        .modules
        .iter()
        .enumerate()
        .map(|(index, module)| {
            let count = graph.modules.len().max(1) as f64;
            let angle =
                -std::f64::consts::FRAC_PI_2 + (index as f64 * std::f64::consts::TAU / count);
            let counts = module_counts.get(module).copied().unwrap_or_default();
            NodeLayout {
                id: module.clone(),
                incoming: counts.incoming,
                outgoing: counts.outgoing,
                x: center_x + radius_x * angle.cos(),
                y: center_y + radius_y * angle.sin(),
            }
        })
        .collect::<Vec<_>>();
    let graph_data = GraphData {
        nodes,
        edges: graph.edges.clone(),
        box_width,
        box_height,
        width,
        height,
    };
    let graph_json = serde_json::to_string(&graph_data).unwrap_or_else(|_| "{}".to_owned());

    let mut rows = String::new();
    for edge in &graph.edges {
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            escape_html(&edge.from),
            escape_html(&edge.to),
            edge.count
        ));
    }

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Top-level module dependencies</title>
<style>
  :root {{
    color: #18202a;
    background: #f4f7fb;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  }}
  body {{
    margin: 0;
    padding: 32px;
  }}
  h1 {{
    margin: 0 0 8px;
    font-size: 28px;
  }}
  p {{
    margin: 0 0 24px;
    color: #4a5565;
  }}
  .panel {{
    background: #ffffff;
    border: 1px solid #d8e0ea;
    border-radius: 8px;
    box-shadow: 0 6px 20px rgba(20, 32, 45, 0.08);
    overflow: auto;
  }}
  svg {{
    display: block;
    width: 100%;
    min-width: 1100px;
    height: auto;
  }}
  .edge line {{
    stroke: #1f5f88;
    opacity: 0.78;
    stroke-linecap: round;
  }}
  .edge.outgoing line {{
    stroke: #167c3f;
    opacity: 0.95;
    stroke-width: 8px;
  }}
  .edge.incoming line {{
    stroke: #c15c16;
    opacity: 0.95;
    stroke-width: 8px;
  }}
  .edge.bidirectional line {{
    stroke: #7b3fb3;
    opacity: 0.95;
    stroke-width: 8px;
  }}
  .edge.hidden,
  .node.hidden {{
    display: none;
  }}
  .edge text {{
    fill: #273444;
    font-size: 12px;
    font-weight: 700;
    paint-order: stroke;
    stroke: #ffffff;
    stroke-width: 4px;
    text-anchor: middle;
  }}
  .edge.outgoing text {{
    fill: #125f32;
  }}
  .edge.incoming text {{
    fill: #94430f;
  }}
  .edge.bidirectional text {{
    fill: #61318d;
  }}
  .node rect {{
    fill: #ffffff;
    stroke: #2f6f9f;
    stroke-width: 2px;
    rx: 8px;
  }}
  .node {{
    cursor: grab;
    touch-action: none;
  }}
  .node.dragging {{
    cursor: grabbing;
  }}
  .node.selected rect {{
    fill: #dff1fb;
    stroke: #bf5b12;
    stroke-width: 3px;
  }}
  .node .module-name {{
    fill: #17202c;
    font-size: 13px;
    font-weight: 700;
    text-anchor: middle;
    dominant-baseline: middle;
  }}
  .node .module-counts {{
    fill: #526174;
    font-size: 12px;
    font-weight: 600;
    text-anchor: middle;
    dominant-baseline: middle;
  }}
  .legend {{
    align-items: center;
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    margin: -8px 0 18px;
  }}
  .legend-item {{
    align-items: center;
    color: #435164;
    display: inline-flex;
    font-size: 13px;
    font-weight: 600;
    gap: 6px;
  }}
  .legend-swatch {{
    border-radius: 999px;
    display: inline-block;
    height: 5px;
    width: 30px;
  }}
  .legend-outgoing {{
    background: #167c3f;
  }}
  .legend-incoming {{
    background: #c15c16;
  }}
  .legend-bidirectional {{
    background: #7b3fb3;
  }}
  table {{
    width: 100%;
    border-collapse: collapse;
    margin-top: 24px;
    background: #ffffff;
    border: 1px solid #d8e0ea;
    border-radius: 8px;
    overflow: hidden;
  }}
  th, td {{
    padding: 10px 12px;
    border-bottom: 1px solid #e7edf4;
    text-align: left;
    font-size: 14px;
  }}
  th {{
    background: #e8f1f8;
  }}
</style>
</head>
<body>
<h1>Top-level module dependencies</h1>
<p>{} modules, {} dependency edges. Drag boxes to reshape the graph; click a box to focus on its incoming, outgoing, and bidirectional dependencies.</p>
<div class="legend" aria-label="Focused edge color legend">
  <span class="legend-item"><span class="legend-swatch legend-outgoing"></span>Outgoing from selected</span>
  <span class="legend-item"><span class="legend-swatch legend-incoming"></span>Incoming to selected</span>
  <span class="legend-item"><span class="legend-swatch legend-bidirectional"></span>Bidirectional</span>
</div>
<div class="panel">
<svg viewBox="0 0 {:.0} {:.0}" role="img" aria-label="Top-level module dependency graph">
<defs>
  <marker id="arrow" viewBox="0 0 18 14" refX="16" refY="7" markerWidth="10" markerHeight="10" orient="auto" markerUnits="strokeWidth">
    <path d="M 2 3.5 L 17 7 L 2 10.5 Z" fill="#1f5f88" stroke="#ffffff" stroke-width="1"></path>
  </marker>
  <marker id="arrow-outgoing" viewBox="0 0 18 14" refX="16" refY="7" markerWidth="10" markerHeight="10" orient="auto" markerUnits="strokeWidth">
    <path d="M 2 3.5 L 17 7 L 2 10.5 Z" fill="#167c3f" stroke="#ffffff" stroke-width="1"></path>
  </marker>
  <marker id="arrow-incoming" viewBox="0 0 18 14" refX="16" refY="7" markerWidth="10" markerHeight="10" orient="auto" markerUnits="strokeWidth">
    <path d="M 2 3.5 L 17 7 L 2 10.5 Z" fill="#c15c16" stroke="#ffffff" stroke-width="1"></path>
  </marker>
  <marker id="arrow-bidirectional" viewBox="0 0 18 14" refX="16" refY="7" markerWidth="10" markerHeight="10" orient="auto" markerUnits="strokeWidth">
    <path d="M 2 3.5 L 17 7 L 2 10.5 Z" fill="#7b3fb3" stroke="#ffffff" stroke-width="1"></path>
  </marker>
</defs>
<g id="edges"></g>
<g id="nodes"></g>
</svg>
</div>
<table>
<thead><tr><th>From</th><th>To</th><th>References</th></tr></thead>
<tbody>
{}</tbody>
</table>
<script type="application/json" id="graph-data">{}</script>
<script>
const graph = JSON.parse(document.getElementById("graph-data").textContent);
const svg = document.querySelector("svg");
const edgeLayer = document.getElementById("edges");
const nodeLayer = document.getElementById("nodes");
const nodes = graph.nodes.map(node => ({{ ...node, vx: 0, vy: 0, fixed: false, dragging: false }}));
const nodeById = new Map(nodes.map(node => [node.id, node]));
const edges = graph.edges
  .map(edge => ({{ ...edge, source: nodeById.get(edge.from), target: nodeById.get(edge.to) }}))
  .filter(edge => edge.source && edge.target);
const edgeKeys = new Set(edges.map(edge => `${{edge.source.id}}->${{edge.target.id}}`));
const maxEdgeCount = Math.max(1, ...edges.map(edge => edge.count));
let selectedNode = null;
let focusAnimation = 0;
let pinnedNode = null;
let activeDragNode = null;
let localRelaxNode = null;
let localRelaxStart = 0;
let localRelaxUntil = 0;
const edgeEls = edges.map(edge => {{
  const group = svgEl("g", {{ class: "edge" }});
  const line = svgEl("line", {{
    "stroke-width": (1 + 5 * edge.count / maxEdgeCount).toFixed(2),
    "marker-end": "url(#arrow)"
  }});
  const label = svgEl("text", {{}}, String(edge.count));
  group.append(line, label);
  edgeLayer.append(group);
  return {{ edge, group, line, label }};
}});
const nodeEls = nodes.map(node => {{
  const group = svgEl("g", {{ class: "node" }});
  const rect = svgEl("rect", {{
    x: -graph.box_width / 2,
    y: -graph.box_height / 2,
    width: graph.box_width,
    height: graph.box_height
  }});
  const name = svgEl("text", {{ class: "module-name", x: 0, y: -8 }}, node.id);
  const counts = svgEl("text", {{ class: "module-counts", x: 0, y: 14 }}, `in: ${{node.incoming}} · out: ${{node.outgoing}}`);
  group.append(rect, name, counts);
  nodeLayer.append(group);
  installDrag(group, node);
  return {{ node, group }};
}});

let alpha = 1;
let running = true;
requestAnimationFrame(tick);

function tick() {{
  taperLocalRelax();
  for (let step = 0; step < 2; step++) applyForces();
  render();
  alpha *= 0.985;
  if (localRelaxNode && performance.now() >= localRelaxUntil) {{
    localRelaxNode = null;
    settleSimulation();
  }}
  running = alpha > 0.01 || nodes.some(node => node.dragging) || localRelaxNode;
  if (running) requestAnimationFrame(tick);
}}

function taperLocalRelax() {{
  if (!localRelaxNode) return;
  const remaining = Math.max(0, localRelaxUntil - performance.now());
  const duration = Math.max(1, localRelaxUntil - localRelaxStart);
  const strength = remaining / duration;
  alpha = Math.min(alpha, 0.75 * easeInOutCubic(strength));
}}

function reheat() {{
  alpha = Math.max(alpha, 0.75);
  if (!running) {{
    running = true;
    requestAnimationFrame(tick);
  }}
}}

function applyForces() {{
  const localForceNode = activeDragNode || localRelaxNode;
  if (localForceNode && !selectedNode) {{
    applyLocalDragForces(localForceNode);
    return;
  }}

  const cx = graph.width / 2;
  const cy = graph.height / 2;
  for (const node of nodes) {{
    if (!node.fixed && !node.dragging) {{
      node.vx += (cx - node.x) * 0.0007 * alpha;
      node.vy += (cy - node.y) * 0.0007 * alpha;
    }}
  }}
  for (const edge of edges) {{
    const dx = edge.target.x - edge.source.x;
    const dy = edge.target.y - edge.source.y;
    const distance = Math.max(1, Math.hypot(dx, dy));
    const desired = 260;
    const force = (distance - desired) * 0.0009 * alpha;
    const fx = dx / distance * force;
    const fy = dy / distance * force;
    if (!edge.source.fixed && !edge.source.dragging) {{
      edge.source.vx += fx;
      edge.source.vy += fy;
    }}
    if (!edge.target.fixed && !edge.target.dragging) {{
      edge.target.vx -= fx;
      edge.target.vy -= fy;
    }}
  }}
  for (let i = 0; i < nodes.length; i++) {{
    for (let j = i + 1; j < nodes.length; j++) {{
      const a = nodes[i];
      const b = nodes[j];
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const distance = Math.max(1, Math.hypot(dx, dy));
      const minDistance = Math.max(graph.box_width, graph.box_height) + 36;
      if (distance < minDistance) {{
        const push = (minDistance - distance) * 0.018 * alpha;
        applyPairForce(a, b, dx / distance * push, dy / distance * push);
      }} else {{
        const repel = 2200 / (distance * distance) * alpha;
        applyPairForce(a, b, dx / distance * repel, dy / distance * repel);
      }}
    }}
  }}
  for (const node of nodes) {{
    if (node.fixed || node.dragging) continue;
    node.vx *= 0.86;
    node.vy *= 0.86;
    node.x = clamp(node.x + node.vx, graph.box_width / 2 + 20, graph.width - graph.box_width / 2 - 20);
    node.y = clamp(node.y + node.vy, graph.box_height / 2 + 20, graph.height - graph.box_height / 2 - 20);
  }}
}}

function applyLocalDragForces(draggedNode) {{
  for (const edge of edges) {{
    if (edge.source !== draggedNode && edge.target !== draggedNode) continue;
    const other = edge.source === draggedNode ? edge.target : edge.source;
    const dx = other.x - draggedNode.x;
    const dy = other.y - draggedNode.y;
    const distance = Math.max(1, Math.hypot(dx, dy));
    const desired = 260;
    const force = (distance - desired) * 0.00055 * alpha;
    if (!other.fixed && !other.dragging) {{
      other.vx -= dx / distance * force;
      other.vy -= dy / distance * force;
    }}
  }}

  applyLocalCollisionForces();

  for (const node of nodes) {{
    if (node.fixed || node.dragging) continue;
    node.vx *= 0.78;
    node.vy *= 0.78;
    node.x = clamp(node.x + node.vx, graph.box_width / 2 + 20, graph.width - graph.box_width / 2 - 20);
    node.y = clamp(node.y + node.vy, graph.box_height / 2 + 20, graph.height - graph.box_height / 2 - 20);
  }}
}}

function applyLocalCollisionForces() {{
  const minDistance = Math.max(graph.box_width, graph.box_height) + 44;
  for (let i = 0; i < nodes.length; i++) {{
    for (let j = i + 1; j < nodes.length; j++) {{
      const a = nodes[i];
      const b = nodes[j];
      let dx = b.x - a.x;
      let dy = b.y - a.y;
      let distance = Math.hypot(dx, dy);
      if (distance < 1) {{
        const angle = (i * 37 + j * 19) * Math.PI / 180;
        dx = Math.cos(angle);
        dy = Math.sin(angle);
        distance = 1;
      }}
      if (distance >= minDistance) continue;

      const push = (minDistance - distance) * 0.045 * alpha;
      applyPairForce(a, b, dx / distance * push, dy / distance * push);
    }}
  }}
}}

function applyPairForce(a, b, fx, fy) {{
  if (!a.fixed && !a.dragging) {{
    a.vx -= fx;
    a.vy -= fy;
  }}
  if (!b.fixed && !b.dragging) {{
    b.vx += fx;
    b.vy += fy;
  }}
}}

function render() {{
  for (const {{ edge, line, label }} of edgeEls) {{
    const points = edgePoints(edge.source, edge.target);
    const labelPoint = edgeLabelPoint(edge);
    setAttrs(line, {{ x1: points.x1, y1: points.y1, x2: points.x2, y2: points.y2 }});
    setAttrs(label, {{ x: labelPoint.x, y: labelPoint.y }});
  }}
  for (const {{ node, group }} of nodeEls) {{
    setAttrs(group, {{ transform: `translate(${{node.x}}, ${{node.y}})` }});
  }}
}}

function edgeLabelPoint(edge) {{
  const dx = edge.target.x - edge.source.x;
  const dy = edge.target.y - edge.source.y;
  const length = Math.max(1, Math.hypot(dx, dy));
  const hasReverse = edgeKeys.has(`${{edge.target.id}}->${{edge.source.id}}`);
  const offset = hasReverse ? 18 : 0;
  return {{
    x: (edge.source.x + edge.target.x) / 2 - dy / length * offset,
    y: (edge.source.y + edge.target.y) / 2 + dx / length * offset
  }};
}}

function edgePoints(from, to) {{
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = Math.max(1, Math.hypot(dx, dy));
  const ux = dx / length;
  const uy = dy / length;
  const fromOffset = boxEdgeOffset(ux, uy);
  const toOffset = boxEdgeOffset(-ux, -uy);
  return {{
    x1: from.x + ux * fromOffset,
    y1: from.y + uy * fromOffset,
    x2: to.x - ux * toOffset,
    y2: to.y - uy * toOffset
  }};
}}

function boxEdgeOffset(ux, uy) {{
  const horizontal = Math.abs(ux) > Number.EPSILON ? graph.box_width / 2 / Math.abs(ux) : Infinity;
  const vertical = Math.abs(uy) > Number.EPSILON ? graph.box_height / 2 / Math.abs(uy) : Infinity;
  return Math.min(horizontal, vertical);
}}

function installDrag(element, node) {{
  element.addEventListener("pointerdown", event => {{
    element.setPointerCapture(event.pointerId);
    localRelaxNode = null;
    releasePreviousPinnedNode(node);
    node.dragging = true;
    node.moved = false;
    node.fixed = true;
    activeDragNode = node;
    node.vx = 0;
    node.vy = 0;
    const pointer = pointerPosition(event);
    node.pointerOffsetX = node.x - pointer.x;
    node.pointerOffsetY = node.y - pointer.y;
    element.classList.add("dragging");
    reheat();
  }});
  element.addEventListener("pointermove", event => {{
    if (!node.dragging) return;
    moveNodeToPointer(event, node);
    reheat();
  }});
  element.addEventListener("pointerup", event => releaseDrag(element, event, node));
  element.addEventListener("pointercancel", event => releaseDrag(element, event, node));
}}

function releaseDrag(element, event, node) {{
  if (element.hasPointerCapture(event.pointerId)) element.releasePointerCapture(event.pointerId);
  const wasClick = !node.moved;
  if (activeDragNode === node) activeDragNode = null;
  node.dragging = false;
  node.fixed = true;
  node.vx = 0;
  node.vy = 0;
  element.classList.remove("dragging");
  if (wasClick) {{
    toggleSelection(node);
  }} else {{
    pinNode(node);
    startLocalRelax(node);
  }}
}}

function startLocalRelax(node) {{
  if (selectedNode) {{
    settleSimulation();
    render();
    return;
  }}
  localRelaxNode = node;
  localRelaxStart = performance.now();
  localRelaxUntil = performance.now() + 1000;
  reheat();
}}

function releasePreviousPinnedNode(nextNode) {{
  if (selectedNode || !pinnedNode || pinnedNode === nextNode) return;
  pinnedNode.fixed = false;
  pinnedNode.vx = 0;
  pinnedNode.vy = 0;
  pinnedNode = null;
}}

function pinNode(node) {{
  if (selectedNode) return;
  pinnedNode = node;
  node.fixed = true;
  node.vx = 0;
  node.vy = 0;
}}

function moveNodeToPointer(event, node) {{
  const local = pointerPosition(event);
  const x = local.x + (node.pointerOffsetX || 0);
  const y = local.y + (node.pointerOffsetY || 0);
  if (Math.hypot(x - node.x, y - node.y) > 4) node.moved = true;
  node.x = clamp(x, graph.box_width / 2 + 20, graph.width - graph.box_width / 2 - 20);
  node.y = clamp(y, graph.box_height / 2 + 20, graph.height - graph.box_height / 2 - 20);
}}

function pointerPosition(event) {{
  const point = svg.createSVGPoint();
  point.x = event.clientX;
  point.y = event.clientY;
  return point.matrixTransform(svg.getScreenCTM().inverse());
}}

function toggleSelection(node) {{
  const clearingSelection = selectedNode === node;
  selectedNode = clearingSelection ? null : node;
  focusAnimation++;
  updateVisibility();
  if (selectedNode) spreadFocus(selectedNode);
  if (clearingSelection) spreadAllVisible();
  render();
  if (!clearingSelection) reheat();
}}

function spreadFocus(node) {{
  const centerX = node.x;
  const centerY = node.y;
  setFocusTarget(node, centerX, centerY);

  const neighbors = focusedNeighbors(node);
  const outgoing = neighbors.filter(item => item.kind === "outgoing");
  const incoming = neighbors.filter(item => item.kind === "incoming");
  const bidirectional = neighbors.filter(item => item.kind === "bidirectional");

  placeFocusGroup(outgoing, -Math.PI * 0.32, Math.PI * 0.32, centerX, centerY);
  placeFocusGroup(incoming, Math.PI * 0.68, Math.PI * 1.32, centerX, centerY);
  placeFocusGroup(bidirectional, Math.PI * 0.34, Math.PI * 0.66, centerX, centerY);
  const focusNodes = [node, ...neighbors.map(item => item.node)];
  relaxFocusTargets(focusNodes, node);
  animateFocusLayout(focusNodes, true);
}}

function spreadAllVisible() {{
  for (const node of nodes) {{
    setFocusTarget(node, node.x, node.y);
  }}
  relaxFocusTargets(nodes, null);
  animateFocusLayout(nodes, false);
}}

function focusedNeighbors(node) {{
  const byId = new Map();
  for (const edge of edges) {{
    if (edge.source !== node && edge.target !== node) continue;
    const other = edge.source === node ? edge.target : edge.source;
    const kind = selectedEdgeKindFor(edge, node);
    const existing = byId.get(other.id);
    if (!existing || kindPriority(kind) > kindPriority(existing.kind)) {{
      byId.set(other.id, {{ node: other, kind }});
    }}
  }}

  return [...byId.values()].sort((left, right) =>
    kindPriority(right.kind) - kindPriority(left.kind) || left.node.id.localeCompare(right.node.id)
  );
}}

function placeFocusGroup(items, startAngle, endAngle, centerX, centerY) {{
  const perRing = 8;
  for (let index = 0; index < items.length; index++) {{
    const ring = Math.floor(index / perRing);
    const ringStart = ring * perRing;
    const ringSize = Math.min(perRing, items.length - ringStart);
    const ringIndex = index - ringStart;
    const fraction = ringSize === 1 ? 0.5 : ringIndex / (ringSize - 1);
    const angle = startAngle + (endAngle - startAngle) * fraction;
    const rx = Math.min(graph.width / 2 - graph.box_width - 45, 320 + ring * 165);
    const ry = Math.min(graph.height / 2 - graph.box_height - 60, 235 + ring * 105);
    setFocusTarget(items[index].node, centerX + Math.cos(angle) * rx, centerY + Math.sin(angle) * ry);
  }}
}}

function setFocusTarget(node, x, y) {{
  node.focusStartX = node.x;
  node.focusStartY = node.y;
  node.focusTargetX = clamp(x, graph.box_width / 2 + 20, graph.width - graph.box_width / 2 - 20);
  node.focusTargetY = clamp(y, graph.box_height / 2 + 20, graph.height - graph.box_height / 2 - 20);
  node.fixed = true;
  node.vx = 0;
  node.vy = 0;
}}

function relaxFocusTargets(focusNodes, anchor) {{
  const minDistance = Math.max(graph.box_width, graph.box_height) + 58;
  for (let iteration = 0; iteration < 90; iteration++) {{
    for (let i = 0; i < focusNodes.length; i++) {{
      for (let j = i + 1; j < focusNodes.length; j++) {{
        const a = focusNodes[i];
        const b = focusNodes[j];
        let dx = b.focusTargetX - a.focusTargetX;
        let dy = b.focusTargetY - a.focusTargetY;
        let distance = Math.hypot(dx, dy);
        if (distance < 1) {{
          const angle = (i * 37 + j * 19) * Math.PI / 180;
          dx = Math.cos(angle);
          dy = Math.sin(angle);
          distance = 1;
        }}
        if (distance >= minDistance) continue;

        const push = (minDistance - distance) * 0.45;
        const pushX = dx / distance * push;
        const pushY = dy / distance * push;
        if (a === anchor) {{
          nudgeFocusTarget(b, pushX, pushY);
        }} else if (b === anchor) {{
          nudgeFocusTarget(a, -pushX, -pushY);
        }} else {{
          nudgeFocusTarget(a, -pushX / 2, -pushY / 2);
          nudgeFocusTarget(b, pushX / 2, pushY / 2);
        }}
      }}
    }}
  }}
}}

function nudgeFocusTarget(node, dx, dy) {{
  node.focusTargetX = clamp(node.focusTargetX + dx, graph.box_width / 2 + 20, graph.width - graph.box_width / 2 - 20);
  node.focusTargetY = clamp(node.focusTargetY + dy, graph.box_height / 2 + 20, graph.height - graph.box_height / 2 - 20);
}}

function animateFocusLayout(focusNodes, keepFixed) {{
  const animation = focusAnimation;
  const start = performance.now();
  const duration = 700;

  function frame(now) {{
    if (animation !== focusAnimation) return;
    const progress = clamp((now - start) / duration, 0, 1);
    const eased = easeOutCubic(progress);
    for (const node of focusNodes) {{
      if (node.dragging) continue;
        node.x = node.focusStartX + (node.focusTargetX - node.focusStartX) * eased;
        node.y = node.focusStartY + (node.focusTargetY - node.focusStartY) * eased;
        node.fixed = true;
    }}
    render();
    if (progress < 1) {{
      requestAnimationFrame(frame);
    }} else {{
      for (const node of focusNodes) {{
        node.x = node.focusTargetX;
        node.y = node.focusTargetY;
        node.fixed = keepFixed || node === pinnedNode;
        node.vx = 0;
        node.vy = 0;
      }}
      if (!keepFixed) settleSimulation();
      render();
    }}
  }}

  requestAnimationFrame(frame);
}}

function settleSimulation() {{
  alpha = 0;
  running = false;
  for (const node of nodes) {{
    if (node.dragging) continue;
    node.vx = 0;
    node.vy = 0;
  }}
}}

function easeOutCubic(progress) {{
  return 1 - Math.pow(1 - progress, 3);
}}

function easeInOutCubic(progress) {{
  return progress < 0.5 ? 4 * progress * progress * progress : 1 - Math.pow(-2 * progress + 2, 3) / 2;
}}

function kindPriority(kind) {{
  if (kind === "bidirectional") return 3;
  if (kind === "outgoing") return 2;
  if (kind === "incoming") return 1;
  return 0;
}}

function updateVisibility() {{
  const visible = new Set();
  if (selectedNode) {{
    visible.add(selectedNode.id);
    for (const edge of edges) {{
      if (edge.source === selectedNode) visible.add(edge.target.id);
      if (edge.target === selectedNode) visible.add(edge.source.id);
    }}
  }}

  for (const {{ node, group }} of nodeEls) {{
    const show = !selectedNode || visible.has(node.id);
    group.classList.toggle("hidden", !show);
    group.classList.toggle("selected", selectedNode === node);
  }}
  for (const {{ edge, group, line }} of edgeEls) {{
    const kind = selectedNode ? selectedEdgeKind(edge) : null;
    const show = !selectedNode || kind !== null;
    group.classList.toggle("hidden", !show);
    group.classList.toggle("outgoing", kind === "outgoing");
    group.classList.toggle("incoming", kind === "incoming");
    group.classList.toggle("bidirectional", kind === "bidirectional");
    line.setAttribute("marker-end", `url(#${{kind ? `arrow-${{kind}}` : "arrow"}})`);
  }}
}}

function selectedEdgeKind(edge) {{
  if (!selectedNode) return null;
  return selectedEdgeKindFor(edge, selectedNode);
}}

function selectedEdgeKindFor(edge, node) {{
  const touchesSelection = edge.source === node || edge.target === node;
  if (!touchesSelection) return null;
  const hasReverse = edgeKeys.has(`${{edge.target.id}}->${{edge.source.id}}`);
  if (hasReverse) return "bidirectional";
  return edge.source === node ? "outgoing" : "incoming";
}}

function svgEl(name, attrs = {{}}, text = "") {{
  const element = document.createElementNS("http://www.w3.org/2000/svg", name);
  setAttrs(element, attrs);
  if (text) element.textContent = text;
  return element;
}}

function setAttrs(element, attrs) {{
  for (const [key, value] of Object.entries(attrs)) element.setAttribute(key, value);
}}

function clamp(value, min, max) {{
  return Math.max(min, Math.min(max, value));
}}
</script>
</body>
</html>
"##,
        graph.modules.len(),
        graph.edges.len(),
        width,
        height,
        rows,
        escape_script_json(&graph_json)
    )
}

#[derive(Serialize)]
struct GraphData {
    nodes: Vec<NodeLayout>,
    edges: Vec<TopLevelEdge>,
    box_width: f64,
    box_height: f64,
    width: f64,
    height: f64,
}

#[derive(Serialize)]
struct NodeLayout {
    id: String,
    incoming: usize,
    outgoing: usize,
    x: f64,
    y: f64,
}

#[derive(Clone, Copy, Default)]
struct ModuleEdgeCounts {
    incoming: usize,
    outgoing: usize,
}

fn module_edge_counts(graph: &TopLevelGraph) -> BTreeMap<String, ModuleEdgeCounts> {
    let mut counts = graph
        .modules
        .iter()
        .map(|module| (module.clone(), ModuleEdgeCounts::default()))
        .collect::<BTreeMap<_, _>>();

    for edge in &graph.edges {
        counts.entry(edge.from.clone()).or_default().outgoing += edge.count;
        counts.entry(edge.to.clone()).or_default().incoming += edge.count;
    }

    counts
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_script_json(input: &str) -> String {
    input
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
}

fn internal_dependencies(
    usages: &[Usage],
    module_paths: &BTreeSet<Vec<String>>,
) -> Vec<InternalDependency> {
    let mut dependencies = BTreeMap::<(PathBuf, String, String), usize>::new();
    for usage in usages {
        let Some(to_module_path) = resolve_internal_module(usage, &module_paths) else {
            continue;
        };
        if usage.module_path == to_module_path {
            continue;
        }

        let from_module = module_path_to_string(&usage.module_path);
        let to_module = module_path_to_string(&to_module_path);
        dependencies
            .entry((usage.file.clone(), from_module, to_module))
            .and_modify(|line| *line = (*line).min(usage.line))
            .or_insert(usage.line);
    }

    let mut dependencies = dependencies
        .into_iter()
        .map(
            |((file, from_module, to_module), line)| InternalDependency {
                file,
                line,
                from_module,
                to_module,
            },
        )
        .collect::<Vec<_>>();

    dependencies.sort_by(|left, right| {
        left.file
            .cmp(&right.file)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.from_module.cmp(&right.from_module))
            .then_with(|| left.to_module.cmp(&right.to_module))
    });
    dependencies
}

fn resolve_internal_module(
    usage: &Usage,
    module_paths: &BTreeSet<Vec<String>>,
) -> Option<Vec<String>> {
    let path = usage_path_segments(usage);
    let absolute_path = absolute_module_candidate(&usage.module_path, &path, module_paths)?;

    (1..=absolute_path.len())
        .rev()
        .map(|len| absolute_path[..len].to_vec())
        .find(|candidate| module_paths.contains(candidate))
}

fn usage_path_segments(usage: &Usage) -> Vec<String> {
    usage
        .origin
        .split("::")
        .chain(usage.symbol.split("::"))
        .filter(|segment| !segment.is_empty() && *segment != "*")
        .map(ToOwned::to_owned)
        .collect()
}

fn absolute_module_candidate(
    from_module: &[String],
    path: &[String],
    module_paths: &BTreeSet<Vec<String>>,
) -> Option<Vec<String>> {
    let first = path.first()?;
    if first == "crate" {
        return Some(path.to_vec());
    }

    if first == "self" {
        let mut absolute = from_module.to_vec();
        absolute.extend(path.iter().skip(1).cloned());
        return Some(absolute);
    }

    if first == "super" {
        let mut absolute = from_module[..from_module.len().saturating_sub(1)].to_vec();
        absolute.extend(path.iter().skip(1).cloned());
        return Some(absolute);
    }

    let mut relative = from_module.to_vec();
    relative.extend(path.iter().cloned());
    if has_module_prefix(&relative, module_paths) {
        return Some(relative);
    }

    let mut crate_absolute = vec!["crate".to_owned()];
    crate_absolute.extend(path.iter().cloned());
    if has_module_prefix(&crate_absolute, module_paths) {
        return Some(crate_absolute);
    }

    None
}

fn has_module_prefix(path: &[String], module_paths: &BTreeSet<Vec<String>>) -> bool {
    (1..=path.len()).any(|len| module_paths.contains(&path[..len]))
}

fn module_path_to_string(module_path: &[String]) -> String {
    module_path.join("::")
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct InternalDependency {
    file: PathBuf,
    line: usize,
    from_module: String,
    to_module: String,
}

fn analyze_project(path: impl AsRef<Path>) -> Result<Analysis, String> {
    let mut analyzer = Analyzer::default();
    analyzer.analyze_file(path.as_ref(), vec!["crate".to_owned()])?;
    Ok(analyzer.into_analysis())
}

#[cfg(test)]
fn dependency_usages_for_file(path: impl AsRef<Path>) -> Result<Vec<Usage>, String> {
    Ok(analyze_project(path)?.usages)
}

struct Analysis {
    usages: Vec<Usage>,
    module_paths: BTreeSet<Vec<String>>,
}

#[cfg(test)]
fn dependency_usages(source: &str) -> syn::Result<Vec<FileLocalUsage>> {
    let file = syn::parse_file(source)?;
    Ok(usages_from_file(&file))
}

#[cfg(test)]
fn usages_from_file(file: &File) -> Vec<FileLocalUsage> {
    file_analysis_from_file(file).usages
}

fn file_analysis_from_file(file: &File) -> FileAnalysis {
    let mut visitor = DependencyVisitor::default();
    visitor.visit_file(file);
    visitor.into_analysis()
}

#[derive(Debug, Eq, PartialEq)]
struct Usage {
    file: PathBuf,
    module_path: Vec<String>,
    line: usize,
    origin: String,
    symbol: String,
}

#[derive(Debug, Eq, PartialEq)]
struct FileLocalUsage {
    module_path_suffix: Vec<String>,
    line: usize,
    origin: String,
    symbol: String,
}

struct FileAnalysis {
    usages: Vec<FileLocalUsage>,
    inline_module_paths: Vec<Vec<String>>,
}

#[derive(Default)]
struct Analyzer {
    seen_files: HashSet<PathBuf>,
    module_paths: BTreeSet<Vec<String>>,
    usages: Vec<Usage>,
}

impl Analyzer {
    fn analyze_file(&mut self, path: &Path, module_path: Vec<String>) -> Result<(), String> {
        let path = normalize_path(path);
        if !self.seen_files.insert(path.clone()) {
            return Ok(());
        }
        self.module_paths.insert(module_path.clone());

        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let file = syn::parse_file(&source)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;

        let file_analysis = file_analysis_from_file(&file);
        self.module_paths.extend(
            file_analysis
                .inline_module_paths
                .into_iter()
                .map(|inline_path| module_path.iter().cloned().chain(inline_path).collect()),
        );
        self.usages
            .extend(file_analysis.usages.into_iter().map(|usage| {
                Usage {
                    file: path.clone(),
                    module_path: module_path
                        .iter()
                        .cloned()
                        .chain(usage.module_path_suffix)
                        .collect(),
                    line: usage.line,
                    origin: usage.origin,
                    symbol: usage.symbol,
                }
            }));

        let module_dir = module_dir_for_child_modules(&path);
        for module in module_files_referenced_by(&file, &module_dir, &module_path) {
            self.analyze_file(&module.file, module.module_path)?;
        }

        Ok(())
    }

    fn into_analysis(self) -> Analysis {
        Analysis {
            usages: self.usages,
            module_paths: self.module_paths,
        }
    }
}

#[derive(Default)]
struct DependencyVisitor {
    usages: BTreeMap<(Vec<String>, String, String), usize>,
    module_path: Vec<String>,
    inline_module_paths: Vec<Vec<String>>,
}

impl DependencyVisitor {
    fn add_usage(&mut self, usage: Option<FileLocalUsage>) {
        let Some(mut usage) = usage else {
            return;
        };
        usage.module_path_suffix = self.module_path.clone();

        self.usages
            .entry((usage.module_path_suffix, usage.origin, usage.symbol))
            .and_modify(|line| *line = (*line).min(usage.line))
            .or_insert(usage.line);
    }

    fn add_use_tree(&mut self, tree: &UseTree, prefix: &mut Vec<String>) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(ident_to_string(&path.ident));
                self.add_use_tree(&path.tree, prefix);
                prefix.pop();
            }
            UseTree::Name(name) => {
                prefix.push(ident_to_string(&name.ident));
                self.add_usage(usage_from_segments(
                    prefix.iter().cloned(),
                    span_line(name.ident.span()),
                    SymbolStyle::Plain,
                ));
                prefix.pop();
            }
            UseTree::Rename(rename) => {
                prefix.push(ident_to_string(&rename.ident));
                self.add_usage(usage_from_segments(
                    prefix.iter().cloned(),
                    span_line(rename.ident.span()),
                    SymbolStyle::Plain,
                ));
                prefix.pop();
            }
            UseTree::Glob(glob) => {
                self.add_usage(usage_from_segments(
                    prefix.iter().cloned().chain(["*".to_owned()]),
                    span_line(glob.star_token.span()),
                    SymbolStyle::Plain,
                ));
            }
            UseTree::Group(group) => {
                for item in &group.items {
                    self.add_use_tree(item, prefix);
                }
            }
        }
    }

    fn add_qualified_path(&mut self, path: &SynPath, line: usize, style: SymbolStyle) {
        if path.leading_colon.is_none() && path.segments.len() < 2 {
            return;
        }

        self.add_usage(usage_from_segments(
            path.segments
                .iter()
                .map(|segment| ident_to_string(&segment.ident)),
            line,
            style,
        ));
    }

    fn add_qself_trait_path(&mut self, path: &SynPath, qself: &QSelf, line: usize) {
        if qself.position == 0 {
            return;
        }

        self.add_usage(usage_from_segments(
            path.segments
                .iter()
                .take(qself.position)
                .map(|segment| ident_to_string(&segment.ident)),
            line,
            SymbolStyle::Plain,
        ));
    }

    fn into_analysis(self) -> FileAnalysis {
        let mut usages = self
            .usages
            .into_iter()
            .map(
                |((module_path_suffix, origin, symbol), line)| FileLocalUsage {
                    module_path_suffix,
                    line,
                    origin,
                    symbol,
                },
            )
            .collect::<Vec<_>>();

        usages.sort_by(|left, right| {
            left.line
                .cmp(&right.line)
                .then_with(|| left.origin.cmp(&right.origin))
                .then_with(|| left.symbol.cmp(&right.symbol))
        });
        FileAnalysis {
            usages,
            inline_module_paths: self.inline_module_paths,
        }
    }
}

impl<'ast> Visit<'ast> for DependencyVisitor {
    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        self.add_qualified_path(
            attribute.path(),
            span_line(attribute.path().span()),
            SymbolStyle::Plain,
        );
        visit::visit_attribute(self, attribute);
    }

    fn visit_expr_path(&mut self, expr_path: &'ast ExprPath) {
        if let Some(qself) = &expr_path.qself {
            self.add_qself_trait_path(&expr_path.path, qself, span_line(expr_path.path.span()));
        }
        self.add_qualified_path(
            &expr_path.path,
            span_line(expr_path.path.span()),
            SymbolStyle::Plain,
        );
        visit::visit_expr_path(self, expr_path);
    }

    fn visit_item_extern_crate(&mut self, item: &'ast ItemExternCrate) {
        self.add_usage(Some(FileLocalUsage {
            module_path_suffix: Vec::new(),
            line: span_line(item.ident.span()),
            origin: "extern crate".to_owned(),
            symbol: ident_to_string(&item.ident),
        }));
        visit::visit_item_extern_crate(self, item);
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        self.add_use_tree(&item.tree, &mut Vec::new());
    }

    fn visit_item_mod(&mut self, item_mod: &'ast ItemMod) {
        let Some((_, items)) = &item_mod.content else {
            return;
        };

        self.module_path.push(ident_to_string(&item_mod.ident));
        self.inline_module_paths.push(self.module_path.clone());
        for item in items {
            self.visit_item(item);
        }
        self.module_path.pop();
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        self.add_qualified_path(&mac.path, span_line(mac.path.span()), SymbolStyle::Macro);
        visit::visit_macro(self, mac);
    }

    fn visit_type_path(&mut self, type_path: &'ast TypePath) {
        if let Some(qself) = &type_path.qself {
            self.add_qself_trait_path(&type_path.path, qself, span_line(type_path.path.span()));
        }
        self.add_qualified_path(
            &type_path.path,
            span_line(type_path.path.span()),
            SymbolStyle::Plain,
        );
        visit::visit_type_path(self, type_path);
    }
}

#[derive(Clone, Copy)]
enum SymbolStyle {
    Plain,
    Macro,
}

fn usage_from_segments(
    segments: impl IntoIterator<Item = String>,
    line: usize,
    style: SymbolStyle,
) -> Option<FileLocalUsage> {
    let segments = segments.into_iter().collect::<Vec<_>>();
    let first = segments.first()?;

    if !is_reportable_origin_root(first) {
        return None;
    }

    let (origin, symbol) = if is_local_root(first) {
        if segments.len() < 3 {
            return None;
        }
        (segments[..2].join("::"), segments[2..].join("::"))
    } else {
        if segments.len() < 2 {
            return None;
        }
        (
            segments[..segments.len() - 1].join("::"),
            segments.last()?.to_owned(),
        )
    };

    if symbol == "*" {
        return Some(FileLocalUsage {
            module_path_suffix: Vec::new(),
            line,
            origin,
            symbol,
        });
    }

    if !symbol.split("::").all(is_identifier) {
        return None;
    }

    Some(FileLocalUsage {
        module_path_suffix: Vec::new(),
        line,
        origin,
        symbol: match style {
            SymbolStyle::Plain => symbol,
            SymbolStyle::Macro => format!("{symbol}!"),
        },
    })
}

fn module_files_referenced_by(
    file: &File,
    module_dir: &Path,
    module_path: &[String],
) -> Vec<ModuleFile> {
    let mut visitor = ModuleFileVisitor {
        module_dir: module_dir.to_owned(),
        module_path: module_path.to_vec(),
        module_files: Vec::new(),
    };
    visitor.visit_file(file);
    visitor.module_files
}

struct ModuleFile {
    file: PathBuf,
    module_path: Vec<String>,
}

struct ModuleFileVisitor {
    module_dir: PathBuf,
    module_path: Vec<String>,
    module_files: Vec<ModuleFile>,
}

impl<'ast> Visit<'ast> for ModuleFileVisitor {
    fn visit_item_mod(&mut self, item_mod: &'ast ItemMod) {
        let module_name = ident_to_string(&item_mod.ident);
        if let Some((_, items)) = &item_mod.content {
            let previous_dir = self.module_dir.clone();
            self.module_dir = self.module_dir.join(&module_name);
            self.module_path.push(module_name);
            for item in items {
                self.visit_item(item);
            }
            self.module_path.pop();
            self.module_dir = previous_dir;
            return;
        }

        if let Some(module_file) = module_file_for(item_mod, &self.module_dir) {
            let mut module_path = self.module_path.clone();
            module_path.push(module_name);
            self.module_files.push(ModuleFile {
                file: module_file,
                module_path,
            });
        }
    }
}

fn module_file_for(item_mod: &ItemMod, module_dir: &Path) -> Option<PathBuf> {
    if item_mod.content.is_some() {
        return None;
    }

    if let Some(path) = path_attribute(item_mod) {
        return Some(module_dir.join(path.value()));
    }

    let module_name = ident_to_string(&item_mod.ident);
    let flat = module_dir.join(format!("{module_name}.rs"));
    if flat.is_file() {
        return Some(flat);
    }

    let nested = module_dir.join(&module_name).join("mod.rs");
    if nested.is_file() {
        return Some(nested);
    }

    Some(flat)
}

fn path_attribute(item_mod: &ItemMod) -> Option<LitStr> {
    item_mod.attrs.iter().find_map(|attribute| {
        if !attribute.path().is_ident("path") {
            return None;
        }
        attribute.parse_args::<LitStr>().ok()
    })
}

fn module_dir_for_child_modules(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    match path.file_name().and_then(|name| name.to_str()) {
        Some("lib.rs" | "main.rs" | "mod.rs") => parent.to_owned(),
        _ => parent.join(path.file_stem().unwrap_or_default()),
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

fn ident_to_string(ident: &Ident) -> String {
    ident.to_string().trim_start_matches("r#").to_owned()
}

fn span_line(span: Span) -> usize {
    span.start().line
}

fn is_reportable_origin_root(candidate: &str) -> bool {
    is_identifier(candidate) && !is_builtin_or_prelude_root(candidate)
}

fn is_local_root(candidate: &str) -> bool {
    matches!(candidate, "self" | "super" | "crate")
}

fn is_builtin_or_prelude_root(candidate: &str) -> bool {
    matches!(
        candidate,
        "_" | "Self"
            | "std"
            | "core"
            | "alloc"
            | "bool"
            | "char"
            | "str"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "Option"
            | "Some"
            | "None"
            | "Result"
            | "Ok"
            | "Err"
            | "Vec"
            | "String"
            | "Box"
            | "ToString"
            | "From"
            | "Into"
            | "AsRef"
            | "AsMut"
            | "Default"
            | "Clone"
            | "Copy"
            | "Drop"
            | "Debug"
            | "Display"
            | "Iterator"
            | "IntoIterator"
            | "Send"
            | "Sync"
            | "Sized"
    )
}

fn is_identifier(candidate: &str) -> bool {
    let mut bytes = candidate.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    is_identifier_start(first) && bytes.all(is_identifier_continue)
}

fn is_identifier_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_identifier_continue(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{
        analyze_project, dependency_usages, dependency_usages_for_file, internal_dependencies,
        render_top_level_html, top_level_graph, FileLocalUsage, InternalDependency, TopLevelEdge,
        Usage,
    };

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
