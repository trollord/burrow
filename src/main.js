// withGlobalTauri is on and this template has no bundler, so the API comes from the injected
// global rather than npm imports — a bare `import "@tauri-apps/api/core"` would 404 in the webview.
const { invoke } = window.__TAURI__.core;
const { openUrl } = window.__TAURI__.opener;

const $ = (id) => document.getElementById(id);
const show = (el, on) => { el.hidden = !on; };

let wireguardInstalled = false;

function log(text, kind = "") {
  const el = $("log");
  el.textContent = text;
  el.className = `log ${kind}`;
}

// ── onboarding ────────────────────────────────────────────────────────────────

// Links open in the real browser, not inside the app window: people need to sign in to AWS, and
// doing that in an embedded webview is both worse and more alarming than using the browser they
// already trust.
document.querySelectorAll("[data-open]").forEach((b) =>
  b.addEventListener("click", () => openUrl(b.dataset.open))
);

$("creds-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const btn = $("save-creds");
  const msg = $("creds-msg");
  btn.disabled = true;
  msg.textContent = "Checking with AWS…";
  msg.className = "msg";
  try {
    const identity = await invoke("save_credentials", {
      accessKeyId: $("akid").value,
      secret: $("secret").value,
    });
    msg.textContent = "Saved.";
    msg.className = "msg ok";
    // Don't leave the secret sitting in a DOM node once it's in the keychain.
    $("secret").value = "";
    await enterMain(identity);
  } catch (err) {
    msg.textContent = String(err);
    msg.className = "msg err";
  } finally {
    btn.disabled = false;
  }
});

// ── main panel ────────────────────────────────────────────────────────────────

async function enterMain(identity) {
  show($("onboarding"), false);
  show($("main-panel"), true);
  $("acct").textContent = identity
    ? `${identity.account} · ${identity.arn.split("/").pop()}`
    : "unknown";

  const select = $("region");
  if (!select.options.length) {
    // Grouped by continent: a flat list of AWS region codes is meaningless to the person this
    // app is for, who is choosing "somewhere in Europe", not eu-central-1.
    const groups = new Map();
    for (const r of await invoke("regions")) {
      if (!groups.has(r.group)) {
        const g = document.createElement("optgroup");
        g.label = r.group;
        groups.set(r.group, g);
        select.appendChild(g);
      }
      const o = document.createElement("option");
      o.value = r.id;
      o.textContent = r.label;
      groups.get(r.group).appendChild(o);
    }
  }
  await refresh();
}

function setState(text, tone) {
  $("state-text").textContent = text;
  $("dot").className = `dot ${tone}`;
}

async function showPlan(region) {
  const el = $("plan");
  el.textContent = "Checking price…";
  try {
    const p = await invoke("region_plan", { region });
    // Transfer allowance is the number that actually varies and the one that can surprise people:
    // Sydney is the same $5 as Virginia for half the data.
    el.textContent =
      `$${p.usd_per_month.toFixed(2)} per month · ${p.transfer_gb} GB of traffic included · ${p.bundle_id}`;
  } catch (err) {
    el.textContent = String(err);
  }
}

async function refresh() {
  const region = $("region").value;
  showPlan(region);
  setState("Checking…", "");
  let node = null;
  try {
    node = await invoke("node_status", { region });
  } catch (err) {
    setState(String(err), "bad");
    return;
  }

  if (node) {
    setState(`Server running at ${node.endpoint}`, "good");
    show($("create"), false);
    show($("destroy"), true);
    show($("connect-card"), true);
    $("connect-cmd").textContent = await invoke("connect_command", { region });
    $("disconnect-cmd").textContent = await invoke("disconnect_command", { region });
    show($("wg-warn"), !wireguardInstalled);
  } else {
    setState("No server in this location", "idle");
    show($("create"), true);
    show($("destroy"), false);
    show($("connect-card"), false);
  }
}

$("region").addEventListener("change", refresh);

$("create").addEventListener("click", async () => {
  const region = $("region").value;
  $("create").disabled = true;
  // Provisioning takes a couple of minutes, almost all of it apt-get on first boot. Saying so up
  // front stops people assuming it has hung and clicking again.
  setState("Creating… this takes about two minutes", "busy");
  log("Allocating an address, starting the server, installing WireGuard…");
  try {
    const node = await invoke("create_node", { region });
    log(`Ready at ${node.endpoint}.`, "ok");
  } catch (err) {
    log(String(err), "err");
  } finally {
    $("create").disabled = false;
    await refresh();
  }
});

$("destroy").addEventListener("click", async () => {
  const region = $("region").value;
  $("destroy").disabled = true;
  setState("Destroying…", "busy");
  try {
    const steps = await invoke("destroy_node", { region });
    log(steps.join(" · "), "ok");
  } catch (err) {
    log(String(err), "err");
  } finally {
    $("destroy").disabled = false;
    await refresh();
  }
});

$("forget").addEventListener("click", async () => {
  await invoke("forget_credentials");
  show($("main-panel"), false);
  show($("onboarding"), true);
});

for (const [btn, src] of [["copy-connect", "connect-cmd"], ["copy-disconnect", "disconnect-cmd"]]) {
  $(btn).addEventListener("click", async () => {
    await navigator.clipboard.writeText($(src).textContent);
    $(btn).textContent = "Copied";
    setTimeout(() => ($(btn).textContent = "Copy"), 1200);
  });
}

// ── startup ───────────────────────────────────────────────────────────────────

const state = await invoke("setup_state");
wireguardInstalled = state.wireguard_installed;
// Credentials present but unverifiable means revoked or offline. Send them back to onboarding
// rather than into a panel where every button will fail with the same opaque error.
if (state.has_credentials && state.identity) {
  await enterMain(state.identity);
} else {
  show($("onboarding"), true);
}
