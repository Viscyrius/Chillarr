// My List: a bookmark that downloads nothing. Titles in the library are
// Jellyfin favourites, so the list matches Jellyfin's own apps on TVs and
// phones; anything else waits in Chillarr and becomes a favourite once it arrives.
import { ask, el, ICON, invoke, toast } from "./api.js";
import { canWatchNow, movieLike, watchNow } from "./player.js";

let marks = null;
let marksAt = 0;
// Bumped when the profile changes, so an answer for the last one isn't kept.
let gen = 0;

async function load() {
  if (!marks || Date.now() - marksAt > 30000) {
    const asked = gen;
    const [keys, ids] = await invoke("media_my_list_marks");
    const fresh = { keys: new Set(keys), ids: new Set(ids) };
    if (asked !== gen) return fresh;
    marks = fresh;
    marksAt = Date.now();
  }
  return marks;
}

// Another profile (or a sign-in) has its own list: ask again.
window.addEventListener("chillarr:refresh", () => { marks = null; gen++; });

/** Episodes aren't listed on their own: their show is. */
export function listable(t) {
  return ["movie", "series", "anime"].includes(t.kind);
}

export async function onMyList(t) {
  const m = await load();
  return m.keys.has(t.key) || (!!t.jf_id && m.ids.has(t.jf_id));
}

/** Puts it on or takes it off. Resolves to whether it's on now. */
export async function setMyList(t, on) {
  const now = await invoke("media_my_list", { tile: t, on });
  const m = await load();
  const set = t.jf_id ? m.ids : m.keys;
  const id = t.jf_id || t.key;
  if (now) set.add(id); else set.delete(id);
  window.dispatchEvent(new Event("chillarr:saved"));
  return now;
}

/* ---------- Completed ---------- */
// Everything finished, still here or deleted (My List > Completed, per profile).

const X = "M18 6 6 18M6 6l12 12";

/** What it's kept under: "tmdb:<id>" for a movie, "tvdb:<id>" for a show. */
export function completedKey(t) {
  return t.kind === "movie" ? `tmdb:${t.tmdb}` : `tvdb:${t.tvdb}`;
}

/** Takes it off this profile's Completed and out of the page. */
export async function removeCompleted(t) {
  try {
    await invoke("completed_remove", { key: completedKey(t) });
    document.querySelectorAll(".completed-grid .tile").forEach((n) => { if (n.dataset.key === t.key) n.remove(); });
    window.dispatchEvent(new Event("chillarr:completed"));
    toast(`Took ${t.title} off Completed.`);
  } catch (e) { toast(String(e), true); }
}

/**
 * What a Completed title offers, in order, as { label, icon, run }. The card
 * on hover and the detail panel both draw from this. `openDetail` is passed in
 * so this file doesn't import the detail panel. Watch now on a show plays
 * the episode you're at there (or its first).
 */
export function completedActions(t, openDetail) {
  const out = [];
  if (canWatchNow(t)) out.push({ id: "watchnow", label: "Watch now", icon: ICON.play, run: () => (movieLike(t) ? watchNow(t) : openDetail(t, { episodes: true })) });
  out.push({ id: "download", label: "Download", icon: ICON.download, run: () => openDetail(t, { get: true }) });
  out.push({ id: "remove", label: "Remove from Completed", icon: X, run: () => removeCompleted(t) });
  return out;
}

/* ---------- Watched ---------- */
// What this profile finished (the watch log), so a title it already saw says
// so everywhere and isn't downloaded again by accident.

let watchedSet = null;
let watchedAt = 0;
let watchedAsk = null;

window.addEventListener("chillarr:refresh", () => { watchedSet = null; watchedAsk = null; });

/** The finished keys ("tmdb:<id>", "tvdb:<id>", "anilist:<id>"), asked for at most every 30s. */
export function loadWatched() {
  if (watchedSet && Date.now() - watchedAt < 30000) return Promise.resolve(watchedSet);
  if (!watchedAsk) {
    watchedAsk = invoke("media_watched_keys")
      .then((keys) => { watchedSet = new Set(keys); watchedAt = Date.now(); return watchedSet; })
      .catch(() => watchedSet || new Set())
      .finally(() => { watchedAsk = null; });
  }
  return watchedAsk;
}

/** Whether this profile finished it: a movie, or a show watched to the end. */
export function isWatched(t, set = watchedSet) {
  if (!set || !listable(t)) return false;
  if (set.has(t.key)) return true;
  return movieLike(t) ? !!t.tmdb && set.has(`tmdb:${t.tmdb}`) : !!t.tvdb && set.has(`tvdb:${t.tvdb}`);
}

/** Adds the "Watched" mark to a poster, once the list is known. Completed tiles have their own. */
export function markWatched(box, t) {
  if (t.completed) return;
  const add = () => { if (isWatched(t) && !box.querySelector(".watched-mark")) box.append(el("div", { class: "watched-mark", text: "Watched" })); };
  if (watchedSet) add(); else loadWatched().then(add);
}

/** Before a download: a title already finished asks first. True to go ahead. */
export async function confirmRedownload(t) {
  if (!isWatched(t, await loadWatched())) return true;
  return ask("You already watched this.", `Download ${t.title} again?`, [], "Download");
}
