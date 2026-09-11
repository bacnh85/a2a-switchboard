// Live fragment swaps for [data-live] regions (peers, logs, peer detail).
// SSE events named in data-live-on and the optional data-live-interval
// backstop poll trigger a debounced fetch of data-live-src; identical HTML
// is a no-op. Fragments are server-rendered askama partials (same trust as
// the page itself) — see templates/_*.html.
// Also localizes timestamps: the server renders UTC text (no-JS fallback);
// .ts[data-ts] becomes local time-of-day (log rows) and .ts-rel[data-ts]
// becomes a relative stamp ("2m ago") once older than a day turns local.
(function () {
  var DAY = 86400;
  function two(n) { return (n < 10 ? '0' : '') + n; }
  function rel(ts) {
    var d = Math.max(0, Math.floor(Date.now() / 1000) - ts);
    if (d < 60) return 'just now';
    if (d < 3600) return Math.floor(d / 60) + 'm ago';
    if (d < DAY) return Math.floor(d / 3600) + 'h ago';
    return null;
  }
  function paint(root) {
    (root || document).querySelectorAll('.ts[data-ts],.ts-rel[data-ts]').forEach(function (el) {
      var ts = parseInt(el.dataset.ts, 10);
      if (!ts) return;
      if (!el.dataset.utc) el.dataset.utc = el.textContent; // server-rendered UTC
      var dt = new Date(ts * 1000);
      var date = dt.getFullYear() + '-' + two(dt.getMonth() + 1) + '-' + two(dt.getDate());
      var time = two(dt.getHours()) + ':' + two(dt.getMinutes()) + ':' + two(dt.getSeconds());
      el.title = date + ' ' + time + ' (local) · ' + el.dataset.utc + ' UTC';
      if (el.classList.contains('ts-rel')) {
        var r = rel(ts);
        el.textContent = r ? r : date + ' ' + time;
      } else {
        el.textContent = time; // audit rows: time-of-day; full date stays in the title
      }
    });
  }
  document.addEventListener('live:swap', function (e) {
    if (e.detail && e.detail.querySelectorAll) paint(e.detail);
  });
  paint();
  setInterval(function () { paint(); }, 60000);

  var regions = [].slice.call(document.querySelectorAll('[data-live]'));
  if (!regions.length) return;
  var es = new EventSource('/api/events');
  var timers = {};

  function schedule(el) {
    // Never yank focus from a control inside the region mid-interaction,
    // and never collapse user-opened state (raw-card <details>, expanded
    // log rows) — the next event/poll picks the change up instead.
    if (el.contains(document.activeElement)) return;
    if (el.querySelector('details[open], .log-detail:not([hidden])')) return;
    if (timers[el.dataset.liveSrc]) return; // already pending — coalesce
    timers[el.dataset.liveSrc] = setTimeout(function () {
      delete timers[el.dataset.liveSrc];
      fetch(el.dataset.liveSrc)
        .then(function (r) {
          // Session expired: the fetch followed a redirect to /login —
          // never swap login markup into a live region.
          if (!r.ok || r.redirected) throw new Error('skip');
          return r.text();
        })
        .then(function (html) {
          if (html === el.dataset.rendered || html === el.innerHTML) return;
          el.dataset.rendered = html;
          el.innerHTML = html;
          document.dispatchEvent(new CustomEvent('live:swap', { detail: el }));
        })
        .catch(function () { /* transient — next event/poll retries */ });
    }, 800);
  }

  regions.forEach(function (el) {
    (el.dataset.liveOn || '').split(',').forEach(function (name) {
      name = name.trim();
      if (name) es.addEventListener(name, function () { schedule(el); });
    });
    var iv = parseInt(el.dataset.liveInterval, 10);
    if (iv > 0) setInterval(function () { schedule(el); }, iv);
  });
})();
