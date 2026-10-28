// Profiles: "Who's watching?" on start (when there's more than one), the
// avatar in the top bar, and adding or editing a profile. Each profile is a
// Jellyfin account of its own (profiles.rs), so its history and My List
// follow it to Jellyfin's TV and phone apps. In Jellyfin's web pages
// (WEB) it's pick and add only, and a pick signs the browser in as them.
import { $, ask, el, icon, invoke, toast, WEB } from "./api.js";

const COLORS = ["#e50914", "#f5a623", "#2ecc71", "#1abc9c", "#3498db", "#9b59b6", "#e84393", "#95a5a6"];
const LOCK = "M7 11V8a5 5 0 0 1 10 0v3M6 11h12v9H6z";
const PENCIL = "M4 20h4L19 9l-4-4L4 16zM14 6l4 4";
const root = $("#profiles");
let list = [];

const initial = (name) => (name.trim()[0] || "?").toUpperCase();

function avatar(p, size = "") {
  return el("span", { class: `pf-avatar ${size}`, style: `--c:${p.color}`, "aria-hidden": "true" }, initial(p.name));
}

async function load() {
  // On the web, this device's key: the main profile asks for its password only once per device.
  list = await invoke("profiles_list", WEB ? { trust: window.__CHILLARR_JF__.trust() } : {}).catch(() => []);
  drawTopbar();
  return list;
}

/** Called once the app has started: asks who's watching when there's a choice. */
export async function startProfiles() {
  await load();
  // More than one profile, or a PIN to ask for: someone has to pick first.
  if (list.length > 1 || list.some((p) => p.has_pin)) {
    root.dataset.startup = "1";
    openPicker();
  }
}

/** Jellyfin's web pages: the switcher, when the signed-in user is one of the
 *  profiles. Otherwise app.js's avatar for who's signed in stays. */
export async function startProfilesWeb() {
  await load();
}

/* ---------- Top bar ---------- */
function drawTopbar() {
  const slot = $("#profile-slot");
  const me = list.find((p) => p.active);
  // On the web the signed-in user's avatar (app.js) stays when they aren't a profile.
  if (!me) { if (!WEB) slot.replaceChildren(); return; }
  const btn = el("button", { class: "pf-top", "aria-label": `Profile: ${me.name}. Switch profile`, title: me.name, onclick: () => openPicker() }, avatar(me, "small"));
  slot.replaceChildren(btn);
}

/* ---------- Who's watching? ---------- */
function openPicker(manage = false) {
  const canClose = list.length && list.some((p) => p.active) && !root.dataset.startup;
  const tiles = list.map((p) => {
    const b = el("button", { class: "pf-tile", style: `--c:${p.color}` },
      el("span", { class: "pf-face" }, avatar(p, "big"), manage ? el("span", { class: "pf-edit" }, icon(PENCIL)) : null),
      el("span", { class: "pf-name" }, p.name, p.has_pin && !manage ? el("span", { class: "pf-lock" }, icon(LOCK)) : null));
    b.addEventListener("click", () => (manage ? edit(p) : choose(p)));
    return b;
  });
  if (list.length < 8) {
    tiles.push(el("button", { class: "pf-tile pf-add", onclick: () => openEditor(null) },
      el("span", { class: "pf-face" }, el("span", { class: "pf-avatar big plus" }, "+")), el("span", { class: "pf-name" }, "Add profile")));
  }
  root.replaceChildren(el("div", { class: "pf-inner" },
    el("h1", { text: manage ? "Manage profiles" : "Who's watching?" }),
    el("div", { class: "pf-grid" }, tiles),
    el("div", { class: "pf-foot" },
      WEB ? el("button", { class: "btn ghost", onclick: () => { closePicker(); location.hash = "settings/account"; } }, "Account")
        : el("button", { class: "btn ghost", onclick: () => openPicker(!manage) }, manage ? "Done" : "Manage profiles"),
      canClose && !manage ? el("button", { class: "btn ghost", onclick: closePicker }, "Back") : null)));
  root.hidden = false;
  root.classList.remove("out");
  document.body.style.overflow = "hidden";
  setBehind(true);
  root.querySelector(".pf-tile")?.focus();
}

/** Jellyfin's apps' back button: closes the picker. */
export function closeProfiles() { if (!root.hidden) closePicker(); }

function closePicker() {
  root.classList.add("out");
  setTimeout(() => { root.hidden = true; root.classList.remove("out"); root.replaceChildren(); delete root.dataset.startup; }, 260);
  document.body.style.overflow = "";
  setBehind(false);
}

// The page behind the picker can't be reached while it's open: not by Tab,
// and not by the phone remote's typing. The yes/no dialog and toasts stay live.
function setBehind(off) {
  for (const x of document.body.children) {
    if (x !== root && x.id !== "ask" && x.id !== "toast" && x.tagName !== "SCRIPT") x.toggleAttribute("inert", off);
  }
}

async function choose(p, pin = null) {
  if (WEB) return chooseWeb(p, pin);
  if (p.has_pin && pin == null) { pinPad(p); return; }
  try {
    await invoke("profiles_switch", { id: p.id, pin });
  } catch (e) {
    if (p.has_pin) { pinPad(p, String(e)); return; }
    toast(String(e), true);
    return;
  }
  await load();
  closePicker();
  window.dispatchEvent(new Event("chillarr:refresh"));
  if (list.length > 1) toast(`Watching as ${p.name}.`);
}

/** Jellyfin's web pages: you're already the active one; anyone else is a sign-in
 *  as them (the shim), after their PIN, or the owner's Jellyfin password. */
async function chooseWeb(p, pin = null, password = null) {
  if (p.active) { closePicker(); return; }
  if (p.has_pin && pin == null) { pinPad(p); return; }
  if (p.needs_password && password == null) { passwordPad(p, "", pin); return; }
  toast(`Switching to ${p.name}...`);
  try { await window.__CHILLARR_JF__.switchProfile(p.id, pin, password); }
  catch (e) {
    // Asked again where it went wrong: a PIN error at the PIN, else where it was last typed.
    if (pin != null && /PIN/.test(String(e))) pinPad(p, String(e));
    else if (password != null) passwordPad(p, String(e), pin);
    else if (pin != null) pinPad(p, String(e));
    else toast(String(e), true);
  }
}

/** An account that needs its Jellyfin password from a browser. `pin`: one typed before it. */
function passwordPad(p, error = "", pin = null) {
  const pass = el("input", { type: "password", autocomplete: "current-password", placeholder: "Password", "aria-label": `${p.name}'s Jellyfin password` });
  const go = el("button", { class: "btn primary" }, "Continue");
  const err = el("p", { class: "pf-pin-err", text: error });
  go.addEventListener("click", () => {
    if (!pass.value) { err.textContent = "Type the password."; return; }
    go.disabled = true;
    go.textContent = "Signing in...";
    chooseWeb(p, pin, pass.value);
  });
  pass.addEventListener("keydown", (e) => { if (e.key === "Enter") go.click(); if (e.key === "Escape") openPicker(); });
  root.replaceChildren(el("div", { class: "pf-inner pf-pin" },
    avatar(p, "big"),
    el("p", { class: "pf-pin-lead", text: `Enter ${p.name}'s Jellyfin password` }),
    el("div", { class: error ? "pf-fields pf-pass shake" : "pf-fields pf-pass" }, pass),
    err,
    p.owner ? el("p", { class: "pf-sub pf-pass-hint", text: "This browser will remember it. Give the main profile a PIN if others use this browser." }) : null,
    el("div", { class: "pf-foot" }, go, el("button", { class: "btn ghost", onclick: () => openPicker() }, "Back"))));
  pass.focus();
}

/* ---------- PIN ---------- */
// `submit` gets the 4 digits (switching by default), `back` leaves the pad,
// `forgot` (optional) adds the way out for a forgotten PIN.
function pinPad(p, error = "", submit = (pin) => choose(p, pin), back = () => openPicker(), forgot = null) {
  const boxes = Array.from({ length: 4 }, () => el("input", { class: "pf-pin-box", type: "password", inputmode: "numeric", maxlength: "1", autocomplete: "off", "aria-label": "PIN digit" }));
  const value = () => boxes.map((b) => b.value).join("");
  boxes.forEach((b, i) => {
    b.addEventListener("input", () => {
      b.value = b.value.replace(/\D/g, "").slice(-1);
      if (b.value && i < 3) boxes[i + 1].focus();
      if (value().length === 4) submit(value());
    });
    b.addEventListener("keydown", (e) => {
      if (e.key === "Backspace" && !b.value && i > 0) boxes[i - 1].focus();
      if (e.key === "Escape") back();
    });
  });
  root.replaceChildren(el("div", { class: "pf-inner pf-pin" },
    avatar(p, "big"),
    el("p", { class: "pf-pin-lead", text: `Enter the PIN for ${p.name}` }),
    el("div", { class: error ? "pf-pin-row shake" : "pf-pin-row" }, boxes),
    el("p", { class: "pf-pin-err", text: error }),
    el("div", { class: "pf-foot" },
      el("button", { class: "btn ghost", onclick: back }, "Back"),
      forgot ? el("button", { class: "btn ghost", onclick: forgot }, "Forgot PIN? Use your Chillarr login") : null)));
  boxes[0].focus();
}

/* ---------- Manage ---------- */
// A profile with a PIN opens for editing only with that PIN (the backend
// checks it again on every change).
function edit(p) {
  if (!p.has_pin) { openEditor(p); return; }
  const back = () => openPicker(true);
  const forgot = p.owner ? null : () => forgotPin(p);
  const submit = async (pin) => {
    try { await invoke("profiles_check_pin", { id: p.id, pin }); } catch (e) { pinPad(p, String(e), submit, back, forgot); return; }
    openEditor(p, pin);
  };
  pinPad(p, "", submit, back, forgot);
}

// A forgotten PIN: the profile can still be removed with the Chillarr login.
function forgotPin(p) {
  const pass = el("input", { type: "password", autocomplete: "current-password", placeholder: "Your Chillarr password", "aria-label": "Your Chillarr password" });
  const err = el("p", { class: "err" });
  const go = el("button", { class: "btn danger-fill" }, "Remove profile");
  go.addEventListener("click", async () => {
    err.textContent = "";
    if (!pass.value) { err.textContent = "Type your Chillarr password."; return; }
    const ok = await ask(`Remove ${p.name}?`, `Their Jellyfin account goes too: what they watched, where they stopped, and their My List. Downloaded movies and shows stay.`, [], "Remove");
    if (!ok) return;
    go.disabled = true;
    try { await invoke("profiles_remove", { id: p.id, login: pass.value }); await load(); window.dispatchEvent(new Event("chillarr:refresh")); openPicker(true); } catch (e) { err.textContent = String(e); go.disabled = false; }
  });
  pass.addEventListener("keydown", (e) => { if (e.key === "Enter") go.click(); });
  root.replaceChildren(el("div", { class: "pf-inner pf-editor" },
    el("h1", { text: `Remove ${p.name}` }),
    el("p", { class: "pf-sub", text: "Forgot the PIN? Type the password you picked when you set up Chillarr to remove this profile." }),
    el("div", { class: "pf-fields" }, pass),
    err,
    el("div", { class: "pf-foot" }, go, el("button", { class: "btn ghost", onclick: () => edit(p) }, "Back"))));
  pass.focus();
}

/* ---------- Add / edit ---------- */
// `currentPin`: the PIN typed to open a profile that has one; every change sends it.
function openEditor(p, currentPin = null) {
  const isNew = !p;
  let color = p?.color || COLORS[list.length % COLORS.length];
  const name = el("input", { type: "text", value: p?.name || "", maxlength: "30", placeholder: "Name", "aria-label": "Name" });
  const face = el("span", { class: "pf-avatar big", style: `--c:${color}` }, initial(p?.name || "?"));
  name.addEventListener("input", () => (face.textContent = initial(name.value || "?")));
  const swatches = el("div", { class: "pf-colors", role: "radiogroup", "aria-label": "Colour" }, COLORS.map((c) => {
    const s = el("button", { class: c === color ? "pf-swatch on" : "pf-swatch", style: `--c:${c}`, role: "radio", "aria-checked": String(c === color), "aria-label": c });
    s.addEventListener("click", () => {
      color = c;
      face.style.setProperty("--c", c);
      swatches.querySelectorAll(".pf-swatch").forEach((x) => { x.classList.toggle("on", x === s); x.setAttribute("aria-checked", String(x === s)); });
    });
    return s;
  }));
  const pin = el("input", { type: "password", inputmode: "numeric", maxlength: "4", placeholder: p?.has_pin ? "New PIN (4 digits)" : "PIN (optional, 4 digits)", "aria-label": "PIN", autocomplete: "off" });
  pin.addEventListener("input", () => (pin.value = pin.value.replace(/\D/g, "").slice(0, 4)));
  // Not the owner: a password lets this person sign in to Jellyfin's own apps
  // on their devices, and join a watch party from there.
  const jfPass = p?.owner || WEB ? null : el("input", { type: "password", autocomplete: "new-password", placeholder: "Password for Jellyfin apps (optional)", "aria-label": "Password for Jellyfin apps" });
  const err = el("p", { class: "err" });
  const save = el("button", { class: "btn primary" }, isNew ? "Add profile" : "Save");
  save.addEventListener("click", async () => {
    err.textContent = "";
    if (!name.value.trim()) { err.textContent = "Give the profile a name."; return; }
    if (pin.value && pin.value.length !== 4) { err.textContent = "A PIN is 4 digits."; return; }
    if (jfPass?.value && jfPass.value.length < 5) { err.textContent = "Use at least 5 characters for the password."; return; }
    save.disabled = true;
    try {
      if (WEB) {
        // Straight in as them: they just typed the PIN, so it isn't asked again.
        const made = await invoke("profiles_add", { name: name.value, color, pin: pin.value });
        save.textContent = "Switching...";
        // It's made either way: a failed switch goes back to the picker, not to a second Add.
        try { await window.__CHILLARR_JF__.switchProfile(made.id, pin.value || null, null); }
        catch (e) { toast(String(e), true); await load(); openPicker(); }
        return;
      }
      let id = p?.id;
      if (isNew) id = (await invoke("profiles_add", { name: name.value, color, pin: pin.value || null })).id;
      else await invoke("profiles_update", { id: p.id, name: name.value !== p.name ? name.value : null, color, pin: pin.value || null, currentPin });
      if (jfPass?.value) {
        // A PIN just set or changed is the one it asks for now.
        const listed = await invoke("profiles_set_password", { id, password: jfPass.value, currentPin: pin.value || currentPin });
        toast(listed
          ? `${name.value.trim()} now shows on Jellyfin's sign-in screen. Sign in there with that password.`
          : `Password set. Jellyfin's sign-in screen doesn't list ${name.value.trim()} yet, so type the name there.`);
      }
      await load();
      openPicker(!isNew);
    } catch (e) { err.textContent = String(e); save.disabled = false; }
  });
  const extras = [];
  if (p?.has_pin) {
    extras.push(el("button", { class: "btn ghost small", onclick: async () => {
      try { await invoke("profiles_update", { id: p.id, pin: "", currentPin }); await load(); toast(`${p.name} has no PIN now.`); openPicker(true); } catch (e) { err.textContent = String(e); }
    } }, "Remove PIN"));
  }
  if (p && !p.owner) {
    extras.push(el("button", { class: "btn ghost small danger", onclick: async () => {
      const ok = await ask(`Remove ${p.name}?`, `Their Jellyfin account goes too: what they watched, where they stopped, and their My List. Downloaded movies and shows stay.`, [], "Remove");
      if (!ok) return;
      try { await invoke("profiles_remove", { id: p.id, pin: currentPin }); await load(); window.dispatchEvent(new Event("chillarr:refresh")); openPicker(true); } catch (e) { err.textContent = String(e); }
    } }, "Remove profile"));
  }
  root.replaceChildren(el("div", { class: "pf-inner pf-editor" },
    el("h1", { text: isNew ? "Add a profile" : "Edit profile" }),
    isNew ? el("p", { class: "pf-sub", text: "Each profile keeps its own Continue watching and My List. It's also a Jellyfin account, so it works the same in Jellyfin's TV and phone apps." }) : null,
    el("div", { class: "pf-edit-row" }, face, el("div", { class: "pf-fields" }, name, swatches, pin, jfPass)),
    err,
    el("div", { class: "pf-foot" }, save, el("button", { class: "btn ghost", onclick: () => openPicker(!isNew) }, "Cancel"), ...extras)));
  name.focus();
}

// Opened from elsewhere (Settings, the phone remote): the picker.
window.addEventListener("chillarr:profiles", () => load().then(() => openPicker()));
// Signing in or out changes who the owner is.
window.addEventListener("chillarr:refresh", () => { if (root.hidden) load(); });
