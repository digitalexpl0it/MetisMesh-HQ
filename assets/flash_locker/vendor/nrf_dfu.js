/*!
 * nRF52 serial DFU for Metis Mesh HQ flash locker.
 * Adapted from nrfutil-web (https://github.com/takkaO/nrfutil-web) — BSD-3-Clause.
 * Requires global JSZip (vendor/jszip.min.js).
 */
(function (global) {
  "use strict";

  function crc16(data, crc) {
    crc = crc == null ? 0xffff : crc;
    for (var i = 0; i < data.length; i++) {
      var byte = data[i];
      crc = ((crc >> 8) & 0x00ff) | ((crc << 8) & 0xff00);
      crc ^= byte;
      crc ^= (crc & 0x00ff) >> 4;
      crc ^= (crc << 8) << 4;
      crc ^= ((crc & 0x00ff) << 4) << 1;
    }
    return crc & 0xffff;
  }

  var DfuCommand = { INIT_PACKET: 1, START_PACKET: 3, DATA_PACKET: 4, STOP_DATA_PACKET: 5 };
  var DfuUpdateMode = { SOFTDEVICE: 1, BOOTLOADER: 2, SD_BL: 3, APPLICATION: 4 };
  var DFU_PACKET_MAX_SIZE = 512;
  var FLASH_PAGE_SIZE = 4096;
  var DfuTiming = {
    TOUCH_RESET_WAIT: 1500,
    DTR_RESET_WAIT: 100,
    ACK_TIMEOUT: 5000,
    FLASH_PAGE_ERASE_TIME: 0.0897,
    FLASH_WORD_WRITE_TIME: 0.0001,
    FLASH_PAGE_WRITE_TIME: (FLASH_PAGE_SIZE / 4) * 0.0001,
  };

  function int32ToBytes(value) {
    return new Uint8Array([
      value & 0xff,
      (value >> 8) & 0xff,
      (value >> 16) & 0xff,
      (value >> 24) & 0xff,
    ]);
  }

  function buildStartPacket(mode, softdeviceSize, bootloaderSize, applicationSize) {
    var payload = new Uint8Array(20);
    payload.set(int32ToBytes(DfuCommand.START_PACKET), 0);
    payload.set(int32ToBytes(mode), 4);
    payload.set(int32ToBytes(softdeviceSize), 8);
    payload.set(int32ToBytes(bootloaderSize), 12);
    payload.set(int32ToBytes(applicationSize), 16);
    return payload;
  }

  function buildInitPacket(initData) {
    var payload = new Uint8Array(4 + initData.length + 2);
    payload.set(int32ToBytes(DfuCommand.INIT_PACKET), 0);
    payload.set(initData, 4);
    return payload;
  }

  function buildDataPacket(chunk) {
    var payload = new Uint8Array(4 + chunk.length);
    payload.set(int32ToBytes(DfuCommand.DATA_PACKET), 0);
    payload.set(chunk, 4);
    return payload;
  }

  function buildStopDataPacket() {
    return int32ToBytes(DfuCommand.STOP_DATA_PACKET);
  }

  function getEraseWaitTime(totalSize) {
    var pages = Math.floor(totalSize / FLASH_PAGE_SIZE) + 1;
    return Math.max(500, pages * DfuTiming.FLASH_PAGE_ERASE_TIME * 1000);
  }

  function getActivateWaitTime(totalSize, singleBank, softdeviceSize) {
    if (singleBank && softdeviceSize === 0) {
      return (DfuTiming.FLASH_PAGE_ERASE_TIME + DfuTiming.FLASH_PAGE_WRITE_TIME) * 1000;
    }
    var pages = Math.floor(totalSize / FLASH_PAGE_SIZE) + 1;
    return (pages * DfuTiming.FLASH_PAGE_ERASE_TIME + pages * DfuTiming.FLASH_PAGE_WRITE_TIME) * 1000;
  }

  var SLIP_BYTE_END = 0xc0;
  var SLIP_BYTE_ESC = 0xdb;
  var SLIP_BYTE_ESC_END = 0xdc;
  var SLIP_BYTE_ESC_ESC = 0xdd;

  function slipEncode(data) {
    var encoded = [SLIP_BYTE_END];
    for (var i = 0; i < data.length; i++) {
      var byte = data[i];
      if (byte === SLIP_BYTE_END) encoded.push(SLIP_BYTE_ESC, SLIP_BYTE_ESC_END);
      else if (byte === SLIP_BYTE_ESC) encoded.push(SLIP_BYTE_ESC, SLIP_BYTE_ESC_ESC);
      else encoded.push(byte);
    }
    encoded.push(SLIP_BYTE_END);
    return new Uint8Array(encoded);
  }

  function SlipDecoder() {
    this.buffer = [];
    this.inFrame = false;
  }
  SlipDecoder.prototype.feed = function (data) {
    var frames = [];
    for (var i = 0; i < data.length; i++) {
      var byte = data[i];
      if (byte === SLIP_BYTE_END) {
        if (this.inFrame && this.buffer.length > 0) {
          var decoded = [];
          var escaped = false;
          for (var j = 0; j < this.buffer.length; j++) {
            var b = this.buffer[j];
            if (escaped) {
              if (b === SLIP_BYTE_ESC_END) decoded.push(SLIP_BYTE_END);
              else if (b === SLIP_BYTE_ESC_ESC) decoded.push(SLIP_BYTE_ESC);
              escaped = false;
            } else if (b === SLIP_BYTE_ESC) escaped = true;
            else decoded.push(b);
          }
          if (decoded.length) frames.push(new Uint8Array(decoded));
        }
        this.buffer = [];
        this.inFrame = true;
      } else if (this.inFrame) {
        this.buffer.push(byte);
      }
    }
    return frames;
  };
  SlipDecoder.prototype.reset = function () {
    this.buffer = [];
    this.inFrame = false;
  };

  function buildHciPacket(sequenceNumber, payload) {
    var seq = sequenceNumber & 0x07;
    var ack = ((sequenceNumber + 1) % 8) & 0x07;
    var payloadLength = payload.length;
    var byte0 = (seq & 0x07) | ((ack & 0x07) << 3) | (1 << 6) | (1 << 7);
    var byte1 = (14 & 0x0f) | ((payloadLength & 0x0f) << 4);
    var byte2 = (payloadLength >> 4) & 0xff;
    var byte3 = (~(byte0 + byte1 + byte2) + 1) & 0xff;
    var packet = new Uint8Array(4 + payload.length + 2);
    packet[0] = byte0;
    packet[1] = byte1;
    packet[2] = byte2;
    packet[3] = byte3;
    packet.set(payload, 4);
    var packetCrc = crc16(packet.subarray(0, 4 + payload.length));
    packet[4 + payload.length] = packetCrc & 0xff;
    packet[4 + payload.length + 1] = (packetCrc >> 8) & 0xff;
    return packet;
  }

  function parseHciResponse(data) {
    if (data.length < 4) return null;
    return { sequenceNumber: data[0] & 0x07, ackNumber: (data[0] >> 3) & 0x07 };
  }

  function sleep(ms) {
    return new Promise(function (resolve) {
      setTimeout(resolve, ms);
    });
  }

  function DfuSerialTransport(port, options) {
    options = options || {};
    this.port = port;
    this.writer = null;
    this.reader = null;
    this.slipDecoder = new SlipDecoder();
    this.sequenceNumber = 0;
    this.ackTimeout = options.ackTimeout != null ? options.ackTimeout : DfuTiming.ACK_TIMEOUT;
    this.maxRetries = options.maxRetries != null ? options.maxRetries : 3;
    this.receivedData = [];
    this.reading = false;
    this.readLoopPromise = null;
  }

  DfuSerialTransport.prototype.open = async function (baudRate) {
    baudRate = baudRate || 115200;
    await this.port.open({ baudRate: baudRate });
    if (!this.port.readable || !this.port.writable) {
      throw new Error("Serial port is not readable/writable after open");
    }
    await sleep(100);
    if (this.port.setSignals) {
      try {
        await this.port.setSignals({ dataTerminalReady: false });
        await sleep(50);
        await this.port.setSignals({ dataTerminalReady: true });
        await sleep(DfuTiming.DTR_RESET_WAIT);
      } catch (_) {}
    }
    this.writer = this.port.writable.getWriter();
    this.sequenceNumber = 0;
    this.receivedData = [];
    this.slipDecoder.reset();
    this.startReading();
  };

  DfuSerialTransport.prototype.close = async function () {
    await this.stopReading();
    try {
      if (this.writer) this.writer.releaseLock();
    } catch (_) {}
    this.writer = null;
    await this.port.close();
  };

  DfuSerialTransport.prototype.startReading = function () {
    var self = this;
    if (this.reading || !this.port.readable) return;
    this.reading = true;
    this.reader = this.port.readable.getReader();
    this.readLoopPromise = (async function () {
      try {
        while (self.reading) {
          var result = await self.reader.read();
          if (result.done) break;
          if (result.value) {
            for (var i = 0; i < result.value.length; i++) self.receivedData.push(result.value[i]);
          }
        }
      } catch (_) {
      } finally {
        self.reading = false;
        try {
          if (self.reader) self.reader.releaseLock();
        } catch (_) {}
        self.reader = null;
      }
    })();
  };

  DfuSerialTransport.prototype.stopReading = async function () {
    if (!this.reading) return;
    this.reading = false;
    try {
      if (this.reader) await this.reader.cancel();
    } catch (_) {}
    await this.readLoopPromise;
    this.readLoopPromise = null;
  };

  DfuSerialTransport.prototype.waitForAck = async function () {
    var deadline = Date.now() + this.ackTimeout;
    while (Date.now() < deadline) {
      var endCount = 0;
      for (var i = 0; i < this.receivedData.length; i++) {
        if (this.receivedData[i] === SLIP_BYTE_END) endCount++;
      }
      if (endCount >= 2) break;
      await sleep(10);
    }
    var ends = 0;
    for (var j = 0; j < this.receivedData.length; j++) {
      if (this.receivedData[j] === SLIP_BYTE_END) ends++;
    }
    if (ends < 2) throw new Error("ACK timeout");
    var rawData = new Uint8Array(this.receivedData);
    var frames = this.slipDecoder.feed(rawData);
    this.receivedData = [];
    for (var k = 0; k < frames.length; k++) {
      var frame = frames[k];
      if (!parseHciResponse(frame)) continue;
      if (frame.length > 6) return frame.subarray(4, frame.length - 2);
      return new Uint8Array([]);
    }
    throw new Error("No valid ACK in received data");
  };

  DfuSerialTransport.prototype.sendPacket = async function (payload) {
    if (!this.writer) throw new Error("Transport is not open");
    var lastError = null;
    for (var attempt = 0; attempt <= this.maxRetries; attempt++) {
      try {
        this.sequenceNumber = (this.sequenceNumber + 1) % 8;
        var hciPacket = buildHciPacket(this.sequenceNumber, payload);
        var slipFrame = slipEncode(hciPacket);
        this.receivedData = [];
        await this.writer.write(slipFrame);
        await sleep(10);
        return await this.waitForAck();
      } catch (err) {
        lastError = err;
        if (attempt < this.maxRetries) {
          this.slipDecoder.reset();
          this.receivedData = [];
          await sleep(100);
        }
      }
    }
    throw new Error(
      "Failed after " +
        (this.maxRetries + 1) +
        " attempts: " +
        (lastError && lastError.message ? lastError.message : lastError)
    );
  };

  async function parseDfuPackage(zipData) {
    if (typeof JSZip === "undefined") {
      throw new Error("JSZip is required for nRF DFU packages");
    }
    var zip = await JSZip.loadAsync(zipData);
    var manifestFile = zip.file("manifest.json");
    if (!manifestFile) throw new Error("DFU package does not contain manifest.json");
    var manifest = JSON.parse(await manifestFile.async("text"));
    if (!manifest.manifest) throw new Error("Invalid manifest format");

    async function extractImage(entry, type) {
      var binFile = zip.file(entry.bin_file);
      var datFile = zip.file(entry.dat_file);
      if (!binFile) throw new Error("Missing firmware binary: " + entry.bin_file);
      if (!datFile) throw new Error("Missing init packet: " + entry.dat_file);
      var firmware = new Uint8Array(await binFile.async("arraybuffer"));
      var initPacket = new Uint8Array(await datFile.async("arraybuffer"));
      var softdeviceSize = 0;
      var bootloaderSize = 0;
      var applicationSize = 0;
      if (type === DfuUpdateMode.APPLICATION) applicationSize = firmware.length;
      else if (type === DfuUpdateMode.SOFTDEVICE) softdeviceSize = firmware.length;
      else if (type === DfuUpdateMode.BOOTLOADER) bootloaderSize = firmware.length;
      else if (type === DfuUpdateMode.SD_BL) {
        softdeviceSize = entry.sd_size || 0;
        bootloaderSize = entry.bl_size || 0;
      }
      return {
        type: type,
        initPacket: initPacket,
        firmware: firmware,
        softdeviceSize: softdeviceSize,
        bootloaderSize: bootloaderSize,
        applicationSize: applicationSize,
      };
    }

    var images = [];
    var m = manifest.manifest;
    if (m.softdevice_bootloader) images.push(await extractImage(m.softdevice_bootloader, DfuUpdateMode.SD_BL));
    if (m.softdevice) images.push(await extractImage(m.softdevice, DfuUpdateMode.SOFTDEVICE));
    if (m.bootloader) images.push(await extractImage(m.bootloader, DfuUpdateMode.BOOTLOADER));
    if (m.application) images.push(await extractImage(m.application, DfuUpdateMode.APPLICATION));
    if (!images.length) throw new Error("No firmware images found in manifest");
    return images;
  }

  async function sendImage(transport, image, singleBank, report) {
    var totalSize = image.firmware.length;
    report("Sending start packet...", 0, totalSize);
    await transport.sendPacket(
      buildStartPacket(image.type, image.softdeviceSize, image.bootloaderSize, image.applicationSize)
    );
    report("Erasing flash...", 0, totalSize);
    await sleep(getEraseWaitTime(totalSize));
    report("Sending init packet...", 0, totalSize);
    await transport.sendPacket(buildInitPacket(image.initPacket));
    report("Sending firmware data...", 0, totalSize);
    var bytesSent = 0;
    var packetCount = 0;
    for (var offset = 0; offset < totalSize; offset += DFU_PACKET_MAX_SIZE) {
      var end = Math.min(offset + DFU_PACKET_MAX_SIZE, totalSize);
      var chunk = image.firmware.subarray(offset, end);
      await transport.sendPacket(buildDataPacket(chunk));
      bytesSent += chunk.length;
      packetCount++;
      report("Sending firmware data...", bytesSent, totalSize);
      if (packetCount % 8 === 0) await sleep(DfuTiming.FLASH_PAGE_WRITE_TIME * 1000);
    }
    await sleep(DfuTiming.FLASH_PAGE_WRITE_TIME * 1000);
    report("Finalizing...", totalSize, totalSize);
    await transport.sendPacket(buildStopDataPacket());
  }

  async function performDfu(port, zipData, options) {
    options = options || {};
    var onProgress = options.onProgress;
    var singleBank = options.singleBank !== false;
    function report(message, sent, total) {
      sent = sent || 0;
      total = total || 0;
      var percent = total > 0 ? Math.round((sent / total) * 100) : 0;
      if (onProgress) onProgress({ sent: sent, total: total, percent: percent, message: message });
    }
    report("Parsing DFU package...");
    var images = await parseDfuPackage(zipData);
    for (var i = 0; i < images.length; i++) {
      var image = images[i];
      report("Opening serial connection...");
      var transport = new DfuSerialTransport(port, {
        ackTimeout: options.ackTimeout,
        maxRetries: options.maxRetries,
      });
      await transport.open();
      try {
        await sendImage(transport, image, singleBank, report);
      } finally {
        await transport.close();
      }
      report("Activating new firmware...");
      await sleep(getActivateWaitTime(image.firmware.length, singleBank, image.softdeviceSize));
    }
    report("DFU complete!", 1, 1);
  }

  async function enterDfuMode(port, waitTime) {
    waitTime = waitTime == null ? DfuTiming.TOUCH_RESET_WAIT : waitTime;
    await port.open({ baudRate: 1200 });
    await sleep(100);
    await port.close();
    await sleep(waitTime);
  }

  global.NrfDfu = {
    performDfu: performDfu,
    enterDfuMode: enterDfuMode,
    parseDfuPackage: parseDfuPackage,
  };
})(typeof window !== "undefined" ? window : globalThis);
