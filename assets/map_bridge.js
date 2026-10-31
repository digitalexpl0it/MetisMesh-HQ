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
    var lat = props.lat != null ? Number(props.lat) : null;
    var lon = props.lon != null ? Number(props.lon) : null;
    var coords =
      lat != null && lon != null && isFinite(lat) && isFinite(lon)
        ? lat.toFixed(4) + ", " + lon.toFixed(4)
        : "—";
    var heard = escapeHtml(props.last_heard_label || "unknown");
    var favorited =
      props.favorite === true ||
      props.favorite === "true" ||
      props.favorite === 1 ||
      props.favorite === "1";
    var bits = ['<div class="metis-map-detail">'];
    bits.push('<div class="metis-map-detail-title">' + name + "</div>");
    bits.push('<div class="metis-map-detail-row"><span>Type</span><b>' + typeLabel + "</b></div>");
    if (enrollment) {
      bits.push(
        '<div class="metis-map-detail-row"><span>Status</span><b>' +
          enrollment +
          "</b></div>"
      );
    }
    bits.push(
      '<div class="metis-map-detail-row"><span>Updated</span><b>' + heard + "</b></div>"
    );
    bits.push(
      '<div class="metis-map-detail-row"><span>Coordinates</span><b class="metis-mono">' +
        escapeHtml(coords) +
        "</b></div>"
    );
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
    bits.push('<div class="metis-map-detail-actions">');
    bits.push(
      '<button type="button" class="metis-map-action" data-metis-fav="' +
        escapeHtml(pubkey) +
        '">' +
        (favorited ? "Unfavorite" : "Favorite") +
        "</button>"
    );
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

  function wireDetailActions(root) {
    if (!root) return;
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

    ensureProtocol: function () {
      if (this.protocolReady || !global.maplibregl || !global.pmtiles) return;
      var protocol = new global.pmtiles.Protocol();
      global.maplibregl.addProtocol("pmtiles", protocol.tile);
      this.protocolReady = true;
    },

    init: async function (mapId, containerId, style, lng, lat, zoom) {
      var maplibregl = await waitForMaplibre(8000);
      this.ensureProtocol();
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
      // Keep canvas sized to the stage (avoids tiny black corner canvas).
      this.watchResize(mapId, container);
      await new Promise(function (resolve) {
        var settled = false;
        function done() {
          if (settled) return;
          settled = true;
          try {
            map.resize();
          } catch (_) {}
          resolve(true);
        }
        map.once("load", done);
        // Never fail init on tile/source errors — outline/pmtiles may load async.
        map.on("error", function () {});
        setTimeout(done, 4000);
      });
      try {
        map.resize();
      } catch (_) {}
      setTimeout(function () {
        try {
          map.resize();
        } catch (_) {}
      }, 200);
      try {
        await this.hydrateOutlineSources(mapId);
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
      var map = this.maps[mapId];
      if (this._resizeObs && this._resizeObs[mapId]) {
        try {
          this._resizeObs[mapId].disconnect();
        } catch (_) {}
        delete this._resizeObs[mapId];
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
      global.__metisMapSelection = JSON.stringify(props);
    },

    hideDetail: function (mapId) {
      var popup = this.detailPopups[mapId];
      if (popup) popup.remove();
      this.detailOpen[mapId] = false;
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

    watchResize: function (mapId, container) {
      var map = this.maps[mapId];
      if (!map || !container) return;
      if (this._resizeObs && this._resizeObs[mapId]) {
        try {
          this._resizeObs[mapId].disconnect();
        } catch (_) {}
      }
      if (!this._resizeObs) this._resizeObs = {};
      if (typeof ResizeObserver === "undefined") return;
      var obs = new ResizeObserver(function () {
        try {
          map.resize();
        } catch (_) {}
      });
      obs.observe(container);
      this._resizeObs[mapId] = obs;
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
      if (self.clusterEnabled) delete self.clusterEnabled[mapId];
      if (this.nodeFp) delete this.nodeFp[mapId];
      // Reset click binders so layers can rebind after style.load.
      Object.keys(map).forEach(function (k) {
        if (k.indexOf("__metisClick_") === 0 || k.indexOf("__metisCluster_") === 0) {
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
  };

  global.MetisMap = MetisMap;
})(typeof window !== "undefined" ? window : globalThis);
