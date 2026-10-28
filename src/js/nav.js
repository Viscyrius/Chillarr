// Moving around without a mouse: a game controller (wired or wireless, Xbox
// and PlayStation pads both show up as standard gamepads), the arrow keys a TV
// remote or air mouse sends, and the phone remote. Focus jumps to the nearest
// thing in the direction pressed, and rows scroll to keep it in view.
import { $, closeDialog, listen, WEB } from "./api.js";
import { heroStep } from "./browse.js";
import { padInput } from "./player.js";
import { goBack, stop as stopRestore } from "./place.js";
import { closeProfiles } from "./profiles.js";

const TABS = ["home", "movies", "series", "anime", "downloads", "mylist"];
const SELECTOR = "button, a[href], input, select, textarea, [tabindex]:not([tabindex='-1'])";

/** The layer on top: an open dialog, the player, or the page. */
function layer() {
  for (const id of ["ask", "together", "detail"]) {
    const d = $(`#${id}`);
    if (d?.open) return d;
  }
  if (!$("#player").hidden) return $("#player");
  return document.body;
}

function visible(e) {
  if (e.disabled || e.closest("[hidden], [inert]")) return false;
  const r = e.getBoundingClientRect();
  if (r.width < 2 || r.height < 2) return false;
  const s = getComputedStyle(e);
  return s.visibility !== "hidden" && s.display !== "none" && Number(s.opacity) > 0.05;
}

function candidates() {
  const root = layer();
  return [...root.querySelectorAll(SELECTOR)].filter((e) => {
    // Inside the page, skip what sits in a closed dialog or the hidden player.
    if (root === document.body && e.closest("dialog, #player")) return false;
    return visible(e);
  });
}

function show(e) {
  stopRestore(); // you're moving: a page still settling doesn't pull you back
  document.body.classList.add("nav-on");
  e.focus({ preventScroll: true, focusVisible: true });
  e.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  // Keep a little of the page above and below in view, not just the element.
  const r = e.getBoundingClientRect();
  if (layer() === document.body) {
    if (r.top < 90) window.scrollBy({ top: r.top - 110, behavior: "smooth" });
    else if (r.bottom > innerHeight - 30) window.scrollBy({ top: r.bottom - innerHeight + 60, behavior: "smooth" });
  }
}

/** Where keys start with nothing focused: the first thing below the top bar. */
const startAt = (all) => all.find((e) => e.getBoundingClientRect().top > 70 && !e.closest(".topbar")) || all[0];

/** Focus the nearest element in a direction ("up", "down", "left", "right"). */
export function move(dir) {
  const all = candidates();
  if (!all.length) return;
  const cur = document.activeElement;
  if (!cur || cur === document.body || !all.includes(cur)) { show(startAt(all)); return; }
  const a = cur.getBoundingClientRect();
  const ax = a.left + a.width / 2, ay = a.top + a.height / 2;
  let best = null, bestScore = Infinity;
  for (const e of all) {
    if (e === cur) continue;
    const b = e.getBoundingClientRect();
    const bx = b.left + b.width / 2, by = b.top + b.height / 2;
    const dx = bx - ax, dy = by - ay;
    let main, side;
    if (dir === "left") { if (b.right > a.left + 4 && dx > -4) continue; main = -dx; side = Math.abs(dy); }
    else if (dir === "right") { if (b.left < a.right - 4 && dx < 4) continue; main = dx; side = Math.abs(dy); }
    else if (dir === "up") { if (b.bottom > a.top + 4 && dy > -4) continue; main = -dy; side = Math.abs(dx); }
    else { if (b.top < a.bottom - 4 && dy < 4) continue; main = dy; side = Math.abs(dx); }
    if (main <= 0) continue;
    // Straight ahead beats diagonal; rows win over the far side of the screen.
    const score = main + side * (dir === "left" || dir === "right" ? 4 : 1.2);
    if (score < bestScore) { bestScore = score; best = e; }
  }
  if (best) show(best);
}

// The keys this file presses itself (for the player), so its own listener skips them.
const ours = new WeakSet();
function key(k) {
  const e = new KeyboardEvent("keydown", { key: k, bubbles: true });
  ours.add(e);
  window.dispatchEvent(e);
}

/** Back: close the top dialog or the player, leave search, or go back a page. */
export function back() {
  const top = layer();
  if (top !== document.body) {
    if (top.id === "player") key("Escape");
    else if (top.id === "ask") closeDialog(top);
    else top.dispatchEvent(new Event("cancel", { cancelable: true }));
    return;
  }
  const search = $("#search");
  if (document.activeElement === search || search.value) { search.value = ""; search.dispatchEvent(new Event("input")); search.blur(); return; }
  if (/^#(explore|filter|settings)/.test(location.hash)) goBack();
}

/* Back from a phone app or a TV remote: the top layer, search, then Home.
   The Jellyfin iOS and Android apps ask window.Emby.Page (jellyfin-web's
   router) and close themselves when there's nowhere to go; a TV's Back key
   lands here (remoteBack) and asks the TV app to close the same way. */
const atHome = () => /^#?(home)?$/.test(location.hash);
const canGoBack = () => layer() !== document.body || !$("#profiles").hidden || !!$("#search").value || !atHome();
function appBack() {
  if (!$("#profiles").hidden) { closeProfiles(); return; }
  const was = [layer(), location.hash, $("#search").value];
  if (was[0] !== document.body || was[2] || /^#(explore|filter|settings)/.test(location.hash)) back();
  // Something back() doesn't know closed nothing: never a dead button.
  if (layer() === was[0] && location.hash === was[1] && $("#search").value === was[2]) {
    if (was[0] !== document.body && was[0].tagName === "DIALOG") closeDialog(was[0]);
    else if (was[0].id === "player") $("#p-back").click();
    else location.hash = "home";
  }
}
if (WEB && !window.Emby) window.Emby = { Page: { canGoBack, back: appBack, goHome: () => (location.hash = "home") } };

/** A TV remote's Back (LG 461, Samsung 10009): at Home with nothing open, the TV app closes. */
const BACK_KEYS = new Set([461, 10009]);
const isRemoteBack = (e) => BACK_KEYS.has(e.keyCode) || e.key === "GoBack" || e.key === "BrowserBack" || e.key === "XF86Back";
function remoteBack() {
  if (canGoBack()) { appBack(); return; }
  const shell = window.NativeShell && window.NativeShell.AppHost;
  if (shell && shell.exit) shell.exit();
  // LG's app without its NativeShell in this page yet: the message its own NativeShell sends.
  else if (window.top !== window && /Web0S|webOS/.test(navigator.userAgent)) window.top.postMessage({ type: "AppHost.exit" }, "*");
}

function select() {
  const e = document.activeElement;
  if (!e || e === document.body) { move("down"); return; }
  if (e.tagName === "INPUT" && (e.type === "text" || e.type === "search" || e.type === "password")) { e.focus(); return; }
  e.click();
}

function tab(step) {
  const cur = (location.hash.slice(1) || "home").split("/")[0];
  const i = TABS.indexOf(cur);
  location.hash = `#${TABS[(i < 0 ? 0 : i + step + TABS.length) % TABS.length]}`;
}

/** One action, from any source. In the player the D-pad seeks and changes volume. */
export function act(a) {
  const inPlayer = layer().id === "player";
  const typing = ["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement?.tagName);
  if (inPlayer && ["left", "right", "up", "down", "play"].includes(a) && !$("#p-menu").contains(document.activeElement)) {
    // Arrows in the player behave like the keyboard: seek and volume.
    if (a === "play") key(" ");
    else key({ left: "ArrowLeft", right: "ArrowRight", up: "ArrowUp", down: "ArrowDown" }[a]);
    return;
  }
  if (["left", "right", "up", "down"].includes(a)) { if (typing && (a === "left" || a === "right")) return; move(a); }
  else if (a === "select") select();
  else if (a === "back") back();
  else if (a === "play") { if (inPlayer) key(" "); else select(); }
  else if (a === "tab-prev") tab(-1);
  else if (a === "tab-next") tab(1);
  else if (a === "hero-prev") heroStep(-1);
  else if (a === "hero-next") heroStep(1);
  else if (a === "search") { $("#search").focus(); document.body.classList.add("nav-on"); }
  else if (a === "seek-back") key("ArrowLeft");
  else if (a === "seek-fwd") key("ArrowRight");
}

/* ---------- Keyboard: arrow keys, Enter and Escape from a TV remote ---------- */
// Not e.isTrusted: jellyfin-web doesn't ask either, and a remote key is all
// a TV gives. The arrows' and OK's names come from their codes (web/shim.js).
window.addEventListener("keydown", (e) => {
  if (ours.has(e) || e.defaultPrevented || e.ctrlKey || e.altKey || e.metaKey) return;
  if (layer().id === "player") return; // the player has its own keys (Back included)
  if (isRemoteBack(e)) { e.preventDefault(); remoteBack(); return; }
  const typing = ["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement?.tagName);
  const dir = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" }[e.key];
  if (dir) {
    if (typing && (dir === "left" || dir === "right")) return;
    e.preventDefault();
    move(dir);
  } else if (e.key === "Enter" && (!document.activeElement || document.activeElement === document.body)) {
    // OK with nothing focused: start where the first arrow would. A focused button presses itself.
    e.preventDefault();
    select();
  } else if (e.key === "Escape" && layer() === document.body) back();
  else if (e.key === "Backspace" && !typing && layer() === document.body) { e.preventDefault(); back(); }
});
// The mouse is back: hide the controller focus ring. A TV keeps it (LG's
// pointer remote moves the mouse too, and the ring is how a remote finds its place).
const onTv = document.documentElement.classList.contains("tv");
if (onTv) document.body.classList.add("nav-on");
window.addEventListener("mousemove", () => { if (!onTv) document.body.classList.remove("nav-on"); }, { passive: true });

/* LG's Jellyfin app shows the server's pages in an iframe and never focuses
   it: the remote's keys go to the app's own page around it, which drops the
   arrows and closes on Back. jellyfin-web gets them only because it focuses
   a card as each page loads, and focusing something in a frame moves the
   keys there. So on a TV, Chillarr does the same: the first poster takes
   focus once one is drawn, unless something else already has it. No time
   limit: a slow TV or a cold server may take long to draw, and a page
   that never got focus never hears the remote at all. Plain focus(), not
   show(): a place being restored (place.js) still wins if it lands later. */
if (onTv) {
  window.focus();
  const claim = setInterval(() => {
    const a = document.activeElement;
    if (a && a !== document.body) { clearInterval(claim); return; }
    // The page itself, not the top bar or the skip link while it's still loading.
    const all = layer() === document.body ? candidates().filter((e) => e.closest("#view")) : [];
    if (all.length) { startAt(all).focus({ preventScroll: true }); clearInterval(claim); }
  }, 250);
}

/* ---------- Game controllers ---------- */
// Standard mapping: 0 A, 1 B, 2 X, 3 Y, 4 LB, 5 RB, 6 LT, 7 RT, 8 Back/View, 9 Start/Menu, 12-15 D-pad.
const BUTTONS = { 0: "select", 1: "back", 2: "play", 3: "search", 4: "tab-prev", 5: "tab-next", 6: "seek-back", 7: "seek-fwd", 8: "back", 9: "play", 12: "up", 13: "down", 14: "left", 15: "right" };
const REPEAT_AFTER = 380, REPEAT_EVERY = 110;
const held = new Map(); // action -> { since, last }
let polling = false;

/* ---------- Controller mode: what the buttons do, shown on screen ---------- */
// Built the first time a pad (or the phone remote) is used; hidden again by
// the mouse or a real key. Nothing runs while it's off.
let guide = null;
let guideKey = "";
const G = (k, cls = "") => `<kbd class="${cls}">${k}</kbd>`;

function guideFor() {
  const top = layer();
  if (top.id === "player") return ""; // the player shows its own
  if (top.id === "ask") return `${G("A", "round g-a")} Choose ${G("B", "round g-b")} Cancel`;
  if (top.tagName === "DIALOG") return `${G("A", "round g-a")} Select ${G("B", "round g-b")} Close ${G("D-pad")} Move`;
  const onHero = !!document.activeElement?.closest?.(".hero");
  return [
    `${G("LB")} ${G("RB")} Switch tab`,
    `${G("A", "round g-a")} Select`,
    `${G("B", "round g-b")} Back`,
    `${G("Y", "round g-y")} Search`,
    `${G("X", "round g-x")} Play`,
    onHero ? `${G("LT")} ${G("RT")} Featured` : `${G("D-pad")} Move`,
  ].join("");
}

function drawGuide() {
  if (!guide) return;
  const html = guideFor();
  if (html === guideKey) return;
  guideKey = html;
  guide.innerHTML = html.replace(/(<kbd[^>]*>[^<]*<\/kbd>)((?:\s*<kbd[^>]*>[^<]*<\/kbd>)*)\s*([^<]+)/g, '<span class="pad-hint">$1$2 $3</span>');
  guide.hidden = !html;
}

function padOn() {
  if (!guide) {
    guide = document.createElement("div");
    guide.className = "pad-guide";
    guide.setAttribute("aria-hidden", "true");
    document.body.append(guide);
    // LB / RB badges around the tabs, so it's clear they switch between them.
    const tabs = document.querySelector('[data-tab="home"]')?.parentElement;
    if (tabs) {
      tabs.prepend(Object.assign(document.createElement("kbd"), { className: "tab-glyph", textContent: "LB" }));
      tabs.append(Object.assign(document.createElement("kbd"), { className: "tab-glyph", textContent: "RB" }));
    }
  }
  if (!document.body.classList.contains("pad-mode")) document.body.classList.add("pad-mode");
  drawGuide();
}

function padOff() {
  if (document.body.classList.contains("pad-mode")) document.body.classList.remove("pad-mode");
}
window.addEventListener("mousedown", padOff, { passive: true });
window.addEventListener("keydown", (e) => { if (e.isTrusted) padOff(); }, { capture: true });
let lastMouse = null;
window.addEventListener("mousemove", (e) => {
  // Small jitters from a resting mouse don't count.
  if (lastMouse && Math.abs(e.screenX - lastMouse[0]) + Math.abs(e.screenY - lastMouse[1]) > 8) padOff();
  lastMouse = [e.screenX, e.screenY];
}, { passive: true });
// What's on screen changed (a dialog opened, a tab loaded): refresh the guide.
for (const ev of ["hashchange", "focusin"]) window.addEventListener(ev, () => { if (document.body.classList.contains("pad-mode")) requestAnimationFrame(drawGuide); });

/** A pad button: the player's controller mode gets it first, then the usual action. */
function padAct(a) {
  padOn();
  if (layer().id === "player" && padInput(a)) return;
  if (a === "seek-back" && layer() === document.body) { heroStep(-1); return; }
  if (a === "seek-fwd" && layer() === document.body) { heroStep(1); return; }
  act(a);
  requestAnimationFrame(drawGuide);
}

function poll() {
  const pads = [...(navigator.getGamepads?.() || [])].filter(Boolean);
  if (!pads.length) { polling = false; held.clear(); return; }
  const now = performance.now();
  const down = new Set();
  for (const p of pads) {
    p.buttons.forEach((b, i) => { if (b.pressed && BUTTONS[i]) down.add(BUTTONS[i]); });
    const [x = 0, y = 0] = p.axes;
    if (x < -0.55) down.add("left"); else if (x > 0.55) down.add("right");
    if (y < -0.55) down.add("up"); else if (y > 0.55) down.add("down");
  }
  for (const a of down) {
    const h = held.get(a);
    const repeats = ["up", "down", "left", "right", "seek-back", "seek-fwd"].includes(a);
    if (!h) { held.set(a, { since: now, last: now }); if (document.hasFocus()) padAct(a); }
    else if (repeats && now - h.since > REPEAT_AFTER && now - h.last > REPEAT_EVERY) { h.last = now; if (document.hasFocus()) padAct(a); }
  }
  for (const a of [...held.keys()]) if (!down.has(a)) held.delete(a);
  requestAnimationFrame(poll);
}

function startPolling() {
  if (polling) return;
  polling = true;
  requestAnimationFrame(poll);
}
window.addEventListener("gamepadconnected", startPolling);
// A pad paired before Chillarr opened only reports itself once a button is pressed;
// a slow check catches it without a loop running when there's no controller.
setInterval(() => { if (!polling && [...(navigator.getGamepads?.() || [])].some(Boolean)) startPolling(); }, 2000);

/* ---------- Phone remote ---------- */
listen("remote", (e) => {
  const { action, text } = e.payload || {};
  if (action === "type") {
    const s = $("#search");
    s.value = text || "";
    s.dispatchEvent(new Event("input"));
    return;
  }
  if (action) { padOn(); act(action); requestAnimationFrame(drawGuide); }
});

/* ---------- Focus comes back where it was ----------
   Closing the detail, a question, Watch together, the player or the profile
   picker used to leave nothing focused, so on a TV the next arrow started
   over from the top. With keys in use (nav-on), whatever opened the layer
   gets focus back once it's closed, if nothing else took it meanwhile. */
const openers = new Map();
const LAYERS = [["#detail", "open"], ["#ask", "open"], ["#together", "open"], ["#player", "hidden"], ["#profiles", "hidden"]];
// The last thing focused outside every layer. Read when a layer opens: by
// then (a mutation callback runs after showModal) focus is already inside it.
let lastOutside = null;
document.addEventListener("focusin", (e) => {
  if (!LAYERS.some(([sel]) => { const l = $(sel); return l && l.contains(e.target); })) lastOutside = e.target;
});
for (const [sel, attr] of LAYERS) {
  const node = $(sel);
  if (!node) continue;
  const isOpen = () => (attr === "open" ? node.hasAttribute("open") : !node.hidden);
  let was = isOpen();
  new MutationObserver(() => {
    const now = isOpen();
    if (now === was) return;
    was = now;
    if (now) {
      const a = node.contains(document.activeElement) ? lastOutside : document.activeElement;
      if (a && a !== document.body && !node.contains(a)) {
        const row = a.closest(".row");
        openers.set(node, { el: a, row: row && row.querySelector("h2") ? row.querySelector("h2").textContent : null });
      }
      return;
    }
    const was0 = openers.get(node);
    const back = was0 && was0.el;
    openers.delete(node);
    // After the closing fade (and again once a redraw has landed): the page
    // under it is often drawn again, so a poster is found again by its key.
    const restore = () => {
      const a = document.activeElement;
      if (!back || !document.body.classList.contains("nav-on")) return;
      if (a && a !== document.body && !a.closest("[hidden], dialog:not([open])")) return;
      const key = back.dataset && back.dataset.key;
      let target = back.isConnected ? back : null;
      if (!target && key) {
        const sel = `.tile[data-key="${CSS.escape(key)}"]`;
        const row = [...document.querySelectorAll("#view .row")].find((r) => r.querySelector("h2") && r.querySelector("h2").textContent === was0.row);
        target = (row && row.querySelector(sel)) || $(`#view ${sel}`);
      }
      if (target) target.focus({ preventScroll: false });
    };
    setTimeout(restore, 260);
    setTimeout(restore, 900);
  }).observe(node, { attributes: true, attributeFilter: [attr] });
}

/* Rows redraw by themselves (On the way as downloads move, progress landing),
   and a redraw takes the focused poster with it: the remote's place was lost
   at any moment. With keys in use, a redraw that leaves nothing focused puts
   focus on the same poster in the same row again. A new page starts fresh. */
{
  const view = $("#view");
  let held = null; // { key, row } of the poster last focused on this page
  const rowName = (t) => { const r = t.closest(".row"); const h = r && r.querySelector("h2"); return h ? h.textContent : null; };
  document.addEventListener("focusin", (e) => {
    // Only while a poster is what's focused: moving to anything else (the
    // hero's buttons, a chip, the top bar) lets go, or a later redraw would
    // pull focus back to a poster the user already left.
    const t = e.target.closest && e.target.closest("#view .tile[data-key]");
    held = t ? { key: t.dataset.key, row: rowName(t) } : null;
  });
  window.addEventListener("hashchange", () => (held = null));
  if (view) new MutationObserver(() => {
    const a = document.activeElement;
    if (!held || (a && a !== document.body) || !document.body.classList.contains("nav-on") || layer() !== document.body) return;
    const sel = `.tile[data-key="${CSS.escape(held.key)}"]`;
    const row = [...view.querySelectorAll(".row")].find((r) => rowName(r.querySelector(".tile") || r) === held.row);
    const t = (row && row.querySelector(sel)) || view.querySelector(sel);
    if (t) t.focus({ preventScroll: true });
  }).observe(view, { childList: true, subtree: true });
}
