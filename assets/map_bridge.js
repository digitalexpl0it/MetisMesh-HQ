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

  function typeKey(props) {
    if (props.type_key) return String(props.type_key);
    if (props.enrollment === "hq") return "hq";
    var t = Number(props.contact_type);
    if (t === 1) return "chat";
    if (t === 2) return "repeater";
    if (t === 3) return "room";
    if (t === 4) return "sensor";
    return "node";
  }

  function typeIconSvg(key) {
    // 24x24 stroke icons, white on colored pin.
    var common =
      'xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"';
    if (key === "hq") {
      return (
        "<svg " +
        common +
        '><path d="M12 3l8 6v11a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V9z"/><path d="M9 21V12h6v9"/></svg>'
      );
    }
    if (key === "chat") {
      return (
        "<svg " +
        common +
        '><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>'
      );
    }
    if (key === "repeater") {
      return (
        "<svg " +
        common +
        '><path d="M4.9 19.1a10 10 0 0 1 0-14.2"/><path d="M7.8 16.2a6 6 0 0 1 0-8.4"/><circle cx="12" cy="12" r="1.4" fill="currentColor" stroke="none"/><path d="M16.2 16.2a6 6 0 0 0 0-8.4"/><path d="M19.1 19.1a10 10 0 0 0 0-14.2"/></svg>'
      );
    }
    if (key === "room") {
      return (
        "<svg " +
        common +
        '><path d="M3 21h18"/><path d="M5 21V8l7-4 7 4v13"/><path d="M10 21v-6h4v6"/></svg>'
      );
    }
    if (key === "sensor") {
      return (
        "<svg " +
        common +
        '><path d="M22 12h-4l-3 9L9 3l-3 9H2"/></svg>'
      );
    }
    return (
      "<svg " +
      common +
      '><circle cx="12" cy="12" r="3"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="M2 12h2"/><path d="M20 12h2"/></svg>'
    );
  }

  function pinHtml(props) {
    var name = escapeHtml(props.name || "Node");
    var typeLabel = escapeHtml(props.type_label || "Node");
    var key = typeKey(props);
    var color = escapeHtml(props.color || "#94a3b8");
    var favorited =
      props.favorite === true ||
      props.favorite === "true" ||
      props.favorite === 1 ||
      props.favorite === "1";
    return (
      '<div class="metis-node-pin-wrap' +
      (favorited ? " is-favorite" : "") +
      '">' +
      '<div class="metis-node-pin metis-type-' +
      escapeHtml(key) +
      '" style="--pin:' +
      color +
      '">' +
      typeIconSvg(key) +
      "</div>" +
      '<div class="metis-node-label">' +
      '<span class="metis-node-label-name">' +
      name +
      "</span>" +
      '<span class="metis-node-label-type">' +
      typeLabel +
      "</span></div></div>"
    );
  }

  function clusterHtml(count) {
    var tone = count >= 40 ? "hot" : count >= 15 ? "warm" : "cool";
    return (
      '<div class="metis-cluster metis-cluster-' +
      tone +
      '"><span>' +
      escapeHtml(count) +
      "</span></div>"
    );
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
    ["click", "mousedown", "mouseup", "pointerdown", "pointerup", "dblclick"].forEach(
      function (type) {
        container.addEventListener(
          type,
          function (ev) {
            // Bubble-phase stop so MapLibre target handlers still run, but the
            // event never reaches the Dioxus root (null element id → console error).
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
    /** Pending place/adjust shape before confirm: { shape, radiusMiles, lng, lat, mode } */
    regionDraft: {},
    regionDraftMarkers: {},

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
      });
      map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");
      this.maps[mapId] = map;
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
      map.on("moveend", function () {
        publishCamera();
        // Clusters/pins can be empty until the source finishes indexing — refresh
        // after pan too when overlays are missing (zoom alone was the old workaround).
        if (
          !self.detailOpen[mapId] &&
          (!self.labels[mapId] || !self.labels[mapId].length) &&
          self.nodeData[mapId]
        ) {
          self.scheduleOverlayRefresh(mapId);
        }
      });
      map.on("zoomend", function () {
        publishCamera();
        if (self._zoomRefreshTimer) clearTimeout(self._zoomRefreshTimer);
        self._zoomRefreshTimer = setTimeout(function () {
          if (!self.detailOpen[mapId]) {
            self.refreshOverlayMarkers(mapId);
          }
        }, 120);
      });
      map.on("sourcedata", function (e) {
        if (
          e &&
          e.sourceId === "nodes" &&
          e.isSourceLoaded &&
          !self.detailOpen[mapId]
        ) {
          self.scheduleOverlayRefresh(mapId);
        }
      });
      map.on("click", function (e) {
        if (e.defaultPrevented) return;
        var layers = ["node-dots", "clusters"].filter(function (id) {
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

    destroy: function (mapId) {
      this.teardownCoverageDrag(mapId);
      this.clearRegionDraft(mapId);
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
      if (!map) return;
      try {
        map.remove();
      } catch (_) {}
      delete this.maps[mapId];
    },

    clearLabels: function (mapId) {
      var list = this.labels[mapId] || [];
      list.forEach(function (marker) {
        try {
          marker.remove();
        } catch (_) {}
      });
      this.labels[mapId] = [];
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

    ensureNodeLayers: function (mapId, cluster) {
      var map = this.maps[mapId];
      if (!map) return;
      var wantCluster = !!cluster;
      var source = map.getSource("nodes");
      if (source && this.clusterEnabled[mapId] !== wantCluster) {
        ["node-dots", "clusters", "cluster-count-hit"].forEach(function (id) {
          if (map.getLayer(id)) map.removeLayer(id);
        });
        map.removeSource("nodes");
        source = null;
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
      if (wantCluster && !map.getLayer("clusters")) {
        // Invisible hit targets only — visible cluster UI is the HTML marker.
        // Drawing both caused the "bubble inside a bubble" look.
        map.addLayer({
          id: "clusters",
          type: "circle",
          source: "nodes",
          filter: ["has", "point_count"],
          paint: {
            "circle-radius": [
              "step",
              ["get", "point_count"],
              22,
              15,
              28,
              40,
              36,
            ],
            "circle-opacity": 0,
          },
        });
      }
      if (!map.getLayer("node-dots")) {
        map.addLayer({
          id: "node-dots",
          type: "circle",
          source: "nodes",
          filter: wantCluster ? ["!", ["has", "point_count"]] : ["all"],
          paint: {
            // Invisible hit target — visible pins are HTML icon markers.
            "circle-radius": 14,
            "circle-color": "#000000",
            "circle-opacity": 0,
            "circle-stroke-width": 0,
          },
        });
      }
      this.bindClusterHandlers(mapId);
      this.bindNodeClicks(mapId, "node-dots");
    },

    bindClusterHandlers: function (mapId) {
      var map = this.maps[mapId];
      if (!map) return;
      var key = "__metisCluster_" + mapId;
      if (map[key]) return;
      map[key] = true;
      map.on("click", "clusters", function (e) {
        e.preventDefault();
        var features = map.queryRenderedFeatures(e.point, {
          layers: ["clusters"],
        });
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
      });
      map.on("mouseenter", "clusters", function () {
        map.getCanvas().style.cursor = "pointer";
      });
      map.on("mouseleave", "clusters", function () {
        map.getCanvas().style.cursor = "";
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
          String(p.type_key || "")
        );
      }
      return parts.join("|");
    },

    scheduleOverlayRefresh: function (mapId) {
      var self = this;
      var map = this.maps[mapId];
      if (!map) return;
      if (this._overlayRefreshTimer) {
        clearTimeout(this._overlayRefreshTimer);
      }
      var run = function () {
        if (!self.detailOpen[mapId]) {
          self.refreshOverlayMarkers(mapId);
        }
      };
      // Clustering / tile index is async after setData — idle + retries avoid
      // empty markers until the user zooms.
      try {
        map.triggerRepaint();
      } catch (_) {}
      map.once("idle", run);
      this._overlayRefreshTimer = setTimeout(run, 80);
      setTimeout(run, 250);
      setTimeout(run, 700);
      setTimeout(run, 1500);
    },

    setNodes: function (mapId, data, cluster, forceRebuild) {
      var map = this.maps[mapId];
      if (!map) return false;
      var fp = this.nodeFingerprint(data, cluster);
      var same = !forceRebuild && this.nodeFp && this.nodeFp[mapId] === fp;
      this.nodeData[mapId] = data;
      if (!this.nodeFp) this.nodeFp = {};
      this.nodeFp[mapId] = fp;
      this.ensureNodeLayers(mapId, cluster);
      var source = map.getSource("nodes");
      if (source) source.setData(data || { type: "FeatureCollection", features: [] });
      var missing =
        !this.labels[mapId] || !this.labels[mapId].length;
      if (!same || missing) {
        this.scheduleOverlayRefresh(mapId);
      }
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
        if (this.regionDraft[mapId]) {
          this.renderRegionDraft(mapId);
          this.bindDraftDrag(mapId);
        }
      } catch (_) {}
      restoreCamera();
      return true;
    },

    refreshOverlayMarkers: function (mapId) {
      var map = this.maps[mapId];
      if (!map || !global.maplibregl) return;
      this.clearLabels(mapId);
      var self = this;
      var clustered = !!this.clusterEnabled[mapId];
      var seen = {};

      function addMarker(el, coords, onClick, anchor) {
        el.style.pointerEvents = "auto";
        el.style.cursor = "pointer";
        // Capture + stopImmediatePropagation: HTML markers are outside the
        // Dioxus VDOM; leaking click events logs "invalid type: null, expected usize".
        wireSealedClick(el, function () {
          if (onClick) onClick();
        });
        // No hover tip popup — it stole pointer events and flickered when
        // markers were rebuilt. Labels on the pin are enough for hover context.
        var marker = new global.maplibregl.Marker({
          element: el,
          anchor: anchor || "bottom",
          offset: [0, 0],
        })
          .setLngLat(coords)
          .addTo(map);
        self.labels[mapId].push(marker);
      }

      if (clustered && map.getSource("nodes")) {
        var features = map.querySourceFeatures("nodes");
        if (features && features.length) {
          features.forEach(function (feature) {
            var props = propsFromFeature(feature);
            var coords =
              feature && feature.geometry && feature.geometry.coordinates;
            if (!coords || coords.length < 2) return;
            if (props.cluster || props.point_count) {
              var cid = "c:" + props.cluster_id;
              if (seen[cid]) return;
              seen[cid] = true;
              var el = document.createElement("div");
              el.className = "metis-cluster-marker";
              el.innerHTML = clusterHtml(props.point_count || 0);
              addMarker(
                el,
                coords,
                function () {
                  var source = map.getSource("nodes");
                  if (
                    !source ||
                    typeof source.getClusterExpansionZoom !== "function"
                  ) {
                    return;
                  }
                  source
                    .getClusterExpansionZoom(props.cluster_id)
                    .then(function (zoom) {
                      map.easeTo({ center: coords, zoom: zoom, duration: 500 });
                    });
                },
                "center"
              );
              return;
            }
            var key =
              props.pubkey || props.name + ":" + coords[0] + "," + coords[1];
            if (seen[key]) return;
            seen[key] = true;
            var label = document.createElement("div");
            label.className = "metis-node-marker";
            label.innerHTML = pinHtml(props);
            addMarker(label, coords, function () {
              self.showDetail(mapId, coords, props);
            });
          });
          return;
        }
        // Cluster index not ready yet — fall through to raw points so pins
        // appear without requiring a manual zoom.
      }

      var collection = this.nodeData[mapId] || { features: [] };
      var list = collection.features || [];
      var zoom = map.getZoom();
      var maxLabels = zoom < 8 ? 48 : zoom < 11 ? 100 : 200;
      list.slice(0, maxLabels).forEach(function (feature) {
        var coords =
          feature && feature.geometry && feature.geometry.coordinates;
        if (!coords || coords.length < 2) return;
        var props = propsFromFeature(feature);
        var el = document.createElement("div");
        el.className = "metis-node-marker";
        el.innerHTML = pinHtml(props);
        addMarker(el, [coords[0], coords[1]], function () {
          self.showDetail(mapId, coords, props);
        });
      });
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

    shapeRing: function (shape, lng, lat, radiusMiles) {
      var r = Math.max(5, Math.min(300, radiusMiles || 50));
      if (shape === "circle") {
        var ring = [];
        var steps = 64;
        for (var i = 0; i <= steps; i++) {
          var ang = (i / steps) * Math.PI * 2;
          ring.push(
            this.offsetLngLat(lng, lat, Math.sin(ang) * r, Math.cos(ang) * r)
          );
        }
        return ring;
      }
      if (shape === "triangle") {
        var top = this.offsetLngLat(lng, lat, 0, r);
        var bl = this.offsetLngLat(lng, lat, -r * 0.9, -r * 0.75);
        var br = this.offsetLngLat(lng, lat, r * 0.9, -r * 0.75);
        return [top, br, bl, top];
      }
      // rectangle (axis-aligned)
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

    clearRegionDraftMarkers: function (mapId) {
      var bag = this.regionDraftMarkers[mapId];
      if (!bag) return;
      try {
        if (bag.actions) bag.actions.remove();
      } catch (_) {}
      delete this.regionDraftMarkers[mapId];
    },

    updateRegionDraftMarkers: function (mapId) {
      var map = this.maps[mapId];
      var draft = this.regionDraft[mapId];
      if (!map || !draft || !global.maplibregl) {
        this.clearRegionDraftMarkers(mapId);
        return;
      }
      var ring = this.shapeRing(draft.shape, draft.lng, draft.lat, draft.radiusMiles);
      var bbox = this.ringBbox(ring);
      var anchor = [bbox[2], bbox[3]]; // NE corner
      var bag = this.regionDraftMarkers[mapId];
      if (!bag) {
        var el = document.createElement("div");
        el.className = "map-region-actions";
        el.innerHTML =
          '<button type="button" class="map-region-btn map-region-confirm" title="Confirm area">✓</button>' +
          '<button type="button" class="map-region-btn map-region-cancel" title="Remove">✕</button>';
        var self = this;
        el.querySelector(".map-region-confirm").addEventListener("click", function (ev) {
          ev.preventDefault();
          ev.stopPropagation();
          self.confirmRegionDraft(mapId);
        });
        el.querySelector(".map-region-cancel").addEventListener("click", function (ev) {
          ev.preventDefault();
          ev.stopPropagation();
          self.cancelRegionDraft(mapId);
        });
        // Keep map drag from eating button clicks.
        el.addEventListener("mousedown", function (ev) {
          ev.stopPropagation();
        });
        el.addEventListener("click", function (ev) {
          ev.stopPropagation();
        });
        bag = {
          actions: new global.maplibregl.Marker({
            element: el,
            anchor: "bottom-left",
            offset: [6, -6],
          })
            .setLngLat(anchor)
            .addTo(map),
        };
        this.regionDraftMarkers[mapId] = bag;
      } else {
        bag.actions.setLngLat(anchor);
      }
    },

    renderRegionDraft: function (mapId) {
      var map = this.maps[mapId];
      var draft = this.regionDraft[mapId];
      if (!map || !draft) return false;
      if (!this.ensureDraftLayers(mapId)) return false;
      var source = map.getSource("region-draft");
      if (!source) return false;
      source.setData(this.draftFeatureCollection(draft));
      this.updateRegionDraftMarkers(mapId);
      return true;
    },

    placeRegionDraft: function (mapId, opts) {
      var map = this.maps[mapId];
      if (!map) return false;
      opts = opts || {};
      var center = map.getCenter();
      var lng = isFinite(opts.lng) ? opts.lng : center.lng;
      var lat = isFinite(opts.lat) ? opts.lat : center.lat;
      var shape = opts.shape || "rectangle";
      if (shape !== "circle" && shape !== "triangle" && shape !== "rectangle") {
        shape = "rectangle";
      }
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
      this.renderRegionDraft(mapId);
      this.bindDraftDrag(mapId);
      return true;
    },

    clearRegionDraft: function (mapId) {
      this.teardownDraftDrag(mapId);
      this.clearRegionDraftMarkers(mapId);
      delete this.regionDraft[mapId];
      var map = this.maps[mapId];
      if (map && map.getSource("region-draft")) {
        try {
          map.getSource("region-draft").setData({
            type: "FeatureCollection",
            features: [],
          });
        } catch (_) {}
      }
      return true;
    },

    confirmRegionDraft: function (mapId) {
      var draft = this.regionDraft[mapId];
      if (!draft) return false;
      var ring = this.shapeRing(draft.shape, draft.lng, draft.lat, draft.radiusMiles);
      var bbox = this.ringBbox(ring);
      global.__metisRegionConfirm = JSON.stringify({
        shape: draft.shape,
        mode: draft.mode || "new",
        radiusMiles: draft.radiusMiles,
        lng: draft.lng,
        lat: draft.lat,
        west: bbox[0],
        south: bbox[1],
        east: bbox[2],
        north: bbox[3],
      });
      this.clearRegionDraft(mapId);
      return true;
    },

    cancelRegionDraft: function (mapId) {
      if (!this.regionDraft[mapId]) return false;
      global.__metisRegionCancel = "1";
      this.clearRegionDraft(mapId);
      return true;
    },

    teardownDraftDrag: function (mapId) {
      var map = this.maps[mapId];
      var state = this.coverageDrag[mapId];
      if (state && map) {
        try {
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
      };

      function beginDrag(e) {
        var draft = self.regionDraft[mapId];
        if (!draft || !e || !e.lngLat) return;
        state.dragging = true;
        state.startLngLat = e.lngLat;
        state.startCenter = { lng: draft.lng, lat: draft.lat };
        map.dragPan.disable();
        map.getCanvas().style.cursor = "grabbing";
        if (e.originalEvent && e.originalEvent.preventDefault) {
          e.originalEvent.preventDefault();
        }
      }

      state.onDownFill = function (e) {
        beginDrag(e);
      };
      state.onDownLine = function (e) {
        beginDrag(e);
      };
      state.onMove = function (e) {
        if (!state.dragging || !state.startLngLat || !state.startCenter) return;
        var draft = self.regionDraft[mapId];
        if (!draft) return;
        var dLng = e.lngLat.lng - state.startLngLat.lng;
        var dLat = e.lngLat.lat - state.startLngLat.lat;
        draft.lng = state.startCenter.lng + dLng;
        draft.lat = Math.max(-85, Math.min(85, state.startCenter.lat + dLat));
        self.renderRegionDraft(mapId);
      };
      state.onUp = function () {
        if (!state.dragging) return;
        state.dragging = false;
        state.startLngLat = null;
        state.startCenter = null;
        map.dragPan.enable();
        map.getCanvas().style.cursor = "grab";
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

    takeRegionConfirm: function () {
      var raw = global.__metisRegionConfirm;
      global.__metisRegionConfirm = null;
      return raw || "";
    },

    takeRegionCancel: function () {
      var raw = global.__metisRegionCancel;
      global.__metisRegionCancel = null;
      return raw || "";
    },

    takeCoverageMove: function () {
      // Legacy no-op (confirm flow replaced auto-commit on drag).
      return "";
    },
  };

  global.MetisMap = MetisMap;
})(typeof window !== "undefined" ? window : globalThis);
