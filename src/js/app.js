// Routing between tabs, the top bar, search, and the first-run tutorial.
import { $, el, invoke, screenMode, store, viewer, WEB, windowFull } from "./api.js";
import { renderTab, renderSearch, renderExplore, stopBrowse } from "./browse.js";
import { renderFilter } from "./filter.js";
import { renderSettings } from "./settings.js";
import { openSetup } from "./setup.js";
import { startProfiles, startProfilesWeb } from "./profiles.js";
import * as place from "./place.js";
import "./nav.js";
import { recentSearches } from "./recent.js";

const view = $("#view");
const TABS = ["home", "movies", "series", "anime", "downloads", "mylist", "settings", "explore", "filter"];
let current = "";

/**
 * Shows the page in the address. `redraw`: the page already on screen again,
 * which stays up until the new one is ready and keeps your place.
 */
function route(redraw = false) {
  const hash = location.hash.slice(1) || "home";
  const tab = TABS.includes(hash.split("/")[0]) ? hash.split("/")[0] : "home";
  const left = place.current();
  const { id, fresh } = place.entry();
  document.querySelectorAll("[data-tab]").forEach((a) => a.classList.toggle("on", a.dataset.tab === tab));
  if (tab === current && tab !== "settings" && tab !== "explore" && tab !== "filter") return;
  // Leaving a page: remember where you were on it. Search results aren't a
  // page of their own; the place from before the search stays.
  if (left && left !== id && current && current !== "search") place.remember(left);
  current = tab;
  $("#search").value = "";
  place.stop();
  if (redraw) place.remember(id);
  // Back or Forward to a page, or a redraw, puts you where you were; a fresh click starts at the top.
  const mode = redraw ? "redraw" : !fresh && place.has(id) ? "back" : null;
  if (!mode) window.scrollTo({ top: 0, behavior: "instant" });
  const want = mode ? place.cards(id) : 0;
  let drawn;
  if (tab === "settings") { stopBrowse(); renderSettings(view, hash.split("/")[1]); }
  else if (tab === "filter") {
    const [, kind, json] = hash.split("/");
    let f = {};
    try { f = JSON.parse(decodeURIComponent(json || "{}")); } catch (e) { /* a broken link starts clean */ }
    drawn = renderFilter(view, kind, f, mode, want);
  }
  else if (tab === "explore") { const [, cat, title] = hash.split("/"); drawn = renderExplore(view, decodeURIComponent(cat || ""), decodeURIComponent(title || "Explore"), mode, want); }
  else drawn = renderTab(view, tab, mode);
  // Once the page is in (it runs before that frame is drawn); what loads after it is followed too.
  if (mode) Promise.resolve(drawn).then(() => { if (place.current() === id) place.restore(id); });
}

window.addEventListener("hashchange", () => route());

// The top bar turns solid once the page scrolls past the hero.
window.addEventListener("scroll", () => $("#topbar").classList.toggle("solid", window.scrollY > 40), { passive: true });

let searchTimer;
function runSearch(term) {
  if (term.length >= 2) {
    // Leaving the page for search: where you were is waiting when you come back.
    if (current !== "search") place.remember();
    current = "search";
    document.querySelectorAll("[data-tab]").forEach((a) => a.classList.remove("on"));
    renderSearch(view, term);
    searched(term);
  } else if (!term) {
    current = "";
    route();
  }
}
// Recent searches, in Chillarr's own dropdown (the browser's is off).
const searched = recentSearches($("#search"), (term) => { clearTimeout(searchTimer); runSearch(term); });
$("#search").addEventListener("input", (e) => {
  clearTimeout(searchTimer);
  const term = e.target.value.trim();
  // Local results answer at once now, so the wait after typing can be short.
  searchTimer = setTimeout(() => runSearch(term), 180);
});
// No browser autofill dropdown on any text box, including ones added later.
document.addEventListener("focusin", (e) => {
  const t = e.target;
  if (t instanceof HTMLInputElement && !t.hasAttribute("autocomplete") && /^(text|search|email|url|tel|number|)$/.test(t.type)) t.setAttribute("autocomplete", "off");
});

// The gear shows a dot when an app needs attention.
function updateHealth() {
  const levels = [...store.cards.values()].map((c) => c.level);
  const level = levels.includes("red") ? "red" : levels.includes("amber") ? "amber" : levels.length ? "ok" : "unknown";
  $("#health-dot").dataset.level = level;
}
store.on((what) => { if (what === "cards") updateHealth(); });

// Full screen for the whole app: the top bar button or F11.
async function setAppFull(on) {
  screenMode.app = await windowFull(on);
  drawAppFull();
}
function drawAppFull() {
  document.body.classList.toggle("app-full", screenMode.app);
  $("#fs-btn").title = screenMode.app ? "Exit full screen (F11)" : "Full screen (F11)";
  $("#fs-btn").setAttribute("aria-label", screenMode.app ? "Exit full screen" : "Full screen");
}
$("#fs-btn").addEventListener("click", () => setAppFull(!screenMode.app));
window.addEventListener("keydown", (e) => {
  if (e.key === "F11" && $("#player").hidden) { e.preventDefault(); setAppFull(!screenMode.app); }
});

window.addEventListener("keydown", (e) => {
  if (e.key === "/" && document.activeElement?.tagName !== "INPUT" && $("#player").hidden) {
    e.preventDefault();
    $("#search").focus();
  }
});

document.addEventListener("contextmenu", (e) => {
  if (!e.target.closest("input, textarea")) e.preventDefault();
});

/* Booted: the app drew something into the page (its loading skeleton
   counts: a slow TV may wait a long time on its first rows), so index.html's
   guard, which is for a page that never came alive, leaves this visit be. */
(() => {
  const seen = () => { if (!window.__CHILLARR_BOOTED__ && view.firstElementChild) { window.__CHILLARR_BOOTED__ = true; mo.disconnect(); } };
  const mo = new MutationObserver(seen);
  mo.observe(view, { childList: true, subtree: true });
})();

/** Start in Jellyfin's web pages: no profiles, setup or app health, just the signed-in user. */
async function startWeb() {
  document.body.classList.add("web");
  try { Object.assign(viewer, await invoke("web_viewer")); }
  catch (e) {
    view.replaceChildren(el("div", { class: "web-down" }, el("h1", { text: "Chillarr isn't answering" }), el("p", { text: String(e) }),
      el("button", { class: "btn primary", onclick: () => location.reload() }, "Try again")));
    return;
  }
  // The browser leaves full screen by itself too (Esc), player open or not: the
  // button follows it. Going in is always ours, so only leaving is followed.
  const leftFull = () => {
    if (document.fullscreenElement || document.webkitFullscreenElement || !screenMode.app) return;
    screenMode.app = false;
    drawAppFull();
  };
  document.addEventListener("fullscreenchange", leftFull);
  document.addEventListener("webkitfullscreenchange", leftFull);
  // In Jellyfin's phone apps the player hides the app's own bars while it's open.
  if (window.NativeShell) {
    new MutationObserver(() => { try { $("#player").hidden ? NativeShell.disableFullscreen?.() : NativeShell.enableFullscreen?.(); } catch { /* an older app */ } })
      .observe($("#player"), { attributes: true, attributeFilter: ["hidden"] });
  }
  // Only the built-in player plays in a browser.
  store.settings = { setup_done: true, player: "chillarr" };
  invoke("media_status").then((m) => (store.media = { ...m, vlc: false }));
  drawViewer();
  route();
  // One of Chillarr's profiles: the switcher takes the avatar's place.
  startProfilesWeb().catch(() => {});
}

/** The signed-in Jellyfin user in the top bar; it opens Settings > Account. */
function drawViewer() {
  const name = viewer.name || "?";
  $("#profile-slot").replaceChildren(el("a", { class: "pf-top", href: "#settings/account", "aria-label": `Signed in as ${name}. Account`, title: name },
    el("span", { class: "pf-avatar small", style: "--c:var(--accent)", "aria-hidden": "true" }, (name.trim()[0] || "?").toUpperCase())));
}

(async () => {
  if (WEB) return startWeb();
  store.settings = await invoke("get_settings");
  invoke("media_status").then((m) => (store.media = m));
  (await invoke("refresh")).forEach((c) => store.cards.set(c.id, c));
  updateHealth();
  route();
  if (!store.settings.setup_done) openSetup();
  else startProfiles();
})();

// Other modules ask for a redraw (after setup, a sign-in, a new download) this way,
// so they don't need to import this file.
window.addEventListener("chillarr:refresh", () => {
  invoke("media_status").then((m) => (store.media = WEB ? { ...m, vlc: false } : m));
  // A search on screen is searched again, not swapped for the tab under it
  // (a download landing used to wipe the box and drop the results coming in).
  const term = $("#search").value.trim();
  if (current === "search" && term.length >= 2) { renderSearch(view, term); return; }
  current = "";
  // Coming back from the player (or a download landing) keeps your place on the page.
  route(true);
});
