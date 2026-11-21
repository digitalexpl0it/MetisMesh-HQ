const RELEASES_API =
  "https://api.github.com/repos/digitalexpl0it/MetisMesh-HQ/releases/latest";
const RELEASES_PAGE =
  "https://github.com/digitalexpl0it/MetisMesh-HQ/releases/latest";

const SLIDES = [
  {
    type: "image",
    src: "media/overview.png",
    caption: "Desk overview",
    blurb: "Companion status, roster pressure, and mesh traffic at a glance.",
  },
  {
    type: "image",
    src: "media/messages.png",
    caption: "Channels & messages",
    blurb: "Public and private channels with session speakers and local history.",
  },
  {
    type: "image",
    src: "media/chat.png",
    caption: "Heard on the mesh",
    blurb: "Channel TX shows Heard repeater pills when flood echoes resolve.",
  },
  {
    type: "image",
    src: "media/roster.png",
    caption: "Roster",
    blurb: "Everyone heard on the mesh — enroll who you trust.",
  },
  {
    type: "image",
    src: "media/map.png",
    caption: "Operational map",
    blurb: "Live pins, Trace paths, and passive Traffic edges — tiles stay local.",
  },
  {
    type: "video",
    src: "media/metis-1795234518.mp4",
    caption: "Live Stream",
    blurb: "Hop-by-hop path FX from a session buffer — collect while chatting, animate on the map.",
  },
  {
    type: "image",
    src: "media/claimdevice.png",
    caption: "Infrastructure",
    blurb: "Claim and admin-verify repeaters, rooms, and sensors over LoRa.",
  },
  {
    type: "image",
    src: "media/cli.png",
    caption: "Operator CLI",
    blurb: "Companion and remote device console when you need the raw tools.",
  },
  {
    type: "image",
    src: "media/firmware_locker.png",
    caption: "Firmware locker",
    blurb: "Sync while online; flash over USB when the internet is gone.",
  },
  {
    type: "image",
    src: "media/guides.png",
    caption: "Offline guides",
    blurb: "Survival and reference packs on disk — marketplace installs once, reads forever.",
  },
  {
    type: "image",
    src: "media/broadcast.png",
    caption: "Broadcast",
    blurb: "Fan out a short community message when everyone needs the same word.",
  },
];

const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function buildShowcase() {
  const stack = document.querySelector("[data-showcase-stack]");
  const dots = document.querySelector("[data-showcase-dots]");
  const caption = document.querySelector("[data-showcase-caption]");
  const blurb = document.querySelector("[data-showcase-blurb]");
  const spacer = document.querySelector("[data-showcase-spacer]");
  if (!stack || !caption || !blurb) return;

  SLIDES.forEach((slide, i) => {
    const item = document.createElement("div");
    item.className = "shot-stack__item";
    item.dataset.index = String(i);

    const shell = document.createElement("div");
    shell.className = "shot-shell";

    const frame = document.createElement("div");
    frame.className = "shot-frame";

    let media;
    if (slide.type === "video") {
      media = document.createElement("video");
      media.src = slide.src;
      media.muted = true;
      media.loop = true;
      media.playsInline = true;
      media.setAttribute("playsinline", "");
      media.preload = "metadata";
      media.setAttribute("aria-label", slide.caption);
    } else {
      media = document.createElement("img");
      media.src = slide.src;
      media.alt = slide.caption;
      media.loading = i === 0 ? "eager" : "lazy";
    }
    frame.appendChild(media);

    const mirror = document.createElement("div");
    mirror.className = "shot-mirror";
    const mirrorClone =
      slide.type === "video"
        ? media.cloneNode(true)
        : (() => {
            const img = document.createElement("img");
            img.src = slide.src;
            img.alt = "";
            img.loading = "lazy";
            return img;
          })();
    if (mirrorClone.tagName === "VIDEO") {
      mirrorClone.muted = true;
      mirrorClone.loop = true;
      mirrorClone.playsInline = true;
    }
    mirror.appendChild(mirrorClone);

    shell.appendChild(frame);
    shell.appendChild(mirror);
    item.appendChild(shell);
    stack.appendChild(item);

    if (dots) {
      const dot = document.createElement("span");
      dot.dataset.index = String(i);
      dots.appendChild(dot);
    }
  });

  if (reduceMotion) {
    caption.textContent = "Product tour";
    blurb.textContent =
      "Scroll the page normally — each screen is shown in order below.";
    stack.querySelectorAll("video").forEach((v) => {
      v.removeAttribute("autoplay");
    });
    return;
  }

  // Tall scrub track: ~0.85 viewport height per slide.
  if (spacer) {
    spacer.style.height = `${Math.max(SLIDES.length, 4) * 85}vh`;
  }

  let active = -1;
  const items = [...stack.querySelectorAll(".shot-stack__item")];
  const dotEls = dots ? [...dots.querySelectorAll("span")] : [];

  function setActive(index, localT) {
    const i = Math.max(0, Math.min(SLIDES.length - 1, index));
    const slide = SLIDES[i];
    caption.textContent = slide.caption;
    blurb.textContent = slide.blurb;

    // Hold the active shot fully clear for most of the segment, then
    // crossfade only near the end (avoids a permanent washed-out look).
    const fadeStart = 0.78;
    const t =
      localT <= fadeStart ? 0 : (localT - fadeStart) / (1 - fadeStart);

    items.forEach((el, j) => {
      const dist = j - i;
      let opacity = 0;
      let z = 0;
      if (j === i) {
        opacity = 1 - t;
        z = 2;
      } else if (j === i + 1) {
        opacity = t;
        z = 3;
      }

      // Mild tilt on the frame only — keeps the floor reflection clear.
      const tiltY = j === i ? -5 + (t - 0.5) * 1.5 : dist * 3 - 2;
      const tiltX = j === i ? 1.5 : 2.5;
      const lift = j === i ? 0 : dist * 8;
      const frame = el.querySelector(".shot-frame");

      el.style.opacity = String(opacity);
      el.style.zIndex = String(z);
      el.style.transform = `translate3d(0, ${lift}px, 0)`;
      if (frame) {
        frame.style.transformOrigin = "40% 100%";
        frame.style.transform = `perspective(1400px) rotateX(${tiltX}deg) rotateY(${tiltY}deg)`;
      }
      const mirror = el.querySelector(".shot-mirror");
      if (mirror) {
        mirror.style.transformOrigin = "40% 0%";
        mirror.style.transform = `perspective(1400px) rotateY(${tiltY}deg)`;
      }
      el.classList.toggle("is-active", j === i);
    });

    dotEls.forEach((d, j) => d.classList.toggle("is-active", j === i));

    items.forEach((el, j) => {
      const video = el.querySelector("video");
      if (!video) return;
      if (j === i) {
        const play = video.play();
        if (play && typeof play.catch === "function") play.catch(() => {});
        const mirrorVid = el.querySelector(".shot-mirror video");
        if (mirrorVid) {
          const mp = mirrorVid.play();
          if (mp && typeof mp.catch === "function") mp.catch(() => {});
        }
      } else {
        video.pause();
        const mirrorVid = el.querySelector(".shot-mirror video");
        if (mirrorVid) mirrorVid.pause();
      }
    });

    active = i;
  }

  function onScroll() {
    const section = document.querySelector(".showcase");
    const sticky = document.querySelector(".showcase__sticky");
    if (!section) return;
    const rect = section.getBoundingClientRect();
    const stickyH = sticky
      ? sticky.getBoundingClientRect().height
      : window.innerHeight * 0.85;
    const total = section.offsetHeight - stickyH;
    if (total <= 0) {
      setActive(0, 0);
      return;
    }
    const scrolled = Math.min(Math.max(-rect.top, 0), total);
    const progress = scrolled / total;
    const scaled = progress * (SLIDES.length - 0.0001);
    const index = Math.floor(scaled);
    const localT = scaled - index;
    setActive(index, localT);
  }

  let ticking = false;
  window.addEventListener(
    "scroll",
    () => {
      if (ticking) return;
      ticking = true;
      requestAnimationFrame(() => {
        onScroll();
        ticking = false;
      });
    },
    { passive: true }
  );
  window.addEventListener(
    "resize",
    () => {
      syncTopbarHeight();
      onScroll();
    },
    { passive: true }
  );
  syncTopbarHeight();
  onScroll();
}

function syncTopbarHeight() {
  const topbar = document.querySelector(".topbar");
  if (!topbar) return;
  const h = Math.ceil(topbar.getBoundingClientRect().height);
  if (h > 0) {
    document.documentElement.style.setProperty("--topbar-h", `${h}px`);
  }
}

function pickAsset(assets, kind) {
  const names = assets.map((a) => ({
    name: a.name || "",
    url: a.browser_download_url,
  }));
  const find = (pred) => names.find((a) => pred(a.name.toLowerCase()));

  if (kind === "windows") {
    return find((n) => n.includes("windows") && n.endsWith(".zip"));
  }
  if (kind === "linux") {
    return (
      find((n) => n.endsWith(".appimage")) ||
      find((n) => n.includes("appimage"))
    );
  }
  if (kind === "deb") {
    return find((n) => n.endsWith(".deb"));
  }
  if (kind === "tarball") {
    return find(
      (n) => n.endsWith(".tar.gz") && n.includes("linux") && !n.includes("source")
    );
  }
  return null;
}

async function wireDownloads() {
  const links = document.querySelectorAll("[data-download]");
  let assets = [];
  try {
    const res = await fetch(RELEASES_API, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (res.ok) {
      const data = await res.json();
      assets = Array.isArray(data.assets) ? data.assets : [];
    }
  } catch (_) {
    /* fall back to releases page */
  }

  links.forEach((el) => {
    const kind = el.getAttribute("data-download");
    const hit = pickAsset(assets, kind);
    if (hit && hit.url) {
      el.href = hit.url;
      el.setAttribute("download", "");
    } else {
      el.href = RELEASES_PAGE;
      el.removeAttribute("download");
    }
  });
}

const HARDWARE_CATEGORY_ORDER = ["Nodes", "Antennas", "Cables", "Repeaters"];

function hardwareCard(item) {
  const url = item.url && item.url !== "#" ? item.url : null;
  const card = document.createElement(url ? "a" : "div");
  card.className = "hardware-card" + (url ? "" : " is-placeholder");
  if (url) {
    card.href = url;
    card.target = "_blank";
    card.rel = "noopener sponsored nofollow";
  }
  const img = escapeHtml(item.image || "media/app_icon_512.png");
  card.innerHTML = `
      <div class="hardware-card__thumb">
        <img src="${img}" alt="${escapeHtml(item.title || "Product")}" width="160" height="160" loading="lazy" />
      </div>
      <h3>${escapeHtml(item.title || "Product")}</h3>
      <p>${escapeHtml(item.blurb || "")}</p>
      <span class="buy">${url ? "Buy on Amazon →" : "Link coming soon"}</span>
    `;
  return card;
}

async function wireHardware() {
  const grid = document.querySelector("[data-hardware-grid]");
  if (!grid) return;
  let items = [];
  try {
    const res = await fetch("hardware.json", { cache: "no-cache" });
    if (res.ok) items = await res.json();
  } catch (_) {
    items = [];
  }
  if (!Array.isArray(items) || items.length === 0) {
    grid.innerHTML =
      "<p class=\"affiliate-note\">Add products in <code>hardware.json</code>.</p>";
    return;
  }

  const byCat = new Map();
  items.forEach((item) => {
    const cat = (item.category || "Other").trim() || "Other";
    if (!byCat.has(cat)) byCat.set(cat, []);
    byCat.get(cat).push(item);
  });

  const order = [
    ...HARDWARE_CATEGORY_ORDER.filter((c) => byCat.has(c)),
    ...[...byCat.keys()].filter((c) => !HARDWARE_CATEGORY_ORDER.includes(c)),
  ];

  grid.innerHTML = "";
  order.forEach((cat) => {
    const section = document.createElement("section");
    section.className = "hardware-cat";
    const heading = document.createElement("h3");
    heading.className = "hardware-cat__title";
    heading.textContent = cat;
    section.appendChild(heading);
    const row = document.createElement("div");
    row.className = "hardware-cat__grid";
    byCat.get(cat).forEach((item) => row.appendChild(hardwareCard(item)));
    section.appendChild(row);
    grid.appendChild(section);
  });
}

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

syncTopbarHeight();
buildShowcase();
wireDownloads();
wireHardware();
