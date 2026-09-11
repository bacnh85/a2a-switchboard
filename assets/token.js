// Reveal/Copy for masked token rows (.token-row) — used by settings.html
// and peer detail (gateway identity). Clipboard API with execCommand
// fallback for plain-HTTP deployments (no CSP in this app).
(function () {
  function copyText(t, ok, fail) {
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(t).then(ok, fail);
    } else {
      var ta = document.createElement('textarea');
      ta.value = t; ta.style.position = 'fixed'; ta.style.opacity = '0';
      document.body.appendChild(ta); ta.select();
      try { document.execCommand('copy'); ok(); } catch (e) { fail(); }
      ta.remove();
    }
  }
  function init() {
    document.querySelectorAll('.token-row').forEach(function (row) {
      if (row.dataset.tokenInit) return;
      row.dataset.tokenInit = '1';
      var code = row.querySelector('.token'), tok = code.dataset.token, shown = false;
      var reveal = row.querySelector('[data-reveal]'), cp = row.querySelector('[data-copy]');
      if (reveal) reveal.addEventListener('click', function () {
        shown = !shown;
        code.textContent = shown ? tok : '••••••••••••••••';
        reveal.textContent = shown ? 'Hide' : 'Reveal';
      });
      if (cp) cp.addEventListener('click', function () {
        copyText(tok, function () {
          cp.textContent = 'Copied';
          setTimeout(function () { cp.textContent = 'Copy'; }, 1500);
        }, function () { cp.textContent = 'Select & copy manually'; });
      });
    });
  }
  // Re-init after live.js swaps a fragment (idempotent per row).
  document.addEventListener('live:swap', init);
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
