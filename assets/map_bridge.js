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

  var MetisMap = {
    maps: {},
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
      await new Promise(function (resolve, reject) {
        map.once("load", function () {
          resolve(true);
        });
        map.once("error", function (err) {
          reject(err && err.error ? err.error : err);
        });
      });
      return true;
    },

    destroy: function (mapId) {
      var map = this.maps[mapId];
      if (!map) return;
      try {
        map.remove();
      } catch (_) {}
      delete this.maps[mapId];
    },

    setGeoJson: function (mapId, sourceId, data, layerSpecs) {
      var map = this.maps[mapId];
      if (!map) return false;
      if (map.getSource(sourceId)) {
        map.getSource(sourceId).setData(data);
      } else {
        map.addSource(sourceId, { type: "geojson", data: data });
        (layerSpecs || []).forEach(function (layer) {
          if (!map.getLayer(layer.id)) map.addLayer(layer);
        });
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
      var key = "__metisClick_" + mapId;
      if (map[key]) return;
      map[key] = true;
      map.on("click", layerId, function (e) {
        if (!e.features || !e.features.length) return;
        var props = e.features[0].properties || {};
        global.__metisMapSelection = JSON.stringify(props);
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
  };

  global.MetisMap = MetisMap;
})(typeof window !== "undefined" ? window : globalThis);
