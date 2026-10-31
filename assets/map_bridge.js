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

  function popupHtml(props) {
    var name = escapeHtml(props.name || "Node");
    var typeLabel = escapeHtml(props.type_label || props.type || "Node");
    var enrollment = escapeHtml(props.enrollment || "");
    var bits = ['<div class="metis-map-popup">', '<strong>' + name + "</strong>"];
    bits.push('<div class="metis-map-popup-meta">' + typeLabel);
    if (enrollment) bits.push(" · " + enrollment);
    bits.push("</div></div>");
    return bits.join("");
  }

  function labelHtml(props) {
    var name = escapeHtml(props.name || "Node");
    var typeLabel = escapeHtml(props.type_label || "Node");
    return (
      '<div class="metis-node-label">' +
      '<span class="metis-node-label-name">' +
      name +
      "</span>" +
      '<span class="metis-node-label-type">' +
      typeLabel +
      "</span></div>"
    );
  }

  var MetisMap = {
    maps: {},
    labels: {},
    popups: {},
    nodeData: {},
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
      var container = document.getElementById(containerId);
      if (!container) throw new Error("map container missing: " + containerId);
      var map = new maplibregl.Map({
        container: container,
        style: style,
        center: [lng, lat],
        zoom: zoom,
        attributionControl: true,
      });
      map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");
      this.maps[mapId] = map;
      this.popups[mapId] = new maplibregl.Popup({
        closeButton: false,
        closeOnClick: false,
        offset: 14,
        className: "metis-maplibre-popup",
      });
      await new Promise(function (resolve, reject) {
        map.once("load", function () {
          resolve(true);
        });
        map.once("error", function (err) {
          // Raster/PMTiles errors should not kill an otherwise usable map.
          var msg = err && err.error ? String(err.error) : String(err || "");
          if (/pmtiles|basemap|tile/i.test(msg)) {
            resolve(true);
            return;
          }
          reject(err && err.error ? err.error : err);
        });
      });
      return true;
    },

    destroy: function (mapId) {
      var map = this.maps[mapId];
      if (this.popups[mapId]) {
        try {
          this.popups[mapId].remove();
        } catch (_) {}
        delete this.popups[mapId];
      }
      this.clearLabels(mapId);
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

    syncNodeLabels: function (mapId, featureCollection) {
      var map = this.maps[mapId];
      if (!map || !global.maplibregl) return false;
      this.clearLabels(mapId);
      var features =
        featureCollection && featureCollection.features
          ? featureCollection.features
          : [];
      var zoom = map.getZoom();
      // Dense meshes: only pin labels when zoomed in enough.
      if (zoom < 7 && features.length > 40) {
        return true;
      }
      var maxLabels = zoom < 9 ? 48 : 160;
      var self = this;
      features.slice(0, maxLabels).forEach(function (feature) {
        var coords =
          feature && feature.geometry && feature.geometry.coordinates;
        if (!coords || coords.length < 2) return;
        var el = document.createElement("div");
        el.className = "metis-node-marker";
        el.innerHTML = labelHtml(propsFromFeature(feature));
        el.addEventListener("click", function (ev) {
          ev.stopPropagation();
          var props = propsFromFeature(feature);
          global.__metisMapSelection = JSON.stringify(props);
          self.showPopup(mapId, coords, props);
        });
        el.addEventListener("mouseenter", function () {
          self.showPopup(mapId, coords, propsFromFeature(feature));
        });
        el.addEventListener("mouseleave", function () {
          self.hidePopup(mapId);
        });
        var marker = new global.maplibregl.Marker({
          element: el,
          anchor: "bottom",
          offset: [0, -6],
        })
          .setLngLat(coords)
          .addTo(map);
        self.labels[mapId].push(marker);
      });
      return true;
    },

    showPopup: function (mapId, lngLat, props) {
      var map = this.maps[mapId];
      var popup = this.popups[mapId];
      if (!map || !popup) return;
      popup.setLngLat(lngLat).setHTML(popupHtml(props)).addTo(map);
    },

    hidePopup: function (mapId) {
      var popup = this.popups[mapId];
      if (popup) popup.remove();
    },

    setGeoJson: function (mapId, sourceId, data, layerSpecs) {
      var map = this.maps[mapId];
      if (!map) return false;
      if (map.getSource(sourceId)) {
        map.getSource(sourceId).setData(data);
      } else {
        map.addSource(sourceId, { type: "geojson", data: data });
      }
      (layerSpecs || []).forEach(function (layer) {
        if (!map.getLayer(layer.id)) map.addLayer(layer);
      });
      if (sourceId === "nodes") {
        this.nodeData[mapId] = data;
        this.syncNodeLabels(mapId, data);
      }
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
        if (!e.features || !e.features.length) return;
        var props = e.features[0].properties || {};
        global.__metisMapSelection = JSON.stringify(props);
        self.showPopup(mapId, e.lngLat, props);
      });
      map.on("mouseenter", layerId, function (e) {
        map.getCanvas().style.cursor = "pointer";
        if (!e.features || !e.features.length) return;
        var props = e.features[0].properties || {};
        self.showPopup(mapId, e.lngLat, props);
      });
      map.on("mouseleave", layerId, function () {
        map.getCanvas().style.cursor = "";
        self.hidePopup(mapId);
      });
      map.on("zoomend", function () {
        if (self.nodeData[mapId]) {
          self.syncNodeLabels(mapId, self.nodeData[mapId]);
        }
      });
    },

    takeSelection: function () {
      var raw = global.__metisMapSelection;
      global.__metisMapSelection = null;
      return raw || "";
    },
  };

  global.MetisMap = MetisMap;
})(typeof window !== "undefined" ? window : globalThis);
