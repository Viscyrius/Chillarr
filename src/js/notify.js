// Notify me (notify.rs): a note when a title you picked comes out, per profile.
// The button sits on a title's panel; the notes pop up on every screen of that
// profile, and ones a screen missed show the next time it opens.
import { el, icon, ICON, invoke, listen, personKey, toast, WEB } from "./api.js";

const BELL = "M6 16V11a6 6 0 0 1 12 0v5l2 2H4zM10 20a2 2 0 0 0 4 0";

/**
 * Notify me for a title, and Get it too once it's on. `target` is
 * { tile } for a film or show, or { book: { title, author, cover, anilist } }.
 * Returns both buttons, for the panel's row of actions.
 */
export function notifyButton(target, label = "Notify me") {
  const b = el("button", { class: "btn soft save-btn", title: "Get a notification when it comes out" }, icon(BELL), label);
  const get = el("button", { class: "btn soft save-btn", title: "Download it by itself as soon as it's out", hidden: true }, icon(ICON.download), "Get it too");
  let st = { on: false, auto: false, can_get: false };
  const draw = () => {
    b.classList.toggle("on", st.on);
    b.replaceChildren(icon(st.on ? ICON.check : BELL), label);
    get.hidden = !st.on || !st.can_get;
    get.classList.toggle("on", st.auto);
    get.replaceChildren(icon(st.auto ? ICON.check : ICON.download), "Get it too");
  };
  invoke("notify_state", { target }).then((s) => { st = s; draw(); }).catch(() => { b.remove(); get.remove(); });
  const send = async (on, auto, btn) => {
    btn.disabled = true;
    try {
      toast(await invoke("notify_set", { target, on, auto }));
      st = { ...st, on, auto: on && auto };
      draw();
    } catch (e) { toast(String(e), true); }
    btn.disabled = false;
  };
  b.addEventListener("click", () => send(!st.on, false, b));
  get.addEventListener("click", () => send(true, !st.auto, get));
  return [b, get];
}

/** What this profile missed while this screen was closed. */
async function showMissed() {
  let notes;
  try { notes = await invoke("notify_unseen"); } catch (e) { return; }
  if (!notes.length) return;
  const last = notes[notes.length - 1].text;
  toast(notes.length === 1 ? last : `${last} (and ${notes.length - 1} more)`);
}

let started = false;
let lastPerson = null;
/** Called once the profile is known. */
export function startNotes() {
  if (started) return;
  started = true;
  lastPerson = personKey();
  showMissed();
  // Phones and TV: a note while the page is open pops up here (the PC shows its own).
  if (WEB) listen("release-note", (e) => { toast(e.payload.text); invoke("notify_unseen").catch(() => {}); });
  // Another profile now: what it missed.
  window.addEventListener("chillarr:refresh", () => {
    if (personKey() === lastPerson) return;
    lastPerson = personKey();
    showMissed();
  });
}
