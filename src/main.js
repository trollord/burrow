// withGlobalTauri is on and this template has no bundler, so the API comes from the injected
// global rather than npm imports — a bare `import "@tauri-apps/api/core"` would 404 in the webview.
const { invoke } = window.__TAURI__.core;
const { openUrl } = window.__TAURI__.opener;

import { mount, PLACES, home } from "./globe.js";

const $ = (id) => document.getElementById(id);
const show = (el, on) => { el.hidden = !on; };

let wgInstalled = false;
let profiles = [];       // saved AWS keys, names only — no secrets ever reach the frontend
let chosen = null;       // region id selected on screen 2
let plans = {};          // region id -> { usd_per_month, transfer_gb, bundle_id }
let node = null;         // the live node, once there is one
let activeProfile = null;
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

function paintProfiles() {
  const wrap = $("profile-list");
  show($("saved"), profiles.length > 0);
  wrap.replaceChildren();
  for (const p of profiles) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "profile";
    row.setAttribute("aria-pressed", String(p.name === activeProfile));
    const tick = document.createElement("span");
    tick.className = "tick";
    tick.textContent = p.name === activeProfile ? "✓" : "";
    const who = document.createElement("span");
    who.className = "who";
    const b = document.createElement("b");
    b.textContent = p.name;
    const sub = document.createElement("span");
    // Account id and key id, never the secret — the frontend never sees one.
    sub.textContent = `${p.account} · ${p.access_key_id}`;
    who.append(b, sub);
    const drop = document.createElement("span");
    drop.className = "drop";
    drop.textContent = "×";
    drop.title = `Forget ${p.name}`;
    drop.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      await invoke("remove_profile", { name: p.name });
      await refreshProfiles();
    });
    row.append(tick, who, drop);
    row.addEventListener("click", async () => {
      await invoke("select_profile", { name: p.name });
      activeProfile = p.name;
      paintProfiles();
      await enterRegions();
    });
    wrap.append(row);
  }
}

async function refreshProfiles() {
  const st = await invoke("startup");
  profiles = st.profiles;
  activeProfile = st.active;
  wgInstalled = st.wireguard_installed;
  paintProfiles();
}

$("creds-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const btn = $("test-conn"), msg = $("creds-msg");
  btn.disabled = true;
  msg.textContent = "Asking AWS…";
  msg.className = "msg muted";
  try {
    const p = await invoke("add_profile", {
      name: $("pname").value,
      accessKeyId: $("akid").value,
      secret: $("secret").value,
    });
    msg.textContent = `Saved as "${p.name}".`;
    msg.className = "msg ok";
    // Clear both: the secret so it does not linger in the DOM, the rest so the form is ready
    // for another key rather than looking like it failed to submit.
    $("secret").value = ""; $("akid").value = ""; $("pname").value = "";
    await refreshProfiles();
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
  if (wrap.childElementCount) { paintRunning(); return; }

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

  paintRunning();

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

let running = [];   // servers that exist right now, whatever the UI is showing

/// Find every running server and show it before anything else can be created.
///
/// This is the fix for a real failure: after adding a key the app went straight to the picker
/// without ever checking, so a server that had been billing since the night before was invisible
/// until AWS refused to make a second one.
async function paintRunning() {
  try { running = await invoke("all_nodes"); } catch { running = []; }
  const wrap = $("running-list");
  show($("running-panel"), running.length > 0);
  wrap.replaceChildren();

  for (const n of running) {
    const row = document.createElement("div");
    row.className = "running";

    const pip = document.createElement("span");
    pip.className = "pip";

    const what = document.createElement("span");
    what.className = "what";
    const b = document.createElement("b");
    b.textContent = PLACES[n.region]?.name ?? n.region;
    const sub = document.createElement("span");
    const up = n.created_at ? Date.now() / 1000 - n.created_at : 0;
    const plan = plans[n.region];
    const cost = plan ? ` · $${((plan.usd_per_month / 730) * (up / 3600)).toFixed(2)} so far` : "";
    sub.textContent = `${n.endpoint} · up ${duration(up)}${cost}`;
    what.append(b, sub);

    const manage = document.createElement("button");
    manage.className = "btn quiet";
    manage.textContent = "Manage";
    manage.addEventListener("click", async () => { node = n; await enterReady(); });

    const kill = document.createElement("button");
    kill.className = "btn danger";
    kill.textContent = "Destroy";
    kill.addEventListener("click", async () => {
      kill.disabled = true;
      kill.textContent = "Destroying…";
      try {
        await invoke("destroy_node", { region: n.region });
        if (node?.region === n.region) node = null;
      } catch (err) {
        $("region-msg").textContent = String(err);
        $("region-msg").className = "msg err";
      }
      await paintRunning();
      markRunningRegions();
    });

    row.append(pip, what, manage, kill);
    wrap.append(row);
  }
  markRunningRegions();
}

/// Badge the picker so an occupied region cannot be walked into by accident.
function markRunningRegions() {
  const live = new Set(running.map((n) => n.region));
  for (const btn of document.querySelectorAll(".region")) {
    const id = btn.dataset.region;
    let badge = btn.querySelector(".badge");
    if (live.has(id)) {
      if (!badge) {
        badge = document.createElement("span");
        badge.className = "badge";
        btn.append(badge);
      }
      badge.textContent = "● Running";
    } else if (badge) {
      badge.remove();
    }
  }
  const sel = chosen && live.has(chosen);
  $("go-connect").textContent = sel ? "Manage this server" : "Next";
}

function pick(id) {
  chosen = id;
  for (const b of document.querySelectorAll(".region")) {
    b.setAttribute("aria-pressed", String(b.dataset.region === id));
  }
  $("go-connect").disabled = false;
  markRunningRegions();
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
  // Occupied region: go straight to it instead of attempting a create that AWS will refuse.
  const existing = running.find((n) => n.region === chosen);
  if (existing) { node = existing; await enterReady(); return; }
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

function bytes(n) {
  if (!n) return "0 MB";
  const mb = n / 1048576;
  if (mb < 1024) return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
  return `${(mb / 1024).toFixed(2)} GB`;
}

function duration(secs) {
  if (secs < 60) return `${Math.round(secs)}s`;
  if (secs < 3600) return `${Math.round(secs / 60)} min`;
  const h = Math.floor(secs / 3600), m = Math.round((secs % 3600) / 60);
  return m ? `${h}h ${m}m` : `${h}h`;
}

let usageTimer = 0;

async function paintUsage() {
  let u;
  try { u = await invoke("usage"); } catch { return; }
  show($("usage-panel"), true);

  const live = u.live && !u.live.ended_at ? u.live : null;
  $("u-live").textContent = live ? bytes(live.rx_bytes + live.tx_bytes) : "—";
  $("u-live-time").textContent = live
    ? `${duration(Date.now() / 1000 - live.started_at)} connected`
    : "not connected";

  $("u-month").textContent = bytes(u.month.bytes);
  $("u-month-n").textContent =
    `${u.month.sessions} session${u.month.sessions === 1 ? "" : "s"} · ${duration(u.month.seconds)}`;

  // The allowance belongs to the instance, and AWS meters it at the server. This counts only
  // what crossed this device's tunnel, so it is a floor, not a reconciliation -- say so rather
  // than implying it matches the bill.
  const plan = node ? plans[node.region] : null;
  if (plan) {
    const usedGb = u.month.bytes / 1073741824;
    const left = Math.max(0, plan.transfer_gb - usedGb);
    $("u-left").textContent = `${left > 100 ? Math.round(left) : left.toFixed(1)} GB`;
    $("u-left-n").textContent = `of ${plan.transfer_gb} GB · estimate`;
  } else {
    $("u-left").textContent = "—";
    $("u-left-n").textContent = "";
  }

  const tbl = $("session-table");
  tbl.replaceChildren();
  for (const s of u.recent) {
    const tr = document.createElement("tr");
    const when = new Date(s.started_at * 1000);
    const secs = (s.ended_at ?? Date.now() / 1000) - s.started_at;
    for (const text of [
      PLACES[s.region]?.name ?? s.region,
      when.toLocaleDateString(undefined, { day: "numeric", month: "short" }) +
        " " + when.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" }),
      duration(secs),
      bytes(s.rx_bytes + s.tx_bytes),
    ]) {
      const td = document.createElement("td");
      td.textContent = text;
      tr.append(td);
    }
    tbl.append(tr);
  }
  if (!u.recent.length) {
    const tr = document.createElement("tr");
    const td = document.createElement("td");
    td.textContent = "No sessions yet.";
    tr.append(td);
    tbl.append(tr);
  }
}

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
  paintUsage();
  clearInterval(costTimer);
  clearInterval(usageTimer);
  costTimer = setInterval(paintCost, 10000);
  // Faster than cost because bytes move while you watch; still cheap (two file reads + netstat).
  usageTimer = setInterval(paintUsage, 4000);
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
    paintUsage();
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

await refreshProfiles();

if (activeProfile) {
  // Already set up. One sweep finds anything already running, so reopening lands where you left
  // off instead of back at step one — and more importantly, never hides a server that is billing.
  await enterRegions();
  if (running.length === 1) {
    node = running[0];
    await enterReady();
  }
}
