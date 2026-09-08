// Shared row-expand for audit tables (logs.html, peer_detail.html).
// A .log-row toggles its next sibling .log-detail, filling it lazily from
// data-* attributes. Works for keyboard (Enter/Space) too.
(function () {
  function init(tbl) {
    if (!tbl || tbl.dataset.expandInit) return;
    tbl.dataset.expandInit = "1";
    tbl.addEventListener('click', function (ev) {
      var row = ev.target.closest('.log-row');
      if (!row) return;
      toggleRow(row);
    });
    tbl.addEventListener('keydown', function (ev) {
      if (ev.key !== 'Enter' && ev.key !== ' ') return;
      var row = ev.target.closest('.log-row');
      if (!row) return;
      ev.preventDefault();
      toggleRow(row);
    });
  }
  function toggleRow(row) {
    var detail = row.nextElementSibling;
    if (!detail || !detail.classList.contains('log-detail')) return;
    var cell = detail.firstElementChild;
    if (!cell.textContent) {
      var lines = ['RPC: ' + (row.dataset.method || '') + ' (HTTP ' + (row.dataset.http || '') + ')'];
      if (row.dataset.rpcId) lines.push('id: ' + row.dataset.rpcId);
      if (row.dataset.taskState) lines.push('task: ' + row.dataset.taskState);
      lines.push('request preview: ' + (row.dataset.preview || '—'));
      lines.push('response preview: ' + (row.dataset.respPreview || '—'));
      cell.textContent = lines.join('\n');
    }
    detail.hidden = !detail.hidden;
    row.setAttribute('aria-expanded', String(!detail.hidden));
    row.classList.toggle('expanded', !detail.hidden);
  }
  function boot() {
    document.querySelectorAll('.tbl').forEach(init);
  }
  // Rows swapped in by live.js need (re-)init; init is idempotent per table.
  document.addEventListener('live:swap', function (e) {
    if (e.detail && e.detail.querySelectorAll) e.detail.querySelectorAll('.tbl').forEach(init);
  });
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();
