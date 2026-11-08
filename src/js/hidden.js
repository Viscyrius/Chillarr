// Hidden (hidden.rs): each profile hides films, series, anime, manga and books
// behind a password of its own. While it's locked, anything hidden is taken
// off every page by one style rule: each card carries its title's ids in
// data-hk, and the rule hides cards holding a hidden id. Kept tabs and rows
// need no redraw when it locks or unlocks.
import { el, invoke, toast } from "./api.js";

let keys = new Set();
let state = { has_password: false, unlocked: false, count: 0 };
const style = el("style", { id: "hidden-rule" });
document.head.append(style);

/** Every id a title or card can be known by. */
export function idsOf(x) {
  const out = [];
  if (!x) return out;
  const add = (k) => { if (k && !out.includes(k)) out.push(k); };
  if (x.tmdb) add(`tmdb:${x.tmdb}`);
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
  } catch (e) {
    keys = new Set();
  }
  apply();
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
    "Hidden things stay off every page of this profile until this password is typed. Use a different one from your PIN and your sign-in.", { twice: true, old: change });
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
  try { await invoke("hidden_unlock", { password: pw }); await refreshHidden(); toast("Hidden things show until you lock them, or for an hour."); return true; }
  catch (e) { toast(String(e), true); return false; }
}

export async function lockHidden() {
  try { await invoke("hidden_lock"); await refreshHidden(); toast("Hidden things are hidden again."); } catch (e) { toast(String(e), true); }
}

/** Hides a title (a tile or a reading card, plus its shelf entry) or shows it again. */
export async function setHidden(hide, label, ...things) {
  const ids = [].concat(...things.map(idsOf)).filter((k, i, a) => a.indexOf(k) === i);
  if (!ids.length) { toast("That can't be hidden.", true); return false; }
  if (!state.has_password && !(await setPassword())) return false;
  try {
    await invoke("hidden_set", { keys: ids, hide, label: label || null });
    await refreshHidden();
    toast(hide ? `${label || "It"} is hidden. Unlock hidden things in Settings > Account & devices to see it.` : `${label || "It"} shows again.`);
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

/** Settings > Account & devices: the password, Unlock / Lock, and what's hidden. */
export function hiddenBox() {
  const box = el("div", { class: "box hd-box" });
  const draw = async () => {
    const s = await refreshHidden();
    const rows = [el("div", { class: "opt" },
      el("span", {}, el("b", { text: "Hidden" }),
        el("small", { text: s.has_password
          ? `${s.count ? `${s.count} hidden id${s.count === 1 ? "" : "s"} on this profile. ` : "Nothing hidden yet. "}Hide a title from its page (Hide). ${s.unlocked ? "Showing them now." : "Locked: they're off every page."}`
          : "Hide films, series, anime, manga and books from every page of this profile, behind a password only you know." })),
      el("div", { class: "acts" }, ...(s.has_password
        ? [s.unlocked ? el("button", { class: "btn small primary", onclick: async () => { await lockHidden(); draw(); } }, "Lock")
          : el("button", { class: "btn small primary", onclick: async () => { if (await unlockHidden()) draw(); } }, "Show hidden"),
          el("button", { class: "btn small ghost", onclick: async () => { if (await setPassword(true)) draw(); } }, "Change password")]
        : [el("button", { class: "btn small primary", onclick: async () => { if (await setPassword()) draw(); } }, "Set a password")])))];
    if (s.unlocked) {
      const list = await invoke("hidden_list").catch(() => []);
      if (list.length) rows.push(el("ul", { class: "st-list hd-list" }, list.map((x) => el("li", {},
        el("div", { class: "st-info" }, el("b", { text: x.label })),
        el("button", { class: "btn small ghost", onclick: async () => {
          try { await invoke("hidden_set", { keys: [x.key], hide: false, label: null }); toast(`${x.label} shows again.`); draw(); } catch (e) { toast(String(e), true); }
        } }, "Show again")))));
    }
    box.replaceChildren(...rows);
  };
  draw();
  return box;
}

// A profile switch (or a web sign-in) changes whose list applies.
window.addEventListener("chillarr:profile", () => refreshHidden());
