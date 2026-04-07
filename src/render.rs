// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;

use include_dir::{include_dir, Dir};
use serde::Serialize;

use crate::graph::{TopLevelEdge, TopLevelGraph};

static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets");

pub(crate) fn render_top_level_html(graph: &TopLevelGraph) -> String {
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
{}</script>
</body>
</html>
"##,
        graph.modules.len(),
        graph.edges.len(),
        width,
        height,
        rows,
        escape_script_json(&graph_json),
        graph_script()
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

fn graph_script() -> &'static str {
    ASSETS
        .get_file("graph.js")
        .and_then(|file| file.contents_utf8())
        .expect("embedded graph.js asset should be present and valid UTF-8")
}
