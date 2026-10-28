// Put into jellyfin-web's index.html (jfweb.rs), before its own scripts.
// Signed in and going to Home: Chillarr's pages open instead. Sign-in,
// your settings and the admin dashboard stay jellyfin-web's.
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
  function session(key) { try { return sessionStorage.getItem(key); } catch (e) { return null; } }
  // A login Chillarr just refused stays in jellyfin-web; signing in again makes a new one.
  function signedIn() {
    var t = token();
    return !!t && session("chillarr_bounce") !== t;
  }
  // #/home, #/home.html, #!/home.html (older versions), or no page at all.
  var HOME = /^#!?\/?(home(\.html)?)?(\?.*)?$/;
  // A browser that can't parse today's JavaScript (TVs from before 2020) gets
  // the page built for it: the same app, bundled down for old Chrome (legacy.js).
  // Only a SyntaxError means old: a page rule against eval throws too, and that browser is fine.
  var modern = true;
  try { new Function("var a; a?.b; a ??= 1"); } catch (e) { modern = !(e instanceof SyntaxError); }
  var PAGE = modern ? DIR + "/" : DIR + "/legacy.html";
  var gone = false;
  function check() {
    // Over https Chillarr's bridge (plain http) can't be called: jellyfin-web's own Home stays.
    // Chillarr's pages gave up on this browser this visit (index.html's guard): Jellyfin's own Home stays too.
    if (gone || location.protocol === "https:" || session("chillarr_fallback") || !HOME.test(location.hash || "#") || !signedIn()) return;
    gone = true;
    location.replace(PAGE);
  }
  // jellyfin-web moves between pages with pushState, which fires no hashchange.
  ["pushState", "replaceState"].forEach(function (name) {
    var orig = history[name];
    history[name] = function () { var r = orig.apply(this, arguments); check(); return r; };
  });
  window.addEventListener("hashchange", check);
  window.addEventListener("popstate", check);
  check();
})();
