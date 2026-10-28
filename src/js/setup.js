// The first-run tutorial: apps, folders, Jellyfin account, indexers, subtitles, done.
import { $, el, invoke, LABEL, ORDER, store, toast } from "./api.js";
import { downloadsRow, folderPicker, loginSummary, offerInstall, suggestions } from "./settings.js";

const root = $("#setup");
const STEPS = ["welcome", "apps", "folders", "account", "indexers", "subtitles", "done"];
let step = 0;
const state = { movies: null, series: null, mode: "one", base: null, downloads: null };

function frame(title, lead, body, { next = "Continue", onNext, back = true, skip = false } = {}) {
  const bar = el("div", { class: "steps" }, STEPS.map((_, i) => el("span", { class: i <= step ? "done" : "" })));
  const err = el("p", { class: "err" });
  const nextBtn = el("button", { class: "btn primary" }, next);
  nextBtn.addEventListener("click", async () => {
    err.textContent = "";
    nextBtn.disabled = true;
    try {
      if (onNext) await onNext();
      if (step < STEPS.length - 1) go(step + 1);
    } catch (e) {
      err.textContent = String(e);
      nextBtn.disabled = false;
    }
  });
  const left = el("div", {});
  if (back && step > 0) left.append(el("button", { class: "btn ghost", onclick: () => go(step - 1) }, "Back"));
  const right = el("div", { style: "display:flex;gap:10px" });
  if (skip) right.append(el("button", { class: "btn ghost", onclick: () => go(step + 1) }, "Skip for now"));
  right.append(nextBtn);
  root.replaceChildren(el("div", { class: "setup-inner" }, bar,
    step > 0 && step < STEPS.length - 1 ? el("p", { class: "step-count", text: `Step ${step} of ${STEPS.length - 2}` }) : null,
    el("h1", { id: "setup-title", text: title }), lead ? el("p", { class: "lead", text: lead }) : null,
    body, err, el("div", { class: "footer" }, left, right)));
  nextBtn.focus();
}

const views = {
  welcome() {
    frame("Welcome to Chillarr", "", el("div", {},
      el("img", { src: "logo.svg", alt: "", class: "welcome-logo" }),
      el("p", { class: "lead", text: "Chillarr keeps your download apps running, finds things to watch, and plays them, all in one place. This takes about two minutes: we'll check your apps, pick where movies and series go, set up Jellyfin for watching, and add a couple of good sources." })),
      { next: "Let's go", back: false });
  },

  async apps() {
    const list = el("ul", { class: "check-list" });
    const draw = () => list.replaceChildren(...ORDER.map((id) => {
      const c = store.cards.get(id) || { level: "unknown", line: "Checking..." };
      const act = c.missing ? el("button", { class: "btn small primary", onclick: async () => { await offerInstall([id]); draw(); } }, "Install") : null;
      return el("li", { dataset: { level: c.level }, style: `--j:${ORDER.indexOf(id)}` }, el("span", { class: "dot" }),
        el("div", {}, el("strong", { text: LABEL[id] }), el("small", { text: c.missing ? "Not installed" : c.line })), act);
    }));
    draw();
    const unsub = store.on((w) => { if (w === "cards") draw(); });
    const missing = ORDER.filter((id) => store.cards.get(id)?.missing);
    const all = missing.length > 1 ? el("button", { class: "btn ghost", style: "margin-top:14px", onclick: () => offerInstall(missing) }, `Install all ${missing.length}`) : null;
    frame("Your apps", "These six work together. Anything missing can be installed from its official source with one click.", el("div", {}, list, all), {
      onNext: async () => { unsub(); },
    });
    // Leaving with Back also stops listening.
    root.querySelector(".footer .btn.ghost")?.addEventListener("click", unsub, { once: true });
  },

  async folders() {
    if (!state.movies) {
      const f = await invoke("setup_folders");
      state.movies = [...f.movies];
      state.series = [...f.series];
      // Already set up: show those folders instead of suggesting one place.
      if (state.movies.length || state.series.length) state.mode = "custom";
    }
    const join = (base, name) => `${base.replace(/[\\/]+$/, "")}\\${name}`;
    const modes = el("div", { class: "segmented", role: "group", "aria-label": "Where things go" },
      el("button", { class: state.mode === "one" ? "on" : "", onclick: () => { state.mode = "one"; views.folders(); } }, "Everything in one place"),
      el("button", { class: state.mode === "custom" ? "on" : "", onclick: () => { state.mode = "custom"; views.folders(); } }, "Choose each folder"));
    let body;
    if (state.mode === "one") {
      const where = el("p", { class: "path", text: state.base || "No folder picked yet" });
      const plan = el("ul", { class: "folder-plan" });
      const drawPlan = () => plan.replaceChildren(...(state.base ? [["Movies", "Movies"], ["Anime films", "Anime Movies"], ["Series", "Series"], ["Anime", "Anime"], ["Downloads", "Downloads"]]
        .map(([what, sub]) => el("li", {}, el("b", { text: what }), el("span", { text: join(state.base, sub) }))) : []));
      drawPlan();
      const pick = el("button", { class: "btn ghost" }, "Pick a folder");
      pick.addEventListener("click", async () => { const p = await invoke("pick_folder"); if (p) { state.base = p; where.textContent = p; drawPlan(); } });
      body = el("div", { class: "folder-step" }, modes,
        el("p", { class: "note", text: "Pick one folder, for example D:\\Media. Chillarr makes a folder inside it for movies, series, anime and downloads." }),
        el("div", { style: "display:flex;gap:12px;align-items:center;flex-wrap:wrap" }, pick, where), plan,
        el("div", { class: "tip" }, el("span", { text: "Everything on one drive means a finished download moves into your library instantly and takes no extra space." })));
    } else {
      body = el("div", { class: "folder-step" }, modes,
        folderPicker("Movies", "For example D:\\Media\\Movies. Add a second one named Anime for anime films.", state.movies),
        folderPicker("Series and anime", "For example D:\\Media\\Series, plus D:\\Media\\Anime for anime.", state.series),
        downloadsRow((p) => { state.downloads = p; }),
        el("div", { class: "tip" }, el("span", { text: "Tip: put these on the same drive as your downloads. Then moving a finished download is instant and takes no extra space." })));
    }
    frame("Where should things go?", "Finished downloads are sorted into these folders by name. A folder with \"Anime\" in its name gets anime.", body, {
      onNext: async () => {
        if (state.mode === "one") {
          if (!state.base) throw "Pick a folder first.";
          state.movies = [join(state.base, "Movies"), join(state.base, "Anime Movies")];
          state.series = [join(state.base, "Series"), join(state.base, "Anime")];
          state.downloads = join(state.base, "Downloads");
        }
        if (!state.movies.length || !state.series.length) throw "Add at least one movie folder and one series folder.";
        await invoke("setup_set_folders", { movies: state.movies, series: state.series });
        if (state.downloads) await invoke("set_download_folder", { path: state.downloads });
      },
    });
  },

  async account() {
    const ms = await invoke("media_status");
    if (!ms.installed) {
      frame("Install Jellyfin to watch here", "Jellyfin plays your movies and series in Chillarr and streams them to your phone or TV. It's free.",
        el("button", { class: "btn primary", onclick: async () => { if (await offerInstall(["Jellyfin"])) views.account(); } }, "Install Jellyfin"), { skip: true, next: "Continue" });
      return;
    }
    // One name and password for everything: Jellyfin's account, and the
    // login qBittorrent, Sonarr, Radarr and Prowlarr would otherwise each ask
    // for on their own first screen.
    const user = el("input", { type: "text", autocomplete: "username", value: ms.signed_in ? ms.user || "" : "" });
    const pass = el("input", { type: "password", autocomplete: ms.signed_in || !ms.setup_done ? "new-password" : "current-password" });
    const title = ms.signed_in ? "One login for your apps" : ms.setup_done ? "Sign in once for everything" : "One login for everything";
    const lead = ms.signed_in
      ? `Jellyfin is ready as ${ms.user}. Pick the name and password qBittorrent, Sonarr, Radarr and Prowlarr should ask for, so none of them stops at its own login screen.`
      : ms.setup_done
        ? "Jellyfin already has an account on this PC: sign in with it. qBittorrent, Sonarr, Radarr and Prowlarr get the same name and password."
        : "Pick a name and password. It's your Jellyfin account (use it on your phone or TV too), and qBittorrent, Sonarr, Radarr and Prowlarr get the same one, so none of them stops at its own login screen.";
    frame(title, lead,
      el("div", { style: "display:grid;gap:14px;max-width:420px" },
        el("label", { class: "field" }, el("small", { text: "Name" }), user),
        el("label", { class: "field" }, el("small", { text: "Password (at least 5 characters)" }), pass)),
      {
        skip: true,
        next: ms.signed_in ? "Use this login" : ms.setup_done ? "Sign in" : "Create login",
        onNext: async () => {
          const name = user.value.trim();
          if (!name || pass.value.length < 5) throw "Enter a name and a password of at least 5 characters.";
          if (!ms.signed_in) await invoke("media_connect", { user: name, password: pass.value });
          const done = await invoke("apps_set_login", { user: name, password: pass.value });
          const summary = loginSummary(done);
          if (summary) toast(summary);
        },
      });
  },

  async indexers() {
    frame("Add good sources", "Indexers are the sites Chillarr searches when you press Download. Press Add all for the full set: big sites for movies and series, and the best anime sites.",
      await suggestions(), { next: "Continue", skip: false });
  },

  // Optional: an OpenSubtitles.com login, handed to Jellyfin's plugin (subs.rs).
  async subtitles() {
    const ms = await invoke("media_status").catch(() => ({}));
    const st = ms.signed_in ? await invoke("subs_status").catch(() => ({})) : {};
    const lead = "When something has no subtitles, or none in your language, Chillarr finds them for you. A free OpenSubtitles account finds the most, and lets your TV and phone search for subtitles too. Without one, Chillarr still uses free sources, just fewer.";
    if (!ms.signed_in) {
      frame("Subtitles", lead, el("p", { class: "note", text: "This needs Jellyfin, so it's skipped for now. You can turn it on later in Settings." }), { next: "Continue" });
      return;
    }
    if (st.ready) {
      frame("Subtitles", lead, el("div", { class: "tip" }, el("span", { text: `On, using OpenSubtitles as ${st.user}.` })), { next: "Continue" });
      return;
    }
    const user = el("input", { type: "text", autocomplete: "username", value: ms.user || "" });
    const pass = el("input", { type: "password", autocomplete: "current-password" });
    const make = el("button", { class: "btn ghost", onclick: () => invoke("open_link", { which: "opensubtitles" }).catch((e) => toast(String(e), true)) }, "Make a free account");
    frame("Subtitles", lead,
      el("div", { style: "display:grid;gap:14px;max-width:420px" },
        el("p", { class: "note", text: "No account yet? Make one on their site (it's free), then come back and sign in here. Using the same name and password as Chillarr keeps it simple." }),
        el("div", {}, make),
        el("label", { class: "field" }, el("small", { text: "OpenSubtitles.com name" }), user),
        el("label", { class: "field" }, el("small", { text: "Password" }), pass)),
      {
        skip: true,
        next: st.installed ? "Turn on" : "Turn on (Jellyfin restarts once)",
        onNext: async () => {
          if (!user.value.trim() || !pass.value) throw "Enter your OpenSubtitles.com name and password, or press Skip for now.";
          toast(await invoke("subs_setup", { username: user.value.trim(), password: pass.value }));
        },
      });
  },

  async done() {
    const s = store.settings;
    s.setup_done = true;
    invoke("set_settings", { s }).catch(() => {});
    const ms = await invoke("media_status").catch(() => ({}));
    const tv = ms.signed_in ? el("div", { class: "tip", style: "flex-direction:column;align-items:flex-start;gap:6px" },
      el("b", { text: "Watch on your TV or phone" }),
      el("span", { text: "Install the free Jellyfin app on it. On the same Wi-Fi it finds this PC by itself as \"Chillarr\"; if it asks for an address, use:" }),
      el("span", { class: "addr selectable", text: ms.address || "http://this-pc:8096" }),
      el("span", { text: "Sign in with the Jellyfin account you just made. Discover Movies and Discover Series are there too: press the heart on anything and this PC gets it." })) : el("div");
    frame("All set", "Chillarr is checking everything one last time. Pick something to watch, or press Download on anything that looks good.", tv, {
      next: "Start watching",
      back: false,
      onNext: async () => {
        invoke("fix_everything").catch(() => {});
        close();
      },
    });
  },
};

function go(i) {
  step = Math.max(0, Math.min(STEPS.length - 1, i));
  views[STEPS[step]]();
}

function close() {
  root.hidden = true;
  document.body.style.overflow = "";
  window.dispatchEvent(new Event("chillarr:refresh"));
}

export function openSetup() {
  root.hidden = false;
  document.body.style.overflow = "hidden";
  state.movies = null;
  go(0);
}

window.addEventListener("keydown", (e) => {
  if (!root.hidden && e.key === "Escape" && store.settings?.setup_done) close();
});
