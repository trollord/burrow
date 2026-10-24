// An orthographic globe with real coastlines and country borders, drawn on canvas.
//
// Canvas rather than SVG because the whole world is ~10,000 points and redrawing that many DOM
// nodes at 60fps is not viable; filling paths on a canvas is.
//
// The projection is arranged so that no trigonometry runs per point per frame. Latitude and
// longitude never change, so cos(lat)·sin(lon), cos(lat)·cos(lon) and sin(lat) are precomputed
// once into flat arrays. Rotating is then the angle-addition identity:
//
//     sin(lon − spin) = sin(lon)cos(spin) − cos(lon)sin(spin)
//     cos(lon − spin) = cos(lon)cos(spin) + sin(lon)sin(spin)
//
// which is four multiplies and two adds per point, with sin/cos of the spin computed once a frame.

import { RINGS } from "./world.js";

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

// Home comes from the system timezone, not an IP lookup. A VPN app calling a geolocation service
// on launch would undercut its own pitch, and a timezone is ample for picking a point on a globe.
const ZONES = [
  [/Asia\/(Kolkata|Calcutta)/,         19.1,  72.9, "India"],
  [/Asia\/(Dubai|Muscat)/,             25.2,  55.3, "the UAE"],
  [/Asia\/(Singapore|Kuala)/,           1.35, 103.8, "Singapore"],
  [/Asia\/(Tokyo|Seoul)/,              35.7, 139.7, "Japan"],
  [/Asia\/(Shanghai|Hong_Kong)/,       31.2, 121.5, "China"],
  [/Europe\/(London|Dublin)/,          51.5,  -0.1, "the UK"],
  [/Europe\//,                         50.1,   8.7, "Europe"],
  [/Africa\//,                          6.5,   3.4, "Africa"],
  [/America\/(Los_Angeles|Vancouver)/, 34.0, -118.2, "the US west coast"],
  [/America\/(New_York|Toronto)/,      40.7, -74.0, "the US east coast"],
  [/America\/(Sao_Paulo|Argentina)/,  -23.5, -46.6, "Brazil"],
  [/America\//,                        39.8, -98.6, "North America"],
  [/Australia\//,                     -33.9, 151.2, "Australia"],
  [/Pacific\/Auckland/,               -36.8, 174.8, "New Zealand"],
];

export function home() {
  let tz = "";
  try { tz = Intl.DateTimeFormat().resolvedOptions().timeZone || ""; } catch { /* ignore */ }
  for (const [re, lat, lon, label] of ZONES) if (re.test(tz)) return { lat, lon, label, tz };
  return { lat: 20, lon: 0, label: "here", tz };
}

/** Pack every ring into flat typed arrays of projection-ready terms. Done once at module load. */
function prepare(rings) {
  return rings.map((flat) => {
    const n = flat.length / 2;
    const a = new Float32Array(n);   // cos(lat)·sin(lon)
    const b = new Float32Array(n);   // cos(lat)·cos(lon)
    const s = new Float32Array(n);   // sin(lat)
    for (let i = 0; i < n; i++) {
      const lon = flat[i * 2] * DEG, lat = flat[i * 2 + 1] * DEG;
      const cl = Math.cos(lat);
      a[i] = cl * Math.sin(lon);
      b[i] = cl * Math.cos(lon);
      s[i] = Math.sin(lat);
    }
    return { a, b, s, n };
  });
}

const LAND = prepare(RINGS);

const css = (name, fallback) =>
  getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;

/**
 * Mount a globe onto a <canvas>.
 *
 * @param canvas         target
 * @param opts.targets   region ids to mark
 * @param opts.focus     region id to send the light to; null cycles through them
 * @param opts.spin      degrees per frame (0 to hold still)
 */
export function mount(canvas, opts = {}) {
  let { targets = Object.keys(PLACES), focus = null, spin = 0.06 } = opts;
  const ctx = canvas.getContext("2d");
  const me = home();
  const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  let lon0 = 20;        // rotate so the Atlantic faces us at rest
  let raf = 0;
  let t0 = performance.now();
  let R = 0, cx = 0, cy = 0;

  function resize() {
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth, h = canvas.clientHeight;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    R = Math.min(w, h) / 2 - 2;
    cx = w / 2;
    cy = h / 2;
  }
  resize();
  new ResizeObserver(resize).observe(canvas);

  const project = (lat, lon, sinS, cosS) => {
    const p = lat * DEG, l = lon * DEG, cl = Math.cos(p);
    const a = cl * Math.sin(l), b = cl * Math.cos(l);
    return { x: cx + R * (a * cosS - b * sinS), y: cy - R * Math.sin(p), z: b * cosS + a * sinS };
  };

  function drawLand(sinS, cosS) {
    ctx.beginPath();
    for (const { a, b, s, n } of LAND) {
      for (let i = 0; i < n; i++) {
        let x = a[i] * cosS - b[i] * sinS;
        let y = -s[i];
        const z = b[i] * cosS + a[i] * sinS;
        if (z <= 0) {
          // Behind the horizon: push the point out to the limb instead of dropping it. Dropping
          // breaks the ring and leaves torn fills on anything spanning the edge — Russia and
          // Antarctica especially. Projecting onto the rim keeps every ring closed, and the
          // circular clip below hides the difference.
          const m = Math.hypot(x, y) || 1;
          x /= m; y /= m;
        }
        const px = cx + R * x, py = cy + R * y;
        if (i === 0) ctx.moveTo(px, py); else ctx.lineTo(px, py);
      }
      ctx.closePath();
    }
    ctx.fillStyle = css("--globe-land", "#d9e8ff");
    ctx.fill("evenodd");
    ctx.strokeStyle = css("--globe-border", "#9cc0f0");
    ctx.lineWidth = 0.6;
    ctx.stroke();
  }

  function drawGraticule(sinS, cosS) {
    ctx.strokeStyle = css("--globe-grid", "#e6effc");
    ctx.lineWidth = 0.6;
    ctx.beginPath();
    for (let lat = -60; lat <= 60; lat += 30) {
      let started = false;
      for (let lon = -180; lon <= 180; lon += 4) {
        const p = project(lat, lon, sinS, cosS);
        if (p.z <= 0) { started = false; continue; }
        if (!started) { ctx.moveTo(p.x, p.y); started = true; } else ctx.lineTo(p.x, p.y);
      }
    }
    for (let lon = -180; lon < 180; lon += 30) {
      let started = false;
      for (let lat = -90; lat <= 90; lat += 4) {
        const p = project(lat, lon, sinS, cosS);
        if (p.z <= 0) { started = false; continue; }
        if (!started) { ctx.moveTo(p.x, p.y); started = true; } else ctx.lineTo(p.x, p.y);
      }
    }
    ctx.stroke();
  }

  /** Bowed arc between two projected points, lifted away from the centre so it reads as a hop. */
  function arc(from, to) {
    const mx = (from.x + to.x) / 2 - cx, my = (from.y + to.y) / 2 - cy;
    const d = Math.hypot(to.x - from.x, to.y - from.y);
    const m = Math.hypot(mx, my) || 1;
    const lift = 1 + (d / R) * 0.5;
    return [cx + (mx / m) * m * lift, cy + (my / m) * m * lift];
  }

  function frame(now) {
    if (!still && spin) lon0 = (lon0 + spin) % 360;
    const sinS = Math.sin(lon0 * DEG), cosS = Math.cos(lon0 * DEG);

    ctx.clearRect(0, 0, canvas.clientWidth, canvas.clientHeight);
    ctx.save();
    ctx.beginPath();
    ctx.arc(cx, cy, R, 0, Math.PI * 2);
    ctx.fillStyle = css("--globe-ocean", "#ffffff");
    ctx.fill();
    ctx.clip();

    drawGraticule(sinS, cosS);
    drawLand(sinS, cosS);
    ctx.restore();

    ctx.beginPath();
    ctx.arc(cx, cy, R, 0, Math.PI * 2);
    ctx.strokeStyle = css("--globe-limb", "#8dbcff");
    ctx.lineWidth = 1;
    ctx.stroke();

    const list = targets.filter((id) => PLACES[id]);
    const cycle = focus ? [focus] : list;
    const active = cycle.length ? cycle[Math.floor((now - t0) / 2600) % cycle.length] : null;
    const h = project(me.lat, me.lon, sinS, cosS);
    const accent = css("--globe-accent", "#1d5fd8");

    // Arcs first so the nodes sit on top of them.
    if (h.z > 0 && active) {
      const t = project(PLACES[active].lat, PLACES[active].lon, sinS, cosS);
      if (t.z > 0) {
        const [qx, qy] = arc(h, t);
        ctx.beginPath();
        ctx.moveTo(h.x, h.y);
        ctx.quadraticCurveTo(qx, qy, t.x, t.y);
        ctx.strokeStyle = accent;
        ctx.globalAlpha = 0.28;
        ctx.lineWidth = 1;
        ctx.stroke();

        // The travelling light: one short dash chasing the arc, drawn with a dash offset rather
        // than a moving sprite so it hugs the curve exactly.
        const phase = ((now - t0) % 2600) / 2600;
        ctx.globalAlpha = 1;
        ctx.setLineDash([22, 2000]);
        ctx.lineDashOffset = -phase * (d2(h, t) + 220);
        ctx.lineWidth = 1.8;
        ctx.lineCap = "round";
        ctx.stroke();
        ctx.setLineDash([]);
      }
    }

    for (const id of list) {
      const p = project(PLACES[id].lat, PLACES[id].lon, sinS, cosS);
      if (p.z <= 0) continue;
      const on = id === active;
      if (on) {
        const t = ((now - t0) % 2400) / 2400;
        ctx.beginPath();
        ctx.arc(p.x, p.y, 3 + t * 13, 0, Math.PI * 2);
        ctx.strokeStyle = accent;
        ctx.globalAlpha = 0.8 * (1 - t);
        ctx.lineWidth = 1;
        ctx.stroke();
        ctx.globalAlpha = 1;
      }
      ctx.beginPath();
      ctx.arc(p.x, p.y, on ? 3.4 : 2.2, 0, Math.PI * 2);
      ctx.fillStyle = accent;
      ctx.fill();
    }

    if (h.z > 0) {
      ctx.beginPath();
      ctx.arc(h.x, h.y, 3.4, 0, Math.PI * 2);
      ctx.fillStyle = css("--globe-home", "#06142e");
      ctx.fill();
    }

    raf = requestAnimationFrame(frame);
  }

  const d2 = (a, b) => Math.hypot(b.x - a.x, b.y - a.y);

  raf = requestAnimationFrame(frame);

  return {
    setFocus(id) { focus = id; t0 = performance.now(); },
    setTargets(ids) { targets = ids; },
    stop() { cancelAnimationFrame(raf); },
  };
}
