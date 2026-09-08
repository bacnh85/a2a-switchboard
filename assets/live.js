// Live fragment swaps for [data-live] regions (peers, logs, peer detail).
// SSE events named in data-live-on and the optional data-live-interval
// backstop poll trigger a debounced fetch of data-live-src; identical HTML
// is a no-op. Fragments are server-rendered askama partials (same trust as
// the page itself) — see templates/_*.html.
(function () {
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
