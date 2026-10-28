// "Subtitles" on Download and in the detail panel: which languages a title
// gets fetched as its files arrive (subfetch.rs). Each change saves at once.
import { $, closeDialog, el, icon, invoke, toast } from "./api.js";

const CC = "M4 6h16v12H4zM10 10.5a2 2 0 1 0 0 3M16 10.5a2 2 0 1 0 0 3";

const nameOf = (c, code) => c?.languages.find(([k]) => k === code)?.[1] || code;
const label = (c) => (!c ? "Subtitles" : !c.langs.length ? "Subtitles: None" : c.langs.length === 1 ? `Subtitles: ${nameOf(c, c.langs[0])}` : `Subtitles: ${c.langs.length} languages`);

let openPop = null;
document.addEventListener("pointerdown", (e) => { if (openPop && !openPop.wrap.contains(e.target)) openPop.close(); });

/**
 * The Subtitles button and its list. `commit()` saves the current choice as
 * picked (Download calls it once the title is asked for), so the default the
 * list opened with is kept even if nothing was changed.
 */
export function subtitlesControl(t, opts = {}) {
  let choice = null;
  let failed = "";
  const btn = el("button", { class: `btn soft subs-btn${opts.small ? " small" : ""}`, type: "button", "aria-haspopup": "true", "aria-expanded": "false" }, icon(CC), label(null));
  const pop = el("div", { class: "subs-pop", role: "dialog", "aria-label": "Subtitles", hidden: true });
  const wrap = el("span", { class: "subs-pick" }, btn, pop);
  const ready = invoke("subs_choice", { tile: t }).then((c) => { choice = c; btn.lastChild.textContent = label(c); }).catch((e) => { failed = String(e); });

  const save = async () => {
    btn.lastChild.textContent = label(choice);
    try { await invoke("subs_set", { key: choice.key, title: choice.title, langs: choice.langs }); choice.picked = true; }
    catch (e) { toast(String(e), true); }
  };

  function draw() {
    if (!choice) {
      pop.replaceChildren(el("p", { class: "subs-note", text: failed ? `Couldn't check this title: ${failed}` : "Checking..." }));
      return;
    }
    const row = (code, text) => {
      const on = code ? choice.langs.includes(code) : !choice.langs.length;
      const box = el("input", { type: code ? "checkbox" : "radio", checked: on, name: "subs-none" });
      box.addEventListener("change", () => {
        if (!code) choice.langs = [];
        else if (box.checked) choice.langs = [...choice.langs, code];
        else choice.langs = choice.langs.filter((x) => x !== code);
        draw();
        save();
      });
      return el("label", { class: `subs-row${on ? " on" : ""}` }, box, el("span", { text }));
    };
    const chosen = choice.languages.filter(([k]) => choice.langs.includes(k));
    const rest = choice.languages.filter(([k]) => !choice.langs.includes(k));
    const list = el("div", { class: "subs-list" }, row(null, "None"), ...chosen.map(([k, n]) => row(k, n)), ...rest.map(([k, n]) => row(k, n)));
    const keep = pop.querySelector(".subs-list")?.scrollTop || 0;
    pop.replaceChildren(
      el("div", { class: "subs-head" }, el("b", { text: "Subtitles" }), choice.original ? el("small", { text: `Spoken in ${choice.original}` }) : null),
      list,
      choice.signed_in ? null : el("p", { class: "subs-note" }, "Using free sources only. ",
        el("a", { href: "#settings/account", onclick: () => { close(); closeDialog($("#detail")); } }, "Sign in to OpenSubtitles"), " for more."));
    pop.querySelector(".subs-list").scrollTop = keep;
  }

  function close() {
    pop.hidden = true;
    btn.setAttribute("aria-expanded", "false");
    if (openPop?.wrap === wrap) openPop = null;
  }
  async function open() {
    openPop?.close();
    openPop = { wrap, close };
    draw();
    pop.hidden = false;
    btn.setAttribute("aria-expanded", "true");
    await ready;
    if (!pop.hidden) draw();
  }
  btn.addEventListener("click", () => (pop.hidden ? open() : close()));
  // Esc closes the list, not the whole panel.
  pop.addEventListener("keydown", (e) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); btn.focus(); } });

  return {
    el: wrap,
    async commit() {
      await ready;
      if (choice && !choice.picked) await save();
    },
  };
}
