// Put into jellyfin-web's index.html (jfweb.rs), before its own scripts.
// Signed in and going to Home (or Movies or Shows): Chillarr's pages open
// instead. Sign-in, your settings, the admin dashboard and the libraries
// Chillarr doesn't show (music, books, live TV...) stay jellyfin-web's.
// ES5 on purpose (var, function, no arrows or template strings): it runs in
// jellyfin-web's page on every TV, however old, and must never be what breaks.
(function () {
  var DIR = "__CHILLARR_DIR__";
  function originOf(addr) { try { return new URL(addr).origin; } catch (e) { return ""; } }
  // The login the shim would use (web/shim.js server()): this server's by
  // address, or the only one there is.
  function token() {
    var all = [];
    try { all = JSON.parse(localStorage.getItem("jellyfin_credentials") || "{}").Servers || []; } catch (e) { return null; }
    var signed = all.filter(function (s) { return s.AccessToken && s.UserId; });
    var mine = signed.filter(function (x) {
      return [x.ManualAddress, x.LocalAddress, x.RemoteAddress].some(function (a) { return a && originOf(a) === location.origin; });
    })[0];
    var s = mine || (signed.length === 1 ? signed[0] : null);
    return s ? s.AccessToken : null;
  }
  // A note Chillarr's pages left for this tab (shim.js, index.html's guard),
  // read once and removed: it holds for THIS page only, for a while, and the
  // next load (or a later trip Home) tries Chillarr's pages again. Kept for
  // the whole tab before, a phone that was slow once stayed on Jellyfin's
  // own pages until the tab was closed, which on a phone is never.
  var HOLD = 5 * 60 * 1000;
  var loaded = new Date().getTime();
  function take(key) {
    var v = null;
    try { v = sessionStorage.getItem(key); if (v !== null) sessionStorage.removeItem(key); } catch (e) { /* storage blocked */ }
    return v;
  }
  // Chillarr's pages gave up on this browser (they never drew, or a script
  // failed): Jellyfin's own pages stay, so a broken copy is never a blank page.
  var fellBack = !!take("chillarr_fallback");
  // A login Chillarr just refused (shim.js toLogin): jellyfin-web may still
  // think it's good and go on to Home, and sending it straight back would
  // loop. Signing in again makes a new one; otherwise the next load (or a
  // trip Home after a while) asks Chillarr again.
  var refused = take("chillarr_bounce");
  function held() { return new Date().getTime() - loaded < HOLD; }
  function signedIn() {
    var t = token();
    return !!t && !(t === refused && held());
  }
  // #/home, #/home.html, #!/home.html (older versions), or no page at all.
  var HOME = /^#!?\/?(home(\.html)?)?(\?.*)?$/;
  // Jellyfin's own Movies and Shows pages (its menu, a restored tab, Back):
  // Chillarr's tabs of the same name.
  var TABS = /^#!?\/(movies|tv)(\.html)?(\?.*)?$/;
  // A browser that can't parse today's JavaScript (TVs from before 2020) gets
  // the page built for it: the same app, bundled down for old Chrome (legacy.js).
  // Only a SyntaxError means old: a page rule against eval throws too, and that browser is fine.
  var modern = true;
  try { new Function("var a; a?.b; a ??= 1"); } catch (e) { modern = !(e instanceof SyntaxError); }
  var PAGE = modern ? DIR + "/" : DIR + "/legacy.html";
  // When this page last set off for Chillarr's: a trip that never happened
  // (cut off, or the page came back from the browser's back/forward cache)
  // doesn't block the next one.
  var goneAt = 0;
  function check() {
    var hash = location.hash || "#";
    var tab = TABS.exec(hash);
    if (!tab && !HOME.test(hash)) return;
    if (new Date().getTime() - goneAt < 8000) return;
    // Over https Chillarr's bridge (plain http) can't be called (mixed
    // content): jellyfin-web's own Home stays. On purpose; shim.js says the same.
    if (location.protocol === "https:") return;
    if (fellBack && held()) return;
    if (!signedIn()) return;
    goneAt = new Date().getTime();
    location.replace(PAGE + (tab ? (tab[1] === "tv" ? "#series" : "#movies") : ""));
  }
  // jellyfin-web moves between pages with pushState, which fires no hashchange.
  ["pushState", "replaceState"].forEach(function (name) {
    var orig = history[name];
    history[name] = function () { var r = orig.apply(this, arguments); check(); return r; };
  });
  window.addEventListener("hashchange", check);
  window.addEventListener("popstate", check);
  // Back to this page from the browser's back/forward cache: no script runs
  // again, so it's looked at here.
  window.addEventListener("pageshow", function (e) { if (e && e.persisted) { goneAt = 0; check(); } });
  check();
})();
