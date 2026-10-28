// Recent searches under the top bar's search box: shown on focus while the box
// is empty, one list per profile, kept in this window's storage.
import { el, icon, invoke } from "./api.js";

const MAX = 8;
// A term is saved once it has stood this long (or on Enter), so half-typed
// words never make the list.
const SETTLE_MS = 1500;

let profile = null;
let saveTimer = null;

async function profileId() {
  if (profile) return profile;
  const me = (await invoke("profiles_list").catch(() => [])).find((p) => p.active);
  profile = me?.id || "default";
  return profile;
}
// A profile switch redraws the app with this event; the next read asks again.
window.addEventListener("chillarr:refresh", () => { profile = null; });

const keyOf = (id) => `chillarr-recent-searches:${id}`;
function read(id) {
  try {
    const v = JSON.parse(localStorage.getItem(keyOf(id)) || "[]");
    return Array.isArray(v) ? v.filter((t) => typeof t === "string").slice(0, MAX) : [];
  } catch (e) { return []; }
}
function write(id, list) {
  try { localStorage.setItem(keyOf(id), JSON.stringify(list.slice(0, MAX))); } catch (e) { /* storage blocked */ }
}

/** Puts a term at the top of this profile's list. */
async function save(term) {
  term = term.trim();
  if (term.length < 2) return;
  const id = await profileId();
  const low = term.toLowerCase();
  write(id, [term, ...read(id).filter((t) => t.toLowerCase() !== low)]);
}

/**
 * Wires the dropdown to the search box. `run(term)` searches.
 * Returns `searched(term)`, to call each time a search runs.
 */
export function recentSearches(input, run) {
  const box = input.closest(".search");
  const menu = el("div", { class: "recent", role: "listbox", "aria-label": "Recent searches", hidden: true });
  box.append(menu);
  input.setAttribute("aria-autocomplete", "list");
  input.setAttribute("aria-expanded", "false");
  let items = [];
  let active = -1;

  const close = () => {
    if (menu.hidden) return;
    menu.hidden = true;
    active = -1;
    input.setAttribute("aria-expanded", "false");
    input.removeAttribute("aria-activedescendant");
  };

  function mark(i) {
    active = i;
    menu.querySelectorAll(".recent-row").forEach((r, n) => r.classList.toggle("on", n === i));
    if (i >= 0) input.setAttribute("aria-activedescendant", `recent-${i}`);
    else input.removeAttribute("aria-activedescendant");
  }

  function pick(term) {
    close();
    input.value = term;
    clearTimeout(saveTimer);
    save(term);
    run(term);
  }

  async function open() {
    if (input.value.trim()) return close();
    const id = await profileId();
    items = read(id);
    // Typed or left while the list was being read.
    if (!items.length || input.value.trim() || document.activeElement !== input) return close();
    const rows = items.map((t, i) => el("div", { class: "recent-row", id: `recent-${i}`, role: "option",
      onmousedown: (e) => e.preventDefault(), onclick: () => pick(t), onmousemove: () => active !== i && mark(i) },
      icon("M12 7v5l3 2M21 12a9 9 0 1 1-9-9 9 9 0 0 1 9 9z"),
      el("span", { class: "recent-term", text: t }),
      el("button", { class: "recent-x", type: "button", tabindex: "-1", title: "Remove", "aria-label": `Remove ${t}`,
        onclick: (e) => { e.stopPropagation(); write(id, read(id).filter((x) => x !== t)); open(); } }, icon("M6 6l12 12M18 6L6 18"))));
    const clear = el("button", { class: "recent-clear", type: "button", tabindex: "-1", onclick: () => { write(id, []); close(); } }, "Clear");
    menu.replaceChildren(el("div", { class: "recent-head", onmousedown: (e) => e.preventDefault() }, el("span", { text: "Recent searches" }), clear), ...rows);
    // Clicks in the menu keep the focus in the box.
    clear.addEventListener("mousedown", (e) => e.preventDefault());
    active = -1;
    if (menu.hidden) {
      menu.hidden = false;
      menu.classList.remove("in");
      void menu.offsetWidth;
      menu.classList.add("in");
    }
    input.setAttribute("aria-expanded", "true");
  }

  input.addEventListener("focus", open);
  input.addEventListener("blur", close);
  input.addEventListener("input", () => (input.value ? close() : open()));
  input.addEventListener("keydown", (e) => {
    if (!menu.hidden) {
      const n = items.length;
      if (e.key === "ArrowDown") { e.preventDefault(); mark((active + 1) % n); return; }
      if (e.key === "ArrowUp") { e.preventDefault(); mark(active <= 0 ? n - 1 : active - 1); return; }
      if (e.key === "Enter" && active >= 0) { e.preventDefault(); pick(items[active]); return; }
      if (e.key === "Escape") { e.preventDefault(); close(); return; }
    }
    if (e.key === "Enter" && input.value.trim().length >= 2) { clearTimeout(saveTimer); save(input.value); }
  });
  document.addEventListener("pointerdown", (e) => { if (!menu.contains(e.target) && !box.contains(e.target)) close(); });

  return (term) => {
    clearTimeout(saveTimer);
    if (term.length < 2) return;
    saveTimer = setTimeout(() => { if (input.value.trim() === term) save(term); }, SETTLE_MS);
  };
}
