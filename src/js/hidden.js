// Hidden (hidden.rs): each profile hides films, series, anime, manga and books.
// Hidden things are never on its pages: one style rule takes them off (each
// card carries its title's ids in data-hk, and the rule hides cards holding a
// hidden id). They show only in the Hidden tab, behind the profile's hidden
// password, on the one device it was typed on, for 15 minutes.
import { el, getPref, invoke, setPref, toast } from "./api.js";

// The last list, so a page drawn before the PC answers already leaves them out,
// and a failed ask never shows them (it used to clear the list).
const SAVED = "chillarr-hidden-keys";
let keys = new Set();
try { keys = new Set(JSON.parse(getPref(SAVED) || "[]")); } catch (e) { /* none saved */ }
let state = { has_password: false, unlocked: false, count: 0 };
const style = el("style", { id: "hidden-rule" });
document.head.append(style);

/** Every id a title or card can be known by. */
export function idsOf(x) {
  const out = [];
  if (!x) return out;
  const add = (k) => { if (k && !out.includes(k)) out.push(k); };
  if (x.tmdb) {
    add(`tmdb:${x.tmdb}`);
    // Films and shows share TMDB numbers: the kind keeps one from hiding the other.
    if (x.kind === "movie") add(`tmdbm:${x.tmdb}`);
    else if (x.kind === "series" || x.kind === "anime") add(`tmdbtv:${x.tmdb}`);
  }
  if (x.tvdb) add(`tvdb:${x.tvdb}`);
  if (x.jf_id && x.kind !== "episode") add(`jf:${x.jf_id}`);
  if (x.series_id) add(`jf:${x.series_id}`);
  if (x.anilist) add(`al:${x.anilist}`);
  if (x.olid) add(`ol:${x.olid}`);
  const k = typeof x.key === "string" ? x.key : "";
  const m = /^(tmdb|tvdb|jf):([\w-]+)$/.exec(k);
  if (m) add(`${m[1]}:${m[2]}`);
  const a = /^anilist:(\d+)$/.exec(k);
  if (a) add(`al:${a[1]}`);
  // A followed series ("m12") or a book ("b0a1b2...") of reading.
  if (/^[mb][0-9a-f]+$/.test(k)) add(`r:${k}`);
  return out;
}

/** Marks a card with its ids, so the rule can hide it. */
export function markHidden(node, ...things) {
  const ids = [].concat(...things.map(idsOf));
  if (ids.length) node.setAttribute("data-hk", ids.join(" "));
  return node;
}

export const isHidden = (...things) => [].concat(...things.map(idsOf)).some((k) => keys.has(k));

/** The ids a hide is saved under: the plain TMDB id only when the kind isn't known. */
function hideIds(things) {
  const ids = [].concat(...things.map(idsOf)).filter((k, i, a) => a.indexOf(k) === i);
  return ids.filter((k) => !(k.startsWith("tmdb:") && ids.some((x) => x === `tmdbm:${k.slice(5)}` || x === `tmdbtv:${k.slice(5)}`)));
}

function apply() {
  // Ids are only letters, digits, - and _ after the kind (hidden.rs clean_key).
  const sel = [...keys].filter((k) => /^[a-z]+:[\w-]+$/.test(k)).map((k) => `[data-hk~="${k}"]`);
  style.textContent = sel.length ? `${sel.join(",")}{display:none !important}` : "";
}

/** Asks the PC what to hide now (on start, after a profile switch, lock or unlock). */
export async function refreshHidden() {
  try {
    state = await invoke("hidden_status");
    keys = new Set(state.keys || []);
    setPref(SAVED, JSON.stringify([...keys]));
  } catch (e) {
    // Not known right now: the last list stays, never an empty one.
  }
  apply();
  document.querySelectorAll(".hd-tab").forEach((a) => { a.hidden = !state.has_password && !state.count; });
  window.dispatchEvent(new Event("chillarr:hidden"));
  return state;
}
export const hiddenState = () => state;

/** A small password box. Resolves to the text, or null when cancelled. */
function askPassword(title, body, { twice = false, old = false } = {}) {
  return new Promise((done) => {
    const field = (ph, ac) => el("input", { type: "password", class: "hd-input", placeholder: ph, "aria-label": ph, autocomplete: ac });
    const oldIn = old ? field("Current hidden password", "current-password") : null;
    const one = field(twice ? "New hidden password" : "Hidden password", twice ? "new-password" : "current-password");
    const two = twice ? field("Type it again", "new-password") : null;
    const err = el("p", { class: "hd-err", hidden: true });
    const d = el("dialog", { class: "hd-ask" });
    const finish = (v) => { d.close(); d.remove(); done(v); };
    const ok = () => {
      if (twice && one.value !== two.value) { err.textContent = "Those don't match."; err.hidden = false; return; }
      if (!one.value) { err.textContent = "Type the password."; err.hidden = false; return; }
      finish(old ? { password: one.value, old: oldIn.value } : one.value);
    };
    const form = el("form", { method: "dialog", onsubmit: (e) => { e.preventDefault(); ok(); } },
      el("h2", { text: title }), el("p", { class: "hd-body", text: body }), oldIn, one, two, err,
      el("div", { class: "ask-actions" },
        el("button", { type: "button", class: "btn ghost", onclick: () => finish(null) }, "Cancel"),
        el("button", { type: "submit", class: "btn primary" }, "OK")));
    d.append(form);
    d.addEventListener("cancel", (e) => { e.preventDefault(); finish(null); });
    // Typing is for the box, not the page's keys.
    d.addEventListener("keydown", (e) => e.stopPropagation());
    document.body.append(d);
    d.showModal();
    (oldIn || one).focus();
  });
}

async function setPassword(change = false) {
  const got = await askPassword(change ? "Change the hidden password" : "Make a hidden password",
    "Hidden things stay off every page of this profile. This password opens the Hidden tab. Use a different one from your PIN and your sign-in. If you forget it, the main profile can reset it on the PC.", { twice: true, old: change });
  if (!got) return false;
  try {
    await invoke("hidden_set_password", change ? { password: got.password, old: got.old } : { password: got });
    toast(change ? "Hidden password changed." : "Hidden password set.");
    await refreshHidden();
    return true;
  } catch (e) { toast(String(e), true); return false; }
}

export async function unlockHidden() {
  const pw = await askPassword("Show hidden things", "Type this profile's hidden password.");
  if (!pw) return false;
  try { await invoke("hidden_unlock", { password: pw }); await refreshHidden(); toast("The Hidden tab is open on this device for 15 minutes."); return true; }
  catch (e) { toast(String(e), true); return false; }
}

export async function lockHidden() {
  try { await invoke("hidden_lock"); await refreshHidden(); toast("The Hidden tab is locked."); } catch (e) { toast(String(e), true); }
}

/** Hides a title (a tile or a reading card, plus its shelf entry) or shows it again. */
export async function setHidden(hide, label, ...things) {
  const ids = hideIds(things);
  if (!ids.length) { toast("That can't be hidden.", true); return false; }
  if (!state.has_password && !(await setPassword())) return false;
  try {
    // The card itself goes too (hide only), so Settings can open its page while unlocked.
    const item = hide && things[0] && typeof things[0] === "object" ? things[0] : null;
    await invoke("hidden_set", { keys: ids, hide, label: label || null, item });
    await refreshHidden();
    toast(hide ? `${label || "It"} is hidden. It's in the Hidden tab now.` : `${label || "It"} shows again.`);
    return true;
  } catch (e) { toast(String(e), true); return false; }
}

/** The Hide / Show again button for a detail panel. `close` shuts the panel once hidden. */
export function hideButton(label, close, ...things) {
  const b = el("button", { class: "btn soft hd-btn", title: "Keep it off every page of this profile, behind a password" }, "Hide");
  // Unlocked, it may already be on the hidden list: then it offers Show again.
  if (state.unlocked) {
    invoke("hidden_list").then((list) => {
      const mine = new Set(list.map((x) => x.key));
      if ([].concat(...things.map(idsOf)).some((k) => mine.has(k))) b.textContent = "Show again";
    }).catch(() => {});
  }
  b.addEventListener("click", async () => {
    const show = b.textContent === "Show again";
    if (await setHidden(!show, label, ...things)) { if (!show && close) close(); else b.textContent = show ? "Hide" : "Show again"; }
  });
  return b;
}

/** Opens a hidden title's page from the unlocked list: a film or show, a
 *  followed series, or a book. Loaded when asked, since those pages import this file. */
async function openHidden(item) {
  try {
    const k = typeof item.key === "string" ? item.key : "";
    if (/^b[0-9a-f]+$/.test(k)) return (await import("./read.js")).openBookPanel(item);
    if (/^m[0-9a-f]+$/.test(k)) return (await import("./read.js")).openSeries({ key: k, card: item });
    return (await import("./detail.js")).openDetail(item);
  } catch (e) { toast(String(e), true); }
}

/** Settings > Account (in the app and on phones): the password, and the way to the Hidden tab. */
export function hiddenBox() {
  const box = el("div", { class: "box hd-box" });
  const draw = async () => {
    const s = await refreshHidden();
    const tab = document.body.dataset.mode === "read" ? "#r-hidden" : "#hidden";
    box.replaceChildren(el("div", { class: "opt" },
      el("span", {}, el("b", { text: "Hidden" }),
        el("small", { text: s.has_password
          ? `${s.count ? `${s.count} hidden on this profile. ` : "Nothing hidden yet. "}Hide a title from its page. Hidden things are only in the Hidden tab.`
          : "Hide films, series, anime, manga and books from every page of this profile. They go to a Hidden tab with a password only you know." })),
      el("div", { class: "acts" }, ...(s.has_password
        ? [el("a", { class: "btn small primary", href: tab }, "Open Hidden"),
          el("button", { class: "btn small ghost", onclick: async () => { if (await setPassword(true)) draw(); } }, "Change password")]
        : [el("button", { class: "btn small primary", onclick: async () => { if (await setPassword()) draw(); } }, "Set a password")]))));
  };
  draw();
  return box;
}

/** A hidden title as a card of its own: the picture, the name, Open and Show again. */
function hiddenCard(x, redraw) {
  const it = x.item || {};
  const src = it.poster || it.cover || it.backdrop || it.banner || null;
  return el("div", { class: "hd-card" },
    el("div", { class: "hd-art" }, src ? el("img", { src, alt: "", loading: "lazy", decoding: "async" }) : el("span", { text: x.label })),
    el("b", { class: "hd-name", text: x.label }),
    el("div", { class: "hd-acts" },
      x.item ? el("button", { class: "btn small primary", onclick: () => openHidden(x.item) }, "Open") : null,
      el("button", { class: "btn small ghost", onclick: async () => {
        try { await invoke("hidden_set", { keys: [x.key], hide: false, label: null }); await refreshHidden(); toast(`${x.label} shows again.`); redraw(); } catch (e) { toast(String(e), true); }
      } }, "Show again")));
}

/** The Hidden tab: locked until this profile's hidden password is typed on this device. */
export async function renderHidden(view) {
  const page = el("section", { class: "hd-page" });
  view.replaceChildren(page);
  const draw = async () => {
    const s = await refreshHidden();
    const head = el("div", { class: "hd-head" }, el("h1", { text: "Hidden" }));
    if (!s.has_password) {
      page.replaceChildren(head, s.count
        // Reset on the PC: the list stayed, a new password opens it.
        ? el("div", { class: "hd-lock" }, el("p", { class: "hd-note", text: "The hidden password was reset. Make a new one to see what's hidden." }),
          el("button", { class: "btn primary", onclick: async () => { if (await setPassword()) draw(); } }, "Make a password"))
        : el("p", { class: "hd-note", text: "Nothing is hidden on this profile. Use Hide on a title's page to put it here." }));
      return;
    }
    if (!s.unlocked) {
      page.replaceChildren(head, el("div", { class: "hd-lock" },
        el("p", { class: "hd-note", text: `${s.count || "No"} title${s.count === 1 ? "" : "s"} hidden. Type this profile's hidden password to see them.` }),
        el("button", { class: "btn primary", onclick: async () => { if (await unlockHidden()) draw(); } }, "Unlock")));
      return;
    }
    const list = await invoke("hidden_list").catch(() => []);
    head.append(el("button", { class: "btn small ghost", onclick: async () => { await lockHidden(); draw(); } }, "Lock"));
    page.replaceChildren(head, list.length
      ? el("div", { class: "hd-grid" }, list.map((x) => hiddenCard(x, draw)))
      : el("p", { class: "hd-note", text: "Nothing hidden yet." }));
  };
  await draw();
}

// A profile switch (or a web sign-in) changes whose list applies.
window.addEventListener("chillarr:profile", () => refreshHidden());
