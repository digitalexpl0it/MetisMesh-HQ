(function () {
  const authed = document.body.dataset.authed === "1";
  const loginPanel = document.getElementById("login-panel");
  const lockerPanel = document.getElementById("locker-panel");
  const loginForm = document.getElementById("login-form");
  const loginError = document.getElementById("login-error");
  const deviceGrid = document.getElementById("device-grid");
  const emptyEl = document.getElementById("locker-empty");
  const statusEl = document.getElementById("locker-status");
  const filterEl = document.getElementById("filter");
  const makerFilter = document.getElementById("maker-filter");
  const sheetFlash = document.getElementById("sheet-flash");
  const flashLog = document.getElementById("flash-log");
  const flashProgress = document.getElementById("flash-progress");
  const flashProgressBar = document.getElementById("flash-progress-bar");
  const flashProgressPct = document.getElementById("flash-progress-pct");
  const flashProgressLabel = document.getElementById("flash-progress-label");
  const hqName = document.getElementById("hq-name");
  const sessionNote = document.getElementById("session-note");
  const sheet = document.getElementById("device-sheet");
  const sheetBackdrop = document.getElementById("sheet-backdrop");
  const sheetBody = document.getElementById("sheet-body");
  const sheetTitle = document.getElementById("sheet-title");
  const sheetCrumb = document.getElementById("sheet-crumb");

  let catalog = null;
  let mode = "all";
  let openDevice = null;
  let openRole = null;
  let sheetMode = "roles"; // roles | detail | console | setup | help | custom

  let consolePort = null;
  let consoleReader = null;
  let consoleWriter = null;
  let consoleReadLoop = null;
  let consoleAlive = false;
  let consoleBuf = "";

  // Download menu lives on document.body — the sheet uses transform, which
  // makes position:fixed descendants stick to the sheet and get clipped.
  let dlMenuEl = null;
  let dlMenuAnchor = null;

  function ensureDownloadMenu() {
    if (dlMenuEl) return dlMenuEl;
    dlMenuEl = document.createElement("div");
    dlMenuEl.className = "dl-menu-list";
    dlMenuEl.hidden = true;
    dlMenuEl.setAttribute("role", "menu");
    dlMenuEl.addEventListener("click", (e) => e.stopPropagation());
    document.body.appendChild(dlMenuEl);
    return dlMenuEl;
  }

  function closeDownloadMenu() {
    if (!dlMenuEl) return;
    dlMenuEl.hidden = true;
    dlMenuEl.innerHTML = "";
    dlMenuEl.style.left = "";
    dlMenuEl.style.top = "";
    dlMenuEl.style.width = "";
    dlMenuEl.style.maxHeight = "";
    dlMenuAnchor = null;
  }

  function placeDownloadMenuAbove(anchor) {
    const menu = ensureDownloadMenu();
    const gap = 6;
    const pad = 8;
    const rect = anchor.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const spaceAbove = Math.max(64, rect.top - gap - pad);
    menu.style.maxHeight = Math.min(spaceAbove, Math.floor(vh * 0.55)) + "px";
    menu.style.left = pad + "px";
    menu.style.top = pad + "px";
    menu.hidden = false;
    const menuW = Math.min(
      Math.max(menu.scrollWidth, rect.width, 256),
      Math.min(448, vw - pad * 2)
    );
    menu.style.width = menuW + "px";
    const menuH = menu.offsetHeight;
    let left = rect.right - menuW;
    left = Math.max(pad, Math.min(left, vw - menuW - pad));
    let top = rect.top - gap - menuH;
    if (top < pad) top = pad;
    menu.style.left = left + "px";
    menu.style.top = top + "px";
  }

  document.addEventListener("click", closeDownloadMenu);
  window.addEventListener("resize", closeDownloadMenu);

  function showLocker(show) {
    loginPanel.hidden = show;
    lockerPanel.hidden = !show;
  }

  function setStatus(msg) {
    if (!statusEl) return;
    if (!msg) {
      statusEl.hidden = true;
      statusEl.textContent = "";
      return;
    }
    statusEl.hidden = false;
    statusEl.textContent = msg;
  }

  function showSheetFlash(show) {
    if (!sheetFlash) return;
    sheetFlash.hidden = !show;
  }

  function logLine(msg) {
    showSheetFlash(true);
    flashLog.hidden = false;
    flashLog.textContent += msg + "\n";
    flashLog.scrollTop = flashLog.scrollHeight;
  }

  function setFlashProgress(pct, label) {
    showSheetFlash(true);
    const n = Math.max(0, Math.min(100, Math.round(pct)));
    flashProgress.hidden = false;
    flashProgressBar.style.width = n + "%";
    flashProgressPct.textContent = n + "%";
    if (label) flashProgressLabel.textContent = label;
  }

  function clearFlashProgress() {
    flashProgress.hidden = true;
    flashProgressBar.style.width = "0%";
    flashProgressPct.textContent = "0%";
    flashProgressLabel.textContent = "Flashing…";
    flashLog.hidden = true;
    flashLog.textContent = "";
    showSheetFlash(false);
  }

  function roleAbbr(role) {
    const pill = (role && role.pill) || "";
    if (pill) return String(pill).slice(0, 4).toUpperCase();
    const key = ((role && role.role) || "").toLowerCase();
    if (key.includes("ble") || key.includes("bluetooth")) return "BT";
    if (key.includes("usb")) return "USB";
    if (key.includes("repeater")) return "RPT";
    if (key.includes("room")) return "ROOM";
    if (key.includes("kiss")) return "KISS";
    const label = (role && role.label) || "?";
    return label.slice(0, 3).toUpperCase();
  }

  function deviceMakerLabel(device) {
    return device.maker_label || device.maker || "Unknown";
  }

  function roleLabel(role) {
    return (role && (role.label || role.name)) || "Role";
  }

  function roleDescription(role) {
    return (
      (role && (role.tooltip || role.description)) ||
      (role && role.web_serial
        ? "Web Serial flash available on this board"
        : "Download firmware for offline flash tools")
    );
  }

  function versionLabel(v) {
    return (v && (v.version || v.tag)) || "unknown";
  }

  function deviceMatchesMode(device) {
    if (mode === "all") return true;
    if (mode === "web") return !!device.web_serial;
    return !device.web_serial;
  }

  function deviceMatchesFilters(device, q, maker) {
    if (maker && device.maker !== maker && deviceMakerLabel(device) !== maker) return false;
    if (!q) return true;
    const hay = [
      device.maker,
      deviceMakerLabel(device),
      device.name,
      device.platform,
      ...(device.roles || []).map((r) => roleLabel(r) + " " + roleDescription(r)),
    ]
      .join(" ")
      .toLowerCase();
    return hay.includes(q);
  }

  async function disconnectConsole() {
    consoleAlive = false;
    consoleReadLoop = null;
    consoleBuf = "";
    try {
      if (consoleReader) consoleReader.releaseLock();
    } catch (_) {}
    try {
      if (consoleWriter) consoleWriter.releaseLock();
    } catch (_) {}
    consoleReader = null;
    consoleWriter = null;
    if (consolePort) {
      try {
        await consolePort.close();
      } catch (_) {}
    }
    consolePort = null;
  }

  function closeSheet() {
    openDevice = null;
    openRole = null;
    sheetMode = "roles";
    sheet.hidden = true;
    sheetBackdrop.hidden = true;
    sheet.setAttribute("aria-hidden", "true");
    sheetBody.innerHTML = "";
    closeDownloadMenu();
    clearFlashProgress();
    disconnectConsole();
  }

  function openSheet() {
    sheet.hidden = false;
    sheetBackdrop.hidden = false;
    sheet.setAttribute("aria-hidden", "false");
  }

  function renderRoleList(device) {
    closeDownloadMenu();
    sheetMode = "roles";
    showSheetFlash(false);
    sheetCrumb.textContent = deviceMakerLabel(device) + " · " + device.name;
    sheetTitle.textContent = "Choose role";
    const list = document.createElement("div");
    list.className = "role-list";
    (device.roles || []).forEach((role) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "role-row";
      btn.innerHTML =
        '<span class="role-icon"></span><span class="role-copy"><strong></strong><span></span></span>';
      btn.querySelector(".role-icon").textContent = roleAbbr(role);
      btn.querySelector("strong").textContent = roleLabel(role);
      btn.querySelector(".role-copy > span").textContent = roleDescription(role);
      btn.addEventListener("click", () => {
        openRole = role;
        renderRoleDetail(device, role);
      });
      list.appendChild(btn);
    });
    sheetBody.innerHTML = "";
    sheetBody.appendChild(list);
  }

  function downloadAsset(asset) {
    if (!asset || !asset.id) {
      logLine("No file to download.");
      return;
    }
    window.location.href = "/api/firmware/" + encodeURIComponent(asset.id);
  }

  function renderRoleDetail(device, role) {
    closeDownloadMenu();
    sheetMode = "detail";
    sheetCrumb.textContent =
      deviceMakerLabel(device) + " · " + device.name + " › " + roleLabel(role);
    sheetTitle.textContent = roleLabel(role);
    const wrap = document.createElement("div");
    wrap.className = "role-detail";

    const desc = document.createElement("p");
    desc.className = "desc";
    desc.textContent = roleDescription(role);
    wrap.appendChild(desc);

    const noticeKey = role.notice_key || "";
    if (noticeKey === "otafixNeeded" || noticeKey === "otafixRecommended") {
      const banner = document.createElement("div");
      banner.className =
        "notice-banner" + (noticeKey === "otafixRecommended" ? " recommend" : "");
      const text = document.createElement("p");
      text.style.margin = "0";
      text.textContent =
        role.notice_text ||
        "Install the OTAFIX bootloader (UF2 drag-drop) before relying on Bluetooth OTA.";
      banner.appendChild(text);
      const steps = document.createElement("ol");
      steps.className = "steps";
      [
        "Put the board in UF2 DFU mode (usually double-tap reset).",
        "Download the OTAFIX .uf2 and copy it onto the USB drive that appears.",
        "Board reboots; then flash MeshCore firmware as usual.",
      ].forEach((line) => {
        const li = document.createElement("li");
        li.textContent = line;
        steps.appendChild(li);
      });
      banner.appendChild(steps);
      const links = document.createElement("div");
      links.className = "actions";
      links.style.marginTop = "0.65rem";
      const boot =
        role.bootloader_asset ||
        (device.bootloader_assets || []).find((a) =>
          String(a.name || "").toLowerCase().endsWith(".uf2")
        ) ||
        (device.bootloader_assets || [])[0];
      if (boot) {
        const dl = document.createElement("button");
        dl.type = "button";
        dl.className = "btn primary";
        dl.textContent = "Download OTAFIX .uf2";
        dl.addEventListener("click", () => downloadAsset(boot));
        links.appendChild(dl);
      }
      if (device.erase_asset) {
        const eraseDl = document.createElement("button");
        eraseDl.type = "button";
        eraseDl.className = "btn";
        eraseDl.textContent = "Download erase image";
        eraseDl.addEventListener("click", () => downloadAsset(device.erase_asset));
        links.appendChild(eraseDl);
      }
      const more = document.createElement("a");
      more.className = "btn ghost";
      more.href = role.otafix_blog || "https://blog.meshcore.io/2026/04/06/otafix-bootloader";
      more.target = "_blank";
      more.rel = "noopener noreferrer";
      more.textContent = "Read more";
      links.appendChild(more);
      banner.appendChild(links);
      wrap.appendChild(banner);
    }

    const versions = role.versions || [];
    const verField = document.createElement("label");
    verField.className = "field";
    verField.innerHTML = "<span>Version</span>";
    const verSelect = document.createElement("select");
    versions.forEach((v, i) => {
      const opt = document.createElement("option");
      opt.value = String(i);
      opt.textContent = versionLabel(v);
      verSelect.appendChild(opt);
    });
    verField.appendChild(verSelect);
    wrap.appendChild(verField);

    const assetHint = document.createElement("p");
    assetHint.className = "asset-pick";
    wrap.appendChild(assetHint);

    function currentVersion() {
      const i = Number(verSelect.value) || 0;
      return versions[i] || null;
    }

    function versionHasWeb(v) {
      return (v.assets || []).some((a) => a.web_serial);
    }

    function refreshAssetHint() {
      const v = currentVersion();
      if (!v) {
        assetHint.textContent = "No build for this role yet.";
        return;
      }
      const names = (v.assets || []).map((a) => a.name).join(", ");
      assetHint.textContent =
        (v.assets || []).length +
        " file(s)" +
        (names ? ": " + names : "") +
        (versionHasWeb(v) ? " · Web Serial ready" : " · download only");
    }
    verSelect.addEventListener("change", refreshAssetHint);
    refreshAssetHint();

    const erase = document.createElement("label");
    erase.className = "check";
    erase.innerHTML =
      '<input type="checkbox" id="erase-flash" /> Erase flash before write (Web Serial)';
    wrap.appendChild(erase);

    const actions = document.createElement("div");
    actions.className = "actions";

    const canFlash = !!role.web_serial && "serial" in navigator;
    const flashBtn = document.createElement("button");
    flashBtn.type = "button";
    flashBtn.className = "btn primary";
    flashBtn.textContent = "Flash!";
    flashBtn.disabled = !canFlash;
    flashBtn.title = role.web_serial
      ? "serial" in navigator
        ? "Flash over Web Serial"
        : "Use Chrome or Edge for Web Serial"
      : "This role is download-only on this board";
    flashBtn.addEventListener("click", async () => {
      const v = currentVersion();
      if (!v) return;
      const asset = pickFlashAsset(v);
      if (!asset) {
        logLine("No asset to flash.");
        return;
      }
      const eraseFirst = erase.querySelector("input").checked;
      const go = await maybeBackupBeforeFlash(eraseFirst, role);
      if (!go) return;
      await flashAsset(asset, eraseFirst);
      await maybeRestoreAfterFlash();
    });
    actions.appendChild(flashBtn);

    const isNrf =
      String(device.platform || "").toLowerCase().includes("nrf") ||
      (role.versions || []).some((v) =>
        (v.assets || []).some(
          (a) =>
            a.flash_kind === "nrf_dfu" ||
            /\.zip$/i.test(a.name || "") ||
            /\.uf2$/i.test(a.name || "")
        )
      );
    if (isNrf && "serial" in navigator) {
      const dfuBtn = document.createElement("button");
      dfuBtn.type = "button";
      dfuBtn.className = "btn";
      dfuBtn.textContent = "Enter DFU mode";
      dfuBtn.title =
        "1200-baud reset into the bootloader (UF2 drive / serial DFU). Pick the app serial port.";
      dfuBtn.addEventListener("click", async () => {
        if (!window.NrfDfu) {
          logLine("nRF DFU library failed to load. Hard-refresh the locker page.");
          return;
        }
        await disconnectConsole();
        showSheetFlash(true);
        flashLog.hidden = false;
        if (!flashLog.textContent) flashLog.textContent = "";
        setFlashProgress(5, "Select serial port…");
        logLine("Enter DFU mode: select the board serial port…");
        let port;
        try {
          port = await navigator.serial.requestPort();
        } catch (err) {
          logLine("Port select failed: " + err);
          setFlashProgress(0, "Port open failed");
          return;
        }
        try {
          setFlashProgress(40, "Sending 1200-baud DFU touch…");
          logLine("Sending 1200-baud touch…");
          await window.NrfDfu.enterDfuMode(port);
          setFlashProgress(100, "DFU mode");
          logLine(
            "DFU mode requested. If a UF2 drive appeared, copy a .uf2 onto it. For Web Serial flash, click Flash! and pick the DFU port when prompted."
          );
        } catch (err) {
          logLine("Enter DFU failed: " + err);
          setFlashProgress(0, "DFU mode failed");
          try {
            await port.close();
          } catch (_) {}
        }
      });
      actions.appendChild(dfuBtn);
    }

    const dlWrap = document.createElement("div");
    dlWrap.className = "dl-menu";
    const dlBtn = document.createElement("button");
    dlBtn.type = "button";
    dlBtn.className = "btn";
    dlBtn.textContent = "Download ▴";
    dlBtn.title = "Firmware, erase, and OTAFIX files";

    function downloadAssetCount() {
      const v = currentVersion();
      const firm = ((v && v.assets) || []).filter((a) => a && a.id && a.name)
        .length;
      const erase = (
        device.erase_assets && device.erase_assets.length
          ? device.erase_assets
          : device.erase_asset
            ? [device.erase_asset]
            : []
      ).filter((a) => a && a.id && a.name).length;
      const boot = (device.bootloader_assets || []).filter(
        (a) => a && a.id && a.name
      ).length;
      return firm + erase + boot;
    }

    function fillDownloadMenu(menu) {
      menu.innerHTML = "";
      function addDlGroup(label, assets) {
        const list = (assets || []).filter((a) => a && a.id && a.name);
        if (!list.length) return 0;
        const lab = document.createElement("span");
        lab.className = "dl-menu-label";
        lab.textContent = label;
        menu.appendChild(lab);
        list.forEach((asset) => {
          const item = document.createElement("button");
          item.type = "button";
          item.setAttribute("role", "menuitem");
          item.textContent = asset.name;
          item.title = asset.name;
          item.addEventListener("click", () => {
            closeDownloadMenu();
            downloadAsset(asset);
          });
          menu.appendChild(item);
        });
        return list.length;
      }
      const v = currentVersion();
      let n = 0;
      n += addDlGroup("Firmware", (v && v.assets) || []);
      n += addDlGroup(
        "Erase",
        device.erase_assets && device.erase_assets.length
          ? device.erase_assets
          : device.erase_asset
            ? [device.erase_asset]
            : []
      );
      n += addDlGroup("OTAFIX bootloader", device.bootloader_assets || []);
      if (!n) {
        const empty = document.createElement("button");
        empty.type = "button";
        empty.disabled = true;
        empty.textContent = "No files on this HQ yet";
        menu.appendChild(empty);
      }
      return n;
    }

    dlBtn.disabled = downloadAssetCount() === 0;
    verSelect.addEventListener("change", () => {
      closeDownloadMenu();
      dlBtn.disabled = downloadAssetCount() === 0;
    });

    dlBtn.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (dlMenuAnchor === dlBtn && dlMenuEl && !dlMenuEl.hidden) {
        closeDownloadMenu();
        return;
      }
      const menu = ensureDownloadMenu();
      fillDownloadMenu(menu);
      dlMenuAnchor = dlBtn;
      placeDownloadMenuAbove(dlBtn);
    });

    dlWrap.appendChild(dlBtn);
    actions.appendChild(dlWrap);

    const consoleBtn = document.createElement("button");
    consoleBtn.type = "button";
    consoleBtn.className = "btn";
    consoleBtn.textContent = "Console";
    consoleBtn.disabled = !("serial" in navigator);
    consoleBtn.title = "Open Web Serial CLI to the USB node";
    consoleBtn.addEventListener("click", () => renderConsole(device));
    actions.appendChild(consoleBtn);

    const roleKey = (role.role || "").toLowerCase();
    if (roleKey === "repeater" || roleKey === "roomserver") {
      const setupBtn = document.createElement("button");
      setupBtn.type = "button";
      setupBtn.className = "btn";
      setupBtn.textContent =
        roleKey === "roomserver" ? "Setup room server" : "Setup repeater";
      setupBtn.disabled = !("serial" in navigator);
      setupBtn.title = "Auto-configure over USB Serial CLI after flash";
      setupBtn.addEventListener("click", () => renderSetupWizard(device, role));
      actions.appendChild(setupBtn);
    }

    const backRoles = document.createElement("button");
    backRoles.type = "button";
    backRoles.className = "btn ghost";
    backRoles.textContent = "Other roles";
    backRoles.addEventListener("click", () => {
      openRole = null;
      clearFlashProgress();
      renderRoleList(device);
    });
    actions.appendChild(backRoles);

    wrap.appendChild(actions);
    sheetBody.innerHTML = "";
    sheetBody.appendChild(wrap);
  }

  function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  async function ensureConsoleConnected(statusEl, outEl) {
    if (consolePort && consoleWriter && consoleReader) {
      return true;
    }
    if (!("serial" in navigator)) {
      if (outEl) outEl.textContent += "Web Serial is not available in this browser.\n";
      return false;
    }
    try {
      const port = await navigator.serial.requestPort();
      await port.open({ baudRate: 115200 });
      consolePort = port;
      consoleWriter = port.writable.getWriter();
      consoleReader = port.readable.getReader();
      consoleBuf = "";
      consoleAlive = true;
      if (statusEl) {
        statusEl.textContent = "Connected · 115200";
        statusEl.classList.add("live");
      }
      if (outEl) outEl.textContent += "[connected]\n";
      // Background read into buffer + optional outEl
      (async function readLoop() {
        const decoder = new TextDecoder();
        while (consoleAlive && consoleReader) {
          try {
            const { value, done } = await consoleReader.read();
            if (done) break;
            if (value && value.length) {
              const chunk = decoder.decode(value, { stream: true });
              consoleBuf += chunk;
              if (consoleBuf.length > 20000) {
                consoleBuf = consoleBuf.slice(-12000);
              }
              if (outEl) {
                outEl.textContent += chunk;
                outEl.scrollTop = outEl.scrollHeight;
              }
            }
          } catch (_) {
            break;
          }
        }
      })();
      return true;
    } catch (err) {
      if (outEl) outEl.textContent += "Connect failed: " + err + "\n";
      await disconnectConsole();
      return false;
    }
  }

  async function consoleSendRaw(line, hideInLog, outEl) {
    if (!consoleWriter) throw new Error("Not connected");
    await consoleWriter.write(new TextEncoder().encode(line + "\n"));
    if (outEl) {
      outEl.textContent += hideInLog ? "[sent]\n" : "> " + line + "\n";
      outEl.scrollTop = outEl.scrollHeight;
    }
  }

  async function waitForConsoleIdle(ms) {
    const startLen = consoleBuf.length;
    const deadline = Date.now() + ms;
    let last = startLen;
    let stableSince = Date.now();
    while (Date.now() < deadline) {
      await sleep(120);
      if (consoleBuf.length !== last) {
        last = consoleBuf.length;
        stableSince = Date.now();
      } else if (Date.now() - stableSince > 350 && consoleBuf.length > startLen) {
        return true;
      }
    }
    return consoleBuf.length > startLen;
  }

  function renderSetupWizard(device, role) {
    closeDownloadMenu();
    sheetMode = "setup";
    showSheetFlash(false);
    clearFlashProgress();
    openSheet();
    const isRoom = (role.role || "").toLowerCase() === "roomserver";
    sheetCrumb.textContent = deviceMakerLabel(device) + " · " + device.name;
    sheetTitle.textContent = isRoom ? "Setup room server" : "Setup repeater";

    const wrap = document.createElement("div");
    wrap.className = "wizard-panel";

    const hint = document.createElement("p");
    hint.className = "console-hint";
    hint.textContent =
      "Flash the " +
      (isRoom ? "room server" : "repeater") +
      " firmware first, plug USB, then run this wizard. Factory admin password is usually “password”.";
    wrap.appendChild(hint);

    function field(label, id, opts) {
      const el = document.createElement("label");
      el.className = "field";
      el.innerHTML = "<span></span>";
      el.querySelector("span").textContent = label;
      const input = document.createElement("input");
      input.id = id;
      input.type = (opts && opts.type) || "text";
      if (opts && opts.placeholder) input.placeholder = opts.placeholder;
      if (opts && opts.value != null) input.value = opts.value;
      if (opts && opts.autocomplete) input.autocomplete = opts.autocomplete;
      el.appendChild(input);
      wrap.appendChild(el);
      return input;
    }

    const nameIn = field("Node name", "wiz-name", {
      placeholder: device.name.slice(0, 16),
    });
    const passIn = field("Admin password", "wiz-pass", {
      type: "password",
      placeholder: "new admin password",
      autocomplete: "new-password",
    });
    let guestIn = null;
    if (isRoom) {
      guestIn = field("Guest password", "wiz-guest", {
        type: "password",
        placeholder: "default is often hello",
        autocomplete: "new-password",
      });
    }
    const latIn = field("Latitude (optional)", "wiz-lat", {
      type: "text",
      placeholder: "e.g. 34.42",
    });
    const lonIn = field("Longitude (optional)", "wiz-lon", {
      type: "text",
      placeholder: "e.g. -117.28",
    });
    const advertIn = field("Advert interval minutes (optional)", "wiz-advert", {
      type: "number",
      placeholder: "e.g. 60",
    });

    let repeatCheck = null;
    if (isRoom) {
      const lab = document.createElement("label");
      lab.className = "check";
      lab.innerHTML =
        '<input type="checkbox" id="wiz-repeat" /> Also act as repeater (set repeat on)';
      wrap.appendChild(lab);
      repeatCheck = lab.querySelector("input");
    }

    const status = document.createElement("p");
    status.className = "wizard-status";
    status.textContent = "Not started";
    wrap.appendChild(status);

    const log = document.createElement("pre");
    log.className = "wizard-log";
    wrap.appendChild(log);

    const actions = document.createElement("div");
    actions.className = "actions";

    const runBtn = document.createElement("button");
    runBtn.type = "button";
    runBtn.className = "btn primary";
    runBtn.textContent = "Run setup";
    runBtn.disabled = !("serial" in navigator);

    const consoleBtn = document.createElement("button");
    consoleBtn.type = "button";
    consoleBtn.className = "btn";
    consoleBtn.textContent = "Open Console";
    consoleBtn.addEventListener("click", () => renderConsole(device));

    const backBtn = document.createElement("button");
    backBtn.type = "button";
    backBtn.className = "btn ghost";
    backBtn.textContent = "Back";
    backBtn.addEventListener("click", () => {
      renderRoleDetail(device, role);
    });

    actions.appendChild(runBtn);
    actions.appendChild(consoleBtn);
    actions.appendChild(backBtn);
    wrap.appendChild(actions);

    async function runSetup() {
      const nodeName = (nameIn.value || "").trim();
      const adminPass = passIn.value || "";
      if (!nodeName) {
        status.className = "wizard-status err";
        status.textContent = "Node name is required.";
        return;
      }
      if (!adminPass) {
        status.className = "wizard-status err";
        status.textContent = "Admin password is required.";
        return;
      }
      if (isRoom && guestIn && !(guestIn.value || "").trim()) {
        status.className = "wizard-status err";
        status.textContent = "Guest password is required for room servers.";
        return;
      }

      runBtn.disabled = true;
      status.className = "wizard-status";
      status.textContent = "Connecting…";
      log.textContent = "";
      await disconnectConsole();
      const ok = await ensureConsoleConnected(null, log);
      if (!ok) {
        status.className = "wizard-status err";
        status.textContent = "Could not open serial port.";
        runBtn.disabled = false;
        return;
      }

      const steps = [];
      steps.push({
        label: "Set admin password",
        line: "password " + adminPass,
        hide: true,
        okLog: "[password set]",
      });
      steps.push({ label: "Set name", line: "set name " + nodeName });
      if (isRoom && guestIn) {
        steps.push({
          label: "Set guest password",
          line: "set guest.password " + guestIn.value.trim(),
          hide: true,
          okLog: "[guest password set]",
        });
      }
      const lat = (latIn.value || "").trim();
      const lon = (lonIn.value || "").trim();
      if (lat) steps.push({ label: "Set lat", line: "set lat " + lat });
      if (lon) steps.push({ label: "Set lon", line: "set lon " + lon });
      const advert = (advertIn.value || "").trim();
      if (advert) {
        steps.push({
          label: "Set advert interval",
          line: "set advert.interval " + advert,
        });
      }
      if (isRoom && repeatCheck && repeatCheck.checked) {
        steps.push({ label: "Enable repeat", line: "set repeat on" });
      }
      steps.push({ label: "Send advert", line: "advert" });
      steps.push({
        label: "Reboot",
        line: "reboot",
        allowTimeout: true,
      });

      try {
        for (let i = 0; i < steps.length; i++) {
          const step = steps[i];
          status.textContent = "Step " + (i + 1) + "/" + steps.length + ": " + step.label;
          consoleBuf = "";
          await consoleSendRaw(step.line, !!step.hide, null);
          if (step.okLog) {
            log.textContent += step.okLog + "\n";
          } else if (!step.hide) {
            log.textContent += "> " + step.line + "\n";
          }
          log.scrollTop = log.scrollHeight;
          if (step.allowTimeout) {
            await sleep(800);
            log.textContent += "[reboot sent — timeout is normal]\n";
          } else {
            await waitForConsoleIdle(2500);
            await sleep(200);
          }
        }
        status.className = "wizard-status ok";
        status.textContent = "Setup complete. Claim the node under Infra on the HQ desktop when ready.";
        log.textContent += "\nDone.\n";
      } catch (err) {
        status.className = "wizard-status err";
        status.textContent = "Failed: " + err;
        log.textContent += "\n[error] " + err + "\n";
      } finally {
        runBtn.disabled = false;
        runBtn.textContent = "Retry";
      }
    }

    runBtn.addEventListener("click", runSetup);
    sheetBody.innerHTML = "";
    sheetBody.appendChild(wrap);
  }

  function consoleAppend(outEl, text) {
    outEl.textContent += text;
    outEl.scrollTop = outEl.scrollHeight;
  }

  function renderCustomFirmware() {
    closeDownloadMenu();
    sheetMode = "custom";
    openDevice = null;
    openRole = null;
    showSheetFlash(false);
    clearFlashProgress();
    openSheet();
    sheetCrumb.textContent = "Firmware locker";
    sheetTitle.textContent = "Custom firmware";

    const wrap = document.createElement("div");
    wrap.className = "wizard-panel";

    const hint = document.createElement("p");
    hint.className = "console-hint";
    hint.textContent =
      "Flash a file from this computer — not from the HQ catalog. Use .bin for ESP32 (SLIP) or a MeshCore/Adafruit DFU .zip for nRF52. Verify the file yourself; HQ does not checksum custom uploads.";
    wrap.appendChild(hint);

    const fileField = document.createElement("label");
    fileField.className = "field";
    fileField.innerHTML = "<span>Firmware file</span>";
    const fileInput = document.createElement("input");
    fileInput.type = "file";
    fileInput.accept = ".bin,.zip,application/zip,application/octet-stream";
    fileField.appendChild(fileInput);
    wrap.appendChild(fileField);

    const fileMeta = document.createElement("p");
    fileMeta.className = "asset-pick";
    fileMeta.textContent = "No file selected.";
    wrap.appendChild(fileMeta);

    const erase = document.createElement("label");
    erase.className = "check";
    erase.innerHTML =
      '<input type="checkbox" id="custom-erase" /> Erase flash before write (ESP .bin only)';
    wrap.appendChild(erase);

    const status = document.createElement("p");
    status.className = "wizard-status";
    status.textContent = "Ready";
    wrap.appendChild(status);

    const actions = document.createElement("div");
    actions.className = "actions";
    const flashBtn = document.createElement("button");
    flashBtn.type = "button";
    flashBtn.className = "btn primary";
    flashBtn.textContent = "Flash!";
    flashBtn.disabled = !("serial" in navigator);
    const closeBtn = document.createElement("button");
    closeBtn.type = "button";
    closeBtn.className = "btn ghost";
    closeBtn.textContent = "Close";
    closeBtn.addEventListener("click", closeSheet);
    actions.appendChild(flashBtn);
    actions.appendChild(closeBtn);
    wrap.appendChild(actions);

    let picked = null; // { name, buffer, kind }

    fileInput.addEventListener("change", async () => {
      picked = null;
      const file = fileInput.files && fileInput.files[0];
      if (!file) {
        fileMeta.textContent = "No file selected.";
        status.textContent = "Ready";
        status.className = "wizard-status";
        return;
      }
      const name = file.name || "firmware.bin";
      const lower = name.toLowerCase();
      if (!lower.endsWith(".bin") && !lower.endsWith(".zip")) {
        fileMeta.textContent = "Choose a .bin (ESP) or .zip (nRF DFU) file.";
        status.className = "wizard-status err";
        status.textContent = "Unsupported file type.";
        return;
      }
      try {
        const buffer = await file.arrayBuffer();
        const kind = lower.endsWith(".zip") ? "nrf_dfu" : "esp_slip";
        picked = { name: name, buffer: buffer, kind: kind };
        fileMeta.textContent =
          name +
          " · " +
          buffer.byteLength +
          " bytes · " +
          (kind === "nrf_dfu" ? "nRF DFU zip" : "ESP binary");
        status.className = "wizard-status";
        status.textContent = "File loaded — connect USB and Flash!";
      } catch (err) {
        status.className = "wizard-status err";
        status.textContent = "Could not read file: " + err;
      }
    });

    flashBtn.addEventListener("click", async () => {
      if (!picked) {
        status.className = "wizard-status err";
        status.textContent = "Select a .bin or .zip first.";
        return;
      }
      if (!("serial" in navigator)) {
        status.className = "wizard-status err";
        status.textContent = "Web Serial requires Chrome or Edge.";
        return;
      }
      status.className = "wizard-status";
      status.textContent = "Flashing…";
      flashBtn.disabled = true;
      try {
        await flashLocalBuffer(
          picked.name,
          picked.buffer,
          picked.kind,
          erase.querySelector("input").checked
        );
        status.className = "wizard-status ok";
        status.textContent = "Flash finished (see log below).";
      } catch (err) {
        status.className = "wizard-status err";
        status.textContent = "Flash failed: " + err;
      } finally {
        flashBtn.disabled = false;
      }
    });

    sheetBody.innerHTML = "";
    sheetBody.appendChild(wrap);
  }

  function renderHelpGuide() {
    closeDownloadMenu();
    sheetMode = "help";
    openDevice = null;
    openRole = null;
    showSheetFlash(false);
    clearFlashProgress();
    openSheet();
    sheetCrumb.textContent = "Firmware locker";
    sheetTitle.textContent = "Help";
    const wrap = document.createElement("div");
    wrap.className = "help-guide";
    wrap.innerHTML =
      '<div class="help-block">' +
      "<h3>Before you start</h3>" +
      "<ul>" +
      "<li>Use <strong>Chrome</strong> or <strong>Edge</strong> on a laptop (Web Serial).</li>" +
      "<li>Accept the self-signed HTTPS warning for this HQ, then unlock with the room <strong>PIN</strong>.</li>" +
      "<li>Use a <strong>data</strong> USB cable. Disconnect the phone app from the radio first.</li>" +
      "</ul>" +
      "</div>" +
      '<div class="help-block">' +
      "<h3>Find a board</h3>" +
      "<ol>" +
      "<li>Filter with <strong>All boards</strong>, <strong>Web Serial</strong>, or <strong>Download only</strong>.</li>" +
      "<li>Search or pick a maker, then tap the board card.</li>" +
      "<li>Choose a role (Companion Bluetooth/USB, Repeater, Room Server, …).</li>" +
      "<li>Pick a version, then <strong>Flash!</strong> or open <strong>Download ▾</strong> for firmware, erase, and OTAFIX files.</li>" +
      "</ol>" +
      "</div>" +
      '<div class="help-block">' +
      "<h3>Flash over Web Serial</h3>" +
      "<p><strong>ESP32</strong> boards: one port picker, then watch the progress bar in this sheet.</p>" +
      "<p><strong>nRF52</strong> boards (Wio Tracker L1, SenseCAP, RAK, …): Flash! uses the DFU <code>.zip</code>. The browser asks for the serial port <strong>twice</strong> — once for the app, again after the board reboots into DFU.</p>" +
      "<ul>" +
      "<li><strong>Enter DFU mode</strong> — optional 1200-baud reset into the bootloader (UF2 drive or serial DFU) without starting a full flash.</li>" +
      "<li>Leave erase unchecked for a normal update (full erase wipes node identity).</li>" +
      "<li>Before Flash!, HQ can prompt to back up identity over USB into an encrypted <code>.metisnode</code> kit; after flash you can restore it.</li>" +
      "<li>If sync fails, hold BOOT / double-tap RESET and try again.</li>" +
      "</ul>" +
      "</div>" +
      '<div class="help-block">' +
      "<h3>Node identity kits</h3>" +
      "<p>USB-only private key backup (not LoRa). Kits live on HQ under <code>node-kits/</code>, separate from HQ <code>.metisbak</code>. Passphrase-encrypts <code>prv.key</code>; never paste keys into chat.</p>" +
      "</div>" +
      '<div class="help-block">' +
      "<h3>OTAFIX bootloader (nRF)</h3>" +
      "<p>Some repeater/room boards need OTAFIX before Bluetooth OTA works. When the role shows a warning:</p>" +
      "<ol>" +
      "<li>Use <strong>Enter DFU mode</strong> or double-tap reset until a USB drive appears.</li>" +
      "<li><strong>Download ▾</strong> → OTAFIX <code>.uf2</code> and copy it onto that drive.</li>" +
      "<li>Then flash MeshCore firmware as usual.</li>" +
      "</ol>" +
      '<p><a href="https://blog.meshcore.io/2026/04/06/otafix-bootloader" target="_blank" rel="noopener noreferrer">MeshCore OTAFIX guide</a></p>' +
      "</div>" +
      '<div class="help-block">' +
      "<h3>Download only / UF2</h3>" +
      "<p>If Flash! is unavailable, download the <code>.uf2</code> (or <code>.bin</code>) and use drag-and-drop DFU or your usual tool. Files on this locker were synced and checksum-checked on HQ.</p>" +
      "</div>" +
      '<div class="help-block">' +
      "<h3>Custom firmware</h3>" +
      "<p>Toolbar <strong>Custom</strong> flashes a file from this laptop (not the HQ catalog): <code>.bin</code> for ESP32, DFU <code>.zip</code> for nRF52. You are responsible for trusting that file — HQ does not verify custom uploads.</p>" +
      "</div>" +
      '<div class="help-block">' +
      "<h3>Console &amp; setup</h3>" +
      "<ul>" +
      "<li><strong>Console</strong> — USB serial CLI at 115200 after the board is running firmware.</li>" +
      "<li><strong>Setup repeater / Setup room server</strong> — guided CLI (name, passwords, optional map coords). Do this after a successful flash.</li>" +
      "<li>Claiming the node for HQ Infra over LoRa stays on the desktop app.</li>" +
      "</ul>" +
      "</div>" +
      '<div class="help-block">' +
      "<h3>Empty catalog?</h3>" +
      "<p>Ask the HQ operator to open Settings → Flasher, Sync while online, and select boards under Manage. This page only serves trusted builds already on the HQ.</p>" +
      "</div>";
    sheetBody.innerHTML = "";
    sheetBody.appendChild(wrap);
  }

  function renderConsole(fromDevice) {
    closeDownloadMenu();
    sheetMode = "console";
    showSheetFlash(false);
    clearFlashProgress();
    openSheet();
    if (fromDevice) {
      sheetCrumb.textContent = deviceMakerLabel(fromDevice) + " · " + fromDevice.name;
    } else {
      sheetCrumb.textContent = "Web Serial";
    }
    sheetTitle.textContent = "Console";

    const wrap = document.createElement("div");
    wrap.className = "console-panel";

    const hint = document.createElement("p");
    hint.className = "console-hint";
    hint.textContent =
      "Connect over USB Serial to run node CLI commands (115200 baud). Close any other app using the port first.";
    wrap.appendChild(hint);

    const status = document.createElement("p");
    status.className = "console-status";
    status.id = "console-status";
    status.textContent = consolePort ? "Connected" : "Disconnected";
    if (consolePort) status.classList.add("live");
    wrap.appendChild(status);

    const out = document.createElement("pre");
    out.className = "console-out";
    out.id = "console-out";
    out.textContent = "";
    wrap.appendChild(out);

    const row = document.createElement("div");
    row.className = "console-row";
    const input = document.createElement("input");
    input.type = "text";
    input.placeholder = "CLI command…";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.disabled = !consolePort;
    const sendBtn = document.createElement("button");
    sendBtn.type = "button";
    sendBtn.className = "btn primary";
    sendBtn.textContent = "Send";
    sendBtn.disabled = !consolePort;
    row.appendChild(input);
    row.appendChild(sendBtn);
    wrap.appendChild(row);

    const actions = document.createElement("div");
    actions.className = "actions";
    const connectBtn = document.createElement("button");
    connectBtn.type = "button";
    connectBtn.className = "btn";
    connectBtn.textContent = consolePort ? "Disconnect" : "Connect";
    connectBtn.disabled = !("serial" in navigator);
    actions.appendChild(connectBtn);

    const clearBtn = document.createElement("button");
    clearBtn.type = "button";
    clearBtn.className = "btn ghost";
    clearBtn.textContent = "Clear";
    clearBtn.addEventListener("click", () => {
      out.textContent = "";
    });
    actions.appendChild(clearBtn);
    wrap.appendChild(actions);

    async function startReadLoop() {
      consoleAlive = true;
      const decoder = new TextDecoder();
      while (consoleAlive && consoleReader) {
        try {
          const { value, done } = await consoleReader.read();
          if (done) break;
          if (value && value.length) {
            consoleAppend(out, decoder.decode(value, { stream: true }));
          }
        } catch (_) {
          break;
        }
      }
    }

    async function sendLine() {
      const line = input.value;
      if (!consoleWriter) return;
      try {
        await consoleWriter.write(new TextEncoder().encode(line + "\n"));
        consoleAppend(out, line + "\n");
        input.value = "";
      } catch (err) {
        consoleAppend(out, "\n[send failed: " + err + "]\n");
      }
    }

    sendBtn.addEventListener("click", sendLine);
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        sendLine();
      }
    });

    connectBtn.addEventListener("click", async () => {
      if (consolePort) {
        await disconnectConsole();
        status.textContent = "Disconnected";
        status.classList.remove("live");
        connectBtn.textContent = "Connect";
        input.disabled = true;
        sendBtn.disabled = true;
        consoleAppend(out, "\n[disconnected]\n");
        return;
      }
      if (!("serial" in navigator)) {
        consoleAppend(out, "Web Serial is not available in this browser.\n");
        return;
      }
      try {
        const port = await navigator.serial.requestPort();
        await port.open({ baudRate: 115200 });
        consolePort = port;
        consoleWriter = port.writable.getWriter();
        consoleReader = port.readable.getReader();
        status.textContent = "Connected · 115200";
        status.classList.add("live");
        connectBtn.textContent = "Disconnect";
        input.disabled = false;
        sendBtn.disabled = false;
        input.focus();
        consoleAppend(out, "[connected]\n");
        consoleReadLoop = startReadLoop();
      } catch (err) {
        consoleAppend(out, "Connect failed: " + err + "\n");
        await disconnectConsole();
      }
    });

    sheetBody.innerHTML = "";
    sheetBody.appendChild(wrap);
  }

  function renderDevices() {
    if (!catalog) return;
    const q = (filterEl.value || "").trim().toLowerCase();
    const maker = makerFilter.value || "";
    const devices = (catalog.devices || []).filter(
      (d) => deviceMatchesMode(d) && deviceMatchesFilters(d, q, maker)
    );

    deviceGrid.innerHTML = "";
    if ((catalog.devices || []).length === 0) {
      emptyEl.hidden = false;
      const count = catalog.count || 0;
      emptyEl.textContent = count
        ? "Firmware files are on disk (" +
          count +
          ") but no boards could be listed. Sync the locker on the desktop Flasher tab."
        : "No firmware on this HQ yet. Sync the locker on the desktop Flasher tab.";
      return;
    }
    if (devices.length === 0) {
      emptyEl.hidden = false;
      emptyEl.textContent = "No boards match these filters.";
      return;
    }
    emptyEl.hidden = true;

    devices.forEach((device) => {
      const card = document.createElement("button");
      card.type = "button";
      card.className = "device-card";
      const roles = (device.roles || []).length;
      const web = device.web_serial;
      card.innerHTML =
        "<h3></h3><p class=\"meta\"></p><p class=\"meta\"></p>";
      card.querySelector("h3").textContent = device.name;
      card.querySelectorAll(".meta")[0].textContent =
        deviceMakerLabel(device) + " · " + roles + " role" + (roles === 1 ? "" : "s");
      card.querySelectorAll(".meta")[1].innerHTML = web
        ? '<span class="pill ok">Web Serial</span>'
        : '<span class="pill warn">Download</span>';
      card.addEventListener("click", () => {
        openDevice = device;
        openRole = null;
        clearFlashProgress();
        openSheet();
        renderRoleList(device);
      });
      deviceGrid.appendChild(card);
    });
  }

  function fillMakers() {
    makerFilter.innerHTML = '<option value="">All makers</option>';
    const fromApi = catalog.makers || [];
    if (fromApi.length) {
      fromApi.forEach((m) => {
        const opt = document.createElement("option");
        opt.value = m.id || m.label || "";
        opt.textContent = m.label || m.id || "Maker";
        makerFilter.appendChild(opt);
      });
      return;
    }
    const makers = Array.from(
      new Set((catalog.devices || []).map((d) => deviceMakerLabel(d)).filter(Boolean))
    ).sort();
    makers.forEach((m) => {
      const opt = document.createElement("option");
      opt.value = m;
      opt.textContent = m;
      makerFilter.appendChild(opt);
    });
  }

  async function loadCatalog() {
    setStatus("Loading firmware catalog…");
    emptyEl.hidden = true;
    deviceGrid.innerHTML = "";
    const ac = typeof AbortController !== "undefined" ? new AbortController() : null;
    const timer = ac
      ? setTimeout(function () {
          ac.abort();
        }, 45000)
      : null;
    let res;
    try {
      res = await fetch("/api/catalog", {
        credentials: "same-origin",
        signal: ac ? ac.signal : undefined,
      });
    } finally {
      if (timer) clearTimeout(timer);
    }
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new Error(body.error || "catalog " + res.status);
    }
    catalog = await res.json();
    hqName.textContent = (catalog.hq_name || "LAN flash table") + " · trusted builds only";
    const mins = Number(catalog.session_mins) || 480;
    const hours = Math.max(1, Math.round(mins / 60));
    const boards = (catalog.devices || []).length;
    const files = Number(catalog.count) || 0;
    sessionNote.textContent =
      "Session ~" + hours + "h · " + boards + " boards · " + files + " files";
    if (catalog.catalog_error) {
      setStatus("Catalog note: " + catalog.catalog_error);
    } else {
      setStatus(boards ? "" : "No boards in catalog yet.");
    }
    fillMakers();
    renderDevices();
  }

  document.querySelectorAll(".mode-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      mode = tab.dataset.mode || "all";
      document.querySelectorAll(".mode-tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      renderDevices();
    });
  });
  filterEl.addEventListener("input", renderDevices);
  makerFilter.addEventListener("change", renderDevices);
  document.getElementById("sheet-back").addEventListener("click", () => {
    if (sheetMode === "help" || sheetMode === "custom") {
      closeSheet();
      return;
    }
    if (sheetMode === "console" || sheetMode === "setup") {
      if (openDevice && openRole) {
        disconnectConsole();
        renderRoleDetail(openDevice, openRole);
        return;
      }
      if (openDevice) {
        disconnectConsole();
        renderRoleList(openDevice);
        return;
      }
      closeSheet();
      return;
    }
    if (openRole && openDevice) {
      openRole = null;
      clearFlashProgress();
      renderRoleList(openDevice);
      return;
    }
    closeSheet();
  });
  sheetBackdrop.addEventListener("click", closeSheet);
  document.getElementById("help-btn").addEventListener("click", () => {
    renderHelpGuide();
  });
  const loginHelpBtn = document.getElementById("login-help-btn");
  if (loginHelpBtn) {
    loginHelpBtn.addEventListener("click", () => {
      renderHelpGuide();
    });
  }
  document.getElementById("custom-btn").addEventListener("click", () => {
    renderCustomFirmware();
  });
  document.getElementById("console-btn").addEventListener("click", () => {
    openDevice = openDevice || null;
    openRole = null;
    renderConsole(openDevice);
  });

  loginForm.addEventListener("submit", async (e) => {
    e.preventDefault();
    loginError.hidden = true;
    const pin = document.getElementById("pin").value;
    try {
      const res = await fetch("/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify({ pin }),
      });
      if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        throw new Error(body.error || "Login failed");
      }
      showLocker(true);
      await loadCatalog();
    } catch (err) {
      loginError.hidden = false;
      loginError.textContent = String(err.message || err);
    }
  });

  document.getElementById("logout-btn").addEventListener("click", async () => {
    closeSheet();
    await fetch("/logout", { method: "POST", credentials: "same-origin" });
    showLocker(false);
  });

  const SLIP = {
    END: 0xc0,
    ESC: 0xdb,
    ESC_END: 0xdc,
    ESC_ESC: 0xdd,
  };

  function slipEncode(data) {
    const out = [SLIP.END];
    for (const b of data) {
      if (b === SLIP.END) {
        out.push(SLIP.ESC, SLIP.ESC_END);
      } else if (b === SLIP.ESC) {
        out.push(SLIP.ESC, SLIP.ESC_ESC);
      } else {
        out.push(b);
      }
    }
    out.push(SLIP.END);
    return new Uint8Array(out);
  }

  function checksum(data) {
    let c = 0xef;
    for (const b of data) c ^= b;
    return c & 0xff;
  }

  function u32(n) {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setUint32(0, n >>> 0, true);
    return b;
  }

  function concatBytes(parts) {
    const len = parts.reduce((n, p) => n + p.length, 0);
    const out = new Uint8Array(len);
    let o = 0;
    for (const p of parts) {
      out.set(p, o);
      o += p.length;
    }
    return out;
  }

  function pickFlashAsset(version) {
    const assets = (version && version.assets) || [];
    const nrf = assets.find((a) => a.flash_kind === "nrf_dfu" || (a.web_serial && /\.zip$/i.test(a.name)));
    if (nrf) return nrf;
    const esp = assets.find((a) => a.flash_kind === "esp_slip" || (a.web_serial && /\.bin$/i.test(a.name)));
    if (esp) return esp;
    return assets.find((a) => a.web_serial) || assets[0];
  }

  async function flashNrfDfu(asset, zipBytes) {
    if (!window.NrfDfu || !window.JSZip) {
      logLine("nRF DFU library failed to load. Hard-refresh the locker page.");
      setFlashProgress(0, "DFU library missing");
      return;
    }
    setFlashProgress(10, "Select the serial port (application)…");
    logLine("nRF DFU: select the board serial port, then confirm again after DFU reboot.");
    let appPort;
    try {
      appPort = await navigator.serial.requestPort();
    } catch (err) {
      logLine("Port select failed: " + err);
      setFlashProgress(0, "Port open failed");
      return;
    }
    setFlashProgress(15, "Entering DFU mode (1200 baud)…");
    logLine("Entering DFU mode…");
    try {
      await window.NrfDfu.enterDfuMode(appPort);
    } catch (err) {
      logLine("DFU mode failed: " + err);
      setFlashProgress(0, "DFU mode failed");
      try {
        await appPort.close();
      } catch (_) {}
      return;
    }
    setFlashProgress(20, "Select the DFU serial port…");
    logLine("Select the DFU port (board may have re-enumerated)…");
    let dfuPort;
    try {
      dfuPort = await navigator.serial.requestPort();
    } catch (err) {
      logLine("DFU port select failed: " + err);
      setFlashProgress(0, "DFU port failed");
      return;
    }
    try {
      await window.NrfDfu.performDfu(dfuPort, zipBytes, {
        onProgress: function (p) {
          setFlashProgress(20 + (p.percent || 0) * 0.8, p.message || "Flashing…");
          if (p.message && (p.percent === 0 || p.percent === 100 || (p.percent || 0) % 10 === 0)) {
            logLine(p.message + (p.total ? " (" + p.percent + "%)" : ""));
          }
        },
      });
      setFlashProgress(100, "Flash complete");
      logLine("nRF DFU complete. Board should reboot into firmware.");
    } catch (err) {
      logLine("nRF DFU error: " + err);
      setFlashProgress(0, "Flash error");
      try {
        await dfuPort.close();
      } catch (_) {}
    }
  }

  function firstHexBlob(text, minLen) {
    let best = "";
    let cur = "";
    for (const c of text) {
      if (/[0-9a-fA-F]/.test(c)) cur += c.toLowerCase();
      else {
        if (cur.length >= minLen && cur.length > best.length) best = cur;
        cur = "";
      }
    }
    if (cur.length >= minLen && cur.length > best.length) best = cur;
    return best;
  }

  function stripCliNoise(raw) {
    return String(raw || "")
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter(
        (l) =>
          l &&
          !l.startsWith(">") &&
          l.toLowerCase() !== "ok" &&
          !l.startsWith("→")
      )
      .join(" ")
      .trim();
  }

  async function openTempSerial() {
    const port = await navigator.serial.requestPort();
    await port.open({ baudRate: 115200 });
    const writer = port.writable.getWriter();
    const reader = port.readable.getReader();
    return { port, writer, reader };
  }

  async function closeTempSerial(sess) {
    if (!sess) return;
    try {
      sess.reader.releaseLock();
    } catch (_) {}
    try {
      sess.writer.releaseLock();
    } catch (_) {}
    try {
      await sess.port.close();
    } catch (_) {}
  }

  async function serialCmd(sess, command, waitMs) {
    const enc = new TextEncoder();
    const dec = new TextDecoder();
    await sess.writer.write(enc.encode(command + "\n"));
    const deadline = Date.now() + (waitMs || 800);
    let out = "";
    while (Date.now() < deadline) {
      const remain = Math.max(1, deadline - Date.now());
      const raced = await Promise.race([
        sess.reader.read(),
        new Promise((resolve) =>
          setTimeout(() => resolve({ timeout: true }), remain)
        ),
      ]);
      if (raced.timeout) break;
      const { value, done } = raced;
      if (done) break;
      if (value && value.length) out += dec.decode(value, { stream: true });
    }
    return out;
  }

  async function harvestKitOverSerial(roleHint) {
    const sess = await openTempSerial();
    try {
      await serialCmd(sess, "", 200);
      const nameRaw = await serialCmd(sess, "get name", 800);
      const pubRaw = await serialCmd(sess, "get public.key", 1200);
      const prvRaw = await serialCmd(sess, "get prv.key", 1500);
      const public_key = firstHexBlob(pubRaw, 32);
      const private_key = firstHexBlob(prvRaw, 32);
      if (!public_key || !private_key) {
        throw new Error(
          "Could not read keys — is the node running MeshCore CLI on this USB port?"
        );
      }
      const kit = {
        public_key,
        private_key,
        name: stripCliNoise(nameRaw),
        role_hint: roleHint || "",
        board: stripCliNoise(await serialCmd(sess, "board", 600)),
        radio: stripCliNoise(await serialCmd(sess, "get radio", 800)).slice(0, 160),
        lat: stripCliNoise(await serialCmd(sess, "get lat", 500)),
        lon: stripCliNoise(await serialCmd(sess, "get lon", 500)),
        advert_interval: stripCliNoise(
          await serialCmd(sess, "get advert.interval", 500)
        ),
        path_hash_mode: stripCliNoise(
          await serialCmd(sess, "get path.hash.mode", 500)
        ),
        guest_password: stripCliNoise(
          await serialCmd(sess, "get guest.password", 500)
        ),
      };
      return kit;
    } finally {
      await closeTempSerial(sess);
    }
  }

  async function restoreKitOverSerial(kit) {
    const sess = await openTempSerial();
    try {
      const prv = String(kit.private_key || "").replace(/[^0-9a-fA-F]/g, "");
      const setRaw = await serialCmd(sess, "set prv.key " + prv, 2000);
      const low = setRaw.toLowerCase();
      if (low.includes("error") || low.includes("fail") || low.includes("denied")) {
        throw new Error("set prv.key failed");
      }
      logLine("[private key set]");
      try {
        await serialCmd(sess, "reboot", 400);
      } catch (_) {}
    } finally {
      await closeTempSerial(sess);
    }
    await new Promise((r) => setTimeout(r, 3000));
    const sess2 = await openTempSerial();
    try {
      if (kit.name) {
        await serialCmd(sess2, "set name " + kit.name, 800);
      }
      if (kit.lat) await serialCmd(sess2, "set lat " + kit.lat, 500);
      if (kit.lon) await serialCmd(sess2, "set lon " + kit.lon, 500);
      if (kit.advert_interval) {
        await serialCmd(
          sess2,
          "set advert.interval " + kit.advert_interval,
          500
        );
      }
      if (kit.guest_password) {
        await serialCmd(
          sess2,
          "set guest.password " + kit.guest_password,
          500
        );
      }
      const pubRaw = await serialCmd(sess2, "get public.key", 1500);
      const got = firstHexBlob(pubRaw, 32);
      const expect = String(kit.public_key || "")
        .replace(/[^0-9a-fA-F]/g, "")
        .toLowerCase();
      if (got && expect && got !== expect) {
        throw new Error("Public key mismatch after restore");
      }
      logLine(
        "Identity restored" +
          (got ? " (" + got.slice(0, 8) + "…)" : "") +
          "."
      );
    } finally {
      await closeTempSerial(sess2);
    }
  }

  function promptKitPassphrase(title) {
    const a = window.prompt(title + "\n\nKit passphrase:");
    if (a === null) return null;
    const b = window.prompt("Confirm kit passphrase:");
    if (b === null) return null;
    if (!a.trim() || a !== b) {
      window.alert("Passphrases empty or do not match.");
      return null;
    }
    return a;
  }

  async function uploadNodeKit(kit, passphrase) {
    const res = await fetch("/api/node-kit", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(Object.assign({ passphrase: passphrase }, kit)),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new Error(data.error || "kit upload failed (" + res.status + ")");
    }
    return data;
  }

  async function unlockNodeKit(id, passphrase) {
    const res = await fetch("/api/node-kit/unlock", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: id, passphrase: passphrase }),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new Error(data.error || "unlock failed (" + res.status + ")");
    }
    return data;
  }

  async function maybeBackupBeforeFlash(eraseFirst, role) {
    const msg = eraseFirst
      ? "Erase wipes the MeshCore private key. Back up identity over USB into an encrypted kit on this HQ first?"
      : "Back up this node’s identity over USB before flashing?";
    if (!window.confirm(msg)) {
      if (
        eraseFirst &&
        !window.confirm("Flash with erase without saving a kit backup?")
      ) {
        return false;
      }
      return true;
    }
    if (!("serial" in navigator)) {
      logLine("Web Serial required to back up identity.");
      return false;
    }
    const pass = promptKitPassphrase("Encrypt node identity kit");
    if (pass === null) return false;
    try {
      await disconnectConsole();
      showSheetFlash(true);
      flashLog.hidden = false;
      logLine("Backing up identity over USB… pick the app serial port.");
      const kit = await harvestKitOverSerial(
        (role && (role.role || role.label)) || ""
      );
      logLine("[private key read]");
      const saved = await uploadNodeKit(kit, pass);
      logLine("Kit saved on HQ: " + (saved.file_name || saved.id || "ok"));
      return true;
    } catch (err) {
      logLine("Identity backup failed: " + err);
      return window.confirm("Backup failed. Continue flash anyway?");
    }
  }

  async function maybeRestoreAfterFlash() {
    if (
      !window.confirm(
        "Flash finished. Restore a node identity kit over USB now?"
      )
    ) {
      return;
    }
    try {
      const listRes = await fetch("/api/node-kits", {
        credentials: "same-origin",
      });
      const listData = await listRes.json().catch(() => ({}));
      const kits = listData.kits || [];
      if (!kits.length) {
        window.alert("No kits on this HQ yet.");
        return;
      }
      const names = kits
        .map(
          (k, i) =>
            i +
            1 +
            ") " +
            (k.name || "node") +
            " " +
            (k.public_key_prefix || "") +
            "… — " +
            (k.file_name || k.id)
        )
        .join("\n");
      const pick = window.prompt(
        "Enter kit number to restore:\n\n" + names
      );
      if (pick === null) return;
      const idx = Number(pick) - 1;
      if (!(idx >= 0 && idx < kits.length)) {
        window.alert("Invalid kit number.");
        return;
      }
      const id = kits[idx].id || kits[idx].file_name;
      const pass = window.prompt("Kit passphrase:");
      if (pass === null || !pass.trim()) return;
      const unlocked = await unlockNodeKit(id, pass.trim());
      logLine("Restoring identity over USB… pick the serial port.");
      await restoreKitOverSerial(unlocked);
    } catch (err) {
      logLine("Identity restore failed: " + err);
    }
  }

  async function flashLocalBuffer(name, buffer, kind, eraseFirst) {
    if (!("serial" in navigator)) {
      logLine("Web Serial is not available in this browser.");
      return;
    }
    await disconnectConsole();
    flashLog.hidden = false;
    flashLog.textContent = "";
    showSheetFlash(true);
    flashProgress.hidden = false;
    flashProgressBar.style.width = "0%";
    const resolvedKind = kind || (/\.zip$/i.test(name) ? "nrf_dfu" : "esp_slip");
    if (resolvedKind === "nrf_dfu") {
      setFlashProgress(8, "Loaded " + buffer.byteLength + " bytes (DFU zip)");
      logLine("Loaded " + name + " (" + buffer.byteLength + " bytes, nRF DFU).");
      await flashNrfDfu({ name: name }, buffer);
      return;
    }
    await flashEspSlip(name, new Uint8Array(buffer), eraseFirst);
  }

  async function flashAsset(asset, eraseFirst) {
    if (!("serial" in navigator)) {
      logLine("Web Serial is not available in this browser.");
      return;
    }
    await disconnectConsole();
    flashLog.hidden = false;
    flashLog.textContent = "";
    showSheetFlash(true);
    flashProgress.hidden = false;
    flashProgressBar.style.width = "0%";
    setFlashProgress(2, "Fetching " + asset.name + "…");
    logLine("Fetching " + asset.name + "…");
    const binRes = await fetch("/api/firmware/" + encodeURIComponent(asset.id), {
      credentials: "same-origin",
    });
    if (!binRes.ok) {
      logLine("Download failed: " + binRes.status);
      setFlashProgress(0, "Download failed");
      return;
    }
    const buffer = await binRes.arrayBuffer();
    const kind =
      asset.flash_kind ||
      (/\.zip$/i.test(asset.name) ? "nrf_dfu" : "esp_slip");
    await flashLocalBuffer(asset.name, buffer, kind, eraseFirst);
  }

  async function flashEspSlip(name, firmware, eraseFirst) {
    setFlashProgress(8, "Loaded " + firmware.length + " bytes");
    logLine("Loaded " + name + " (" + firmware.length + " bytes). Pick the USB serial port…");

    let port;
    try {
      port = await navigator.serial.requestPort();
      await port.open({ baudRate: 115200 });
    } catch (err) {
      logLine("Port open failed: " + err);
      setFlashProgress(0, "Port open failed");
      return;
    }

    const writer = port.writable.getWriter();
    const reader = port.readable.getReader();

    async function writeCmd(op, data, checksumByte) {
      const hdr = new Uint8Array(8);
      hdr[0] = 0x00;
      hdr[1] = op;
      new DataView(hdr.buffer).setUint16(2, data.length, true);
      new DataView(hdr.buffer).setUint32(4, checksumByte >>> 0, true);
      await writer.write(slipEncode(concatBytes([hdr, data])));
    }

    async function readSlip(timeoutMs) {
      const deadline = Date.now() + timeoutMs;
      const buf = [];
      let inPkt = false;
      while (Date.now() < deadline) {
        const remain = Math.max(1, deadline - Date.now());
        const timer = new Promise((resolve) =>
          setTimeout(() => resolve({ timeout: true }), remain)
        );
        const raced = await Promise.race([reader.read(), timer]);
        if (raced.timeout) break;
        const { value, done } = raced;
        if (done) break;
        for (const b of value) {
          if (b === SLIP.END) {
            if (inPkt && buf.length) {
              const pkt = new Uint8Array(buf);
              buf.length = 0;
              inPkt = false;
              return pkt;
            }
            inPkt = true;
            buf.length = 0;
            continue;
          }
          if (!inPkt) continue;
          if (b === SLIP.ESC) {
            buf.push(b);
            continue;
          }
          if (buf.length && buf[buf.length - 1] === SLIP.ESC) {
            buf.pop();
            if (b === SLIP.ESC_END) buf.push(SLIP.END);
            else if (b === SLIP.ESC_ESC) buf.push(SLIP.ESC);
            else buf.push(b);
          } else {
            buf.push(b);
          }
        }
      }
      return null;
    }

    try {
      setFlashProgress(12, "Syncing…");
      logLine("Syncing rom…");
      let synced = false;
      for (let i = 0; i < 8; i++) {
        await writeCmd(0x08, new Uint8Array(36), 0);
        const resp = await readSlip(200);
        if (resp && resp.length >= 2 && resp[0] === 0x01 && resp[1] === 0x08) {
          logLine("Synced.");
          synced = true;
          break;
        }
      }
      if (!synced) {
        logLine("Could not sync. Hold BOOT, tap RESET, try again.");
        setFlashProgress(0, "Sync failed");
        return;
      }

      if (eraseFirst) {
        setFlashProgress(18, "Erasing flash…");
        logLine("Erase requested — chip erase…");
        await writeCmd(0x0d, new Uint8Array(0), 0);
        await readSlip(15000);
      }

      const blockSize = 0x400;
      const numBlocks = Math.ceil(firmware.length / blockSize);
      setFlashProgress(20, "Writing firmware…");
      logLine("Writing " + numBlocks + " blocks…");
      const begin = concatBytes([
        u32(firmware.length),
        u32(numBlocks),
        u32(blockSize),
        u32(0),
      ]);
      await writeCmd(0x02, begin, 0);
      await readSlip(3000);

      for (let seq = 0; seq < numBlocks; seq++) {
        const start = seq * blockSize;
        let chunk = firmware.subarray(start, Math.min(start + blockSize, firmware.length));
        if (chunk.length < blockSize) {
          const padded = new Uint8Array(blockSize);
          padded.set(chunk);
          chunk = padded;
        }
        const data = concatBytes([u32(chunk.length), u32(seq), u32(0), u32(0), chunk]);
        await writeCmd(0x03, data, checksum(chunk));
        await readSlip(3000);
        const pct = 20 + ((seq + 1) / numBlocks) * 80;
        if (seq % 4 === 0 || seq === numBlocks - 1) {
          setFlashProgress(pct, "Writing block " + (seq + 1) + "/" + numBlocks);
        }
        if (seq % 16 === 0 || seq === numBlocks - 1) {
          logLine("Block " + (seq + 1) + "/" + numBlocks);
        }
      }

      setFlashProgress(100, "Flash complete");
      logLine("Flash complete. Reset the board, then use Console for CLI.");
    } catch (err) {
      logLine("Flash error: " + err);
      setFlashProgress(0, "Flash error");
    } finally {
      try {
        reader.releaseLock();
      } catch (_) {}
      try {
        writer.releaseLock();
      } catch (_) {}
      try {
        await port.close();
      } catch (_) {}
    }
  }

  if (authed) {
    showLocker(true);
    loadCatalog().catch((err) => {
      setStatus("Could not load catalog: " + (err.message || err));
      emptyEl.hidden = false;
      emptyEl.textContent = String(err.message || err);
    });
  } else {
    showLocker(false);
  }
})();
