// Messenger island for /chat — vanilla JS, no dependencies.
// Server-rendered shell + JSON API (/api/chat/*) + SSE `chat` events.
// All message text is rendered via textContent (never innerHTML).
(function () {
  var $ = function (id) { return document.getElementById(id); };
  var convsEl = $('chat-convs'), msgsEl = $('chat-msgs'), headEl = $('chat-head-id'),
      inputEl = $('chat-input'), asEl = $('chat-as'), composerEl = $('composer'),
      sendBtn = $('send-btn'), emojiBtn = $('emoji-btn'), emojiPop = $('emoji-pop'),
      roomForm = $('room-form'), roomNew = $('room-new'), memberList = $('room-member-list'),
      backBtn = $('chat-back'), cmdPop = $('cmd-pop');

  var state = {
    data: null,          // /api/chat/state payload
    conv: null,          // open conversation id
    as: null,            // selected human identity
    unread: {},          // conv -> count
    lastTs: 0,           // ts of last bubble in the open thread (date seps)
    pending: false,      // send in flight
  };

  // Stable per-node color: same name → same c0..c7 class (client-side only).
  function colorOf(name) {
    var h = 0;
    for (var i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
    return 'c' + (h % 8);
  }

  function avatar(name, extra) {
    var a = document.createElement('span');
    a.className = 'avatar ' + colorOf(name) + (extra ? ' ' + extra : '');
    a.textContent = (name || '?').charAt(0).toUpperCase();
    return a;
  }

  function humans() { return (state.data && state.data.humans) || []; }
  function peerNames() {
    return state.data ? state.data.peers.map(function (p) { return p.name; }) : [];
  }

  // ----- sidebar -----

  function conversationMeta(id) {
    // Returns {kind, title, members} for a conv id, synthesizing DMs with
    // any known peer/gateway even when no messages exist yet.
    if (id.indexOf('room:') === 0) {
      var r = (state.data.rooms || []).filter(function (x) { return 'room:' + x.id === id; })[0];
      if (r) return { kind: 'room', title: r.name, members: r.members };
      return { kind: 'room', title: id, members: [] };
    }
    var parts = id.replace(/^dm:/, '').split('|');
    return { kind: 'dm', title: parts.join(' ⇄ '), members: parts };
  }

  function dmId(a, b) { return a <= b ? 'dm:' + a + '|' + b : 'dm:' + b + '|' + a; }

  function renderSidebar() {
    convsEl.textContent = '';
    var data = state.data;
    if (!data) return;
    var lastByConv = {};
    (data.conversations || []).forEach(function (c) { lastByConv[c.id] = c.last; });

    function section(label) {
      var h = document.createElement('div');
      h.className = 'chat-section';
      h.textContent = label;
      convsEl.appendChild(h);
    }
    function item(id) {
      var meta = conversationMeta(id);
      var last = lastByConv[id];
      var b = document.createElement('button');
      b.type = 'button';
      b.className = 'conv-item' + (id === state.conv ? ' open' : '');
      var main = avatar(meta.kind === 'room' ? meta.title : (meta.members.filter(function (m) { return m !== state.as; })[0] || meta.title));
      b.appendChild(main);
      var body = document.createElement('span');
      body.className = 'conv-body';
      var t = document.createElement('span');
      t.className = 'conv-title';
      t.textContent = meta.title;
      body.appendChild(t);
      var p = document.createElement('span');
      p.className = 'conv-preview';
      if (last) {
        p.textContent = (last.src === 'system' ? '' : last.src + ': ') + last.text;
      } else if (meta.kind === 'dm') {
        p.textContent = 'start chatting';
      }
      body.appendChild(p);
      b.appendChild(body);
      if (state.unread[id]) {
        var badge = document.createElement('span');
        badge.className = 'unread';
        badge.textContent = state.unread[id] > 9 ? '9+' : state.unread[id];
        b.appendChild(badge);
      }
      b.addEventListener('click', function () { openConv(id); });
      convsEl.appendChild(b);
    }

    section('Rooms');
    (data.rooms || []).forEach(function (r) { item('room:' + r.id); });
    if (!(data.rooms || []).length) {
      var none = document.createElement('div');
      none.className = 'muted chat-empty';
      none.textContent = 'No rooms yet.';
      convsEl.appendChild(none);
    }
    section('Direct');
    var dmConvs = (data.conversations || []).filter(function (c) { return c.kind === 'dm'; })
      .map(function (c) { return c.id; });
    dmConvs.forEach(item);
    // Offer DMs with everyone known (gateway, peers) that lack a thread yet.
    var known = ['gateway'].concat(peerNames());
    known.forEach(function (n) {
      var id = dmId(state.as || '_', n);
      if (dmConvs.indexOf(id) === -1) item(id);
    });
  }

  function renderRoomForm() {
    memberList.textContent = '';
    peerNames().concat(humans()).forEach(function (n) {
      var l = document.createElement('label');
      l.className = 'chk';
      var cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.value = n;
      l.appendChild(cb);
      l.appendChild(document.createTextNode(n));
      memberList.appendChild(l);
    });
  }

  // ----- thread -----

  function clearThread() {
    msgsEl.textContent = '';
    while (msgsEl.firstChild) msgsEl.removeChild(msgsEl.firstChild);
    state.lastTs = 0;
    stopTyping();
  }

  // ----- markdown rendering (zero-dep, XSS-safe subset) -----
  // Escape EVERYTHING first; the only tags in the output are the ones the
  // transforms below emit. Agents reply in markdown — peers' replies render
  // with code, bold/italic, links, lists, headings, quotes.
  function esc(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;')
            .replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  function renderMd(src) {
    var codes = [];
    var t = src.replace(/```[a-zA-Z0-9_-]*\n?([\s\S]*?)```/g, function (_, c) {
      codes.push(c); return '\u0000C' + (codes.length - 1) + '\u0000';
    });
    function inline(s) {
      s = esc(s);
      s = s.replace(/`([^`\n]+)`/g, '<code>$1</code>');
      s = s.replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>');
      s = s.replace(/\*([^*\n]+)\*/g, '<i>$1</i>');
      s = s.replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g,
                    '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>');
      return s;
    }
    var out = [], list = null, items = [], m;
    // Emit each list as one block string — join('\n') + pre-wrap would
    // otherwise render a visible gap for the newline between list tags.
    function flushList() {
      if (list) {
        out.push('<' + list + '>' + items.map(function (x) {
          return '<li>' + x + '</li>';
        }).join('') + '</' + list + '>');
        list = null; items = [];
      }
    }
    var lines = t.split('\n');
    for (var i = 0; i < lines.length; i++) {
      var raw = lines[i];
      if ((m = raw.match(/^\u0000C(\d+)\u0000\s*$/))) {
        flushList();
        // A peer reply may literally contain our placeholder — fall back to
        // the escaped line when there is no matching fenced block.
        var code = codes[+m[1]];
        out.push('<pre><code>' + (code !== undefined
          ? esc(code.replace(/\n$/, ''))
          : esc(raw)) + '</code></pre>');
      } else if ((m = raw.match(/^\s*[-*]\s+(.*)/))) {
        if (list !== 'ul') { flushList(); list = 'ul'; }
        items.push(inline(m[1]));
      } else if ((m = raw.match(/^\s*\d+[.)]\s+(.*)/))) {
        if (list !== 'ol') { flushList(); list = 'ol'; }
        items.push(inline(m[1]));
      } else if ((m = raw.match(/^#{1,6}\s+(.*)/))) {
        flushList();
        out.push('<b>' + inline(m[1]) + '</b>');
      } else if ((m = raw.match(/^>\s?(.*)/))) {
        flushList();
        out.push('<blockquote>' + inline(m[1]) + '</blockquote>');
      } else {
        flushList();
        out.push(inline(raw));
      }
    }
    flushList();
    return out.join('\n');
  }

  function msgNode(m) {
    var li = document.createElement('li');
    // id 0 is legal (first message after a server restart) — only null is
    // "no id" (optimistic bubbles). The SSE/POST dedupe keys off data-mid.
    if (m.id != null) li.dataset.mid = m.id;
    if (m.kind === 'system') {
      li.className = 'msg-system' + (m.status === 'err' ? ' err' : '');
      li.textContent = m.text;
      return li;
    }
    var own = m.src === state.as;
    li.className = 'msg' + (own ? ' own' : '');
    var bubble = document.createElement('div');
    bubble.className = 'bubble';
    var head = document.createElement('span');
    head.className = 'msg-head ' + colorOf(m.src);
    head.textContent = m.src;
    var meta = document.createElement('span');
    meta.className = 'msg-meta' +
      (own ? (m.status === 'err' ? ' tick-err' : m.status === 'ok' ? ' tick-read' : '') : '');
    meta.appendChild(document.createTextNode(fmtTime(m.ts)));
    if (own && m.status !== 'err') {
      meta.appendChild(tickSvg(m.status !== 'sent'));
    } else if (own) {
      meta.appendChild(document.createTextNode(' ✗'));
    }
    if (m.status === 'err') meta.title = m.error || 'delivery failed';
    bubble.appendChild(head);
    // Peers reply in markdown — render the safe subset (esc-first pipeline).
    // Text first, meta last: the float sits at the end of the last line.
    bubble.insertAdjacentHTML('beforeend', renderMd(m.text));
    bubble.appendChild(meta);
    li.appendChild(bubble);
    return li;
  }

  // Telegram-style delivery tick: an SVG whose two checks overlap closely
  // (text '✓✓' renders with a wide glyph gap). Inherits the meta tick color.
  function tickSvg(double) {
    var ns = 'http://www.w3.org/2000/svg';
    var svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('viewBox', double ? '0 0 18 11' : '0 0 12 11');
    svg.setAttribute('class', 'tick-svg');
    svg.setAttribute('aria-hidden', 'true');
    var ds = ['M1.5 5.8 L4.6 9 L10.6 1.8'];
    if (double) ds.push('M7 5.8 L10.1 9 L16.1 1.8');
    ds.forEach(function (d) {
      var p = document.createElementNS(ns, 'path');
      p.setAttribute('d', d);
      p.setAttribute('fill', 'none');
      p.setAttribute('stroke', 'currentColor');
      p.setAttribute('stroke-width', '1.7');
      p.setAttribute('stroke-linecap', 'round');
      p.setAttribute('stroke-linejoin', 'round');
      svg.appendChild(p);
    });
    return svg;
  }

  // ----- typing indicator -----
  // Client-side "peer is thinking" bubble: shown from send until the reply
  // lands (POST response or SSE), matching the messenger perception. The
  // send request blocks server-side while the agent peer works.
  var typingEl = null, typingTimer = 0;

  function typingPeer(conv) {
    // Name to show dots for — only when an agent peer is on the hook.
    var meta = conversationMeta(conv);
    var isHuman = function (n) { return humans().indexOf(n) !== -1; };
    if (meta.kind === 'dm') {
      var other = meta.members.filter(function (m) { return m !== state.as; })[0];
      return other && !isHuman(other) ? other : null;
    }
    return meta.members.some(function (m) { return !isHuman(m); }) ? meta.title : null;
  }

  function startTyping(conv) {
    if (typingEl || !typingPeer(conv)) return;
    typingEl = document.createElement('li');
    typingEl.className = 'msg msg-typing';
    typingEl.setAttribute('aria-hidden', 'true');
    var bub = document.createElement('div');
    bub.className = 'bubble typing-bubble';
    for (var i = 0; i < 3; i++) {
      var d = document.createElement('span');
      d.className = 'dot';
      bub.appendChild(d);
    }
    typingEl.appendChild(bub);
    msgsEl.appendChild(typingEl);
    scrollBottom();
    typingTimer = setTimeout(stopTyping, 65000); // server fanout cap is 60s
  }

  function stopTyping() {
    clearTimeout(typingTimer);
    typingTimer = 0;
    if (typingEl) { typingEl.remove(); typingEl = null; }
  }

  function keepTypingLast() { if (typingEl) msgsEl.appendChild(typingEl); }

  function fmtTime(ts) {
    if (!ts) return '';
    var d = new Date(ts * 1000);
    function p(n) { return n < 10 ? '0' + n : n; }
    return p(d.getHours()) + ':' + p(d.getMinutes());
  }

  function nearBottom() {
    return msgsEl.scrollHeight - msgsEl.scrollTop - msgsEl.clientHeight < 80;
  }
  function scrollBottom() { msgsEl.scrollTop = msgsEl.scrollHeight; }

  function renderMessages(list) {
    var stick = nearBottom();
    list.forEach(function (m) {
      // SSE may have already landed this bubble (broadcast precedes the send
      // response) — dedupe by id and drop the matching optimistic bubble.
      if (m.id != null && msgsEl.querySelector('[data-mid="' + m.id + '"]')) {
        dropTempOwn(m.text);
        return;
      }
      if (m.src === state.as) dropTempOwn(m.text);
      maybeDateSep(m.ts);
      msgsEl.appendChild(msgNode(m));
      if (m.id > state.lastId) state.lastId = m.id;
    });
    keepTypingLast();
    if (stick) scrollBottom();
  }

  // Remove the optimistic (✓) bubble once its real server-recorded bubble
  // exists (matched by the raw text stored on the temp node).
  function dropTempOwn(text) {
    var t = msgsEl.querySelector('li[data-tmp]');
    if (t && t.dataset.text === text) t.remove();
  }

  // Telegram-style day separator between messages from different dates.
  function dateLabel(ts) {
    var d = new Date(ts * 1000), now = new Date();
    function day(x) { return x.getFullYear() + '-' + x.getMonth() + '-' + x.getDate(); }
    if (day(d) === day(now)) return 'Today';
    if (day(d) === day(new Date(now.getTime() - 86400000))) return 'Yesterday';
    function p(n) { return n < 10 ? '0' + n : n; }
    return d.getFullYear() + '-' + p(d.getMonth() + 1) + '-' + p(d.getDate());
  }
  function maybeDateSep(ts) {
    if (state.lastTs && dateLabel(ts) !== dateLabel(state.lastTs)) {
      var li = document.createElement('li');
      li.className = 'msg-date';
      li.textContent = dateLabel(ts);
      msgsEl.appendChild(li);
    }
    state.lastTs = ts;
  }

  function openConv(id) {
    state.conv = id;
    state.unread[id] = 0;
    clearThread();
    renderCmdMenu();
    var meta = conversationMeta(id);
    headEl.textContent = '';
    var t = document.createElement('strong');
    t.textContent = meta.title;
    headEl.appendChild(avatar(meta.kind === 'room' ? meta.title : (meta.members[0] || '?')));
    headEl.appendChild(t);
    var sub = document.createElement('span');
    sub.className = 'muted chat-head-sub';
    sub.textContent = meta.kind === 'room'
      ? 'room · ' + (meta.members.join(', ') || 'no members')
      : 'direct';
    headEl.appendChild(sub);
    if (window.innerWidth <= 880) {
      document.getElementById('chat').classList.add('thread-open');
      backBtn.hidden = false;
    }
    fetch('/api/chat/messages?conv=' + encodeURIComponent(id))
      .then(function (r) { if (!r.ok) throw 0; return r.json(); })
      .then(function (j) {
        if (state.conv !== id) return; // stale: user switched conversations
        clearThread();
        renderMessages(j.messages || []);
        scrollBottom();
      })
      .catch(function () {});
    renderSidebar();
    inputEl.focus();
  }

  backBtn.addEventListener('click', function () {
    document.getElementById('chat').classList.remove('thread-open');
    backBtn.hidden = true;
  });

  // ----- sending -----

  composerEl.addEventListener('submit', function (e) {
    e.preventDefault();
    if (!state.conv) return;
    var text = inputEl.value.trim();
    if (!text) return;
    // Optimistic bubble: ✓ as soon as it's displayed; the send response
    // upgrades it to ✓✓ (reached the peer) or ✗ (failed).
    var tmp = msgNode({ id: null, ts: Math.floor(Date.now() / 1000), conv: state.conv,
                        src: state.as, text: text, kind: 'chat', status: 'sent' });
    tmp.dataset.tmp = '1';
    tmp.dataset.text = text;
    msgsEl.appendChild(tmp);
    scrollBottom();
    startTyping(state.conv);
    inputEl.value = '';
    autoGrow();
    renderCmdMenu();
    fetch('/api/chat/send', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ conv: state.conv, as: state.as, text: text }),
    })
      .then(function (r) { if (!r.ok) return r.json().then(function (j) { throw (j.error || 'send failed'); }); return r.json(); })
      .then(function (j) {
        stopTyping();
        tmp.remove();
        renderMessages(j.messages || []);
        scrollBottom();
      })
      .catch(function (err) {
        stopTyping();
        tmp.remove();
        var fail = msgNode({ id: null, ts: Math.floor(Date.now() / 1000), conv: state.conv,
                             src: state.as, text: text, kind: 'chat', status: 'err',
                             error: typeof err === 'string' ? err : 'send failed' });
        msgsEl.appendChild(fail);
        scrollBottom();
      });
  });

  inputEl.addEventListener('keydown', function (e) {
    var open = !cmdPop.hidden;
    if (open && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
      e.preventDefault();
      var n = cmdPop.children.length;
      cmdIdx = (cmdIdx + (e.key === 'ArrowDown' ? 1 : n - 1)) % n;
      for (var i = 0; i < n; i++) {
        cmdPop.children[i].classList.toggle('active', i === cmdIdx);
      }
      return;
    }
    // Tab (or click) completes; Enter keeps sending exactly what's typed.
    if (open && e.key === 'Tab' && cmdIdx >= 0) {
      e.preventDefault();
      completeCmd(cmdPop.children[cmdIdx].querySelector('.cmd-name').textContent);
      return;
    }
    if (open && e.key === 'Escape') {
      cmdPop.hidden = true;
      cmdIdx = -1;
      return;
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      composerEl.requestSubmit();
    }
  });
  function autoGrow() {
    inputEl.style.height = 'auto';
    inputEl.style.height = Math.min(inputEl.scrollHeight, 140) + 'px';
  }
  inputEl.addEventListener('input', function () {
    autoGrow();
    renderCmdMenu();
  });

  // ----- gateway slash-command hints -----
  // Client-side copy of gateway_agent's command table (src/chat.rs
  // command_reply) — shown only while typing a /command in a gateway DM.
  var CMDS = [
    ['/help', 'show the command list'],
    ['/peers', 'fleet status'],
    ['/rooms', 'chat rooms'],
    ['/whoami', 'your identity'],
  ];
  var cmdIdx = -1;

  function isGatewayDm() {
    if (!state.conv) return false;
    var meta = conversationMeta(state.conv);
    return meta.kind === 'dm' && meta.members.indexOf('gateway') !== -1;
  }

  function renderCmdMenu() {
    var v = inputEl.value;
    var show = isGatewayDm() && v.charAt(0) === '/' && v.indexOf(' ') === -1;
    var items = show
      ? CMDS.filter(function (c) { return c[0].indexOf(v.toLowerCase()) === 0; })
      : [];
    if (!items.length) {
      cmdPop.hidden = true;
      cmdIdx = -1;
      return;
    }
    cmdPop.textContent = '';
    items.forEach(function (c, i) {
      var b = document.createElement('button');
      b.type = 'button';
      b.className = 'cmd-item' + (i === 0 ? ' active' : '');
      var n = document.createElement('span');
      n.className = 'cmd-name';
      n.textContent = c[0];
      var d = document.createElement('span');
      d.className = 'cmd-desc';
      d.textContent = c[1];
      b.appendChild(n);
      b.appendChild(d);
      // mousedown prevented so the input keeps focus/caret.
      b.addEventListener('mousedown', function (e) { e.preventDefault(); });
      b.addEventListener('click', function () { completeCmd(c[0]); });
      cmdPop.appendChild(b);
    });
    cmdIdx = 0;
    cmdPop.hidden = false;
  }

  function completeCmd(cmd) {
    inputEl.value = cmd + ' ';
    cmdPop.hidden = true;
    cmdIdx = -1;
    inputEl.focus();
  }

  // ----- emoji picker (native unicode, no dependency) -----

  var EMOJI = ['😀','😁','😂','🤣','😊','😍','🤔','😎','🥳','😴','😭','😡',
               '👍','👎','👋','🤝','🙏','💪','👏','🫡','🤖','🧠','👀','🔥',
               '❤️','✅','❌','⚠️','💡','📌','🚀','🎉','🕸️','🛰️','📡','🔧',
               '🟢','🔴','🟡','🤷','🙋','🐛','☕','🍕','🎮','💾','📎','⏰'];
  EMOJI.forEach(function (e) {
    var b = document.createElement('button');
    b.type = 'button';
    b.className = 'emoji';
    b.textContent = e;
    b.addEventListener('click', function () {
      var s = inputEl.selectionStart == null ? inputEl.value.length : inputEl.selectionStart;
      var end = inputEl.selectionEnd == null ? s : inputEl.selectionEnd;
      inputEl.value = inputEl.value.slice(0, s) + e + inputEl.value.slice(end);
      inputEl.selectionStart = inputEl.selectionEnd = s + e.length;
      inputEl.focus();
      autoGrow();
    });
    emojiPop.appendChild(b);
  });
  emojiBtn.addEventListener('click', function (e) {
    e.stopPropagation();
    emojiPop.hidden = !emojiPop.hidden;
    emojiBtn.setAttribute('aria-expanded', emojiPop.hidden ? 'false' : 'true');
  });
  document.addEventListener('click', function (e) {
    if (!emojiPop.hidden && !emojiPop.contains(e.target)) {
      emojiPop.hidden = true;
      emojiBtn.setAttribute('aria-expanded', 'false');
    }
    if (!cmdPop.hidden && !cmdPop.contains(e.target) && e.target !== inputEl) {
      cmdPop.hidden = true;
      cmdIdx = -1;
    }
    if (roomNew.open && !roomNew.contains(e.target)) {
      roomNew.removeAttribute('open');
    }
  });
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape' && !emojiPop.hidden) {
      emojiPop.hidden = true;
      emojiBtn.setAttribute('aria-expanded', 'false');
    }
    if (e.key === 'Escape' && roomNew.open) {
      roomNew.removeAttribute('open');
    }
  });

  // ----- rooms -----

  roomForm.addEventListener('submit', function (e) {
    e.preventDefault();
    var name = $('room-name').value.trim();
    var members = [].slice.call(memberList.querySelectorAll('input:checked'))
      .map(function (cb) { return cb.value; });
    if (!name) return;
    fetch('/api/chat/rooms', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: name, members: members, as: state.as }),
    })
      .then(function (r) { if (!r.ok) return r.json().then(function (j) { throw (j.error || 'create failed'); }); return r.json(); })
      .then(function (j) {
        roomNew.removeAttribute('open');
        $('room-name').value = '';
        return loadState().then(function () {
          openConv('room:' + j.room.id);
        });
      })
      .catch(function (err) {
        headEl.textContent = '';
        var s = document.createElement('span');
        s.className = 'muted';
        s.textContent = typeof err === 'string' ? err : 'room creation failed';
        headEl.appendChild(s);
      });
  });

  // ----- identity -----

  asEl.addEventListener('change', function () {
    state.as = asEl.value || null;
    renderSidebar();
    if (state.conv) openConv(state.conv);
  });

  function renderIdentity() {
    asEl.textContent = '';
    humans().forEach(function (n) {
      var o = document.createElement('option');
      o.value = n;
      o.textContent = n;
      asEl.appendChild(o);
    });
    if (!humans().length) {
      var o = document.createElement('option');
      o.value = '';
      o.textContent = 'no human identity';
      asEl.appendChild(o);
      var hint = document.createElement('a');
      hint.href = '/settings';
      hint.textContent = 'create one in Settings';
      hint.className = 'muted';
      asEl.replaceWith(hint);
    } else {
      state.as = asEl.value;
    }
  }

  // ----- live updates (SSE) -----

  function onChatEvent(e) {
    var m;
    try { m = JSON.parse(e.data); } catch (_) { return; }
    if (!state.data) return;
    // Room created in another session? conversationMeta maps every room: id
    // to kind 'room', so it cannot discriminate — check the roster instead.
    var knownRoom = (state.data.rooms || []).some(function (r) {
      return 'room:' + r.id === m.conv;
    });
    if (m.conv === state.conv) {
      if (m.src === state.as) dropTempOwn(m.text);
      else stopTyping(); // the peer replied (or delivery failed) — dots done
      if (!msgsEl.querySelector('[data-mid="' + m.id + '"]')) {
        maybeDateSep(m.ts);
        msgsEl.appendChild(msgNode(m));
        if (m.id > state.lastId) state.lastId = m.id;
        if (nearBottom()) scrollBottom();
        keepTypingLast();
      }
    } else {
      state.unread[m.conv] = (state.unread[m.conv] || 0) + 1;
    }
    // Room list may have changed (new room) — refresh state quietly.
    if (m.conv.indexOf('room:') === 0 && !knownRoom) loadState().then(renderSidebar);
    else renderSidebar();
  }

  // Backfill bubbles broadcast while the SSE stream was down (reconnect):
  // the server's since_id path returns only messages newer than the last
  // one rendered; renderMessages dedupes against what's already there.
  function backfill() {
    if (!state.conv) return;
    fetch('/api/chat/messages?conv=' + encodeURIComponent(state.conv) +
          '&since_id=' + (state.lastId || 0))
      .then(function (r) { if (!r.ok) throw 0; return r.json(); })
      .then(function (j) {
        renderMessages(j.messages || []);
        if (nearBottom()) scrollBottom();
      })
      .catch(function () {});
  }

  // ----- boot -----

  function loadState() {
    return fetch('/api/chat/state')
      .then(function (r) { if (!r.ok) throw 0; return r.json(); })
      .then(function (j) {
        state.data = j;
        // Gateway restarted with smaller chat ids (seq resets to its stored
        // max): resync so backfill/dedupe don't key off stale ids.
        var lid = j.last_id || 0;
        if (lid < state.lastId) state.lastId = lid;
      });
  }

  loadState()
    .then(function () {
      renderIdentity();
      renderRoomForm();
      renderSidebar();
      var wanted = new URLSearchParams(location.search).get('dm');
      if (wanted) openConv(dmId(state.as || '_', wanted));
      state.lastId = state.data.last_id || 0;
      // static=1: screenshot/smoke rigs skip the never-ending SSE stream.
      if (!new URLSearchParams(location.search).has('static')) {
        var es = new EventSource('/api/events');
        es.addEventListener('chat', onChatEvent);
        es.addEventListener('open', backfill);
        // tiny live indicator in the thread header
        var live = document.createElement('span');
        live.className = 'chat-live';
        live.textContent = 'live';
        live.hidden = true;
        headEl.appendChild(live);
        es.addEventListener('open', function () { live.hidden = false; live.classList.remove('off'); live.textContent = 'live'; });
        es.addEventListener('error', function () { live.hidden = false; live.classList.add('off'); live.textContent = 'reconnecting…'; });
      }
    })
    .catch(function () {
      headEl.textContent = '';
      var s = document.createElement('span');
      s.className = 'banner banner-warn';
      s.setAttribute('role', 'alert');
      s.textContent = 'Could not load chat state — is the gateway still up? Reopen this page to retry.';
      headEl.appendChild(s);
    });
})();
