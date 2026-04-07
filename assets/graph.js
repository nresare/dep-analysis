// SPDX-License-Identifier: MIT

const graph = JSON.parse(document.getElementById("graph-data").textContent);
const svg = document.querySelector("svg");
const edgeLayer = document.getElementById("edges");
const nodeLayer = document.getElementById("nodes");
const nodes = graph.nodes.map(node => ({ ...node, vx: 0, vy: 0, fixed: false, dragging: false }));
const nodeById = new Map(nodes.map(node => [node.id, node]));
const edges = graph.edges
  .map(edge => ({ ...edge, source: nodeById.get(edge.from), target: nodeById.get(edge.to) }))
  .filter(edge => edge.source && edge.target);
const edgeKeys = new Set(edges.map(edge => `${edge.source.id}->${edge.target.id}`));
const maxEdgeCount = Math.max(1, ...edges.map(edge => edge.count));
let selectedNode = null;
let focusAnimation = 0;
let pinnedNode = null;
let activeDragNode = null;
let localRelaxNode = null;
let localRelaxStart = 0;
let localRelaxUntil = 0;
const edgeEls = edges.map(edge => {
  const group = svgEl("g", { class: "edge" });
  const line = svgEl("line", {
    "stroke-width": (1 + 5 * edge.count / maxEdgeCount).toFixed(2),
    "marker-end": "url(#arrow)"
  });
  const label = svgEl("text", {}, String(edge.count));
  group.append(line, label);
  edgeLayer.append(group);
  return { edge, group, line, label };
});
const nodeEls = nodes.map(node => {
  const group = svgEl("g", { class: "node" });
  const rect = svgEl("rect", {
    x: -graph.box_width / 2,
    y: -graph.box_height / 2,
    width: graph.box_width,
    height: graph.box_height
  });
  const name = svgEl("text", { class: "module-name", x: 0, y: -8 }, node.id);
  const counts = svgEl("text", { class: "module-counts", x: 0, y: 14 }, `in: ${node.incoming} · out: ${node.outgoing}`);
  group.append(rect, name, counts);
  nodeLayer.append(group);
  installDrag(group, node);
  return { node, group };
});

let alpha = 1;
let running = true;
requestAnimationFrame(tick);

function tick() {
  taperLocalRelax();
  for (let step = 0; step < 2; step++) applyForces();
  render();
  alpha *= 0.985;
  if (localRelaxNode && performance.now() >= localRelaxUntil) {
    localRelaxNode = null;
    settleSimulation();
  }
  running = alpha > 0.01 || nodes.some(node => node.dragging) || localRelaxNode;
  if (running) requestAnimationFrame(tick);
}

function taperLocalRelax() {
  if (!localRelaxNode) return;
  const remaining = Math.max(0, localRelaxUntil - performance.now());
  const duration = Math.max(1, localRelaxUntil - localRelaxStart);
  const strength = remaining / duration;
  alpha = Math.min(alpha, 0.75 * easeInOutCubic(strength));
}

function reheat() {
  alpha = Math.max(alpha, 0.75);
  if (!running) {
    running = true;
    requestAnimationFrame(tick);
  }
}

function applyForces() {
  const localForceNode = activeDragNode || localRelaxNode;
  if (localForceNode && !selectedNode) {
    applyLocalDragForces(localForceNode);
    return;
  }

  const cx = graph.width / 2;
  const cy = graph.height / 2;
  for (const node of nodes) {
    if (!node.fixed && !node.dragging) {
      node.vx += (cx - node.x) * 0.0007 * alpha;
      node.vy += (cy - node.y) * 0.0007 * alpha;
    }
  }
  for (const edge of edges) {
    const dx = edge.target.x - edge.source.x;
    const dy = edge.target.y - edge.source.y;
    const distance = Math.max(1, Math.hypot(dx, dy));
    const desired = 260;
    const force = (distance - desired) * 0.0009 * alpha;
    const fx = dx / distance * force;
    const fy = dy / distance * force;
    if (!edge.source.fixed && !edge.source.dragging) {
      edge.source.vx += fx;
      edge.source.vy += fy;
    }
    if (!edge.target.fixed && !edge.target.dragging) {
      edge.target.vx -= fx;
      edge.target.vy -= fy;
    }
  }
  for (let i = 0; i < nodes.length; i++) {
    for (let j = i + 1; j < nodes.length; j++) {
      const a = nodes[i];
      const b = nodes[j];
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const distance = Math.max(1, Math.hypot(dx, dy));
      const minDistance = Math.max(graph.box_width, graph.box_height) + 36;
      if (distance < minDistance) {
        const push = (minDistance - distance) * 0.018 * alpha;
        applyPairForce(a, b, dx / distance * push, dy / distance * push);
      } else {
        const repel = 2200 / (distance * distance) * alpha;
        applyPairForce(a, b, dx / distance * repel, dy / distance * repel);
      }
    }
  }
  for (const node of nodes) {
    if (node.fixed || node.dragging) continue;
    node.vx *= 0.86;
    node.vy *= 0.86;
    node.x = clamp(node.x + node.vx, graph.box_width / 2 + 20, graph.width - graph.box_width / 2 - 20);
    node.y = clamp(node.y + node.vy, graph.box_height / 2 + 20, graph.height - graph.box_height / 2 - 20);
  }
}

function applyLocalDragForces(draggedNode) {
  for (const edge of edges) {
    if (edge.source !== draggedNode && edge.target !== draggedNode) continue;
    const other = edge.source === draggedNode ? edge.target : edge.source;
    const dx = other.x - draggedNode.x;
    const dy = other.y - draggedNode.y;
    const distance = Math.max(1, Math.hypot(dx, dy));
    const desired = 260;
    const force = (distance - desired) * 0.00055 * alpha;
    if (!other.fixed && !other.dragging) {
      other.vx -= dx / distance * force;
      other.vy -= dy / distance * force;
    }
  }

  applyLocalCollisionForces();

  for (const node of nodes) {
    if (node.fixed || node.dragging) continue;
    node.vx *= 0.78;
    node.vy *= 0.78;
    node.x = clamp(node.x + node.vx, graph.box_width / 2 + 20, graph.width - graph.box_width / 2 - 20);
    node.y = clamp(node.y + node.vy, graph.box_height / 2 + 20, graph.height - graph.box_height / 2 - 20);
  }
}

function applyLocalCollisionForces() {
  const minDistance = Math.max(graph.box_width, graph.box_height) + 44;
  for (let i = 0; i < nodes.length; i++) {
    for (let j = i + 1; j < nodes.length; j++) {
      const a = nodes[i];
      const b = nodes[j];
      let dx = b.x - a.x;
      let dy = b.y - a.y;
      let distance = Math.hypot(dx, dy);
      if (distance < 1) {
        const angle = (i * 37 + j * 19) * Math.PI / 180;
        dx = Math.cos(angle);
        dy = Math.sin(angle);
        distance = 1;
      }
      if (distance >= minDistance) continue;

      const push = (minDistance - distance) * 0.045 * alpha;
      applyPairForce(a, b, dx / distance * push, dy / distance * push);
    }
  }
}

function applyPairForce(a, b, fx, fy) {
  if (!a.fixed && !a.dragging) {
    a.vx -= fx;
    a.vy -= fy;
  }
  if (!b.fixed && !b.dragging) {
    b.vx += fx;
    b.vy += fy;
  }
}

function render() {
  for (const { edge, line, label } of edgeEls) {
    const points = edgePoints(edge.source, edge.target);
    const labelPoint = edgeLabelPoint(edge);
    setAttrs(line, { x1: points.x1, y1: points.y1, x2: points.x2, y2: points.y2 });
    setAttrs(label, { x: labelPoint.x, y: labelPoint.y });
  }
  for (const { node, group } of nodeEls) {
    setAttrs(group, { transform: `translate(${node.x}, ${node.y})` });
  }
}

function edgeLabelPoint(edge) {
  const dx = edge.target.x - edge.source.x;
  const dy = edge.target.y - edge.source.y;
  const length = Math.max(1, Math.hypot(dx, dy));
  const hasReverse = edgeKeys.has(`${edge.target.id}->${edge.source.id}`);
  const offset = hasReverse ? 18 : 0;
  return {
    x: (edge.source.x + edge.target.x) / 2 - dy / length * offset,
    y: (edge.source.y + edge.target.y) / 2 + dx / length * offset
  };
}

function edgePoints(from, to) {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = Math.max(1, Math.hypot(dx, dy));
  const ux = dx / length;
  const uy = dy / length;
  const fromOffset = boxEdgeOffset(ux, uy);
  const toOffset = boxEdgeOffset(-ux, -uy);
  return {
    x1: from.x + ux * fromOffset,
    y1: from.y + uy * fromOffset,
    x2: to.x - ux * toOffset,
    y2: to.y - uy * toOffset
  };
}

function boxEdgeOffset(ux, uy) {
  const horizontal = Math.abs(ux) > Number.EPSILON ? graph.box_width / 2 / Math.abs(ux) : Infinity;
  const vertical = Math.abs(uy) > Number.EPSILON ? graph.box_height / 2 / Math.abs(uy) : Infinity;
  return Math.min(horizontal, vertical);
}

function installDrag(element, node) {
  element.addEventListener("pointerdown", event => {
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
  });
  element.addEventListener("pointermove", event => {
    if (!node.dragging) return;
    moveNodeToPointer(event, node);
    reheat();
  });
  element.addEventListener("pointerup", event => releaseDrag(element, event, node));
  element.addEventListener("pointercancel", event => releaseDrag(element, event, node));
}

function releaseDrag(element, event, node) {
  if (element.hasPointerCapture(event.pointerId)) element.releasePointerCapture(event.pointerId);
  const wasClick = !node.moved;
  if (activeDragNode === node) activeDragNode = null;
  node.dragging = false;
  node.fixed = true;
  node.vx = 0;
  node.vy = 0;
  element.classList.remove("dragging");
  if (wasClick) {
    toggleSelection(node);
  } else {
    pinNode(node);
    startLocalRelax(node);
  }
}

function startLocalRelax(node) {
  if (selectedNode) {
    settleSimulation();
    render();
    return;
  }
  localRelaxNode = node;
  localRelaxStart = performance.now();
  localRelaxUntil = performance.now() + 1000;
  reheat();
}

function releasePreviousPinnedNode(nextNode) {
  if (selectedNode || !pinnedNode || pinnedNode === nextNode) return;
  pinnedNode.fixed = false;
  pinnedNode.vx = 0;
  pinnedNode.vy = 0;
  pinnedNode = null;
}

function pinNode(node) {
  if (selectedNode) return;
  pinnedNode = node;
  node.fixed = true;
  node.vx = 0;
  node.vy = 0;
}

function moveNodeToPointer(event, node) {
  const local = pointerPosition(event);
  const x = local.x + (node.pointerOffsetX || 0);
  const y = local.y + (node.pointerOffsetY || 0);
  if (Math.hypot(x - node.x, y - node.y) > 4) node.moved = true;
  node.x = clamp(x, graph.box_width / 2 + 20, graph.width - graph.box_width / 2 - 20);
  node.y = clamp(y, graph.box_height / 2 + 20, graph.height - graph.box_height / 2 - 20);
}

function pointerPosition(event) {
  const point = svg.createSVGPoint();
  point.x = event.clientX;
  point.y = event.clientY;
  return point.matrixTransform(svg.getScreenCTM().inverse());
}

function toggleSelection(node) {
  const clearingSelection = selectedNode === node;
  selectedNode = clearingSelection ? null : node;
  focusAnimation++;
  updateVisibility();
  if (selectedNode) spreadFocus(selectedNode);
  if (clearingSelection) spreadAllVisible();
  render();
  if (!clearingSelection) reheat();
}

function spreadFocus(node) {
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
}

function spreadAllVisible() {
  for (const node of nodes) {
    setFocusTarget(node, node.x, node.y);
  }
  relaxFocusTargets(nodes, null);
  animateFocusLayout(nodes, false);
}

function focusedNeighbors(node) {
  const byId = new Map();
  for (const edge of edges) {
    if (edge.source !== node && edge.target !== node) continue;
    const other = edge.source === node ? edge.target : edge.source;
    const kind = selectedEdgeKindFor(edge, node);
    const existing = byId.get(other.id);
    if (!existing || kindPriority(kind) > kindPriority(existing.kind)) {
      byId.set(other.id, { node: other, kind });
    }
  }

  return [...byId.values()].sort((left, right) =>
    kindPriority(right.kind) - kindPriority(left.kind) || left.node.id.localeCompare(right.node.id)
  );
}

function placeFocusGroup(items, startAngle, endAngle, centerX, centerY) {
  const perRing = 8;
  for (let index = 0; index < items.length; index++) {
    const ring = Math.floor(index / perRing);
    const ringStart = ring * perRing;
    const ringSize = Math.min(perRing, items.length - ringStart);
    const ringIndex = index - ringStart;
    const fraction = ringSize === 1 ? 0.5 : ringIndex / (ringSize - 1);
    const angle = startAngle + (endAngle - startAngle) * fraction;
    const rx = Math.min(graph.width / 2 - graph.box_width - 45, 320 + ring * 165);
    const ry = Math.min(graph.height / 2 - graph.box_height - 60, 235 + ring * 105);
    setFocusTarget(items[index].node, centerX + Math.cos(angle) * rx, centerY + Math.sin(angle) * ry);
  }
}

function setFocusTarget(node, x, y) {
  node.focusStartX = node.x;
  node.focusStartY = node.y;
  node.focusTargetX = clamp(x, graph.box_width / 2 + 20, graph.width - graph.box_width / 2 - 20);
  node.focusTargetY = clamp(y, graph.box_height / 2 + 20, graph.height - graph.box_height / 2 - 20);
  node.fixed = true;
  node.vx = 0;
  node.vy = 0;
}

function relaxFocusTargets(focusNodes, anchor) {
  const minDistance = Math.max(graph.box_width, graph.box_height) + 58;
  for (let iteration = 0; iteration < 90; iteration++) {
    for (let i = 0; i < focusNodes.length; i++) {
      for (let j = i + 1; j < focusNodes.length; j++) {
        const a = focusNodes[i];
        const b = focusNodes[j];
        let dx = b.focusTargetX - a.focusTargetX;
        let dy = b.focusTargetY - a.focusTargetY;
        let distance = Math.hypot(dx, dy);
        if (distance < 1) {
          const angle = (i * 37 + j * 19) * Math.PI / 180;
          dx = Math.cos(angle);
          dy = Math.sin(angle);
          distance = 1;
        }
        if (distance >= minDistance) continue;

        const push = (minDistance - distance) * 0.45;
        const pushX = dx / distance * push;
        const pushY = dy / distance * push;
        if (a === anchor) {
          nudgeFocusTarget(b, pushX, pushY);
        } else if (b === anchor) {
          nudgeFocusTarget(a, -pushX, -pushY);
        } else {
          nudgeFocusTarget(a, -pushX / 2, -pushY / 2);
          nudgeFocusTarget(b, pushX / 2, pushY / 2);
        }
      }
    }
  }
}

function nudgeFocusTarget(node, dx, dy) {
  node.focusTargetX = clamp(node.focusTargetX + dx, graph.box_width / 2 + 20, graph.width - graph.box_width / 2 - 20);
  node.focusTargetY = clamp(node.focusTargetY + dy, graph.box_height / 2 + 20, graph.height - graph.box_height / 2 - 20);
}

function animateFocusLayout(focusNodes, keepFixed) {
  const animation = focusAnimation;
  const start = performance.now();
  const duration = 700;

  function frame(now) {
    if (animation !== focusAnimation) return;
    const progress = clamp((now - start) / duration, 0, 1);
    const eased = easeOutCubic(progress);
    for (const node of focusNodes) {
      if (node.dragging) continue;
        node.x = node.focusStartX + (node.focusTargetX - node.focusStartX) * eased;
        node.y = node.focusStartY + (node.focusTargetY - node.focusStartY) * eased;
        node.fixed = true;
    }
    render();
    if (progress < 1) {
      requestAnimationFrame(frame);
    } else {
      for (const node of focusNodes) {
        node.x = node.focusTargetX;
        node.y = node.focusTargetY;
        node.fixed = keepFixed || node === pinnedNode;
        node.vx = 0;
        node.vy = 0;
      }
      if (!keepFixed) settleSimulation();
      render();
    }
  }

  requestAnimationFrame(frame);
}

function settleSimulation() {
  alpha = 0;
  running = false;
  for (const node of nodes) {
    if (node.dragging) continue;
    node.vx = 0;
    node.vy = 0;
  }
}

function easeOutCubic(progress) {
  return 1 - Math.pow(1 - progress, 3);
}

function easeInOutCubic(progress) {
  return progress < 0.5 ? 4 * progress * progress * progress : 1 - Math.pow(-2 * progress + 2, 3) / 2;
}

function kindPriority(kind) {
  if (kind === "bidirectional") return 3;
  if (kind === "outgoing") return 2;
  if (kind === "incoming") return 1;
  return 0;
}

function updateVisibility() {
  const visible = new Set();
  if (selectedNode) {
    visible.add(selectedNode.id);
    for (const edge of edges) {
      if (edge.source === selectedNode) visible.add(edge.target.id);
      if (edge.target === selectedNode) visible.add(edge.source.id);
    }
  }

  for (const { node, group } of nodeEls) {
    const show = !selectedNode || visible.has(node.id);
    group.classList.toggle("hidden", !show);
    group.classList.toggle("selected", selectedNode === node);
  }
  for (const { edge, group, line } of edgeEls) {
    const kind = selectedNode ? selectedEdgeKind(edge) : null;
    const show = !selectedNode || kind !== null;
    group.classList.toggle("hidden", !show);
    group.classList.toggle("outgoing", kind === "outgoing");
    group.classList.toggle("incoming", kind === "incoming");
    group.classList.toggle("bidirectional", kind === "bidirectional");
    line.setAttribute("marker-end", `url(#${kind ? `arrow-${kind}` : "arrow"})`);
  }
}

function selectedEdgeKind(edge) {
  if (!selectedNode) return null;
  return selectedEdgeKindFor(edge, selectedNode);
}

function selectedEdgeKindFor(edge, node) {
  const touchesSelection = edge.source === node || edge.target === node;
  if (!touchesSelection) return null;
  const hasReverse = edgeKeys.has(`${edge.target.id}->${edge.source.id}`);
  if (hasReverse) return "bidirectional";
  return edge.source === node ? "outgoing" : "incoming";
}

function svgEl(name, attrs = {}, text = "") {
  const element = document.createElementNS("http://www.w3.org/2000/svg", name);
  setAttrs(element, attrs);
  if (text) element.textContent = text;
  return element;
}

function setAttrs(element, attrs) {
  for (const [key, value] of Object.entries(attrs)) element.setAttribute(key, value);
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}
