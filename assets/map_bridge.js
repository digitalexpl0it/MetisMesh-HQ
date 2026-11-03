/**
 * Metis Mesh HQ desktop MapLibre host.
 * Loaded after maplibre-gl.js and pmtiles.js. Exposes window.MetisMap.
 */
(function (global) {
  "use strict";

  function waitForMaplibre(timeoutMs) {
    return new Promise(function (resolve, reject) {
      var start = Date.now();
      (function tick() {
        if (global.maplibregl) return resolve(global.maplibregl);
        if (Date.now() - start > timeoutMs) {
          return reject(new Error("maplibregl failed to load"));
        }
        setTimeout(tick, 40);
      })();
    });
  }

  function escapeHtml(value) {
    return String(value == null ? "" : value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  function propsFromFeature(feature) {
    return (feature && feature.properties) || {};
  }

  function tipHtml(props) {
    var name = escapeHtml(props.name || "Node");
    var typeLabel = escapeHtml(props.type_label || "Node");
    return (
      '<div class="metis-map-tip"><strong>' +
      name +
      "</strong><div>" +
      typeLabel +
      "</div></div>"
    );
  }

  function detailHtml(props) {
    var name = escapeHtml(props.name || "Node");
    var typeLabel = escapeHtml(props.type_label || "Node");
    var enrollment = escapeHtml(props.enrollment || "");
    var pubkey = String(props.pubkey || "");
    var pubkeyShort =
      pubkey.length > 16
        ? escapeHtml(pubkey.slice(0, 10) + "…" + pubkey.slice(-6))
        : escapeHtml(pubkey);
    var prefix = String(props.prefix || "");
    var lat = props.lat != null ? Number(props.lat) : null;
    var lon = props.lon != null ? Number(props.lon) : null;
    var coords =
      lat != null && lon != null && isFinite(lat) && isFinite(lon)
        ? lat.toFixed(4) + ", " + lon.toFixed(4)
        : "—";
    var heard = escapeHtml(props.last_heard_label || "unknown");
    var advert = props.last_advert_label
      ? escapeHtml(String(props.last_advert_label))
      : "";
    var lastRadio = props.last_radio
      ? escapeHtml(String(props.last_radio))
      : "";
    var hops =
      props.hops != null && props.hops !== "" && Number(props.hops) >= 0
        ? String(Number(props.hops))
        : "";
    var contactUri = String(props.contact_uri || "");
    var qrUrl = String(props.qr_url || props.qr_data_url || "");
    var wantQr = !!(qrUrl || contactUri);
    var favorited =
      props.favorite === true ||
      props.favorite === "true" ||
      props.favorite === 1 ||
      props.favorite === "1";
    function rfLine() {
      var parts = [];
      if (props.noise != null && props.noise !== "")
        parts.push("noise " + Number(props.noise) + " dBm");
      if (props.rssi != null && props.rssi !== "")
        parts.push("RSSI " + Number(props.rssi) + " dBm");
      if (props.snr != null && props.snr !== "")
        parts.push("SNR " + Number(props.snr).toFixed(1) + " dB");
      return parts.join(" · ");
    }
    var rf = rfLine();
    var bits = ['<div class="metis-map-detail">'];
    bits.push('<div class="metis-map-detail-title">' + name + "</div>");
    bits.push('<div class="metis-map-detail-body">');
    bits.push('<div class="metis-map-detail-cols">');
    bits.push('<div class="metis-map-detail-meta">');
    bits.push(
      '<div class="metis-map-detail-row"><span>Type</span><b>' +
        typeLabel +
        "</b></div>"
    );
    if (enrollment) {
      bits.push(
        '<div class="metis-map-detail-row"><span>Status</span><b>' +
          enrollment +
          "</b></div>"
      );
    }
    bits.push(
      '<div class="metis-map-detail-row"><span>Updated</span><b>' +
        heard +
        "</b></div>"
    );
    if (lastRadio) {
      bits.push(
        '<div class="metis-map-detail-row"><span>Last via</span><b>' +
          lastRadio +
          "</b></div>"
      );
    }
    if (advert && advert !== "unknown") {
      bits.push(
        '<div class="metis-map-detail-row"><span>Last advert</span><b>' +
          advert +
          "</b></div>"
      );
    }
    if (hops) {
      bits.push(
        '<div class="metis-map-detail-row"><span>Hops</span><b>' +
          escapeHtml(hops) +
          "</b></div>"
      );
    }
    if (prefix) {
      bits.push(
        '<div class="metis-map-detail-row"><span>Prefix</span><b class="metis-mono">' +
          escapeHtml(prefix) +
          "</b></div>"
      );
    }
    bits.push(
      '<div class="metis-map-detail-row"><span>Coordinates</span><b class="metis-mono">' +
        escapeHtml(coords) +
        "</b></div>"
    );
    if (rf) {
      bits.push(
        '<div class="metis-map-detail-row metis-map-detail-rf"><span>Last RF</span><b>' +
          escapeHtml(rf) +
          "</b></div>"
      );
    }
    if (pubkey) {
      bits.push(
        '<div class="metis-map-detail-row metis-map-detail-key"><span>Public key</span>' +
          '<button type="button" class="metis-map-link" data-metis-copy="' +
          escapeHtml(pubkey) +
          '">' +
          pubkeyShort +
          "</button></div>"
      );
    }
    bits.push("</div>");
    if (wantQr) {
      bits.push('<div class="metis-map-detail-qr">');
      if (qrUrl) {
        bits.push(
          '<img class="metis-map-detail-qr-img" alt="Contact QR" src="' +
            escapeHtml(qrUrl) +
            '"/>'
        );
      } else {
        // Contact URI known but URL not on the feature yet — placeholder until inject.
        bits.push(
          '<img class="metis-map-detail-qr-img" alt="Contact QR" src="" hidden/>' +
            '<div class="metis-map-detail-qr-pending">QR…</div>'
        );
      }
      bits.push("</div>");
    }
    bits.push("</div></div>");
    bits.push('<div class="metis-map-detail-actions">');
    bits.push(
      '<button type="button" class="metis-map-action" data-metis-fav="' +
        escapeHtml(pubkey) +
        '">' +
        (favorited ? "Unfavorite" : "Favorite") +
        "</button>"
    );
    if (contactUri) {
      bits.push(
        '<button type="button" class="metis-map-action" data-metis-copy="' +
          escapeHtml(contactUri) +
          '">Copy link</button>'
      );
    }
    bits.push("</div></div>");
    return bits.join("");
  }

  var PIN_TYPE_COLORS = {
    hq: "#f472b6",
    chat: "#38bdf8",
    repeater: "#34d399",
    room: "#fbbf24",
    sensor: "#fb923c",
    node: "#94a3b8",
  };

  function strokePath(ctx, d) {
    try {
      ctx.stroke(new Path2D(d));
    } catch (_) {}
  }

  /** Draw the 24x24 type glyph into ctx (origin at icon top-left). */
  function drawTypeIcon(ctx, key) {
    ctx.strokeStyle = "#0b1220";
    ctx.fillStyle = "#0b1220";
    ctx.lineWidth = 2.2;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    if (key === "hq") {
      strokePath(ctx, "M12 3l8 6v11a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V9z");
      strokePath(ctx, "M9 21V12h6v9");
      return;
    }
    if (key === "chat") {
      strokePath(
        ctx,
        "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"
      );
      return;
    }
    if (key === "repeater") {
      strokePath(ctx, "M4.9 19.1a10 10 0 0 1 0-14.2");
      strokePath(ctx, "M7.8 16.2a6 6 0 0 1 0-8.4");
      strokePath(ctx, "M16.2 16.2a6 6 0 0 0 0-8.4");
      strokePath(ctx, "M19.1 19.1a10 10 0 0 0 0-14.2");
      ctx.beginPath();
      ctx.arc(12, 12, 1.4, 0, Math.PI * 2);
      ctx.fill();
      return;
    }
    if (key === "room") {
      strokePath(ctx, "M3 21h18");
      strokePath(ctx, "M5 21V8l7-4 7 4v13");
      strokePath(ctx, "M10 21v-6h4v6");
      return;
    }
    if (key === "sensor") {
      strokePath(ctx, "M22 12h-4l-3 9L9 3l-3 9H2");
      return;
    }
    ctx.beginPath();
    ctx.arc(12, 12, 3, 0, Math.PI * 2);
    ctx.stroke();
    strokePath(ctx, "M12 2v2");
    strokePath(ctx, "M12 20v2");
    strokePath(ctx, "M2 12h2");
    strokePath(ctx, "M20 12h2");
  }

  /**
   * Stretchable name-pill sprite for icon-text-fit labels.
   * Height must exceed 2× radius so stretchY is a real band (empty stretch
   * makes MapLibre reject / ignore the image — pills vanish, text-only left).
   */
  function rasterLabelPill(opts) {
    opts = opts || {};
    var fill = opts.fill || "rgba(11, 18, 32, 0.92)";
    var stroke = opts.stroke || "rgba(244, 244, 245, 0.28)";
    var pr = 2;
    var w = 56 * pr;
    var h = 24 * pr;
    var r = 8 * pr;
    var canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    var ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("no 2d context");
    function roundRect(x, y, rw, rh, rad) {
      var rr = Math.min(rad, rw / 2, rh / 2);
      ctx.beginPath();
      ctx.moveTo(x + rr, y);
      ctx.arcTo(x + rw, y, x + rw, y + rh, rr);
      ctx.arcTo(x + rw, y + rh, x, y + rh, rr);
      ctx.arcTo(x, y + rh, x, y, rr);
      ctx.arcTo(x, y, x + rw, y, rr);
      ctx.closePath();
    }
    roundRect(1, 1, w - 2, h - 2, r);
    ctx.fillStyle = fill;
    ctx.fill();
    ctx.strokeStyle = stroke;
    ctx.lineWidth = 1.5;
    ctx.stroke();
    var padX = 4 * pr;
    var padY = 3 * pr;
    var stretchX0 = r;
    var stretchX1 = w - r;
    var stretchY0 = r;
    var stretchY1 = h - r;
    if (stretchX1 - stretchX0 < 2 || stretchY1 - stretchY0 < 2) {
      throw new Error("pill stretch band too small");
    }
    return {
      image: ctx.getImageData(0, 0, w, h),
      options: {
        pixelRatio: pr,
        stretchX: [[stretchX0, stretchX1]],
        stretchY: [[stretchY0, stretchY1]],
        content: [padX, padY, w - padX, h - padY],
      },
    };
  }

  function addLabelPillImage(map, id, opts, force) {
    if (!map) return false;
    if (map.hasImage(id)) {
      if (!force) return true;
      try {
        map.removeImage(id);
      } catch (_) {}
    }
    try {
      var pill = rasterLabelPill(opts);
      map.addImage(id, pill.image, pill.options);
      return map.hasImage(id);
    } catch (_) {
      return false;
    }
  }

  /**
   * Category-colored highway shields (own image ids — don’t fight the sprite sheet).
   * interstate → blue; highway (US / generic) → white.
   */
  function rasterHighwayShield(kind, chars) {
    var pr = 2;
    var n = Math.max(1, Math.min(5, chars | 0));
    var interstate = kind === "interstate";
    // Interstates stay closer to a tall shield; highways can be a bit wider.
    var w = (interstate ? 14 + n * 5.5 : 16 + n * 8) * pr;
    var h = (interstate ? 22 : 20) * pr;
    var canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    var ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("no 2d context");
    var fill = interstate ? "#1d4ed8" : "#f8fafc";
    var stroke = interstate ? "#93c5fd" : "#334155";
    var r = 4 * pr;
    function roundRect(x, y, rw, rh, rad) {
      var rr = Math.min(rad, rw / 2, rh / 2);
      ctx.beginPath();
      ctx.moveTo(x + rr, y);
      ctx.arcTo(x + rw, y, x + rw, y + rh, rr);
      ctx.arcTo(x + rw, y + rh, x, y + rh, rr);
      ctx.arcTo(x, y + rh, x, y, rr);
      ctx.arcTo(x, y, x + rw, y, rr);
      ctx.closePath();
    }
    if (interstate) {
      // Simplified interstate silhouette: flat top, slight point at bottom.
      var x = 1.5 * pr;
      var y = 1.5 * pr;
      var rw = w - 3 * pr;
      var rh = h - 3 * pr;
      ctx.beginPath();
      ctx.moveTo(x + 2 * pr, y);
      ctx.lineTo(x + rw - 2 * pr, y);
      ctx.quadraticCurveTo(x + rw, y, x + rw, y + 3 * pr);
      ctx.lineTo(x + rw, y + rh - 5 * pr);
      ctx.lineTo(x + rw / 2, y + rh);
      ctx.lineTo(x, y + rh - 5 * pr);
      ctx.lineTo(x, y + 3 * pr);
      ctx.quadraticCurveTo(x, y, x + 2 * pr, y);
      ctx.closePath();
    } else {
      roundRect(1.5 * pr, 1.5 * pr, w - 3 * pr, h - 3 * pr, r);
    }
    ctx.fillStyle = fill;
    ctx.fill();
    ctx.strokeStyle = stroke;
    ctx.lineWidth = 1.5 * pr;
    ctx.stroke();
    // Thin red cap on interstates (MUTCD cue) without eating the number area.
    if (interstate) {
      ctx.save();
      ctx.beginPath();
      ctx.moveTo(2.5 * pr, 2 * pr);
      ctx.lineTo(w - 2.5 * pr, 2 * pr);
      ctx.lineTo(w - 2.5 * pr, 5.5 * pr);
      ctx.lineTo(2.5 * pr, 5.5 * pr);
      ctx.closePath();
      ctx.fillStyle = "#dc2626";
      ctx.fill();
      ctx.restore();
    }
    return ctx.getImageData(0, 0, w, h);
  }

  function addHighwayShieldImages(map, force) {
    if (!map) return false;
    var ok = true;
    var kinds = [
      { prefix: "metis-shield-i-", kind: "interstate" },
      { prefix: "metis-shield-hwy-", kind: "highway" },
    ];
    kinds.forEach(function (spec) {
      for (var n = 1; n <= 5; n++) {
        var id = spec.prefix + n + "char";
        if (map.hasImage(id)) {
          if (!force) continue;
          try {
            map.removeImage(id);
          } catch (_) {}
        }
        try {
          map.addImage(id, rasterHighwayShield(spec.kind, n), { pixelRatio: 2 });
        } catch (_) {
          ok = false;
        }
      }
    });
    return ok;
  }

  /**
   * Rasterize pin sprites with canvas. Desktop WebViews often never complete
   * map.loadImage() for SVG data URLs, which left node layers unregistered.
   */
  function rasterPinImageData(key, favorite) {
    var size = 64;
    var canvas = document.createElement("canvas");
    canvas.width = size;
    canvas.height = size;
    var ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("no 2d context");
    var scale = size / 32;
    ctx.setTransform(scale, 0, 0, scale, 0, 0);
    var color = PIN_TYPE_COLORS[key] || PIN_TYPE_COLORS.node;
    ctx.beginPath();
    ctx.arc(16, 16, 13, 0, Math.PI * 2);
    ctx.fillStyle = color;
    ctx.fill();
    ctx.lineWidth = 2;
    ctx.strokeStyle = "#f4f4f5";
    ctx.stroke();
    if (favorite) {
      ctx.beginPath();
      ctx.arc(16, 16, 15.2, 0, Math.PI * 2);
      ctx.strokeStyle = "#fbbf24";
      ctx.lineWidth = 2.5;
      ctx.stroke();
    }
    ctx.save();
    ctx.translate(4, 4);
    drawTypeIcon(ctx, key);
    ctx.restore();
    return ctx.getImageData(0, 0, size, size);
  }

  function isFavoriteProp(props) {
    return (
      props.favorite === true ||
      props.favorite === "true" ||
      props.favorite === 1 ||
      props.favorite === "1"
    );
  }

  function enrichNodeFeatures(data) {
    var out = data || { type: "FeatureCollection", features: [] };
    var feats = out.features || [];
    for (var i = 0; i < feats.length; i++) {
      var f = feats[i];
      if (!f) continue;
      var p = Object.assign({}, f.properties || {});
      var key = String(p.type_key || "node");
      if (!PIN_TYPE_COLORS[key]) key = "node";
      p.type_key = key;
      p.icon = isFavoriteProp(p) ? "pin-" + key + "-fav" : "pin-" + key;
      f.properties = p;
    }
    return out;
  }

  /** Keep MapLibre DOM events out of Dioxus (avoids element:null parse errors). */
  function sealDomEvent(ev) {
    if (!ev) return;
    try {
      ev.preventDefault();
    } catch (_) {}
    try {
      ev.stopPropagation();
    } catch (_) {}
    try {
      if (typeof ev.stopImmediatePropagation === "function") {
        ev.stopImmediatePropagation();
      }
    } catch (_) {}
  }

  function wireSealedClick(el, handler) {
    if (!el) return;
    ["click", "mousedown", "mouseup", "pointerdown", "pointerup"].forEach(
      function (type) {
        el.addEventListener(
          type,
          function (ev) {
            sealDomEvent(ev);
            if (type === "click" && handler) handler(ev);
          },
          true
        );
      }
    );
  }

  function isolateMapContainerFromDioxus(container) {
    if (!container || container.__metisDioxusSeal) return;
    container.__metisDioxusSeal = true;
    ["click", "mousedown", "mouseup", "pointerdown", "pointerup", "dblclick", "wheel"].forEach(
      function (type) {
        container.addEventListener(
          type,
          function (ev) {
            // Bubble-phase stop so MapLibre target handlers still run, but the
            // event never reaches the Dioxus root (null element id → console error).
            // Wheel: keep trackpad zoom on the map; do not let the page chrome eat it.
            try {
              ev.stopPropagation();
            } catch (_) {}
          },
          false
        );
      }
    );
  }

  function hydrateDetailQr(root) {
    if (!root) return;
    var img = root.querySelector(".metis-map-detail-qr-img");
    if (!img) return;
    var src = img.getAttribute("src") || "";
    if (src.indexOf("metis://") !== 0) return;
    fetch(src)
      .then(function (res) {
        if (!res.ok) throw new Error("qr " + res.status);
        return res.text();
      })
      .then(function (svg) {
        img.removeAttribute("hidden");
        img.hidden = false;
        img.src =
          "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
        var pending = root.querySelector(".metis-map-detail-qr-pending");
        if (pending) pending.remove();
      })
      .catch(function () {});
  }

  function wireDetailActions(root) {
    if (!root) return;
    hydrateDetailQr(root);
    root.querySelectorAll("[data-metis-copy]").forEach(function (btn) {
      wireSealedClick(btn, function () {
        var text = btn.getAttribute("data-metis-copy") || "";
        if (navigator.clipboard && navigator.clipboard.writeText) {
          navigator.clipboard.writeText(text).catch(function () {});
        }
        btn.classList.add("copied");
        setTimeout(function () {
          btn.classList.remove("copied");
        }, 900);
      });
    });
    root.querySelectorAll("[data-metis-fav]").forEach(function (btn) {
      wireSealedClick(btn, function () {
        var key = btn.getAttribute("data-metis-fav") || "";
        if (key) global.__metisMapFavoriteToggle = key;
      });
    });
  }

  var MetisMap = {
    maps: {},
    labels: {},
    tipPopups: {},
    detailPopups: {},
    detailOpen: {},
    nodeData: {},
    clusterEnabled: {},
    protocolReady: false,
    _pmtilesProtocol: null,
    coverageDrag: {},
    coverageBbox: {},
    /** Pending place/adjust square before confirm: { shape, radiusMiles, lng, lat, mode } */
    regionDraft: {},

    ensureProtocol: function () {
      if (this.protocolReady || !global.maplibregl || !global.pmtiles) return;
      var protocol = new global.pmtiles.Protocol();
      global.maplibregl.addProtocol("pmtiles", protocol.tile);
      this._pmtilesProtocol = protocol;
      this.protocolReady = true;
    },

    /** Drop cached PMTiles readers so a re-download of the same region id loads. */
    clearPmtilesCache: function () {
      var protocol = this._pmtilesProtocol;
      if (!protocol) return;
      try {
        if (protocol.tiles && typeof protocol.tiles.clear === "function") {
          protocol.tiles.clear();
        }
      } catch (_) {}
    },

    /** Remember pack bbox from the style's tile-coverage source (if present). */
    rememberCoverageFromStyle: function (mapId, style) {
      try {
        var src = style && style.sources && style.sources["tile-coverage"];
        var ring =
          src &&
          src.data &&
          src.data.features &&
          src.data.features[0] &&
          src.data.features[0].geometry &&
          src.data.features[0].geometry.coordinates[0];
        if (!ring || ring.length < 4) {
          delete this.coverageBbox[mapId];
          return;
        }
        var lons = ring.map(function (p) {
          return p[0];
        });
        var lats = ring.map(function (p) {
          return p[1];
        });
        this.coverageBbox[mapId] = [
          Math.min.apply(null, lons),
          Math.min.apply(null, lats),
          Math.max.apply(null, lons),
          Math.max.apply(null, lats),
        ];
      } catch (_) {
        delete this.coverageBbox[mapId];
      }
    },

    init: async function (mapId, containerId, style, lng, lat, zoom) {
      var maplibregl = await waitForMaplibre(8000);
      this.ensureProtocol();
      this.clearPmtilesCache();
      this.rememberCoverageFromStyle(mapId, style);
      var existing = this.maps[mapId];
      if (existing) {
        try {
          existing.remove();
        } catch (_) {}
        delete this.maps[mapId];
      }
      this.clearLabels(mapId);
      this.detailOpen[mapId] = false;
      var container = document.getElementById(containerId);
      if (!container) throw new Error("map container missing: " + containerId);
      isolateMapContainerFromDioxus(container);
      var map = new maplibregl.Map({
        container: container,
        style: style,
        center: [lng, lat],
        zoom: zoom,
        attributionControl: true,
        // Explicit: trackpad / wheel zoom must stay on (WebKitGTK can feel inert otherwise).
        interactive: true,
        scrollZoom: true,
        boxZoom: true,
        dragPan: true,
        dragRotate: false,
        keyboard: true,
        doubleClickZoom: true,
        touchZoomRotate: true,
        cooperativeGestures: false,
      });
      this.maps[mapId] = map;
      this.ensureScrollZoom(mapId);
      this.ensureZoomControls(mapId);
      this.tipPopups[mapId] = new maplibregl.Popup({
        closeButton: false,
        closeOnClick: false,
        offset: 12,
        className: "metis-maplibre-popup metis-maplibre-tip",
      });
      this.detailPopups[mapId] = new maplibregl.Popup({
        closeButton: true,
        closeOnClick: false,
        offset: 18,
        maxWidth: "20rem",
        className: "metis-maplibre-popup metis-maplibre-detail",
      });
      var self = this;
      this.detailPopups[mapId].on("close", function () {
        self.detailOpen[mapId] = false;
      });
      // Keep GL buffer matched to the stage (avoids cut-off tiles / black gutters).
      this.watchResize(mapId, container);
      await new Promise(function (resolve) {
        var settled = false;
        function done() {
          if (settled) return;
          settled = true;
          self.forceResize(mapId);
          resolve(true);
        }
        map.once("load", done);
        // Never fail init on tile/source errors — outline/pmtiles may load async.
        map.on("error", function () {});
        setTimeout(done, 4000);
      });
      this.forceResize(mapId);
      setTimeout(function () {
        self.forceResize(mapId);
      }, 250);
      try {
        await this.hydrateOutlineSources(mapId);
      } catch (_) {}
      // Street-name pills are style images — register before overlays arrive.
      try {
        this.ensurePinImages(mapId);
      } catch (_) {}
      try {
        if (this.regionDraft[mapId]) {
          this.renderRegionDraft(mapId);
          this.bindDraftDrag(mapId);
        }
      } catch (_) {}
      function publishCamera() {
        var c = map.getCenter();
        global.__metisMapCamera = JSON.stringify({
          lng: c.lng,
          lat: c.lat,
          zoom: map.getZoom(),
        });
      }
      function setMoving(on) {
        global.__metisMapMoving = !!on;
      }
      setMoving(false);
      map.on("movestart", function () {
        setMoving(true);
      });
      map.on("zoomstart", function () {
        setMoving(true);
      });
      map.on("moveend", function () {
        setMoving(false);
        publishCamera();
      });
      map.on("zoomend", function () {
        setMoving(false);
        publishCamera();
      });
      map.on("click", function (e) {
        if (e.defaultPrevented) return;
        var layers = [
          "node-pins",
          "node-labels",
          "clusters",
          "cluster-count",
        ].filter(function (id) {
          return map.getLayer(id);
        });
        var feats = layers.length
          ? map.queryRenderedFeatures(e.point, { layers: layers })
          : [];
        if (!feats.length && self.detailOpen[mapId]) {
          self.hideDetail(mapId);
        }
      });
      publishCamera();
      return true;
    },

    getCamera: function (mapId) {
      var map = this.maps[mapId];
      if (!map) return global.__metisMapCamera || "";
      var c = map.getCenter();
      return JSON.stringify({
        lng: c.lng,
        lat: c.lat,
        zoom: map.getZoom(),
      });
    },

    takeCamera: function () {
      var raw = global.__metisMapCamera || "";
      return raw;
    },

    /** True only when the map is mounted in the container currently in the page. */
    isLive: function (mapId, containerId) {
      var map = this.maps[mapId];
      if (!map) return false;
      var container = map.getContainer && map.getContainer();
      if (!container || !container.isConnected) return false;
      return !containerId || document.getElementById(containerId) === container;
    },

    /** Page-leave cleanup. Skips a map that a remount already attached to the page. */
    destroyIfDetached: function (mapId) {
      var self = this;
      var run = function () {
        var map = self.maps[mapId];
        if (!map) return;
        var container = map.getContainer && map.getContainer();
        // App exit hides the document while the container is still connected —
        // still tear down so WebKitGTK does not assert in epoxy on GL teardown.
        if (container && container.isConnected && !document.hidden) return;
        self.destroy(mapId);
      };
      if (document.hidden) {
        run();
        return true;
      }
      setTimeout(run, 50);
      return true;
    },

    /** Force-drop every map (window close / pagehide). */
    destroyAll: function () {
      var self = this;
      Object.keys(self.maps || {}).forEach(function (id) {
        try {
          self.destroy(id);
        } catch (_) {}
      });
    },

    /** Drop WebGL before map.remove — cuts WebKitGTK epoxy assert noise on exit. */
    loseWebGl: function (map) {
      if (!map) return;
      try {
        if (typeof map.stop === "function") map.stop();
      } catch (_) {}
      try {
        var canvas = map.getCanvas && map.getCanvas();
        if (!canvas || typeof canvas.getContext !== "function") return;
        ["webgl2", "webgl", "experimental-webgl"].forEach(function (kind) {
          try {
            var gl = canvas.getContext(kind);
            if (!gl) return;
            var ext = gl.getExtension("WEBGL_lose_context");
            if (ext && typeof ext.loseContext === "function") ext.loseContext();
          } catch (_) {}
        });
      } catch (_) {}
    },

    /** Trackpad / mouse wheel zoom — re-enable and keep gestures on the canvas. */
    ensureScrollZoom: function (mapId) {
      var map = this.maps[mapId];
      if (!map) return;
      try {
        map.scrollZoom.enable();
        // Slightly snappier than MapLibre defaults on desktop trackpads.
        if (typeof map.scrollZoom.setWheelZoomRate === "function") {
          map.scrollZoom.setWheelZoomRate(1 / 300);
        }
        if (typeof map.scrollZoom.setZoomRate === "function") {
          map.scrollZoom.setZoomRate(1 / 80);
        }
        map.touchZoomRotate.enable();
        map.doubleClickZoom.enable();
      } catch (_) {}
      var canvas = null;
      try {
        canvas = map.getCanvas();
      } catch (_) {}
      if (canvas && !canvas.__metisWheelSeal) {
        canvas.__metisWheelSeal = true;
        // Bubble-phase only: MapLibre already handled the wheel; stop the page chrome
        // (hq-main scroll / Dioxus) from eating the gesture afterward.
        canvas.addEventListener(
          "wheel",
          function (ev) {
            try {
              ev.stopPropagation();
            } catch (_) {}
          },
          { passive: true }
        );
      }
    },

    removeZoomControls: function (mapId) {
      var el = this._zoomControls && this._zoomControls[mapId];
      if (el && el.parentNode) {
        try {
          el.parentNode.removeChild(el);
        } catch (_) {}
      }
      if (this._zoomControls) delete this._zoomControls[mapId];
    },

    /** HQ-styled +/- in the map stage top-right (Tailwind preflight blanks MapLibre icons). */
    ensureZoomControls: function (mapId) {
      var map = this.maps[mapId];
      if (!map) return;
      var container = map.getContainer();
      var stage = container && container.parentElement;
      if (!stage) return;
      this.removeZoomControls(mapId);
      var wrap = document.createElement("div");
      wrap.className = "map-zoom-controls";
      wrap.setAttribute("role", "group");
      wrap.setAttribute("aria-label", "Map zoom");
      var btnIn = document.createElement("button");
      btnIn.type = "button";
      btnIn.className = "map-zoom-btn";
      btnIn.title = "Zoom in";
      btnIn.setAttribute("aria-label", "Zoom in");
      btnIn.textContent = "+";
      var btnOut = document.createElement("button");
      btnOut.type = "button";
      btnOut.className = "map-zoom-btn";
      btnOut.title = "Zoom out";
      btnOut.setAttribute("aria-label", "Zoom out");
      btnOut.textContent = "−";
      wrap.appendChild(btnIn);
      wrap.appendChild(btnOut);
      stage.appendChild(wrap);
      if (!this._zoomControls) this._zoomControls = {};
      this._zoomControls[mapId] = wrap;
      wireSealedClick(btnIn, function () {
        try {
          map.zoomIn({ duration: 220 });
        } catch (_) {}
      });
      wireSealedClick(btnOut, function () {
        try {
          map.zoomOut({ duration: 220 });
        } catch (_) {}
      });
    },

    destroy: function (mapId) {
      this.teardownCoverageDrag(mapId);
      this.clearRegionDraft(mapId);
      this.clearSonar(mapId);
      this.removeZoomControls(mapId);
      var map = this.maps[mapId];
      if (this._resizeObs && this._resizeObs[mapId]) {
        try {
          this._resizeObs[mapId].disconnect();
        } catch (_) {}
        delete this._resizeObs[mapId];
      }
      if (this._windowResize && this._windowResize[mapId]) {
        try {
          window.removeEventListener("resize", this._windowResize[mapId]);
        } catch (_) {}
        delete this._windowResize[mapId];
      }
      ["tipPopups", "detailPopups"].forEach(function (bag) {
        if (MetisMap[bag][mapId]) {
          try {
            MetisMap[bag][mapId].remove();
          } catch (_) {}
          delete MetisMap[bag][mapId];
        }
      });
      this.clearLabels(mapId);
      delete this.nodeData[mapId];
      delete this.clusterEnabled[mapId];
      delete this.detailOpen[mapId];
      if (this.nodeFp) delete this.nodeFp[mapId];
      if (this.seenPubkeys) delete this.seenPubkeys[mapId];
      if (this.sonarMarkers) delete this.sonarMarkers[mapId];
      if (!map) return;
      this.loseWebGl(map);
      try {
        map.remove();
      } catch (_) {}
      delete this.maps[mapId];
    },

    clearLabels: function (mapId) {
      // Legacy HTML pin markers removed — pins are GL layers now.
      var list = this.labels[mapId] || [];
      list.forEach(function (marker) {
        try {
          marker.remove();
        } catch (_) {}
      });
      this.labels[mapId] = [];
    },

    /** Register pin + label-pill sprites once per style (canvas ImageData). */
    ensurePinImages: function (mapId, done) {
      var map = this.maps[mapId];
      if (!map) {
        if (done) done(false);
        return;
      }
      // Device-name pills only (point symbols). Street line labels use text-halo —
      // icon-text-fit on lines leaves empty white boxes where text can't place.
      var forcePills = !map.__metisPillsBuilt;
      if (addLabelPillImage(map, "label-pill", null, forcePills)) {
        map.__metisPillsBuilt = true;
      }
      // Bump version when shield geometry changes so remounts pick up new art.
      var shieldVer = 2;
      var forceShields =
        !map.__metisShieldsBuilt || map.__metisShieldsVer !== shieldVer;
      if (addHighwayShieldImages(map, forceShields)) {
        map.__metisShieldsBuilt = true;
        map.__metisShieldsVer = shieldVer;
      }
      try {
        if (map.hasImage("street-label-pill")) map.removeImage("street-label-pill");
      } catch (_) {}
      this.stripStreetLabelPills(mapId);
      if (map.__metisPinsReady) {
        if (done) done(true);
        return;
      }
      var keys = Object.keys(PIN_TYPE_COLORS);
      var ok = 0;
      keys.forEach(function (key) {
        [false, true].forEach(function (fav) {
          var id = fav ? "pin-" + key + "-fav" : "pin-" + key;
          if (map.hasImage(id)) {
            ok += 1;
            return;
          }
          try {
            var imgData = rasterPinImageData(key, fav);
            map.addImage(id, imgData, { pixelRatio: 2 });
            ok += 1;
          } catch (_) {}
        });
      });
      map.__metisPinsReady = ok > 0;
      map.__metisPinsLoading = false;
      if (done) done(map.__metisPinsReady);
    },

    /** Drop leftover street icon-text-fit (empty white boxes on line labels). */
    stripStreetLabelPills: function (mapId) {
      var map = this.maps[mapId];
      if (!map || !map.getLayer("road_labels_major")) return;
      try {
        // null clears a layout property in MapLibre.
        map.setLayoutProperty("road_labels_major", "icon-image", null);
      } catch (_) {}
      try {
        map.setLayoutProperty("road_labels_major", "icon-text-fit", "none");
      } catch (_) {}
      try {
        map.setPaintProperty("road_labels_major", "text-color", "#0b1220");
        map.setPaintProperty("road_labels_major", "text-halo-color", "#ffffff");
        map.setPaintProperty("road_labels_major", "text-halo-width", 4.5);
        map.setPaintProperty("road_labels_major", "text-halo-blur", 0.2);
      } catch (_) {}
    },

    showTip: function (mapId, lngLat, props) {
      if (this.detailOpen[mapId]) return;
      var map = this.maps[mapId];
      var popup = this.tipPopups[mapId];
      if (!map || !popup) return;
      popup.setLngLat(lngLat).setHTML(tipHtml(props)).addTo(map);
    },

    hideTip: function (mapId) {
      var popup = this.tipPopups[mapId];
      if (popup) popup.remove();
    },

    showDetail: function (mapId, lngLat, props) {
      var map = this.maps[mapId];
      var popup = this.detailPopups[mapId];
      if (!map || !popup) return;
      this.hideTip(mapId);
      this.detailOpen[mapId] = true;
      popup.setLngLat(lngLat).setHTML(detailHtml(props)).addTo(map);
      // Wire buttons after DOM attach.
      var self = this;
      setTimeout(function () {
        wireDetailActions(popup.getElement());
      }, 0);
      // Always JSON so Rust eval cannot mangle meshcore:// URIs.
      global.__metisMapSelection = JSON.stringify({
        contact_uri: String(props.contact_uri || ""),
        pubkey: String(props.pubkey || ""),
        qr_url: String(props.qr_url || ""),
      });
    },

    hideDetail: function (mapId) {
      var popup = this.detailPopups[mapId];
      if (popup) popup.remove();
      this.detailOpen[mapId] = false;
    },

    setDetailQr: function (mapId, url) {
      var popup = this.detailPopups[mapId];
      if (!popup || !url) return false;
      var root = popup.getElement();
      if (!root) return false;
      var img = root.querySelector(".metis-map-detail-qr-img");
      if (!img) return false;
      function applySrc(src) {
        img.removeAttribute("hidden");
        img.hidden = false;
        img.src = src;
        var pending = root.querySelector(".metis-map-detail-qr-pending");
        if (pending) pending.remove();
      }
      // metis:// often fails as a raw <img src>; fetch then use a data URL.
      if (String(url).indexOf("metis://") === 0) {
        fetch(url)
          .then(function (res) {
            if (!res.ok) throw new Error("qr fetch " + res.status);
            return res.text();
          })
          .then(function (svg) {
            applySrc(
              "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg)
            );
          })
          .catch(function () {
            applySrc(url);
          });
        return true;
      }
      applySrc(url);
      return true;
    },

    NODE_LAYER_IDS: [
      "clusters",
      "cluster-count",
      "node-pins",
      "node-labels",
      "node-dots",
      "cluster-count-hit",
    ],

    ensureNodeLayers: function (mapId, cluster) {
      var map = this.maps[mapId];
      if (!map) return;
      var self = this;
      var wantCluster = !!cluster;
      var source = map.getSource("nodes");
      if (source && this.clusterEnabled[mapId] !== wantCluster) {
        this.NODE_LAYER_IDS.forEach(function (id) {
          if (map.getLayer(id)) map.removeLayer(id);
        });
        map.removeSource("nodes");
        source = null;
        delete map["__metisCluster_" + mapId];
        delete map["__metisClick_" + mapId + "_node-pins"];
        delete map["__metisClick_" + mapId + "_node-labels"];
        delete map["__metisClick_" + mapId + "_node-dots"];
      }
      this.clusterEnabled[mapId] = wantCluster;
      if (!source) {
        map.addSource(
          "nodes",
          wantCluster
            ? {
                type: "geojson",
                data: { type: "FeatureCollection", features: [] },
                cluster: true,
                clusterMaxZoom: 12,
                clusterRadius: 56,
                clusterMinPoints: 5,
              }
            : {
                type: "geojson",
                data: { type: "FeatureCollection", features: [] },
              }
        );
      }
      function addVisibleLayers() {
        var unclustered = wantCluster
          ? ["!", ["has", "point_count"]]
          : ["all"];
        if (wantCluster && !map.getLayer("clusters")) {
          map.addLayer({
            id: "clusters",
            type: "circle",
            source: "nodes",
            filter: ["has", "point_count"],
            paint: {
              "circle-color": [
                "step",
                ["get", "point_count"],
                "#34d399",
                15,
                "#fbbf24",
                40,
                "#fb7185",
              ],
              "circle-radius": [
                "interpolate",
                ["linear"],
                ["zoom"],
                8,
                ["step", ["get", "point_count"], 14, 15, 17, 40, 21],
                12,
                ["step", ["get", "point_count"], 18, 15, 22, 40, 28],
                16,
                ["step", ["get", "point_count"], 26, 15, 32, 40, 40],
              ],
              "circle-stroke-width": 2,
              "circle-stroke-color": "#0b1220",
              "circle-opacity": 0.95,
            },
          });
        }
        if (wantCluster && !map.getLayer("cluster-count")) {
          map.addLayer({
            id: "cluster-count",
            type: "symbol",
            source: "nodes",
            filter: ["has", "point_count"],
            layout: {
              "text-field": ["get", "point_count_abbreviated"],
              "text-size": [
                "interpolate",
                ["linear"],
                ["zoom"],
                8,
                11,
                16,
                15,
              ],
              "text-font": ["Noto Sans Medium"],
              "text-allow-overlap": true,
            },
            paint: {
              "text-color": "#0b1220",
            },
          });
        }
        // Colored circles always paint even if a symbol sprite is missing.
        if (!map.getLayer("node-dots")) {
          map.addLayer({
            id: "node-dots",
            type: "circle",
            source: "nodes",
            filter: unclustered,
            paint: {
              "circle-radius": [
                "interpolate",
                ["linear"],
                ["zoom"],
                8,
                8,
                12,
                12,
                15,
                18,
                18,
                24,
              ],
              "circle-color": [
                "match",
                ["get", "type_key"],
                "hq",
                PIN_TYPE_COLORS.hq,
                "chat",
                PIN_TYPE_COLORS.chat,
                "repeater",
                PIN_TYPE_COLORS.repeater,
                "room",
                PIN_TYPE_COLORS.room,
                "sensor",
                PIN_TYPE_COLORS.sensor,
                PIN_TYPE_COLORS.node,
              ],
              "circle-stroke-width": 2,
              "circle-stroke-color": "#f4f4f5",
              "circle-opacity": 0.95,
            },
          });
        }
        if (!map.getLayer("node-pins")) {
          map.addLayer({
            id: "node-pins",
            type: "symbol",
            source: "nodes",
            filter: unclustered,
            layout: {
              "icon-image": ["coalesce", ["get", "icon"], "pin-node"],
              // Grow with zoom so street-level pins are easy to hit/read.
              "icon-size": [
                "interpolate",
                ["linear"],
                ["zoom"],
                8,
                0.75,
                11,
                1.0,
                14,
                1.4,
                17,
                1.85,
              ],
              "icon-anchor": "center",
              "icon-allow-overlap": true,
              "icon-ignore-placement": true,
            },
            paint: {
              // Tracked movers go stale when share-location ads stop.
              "icon-opacity": [
                "case",
                ["==", ["get", "stale"], true],
                0.42,
                ["==", ["to-string", ["get", "stale"]], "true"],
                0.42,
                1,
              ],
            },
          });
        } else {
          try {
            map.setPaintProperty("node-pins", "icon-opacity", [
              "case",
              ["==", ["get", "stale"], true],
              0.42,
              ["==", ["to-string", ["get", "stale"]], "true"],
              0.42,
              1,
            ]);
          } catch (_) {}
        }
        // Rebuild label layer if it lacks the pill (e.g. created before sprites).
        if (map.getLayer("node-labels")) {
          var labelIcon = null;
          try {
            labelIcon = map.getLayoutProperty("node-labels", "icon-image");
          } catch (_) {}
          if (labelIcon !== "label-pill" || !map.hasImage("label-pill")) {
            try {
              map.removeLayer("node-labels");
            } catch (_) {}
            delete map["__metisClick_" + mapId + "_node-labels"];
          }
        }
        if (!map.getLayer("node-labels")) {
          var namedUnclustered = wantCluster
            ? [
                "all",
                ["!", ["has", "point_count"]],
                ["!=", ["coalesce", ["get", "name"], ""], ""],
              ]
            : ["!=", ["coalesce", ["get", "name"], ""], ""];
          var labelLayout = {
            "text-field": ["coalesce", ["get", "name"], ""],
            "text-size": [
              "interpolate",
              ["linear"],
              ["zoom"],
              10,
              12,
              14,
              14,
              17,
              16,
            ],
            "text-font": ["Noto Sans Regular"],
            "text-anchor": "top",
            "text-offset": [
              "interpolate",
              ["linear"],
              ["zoom"],
              10,
              ["literal", [0, 1.55]],
              14,
              ["literal", [0, 2.0]],
              17,
              ["literal", [0, 2.4]],
            ],
            "text-max-width": 10,
            "text-allow-overlap": false,
            "text-ignore-placement": false,
          };
          var labelPaint = {
            "text-color": "#f4f4f5",
          };
          if (map.hasImage("label-pill")) {
            labelLayout["icon-image"] = "label-pill";
            labelLayout["icon-text-fit"] = "both";
            labelLayout["icon-text-fit-padding"] = [2, 6, 2, 6];
            labelLayout["icon-anchor"] = "top";
            labelLayout["icon-allow-overlap"] = false;
            labelLayout["icon-ignore-placement"] = false;
            labelLayout["text-optional"] = false;
            labelPaint["text-halo-width"] = 0;
          } else {
            labelLayout["text-optional"] = true;
            labelPaint["text-halo-color"] = "#0b1220";
            labelPaint["text-halo-width"] = 1.5;
          }
          map.addLayer({
            id: "node-labels",
            type: "symbol",
            source: "nodes",
            filter: namedUnclustered,
            minzoom: 10,
            layout: labelLayout,
            paint: labelPaint,
          });
        }
        self.bindClusterHandlers(mapId);
        self.bindNodeClicks(mapId, "node-pins");
        self.bindNodeClicks(mapId, "node-labels");
        self.bindNodeClicks(mapId, "node-dots");
        // Circles are the fallback when sprites fail; hide them under working icons.
        if (map.getLayer("node-dots")) {
          map.setLayoutProperty(
            "node-dots",
            "visibility",
            map.__metisPinsReady ? "none" : "visible"
          );
        }
      }
      // Register sprites first (sync canvas), then layers — never gate on SVG loadImage.
      this.ensurePinImages(mapId);
      addVisibleLayers();
    },

    bindClusterHandlers: function (mapId) {
      var map = this.maps[mapId];
      if (!map) return;
      var key = "__metisCluster_" + mapId;
      if (map[key]) return;
      map[key] = true;
      function onClusterClick(e) {
        e.preventDefault();
        var layers = ["clusters", "cluster-count"].filter(function (id) {
          return map.getLayer(id);
        });
        var features = layers.length
          ? map.queryRenderedFeatures(e.point, { layers: layers })
          : [];
        if (!features.length) return;
        var clusterId = features[0].properties.cluster_id;
        var source = map.getSource("nodes");
        if (!source || typeof source.getClusterExpansionZoom !== "function") {
          return;
        }
        source.getClusterExpansionZoom(clusterId).then(function (zoom) {
          map.easeTo({
            center: features[0].geometry.coordinates,
            zoom: zoom,
            duration: 500,
          });
        });
      }
      function pointerOn() {
        map.getCanvas().style.cursor = "pointer";
      }
      function pointerOff() {
        map.getCanvas().style.cursor = "";
      }
      ["clusters", "cluster-count"].forEach(function (layerId) {
        map.on("click", layerId, onClusterClick);
        map.on("mouseenter", layerId, pointerOn);
        map.on("mouseleave", layerId, pointerOff);
      });
    },

    nodeFingerprint: function (data, cluster) {
      var feats = (data && data.features) || [];
      var parts = [cluster ? "1" : "0", String(feats.length)];
      for (var i = 0; i < feats.length; i++) {
        var p = (feats[i] && feats[i].properties) || {};
        parts.push(
          String(p.pubkey || p.name || i),
          String(p.color || ""),
          String(p.favorite || ""),
          String(p.type_key || ""),
          String(p.lat || ""),
          String(p.lon || ""),
          String(p.stale || "")
        );
      }
      return parts.join("|");
    },

    prefersReducedMotion: function () {
      try {
        return !!(
          global.matchMedia &&
          global.matchMedia("(prefers-reduced-motion: reduce)").matches
        );
      } catch (_) {
        return false;
      }
    },

    /**
     * Ephemeral sonar at a lon/lat (message TX/RX, new pin, move).
     * kind: "message" | "arrive" | "move" — arrive adds a brief warp core.
     */
    pulseAt: function (mapId, lng, lat, color, kind) {
      var map = this.maps[mapId];
      if (!map || !global.maplibregl || typeof global.maplibregl.Marker !== "function") {
        return false;
      }
      if (!isFinite(lng) || !isFinite(lat)) return false;
      if (this.prefersReducedMotion()) return false;
      var c =
        typeof color === "string" && color
          ? color
          : "rgba(255, 255, 255, 0.9)";
      var mode = kind === "arrive" || kind === "move" ? kind : "message";
      var root = document.createElement("div");
      root.className =
        mode === "arrive" ? "metis-sonar metis-sonar--warp" : "metis-sonar";
      root.style.setProperty("--metis-sonar-color", c);
      if (mode === "arrive") {
        var core = document.createElement("div");
        core.className = "metis-sonar__core";
        root.appendChild(core);
      }
      var ring1 = document.createElement("div");
      ring1.className = "metis-sonar__ring";
      root.appendChild(ring1);
      var ring2 = document.createElement("div");
      ring2.className = "metis-sonar__ring metis-sonar__ring--delay";
      root.appendChild(ring2);
      var marker = new global.maplibregl.Marker({
        element: root,
        anchor: "center",
        pitchAlignment: "map",
        rotationAlignment: "map",
      })
        .setLngLat([lng, lat])
        .addTo(map);
      if (!this.sonarMarkers) this.sonarMarkers = {};
      if (!this.sonarMarkers[mapId]) this.sonarMarkers[mapId] = [];
      this.sonarMarkers[mapId].push(marker);
      var self = this;
      var lifetime = mode === "arrive" ? 1300 : 1200;
      setTimeout(function () {
        try {
          marker.remove();
        } catch (_) {}
        var bag = self.sonarMarkers && self.sonarMarkers[mapId];
        if (!bag) return;
        var idx = bag.indexOf(marker);
        if (idx >= 0) bag.splice(idx, 1);
      }, lifetime);
      return true;
    },

    /** Look up a node in the current map GeoJSON and pulse its coordinates. */
    pulsePubkey: function (mapId, pubkey, color, kind) {
      var data = this.nodeData[mapId];
      var feats = (data && data.features) || [];
      var key = String(pubkey || "");
      if (!key) return false;
      for (var i = 0; i < feats.length; i++) {
        var f = feats[i];
        var p = (f && f.properties) || {};
        if (String(p.pubkey || "") !== key) continue;
        var coords = f.geometry && f.geometry.coordinates;
        if (!coords || coords.length < 2) return false;
        var tint = color || p.color || null;
        return this.pulseAt(mapId, coords[0], coords[1], tint, kind);
      }
      return false;
    },

    clearSonar: function (mapId) {
      var bag = this.sonarMarkers && this.sonarMarkers[mapId];
      if (!bag) return;
      bag.forEach(function (marker) {
        try {
          marker.remove();
        } catch (_) {}
      });
      this.sonarMarkers[mapId] = [];
    },

    /** First paint seeds; later never-seen pubkeys with coords get a warp-in. */
    noteArrivals: function (mapId, data) {
      if (!this.seenPubkeys) this.seenPubkeys = {};
      if (!this.seenPubkeys[mapId]) {
        this.seenPubkeys[mapId] = { seeded: false, keys: {} };
      }
      var state = this.seenPubkeys[mapId];
      var feats = (data && data.features) || [];
      var arrivals = [];
      for (var i = 0; i < feats.length; i++) {
        var f = feats[i];
        var p = (f && f.properties) || {};
        var key = String(p.pubkey || "");
        if (!key) continue;
        var coords = f.geometry && f.geometry.coordinates;
        if (!coords || coords.length < 2) continue;
        if (!isFinite(coords[0]) || !isFinite(coords[1])) continue;
        if (!state.seeded) {
          state.keys[key] = true;
          continue;
        }
        if (state.keys[key]) continue;
        state.keys[key] = true;
        arrivals.push({
          lng: coords[0],
          lat: coords[1],
          color: p.color || null,
        });
      }
      if (!state.seeded) {
        state.seeded = true;
        return;
      }
      for (var j = 0; j < arrivals.length && j < 8; j++) {
        var a = arrivals[j];
        this.pulseAt(mapId, a.lng, a.lat, a.color, "arrive");
      }
    },

    setNodes: function (mapId, data, cluster, forceRebuild) {
      var map = this.maps[mapId];
      if (!map) return false;
      var enriched = enrichNodeFeatures(
        data || { type: "FeatureCollection", features: [] }
      );
      if (!this.nodeFp) this.nodeFp = {};
      var fp = this.nodeFingerprint(enriched, cluster);
      var same = !forceRebuild && this.nodeFp[mapId] === fp;
      this.nodeData[mapId] = enriched;
      this.ensureNodeLayers(mapId, cluster);
      var source = map.getSource("nodes");
      if (!source) return false;
      var empty = false;
      try {
        empty =
          !source._data ||
          !source._data.features ||
          !source._data.features.length;
      } catch (_) {
        empty = true;
      }
      if (!same || forceRebuild || empty) {
        source.setData(enriched);
      }
      this.nodeFp[mapId] = fp;
      this.noteArrivals(mapId, enriched);
      return true;
    },

    forceResize: function (mapId) {
      var map = this.maps[mapId];
      if (!map) return;
      try {
        // Prefer the stage host size so flex layout wins over a stale child box.
        var container = map.getContainer();
        var host = container && container.parentElement;
        if (host && container) {
          var w = host.clientWidth;
          var h = host.clientHeight;
          if (w > 0 && h > 0) {
            container.style.width = w + "px";
            container.style.height = h + "px";
          }
        }
        map.resize();
      } catch (_) {}
      // Second pass after layout/paint — WebView often reports the old box once.
      var self = this;
      if (typeof requestAnimationFrame === "function") {
        requestAnimationFrame(function () {
          try {
            map.resize();
          } catch (_) {}
          requestAnimationFrame(function () {
            try {
              map.resize();
            } catch (_) {}
          });
        });
      } else {
        setTimeout(function () {
          try {
            self.maps[mapId] && self.maps[mapId].resize();
          } catch (_) {}
        }, 50);
      }
    },

    watchResize: function (mapId, container) {
      var map = this.maps[mapId];
      if (!map || !container) return;
      if (this._resizeObs && this._resizeObs[mapId]) {
        try {
          this._resizeObs[mapId].disconnect();
        } catch (_) {}
      }
      if (!this._resizeObs) this._resizeObs = {};
      var self = this;
      var schedule = function () {
        if (self._resizeTimer) clearTimeout(self._resizeTimer);
        self._resizeTimer = setTimeout(function () {
          self.forceResize(mapId);
        }, 40);
      };
      if (typeof ResizeObserver !== "undefined") {
        var obs = new ResizeObserver(schedule);
        obs.observe(container);
        if (container.parentElement) obs.observe(container.parentElement);
        this._resizeObs[mapId] = obs;
      }
      if (typeof window !== "undefined") {
        if (this._windowResize && this._windowResize[mapId]) {
          try {
            window.removeEventListener("resize", this._windowResize[mapId]);
          } catch (_) {}
        }
        if (!this._windowResize) this._windowResize = {};
        this._windowResize[mapId] = schedule;
        window.addEventListener("resize", schedule);
      }
      schedule();
    },

    /** If outline GeoJSON URL sources failed to populate, fetch via metis:// and setData. */
    hydrateOutlineSources: async function (mapId) {
      var map = this.maps[mapId];
      if (!map) return;
      var ids = ["world-land", "admin0-borders", "admin1-lines"];
      for (var i = 0; i < ids.length; i++) {
        var id = ids[i];
        var source = map.getSource(id);
        if (!source || typeof source.setData !== "function") continue;
        try {
          var res = await fetch("metis://local/geojson/" + id);
          if (!res || !res.ok) continue;
          var data = await res.json();
          source.setData(data);
        } catch (_) {}
      }
    },

    setStyle: async function (mapId, style, camera) {
      var map = this.maps[mapId];
      if (!map) return false;
      this.clearPmtilesCache();
      this.rememberCoverageFromStyle(mapId, style);
      var center =
        camera && isFinite(camera.lng) && isFinite(camera.lat)
          ? [camera.lng, camera.lat]
          : map.getCenter();
      var zoom =
        camera && isFinite(camera.zoom) ? camera.zoom : map.getZoom();
      var bearing = map.getBearing();
      var pitch = map.getPitch();
      var self = this;
      function restoreCamera() {
        try {
          map.jumpTo({
            center: center,
            zoom: zoom,
            bearing: bearing,
            pitch: pitch,
          });
          map.resize();
        } catch (_) {}
      }
      // Drop overlay state that belonged to the previous style.
      this.clearLabels(mapId);
      this.teardownCoverageDrag(mapId);
      if (self.clusterEnabled) delete self.clusterEnabled[mapId];
      if (this.nodeFp) delete this.nodeFp[mapId];
      // Style swap clears GL images — re-register pin/pill/shield sprites after load.
      map.__metisPinsReady = false;
      map.__metisPinsLoading = false;
      map.__metisPillsBuilt = false;
      map.__metisShieldsBuilt = false;
      // Reset click binders so layers can rebind after style.load.
      Object.keys(map).forEach(function (k) {
        if (
          k.indexOf("__metisClick_") === 0 ||
          k.indexOf("__metisCluster_") === 0 ||
          k === "__metisCoverageBound"
        ) {
          try {
            delete map[k];
          } catch (_) {}
        }
      });
      await new Promise(function (resolve) {
        var settled = false;
        function done() {
          if (settled) return;
          settled = true;
          restoreCamera();
          // Style load can settle before layout; re-assert camera shortly after.
          setTimeout(restoreCamera, 50);
          setTimeout(restoreCamera, 250);
          resolve(true);
        }
        map.once("style.load", done);
        // Style/source/tile errors must not abort the swap (was "Map failed").
        setTimeout(done, 4000);
        try {
          map.setStyle(style);
        } catch (err) {
          done();
        }
      });
      try {
        await this.hydrateOutlineSources(mapId);
      } catch (_) {}
      try {
        this.ensurePinImages(mapId);
      } catch (_) {}
      try {
        if (this.regionDraft[mapId]) {
          this.renderRegionDraft(mapId);
          this.bindDraftDrag(mapId);
        }
      } catch (_) {}
      restoreCamera();
      return true;
    },

    setGeoJson: function (mapId, sourceId, data, layerSpecs) {
      var map = this.maps[mapId];
      if (!map) return false;
      if (sourceId === "nodes") {
        return this.setNodes(mapId, data, this.clusterEnabled[mapId] !== false);
      }
      if (map.getSource(sourceId)) {
        map.getSource(sourceId).setData(data);
      } else {
        map.addSource(sourceId, { type: "geojson", data: data });
      }
      (layerSpecs || []).forEach(function (layer) {
        if (!map.getLayer(layer.id)) map.addLayer(layer);
      });
      return true;
    },

    setVisibility: function (mapId, layerId, visible) {
      var map = this.maps[mapId];
      if (!map || !map.getLayer(layerId)) return;
      map.setLayoutProperty(layerId, "visibility", visible ? "visible" : "none");
    },

    fitBounds: function (mapId, west, south, east, north, padding) {
      var map = this.maps[mapId];
      if (!map) return;
      if (
        !isFinite(west) ||
        !isFinite(south) ||
        !isFinite(east) ||
        !isFinite(north) ||
        west === east ||
        south === north
      ) {
        return;
      }
      map.fitBounds(
        [
          [west, south],
          [east, north],
        ],
        { padding: padding || 48, maxZoom: 14, duration: 600 }
      );
    },

    flyTo: function (mapId, lng, lat, zoom) {
      var map = this.maps[mapId];
      if (!map) return;
      map.flyTo({ center: [lng, lat], zoom: zoom || 12, duration: 700 });
    },

    /** Soft recenter for Tracker follow — keeps zoom, skips if operator is dragging. */
    easeTo: function (mapId, lng, lat) {
      var map = this.maps[mapId];
      if (!map || !isFinite(lng) || !isFinite(lat)) return false;
      if (global.__metisMapMoving) return false;
      try {
        map.easeTo({
          center: [lng, lat],
          zoom: map.getZoom(),
          duration: 650,
          essential: true,
        });
        return true;
      } catch (_) {
        return false;
      }
    },

    bindNodeClicks: function (mapId, layerId) {
      var map = this.maps[mapId];
      if (!map) return;
      var key = "__metisClick_" + mapId + "_" + layerId;
      if (map[key]) return;
      map[key] = true;
      var self = this;
      map.on("click", layerId, function (e) {
        e.preventDefault();
        if (!e.features || !e.features.length) return;
        var props = e.features[0].properties || {};
        if (props.cluster || props.point_count) return;
        var coords = e.features[0].geometry && e.features[0].geometry.coordinates;
        self.showDetail(mapId, coords || e.lngLat, props);
      });
      map.on("mouseenter", layerId, function () {
        map.getCanvas().style.cursor = "pointer";
      });
      map.on("mouseleave", layerId, function () {
        map.getCanvas().style.cursor = "";
      });
    },

    takeSelection: function () {
      var raw = global.__metisMapSelection;
      global.__metisMapSelection = null;
      return raw || "";
    },

    takeFavoriteToggle: function () {
      var raw = global.__metisMapFavoriteToggle;
      global.__metisMapFavoriteToggle = null;
      return raw || "";
    },

    coveragePolygon: function (bbox) {
      var w = bbox[0],
        s = bbox[1],
        e = bbox[2],
        n = bbox[3];
      return {
        type: "FeatureCollection",
        features: [
          {
            type: "Feature",
            properties: { shape: "rectangle" },
            geometry: {
              type: "Polygon",
              coordinates: [
                [
                  [w, s],
                  [e, s],
                  [e, n],
                  [w, n],
                  [w, s],
                ],
              ],
            },
          },
        ],
      };
    },

    readCoverageBbox: function (mapId) {
      var bbox = this.coverageBbox[mapId];
      return bbox && bbox.length === 4 ? bbox.slice() : null;
    },

    setCoverageBbox: function (mapId, bbox) {
      var map = this.maps[mapId];
      if (!map || !bbox) return false;
      this.coverageBbox[mapId] = bbox.slice();
      var source = map.getSource("tile-coverage");
      if (!source || typeof source.setData !== "function") return false;
      source.setData(this.coveragePolygon(bbox));
      return true;
    },

    /** Committed pack ring is display-only; placement uses the draft shape + ✓/✕. */
    teardownCoverageDrag: function (mapId) {
      var map = this.maps[mapId];
      var state = this.coverageDrag[mapId];
      if (state && map) {
        try {
          if (state.onMove) map.off("mousemove", state.onMove);
          if (state.onUp) {
            map.off("mouseup", state.onUp);
            map.off("touchend", state.onUp);
          }
          if (state.onDownFill) map.off("mousedown", "tile-coverage-fill", state.onDownFill);
          if (state.onDownLine) map.off("mousedown", "tile-coverage-line", state.onDownLine);
          if (state.onEnter) {
            map.off("mouseenter", "tile-coverage-fill", state.onEnter);
            map.off("mouseenter", "tile-coverage-line", state.onEnter);
          }
          if (state.onLeave) {
            map.off("mouseleave", "tile-coverage-fill", state.onLeave);
            map.off("mouseleave", "tile-coverage-line", state.onLeave);
          }
        } catch (_) {}
      }
      delete this.coverageDrag[mapId];
      if (map) {
        try {
          delete map.__metisCoverageBound;
          map.dragPan.enable();
          map.getCanvas().style.cursor = "";
        } catch (_) {}
      }
    },

    bindCoverageDrag: function () {
      // No-op: editing goes through placeRegionDraft + confirm/cancel markers.
      return false;
    },

    offsetLngLat: function (lng, lat, eastMiles, northMiles) {
      var cos = Math.cos((lat * Math.PI) / 180);
      if (Math.abs(cos) < 0.2) cos = cos < 0 ? -0.2 : 0.2;
      return [lng + eastMiles / (69.0 * cos), lat + northMiles / 69.0];
    },

    shapeRing: function (_shape, lng, lat, radiusMiles) {
      // Offline packs are always an axis-aligned square (PMTiles bbox extract).
      var r = Math.max(5, Math.min(300, radiusMiles || 50));
      var nw = this.offsetLngLat(lng, lat, -r, r);
      var ne = this.offsetLngLat(lng, lat, r, r);
      var se = this.offsetLngLat(lng, lat, r, -r);
      var sw = this.offsetLngLat(lng, lat, -r, -r);
      return [nw, ne, se, sw, nw];
    },

    ringBbox: function (ring) {
      var lons = ring.map(function (p) {
        return p[0];
      });
      var lats = ring.map(function (p) {
        return p[1];
      });
      return [
        Math.min.apply(null, lons),
        Math.min.apply(null, lats),
        Math.max.apply(null, lons),
        Math.max.apply(null, lats),
      ];
    },

    draftFeatureCollection: function (draft) {
      var ring = this.shapeRing(draft.shape, draft.lng, draft.lat, draft.radiusMiles);
      return {
        type: "FeatureCollection",
        features: [
          {
            type: "Feature",
            properties: {
              shape: draft.shape,
              mode: draft.mode || "new",
            },
            geometry: { type: "Polygon", coordinates: [ring] },
          },
        ],
      };
    },

    ensureDraftLayers: function (mapId) {
      var map = this.maps[mapId];
      if (!map) return false;
      if (!map.getSource("region-draft")) {
        map.addSource("region-draft", {
          type: "geojson",
          data: { type: "FeatureCollection", features: [] },
        });
      }
      if (!map.getLayer("region-draft-fill")) {
        map.addLayer({
          id: "region-draft-fill",
          type: "fill",
          source: "region-draft",
          paint: {
            "fill-color": "#34d399",
            "fill-opacity": 0.14,
          },
        });
      }
      if (!map.getLayer("region-draft-line")) {
        map.addLayer({
          id: "region-draft-line",
          type: "line",
          source: "region-draft",
          paint: {
            "line-color": "#34d399",
            "line-width": 2.25,
            "line-dasharray": [1.5, 1.25],
            "line-opacity": 0.95,
          },
        });
      }
      return true;
    },

    setCommittedCoverageVisible: function (mapId, visible) {
      var map = this.maps[mapId];
      if (!map) return;
      ["tile-coverage-fill", "tile-coverage-line"].forEach(function (id) {
        if (map.getLayer(id)) {
          try {
            map.setLayoutProperty(id, "visibility", visible ? "visible" : "none");
          } catch (_) {}
        }
      });
    },

    /** Full redraw (place / style restore). Avoid on every pointer move. */
    renderRegionDraft: function (mapId) {
      var map = this.maps[mapId];
      var draft = this.regionDraft[mapId];
      if (!map || !draft) return false;
      if (!this.ensureDraftLayers(mapId)) return false;
      var source = map.getSource("region-draft");
      if (!source) return false;
      var ring = this.shapeRing(
        draft.shape,
        draft.lng,
        draft.lat,
        draft.radiusMiles
      );
      this.applyDraftRingMeta(draft, ring);
      source.setData(this.draftFeatureCollection(draft));
      return true;
    },

    /** Stamp bbox from the ring currently painted (authoritative after drag). */
    applyDraftRingMeta: function (draft, ring) {
      if (!draft || !ring || !ring.length) return;
      var bbox = this.ringBbox(ring);
      draft.west = bbox[0];
      draft.south = bbox[1];
      draft.east = bbox[2];
      draft.north = bbox[3];
      draft.lng = (bbox[0] + bbox[2]) / 2;
      draft.lat = (bbox[1] + bbox[3]) / 2;
      draft.lastRing = ring;
    },

    /** Cheap drag frame: translate a cached ring + setData only (no marker DOM). */
    paintDraftRing: function (mapId, ring) {
      var map = this.maps[mapId];
      var draft = this.regionDraft[mapId];
      if (!map || !draft) return;
      var source = map.getSource("region-draft");
      if (!source) return;
      this.applyDraftRingMeta(draft, ring);
      source.setData({
        type: "FeatureCollection",
        features: [
          {
            type: "Feature",
            properties: {
              shape: draft.shape,
              mode: draft.mode || "new",
            },
            geometry: { type: "Polygon", coordinates: [ring] },
          },
        ],
      });
    },

    placeRegionDraft: function (mapId, opts) {
      var map = this.maps[mapId];
      if (!map) return false;
      opts = opts || {};
      // Always prefer the live map center unless the caller opts into explicit coords.
      // Rust used to pass a hardcoded LA fallback when getCamera failed, which pinned
      // every pack to -118.24,34.05 even after the operator dragged the square.
      var center = map.getCenter();
      var useCoords = !!opts.useCoords;
      var lng = useCoords && isFinite(opts.lng) ? opts.lng : center.lng;
      var lat = useCoords && isFinite(opts.lat) ? opts.lat : center.lat;
      var shape = "rectangle";
      var radius = isFinite(opts.radiusMiles) ? opts.radiusMiles : 50;
      radius = Math.max(5, Math.min(300, radius));
      this.regionDraft[mapId] = {
        shape: shape,
        radiusMiles: radius,
        lng: lng,
        lat: lat,
        mode: opts.mode === "adjust" ? "adjust" : "new",
      };
      this.teardownDraftDrag(mapId);
      // Hide committed pack ring so it does not look like a lagging ghost box.
      this.setCommittedCoverageVisible(mapId, false);
      this.renderRegionDraft(mapId);
      var ring = this.shapeRing(shape, lng, lat, radius);
      this.applyDraftRingMeta(this.regionDraft[mapId], ring);
      this.bindDraftDrag(mapId);
      this.publishLiveDraft(mapId);
      return true;
    },

    clearRegionDraft: function (mapId) {
      this.teardownDraftDrag(mapId);
      delete this.regionDraft[mapId];
      this.clearDraftGlobals();
      var map = this.maps[mapId];
      if (map && map.getSource("region-draft")) {
        try {
          map.getSource("region-draft").setData({
            type: "FeatureCollection",
            features: [],
          });
        } catch (_) {}
      }
      this.setCommittedCoverageVisible(mapId, true);
      return true;
    },

    draftPayload: function (draft) {
      var bbox;
      if (
        isFinite(draft.west) &&
        isFinite(draft.south) &&
        isFinite(draft.east) &&
        isFinite(draft.north)
      ) {
        bbox = [draft.west, draft.south, draft.east, draft.north];
      } else {
        var ring = this.shapeRing(
          draft.shape,
          draft.lng,
          draft.lat,
          draft.radiusMiles
        );
        bbox = this.ringBbox(ring);
      }
      return {
        shape: draft.shape,
        mode: draft.mode || "new",
        radiusMiles: draft.radiusMiles,
        lng: isFinite(draft.lng) ? draft.lng : (bbox[0] + bbox[2]) / 2,
        lat: isFinite(draft.lat) ? draft.lat : (bbox[1] + bbox[3]) / 2,
        west: bbox[0],
        south: bbox[1],
        east: bbox[2],
        north: bbox[3],
      };
    },

    clearDraftGlobals: function () {
      global.__metisRegionDraftLive = "";
      global.__metisDraftW = null;
      global.__metisDraftS = null;
      global.__metisDraftE = null;
      global.__metisDraftN = null;
      global.__metisDraftLng = null;
      global.__metisDraftLat = null;
      global.__metisDraftR = null;
      global.__metisDraftMode = "";
      global.__metisDraftShape = "";
    },

    /** Publish draft to window globals. Numeric fields are the reliable path for Rust. */
    publishLiveDraft: function (mapId) {
      var draft = this.regionDraft[mapId];
      if (!draft) {
        this.clearDraftGlobals();
        return "";
      }
      var p = this.draftPayload(draft);
      var json = JSON.stringify(p);
      global.__metisRegionDraftLive = json;
      // Plain numbers — desktop eval often mangles long JSON strings.
      global.__metisDraftW = p.west;
      global.__metisDraftS = p.south;
      global.__metisDraftE = p.east;
      global.__metisDraftN = p.north;
      global.__metisDraftLng = p.lng;
      global.__metisDraftLat = p.lat;
      global.__metisDraftR = p.radiusMiles;
      global.__metisDraftMode = p.mode || "new";
      global.__metisDraftShape = p.shape || "rectangle";
      global.__metisRegionDraftSeq = (global.__metisRegionDraftSeq || 0) + 1;
      return json;
    },

    /** Resolve active draft (exact mapId or any). */
    activeRegionDraft: function (mapId) {
      var draft = this.regionDraft[mapId];
      if (draft) return { mapId: mapId, draft: draft };
      var keys = Object.keys(this.regionDraft || {});
      if (!keys.length) return null;
      return { mapId: keys[0], draft: this.regionDraft[keys[0]] };
    },

    /** Recover draft memory from the GeoJSON source if regionDraft was dropped. */
    recoverDraftFromSource: function (mapId) {
      var map = this.maps[mapId];
      if (!map) {
        var ids = Object.keys(this.maps || {});
        if (!ids.length) return null;
        mapId = ids[0];
        map = this.maps[mapId];
      }
      if (!map) return null;
      var source = map.getSource("region-draft");
      if (!source) return null;
      var data = null;
      try {
        data = source._data || (source.serialize && source.serialize().data) || null;
      } catch (_) {
        data = null;
      }
      var ring =
        data &&
        data.features &&
        data.features[0] &&
        data.features[0].geometry &&
        data.features[0].geometry.coordinates &&
        data.features[0].geometry.coordinates[0];
      if (!ring || ring.length < 4) return null;
      var props =
        (data.features[0] && data.features[0].properties) || {};
      var draft = {
        shape: props.shape || "rectangle",
        radiusMiles: 50,
        lng: 0,
        lat: 0,
        mode: props.mode === "adjust" ? "adjust" : "new",
      };
      this.applyDraftRingMeta(draft, ring);
      // Approximate radius from north-south span.
      draft.radiusMiles = Math.max(
        5,
        Math.min(300, ((draft.north - draft.south) / 2) * 69)
      );
      this.regionDraft[mapId] = draft;
      return { mapId: mapId, draft: draft };
    },

    /** Live draft as JSON string (does not clear). Refreshes from painted ring. */
    snapshotRegionDraft: function (mapId) {
      var active = this.activeRegionDraft(mapId) || this.recoverDraftFromSource(mapId);
      if (!active) {
        return global.__metisRegionDraftLive || "";
      }
      var draft = active.draft;
      // Prefer the ring currently painted on the map (post-drag), not Place coords.
      if (draft.lastRing && draft.lastRing.length) {
        this.applyDraftRingMeta(draft, draft.lastRing);
      } else {
        var ring = this.shapeRing(
          draft.shape,
          draft.lng,
          draft.lat,
          draft.radiusMiles
        );
        this.applyDraftRingMeta(draft, ring);
      }
      return this.publishLiveDraft(active.mapId);
    },

    /**
     * One-shot wire for Rust: refresh draft + return a short pipe string.
     * Desktop eval handles short strings (same as getCamera); objects/long JSON fail.
     * Format: west|south|east|north|lng|lat|radiusMiles|mode|shape
     */
    snapshotRegionDraftWire: function (mapId) {
      this.snapshotRegionDraft(mapId);
      var w = global.__metisDraftW,
        s = global.__metisDraftS,
        e = global.__metisDraftE,
        n = global.__metisDraftN;
      if (
        ![w, s, e, n].every(function (x) {
          return typeof x === "number" && isFinite(x);
        })
      ) {
        return "";
      }
      return [
        w,
        s,
        e,
        n,
        global.__metisDraftLng,
        global.__metisDraftLat,
        global.__metisDraftR || 50,
        global.__metisDraftMode || "new",
        global.__metisDraftShape || "rectangle",
      ].join("|");
    },

    teardownDraftDrag: function (mapId) {
      var map = this.maps[mapId];
      var state = this.coverageDrag[mapId];
      if (state && map) {
        try {
          if (state.raf) cancelAnimationFrame(state.raf);
          if (state.onMove) map.off("mousemove", state.onMove);
          if (state.onUp) {
            map.off("mouseup", state.onUp);
            map.off("touchend", state.onUp);
          }
          if (state.onDownFill) map.off("mousedown", "region-draft-fill", state.onDownFill);
          if (state.onDownLine) map.off("mousedown", "region-draft-line", state.onDownLine);
          if (state.onEnter) {
            map.off("mouseenter", "region-draft-fill", state.onEnter);
            map.off("mouseenter", "region-draft-line", state.onEnter);
          }
          if (state.onLeave) {
            map.off("mouseleave", "region-draft-fill", state.onLeave);
            map.off("mouseleave", "region-draft-line", state.onLeave);
          }
        } catch (_) {}
      }
      delete this.coverageDrag[mapId];
      if (map) {
        try {
          delete map.__metisDraftBound;
          map.dragPan.enable();
          map.getCanvas().style.cursor = "";
        } catch (_) {}
      }
    },

    bindDraftDrag: function (mapId) {
      var map = this.maps[mapId];
      if (!map || !map.getLayer("region-draft-fill")) return false;
      this.teardownDraftDrag(mapId);
      var self = this;
      var state = {
        dragging: false,
        startLngLat: null,
        startCenter: null,
        baseRing: null,
        pendingLngLat: null,
        raf: 0,
      };

      function beginDrag(e) {
        var draft = self.regionDraft[mapId];
        if (!draft || !e || !e.lngLat) return;
        state.dragging = true;
        state.startLngLat = { lng: e.lngLat.lng, lat: e.lngLat.lat };
        state.startCenter = { lng: draft.lng, lat: draft.lat };
        // Prefer the last painted ring so drag continues from where it looks.
        state.baseRing =
          draft.lastRing && draft.lastRing.length
            ? draft.lastRing.slice()
            : self.shapeRing(
                draft.shape,
                draft.lng,
                draft.lat,
                draft.radiusMiles
              );
        map.dragPan.disable();
        map.getCanvas().style.cursor = "grabbing";
        if (e.originalEvent && e.originalEvent.preventDefault) {
          e.originalEvent.preventDefault();
        }
      }

      function flushMove() {
        state.raf = 0;
        if (!state.dragging || !state.pendingLngLat || !state.startLngLat || !state.startCenter) {
          return;
        }
        var draft = self.regionDraft[mapId];
        if (!draft || !state.baseRing) return;
        var dLng = state.pendingLngLat.lng - state.startLngLat.lng;
        var dLat = state.pendingLngLat.lat - state.startLngLat.lat;
        var moved = new Array(state.baseRing.length);
        for (var i = 0; i < state.baseRing.length; i++) {
          var p = state.baseRing[i];
          moved[i] = [p[0] + dLng, p[1] + dLat];
        }
        // Paint only while dragging — publish JSON once on mouseup (keeps drag snappy).
        self.paintDraftRing(mapId, moved);
      }

      state.onDownFill = function (e) {
        beginDrag(e);
      };
      state.onDownLine = function (e) {
        beginDrag(e);
      };
      state.onMove = function (e) {
        if (!state.dragging || !e || !e.lngLat) return;
        state.pendingLngLat = { lng: e.lngLat.lng, lat: e.lngLat.lat };
        if (state.raf) return;
        state.raf = requestAnimationFrame(flushMove);
      };
      state.onUp = function () {
        if (!state.dragging) return;
        if (state.raf) {
          cancelAnimationFrame(state.raf);
          flushMove();
        }
        state.dragging = false;
        state.startLngLat = null;
        state.startCenter = null;
        state.baseRing = null;
        state.pendingLngLat = null;
        map.dragPan.enable();
        map.getCanvas().style.cursor = "grab";
        self.publishLiveDraft(mapId);
      };
      state.onEnter = function () {
        if (!state.dragging) map.getCanvas().style.cursor = "grab";
      };
      state.onLeave = function () {
        if (!state.dragging) map.getCanvas().style.cursor = "";
      };

      map.on("mousedown", "region-draft-fill", state.onDownFill);
      map.on("mousedown", "region-draft-line", state.onDownLine);
      map.on("mousemove", state.onMove);
      map.on("mouseup", state.onUp);
      map.on("touchend", state.onUp);
      map.on("mouseenter", "region-draft-fill", state.onEnter);
      map.on("mouseenter", "region-draft-line", state.onEnter);
      map.on("mouseleave", "region-draft-fill", state.onLeave);
      map.on("mouseleave", "region-draft-line", state.onLeave);
      this.coverageDrag[mapId] = state;
      map.__metisDraftBound = true;
      return true;
    },
  };

  global.MetisMap = MetisMap;

  // WebKitGTK + libepoxy asserts if WebGL outlives the window. Wipe maps
  // synchronously on exit before the web process tears down GL.
  if (typeof global.addEventListener === "function" && !global.__metisMapExitHook) {
    global.__metisMapExitHook = true;
    var wipeMapsOnExit = function () {
      try {
        if (global.MetisMap && typeof global.MetisMap.destroyAll === "function") {
          global.MetisMap.destroyAll();
        }
      } catch (_) {}
    };
    global.addEventListener("pagehide", wipeMapsOnExit);
    global.addEventListener("beforeunload", wipeMapsOnExit);
  }
})(typeof window !== "undefined" ? window : globalThis);
