// First run: one screen of questions, one press, one bar. Chillarr does the
// rest: every download, one Windows prompt, starting and connecting the apps,
// the login, the folders and the sources (onboard.rs).
import { $, audioChoice, el, invoke, listen, store, toast } from "./api.js";
import { downloadsRow, folderPicker, saveLogsButton } from "./settings.js";
import { logoImg, themePicker } from "./theme.js";

const root = $("#setup");
const state = { movies: [], series: [], downloads: null, base: null, free: 0, custom: false, loaded: false, audio: { audio_pref: "sub", audio_lang: "eng" } };

/* ---------- The form ---------- */
const join = (base, name) => `${base.replace(/[\\/]+$/, "")}\\${name}`;
const gbText = (b) => (b >= 1e12 ? `${(b / 1e12).toFixed(1)} TB` : `${Math.round(b / 1e9)} GB`);

async function loadForm() {
  if (state.loaded) return;
  const [f, suggest] = await Promise.all([invoke("setup_folders").catch(() => ({ movies: [], series: [] })), invoke("setup_suggest").catch(() => null)]);
  state.movies = [...(f.movies || [])];
  state.series = [...(f.series || [])];
  state.custom = !!(state.movies.length || state.series.length);
  state.audio = { audio_pref: store.settings?.audio_pref || "sub", audio_lang: store.settings?.audio_lang || "eng" };
  if (suggest) { state.base = suggest.path; state.free = suggest.free; }
  state.loaded = true;
}

async function showForm() {
  await loadForm();
  const ms = await invoke("media_status").catch(() => ({}));
  const err = el("p", { class: "err", role: "alert" });
  const user = el("input", { type: "text", autocomplete: "username", value: ms.user || "", "aria-label": "Name" });
  const pass = el("input", { type: "password", autocomplete: ms.setup_done ? "current-password" : "new-password", "aria-label": "Password" });
  const where = el("div");

  const drawWhere = () => {
    if (state.custom) {
      where.replaceChildren(el("div", { class: "folder-step" },
        folderPicker("Movies", "Add a second folder named Anime for anime films.", state.movies),
        folderPicker("Series and anime", "A folder with Anime in its name gets anime.", state.series),
        downloadsRow((p) => { state.downloads = p; }),
        el("button", { class: "link-btn", type: "button", onclick: () => { state.custom = false; drawWhere(); } }, "Put everything in one folder instead")));
      return;
    }
    const pick = el("button", { class: "btn small ghost", type: "button" }, "Change");
    pick.addEventListener("click", async () => {
      const p = await invoke("pick_folder");
      if (p) { state.base = p; state.free = 0; drawWhere(); }
    });
    where.replaceChildren(el("div", { class: "where-one" },
      el("div", {}, el("strong", { class: "path", text: state.base || "Pick a folder" }),
        el("small", { text: state.free ? `${gbText(state.free)} free. Movies, series, anime and downloads each get a folder inside it.` : "Movies, series, anime and downloads each get a folder inside it." })),
      pick),
      el("button", { class: "link-btn", type: "button", onclick: () => { state.custom = true; drawWhere(); } }, "Choose each folder"));
  };
  drawWhere();

  const voices = audioChoice(() => state.audio, (change) => Object.assign(state.audio, change));
  const signedIn = !!ms.signed_in;
  const hasAccount = !signedIn && !!ms.setup_done;
  const go = el("button", { class: "btn primary large", type: "submit" }, "Set up Chillarr");
  const form = el("form", { class: "setup-form" },
    el("section", {}, el("h2", { text: "Where should everything go?" }), where),
    el("section", {}, el("h2", { text: signedIn ? `Jellyfin is ready as ${ms.user}` : hasAccount ? "Sign in to Jellyfin" : "Pick one login" }),
      el("p", { class: "note", text: signedIn ? "Enter a password to use the same login for your download apps, or leave it empty." : hasAccount ? "Jellyfin already has an account on this PC. Your download apps get the same login." : "Use it to watch on your TV and phone. Your download apps get the same one, so none of them stops to ask." }),
      el("div", { class: "login-row" },
        el("label", { class: "field" }, el("small", { text: "Name" }), user),
        el("label", { class: "field" }, el("small", { text: "Password, at least 5 characters" }), pass))),
    el("section", {}, el("h2", { text: "Anime and movies" }), el("p", { class: "note", text: "You can switch for any title in the player, when its file has more than one." }), voices),
    el("section", {}, el("h2", { text: "Pick a colour" }), el("p", { class: "note", text: "It colours the app and your desktop icon. You can change it any time in Settings." }), themePicker()),
    el("div", { class: "setup-go" }, go, el("p", { class: "note", text: "Windows asks for your OK once, then everything downloads and installs by itself." })),
    err);
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    err.textContent = "";
    const req = { ...state.audio };
    Object.assign(store.settings, state.audio);
    if (state.custom) {
      if (!state.movies.length || !state.series.length) { err.textContent = "Add at least one movie folder and one series folder."; return; }
      req.movies = state.movies; req.series = state.series; req.downloads = state.downloads;
    } else {
      if (!state.base) { err.textContent = "Pick a folder first."; return; }
      req.movies = [join(state.base, "Movies"), join(state.base, "Anime Movies")];
      req.series = [join(state.base, "Series"), join(state.base, "Anime")];
      req.downloads = join(state.base, "Downloads");
    }
    const name = user.value.trim();
    if (signedIn && !pass.value) { /* keeps the login it has */ }
    else if (!name || pass.value.length < 5) { err.textContent = "Enter a name and a password of at least 5 characters."; return; }
    else { req.user = name; req.password = pass.value; }
    startRun(() => invoke("setup_run", { req }));
  });
  root.replaceChildren(el("div", { class: "setup-inner" },
    el("header", { class: "setup-hello" }, logoImg("welcome-logo"),
      el("div", {}, el("h1", { id: "setup-title", text: "Let's get you watching" }),
        el("p", { class: "lead", text: "A few quick choices. Chillarr installs and connects everything else." }))),
    form));
  user.focus();
}

/* ---------- The bar ---------- */
const STAGE_WORDS = {
  resolve: "Finding the latest versions",
  download: "Downloading",
  permission: "Waiting for Windows",
  install: "Installing",
  start: "Starting your apps",
  connect: "Connecting everything",
  sources: "Adding sources",
  finish: "Finishing up",
  done: "All set",
};
const mb = (b) => (b >= 1e9 ? `${(b / 1e9).toFixed(2)} GB` : `${Math.round(b / 1e6)} MB`);
const timeLeft = (s) => (s < 60 ? "under a minute left" : s < 3600 ? `about ${Math.ceil(s / 60)} min left` : `about ${(s / 3600).toFixed(1)} hours left`);
const ROW_WORDS = { wait: "Waiting", download: "Downloading", check: "Checking", ready: "Downloaded", install: "Installing", done: "Installed", have: "Already here", failed: "Didn't work" };

let run = null;

function showBar(onlyInstalling, onFinish) {
  const pct = el("span", { class: "meter-num", "aria-hidden": "true", text: "0" });
  const fill = el("span", { class: "meter-fill" });
  const bar = el("div", { class: "meter-bar", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100", "aria-label": "Setup progress" }, fill);
  const headline = el("h1", { id: "setup-title", text: "Getting everything ready" });
  const detail = el("p", { class: "meter-detail", "aria-live": "polite" });
  const callout = el("div", { class: "meter-callout", hidden: true });
  const rows = el("ul", { class: "meter-rows" });
  const sites = el("p", { class: "meter-sites" });
  root.replaceChildren(el("div", { class: "setup-inner meter" }, headline, el("div", { class: "meter-top" }, el("div", { class: "meter-big" }, pct, el("span", { class: "meter-pc", text: "%" })), detail), bar, callout, rows, sites));

  const rowEls = new Map();
  let over = false;
  let shown = 0, target = 0, shownBytes = 0, targetBytes = 0, speed = 0, lastGot = 0, lastAt = 0, raf = 0;

  const tick = () => {
    // Glides toward the latest number, so a burst of bytes never makes it jump.
    shown += (target - shown) * 0.12;
    shownBytes += (targetBytes - shownBytes) * 0.2;
    if (Math.abs(target - shown) < 0.02) shown = target;
    pct.textContent = String(Math.floor(shown));
    fill.style.transform = `scaleX(${shown / 100})`;
    bar.setAttribute("aria-valuenow", String(Math.floor(shown)));
    if (run?.m?.stage === "download") drawDetail(run.m);
    raf = requestAnimationFrame(tick);
  };

  const drawDetail = (m) => {
    if (m.stage === "download" && m.total > 0) {
      const left = speed > 1000 ? (m.total - m.got) / speed : 0;
      detail.textContent = [`${mb(Math.min(shownBytes, m.total))} of ${mb(m.total)}`, speed > 1000 ? `${mb(speed)}/s` : "", left ? timeLeft(left) : ""].filter(Boolean).join(", ");
    } else {
      detail.textContent = m.line || STAGE_WORDS[m.stage] || "";
    }
  };

  const drawRow = (r) => {
    let e = rowEls.get(r.id);
    if (!e) {
      const fillEl = el("span");
      e = { li: el("li", {}, el("span", { class: "dot" }), el("strong", { text: r.label }), el("small"), el("span", { class: "row-bar" }, fillEl)), fill: fillEl };
      e.note = e.li.querySelector("small");
      rowEls.set(r.id, e);
      rows.append(e.li);
    }
    e.li.dataset.state = r.state;
    const pctDone = r.total ? Math.min(100, (r.got / r.total) * 100) : 0;
    e.note.textContent = r.note || (r.state === "download" && r.total > 1 ? `${mb(r.got)} of ${mb(r.total)}` : ROW_WORDS[r.state] || "");
    e.fill.style.transform = `scaleX(${r.state === "download" ? pctDone / 100 : ["ready", "install", "check", "done"].includes(r.state) ? 1 : 0})`;
  };

  const draw = (m) => {
    if (over) return;
    run.m = m;
    target = Math.max(target, m.percent || 0);
    targetBytes = m.got || 0;
    const now = performance.now();
    if (lastAt && now - lastAt > 300) {
      const inst = Math.max(0, (m.got - lastGot) / ((now - lastAt) / 1000));
      speed = speed ? speed * 0.8 + inst * 0.2 : inst;
      lastGot = m.got; lastAt = now;
    } else if (!lastAt) { lastGot = m.got; lastAt = now; }
    headline.textContent = STAGE_WORDS[m.stage] || "Getting everything ready";
    drawDetail(m);
    (m.apps || []).forEach(drawRow);
    sites.textContent = m.sites_total ? `Sources: ${m.sites_added} of ${m.sites_total} added` : "";
    if (m.declined) {
      cancelAnimationFrame(raf);
      callout.hidden = false;
      callout.replaceChildren(el("div", {}, el("strong", { text: "Windows needs your OK to install the apps" }),
        el("p", { text: "Nothing was changed, and your downloads are kept. Press Try again and choose Yes." })),
        el("div", { class: "callout-acts" },
          el("button", { class: "btn ghost", type: "button", onclick: () => { run?.stop?.(); run = null; if (onlyInstalling) onFinish({ ...m, problems: ["Nothing was installed."] }); else showForm().catch((e) => toast(String(e), true)); } }, "Not now"),
          el("button", { class: "btn primary", type: "button", onclick: () => { const again = run.again; run.stop?.(); run = null; startRun(again, onlyInstalling, onFinish); } }, "Try again")));
    } else if (m.stage === "permission") {
      callout.hidden = false;
      callout.replaceChildren(el("div", {}, el("strong", { text: "Choose Yes on Windows' prompt" }),
        el("p", { text: "This is the only time it asks. Don't see it? Look for the shield on your taskbar." })));
    } else {
      callout.hidden = true;
    }
    if (!over && (m.finished || (!m.running && !m.declined && m.problems?.length))) {
      over = true;
      cancelAnimationFrame(raf);
      pct.textContent = m.finished ? "100" : pct.textContent;
      fill.style.transform = m.finished ? "scaleX(1)" : fill.style.transform;
      setTimeout(() => onFinish(m), m.finished ? 450 : 0);
    }
  };
  raf = requestAnimationFrame(tick);
  return draw;
}

/** Runs `call` with the bar on screen. `call` may already be running in the back end. */
function startRun(call, onlyInstalling = false, onFinish = showDone) {
  const mine = { again: call, m: null, stop: null };
  run = mine;
  const draw = showBar(onlyInstalling, (m) => { mine.stop?.(); if (run === mine) run = null; onFinish(m); });
  listen("setup-progress", (e) => { if (run === mine) draw(e.payload); }).then((un) => { if (run === mine) mine.stop = un; else un(); });
  Promise.resolve().then(call).then((m) => { if (run === mine && m) draw(m); }).catch((e) => {
    if (run !== mine) return;
    draw({ running: false, problems: [String(e)], apps: [], stage: "done", percent: 0 });
  });
}

/* ---------- Done ---------- */
async function showDone(m) {
  const ms = await invoke("media_status").catch(() => ({}));
  // A run that stopped short isn't setup done: the next start offers it again.
  if (m.finished) store.settings.setup_done = true;
  const problems = m.problems || [];
  const waiting = m.sites_waiting || [];
  const bits = [];
  if (problems.length) {
    bits.push(el("div", { class: "done-note bad" }, el("strong", { text: "A few things need another try" }),
      el("ul", {}, problems.map((p) => el("li", { text: p }))),
      el("p", { class: "note", text: "Settings > System shows what's left, and Fix everything tries again. Need help? Save logs makes a file to send." }),
      el("div", { style: "margin-top:10px" }, saveLogsButton("btn small ghost"))));
  }
  if (waiting.length) {
    bits.push(el("div", { class: "done-note" }, el("strong", { text: `${m.sites_added} of ${m.sites_total} sources are ready` }),
      el("p", { class: "note", text: `${waiting.join(", ")} didn't answer just now. Chillarr keeps trying by itself.` })));
  }
  if (ms.signed_in) {
    bits.push(el("div", { class: "done-note" }, el("strong", { text: "Watch on your TV or phone" }),
      el("p", { class: "note", text: "Install the free Jellyfin app on it. On the same Wi-Fi it finds this PC as \"Chillarr\". If it asks for an address, use:" }),
      el("span", { class: "addr selectable", text: ms.address || "http://this-pc:8096" }),
      el("p", { class: "note", text: "More subtitle sources are in Settings if you want them." })));
  }
  const start = m.finished
    ? el("button", { class: "btn primary large", type: "button", onclick: close }, "Start watching")
    : el("button", { class: "btn primary large", type: "button", onclick: () => showForm().catch((e) => toast(String(e), true)) }, "Try again");
  root.replaceChildren(el("div", { class: "setup-inner done-screen" }, logoImg("welcome-logo"),
    el("h1", { id: "setup-title", text: problems.length ? "Nearly there" : "You're all set" }),
    el("p", { class: "lead", text: "Pick something and press Download or Watch now." }),
    ...bits, el("div", { class: "setup-go" }, start)));
  start.focus();
}

function close() {
  root.hidden = true;
  document.body.style.overflow = "";
  window.dispatchEvent(new Event("chillarr:refresh"));
}

export function openSetup() {
  root.hidden = false;
  document.body.style.overflow = "hidden";
  state.loaded = false;
  showForm().catch((e) => toast(String(e), true));
}

/** Install buttons in Settings: the same bar, for just these apps. Resolves when it's done. */
export function installWithMeter(ids) {
  root.hidden = false;
  document.body.style.overflow = "hidden";
  return new Promise((done) => {
    // Hands back where things stand, in case the last event came before the listener.
    startRun(() => invoke("install_apps", { ids }).then(() => invoke("setup_progress")), true, (m) => {
      close();
      const problems = m.problems || [];
      if (problems.length) problems.forEach((p) => toast(p, true));
      else toast("Installed. Connecting it to your other apps now.");
      done(!problems.length);
    });
  });
}

window.addEventListener("keydown", (e) => {
  if (!root.hidden && e.key === "Escape" && !run && store.settings?.setup_done) close();
});
