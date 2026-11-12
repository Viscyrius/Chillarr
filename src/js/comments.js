// Comments on a title's page, under Similar to this. Everyone on this
// Chillarr sees the same ones (comments.rs); you can edit and delete your own.
// Comments are people's own words: they only ever go in as text (el's
// "text"), never as HTML.
//
// One section per page that shows them: a film's or show's panel (detail.js)
// and a reading title's panel (read.js), each with its own state.
import { ask, el, invoke, toast } from "./api.js";

const MAX = 2000;

const initial = (name) => (name.trim()[0] || "?").toUpperCase();

/** "just now", "5 min ago", "3 days ago", then the date. */
function ago(at) {
  const s = Math.max(0, Date.now() / 1000 - at);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) { const h = Math.floor(s / 3600); return `${h} hour${h > 1 ? "s" : ""} ago`; }
  if (s < 7 * 86400) { const d = Math.floor(s / 86400); return `${d} day${d > 1 ? "s" : ""} ago`; }
  const d = new Date(at * 1000);
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short", year: d.getFullYear() === new Date().getFullYear() ? undefined : "numeric" });
}

/** A text box that grows with what's typed and shows how much room is left near the end. */
function writer(value, placeholder) {
  const box = el("textarea", { class: "cm-input", rows: "2", maxlength: String(MAX), placeholder, "aria-label": placeholder });
  box.value = value;
  const left = el("small", { class: "cm-left" });
  const fit = () => {
    box.style.height = "auto";
    box.style.height = `${Math.min(box.scrollHeight + 2, 320)}px`;
    const n = MAX - box.value.length;
    left.textContent = n < 200 ? `${n} left` : "";
  };
  box.addEventListener("input", fit);
  // Typing keys are for the text, not the app's shortcuts ("/" is search).
  // Up at the start and Down at the end move on, like any field; Esc and a
  // remote's Back still reach the panel.
  box.addEventListener("keydown", (e) => {
    if ((e.key === "ArrowUp" && box.selectionEnd === 0) || (e.key === "ArrowDown" && box.selectionStart === box.value.length)) return;
    if (e.key.length === 1 || /^(Enter|Backspace|Delete|Arrow)/.test(e.key)) e.stopPropagation();
  });
  requestAnimationFrame(fit);
  return { box, left, fit };
}

/** A comments section of its own: `box` goes in the page, `load` fills it. */
export function makeComments(id) {
  const box = el("section", { class: "comments", id, "aria-label": "Comments", hidden: true });
  let seq = 0;
  let key = null;
  let total = 0;

  function count(by) {
    total += by;
    const h = box.querySelector(".cm-count");
    if (h) h.textContent = total ? String(total) : "";
    const empty = box.querySelector(".cm-empty");
    if (empty) empty.hidden = total > 0;
  }

  function item(c) {
    const text = el("p", { class: "cm-text", text: c.text });
    const when = el("time", { class: "cm-when", datetime: new Date(c.at * 1000).toISOString(), title: new Date(c.at * 1000).toLocaleString(), text: ago(c.at) });
    const head = el("div", { class: "cm-head" }, el("b", { text: c.name }), when, c.edited ? el("small", { class: "cm-edited", text: "edited" }) : null);
    const body = el("div", { class: "cm-body" }, head, text);
    const row = el("li", { class: "cm-item", dataset: { id: c.id } }, el("span", { class: "cm-avatar", "aria-hidden": "true" }, initial(c.name)), body);
    if (!c.mine) return row;
    const tools = el("div", { class: "cm-tools" },
      el("button", { class: "link-btn", onclick: () => editIn(c, row, body) }, "Edit"),
      el("button", { class: "link-btn", onclick: () => remove(c, row) }, "Delete"));
    body.append(tools);
    return row;
  }

  function editIn(c, row, body) {
    const k = key;
    const w = writer(c.text, "Edit your comment");
    const err = el("p", { class: "err cm-err" });
    const save = el("button", { class: "btn small primary" }, "Save");
    const cancel = el("button", { class: "btn small ghost", onclick: () => row.replaceWith(item(c)) }, "Cancel");
    save.addEventListener("click", async () => {
      err.textContent = "";
      save.disabled = true;
      try {
        const done = await invoke("comments_edit", { key: k, commentId: c.id, text: w.box.value });
        if (k === key) row.replaceWith(item(done));
      } catch (e) { err.textContent = String(e); save.disabled = false; }
    });
    body.replaceChildren(body.firstChild, w.box, el("div", { class: "cm-actions" }, save, cancel, w.left), err);
    w.box.focus();
  }

  async function remove(c, row) {
    const k = key;
    if (!(await ask("Delete this comment?", "It goes for everyone. This can't be undone.", [], "Delete", "Keep it"))) return;
    try {
      await invoke("comments_delete", { key: k, commentId: c.id });
      row.remove();
      count(-1);
    } catch (e) { toast(String(e), true); }
  }

  function draw(list) {
    total = list.length;
    const ul = el("ul", { class: "cm-list" }, list.map(item));
    const w = writer("", "Add a comment");
    const err = el("p", { class: "err cm-err" });
    const post = el("button", { class: "btn small primary" }, "Post");
    post.addEventListener("click", async () => {
      err.textContent = "";
      if (!w.box.value.trim()) { err.textContent = "Write something first."; return; }
      const k = key;
      post.disabled = true;
      try {
        const c = await invoke("comments_add", { key: k, text: w.box.value });
        if (k !== key) return;
        w.box.value = "";
        w.fit();
        ul.prepend(item(c));
        count(1);
      } catch (e) { err.textContent = String(e); }
      post.disabled = false;
    });
    // Ctrl+Enter posts.
    w.box.addEventListener("keydown", (e) => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); post.click(); } });
    box.replaceChildren(
      el("h3", {}, "Comments ", el("span", { class: "cm-count", text: total ? String(total) : "" })),
      el("div", { class: "cm-new" }, w.box, el("div", { class: "cm-actions" }, post, w.left), err),
      el("p", { class: "note cm-empty", text: "No comments yet. Be the first.", hidden: total > 0 }),
      ul);
    box.hidden = false;
  }

  /** Shows the comments kept under `getKey()` (a promise of the key, or null), once the panel has opened. */
  function load(getKey) {
    const my = ++seq;
    key = null;
    box.hidden = true;
    box.replaceChildren();
    setTimeout(async () => {
      if (my !== seq) return;
      const k = await Promise.resolve(getKey()).catch(() => null);
      if (my !== seq || !k) return;
      let list;
      try { list = await invoke("comments_list", { key: k }); } catch (e) { return; }
      if (my !== seq) return;
      key = k;
      draw(list);
    }, 250);
  }

  return { box, load };
}

/* ---------- a film's or show's panel (detail.js) ---------- */
const watching = makeComments("detail-comments");
export const commentsBox = watching.box;
/** Shows the comments for `t`, once the panel has opened. */
export function loadComments(t) {
  watching.load(() => invoke("comments_key", { tile: t }));
}

/* ---------- a reading title's panel (read.js) ---------- */

/** The key a reading title's comments are kept under: the same for every
    copy and source of it (#76). AniList for manga, manhwa, manhua and novels
    when known, Open Library for books, else the followed series or book. */
export function readingKey(c) {
  if (!c) return null;
  if (c.anilist) return `anilist:${c.anilist}`;
  const k = typeof c.key === "string" ? c.key : "";
  if (/^anilist:\d+$/.test(k)) return k;
  const ol = c.olid || (/^ol:([A-Za-z0-9_-]+)$/.exec(k) || [])[1];
  if (ol) return `ol:${ol}`;
  if (/^[mb][0-9a-f]+$/.test(k)) return `read:${k}`;
  return null;
}

/** A comments section for a reading panel: `box` goes in, `show(card)` fills it. */
export function readingComments() {
  const c = makeComments(null);
  let last = null;
  // The panel is drawn again after a change (a chapter marked, a follow):
  // the same title keeps its comments, and what's being typed, as they are.
  return { box: c.box, show: (card) => { const k = readingKey(card); if (k && k === last && c.box.childElementCount) return; last = k; c.load(() => k); } };
}
