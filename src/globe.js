// A hairline globe: orthographic projection, thin graticule, nodes on the surface, and a light
// that travels along a great-circle-ish arc from where you are to where you're going.
//
// Everything is drawn as SVG and redrawn each frame. That is fine at this scale — a few dozen
// paths — and avoids pulling in a mapping library for what is essentially decoration.

const NS = "http://www.w3.org/2000/svg";
const R = 140;
const DEG = Math.PI / 180;

export const PLACES = {
  "us-east-1":      { lat: 38.0,  lon: -78.5,  name: "Virginia" },
  "us-east-2":      { lat: 40.0,  lon: -83.0,  name: "Ohio" },
  "us-west-2":      { lat: 45.8,  lon: -119.7, name: "Oregon" },
  "eu-central-1":   { lat: 50.1,  lon: 8.7,    name: "Frankfurt" },
  "eu-west-2":      { lat: 51.5,  lon: -0.1,   name: "London" },
  "eu-west-1":      { lat: 53.3,  lon: -6.3,   name: "Ireland" },
  "ap-southeast-2": { lat: -33.9, lon: 151.2,  name: "Sydney" },
};

// Home is derived from the system timezone, not an IP lookup. A VPN app calling a geolocation
// service on launch would be a poor look, and the timezone is more than precise enough to decide
// which bit of the globe to start the light from.
const ZONES = [
  [/Asia\/(Kolkata|Calcutta)/,        20.0,  77.0],
  [/Asia\/(Dubai|Muscat)/,            25.2,  55.3],
  [/Asia\/(Singapore|Kuala)/,          1.35, 103.8],
  [/Asia\/(Tokyo|Seoul)/,             36.0, 138.0],
  [/Asia\/(Shanghai|Hong_Kong)/,      31.2, 121.5],
  [/Europe\/(London|Dublin|Lisbon)/,  51.5,  -0.1],
  [/Europe\//,                        50.1,   8.7],
  [/Africa\//,                         6.5,   3.4],
  [/America\/(Los_Angeles|Vancouver)/,34.0,-118.2],
  [/America\/(New_York|Toronto)/,     40.7, -74.0],
  [/America\/(Sao_Paulo|Argentina)/, -23.5, -46.6],
  [/America\//,                       39.8, -98.6],
  [/Australia\//,                    -33.9, 151.2],
  [/Pacific\/Auckland/,              -36.8, 174.8],
];

export function home() {
  let tz = "";
  try { tz = Intl.DateTimeFormat().resolvedOptions().timeZone || ""; } catch { /* ignore */ }
  for (const [re, lat, lon] of ZONES) if (re.test(tz)) return { lat, lon, tz };
  return { lat: 20, lon: 0, tz };
}

const el = (n, attrs) => {
  const e = document.createElementNS(NS, n);
  for (const k in attrs) e.setAttribute(k, attrs[k]);
  return e;
};

// Orthographic projection. z > 0 means the point is on the near face of the sphere; anything
// behind is hidden rather than drawn flipped onto the front.
function project(lat, lon, spin) {
  const p = lat * DEG, l = (lon - spin) * DEG;
  return {
    x: R * Math.cos(p) * Math.sin(l),
    y: -R * Math.sin(p),
    z: Math.cos(p) * Math.cos(l),
  };
}

/** Arc that bows away from the sphere, so it reads as a hop rather than a chord. */
function arcPath(a, b) {
  const mx = (a.x + b.x) / 2, my = (a.y + b.y) / 2;
  const d = Math.hypot(b.x - a.x, b.y - a.y);
  const m = Math.hypot(mx, my) || 1;
  const lift = 1 + (d / R) * 0.42;
  return `M ${a.x} ${a.y} Q ${(mx / m) * m * lift} ${(my / m) * m * lift} ${b.x} ${b.y}`;
}

/**
 * Mount a globe into an <svg>.
 *
 * @param svg      target element
 * @param opts.targets  region ids to mark
 * @param opts.focus    region id to send light to; null cycles through all of them
 */
export function mount(svg, opts = {}) {
  let { targets = Object.keys(PLACES), focus = null } = opts;
  let spin = -20;
  let raf = 0;
  let t0 = performance.now();
  const me = home();

  const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  svg.replaceChildren();
  const gGrid = el("g"), gArcs = el("g"), gNodes = el("g");
  svg.append(gGrid, gArcs, gNodes);

  function drawGraticule() {
    gGrid.replaceChildren();
    gGrid.append(el("circle", { class: "limb", cx: 0, cy: 0, r: R }));
    // Latitude rings are ellipses in orthographic; longitude meridians are too, which is why both
    // can be drawn as simple arcs rather than sampled polylines.
    for (let lat = -60; lat <= 60; lat += 30) {
      const ry = R * Math.sin(Math.abs(lat) * DEG);
      const y = -R * Math.sin(lat * DEG);
      const rx = R * Math.cos(lat * DEG);
      gGrid.append(el("ellipse", { class: "graticule", cx: 0, cy: y, rx, ry: ry * 0.001 + rx * 0.22 }));
    }
    for (let k = 0; k < 6; k++) {
      const lon = k * 30 - spin;
      const rx = Math.abs(R * Math.sin(lon * DEG));
      gGrid.append(el("ellipse", { class: "graticule", cx: 0, cy: 0, rx, ry: R }));
    }
  }

  function frame(now) {
    if (!still) spin = (spin + 0.075) % 360;
    drawGraticule();
    gArcs.replaceChildren();
    gNodes.replaceChildren();

    const h = project(me.lat, me.lon, spin);
    const list = targets.filter((id) => PLACES[id]);
    const cycle = focus ? [focus] : list;
    const active = cycle[Math.floor((now - t0) / 2600) % cycle.length];

    for (const id of list) {
      const p = project(PLACES[id].lat, PLACES[id].lon, spin);
      if (p.z <= 0) continue;
      const on = id === active;
      gNodes.append(el("circle", { class: "node", cx: p.x, cy: p.y, r: on ? 3.4 : 2.2 }));
      if (on) gNodes.append(el("circle", { class: "node-ring ping", cx: p.x, cy: p.y, r: 4 }));
    }

    if (h.z > 0) {
      gNodes.append(el("circle", { class: "node home", cx: h.x, cy: h.y, r: 3.2 }));
      const t = project(PLACES[active]?.lat ?? 0, PLACES[active]?.lon ?? 0, spin);
      if (t.z > 0) {
        const d = arcPath(h, t);
        gArcs.append(el("path", { class: "arc", d }));
        // The travelling light is one short dash chasing along the arc. Cheaper and crisper than
        // animating a circle with animateMotion, and it inherits the hairline stroke.
        const spark = el("path", { class: "spark", d });
        const len = 420;
        const phase = ((now - t0) % 2600) / 2600;
        spark.setAttribute("stroke-dasharray", `26 ${len}`);
        spark.setAttribute("stroke-dashoffset", String(len * (1 - phase) * 1.1));
        gArcs.append(spark);
      }
    }
    raf = requestAnimationFrame(frame);
  }

  raf = requestAnimationFrame(frame);

  return {
    setFocus(id) { focus = id; t0 = performance.now(); },
    setTargets(ids) { targets = ids; },
    stop() { cancelAnimationFrame(raf); },
  };
}
