// Live routing topology: peers on an ellipse around the central gateway node.
// Layout math from 9router's ProviderTopology; no graph lib — plain SVG.
// Real-time: one EventSource on /api/events drives the flow animation AND the
// communication log. Peer-set refreshes via /api/topology poll (10s).
(function () {
  var el = document.getElementById('topo');
  var log = document.getElementById('flowlog');
  if (!el) return;
  var NS = 'http://www.w3.org/2000/svg';
  var NODE_H = 34, GAP = 24, GATE_W = 130, GATE_H = 44;
  // Node pill width sized to its label (dot + padding + ~7.6px/char at 13px).
  function nodeW(name, channel) {
    var chars = name.length + (channel ? 2 : 0); // ⛓ suffix space
    return Math.max(110, Math.min(300, Math.ceil(chars * 7.6) + 32));
  }
  var ACTIVE_DECAY_MS = 8000;
  var FLOW_STEP_MS = 450;       // per-hop packet duration
  var state = { peers: [], lastActive: {}, counts: {}, errs: {}, timer: null };
  var edgePaths = {};           // peer name -> "d" path gateway→peer
  var nodePos = {};             // peer name -> {x,y}
  var svg = null;               // persistent <svg>; packets layer survives re-renders
  var packetsG = null;          // <g> for in-flight packet circles

  function layout(peers) {
    var n = peers.length;
    var maxW = 0;
    peers.forEach(function (p) { maxW = Math.max(maxW, nodeW(p.name, p.channel)); });
    var minRx = ((maxW + GAP) * Math.max(n, 1)) / (2 * Math.PI);
    var rx = Math.max(280, minRx);
    // small peer counts: flat wide ellipse, half-step start angle so 2 peers
    // sit left/right of the gateway instead of stacking vertically
    var ry = Math.max(110, rx * (n <= 4 ? 0.4 : 0.55));
    var a0 = -Math.PI / 2 + (n > 1 ? Math.PI / n : 0);
    var nodes = [{ id: 'gateway', x: 0, y: 0, w: GATE_W, h: GATE_H }];
    for (var i = 0; i < n; i++) {
      var a = a0 + (2 * Math.PI * i) / n;
      nodes.push({
        id: peers[i].name, x: rx * Math.cos(a), y: ry * Math.sin(a),
        w: nodeW(peers[i].name, peers[i].channel), h: NODE_H, peer: peers[i]
      });
    }
    return { nodes: nodes, w: rx * 2 + maxW + GATE_W, h: ry * 2 + NODE_H + GATE_H };
  }

  function edgePath(gate, node) {
    var mx = (gate.x + node.x) / 2, my = (gate.y + node.y) / 2 - 18;
    return 'M' + gate.x + ',' + gate.y + ' Q' + mx + ',' + my + ' ' + node.x + ',' + node.y;
  }

  function edgeClass(p) {
    var since = state.lastActive[p.name] ? Date.now() - state.lastActive[p.name] : Infinity;
    if (since < ACTIVE_DECAY_MS) return state.errs[p.name] ? 'edge-err' : 'edge-active';
    if (state.errs[p.name]) return 'edge-err';
    if (p.state === 'pending') return 'edge-pending';
    return 'edge-idle';
  }

  function dotClass(p) {
    if (p.state === 'pending') return 'dot-pending';
    if (p.state !== 'accepted') return 'dot-pending';
    if (p.healthy === false) return 'dot-bad';
    if (p.healthy == null) return 'dot-unknown';
    return 'dot-ok';
  }

  function svgEl(tag, attrs) {
    var e = document.createElementNS(NS, tag);
    for (var k in attrs) e.setAttribute(k, attrs[k]);
    return e;
  }

  function render() {
    var l = layout(state.peers);
    var gate = l.nodes[0];
    if (!svg) {
      svg = svgEl('svg', {
        viewBox: (-l.w / 2) + ' ' + (-l.h / 2) + ' ' + l.w + ' ' + l.h,
        class: 'topo-svg'
      });
      packetsG = svgEl('g', {});
      el.replaceChildren(svg);
      svg.appendChild(packetsG);
    }
    var fresh = svgEl('g', {});
    var byId = {};
    l.nodes.forEach(function (n) { byId[n.id] = n; });

    // edges first (under nodes); remember paths for flow animation
    edgePaths = {};
    state.peers.forEach(function (p) {
      var node = byId[p.name];
      var d = edgePath(gate, node);
      edgePaths[p.name] = d;
      nodePos[p.name] = { x: node.x, y: node.y };
      var cls = edgeClass(p);
      var g = svgEl('g', { class: 'edge ' + cls });
      g.appendChild(svgEl('path', { d: d }));
      fresh.appendChild(g);
    });

    // gateway node
    var gg = svgEl('g', {
      class: 'topo-gate' + (Object.keys(state.lastActive).some(function (k) {
        return Date.now() - state.lastActive[k] < ACTIVE_DECAY_MS;
      }) ? ' busy' : ''),
      transform: 'translate(0,0)'
    });
    gg.appendChild(svgEl('rect', { x: -GATE_W / 2, y: -GATE_H / 2, width: GATE_W, height: GATE_H, rx: 10 }));
    var gt = svgEl('text', { y: 5, class: 'topo-gate-label' });
    gt.textContent = '◈ gateway';
    gg.appendChild(gt);
    var gateLink = svgEl('a', { href: '/logs/full' });
    gateLink.appendChild(gg);
    fresh.appendChild(gateLink);

    // peer nodes
    state.peers.forEach(function (p) {
      var node = byId[p.name];
      var w = node.w;
      var g = svgEl('g', { transform: 'translate(' + node.x + ',' + node.y + ')' });
      var box = svgEl('rect', {
        x: -w / 2, y: -NODE_H / 2, width: w, height: NODE_H, rx: 17,
        class: 'topo-node' + (state.lastActive[p.name] && Date.now() - state.lastActive[p.name] < ACTIVE_DECAY_MS ? ' busy' : '')
      });
      g.appendChild(box);
      g.appendChild(svgEl('circle', { cx: -w / 2 + 15, cy: 0, r: 4, class: 'dot ' + dotClass(p) }));
      var t = svgEl('text', { x: 4, y: 4, class: 'topo-label' });
      var max = Math.floor((w - 32) / 7.6); // label budget inside the pill
      var label = p.name.length > max ? p.name.slice(0, max - 1) + '…' : p.name;
      t.textContent = label + (p.channel ? ' ⛓' : '');
      g.appendChild(t);
      var c = state.counts[p.name] || 0;
      if (c > 0) {
        var ct = svgEl('text', { x: w / 2 - 12, y: 4, class: 'topo-count' });
        ct.textContent = String(c);
        g.appendChild(ct);
      }
      var title = svgEl('title');
      title.textContent = p.name + (p.channel ? ' (reverse channel)' : '') + ' — ' + c + ' routed';
      g.appendChild(title);
      var link = svgEl('a', { href: '/peers/' + encodeURIComponent(p.name), class: 'topo-node-link' });
      link.appendChild(g);
      fresh.appendChild(link);
    });

    if (!state.peers.length) {
      var et = svgEl('text', { y: 6, class: 'topo-empty' });
      et.textContent = 'No peers connected';
      fresh.appendChild(et);
    }
    // rebuild base content inside the svg; packetsG stays in place as the
    // last child so in-flight animateMotion circles never restart
    while (svg.firstChild && svg.firstChild !== packetsG) svg.removeChild(svg.firstChild);
    svg.insertBefore(fresh, packetsG);
  }

  // --- flow animation: a packet traveling src→gate→dst (then back) ---
  // SMIL is not affected by CSS prefers-reduced-motion; gate it here.
  var reduceMotion = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  function spawnPacket(pathD, delayMs, ok) {
    if (reduceMotion) return;
    var circle = svgEl('circle', { r: 4, class: 'flow-packet' + (ok ? '' : ' flow-err') });
    var anim = svgEl('animateMotion', {
      dur: FLOW_STEP_MS + 'ms', begin: delayMs + 'ms', fill: 'freeze', path: pathD
    });
    circle.appendChild(anim);
    packetsG.appendChild(circle);
    setTimeout(function () { circle.remove(); }, delayMs + FLOW_STEP_MS + 120);
  }

  function animateFlow(src, dst, ok) {
    if (!svg || !packetsG) return;
    var toGate = src && edgePaths[src];
    var fromGate = dst && edgePaths[dst];
    // src node → gateway (reverse of the stored path)
    if (toGate) {
      var rev = reversePath(toGate);
      spawnPacket(rev, 0, ok);
    }
    // gateway → dst
    if (fromGate) {
      spawnPacket(fromGate, toGate ? FLOW_STEP_MS : 0, ok);
    }
  }

  function reversePath(d) {
    // 'M gx,gy Q mx,my nx,ny' → 'M nx,ny Q mx,my gx,gy'
    var m = d.match(/^M ([\d.-]+),([\d.-]+) Q ([\d.-]+),([\d.-]+) ([\d.-]+),([\d.-]+)$/);
    if (!m) return d;
    return 'M ' + m[5] + ',' + m[6] + ' Q ' + m[3] + ',' + m[4] + ' ' + m[1] + ',' + m[2];
  }

  // --- communication log ---
  function pad2(n) { return (n < 10 ? '0' : '') + n; }
  function fmtTime(ts) {
    var d = new Date(ts * 1000);
    return pad2(d.getHours()) + ':' + pad2(d.getMinutes()) + ':' + pad2(d.getSeconds());
  }
  function esc(s) {
    var d = document.createElement('div');
    d.textContent = String(s);
    return d.innerHTML;
  }
  function logRow(e) {
    if (!log) return;
    var li = document.createElement('li');
    li.className = 'flow-row';
    li.dataset.http = e.method;
    if (e.rpc_method) li.dataset.method = e.rpc_method;
    if (e.rpc_id) li.dataset.rpcId = e.rpc_id;
    if (e.preview) li.dataset.preview = e.preview;
    var btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'flow-line';
    btn.setAttribute('aria-expanded', 'false');
    btn.innerHTML =
      '<span class="flow-time">' + fmtTime(e.ts) + '</span>' +
      '<span class="flow-src">' + esc(e.src) + '</span>' +
      '<span class="flow-arrow">→</span>' +
      '<span class="flow-dst">' + esc(e.dst) + '</span>' +
      '<span class="flow-method">' + esc(e.rpc_method || e.method) + '</span>' +
      '<span class="flow-status' + (e.status >= 400 ? ' bad' : '') + '">' + e.status + '</span>' +
      '<span class="flow-ms">' + e.latency_ms + 'ms</span>';
    var det = document.createElement('div');
    det.className = 'flow-detail';
    det.hidden = true;
    li.appendChild(btn);
    li.appendChild(det);
    log.prepend(li);
    while (log.children.length > 100) log.lastChild.remove();
  }

  // click-to-expand audited detail on any flow row (live + server-rendered)
  if (log) {
    log.addEventListener('click', function (ev) {
      var btn = ev.target.closest('.flow-line');
      if (!btn) return;
      var li = btn.parentElement;
      var det = li.querySelector('.flow-detail');
      if (!det) return;
      if (!det.textContent) {
        var lines = ['RPC: ' + (li.dataset.method || '') + ' (HTTP ' + (li.dataset.http || '') + ')'];
        if (li.dataset.rpcId) lines.push('id: ' + li.dataset.rpcId);
        lines.push('request preview: ' + (li.dataset.preview || '—'));
        det.textContent = lines.join('\n');
      }
      det.hidden = !det.hidden;
      btn.setAttribute('aria-expanded', det.hidden ? 'false' : 'true');
    });
  }

  function onRoute(e) {
    var m = JSON.parse(e.data);
    if (!m || !m.dst) return;
    var ok = m.status < 400;
    // live windowed counters (covers server render gap; a fresh event is
    // always inside the server's 1h window)
    bumpRouted();
    if (ok) { /* errors unchanged */ } else bumpErrors();
    // attribution: strip channel-/client- prefixes when they match a known peer
    var srcName = normalizeCaller(m.src);
    var knownSrc = state.peers.some(function (p) { return p.name === srcName; });
    if (srcName && knownSrc) {
      state.lastActive[srcName] = Date.now();
      if (ok) delete state.errs[srcName]; else state.errs[srcName] = true;
      state.counts[srcName] = (state.counts[srcName] || 0) + 1;
    }
    if (m.dst) {
      state.lastActive[m.dst] = Date.now();
      if (ok) delete state.errs[m.dst]; else state.errs[m.dst] = true;
      state.counts[m.dst] = (state.counts[m.dst] || 0) + 1;
    }
    // unknown dst (registered after last render) → refresh peer set, then animate
    if (!edgePaths[m.dst]) {
      refreshPeers(function () { animateFlow(knownSrc ? srcName : null, m.dst, ok); });
    } else {
      animateFlow(knownSrc ? srcName : null, m.dst, ok);
    }
    logRow(m);
    render();
  }

  // client-<fp8> / channel-client-<fp8> / bootstrap / plain names
  function normalizeCaller(src) {
    if (!src) return '';
    var s = String(src);
    // if a caller name is self-declared (no prefix), keep it
    if (state.peers.some(function (p) { return p.name === s; })) return s;
    return '';
  }

  // lazy live counters — windowed KPIs render server-side; SSE events only
  // increment (never resync from /api/topology: its total_routes is the
  // all-time ring length and would clobber the 1h window)
  function statNum(id) { return document.getElementById(id); }
  function bumpRouted() {
    var el = statNum('stat-routed');
    if (el) el.textContent = String((parseInt(el.textContent, 10) || 0) + 1);
  }
  function bumpErrors() {
    var el = statNum('stat-errors');
    if (el) el.textContent = String((parseInt(el.textContent, 10) || 0) + 1);
    var card = el && el.closest('.stat');
    if (card && parseInt(el.textContent, 10) > 0) card.classList.add('stat-bad');
  }

  function refreshPeers(cb) {
    fetch('/api/topology').then(function (r) { return r.json(); }).then(function (d) {
      var peers = d.peers || [];
      var key = peers.map(function (p) { return p.name + ':' + p.state + ':' + !!p.healthy; }).sort().join('|');
      var oldKey = state.peers.map(function (p) { return p.name + ':' + p.state + ':' + !!p.healthy; }).sort().join('|');
      state.peers = peers;
      if (key !== oldKey) render();
      if (cb) cb();
    }).catch(function () { if (cb) cb(); });
  }

  function schedule() {
    if (state.timer) return;
    state.timer = setInterval(function () {
      var changed = false;
      for (var k in state.lastActive) {
        if (Date.now() - state.lastActive[k] >= ACTIVE_DECAY_MS) { delete state.lastActive[k]; changed = true; }
      }
      if (changed) render();
    }, 2000);
  }

  function boot() {
    state.peers = JSON.parse(el.dataset.peers || '[]');
    render();
    // format server-rendered log rows (raw epoch ts) like live rows
    if (log) {
      log.querySelectorAll('.flow-time').forEach(function (t) {
        var ts = parseInt(t.textContent, 10);
        if (!isNaN(ts) && ts > 1000000000) t.textContent = fmtTime(ts);
      });
    }
    // live routing events
    var es = new EventSource('/api/events');
    es.addEventListener('route', onRoute);

    // peer-set refresh
    setInterval(function () { refreshPeers(); }, 10000);
    schedule();
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot);
  else boot();
})();
