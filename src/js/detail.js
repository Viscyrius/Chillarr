// The detail panel: what a title is, Play / Resume / Download, My List, and episodes.
import { $, ask, canDelete, closeDialog, el, icon, ICON, invoke, listen, metaBits, store, toast } from "./api.js";
import { canWatchNow, isWatchNow, movieLike, play, removeWatchNow, watchNow } from "./player.js";
import { completedActions, confirmRedownload, isWatched, listable, loadWatched, onMyList, setMyList } from "./mylist.js";
import { subtitlesControl } from "./subpick.js";

const dlg = $("#detail");
const shut = () => {
  stopTrailer();
  closeDialog(dlg);
};
$("#detail-close").addEventListener("click", shut);
dlg.addEventListener("click", (e) => { if (e.target === dlg) shut(); });
// Esc while a trailer plays goes back to the title; otherwise it closes.
dlg.addEventListener("cancel", (e) => { e.preventDefault(); if ($("#detail-hero").classList.contains("playing")) stopTrailer(); else shut(); });

let episodes = [];

function renderEpisodes(season) {
  const list = episodes.filter((e) => (e.subtitle || "").startsWith(`S${season} `));
  $("#episode-list").replaceChildren(
    ...list.map((ep, i) => {
      const num = (ep.subtitle || "").match(/E(\d+)/)?.[1] || String(i + 1);
      const name = (ep.subtitle || "").split(" · ").slice(1).join(" · ") || ep.title;
      const thumb = el("div", { class: "thumb" });
      if (ep.backdrop || ep.poster) thumb.append(el("img", { src: ep.backdrop || ep.poster, alt: "", loading: "lazy" }));
      if (ep.progress) thumb.append(el("div", { class: "bar" }, el("span", { style: `width:${Math.round(ep.progress * 100)}%` })));
      const li = el("li", { class: "ep", tabindex: "0", role: "button", "aria-label": `Play ${name}` },
        el("span", { class: "num", text: num }), thumb,
        el("div", {}, el("h4", { text: name }), el("p", { text: ep.overview || "" })));
      li.style.setProperty("--j", Math.min(i, 12));
      const go = () => { shut(); play(ep, episodes); };
      li.addEventListener("click", go);
      li.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); go(); } });
      return li;
    })
  );
}

let openSeq = 0;

async function loadEpisodes(seriesId) {
  const seq = openSeq;
  const list = await invoke("media_episodes", { seriesId });
  if (seq !== openSeq) return null;
  episodes = list;
  const seasons = [...new Set(episodes.map((e) => (e.subtitle || "").match(/^S(\d+)/)?.[1]).filter(Boolean))];
  const pick = $("#season-pick");
  pick.replaceChildren(...seasons.map((s) => el("option", { value: s, text: s === "0" ? "Specials" : `Season ${s}` })));
  // Start on the first season with something unwatched.
  const firstOpen = episodes.find((e) => (e.progress || 0) < 0.95);
  const start = firstOpen?.subtitle?.match(/^S(\d+)/)?.[1] || seasons[0];
  if (start) pick.value = start;
  $("#detail-episodes").hidden = !episodes.length;
  if (start) renderEpisodes(start);
  return firstOpen;
}

/* ---------- A show: every episode, and where you're at ---------- */
// From media_show_episodes: every season (Sonarr's list, or skyhook's), each
// episode's state on this PC, ticks for what you've finished, and the spot
// Chillarr keeps for this profile. Rows only play; the spot moves on its own
// as you finish episodes, or on purpose from "Where you're at".
let show = null;

const tag = (r) => `S${r.season} E${r.episode}`;
const epName = (r) => r.title || `Episode ${r.episode}`;
const seasonName = (n) => (n === 0 ? "Specials" : `Season ${n}`);
const atSpot = (r) => !!show?.data.spot && r.season === show.data.spot.season && r.episode === show.data.spot.episode;
const spotRow = () => show?.data.episodes.find(atSpot) || null;
const playable = () => show.data.episodes.filter((r) => r.tile).map((r) => r.tile);
/**
 * A show's Play / Resume from a hover card: the episode you're at (as the
 * panel's big button picks it), played without opening the panel. False when
 * that episode isn't downloaded; the panel is the way then.
 */
export async function playShowNow(t) {
  const data = await invoke("media_show_episodes", { tile: t }).catch(() => null);
  if (!data?.episodes.length) return false;
  const here = (r) => !!data.spot && r.season === data.spot.season && r.episode === data.spot.episode;
  const target = data.episodes.find(here) || data.episodes.find((r) => r.season > 0) || data.episodes[0];
  if (!target?.tile) return false;
  play(target.tile, data.episodes.filter((r) => r.tile).map((r) => r.tile));
  return true;
}

/** The show as Watch now and Download want it: its own tile, with its TVDB id. */
const showTile = () => { const t = show.data.show || show.tile; return { ...t, tvdb: show.data.tvdb ?? t.tvdb }; };
/** Watch now on one episode of the open show (it plays at once, so the panel closes). */
const watchEpisode = (r, opts = {}) => { shut(); watchNow(showTile(), { ...opts, season: r.season, episode: r.episode }); };

function stateText(r) {
  if (r.state === "downloaded") return "Downloaded";
  if (r.state === "downloading") return `Downloading ${Math.round((r.pct || 0) * 100)}%`;
  if (r.state === "half") return "Half watched";
  if (!r.aired) return r.air_date ? `Airs ${r.air_date}` : "Not out yet";
  return "Not downloaded";
}

/** Gets one episode, the quality last picked. */
async function downloadEpisode(r) {
  let quality = "best";
  try { quality = localStorage.getItem("chillarr-quality") || "best"; } catch (e) { /* storage blocked */ }
  const msg = await invoke("media_get_picked", { tile: showTile(), quality, pick: { seasons: [], episodes: [[r.season, r.episode]] } });
  toast(`${msg} You'll get a notification when it's ready.`);
  store.emit("added");
}

/**
 * What an episode that isn't on this PC offers, in order, as { id, label, icon, run }.
 * `plays`: it opens the player (the panel closes) instead of asking for a download.
 */
export function episodeActions(r) {
  const out = [];
  if (show?.data.tvdb) out.push({ id: "watchnow", label: "Watch now", icon: ICON.play, plays: true, run: () => watchEpisode(r) });
  out.push({ id: "download", label: "Download", icon: ICON.download, run: () => downloadEpisode(r) });
  return out;
}

function episodeRow(r, i) {
  const here = atSpot(r);
  const thumb = el("div", { class: "thumb" });
  if (r.still) thumb.append(el("img", { src: r.still, alt: "", loading: "lazy", decoding: "async" }));
  if (r.progress) thumb.append(el("div", { class: "bar" }, el("span", { style: `width:${Math.round(r.progress * 100)}%` })));
  if (r.played) thumb.append(el("span", { class: "ep-tick", title: "Watched", "aria-label": "Watched" }, icon(ICON.check)));
  if (r.tile) thumb.append(el("span", { class: "ep-play", "aria-hidden": "true" }, icon(ICON.play)));
  const state = el("span", { class: `ep-state ${r.aired ? r.state : "later"}` }, r.state === "downloading" ? el("i", { style: `--p:${Math.round((r.pct || 0) * 100)}%` }) : null, stateText(r));
  const foot = el("div", { class: "ep-foot" }, state);
  if (!r.tile && r.aired && r.state === "missing") {
    for (const a of episodeActions(r)) {
      const b = el("button", { class: `btn small ${a.plays ? "soft wn-btn" : "ghost"}` }, icon(a.icon), a.label);
      b.addEventListener("click", async (e) => {
        e.stopPropagation();
        if (a.plays) { a.run(); return; }
        b.disabled = true;
        b.lastChild.textContent = "Adding...";
        try { await a.run(); b.lastChild.textContent = "Asked for"; state.lastChild.textContent = "Looking for a copy"; }
        catch (err) { toast(String(err), true); b.disabled = false; b.lastChild.textContent = a.label; }
      });
      foot.append(b);
    }
  }
  const li = el("li", { class: `ep${r.tile || r.watch_key ? "" : " off"}${here ? " here" : ""}${r.played ? " seen" : ""}`, dataset: { s: String(r.season), e: String(r.episode) } },
    el("span", { class: "num", text: String(r.episode) }), thumb,
    el("div", { class: "ep-body" },
      el("div", { class: "ep-title" }, el("h4", { text: epName(r) }), here ? el("span", { class: "here-tag", text: "You're here" }) : null),
      r.overview ? el("p", { text: r.overview }) : null, foot));
  li.style.setProperty("--j", Math.min(i, 12));
  if (r.tile || r.watch_key) {
    li.tabIndex = 0;
    li.setAttribute("role", "button");
    li.setAttribute("aria-label", `${r.tile ? "Play" : "Resume"} ${tag(r)} ${epName(r)}`);
    // A Watch now copy picks up where you stopped, its download started again.
    const go = r.tile ? () => { shut(); play(r.tile, playable()); } : () => watchEpisode(r);
    li.addEventListener("click", go);
    li.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); go(); } });
  }
  return li;
}

function drawSeason(season, toSpot = false) {
  show.view = season;
  $("#season-pick").value = String(season);
  syncRestartSeason();
  const list = $("#episode-list");
  list.replaceChildren(...show.data.episodes.filter((r) => r.season === season).map(episodeRow));
  if (toSpot) {
    const here = list.querySelector(".ep.here");
    // After the panel has opened, so the scroll is seen.
    if (here) setTimeout(() => { if (here.isConnected) here.scrollIntoView({ behavior: "smooth", block: "center" }); }, 420);
  }
}

/**
 * The big button: Play or Resume where you're at. Not here yet: Watch now
 * first, Download beside it; already downloading: where it is, and Watch now.
 */
function drawMain() {
  const slot = show.main;
  const target = spotRow() || show.data.episodes.find((r) => r.season > 0) || show.data.episodes[0];
  slot.replaceChildren();
  if (!target) return;
  const wn = !!show.data.tvdb;
  if (target.tile) {
    slot.append(el("button", { class: "btn play", onclick: () => { shut(); play(target.tile, playable()); } }, icon(ICON.play), `${target.progress ? "Resume" : "Play"} ${tag(target)}`));
    otherPlayer(slot, target.tile, playable());
  } else if (target.watch_key) {
    slot.append(el("button", { class: "btn play", onclick: () => watchEpisode(target) }, icon(ICON.play), `${target.progress ? "Resume" : "Watch now"} ${tag(target)}`));
    slot.append(el("span", { class: "status-pill", text: stateText(target) }));
  } else if (target.aired && target.state === "missing") {
    if (wn) slot.append(el("button", { class: "btn play", onclick: () => watchEpisode(target) }, icon(ICON.play), `Watch now ${tag(target)}`));
    const b = el("button", { class: wn ? "btn soft" : "btn play" }, icon(ICON.download), `Download ${tag(target)}`);
    b.addEventListener("click", async () => {
      b.disabled = true;
      try { await downloadEpisode(target); b.lastChild.textContent = `${tag(target)} asked for`; } catch (e) { toast(String(e), true); b.disabled = false; }
    });
    slot.append(b);
  } else {
    slot.append(el("span", { class: "status-pill", text: `${tag(target)}: ${stateText(target)}` }));
    if (wn && target.state === "downloading") slot.append(el("button", { class: "btn soft", onclick: () => watchEpisode(target) }, icon(ICON.play), "Watch now"));
  }
}

/** "Where you're at": the spot as words, and the only ways to move it on purpose. */
function drawWhere() {
  const box = $("#detail-where");
  if (!show.data.tvdb || !show.data.episodes.some((r) => r.season > 0)) { box.hidden = true; return; }
  const spot = spotRow();
  const restart = async (season) => {
    const what = season == null ? show.tile.title : `season ${season}`;
    const ok = await ask(season == null ? `Start ${show.tile.title} over?` : `Start season ${season} over?`,
      `Every episode of ${what} goes back to unwatched for this profile, and you start from its first episode. Other profiles keep theirs.`, [], "Start over");
    if (!ok) return;
    try { toast(await invoke("marker_restart", { tvdb: show.data.tvdb, season })); } catch (e) { toast(String(e), true); }
    await reloadShow(season ?? null);
  };
  const picker = el("div", { class: "where-pick", hidden: true });
  const pickBtn = el("button", { class: "btn soft small", "aria-expanded": "false" }, "Pick a different episode");
  pickBtn.addEventListener("click", () => {
    const open = picker.hidden;
    picker.hidden = !open;
    pickBtn.setAttribute("aria-expanded", String(open));
    if (open) drawPicker(picker, spot?.season ?? show.view);
  });
  box.replaceChildren(
    el("div", { class: "where-head" },
      el("div", {}, el("h3", { text: "Where you're at" }),
        el("p", { class: "where-now" }, spot ? [el("b", { text: tag(spot) }), ` · ${epName(spot)}`] : "You haven't started this show yet.")),
      el("div", { class: "where-acts" },
        el("button", { class: "btn soft small", onclick: () => restart(null) }, "Start the show over"),
        show.restartSeason = el("button", { class: "btn soft small", onclick: () => restart(show.view) }),
        pickBtn)),
    picker);
  syncRestartSeason();
  box.hidden = false;
}

/** "Start season N over" is about the season on screen. */
function syncRestartSeason() {
  const b = show?.restartSeason;
  if (!b) return;
  b.hidden = !(show.view > 0);
  b.textContent = `Start season ${show.view} over`;
}

/** A compact list of the show's episodes; choosing one asks once, then it's where you're at. */
function drawPicker(box, season) {
  const seasons = show.data.seasons.filter((s) => s > 0);
  const tabs = el("div", { class: "where-tabs" }, seasons.map((s) => el("button", { class: `chip${s === season ? " on" : ""}`, onclick: () => drawPicker(box, s) }, seasonName(s))));
  const list = el("ol", { class: "where-list" }, show.data.episodes.filter((r) => r.season === season).map((r) => {
    const b = el("button", { class: `where-ep${atSpot(r) ? " on" : ""}${r.played ? " seen" : ""}` },
      el("span", { class: "n", text: `E${r.episode}` }), el("span", { class: "t", text: epName(r) }), r.played ? icon(ICON.check) : null);
    b.addEventListener("click", async () => {
      if (atSpot(r)) return;
      const ok = await ask(`Start from ${tag(r)}?`, `Play and Resume go to ${tag(r)} · ${epName(r)} from now on. Nothing is marked watched or unwatched.`, [], "Set as where I'm at");
      if (!ok) return;
      try { await invoke("marker_set", { tvdb: show.data.tvdb, season: r.season, episode: r.episode }); toast(`You're at ${tag(r)} now.`); }
      catch (e) { toast(String(e), true); return; }
      await reloadShow(r.season);
    });
    return el("li", {}, b);
  }));
  box.replaceChildren(tabs, list);
}

function drawShow(season = null) {
  const seasons = show.data.seasons;
  $("#season-pick").replaceChildren(...seasons.map((s) => el("option", { value: String(s), text: seasonName(s) })));
  $("#detail-episodes").hidden = !show.data.episodes.length;
  const start = season ?? show.data.spot?.season ?? seasons.find((s) => s > 0) ?? seasons[0];
  if (start != null) drawSeason(start, season == null);
  drawMain();
  drawWhere();
}

/** Asks again after the spot moved on purpose; the list stays on `season`. */
async function reloadShow(season) {
  const seq = openSeq;
  const data = await invoke("media_show_episodes", { tile: show.tile }).catch((e) => { toast(String(e), true); return null; });
  if (!data || seq !== openSeq) return;
  show.data = data;
  drawShow(season ?? data.spot?.season ?? null);
  $("#episode-list .ep.here")?.scrollIntoView({ behavior: "smooth", block: "center" });
}

/** Loads the show into the panel. False when it can't be listed (the panel then falls back to Jellyfin's own list). */
async function loadShow(t, main) {
  const seq = openSeq;
  const data = await invoke("media_show_episodes", { tile: t }).catch(() => null);
  if (seq !== openSeq || !data || !data.episodes.length) return false;
  show = { tile: t, data, view: null, main };
  // Opened from an episode: the show's own words.
  if (data.show?.overview && !$("#detail-overview").textContent) $("#detail-overview").textContent = data.show.overview;
  drawShow();
  return true;
}
$("#season-pick").addEventListener("change", (e) => (show && dlg.open ? drawSeason(+e.target.value) : renderEpisodes(e.target.value)));

/** The other way to play: "Play in VLC" or "Play in Chillarr". */
function otherPlayer(actions, t, list) {
  const vlcIsDefault = store.settings?.player === "vlc";
  if (!vlcIsDefault && !store.media?.vlc) return;
  const where = vlcIsDefault ? "chillarr" : "vlc";
  actions.append(el("button", { class: "btn soft", onclick: () => { shut(); play(t, list, where); } }, vlcIsDefault ? "Play in Chillarr" : "Play in VLC"));
}

function trailerButton(t) {
  if (!t.trailer) return null;
  return el("button", { class: "btn soft trailer-btn", onclick: () => {
    if ($("#detail-hero .trailer")) return;
    const frame = el("iframe", {
      class: "trailer",
      src: `https://www.youtube-nocookie.com/embed/${encodeURIComponent(t.trailer)}?autoplay=1&rel=0&modestbranding=1`,
      allow: "autoplay; encrypted-media; fullscreen",
      allowfullscreen: true,
      title: `${t.title} trailer`,
    });
    const back = el("button", { class: "trailer-back", onclick: stopTrailer }, icon(ICON.left), "Back");
    $("#detail-hero").prepend(frame, back);
    $("#detail-hero").classList.add("playing");
    back.focus();
  } }, "Trailer");
}

/** Ends the trailer and brings back the title, Download and the rest. */
function stopTrailer() {
  const hero = $("#detail-hero");
  hero.querySelector(".trailer")?.remove();
  hero.querySelector(".trailer-back")?.remove();
  hero.classList.remove("playing");
}

/* ---------- What to get: whole show, whole seasons, single episodes ---------- */
let pickState = null;

/** `all` is the whole show (an empty pick asks the backend for that); nothing ticked is told apart by it. */
function currentPick() {
  if (!pickState) return { all: false, seasons: [], episodes: [] };
  if (pickState.all) return { all: true, seasons: [], episodes: [] };
  return { all: false, seasons: [...pickState.seasons], episodes: [...pickState.eps].flatMap(([s, set]) => [...set].map((e) => [s, e])) };
}

function pickLabel(p) {
  const s = p.seasons.length, e = p.episodes.length;
  if (p.all) return "Download whole show";
  if (!s && !e) return "Pick what to get";
  const parts = [];
  if (s) parts.push(s === 1 ? `season ${p.seasons[0]}` : `${s} seasons`);
  if (e) parts.push(e === 1 ? "1 episode" : `${e} episodes`);
  return `Download ${parts.join(" + ")}`;
}

/**
 * Lists the show's seasons and episodes to tick. Nothing starts ticked:
 * Whole show takes everything, a season's top box takes that season (and its
 * new episodes), single boxes take single episodes. Calls onChange on every change.
 */
async function loadPicker(t, onChange, foot = null) {
  const seq = openSeq;
  const box = $("#detail-pick");
  const title = el("h3", { text: t.jf_id ? "Get more of this show" : "What to get" });
  box.replaceChildren(el("div", { class: "ep-pick-head" }, title), el("p", { class: "note", text: "Loading seasons..." }));
  box.hidden = false;
  let data;
  // No season list: a show you don't have yet can still be got whole; one you have shows nothing.
  const bail = () => {
    if (seq !== openSeq) return;
    box.hidden = true;
    if (!t.jf_id) { pickState = { all: true }; onChange(); }
  };
  try { data = await invoke("media_seasons", { tile: t }); } catch (e) { bail(); return; }
  if (seq !== openSeq) return;
  const seasons = data.seasons.filter((s) => s.episodes.length);
  if (!seasons.length) { bail(); return; }
  const name = (n) => (n === 0 ? "Specials" : `Season ${n}`);
  const wanted = (s) => s.episodes.filter((e) => !e.has_file).map((e) => e.number);
  const open = seasons.filter((s) => wanted(s).length);
  if (!open.length && t.jf_id) {
    // Everything out so far is here: nothing to offer.
    box.hidden = true;
    return;
  }
  // Start on the season a "Season 3" anime entry names, else the first with something missing.
  const state = { all: false, seasons: new Set(), eps: new Map(), view: data.suggested ?? (open[0] || seasons[0]).number };
  pickState = state;

  // Changing one part of the whole show: everything else stays ticked.
  function leaveWhole() {
    if (!state.all) return;
    state.all = false;
    state.seasons = new Set(open.map((s) => s.number));
  }
  function setEpisode(s, n, on) {
    leaveWhole();
    let set = state.eps.get(s.number);
    if (state.seasons.has(s.number)) {
      // Unticking one episode of a whole season: the rest become single picks.
      state.seasons.delete(s.number);
      set = new Set(wanted(s));
    }
    set = set || new Set();
    if (on) set.add(n); else set.delete(n);
    if (set.size) state.eps.set(s.number, set); else state.eps.delete(s.number);
    redraw();
  }
  function setSeason(s, on) {
    leaveWhole();
    state.eps.delete(s.number);
    if (on) state.seasons.add(s.number); else state.seasons.delete(s.number);
    redraw();
  }

  const wholeBtn = el("button", { class: "ep-whole", onclick: () => { state.all = !state.all; state.seasons.clear(); state.eps.clear(); redraw(); } });
  const tabs = el("div", { class: "ep-tabs", role: "tablist" });
  const panel = el("div", { class: "ep-panel" });
  // Kept across redraws so ticking an episode doesn't scroll the list back to E1 or lose focus.
  const list = el("ol", { class: "ep-list" });
  let shownView = null, listView = null;
  const summary = el("p", { class: "ep-summary" });

  function drawWhole() {
    wholeBtn.classList.toggle("on", state.all);
    wholeBtn.setAttribute("aria-pressed", String(state.all));
    wholeBtn.replaceChildren(el("span", { class: "ep-box" }, state.all ? icon(ICON.check) : null), "Whole show");
  }

  function drawTabs() {
    tabs.replaceChildren(...seasons.map((s) => {
      const left = wanted(s).length;
      const full = state.all ? left > 0 : state.seasons.has(s.number);
      const some = state.eps.get(s.number)?.size || 0;
      const note = !left ? "Downloaded" : full ? "All" : some ? `${some} of ${left}` : left < s.episodes.length ? `${left} to get` : `${left} episodes`;
      const tab = el("button", { class: `ep-tab${state.view === s.number ? " viewing" : ""}${full || some ? " on" : ""}${!left ? " have" : ""}`, role: "tab",
        "aria-selected": String(state.view === s.number), onclick: () => { state.view = s.number; redraw(); } },
        el("b", { text: name(s.number) }), el("small", { text: note }));
      return tab;
    }));
    // Only on a season change, and only the strip: scrollIntoView would move the dialog too.
    if (shownView !== state.view) {
      shownView = state.view;
      const tab = tabs.querySelector(".viewing");
      if (tab && (tab.offsetLeft < tabs.scrollLeft || tab.offsetLeft + tab.offsetWidth > tabs.scrollLeft + tabs.clientWidth)) {
        tabs.scrollTo({ left: tab.offsetLeft - 8, behavior: "smooth" });
      }
    }
  }

  function drawPanel() {
    const s = seasons.find((x) => x.number === state.view) || seasons[0];
    const left = wanted(s).length;
    const whole = state.all ? left > 0 : state.seasons.has(s.number);
    const picked = state.eps.get(s.number) || new Set();
    const allBox = el("input", { type: "checkbox", checked: whole || !left, disabled: !left });
    allBox.addEventListener("change", () => setSeason(s, allBox.checked));
    const top = el("label", { class: `ep-all${!left ? " have" : ""}` }, allBox,
      el("span", {}, el("b", { text: left ? `All of ${name(s.number).toLowerCase()}` : `${name(s.number)} is downloaded` }),
        el("small", { text: left ? `${left} to get, plus new ones as they air` : "Every episode out so far is here" })));
    const rows = s.episodes.map((e) => {
      const box = el("input", { type: "checkbox", checked: e.has_file || whole || picked.has(e.number), disabled: e.has_file, dataset: { ep: String(e.number) } });
      box.addEventListener("change", () => setEpisode(s, e.number, box.checked));
      const when = e.has_file ? "Downloaded" : !e.aired && e.air_date ? `Airs ${e.air_date}` : "";
      return el("li", { class: e.has_file ? "have" : "" }, el("label", {}, box,
        el("span", { class: "n", text: `E${e.number}` }), el("span", { class: "t", text: e.title || `Episode ${e.number}` }), when ? el("span", { class: "when", text: when }) : null));
    });
    const focused = list.contains(document.activeElement) ? document.activeElement.dataset.ep : null;
    const keep = listView === s.number ? list.scrollTop : 0;
    // The list stays in the page (detaching it would reset its scroll); only its rows and the top box change.
    if (list.isConnected) panel.replaceChild(top, panel.firstElementChild); else panel.replaceChildren(top, list);
    list.replaceChildren(...rows);
    list.scrollTop = keep;
    listView = s.number;
    if (focused) list.querySelector(`input[data-ep="${focused}"]`)?.focus({ preventScroll: true });
  }

  function drawSummary() {
    const p = currentPick();
    summary.textContent = p.all ? "Every episode, and new ones as they air." : !p.seasons.length && !p.episodes.length ? "Nothing picked yet. Tick a season, some episodes, or Whole show." : `${pickLabel(p).replace(/^Download /, "")[0].toUpperCase()}${pickLabel(p).replace(/^Download /, "").slice(1)} picked.`;
  }

  function redraw() { drawWhole(); drawTabs(); drawPanel(); drawSummary(); onChange(); }
  box.replaceChildren(el("div", { class: "ep-pick-head" }, title, wholeBtn), tabs, panel, el("div", { class: "ep-foot" }, summary, foot));
  redraw();
}

/** Asked for and not watchable yet (looking, downloading, or not out). */
function requested(t) {
  const s = t.status || "";
  return s === "Looking for a copy" || s === "Not out yet" || s.startsWith("Downloading");
}

/** Opened from My List > Completed: what's completed, and its actions (Download is already there). */
function completedBits(t, actions) {
  $("#detail-meta").append(el("span", { class: "sep" }), el("span", { class: "owned", text: t.mark ? `${t.completed} · ${t.mark}` : t.completed }));
  // Download and Watch now are already there.
  for (const a of completedActions(t, openDetail).filter((a) => a.id !== "download" && a.id !== "watchnow")) {
    actions.append(el("button", { class: "btn soft", onclick: async () => { await a.run(); shut(); } }, icon(a.icon), a.label));
  }
}

/** Watch now on a show from outside its panel: open it at the episode list, so
 *  you pick the episode. The list loads after the panel opens, so wait for it
 *  (a few seconds at most) and only for this opening. */
function showEpisodes(seq) {
  const until = Date.now() + 6000;
  const tick = () => {
    if (seq !== openSeq) return;
    const box = $("#detail-episodes");
    if (box && !box.hidden && box.querySelector(".ep-row, li, article")) { box.scrollIntoView({ behavior: "smooth", block: "start" }); return; }
    if (Date.now() < until) setTimeout(tick, 150);
  };
  setTimeout(tick, 300);
}

/** Watch now for a title that isn't here: a movie plays, a show opens at its episodes to pick one. */
function watchNowButton(t, primary) {
  if (!canWatchNow(t)) return null;
  return el("button", { class: `btn ${primary ? "play" : "soft"} wn-btn`, onclick: () => { if (movieLike(t)) { shut(); watchNow(t); } else $("#detail-episodes")?.scrollIntoView({ behavior: "smooth", block: "start" }); } }, icon(ICON.play), "Watch now");
}

/** A Watch now title (Continue Watching): Resume or Start over, where its download is, and Remove. */
function watchNowActions(t, actions) {
  actions.append(el("button", { class: "btn play", onclick: () => { shut(); watchNow(t); } }, icon(ICON.play), t.resume_seconds ? "Resume" : "Play"));
  if (t.resume_seconds) actions.append(el("button", { class: "btn soft", onclick: () => { shut(); watchNow(t, { start: 0 }); } }, "Start over"));
  if (t.status) actions.append(el("span", { class: "status-pill", text: t.status }));
  actions.append(el("button", { class: "btn soft danger", onclick: async () => { if (await removeWatchNow(t)) shut(); } }, "Remove"));
}

/** My List: a bookmark that downloads nothing (see mylist.js). */
function myListButton(t) {
  const b = el("button", { class: "btn soft save-btn", title: "Keep it on My List. Nothing downloads." }, icon(ICON.plus), "My List");
  const draw = (on) => { b.replaceChildren(icon(on ? ICON.check : ICON.plus), "My List"); b.classList.toggle("on", on); b.dataset.on = on ? "1" : ""; };
  const seq = openSeq;
  onMyList(t).then((on) => { if (seq === openSeq) draw(on); }).catch(() => {});
  b.addEventListener("click", async () => {
    b.disabled = true;
    try {
      const on = await setMyList(t, !b.dataset.on);
      draw(on);
      toast(on ? `Added ${t.title} to My List.` : `Took ${t.title} off My List.`);
    } catch (e) { toast(String(e), true); }
    b.disabled = false;
  });
  return b;
}

/** A show's Delete: the whole show, or the episodes ticked here (only ones on your PC). */
async function loadDeletePicker(t, wholeDelete) {
  const seq = openSeq;
  const box = $("#detail-pick");
  const title = el("h3", { text: "What to delete" });
  box.replaceChildren(el("div", { class: "ep-pick-head" }, title), el("p", { class: "note", text: "Loading episodes..." }));
  box.hidden = false;
  box.classList.add("deleting");
  box.scrollIntoView({ behavior: "smooth", block: "start" });
  const close = () => { box.hidden = true; box.classList.remove("deleting"); };
  // Cancel puts the panel back as it was (the "Get more" picker included).
  const back = () => { if (seq === openSeq) openDetail(t); };
  let data;
  try { data = await invoke("media_seasons", { tile: t }); } catch (e) { if (seq === openSeq) { close(); wholeDelete(); } return; }
  if (seq !== openSeq) return;
  const seasons = data.seasons.map((s) => ({ ...s, episodes: s.episodes.filter((e) => e.has_file) })).filter((s) => s.episodes.length);
  // Nothing Sonarr knows about on disk (Jellyfin-only copies): the whole show is the only choice.
  if (!seasons.length) { close(); wholeDelete(); return; }
  const name = (n) => (n === 0 ? "Specials" : `Season ${n}`);
  const state = { all: false, picked: new Map(), view: seasons[0].number };
  const count = () => [...state.picked.values()].reduce((n, s) => n + s.size, 0);
  const total = seasons.reduce((n, s) => n + s.episodes.length, 0);

  const wholeBtn = el("button", { class: "ep-whole danger", onclick: () => { state.all = !state.all; state.picked.clear(); redraw(); } });
  const tabs = el("div", { class: "ep-tabs", role: "tablist" });
  const panel = el("div", { class: "ep-panel" });
  const list = el("ol", { class: "ep-list" });
  const summary = el("p", { class: "ep-summary" });
  const go = el("button", { class: "btn danger-fill" });
  const cancel = el("button", { class: "btn soft", onclick: back }, "Cancel");
  let listView = null;

  function setSeason(s, on) {
    state.all = false;
    if (on) state.picked.set(s.number, new Set(s.episodes.map((e) => e.number))); else state.picked.delete(s.number);
    redraw();
  }
  function setEpisode(s, n, on) {
    if (state.all) {
      // Unticking one from the whole show: the rest stay picked.
      state.all = false;
      seasons.forEach((x) => state.picked.set(x.number, new Set(x.episodes.map((e) => e.number))));
    }
    const set = state.picked.get(s.number) || new Set();
    if (on) set.add(n); else set.delete(n);
    if (set.size) state.picked.set(s.number, set); else state.picked.delete(s.number);
    redraw();
  }
  function redraw() {
    wholeBtn.classList.toggle("on", state.all);
    wholeBtn.replaceChildren(el("span", { class: "ep-box" }, state.all ? icon(ICON.check) : null), "Whole show");
    tabs.replaceChildren(...seasons.map((s) => {
      const n = state.all ? s.episodes.length : state.picked.get(s.number)?.size || 0;
      return el("button", { class: `ep-tab${state.view === s.number ? " viewing" : ""}${n ? " on" : ""}`, onclick: () => { state.view = s.number; redraw(); } },
        el("b", { text: name(s.number) }), el("small", { text: n === s.episodes.length ? "All" : n ? `${n} of ${s.episodes.length}` : `${s.episodes.length} on your PC` }));
    }));
    const s = seasons.find((x) => x.number === state.view) || seasons[0];
    const set = state.picked.get(s.number) || new Set();
    const whole = state.all || set.size === s.episodes.length;
    const allBox = el("input", { type: "checkbox", checked: whole });
    allBox.addEventListener("change", () => setSeason(s, allBox.checked));
    const top = el("label", { class: "ep-all" }, allBox, el("span", {}, el("b", { text: `All of ${name(s.number).toLowerCase()}` }), el("small", { text: `${s.episodes.length} on your PC` })));
    const keep = listView === s.number ? list.scrollTop : 0;
    list.replaceChildren(...s.episodes.map((e) => {
      const b = el("input", { type: "checkbox", checked: state.all || set.has(e.number) });
      b.addEventListener("change", () => setEpisode(s, e.number, b.checked));
      return el("li", {}, el("label", {}, b, el("span", { class: "n", text: `E${e.number}` }), el("span", { class: "t", text: e.title || `Episode ${e.number}` })));
    }));
    if (list.isConnected) panel.replaceChild(top, panel.firstElementChild); else panel.replaceChildren(top, list);
    list.scrollTop = keep;
    listView = s.number;
    const n = state.all ? total : count();
    summary.textContent = state.all ? "Every episode, and the show leaves your library." : n ? `${n} episode${n === 1 ? "" : "s"} picked. They won't download again.` : "Tick the episodes to delete, or Whole show.";
    go.textContent = state.all ? "Delete whole show" : n ? `Delete ${n} episode${n === 1 ? "" : "s"}` : "Delete";
    go.disabled = !state.all && !n;
  }
  go.addEventListener("click", async () => {
    if (state.all) { wholeDelete(); return; }
    const episodes = [...state.picked].flatMap(([s, set]) => [...set].map((e) => [s, e]));
    if (!episodes.length) return;
    const ok = await ask(`Delete ${episodes.length} episode${episodes.length === 1 ? "" : "s"} of ${t.title}?`, "They're deleted from your PC and won't download again. You can always get them back from Get more of this show.", [], "Delete");
    if (!ok) return;
    go.disabled = true;
    try {
      toast(await invoke("media_delete_episodes", { tile: t, episodes }));
      shut();
      setTimeout(() => window.dispatchEvent(new Event("chillarr:refresh")), 1500);
    } catch (e) { toast(String(e), true); go.disabled = false; }
  });
  box.replaceChildren(el("div", { class: "ep-pick-head" }, title, wholeBtn), tabs, panel, el("div", { class: "ep-foot" }, summary, el("div", { class: "ep-get" }, cancel, go)));
  redraw();
}

/** Delete from the library, files included, after asking. A show asks what to delete first. */
function deleteButton(t) {
  const b = el("button", { class: "btn soft danger", title: "Delete from your PC" }, "Delete");
  const isShow = t.kind === "series" || (t.kind === "anime" && t.format !== "MOVIE");
  if (isShow) {
    b.addEventListener("click", () => loadDeletePicker(t, wholeShow));
    return b;
  }
  b.addEventListener("click", wholeShow);
  return b;
  async function wholeShow() {
    const what = t.kind === "movie" ? "this movie" : "every episode of this show";
    const ok = await ask(`Delete ${t.title}?`, `This deletes ${what} from your PC. It can't be undone, but you can always get it again.`, [], "Delete");
    if (!ok) return;
    b.disabled = true;
    try {
      toast(await invoke("media_delete", { tile: t }));
      shut();
      setTimeout(() => window.dispatchEvent(new Event("chillarr:refresh")), 1500);
    } catch (e) { toast(String(e), true); b.disabled = false; }
  }
}

/** Ratings, runtime, who made it, and the cast, below the overview. */
async function loadAbout(t) {
  const seq = openSeq;
  const box = $("#detail-about");
  box.hidden = true;
  const d = await invoke("media_details", { tile: t }).catch(() => null);
  if (seq !== openSeq || !d || (!d.facts.length && !d.cast.length && !d.crew.length)) return;
  const parts = [];
  if (d.facts.length || d.crew.length) {
    parts.push(el("dl", { class: "facts" }, [...d.facts, ...d.crew].map(([k, v]) => el("div", {}, el("dt", { text: k }), el("dd", { text: v })))));
  }
  if (d.cast.length) {
    parts.push(el("h3", { text: "Cast" }), el("div", { class: "cast" }, d.cast.map((p, i) => {
      const face = el("div", { class: "face" });
      if (p.photo) face.append(el("img", { src: p.photo, alt: "", loading: "lazy", decoding: "async" }));
      else face.append(el("span", { text: p.name.split(" ").map((w) => w[0]).join("").slice(0, 2) }));
      const card = el("div", { class: "person" }, face, el("b", { text: p.name }), p.role ? el("small", { text: p.role }) : null);
      card.style.setProperty("--j", Math.min(i, 14));
      return card;
    })));
  }
  box.replaceChildren(...parts);
  box.hidden = false;
}

let statusPill = null;
let openKey = null;
listen("get-progress", (e) => {
  if (statusPill && dlg.open && e.payload.key === openKey && e.payload.status) statusPill.textContent = e.payload.status;
});

/**
 * Download and its quality choice. For a show the picker below says what to
 * get, and Download stays off until something is ticked. `inPicker` puts
 * both under the picker (a show you already have, whose top buttons play it).
 */
function downloadControls(t, isPickable, inPicker = false) {
  const get = el("button", { class: inPicker ? "btn primary" : "btn play", disabled: isPickable }, icon(ICON.download), isPickable ? "Pick what to get" : "Download");
  // The quality to ask for; the last choice is remembered.
  let saved = "best";
  try { saved = localStorage.getItem("chillarr-quality") || "best"; } catch (e) { /* storage blocked */ }
  const quality = el("select", { class: "quality", "aria-label": "Quality" },
    [["best", "Best (720p/1080p)"], ["4k", "4K"], ["1080p", "1080p"], ["720p", "720p"]].map(([v, label]) => el("option", { value: v, text: label, selected: v === saved })));
  quality.addEventListener("change", () => { try { localStorage.setItem("chillarr-quality", quality.value); } catch (e) { /* storage blocked */ } });
  // Library titles get theirs in the top buttons (openDetail), so it's there even with nothing left to get.
  const subs = t.jf_id ? null : subtitlesControl(t);
  let busy = false;
  const nothing = (p) => !p.all && !p.seasons.length && !p.episodes.length;
  get.addEventListener("click", async () => {
    const p = currentPick();
    // An empty pick would ask for the whole show.
    if (isPickable && nothing(p)) return;
    if (!(await confirmRedownload(t))) return;
    busy = true;
    get.disabled = true;
    quality.disabled = true;
    get.lastChild.textContent = "Adding...";
    try {
      const msg = isPickable ? await invoke("media_get_picked", { tile: t, quality: quality.value, pick: { seasons: p.seasons, episodes: p.episodes } }) : await invoke("media_get", { tile: t, quality: quality.value });
      get.replaceChildren(icon(ICON.check), "Downloading");
      subs?.commit();
      toast(`${msg} You'll get a notification when it's ready.`);
      store.emit("added");
    } catch (e) {
      busy = false;
      get.disabled = isPickable && nothing(currentPick());
      quality.disabled = false;
      get.lastChild.textContent = isPickable ? pickLabel(currentPick()) : "Download";
      toast(String(e), true);
    }
  });
  if (isPickable) {
    loadPicker(t, () => {
      if (busy) return;
      const p = currentPick();
      get.lastChild.textContent = pickLabel(p);
      get.disabled = nothing(p);
    }, inPicker ? el("div", { class: "ep-get" }, subs?.el, quality, get) : null);
  }
  return { get, quality, subs: inPicker ? null : subs?.el };
}

export async function openDetail(t, opts = {}) {
  // More info on an episode (Continue Watching) is about its show.
  if (t.kind === "episode" && t.series_id) {
    t = { ...t, kind: "series", key: `jf:${t.series_id}`, jf_id: t.series_id, subtitle: null, overview: null, progress: null, resume_seconds: null, tmdb: null, tvdb: null, played: null };
  }
  // A Watch now episode too: its show by TVDB id, every episode, this one resumable from its row.
  let wnEp = null;
  if (isWatchNow(t) && t.kind === "episode" && t.tvdb) {
    wnEp = t;
    t = { key: `tvdb:${t.tvdb}`, kind: "series", title: t.title, poster: t.poster, backdrop: t.backdrop, tvdb: t.tvdb };
  }
  openSeq++;
  show = null;
  $("#detail-where").hidden = true;
  statusPill = null;
  openKey = t.key;
  dlg.classList.remove("closing");
  stopTrailer();
  $("#detail-pick").hidden = true;
  $("#detail-pick").classList.remove("deleting");
  pickState = null;
  loadAbout(t);
  loadSimilar(t);
  const pic = t.backdrop || t.poster;
  $("#detail-hero").style.backgroundImage = pic ? `url("${pic.replace(/"/g, "%22")}")` : "none";
  $("#detail-title").textContent = t.title;
  $("#detail-meta").replaceChildren(...metaBits(t));
  if (!t.completed) loadWatched().then((set) => { if (openKey === t.key && isWatched(t, set)) $("#detail-meta").append(el("span", { class: "sep" }), el("span", { class: "owned", text: "Watched" })); });
  $("#detail-overview").textContent = t.overview || "";
  $("#detail-episodes").hidden = true;
  const actions = $("#detail-actions");
  actions.replaceChildren();
  dlg.showModal();

  const seq = openSeq;
  const isShow = t.kind === "series" || (t.kind === "anime" && t.jf_id);
  const watching = isWatchNow(t);
  // Not in the library: a show not here yet, a Watch now episode's show, or a
  // Completed show whose files are all gone. Its full list (the backend finds
  // the show from the tile), finished ones ticked, each episode with its own
  // Watch now, so you pick which one to watch.
  const listedByTvdb = !t.jf_id && !movieLike(t) && (t.kind === "series" || t.kind === "anime" || !!wnEp || !!t.completed);
  const main = listedByTvdb ? el("span", { class: "main-play" }) : null;
  if (main) actions.append(main);
  const listed = listedByTvdb && (await loadShow(t, main));
  if (main && !listed) main.remove();
  if (listedByTvdb && seq !== openSeq) return;
  if (watching || (wnEp && !listed)) {
    watchNowActions(wnEp || t, actions);
  } else if (listed) {
    if (wnEp) actions.append(el("button", { class: "btn soft danger", onclick: async () => { if (await removeWatchNow(wnEp)) shut(); } }, `Remove ${(wnEp.subtitle || "").split(" · ")[0]}`.trim()));
    downloadControls(t, true, true);
    if (opts.get) setTimeout(() => $("#detail-pick").scrollIntoView({ behavior: "smooth", block: "start" }), 400);
  } else if (t.jf_id && isShow) {
    const main = el("span", { class: "main-play" });
    actions.append(main);
    if (!(await loadShow(t, main)) && seq === openSeq) {
      // The full list couldn't be had: Jellyfin's own episodes, as before, and a quiet word.
      toast("Couldn't load every episode.");
      const next = await loadEpisodes(t.series_id || t.jf_id);
      if (next) {
        main.append(el("button", { class: "btn play", onclick: () => { shut(); play(next, episodes); } }, icon(ICON.play), next.resume_seconds ? `Resume ${next.subtitle?.split(" · ")[0] || ""}` : `Play ${next.subtitle?.split(" · ")[0] || ""}`));
        otherPlayer(main, next, episodes);
      }
    }
    // Seasons and episodes not downloaded yet can still be got from here.
    if (seq === openSeq) {
      downloadControls(t, true, true);
      if (opts.get) setTimeout(() => $("#detail-pick").scrollIntoView({ behavior: "smooth", block: "start" }), 400);
    }
  } else if (t.jf_id) {
    // Resume and Start over side by side: the player doesn't ask again.
    actions.append(el("button", { class: "btn play", onclick: () => { shut(); play({ ...t, chose: true }); } }, icon(ICON.play), t.resume_seconds ? "Resume" : "Play"));
    if (t.resume_seconds) actions.append(el("button", { class: "btn soft", onclick: () => { shut(); play({ ...t, resume_seconds: 0 }); } }, "Start over"));
    otherPlayer(actions, t, null);
  } else if (t.status && requested(t)) {
    // Asked for already: say where it is (kept current below), and offer Search again.
    statusPill = el("span", { class: "status-pill", text: t.status });
    actions.append(statusPill, subtitlesControl(t).el);
    // Already downloading: Watch now plays it as it comes; that download keeps its own rules.
    const now = watchNowButton(t, false);
    if (now) actions.append(now);
    if (!t.status.startsWith("Downloading")) {
      const again = el("button", { class: "btn soft" }, "Search again");
      again.addEventListener("click", async () => {
        again.disabled = true;
        try { toast(await invoke("media_get", { tile: t, quality: "best" })); } catch (e) { toast(String(e), true); again.disabled = false; }
      });
      actions.append(again);
    }
  } else if (t.status) {
    actions.append(el("button", { class: "btn soft", disabled: true }, t.status));
  } else {
    const isPickable = t.kind === "series" || (t.kind === "anime" && t.format !== "MOVIE");
    const { get, quality, subs } = downloadControls(t, isPickable);
    // Not here yet: Watch now leads, Download sits beside it.
    const now = watchNowButton(t, true);
    if (now) get.classList.replace("play", "soft");
    actions.append(...[now, get, quality, subs].filter(Boolean));
    if (isPickable && opts.get) setTimeout(() => $("#detail-pick").scrollIntoView({ behavior: "smooth", block: "start" }), 400);
    else if (opts.get) setTimeout(() => get.click(), 50);
  }
  // A Watch now title's Jellyfin item is a placeholder: no My List, no Delete (Remove does that).
  if (t.completed) completedBits(t, actions);
  if (opts.episodes) showEpisodes(seq);
  else if (listable(t) && !watching) actions.append(myListButton(t));
  if (t.jf_id && t.kind !== "episode" && !watching) actions.append(subtitlesControl(t).el, canDelete() ? deleteButton(t) : null);
  const tr = trailerButton(t);
  if (tr) actions.append(tr);
  else {
    // Not known up front: ask (Jellyfin, AniList, KinoCheck) and add the button if one turns up.
    const seq = openSeq;
    invoke("media_trailer", { tile: t }).then((id) => {
      if (seq === openSeq && id && !actions.querySelector(".trailer-btn")) actions.append(trailerButton({ ...t, trailer: id }));
    }).catch(() => {});
  }
  if (t.genres?.length) $("#detail-meta").append(el("span", { class: "sep" }), el("span", { text: t.genres.slice(0, 3).join(", ") }));
}

/* ---------- Similar to this ---------- */
// The last section: titles like this one (similar.rs), asked for once the
// panel has opened. A card opens its title in place; Back (Esc, B, the phone
// remote, or the Back pill) returns to the title before, at the same card.
const simBox = el("section", { class: "similar", id: "detail-similar", "aria-label": "Similar to this", hidden: true });
$("#detail .detail-body").append(simBox);
/** Where the row sits in the panel's content, leaving out the slide a title swap gives the body. */
function simTop() {
  const body = simBox.parentElement;
  return body.offsetTop + simBox.getBoundingClientRect().top - body.getBoundingClientRect().top;
}
/** Titles opened before this one from a card: { tile, key, left }. */
let trail = [];
let simTile = null;
/** Set while going back: the card to come back to once the row is drawn. */
let backTo = null;
let swapTimer;

function simCard(s, i) {
  const art = el("div", { class: "art" });
  const src = s.poster || s.backdrop;
  const noart = () => el("div", { class: "noart", text: s.title });
  if (src) {
    const img = el("img", { src, alt: "", loading: "lazy", decoding: "async" });
    img.addEventListener("load", () => img.classList.add("loaded"), { once: true });
    img.addEventListener("error", () => img.replaceWith(noart()), { once: true });
    art.append(img);
  } else art.append(noart());
  if (s.jf_id) art.append(el("span", { class: "badge", text: "In library" }));
  else if (s.status) art.append(el("div", { class: "over", text: s.status }));
  const b = el("button", { class: "tile", "aria-label": s.title, dataset: { key: s.key } }, art,
    el("div", { class: "cap" }, el("strong", { text: s.title }), s.year ? ` · ${s.year}` : ""));
  b.style.setProperty("--j", Math.min(i, 10));
  b.addEventListener("click", () => simOpen(s, b));
  return b;
}

/** Fades the panel out, runs `swap` (the next title), and fades back in once `ready` resolves. */
function simSwap(swap, ready = null) {
  clearTimeout(swapTimer);
  dlg.classList.add("swapping");
  swapTimer = setTimeout(async () => {
    swap();
    dlg.scrollTop = 0;
    if (ready) await Promise.race([ready, new Promise((r) => setTimeout(r, 1500))]);
    requestAnimationFrame(() => dlg.classList.remove("swapping"));
  }, 170);
}

function simOpen(next, card) {
  // Where the row sat in the panel too, so Back lands on it exactly.
  const at = simTop() - dlg.scrollTop;
  trail.push({ tile: simTile, key: next.key, left: card.parentElement.scrollLeft, at });
  simSwap(() => openDetail(next));
  if (document.body.classList.contains("nav-on")) setTimeout(() => $("#detail-actions button")?.focus({ preventScroll: true }), 450);
}

function simBack() {
  const prev = trail.pop();
  if (!prev) return;
  let finish;
  const done = new Promise((r) => (finish = r));
  backTo = { ...prev, finish };
  simSwap(() => openDetail(prev.tile), done);
}

// Back with titles behind this one goes to the one before instead of closing.
// Capture, so it runs before the panel's own Esc handling above.
dlg.addEventListener("cancel", (e) => {
  if (!trail.length || $("#detail-hero").classList.contains("playing")) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  simBack();
}, true);
dlg.addEventListener("close", () => {
  trail = []; backTo = null; dlg.classList.remove("swapping");
  // Closing hands focus back to the card you opened. With the mouse that would
  // leave a ring on it; the ring is for controller and keyboard moves only.
  const b = document.body.classList;
  if (!b.contains("nav-on") && !b.contains("pad-mode") && document.activeElement?.closest?.("#view .tile")) document.activeElement.blur();
});

function drawBackPill() {
  $("#detail-hero .detail-back")?.remove();
  if (!trail.length) return;
  const prev = trail[trail.length - 1].tile;
  $("#detail-hero").append(el("button", { class: "detail-back", title: `Back to ${prev.title}`, onclick: simBack }, icon(ICON.left), "Back"));
}

function loadSimilar(t) {
  // A fresh open (not from a card or Back) starts a new trail.
  if (!dlg.open) trail = [];
  simTile = t;
  drawBackPill();
  const seq = openSeq;
  const skel = Array.from({ length: 7 }, () => el("div", { class: "skel skel-tile" }));
  simBox.classList.remove("ready");
  simBox.replaceChildren(el("div", { class: "skel sim-skel-head" }), el("div", { class: "track sim-track" }, skel));
  simBox.hidden = false;
  const back = backTo;
  backTo = null;
  const end = () => back?.finish();
  // After the panel's opening animation, so it doesn't compete with it.
  setTimeout(async () => {
    if (seq !== openSeq) { end(); return; }
    const list = await invoke("media_similar", { tile: t }).catch(() => []);
    if (seq !== openSeq) { end(); return; }
    if (!list.length) { simBox.hidden = true; end(); return; }
    const track = el("div", { class: "track sim-track" }, list.map(simCard));
    const step = () => track.clientWidth * 0.85;
    const wrap = el("div", { class: "track-wrap" },
      el("button", { class: "arrow prev", "aria-label": "Scroll left", tabindex: "-1", onclick: () => track.scrollBy({ left: -step(), behavior: "smooth" }) }, icon(ICON.left)),
      track,
      el("button", { class: "arrow next", "aria-label": "Scroll right", tabindex: "-1", onclick: () => track.scrollBy({ left: step(), behavior: "smooth" }) }, icon(ICON.right)));
    simBox.replaceChildren(el("h3", { text: "Similar to this" }), wrap);
    simBox.classList.add("ready");
    requestAnimationFrame(() => wrap.classList.toggle("fits", track.scrollWidth - track.clientWidth < 48));
    if (back) {
      // Back where you left: the row in view, scrolled to the card you picked.
      track.scrollLeft = back.left;
      dlg.scrollTop = simTop() - back.at;
      const card = [...track.children].find((c) => c.dataset.key === back.key);
      if (card && document.body.classList.contains("nav-on")) card.focus({ preventScroll: true });
    }
    end();
  }, back ? 0 : 380);
}
