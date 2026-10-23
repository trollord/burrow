// withGlobalTauri is on and this template has no bundler, so the API comes from the injected
// global rather than npm imports — a bare `import "@tauri-apps/api/core"` would 404 in the webview.
const { invoke } = window.__TAURI__.core;
const { openUrl } = window.__TAURI__.opener;

import { mount, PLACES, home } from "./globe.js";

const $ = (id) => document.getElementById(id);
const show = (el, on) => { el.hidden = !on; };

let wgInstalled = false;
let chosen = null;       // region id selected on screen 2
let plans = {};          // region id -> { usd_per_month, transfer_gb, bundle_id }
let node = null;         // the live node, once there is one
let costTimer = 0;

// ── screens ───────────────────────────────────────────────────────────────

const SCREENS = ["s-hero", "s-creds", "s-region", "s-progress", "s-ready"];
function go(id) {
  for (const s of SCREENS) $(s).classList.toggle("on", s === id);
}

// Two globes: one idling on the hero, one focused during the dig.
const heroGlobe = mount($("globe"));
const progGlobe = mount($("globe2"), { targets: [] });

document.querySelectorAll("[data-open]").forEach((b) =>
  b.addEventListener("click", () => openUrl(b.dataset.open))
);

$("go-start").addEventListener("click", () => go("s-creds"));
$("back-hero").addEventListener("click", () => go("s-hero"));
$("back-creds").addEventListener("click", () => go("s-creds"));

// ── 1. credentials ────────────────────────────────────────────────────────

$("creds-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const btn = $("test-conn"), msg = $("creds-msg");
  btn.disabled = true;
  msg.textContent = "Asking AWS…";
  msg.className = "msg muted";
  try {
    await invoke("save_credentials", {
      accessKeyId: $("akid").value,
      secret: $("secret").value,
    });
    msg.textContent = "Connected.";
    msg.className = "msg ok";
    $("secret").value = "";   // don't leave the secret in the DOM once it's in the keychain
    await enterRegions();
  } catch (err) {
    msg.textContent = String(err);
    msg.className = "msg err";
  } finally {
    btn.disabled = false;
  }
});

// ── 2. regions ────────────────────────────────────────────────────────────

async function enterRegions() {
  go("s-region");
  const wrap = $("region-groups");
  if (wrap.childElementCount) return;

  const regions = await invoke("regions");
  const groups = new Map();
  for (const r of regions) {
    if (!groups.has(r.group)) groups.set(r.group, []);
    groups.get(r.group).push(r);
  }

  for (const [group, list] of groups) {
    const h = document.createElement("div");
    h.className = "group-title";
    h.textContent = group;
    wrap.append(h);
    const grid = document.createElement("div");
    grid.className = "grid";
    for (const r of list) {
      const b = document.createElement("button");
      b.className = "region";
      b.type = "button";
      b.setAttribute("aria-pressed", "false");
      b.dataset.region = r.id;
      b.innerHTML = `<b></b><span class="sub" data-price>checking…</span>`;
      b.querySelector("b").textContent = r.label;
      b.addEventListener("click", () => pick(r.id));
      grid.append(b);
    }
    wrap.append(grid);
  }

  // Prices are fetched per region, in parallel. They differ in ways people do not expect —
  // Sydney bills the same $5 for half the traffic — so they belong on the card, not in a footnote.
  for (const r of regions) {
    invoke("region_plan", { region: r.id })
      .then((p) => {
        plans[r.id] = p;
        const sub = wrap.querySelector(`[data-region="${r.id}"] [data-price]`);
        if (sub) sub.textContent = `$${p.usd_per_month.toFixed(2)}/mo · ${p.transfer_gb} GB`;
      })
      .catch(() => {
        const sub = wrap.querySelector(`[data-region="${r.id}"] [data-price]`);
        if (sub) sub.textContent = "price unavailable";
      });
  }
}

function pick(id) {
  chosen = id;
  for (const b of document.querySelectorAll(".region")) {
    b.setAttribute("aria-pressed", String(b.dataset.region === id));
  }
  $("go-connect").disabled = false;
  const p = plans[id];
  $("region-msg").textContent = p
    ? `${PLACES[id]?.name ?? id} · ${p.transfer_gb} GB of traffic included`
    : "";
}

// ── 3. digging ────────────────────────────────────────────────────────────

function mark(key, state) {
  const li = document.querySelector(`#prog-steps [data-k="${key}"]`);
  if (li) li.className = state;
}

$("go-connect").addEventListener("click", async () => {
  if (!chosen) return;
  go("s-progress");
  progGlobe.setTargets([chosen]);
  progGlobe.setFocus(chosen);
  $("prog-title").textContent = `Digging to ${PLACES[chosen]?.name ?? chosen}`;
  $("prog-sub").textContent = "This takes about two minutes.";
  $("prog-msg").textContent = "";
  $("prog-msg").className = "msg";
  show($("prog-actions"), false);
  for (const k of ["ip", "vm", "wg", "fw"]) mark(k, "");

  // The backend doesn't stream progress yet, so these are paced to the real sequence rather than
  // invented: the address is immediate, the VM takes ~40s to report running, and apt is the long
  // pole. Paced honestly, so a stall still reads as a stall instead of a fake finish.
  mark("ip", "doing");
  const pace = [
    [1200,  () => { mark("ip", "done"); mark("vm", "doing"); }],
    [45000, () => { mark("vm", "done"); mark("wg", "doing"); }],
  ];
  const timers = pace.map(([ms, fn]) => setTimeout(fn, ms));

  try {
    node = await invoke("create_node", { region: chosen });
    timers.forEach(clearTimeout);
    for (const k of ["ip", "vm", "wg", "fw"]) mark(k, "done");
    await enterReady();
  } catch (err) {
    timers.forEach(clearTimeout);
    $("prog-title").textContent = "That didn't work";
    $("prog-sub").textContent = "";
    $("prog-msg").textContent = String(err);
    $("prog-msg").className = "msg err";
    show($("prog-actions"), true);
  }
});

$("prog-back").addEventListener("click", () => go("s-region"));

// ── 4. ready + cost ───────────────────────────────────────────────────────

const money = (n) => `$${n.toFixed(2)}`;

function paintCost() {
  if (!node) return;
  const plan = plans[node.region];
  if (!plan) return;
  // Lightsail bills hourly against a monthly cap, so the honest hourly figure is month/730 and
  // "so far" is capped at the monthly price rather than growing past it.
  const perHour = plan.usd_per_month / 730;
  const secs = node.created_at ? Math.max(0, Date.now() / 1000 - node.created_at) : 0;
  const hours = secs / 3600;

  $("c-sofar").textContent = money(Math.min(perHour * hours, plan.usd_per_month));
  $("c-uptime").textContent =
    hours < 1 ? `${Math.round(secs / 60)} min running` : `${hours.toFixed(1)} h running`;
  $("c-hour").textContent = `${money(perHour)} per hour`;
  $("c-day").textContent = money(perHour * 24);
  $("c-month").textContent = money(plan.usd_per_month);
  $("c-transfer").textContent = `${plan.transfer_gb} GB included`;
}

async function enterReady() {
  go("s-ready");
  const region = node.region;
  $("ready-title").textContent = `You're in ${PLACES[region]?.name ?? region}`;
  $("ready-sub").textContent = `Server running at ${node.endpoint}. Nothing is connected yet.`;
  show($("wg-warn"), !wgInstalled);
  $("connect").disabled = !wgInstalled;
  paintTunnel(await invoke("tunnel_status", { region }).catch(() => null));
  if (!plans[region]) {
    try { plans[region] = await invoke("region_plan", { region }); } catch { /* cost stays blank */ }
  }
  paintCost();
  clearInterval(costTimer);
  costTimer = setInterval(paintCost, 10000);
}

// The indicator answers the only question that matters — is my traffic actually leaving through
// the exit node — rather than the easier one of whether an interface exists. An interface that is
// up but never handshook looks identical to a working VPN until you check your address.
let connected = false;
function paintTunnel(st) {
  connected = !!(st && st.tunnelled);
  const title = $("conn-title"), sub = $("conn-sub"), btn = $("connect");
  if (connected) {
    title.textContent = `Connected via ${PLACES[node?.region]?.name ?? node?.region}`;
    sub.textContent = `The internet sees you at ${st.public_ip}.`;
    btn.textContent = "Disconnect";
    btn.classList.remove("primary");
  } else if (st && st.up) {
    title.textContent = "Tunnel up, but not carrying traffic";
    sub.textContent = st.public_ip ? `You still appear at ${st.public_ip}.` : "";
    btn.textContent = "Reconnect";
    btn.classList.add("primary");
  } else {
    title.textContent = "Not connected";
    sub.textContent = "Your traffic is still going out over your own connection.";
    btn.textContent = "Connect";
    btn.classList.add("primary");
  }
}

$("connect").addEventListener("click", async () => {
  if (!node) return;
  const btn = $("connect"), msg = $("conn-msg");
  btn.disabled = true;
  msg.className = "msg muted";
  msg.textContent = connected ? "Disconnecting…" : "Asking for permission…";
  try {
    const st = connected
      ? await invoke("disconnect", { region: node.region })
      : await invoke("connect", { region: node.region });
    paintTunnel(st);
    msg.textContent = "";
  } catch (err) {
    // Dismissing the password dialog is a decision, not a fault; say so plainly.
    msg.textContent = String(err) === "cancelled" ? "Cancelled." : String(err);
    msg.className = String(err) === "cancelled" ? "msg muted" : "msg err";
    paintTunnel(await invoke("tunnel_status", { region: node.region }).catch(() => null));
  } finally {
    btn.disabled = false;
  }
});

$("change-region").addEventListener("click", () => go("s-region"));

$("destroy").addEventListener("click", async () => {
  const btn = $("destroy"), msg = $("ready-msg");
  btn.disabled = true;
  msg.textContent = "Destroying…";
  msg.className = "msg muted";
  try {
    const steps = await invoke("destroy_node", { region: node.region });
    clearInterval(costTimer);
    node = null;
    msg.textContent = steps.join(" · ");
    msg.className = "msg ok";
    setTimeout(() => go("s-region"), 900);
  } catch (err) {
    msg.textContent = String(err);
    msg.className = "msg err";
  } finally {
    btn.disabled = false;
  }
});

// ── startup ───────────────────────────────────────────────────────────────

heroGlobe.setTargets(Object.keys(PLACES));
go("s-hero");

const state = await invoke("setup_state");
wgInstalled = state.wireguard_installed;

if (state.has_credentials && state.identity) {
  // Already set up. Look for a node that is already running before dropping them on the picker,
  // so reopening the app lands where they left off rather than back at step one.
  await enterRegions();
  for (const id of Object.keys(PLACES)) {
    try {
      const n = await invoke("node_status", { region: id });
      if (n) { node = n; await enterReady(); break; }
    } catch { /* a region that errors is simply not where the node is */ }
  }
}
