// Chillarr inside Jellyfin's web pages: window.__TAURI__ over HTTP.
// Loaded (not as a module) before the app's own scripts, so api.js finds
// invoke and listen where it expects them. Every call goes to Chillarr's
// bridge on the server PC (bridge.rs) and runs as the signed-in Jellyfin
// user. Its port is in bridge.json next to this folder's index.html.
//
// Written as ES2017 on purpose: TVs run browsers as old as Chrome 68 (LG
// webOS 5), where ?. ?? ||= and `catch {` without a name don't even parse.
// No new syntax in here; the app itself is bundled down for them (legacy.js).
(() => {
  /* ---------- What this browser is ---------- */
  const html = document.documentElement;
  // Calls the app makes that old browsers don't have.
  if (!Element.prototype.replaceChildren) {
    const replace = function (...kids) { while (this.lastChild) this.removeChild(this.lastChild); this.append(...kids); };
    Element.prototype.replaceChildren = replace;
    if (window.DocumentFragment) DocumentFragment.prototype.replaceChildren = replace;
  }
  if (!Element.prototype.toggleAttribute) {
    Element.prototype.toggleAttribute = function (name, force) {
      const on = force === undefined ? !this.hasAttribute(name) : !!force;
      if (on) this.setAttribute(name, ""); else this.removeAttribute(name);
      return on;
    };
  }
  if (!Array.prototype.flat) {
    Object.defineProperty(Array.prototype, "flat", { configurable: true, writable: true, value: function (depth) {
      const d = depth === undefined ? 1 : Number(depth);
      return d < 1 ? this.slice() : this.reduce((a, x) => a.concat(Array.isArray(x) ? x.flat(d - 1) : x), []);
    } });
  }
  if (!Array.prototype.flatMap) {
    Object.defineProperty(Array.prototype, "flatMap", { configurable: true, writable: true, value: function (f, that) { return this.map(f, that).flat(1); } });
  }
  /* Chrome 53 (LG webOS 4, 2018) is the oldest seen: these came after it. */
  // append/prepend (ParentNode) and before/after/replaceWith/remove (ChildNode): Chrome 54.
  const nodes = (args) => {
    if (args.length === 1 && typeof args[0] !== "string") return args[0];
    const f = document.createDocumentFragment();
    for (const a of args) f.appendChild(a instanceof Node ? a : document.createTextNode(String(a)));
    return f;
  };
  for (const P of [Element, Document, DocumentFragment].map((C) => C.prototype)) {
    if (!P.append) P.append = function (...a) { this.appendChild(nodes(a)); };
    if (!P.prepend) P.prepend = function (...a) { this.insertBefore(nodes(a), this.firstChild); };
  }
  for (const P of [Element, CharacterData, DocumentType].map((C) => C.prototype)) {
    if (!P.remove) P.remove = function () { if (this.parentNode) this.parentNode.removeChild(this); };
    if (!P.before) P.before = function (...a) { if (this.parentNode) this.parentNode.insertBefore(nodes(a), this); };
    if (!P.after) P.after = function (...a) { if (this.parentNode) this.parentNode.insertBefore(nodes(a), this.nextSibling); };
    if (!P.replaceWith) P.replaceWith = function (...a) { if (this.parentNode) this.parentNode.replaceChild(nodes(a), this); };
  }
  // addEventListener's { once } (Chrome 55): older ones read the object as
  // "capture" and keep the listener forever. Wrapped so it runs once.
  let onceOk = false;
  try {
    const o = Object.defineProperty({}, "once", { get() { onceOk = true; return false; } });
    window.addEventListener("x", null, o); window.removeEventListener("x", null, o);
  } catch (e) { /* no options at all */ }
  if (!onceOk) {
    const add = EventTarget.prototype.addEventListener, remove = EventTarget.prototype.removeEventListener;
    const capture = (o) => (typeof o === "object" && o !== null ? !!o.capture : !!o);
    // Wrappers per target, per type and capture: removing a listener finds
    // its own wrapper, and a listener added without once is left alone.
    const wrappers = new WeakMap();
    const slot = (target, type, cap) => {
      let byKey = wrappers.get(target);
      if (!byKey) { byKey = new Map(); wrappers.set(target, byKey); }
      const k = `${type}|${cap}`;
      if (!byKey.has(k)) byKey.set(k, new Map());
      return byKey.get(k);
    };
    EventTarget.prototype.addEventListener = function (type, fn, o) {
      if (!fn || typeof o !== "object" || o === null || !o.once) return add.call(this, type, fn, capture(o) ? { capture: true, passive: !!(o && o.passive) } : (o && o.passive ? { passive: true } : false));
      const target = this, cap = capture(o), mine = slot(target, type, cap);
      if (mine.has(fn)) return; // already waiting, as the browser would have it
      const once = function (e) { mine.delete(fn); remove.call(target, type, once, cap); return typeof fn === "function" ? fn.call(this, e) : fn.handleEvent(e); };
      mine.set(fn, once);
      return add.call(this, type, once, cap);
    };
    EventTarget.prototype.removeEventListener = function (type, fn, o) {
      const cap = capture(o), byKey = wrappers.get(this), mine = byKey && byKey.get(`${type}|${cap}`);
      const once = mine && fn && mine.get(fn);
      if (once) { mine.delete(fn); remove.call(this, type, once, cap); }
      else remove.call(this, type, fn, cap);
    };
  }
  // Chrome 54: esbuild's code for { ...x } on old targets calls it, so without
  // it every object spread in the app throws and nothing draws.
  if (!Object.getOwnPropertyDescriptors) {
    Object.getOwnPropertyDescriptors = (o) => {
      const out = {};
      for (const k of Object.getOwnPropertyNames(o).concat(Object.getOwnPropertySymbols ? Object.getOwnPropertySymbols(o) : [])) out[k] = Object.getOwnPropertyDescriptor(o, k);
      return out;
    };
  }
  if (!Object.entries) Object.entries = (o) => Object.keys(o).map((k) => [k, o[k]]);
  if (!Object.values) Object.values = (o) => Object.keys(o).map((k) => o[k]);
  if (!Object.fromEntries) Object.fromEntries = (it) => { const o = {}; for (const [k, v] of it) o[k] = v; return o; };
  const pad = (s, n, f, start) => {
    s = String(s); n = n >> 0; f = f === undefined ? " " : String(f);
    if (s.length >= n || !f) return s;
    let p = ""; while (p.length < n - s.length) p += f;
    p = p.slice(0, n - s.length);
    return start ? p + s : s + p;
  };
  if (!String.prototype.padStart) String.prototype.padStart = function (n, f) { return pad(this, n, f, true); };
  if (!String.prototype.padEnd) String.prototype.padEnd = function (n, f) { return pad(this, n, f, false); };
  if (!Promise.prototype.finally) {
    Promise.prototype.finally = function (f) {
      return this.then((v) => Promise.resolve(f()).then(() => v), (e) => Promise.resolve(f()).then(() => { throw e; }));
    };
  }
  // Scrolling with an options object (Chrome 61): window.scrollTo({ top })
  // threw, and scrollIntoView({ block: "nearest" }) only knew true/false.
  // The D-pad leans on "nearest" to keep the focused poster in its row.
  if (!("scrollBehavior" in html.style)) {
    const root = () => document.scrollingElement || html;
    const glide = (el, left, top, smooth) => {
      const x0 = el.scrollLeft, y0 = el.scrollTop;
      if (!smooth) { el.scrollLeft = left; el.scrollTop = top; return; }
      const t0 = performance.now();
      const step = (t) => {
        const k = Math.min(1, (t - t0) / 180), e = 1 - Math.pow(1 - k, 3);
        el.scrollLeft = x0 + (left - x0) * e; el.scrollTop = y0 + (top - y0) * e;
        if (k < 1) requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    };
    const opt = (el, a, b, by) => {
      const o = typeof a === "object" && a !== null ? a : { left: a, top: b };
      const left = o.left === undefined ? (by ? 0 : el.scrollLeft) : Number(o.left) || 0;
      const top = o.top === undefined ? (by ? 0 : el.scrollTop) : Number(o.top) || 0;
      glide(el, by ? el.scrollLeft + left : left, by ? el.scrollTop + top : top, o.behavior === "smooth");
    };
    window.scrollTo = window.scroll = (a, b) => opt(root(), a, b, false);
    window.scrollBy = (a, b) => opt(root(), a, b, true);
    Element.prototype.scrollTo = Element.prototype.scroll = function (a, b) { opt(this, a, b, false); };
    Element.prototype.scrollBy = function (a, b) { opt(this, a, b, true); };
    const native = Element.prototype.scrollIntoView;
    // How far to move a box so `r` sits at `where` inside `box` (start, end, center, nearest).
    const shift = (where, rs, re, bs, be) => {
      if (where === "start") return rs - bs;
      if (where === "end") return re - be;
      if (where === "center") return (rs + re) / 2 - (bs + be) / 2;
      if (rs < bs || re - rs > be - bs) return rs - bs;
      return re > be ? re - be : 0;
    };
    Element.prototype.scrollIntoView = function (o) {
      if (typeof o !== "object" || o === null) return native.call(this, o);
      const block = o.block || "start", inline = o.inline || "nearest", smooth = o.behavior === "smooth";
      // Every box that scrolls, innermost first, then the page.
      for (let p = this.parentElement; p; p = p.parentElement) {
        const cs = getComputedStyle(p);
        const sy = /(auto|scroll)/.test(cs.overflowY) && p.scrollHeight > p.clientHeight;
        const sx = /(auto|scroll)/.test(cs.overflowX) && p.scrollWidth > p.clientWidth;
        const isRoot = p === root() || p === document.body;
        if (!sy && !sx && !isRoot) continue;
        const r = this.getBoundingClientRect();
        const b = isRoot ? { left: 0, top: 0, right: innerWidth, bottom: innerHeight } : p.getBoundingClientRect();
        const el = isRoot ? root() : p;
        const dx = sx || isRoot ? shift(inline, r.left, r.right, b.left, b.right) : 0;
        const dy = sy || isRoot ? shift(block, r.top, r.bottom, b.top, b.bottom) : 0;
        if (dx || dy) glide(el, el.scrollLeft + dx, el.scrollTop + dy, smooth);
        if (isRoot) break;
      }
    };
  }
  // ResizeObserver (Chrome 64): a stand-in fed by the window's own resizes.
  if (!window.ResizeObserver) {
    window.ResizeObserver = class {
      constructor(cb) { this.cb = cb; this.els = new Set(); this.fire = () => this.cb([...this.els].map((target) => ({ target, contentRect: target.getBoundingClientRect() })), this); }
      observe(el) {
        if (!this.els.size) { window.addEventListener("resize", this.fire); window.addEventListener("orientationchange", this.fire); }
        this.els.add(el);
        requestAnimationFrame(() => this.cb([{ target: el, contentRect: el.getBoundingClientRect() }], this));
      }
      unobserve(el) { this.els.delete(el); if (!this.els.size) this.disconnect(); }
      disconnect() { this.els.clear(); window.removeEventListener("resize", this.fire); window.removeEventListener("orientationchange", this.fire); }
    };
  }
  // CSS grid (57) and sticky (56) are missing too: styles.css has fallbacks under these.
  if (!(window.CSS && CSS.supports && CSS.supports("display", "grid"))) html.classList.add("no-grid");
  if (!(window.CSS && CSS.supports && (CSS.supports("position", "sticky") || CSS.supports("position", "-webkit-sticky")))) html.classList.add("no-sticky");
  // A TV (10-foot sizing, remote keys, no blur). The LG and Samsung apps put
  // their NativeShell in only after this runs, so it's the browser's name here;
  // app.js looks at NativeShell again once it's there.
  const ua = navigator.userAgent || "";
  if (/Web0S|webOS|SMART-TV|SmartTV|Tizen|AFT[A-Z]|Android TV|BRAVIA|CrKey|HbbTV/i.test(ua)) html.classList.add("tv");
  // A TV remote's arrows and OK by their codes, as jellyfin-web reads them
  // (keyboardnavigation's keyCode table first): a remote can send only the
  // code, with key empty or "Unidentified". Set before any of the app's
  // listeners see the event, so every one of them can go by e.key. It must
  // stay registered before nav.js and player.js add theirs (this file loads first).
  const KEYS = { 13: "Enter", 27: "Escape", 37: "ArrowLeft", 38: "ArrowUp", 39: "ArrowRight", 40: "ArrowDown" };
  window.addEventListener("keydown", (e) => {
    const k = KEYS[e.keyCode];
    if (k && e.key !== k) try { Object.defineProperty(e, "key", { value: k }); } catch (x) { /* read-only here: left as sent */ }
  }, true);
  // Flexbox gap came in Chrome 84: older ones get margins instead (styles.css, html.no-flexgap).
  (() => {
    const box = document.createElement("div");
    box.style.cssText = "display:flex;flex-direction:column;gap:1px;position:absolute;visibility:hidden";
    box.appendChild(document.createElement("div"));
    box.appendChild(document.createElement("div"));
    html.appendChild(box);
    if (box.scrollHeight !== 1) html.classList.add("no-flexgap");
    html.removeChild(box);
  })();

  /* ---------- Who's signed in ---------- */
  // The Jellyfin login this page came with, as jellyfin-web keeps it.
  const CREDS = "jellyfin_credentials";
  const originOf = (addr) => { try { return new URL(addr).origin; } catch (e) { return ""; } };
  function server() {
    let all = [];
    try { all = JSON.parse(localStorage.getItem(CREDS) || "{}").Servers || []; } catch (e) { /* no login */ }
    const signed = all.filter((s) => s.AccessToken && s.UserId);
    // This server's entry by address. Without one, only a lone login is
    // taken as this server's: a token is never sent to a host it may not be from.
    return signed.find((s) => [s.ManualAddress, s.LocalAddress, s.RemoteAddress].some((a) => a && originOf(a) === location.origin))
      || (signed.length === 1 ? signed[0] : null);
  }
  // jellyfin-web's device id; made the way it makes one when this browser has none yet.
  // Inside Jellyfin's iOS and Android apps, the app's own (it signed this login in with it).
  const APP = window.ExpoAppInfo || null;
  const appId = APP && APP.deviceId;
  function device() {
    // Only an id the bridge accepts; anything else would 401 every call.
    if (/^[A-Za-z0-9+/=_.-]{1,256}$/.test(appId || "")) return appId;
    let id = localStorage.getItem("_deviceId2");
    if (!id) {
      // btoa throws on a user agent outside Latin-1.
      try { id = btoa([navigator.userAgent, Date.now()].join("|")).replace(/=/g, "1"); }
      catch (e) { id = PAGE + Date.now().toString(16); }
      try { localStorage.setItem("_deviceId2", id); } catch (e) { /* storage blocked */ }
    }
    return id;
  }
  // One id per page load: the bridge keeps this page's searches to this page.
  const PAGE = (crypto.randomUUID ? crypto.randomUUID() : Array.from(crypto.getRandomValues(new Uint8Array(8)), (b) => b.toString(16).padStart(2, "0")).join("")).replace(/-/g, "");

  /** Back to jellyfin-web's sign-in. Waits forever, so nothing after it runs on a dead login. */
  let leaving = false;
  function toLogin() {
    if (!leaving) {
      leaving = true;
      const s = server();
      // loader.js won't send this login straight back here: jellyfin-web may
      // still think it's good and go on to Home. A new sign-in clears it.
      try { sessionStorage.setItem("chillarr_bounce", (s && s.AccessToken) || "none"); } catch (e) { /* storage blocked */ }
      location.replace(`../index.html#/login${s && s.Id ? `?serverid=${encodeURIComponent(s.Id)}` : ""}`);
    }
    return new Promise(() => {});
  }

  /* ---------- Calls ---------- */
  // Where the bridge listens. Asked once; asked again if it stops answering
  // (Chillarr restarted and may have taken another port).
  let basePromise = null;
  function base(fresh = false) {
    if (fresh) basePromise = null;
    if (!basePromise) {
      basePromise = fetch("bridge.json", { cache: "no-store" })
        .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
        .then((j) => `${location.protocol}//${location.hostname}:${j.port}`)
        .catch((e) => { basePromise = null; throw e; });
    }
    return basePromise;
  }

  const OFFLINE = "Can't reach Chillarr on the server. Make sure it's running there.";
  // Chillarr answers on plain http, which a browser won't call from an https page.
  const HTTPS = "Chillarr's pages need Jellyfin's http address on your network, like http://your-pc:8096.";

  function send(root, cmd, args, s) {
    return fetch(`${root}/invoke/${encodeURIComponent(cmd)}`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Chillarr-Token": s.AccessToken,
        "X-Chillarr-Device": device(),
        "X-Chillarr-Origin": location.origin,
        "X-Chillarr-Page": PAGE,
      },
      body: JSON.stringify(args == null ? {} : args),
    });
  }

  /** Full screen is the browser's here, not the window's. Answers the new state like the app's command. */
  // iPad Safari only has the webkit names; iPhone Safari and the Jellyfin apps
  // have none (the player already fills the window there).
  const enter = html.requestFullscreen || html.webkitRequestFullscreen;
  const fsNow = () => document.fullscreenElement || document.webkitFullscreenElement;
  if (!enter) html.classList.add("no-fs");
  function fullscreen(on) {
    if (on && !fsNow()) return enter ? Promise.resolve(enter.call(html)).then(() => true, () => false) : Promise.resolve(false);
    if (!on && fsNow()) return Promise.resolve((document.exitFullscreen || document.webkitExitFullscreen).call(document)).then(() => false, () => true);
    return Promise.resolve(!!fsNow());
  }

  async function invoke(cmd, args = {}) {
    // Before any await: the browser only allows full screen straight from a click.
    if (cmd === "window_fullscreen") return fullscreen(!!(args && args.on));
    const s = server();
    if (!s) return toLogin();
    if (location.protocol === "https:") throw HTTPS;
    // No port yet: nothing has been sent, so asking again is safe.
    let root = await base().catch(() => base(true)).catch(() => null);
    if (!root) throw OFFLINE;
    let r;
    try { r = await send(root, cmd, args, s); }
    catch (e) {
      // A failed request may still have run, so it's only sent again when
      // Chillarr has moved to another port (it can't have reached the old one).
      const now = await base(true).catch(() => null);
      if (!now || now === root) throw OFFLINE;
      root = now;
      try { r = await send(root, cmd, args, s); } catch (e2) { throw OFFLINE; }
    }
    if (r.status === 401) return toLogin();
    const body = await r.text();
    let data;
    try { data = body ? JSON.parse(body) : null; }
    catch (e) { throw r.ok ? "Chillarr sent something it couldn't read." : `Chillarr answered ${r.status}.`; }
    if (r.ok) return data;
    // A command's own error comes back as the string Tauri would have rejected with.
    throw (data && data.error) || (r.status === 413 ? "That's too much to send." : `Chillarr answered ${r.status}.`);
  }

  /* ---------- Events ---------- */
  // One stream per page, shared by every listen(). `live` is set the moment
  // the first listen() asks, before any await, and stays set while a stream
  // is open, opening or waiting to retry: every listen() at start-up used to
  // open its own, and the browser's six connections to the bridge went to
  // event streams with none left for calls.
  const handlers = new Map();
  let live = false;
  let stream = null;
  // The phone remote's key for this browser (Settings > Phone remote gives it):
  // with it, the stream also hears the remote's presses meant for this screen.
  const REMOTE_KEY = "chillarr_remote_key";
  // Held here too, for a browser that won't keep it: it works until the page closes.
  let heldKey = "";
  const remoteKey = () => { try { return heldKey || localStorage.getItem(REMOTE_KEY) || ""; } catch (e) { return heldKey; } };
  let closes = 0;
  function ensureStream() {
    if (live) return;
    live = true;
    openStream();
  }
  const again = (ms) => setTimeout(() => { base(true).catch(() => {}); openStream(); }, ms);
  async function openStream() {
    const s = server();
    if (location.protocol === "https:" || !s) { live = false; return; }
    let root;
    try { root = await base(); } catch (e) { again(5000); return; }
    const q = new URLSearchParams({ token: s.AccessToken, device: device(), page: PAGE });
    if (remoteKey()) q.set("remote", remoteKey());
    const es = new EventSource(`${root}/events?${q}`);
    stream = es;
    es.onmessage = (m) => {
      let e;
      try { e = JSON.parse(m.data); } catch (err) { return; }
      for (const cb of handlers.get(e.event) || []) {
        try { cb({ event: e.event, payload: e.payload }); } catch (err) { console.error(err); }
      }
    };
    // The browser reconnects by itself while it can. Closed means it gave up
    // (a refused login, or a new port): look the port up again and start over.
    es.onopen = () => (closes = 0);
    es.onerror = () => {
      if (es.readyState !== EventSource.CLOSED) return;
      es.onerror = null;
      // Twice in a row: maybe the login was refused. A call says so (a 401 goes to the sign-in).
      if (++closes === 2) invoke("web_viewer").catch(() => {});
      again(3000);
    };
  }

  function listen(event, cb) {
    if (!handlers.has(event)) handlers.set(event, new Set());
    handlers.get(event).add(cb);
    ensureStream();
    return Promise.resolve(() => { const set = handlers.get(event); if (set) set.delete(cb); });
  }

  /** A new phone-remote key: kept, and the stream opened again with it. */
  function setRemoteKey(key) {
    if (!key || key === remoteKey()) return;
    heldKey = key;
    try { localStorage.setItem(REMOTE_KEY, key); } catch (e) { /* storage blocked: heldKey does for now */ }
    if (!live || !stream) return;
    stream.onerror = null;
    stream.close();
    openStream();
  }
  window.__CHILLARR_REMOTE__ = { key: remoteKey, setKey: setRemoteKey };

  window.__TAURI__ = { core: { invoke }, event: { listen } };
  window.__CHILLARR_WEB__ = true;

  /* ---------- Settings and profiles ---------- */
  /** This device's key for the main profile, once its password was typed here (per server). */
  function trust() {
    const s = server();
    try { return (s && localStorage.getItem(`chillarr_trust_${s.Id}`)) || null; } catch (e) { return null; }
  }
  window.__CHILLARR_JF__ = {
    server,
    trust,
    /** Ends this Jellyfin session the way jellyfin-web does, then opens its sign-in. */
    async signOut() {
      const s = server();
      if (s) {
        await fetch(`${location.origin}/Sessions/Logout`, { method: "POST", headers: { "X-Emby-Token": s.AccessToken } }).catch(() => {});
        try {
          const c = JSON.parse(localStorage.getItem(CREDS) || "{}");
          for (const x of c.Servers || []) if (x.Id === s.Id) { x.AccessToken = null; x.UserId = null; }
          localStorage.setItem(CREDS, JSON.stringify(c));
        } catch (e) { /* nothing to clear */ }
      }
      toLogin();
    },
    /** Becomes another Chillarr profile: Jellyfin signs this browser in as
     *  them through Quick Connect, which Chillarr approves once the PIN or
     *  password checks out. Then the page starts again as them. */
    async switchProfile(id, pin = null, password = null) {
      const s = server();
      if (!s) return toLogin();
      const version = (APP && APP.appVersion) || await fetch(`${location.origin}/System/Info/Public`).then((r) => r.json()).then((j) => j.Version).catch(() => "12.1.0");
      const q = (v) => String(v).replace(/"/g, "");
      const auth = `MediaBrowser Client="${q((APP && APP.appName) || "Jellyfin Web")}", Device="${q((APP && APP.deviceName) || "Chillarr web")}", DeviceId="${q(device())}", Version="${q(version)}"`;
      const qc = await fetch(`${location.origin}/QuickConnect/Initiate`, { method: "POST", headers: { Authorization: auth } }).catch(() => null);
      if (!qc || !qc.ok) throw qc && qc.status === 401 ? "Quick Connect is off in Jellyfin. Turn it on in the dashboard to switch profiles here." : "Jellyfin isn't answering. Try again in a moment.";
      const { Code, Secret } = await qc.json();
      // Wrong PIN, wrong password, too many tries: Chillarr says so here.
      const got = await invoke("profiles_switch_web", { id, pin, password, code: Code, trust: trust() });
      const r = await fetch(`${location.origin}/Users/AuthenticateWithQuickConnect`, {
        method: "POST",
        headers: { Authorization: auth, "Content-Type": "application/json" },
        body: JSON.stringify({ Secret }),
      }).catch(() => null);
      if (!r || !r.ok) throw "Jellyfin didn't sign that profile in. Try again.";
      const a = await r.json();
      // Signed in on another DeviceId than the old login (a Jellyfin app id the
      // bridge can't take): Jellyfin didn't end the old one, so it's ended here.
      if (appId && device() !== appId) {
        await fetch(`${location.origin}/Sessions/Logout`, { method: "POST", headers: { "X-Emby-Token": s.AccessToken } }).catch(() => {});
      }
      const c = JSON.parse(localStorage.getItem(CREDS) || "{}");
      for (const x of c.Servers || []) if (x.Id === s.Id) { x.AccessToken = a.AccessToken; x.UserId = a.User.Id; }
      localStorage.setItem(CREDS, JSON.stringify(c));
      // The owner's password was checked: this device doesn't ask for it again.
      if (got && got.trust) { try { localStorage.setItem(`chillarr_trust_${s.Id}`, got.trust); } catch (e) { /* storage blocked */ } }
      // No Logout for the old token: signing in with the same DeviceId already ended it in Jellyfin.
      leaving = true;
      location.replace(location.pathname);
      return new Promise(() => {});
    },
  };
})();
