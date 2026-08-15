// Communication graph renderer: vis-network with data from server-rendered attributes.
// Live-updated by periodic fetch of /api/graph (htmx-agnostic, plain JS here since
// vis needs full dataset swap rather than DOM patching).
(function () {
  var el = document.getElementById('graph');
  if (!el || typeof vis === 'undefined') return;

  function render(nodes, edges) {
    var data = { nodes: new vis.DataSet(nodes), edges: new vis.DataSet(edges) };
    var options = {
      edges: { smooth: { enabled: true, type: 'continuous', roundness: 0.5 } },
      physics: { enabled: true, solver: 'forceAtlas2Based', stabilization: { fit: true } },
      interaction: { hover: true },
    };
    if (window._gwGraph) {
      window._gwGraph.setData(data);
      return;
    }
    window._gwGraph = new vis.Network(el, data, options);
  }

  function boot() {
    render(JSON.parse(el.dataset.nodes || '[]'), JSON.parse(el.dataset.edges || '[]'));
    setInterval(function () {
      fetch('/api/graph')
        .then(function (r) { return r.json(); })
        .then(function (d) { render(d.nodes, d.edges); })
        .catch(function () {});
    }, 5000);
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot);
  else boot();
})();
