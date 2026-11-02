// Test vectors for the board's relay client (rust/logic/src/relay.rs and
// endpoint.rs), produced by Paseo's own installed code: its Noise NK handshake,
// record framing, daemon-side secure channel and pairing-link helpers. Nothing
// here touches a network or a daemon.
//
//   node tools/relay_vectors.mjs > protocol/fixtures/relay.json
//
// Paseo is found under `npm root -g` (@getpaseo/cli); PASEO_NODE_MODULES
// overrides the node_modules directory that holds @getpaseo/*. Regenerate when
// Paseo changes its relay protocol, and keep the Paseo version in the output.

import assert from "node:assert/strict";
import { execSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const ROOT =
  process.env.PASEO_NODE_MODULES ??
  `${execSync("npm root -g", { encoding: "utf8" }).trim()}/@getpaseo/cli/node_modules`;
const at = (rel) => new URL(`file://${ROOT}/${rel}`).href;

const hex = (bytes) => Buffer.from(bytes).toString("hex");
const b64 = (bytes) => Buffer.from(bytes).toString("base64");
const fixed = (label) => new Uint8Array(createHash("sha256").update(label).digest());

// The daemon's channel draws its Noise ephemeral from crypto.getRandomValues.
const draws = [];
const realCrypto = globalThis.crypto;
Object.defineProperty(globalThis, "crypto", {
  configurable: true,
  value: {
    subtle: realCrypto.subtle,
    getRandomValues(buf) {
      const next = draws.shift();
      if (!next || next.byteLength !== buf.byteLength) throw new Error(`unexpected random draw of ${buf.byteLength} bytes`);
      buf.set(next);
      return buf;
    },
  },
});

const noise = await import(at("@getpaseo/relay/dist/noise/index.js"));
const records = await import(at("@getpaseo/relay/dist/protected-records.js"));
const e2ee = await import(at("@getpaseo/relay/dist/e2ee.js"));
const endpoints = await import(at("@getpaseo/protocol/dist/daemon-endpoints.js"));
const offers = await import(at("@getpaseo/protocol/dist/connection-offer.js"));
const offerServer = await import(at("@getpaseo/server/dist/server/server/connection-offer.js"));
const { x25519 } = await import(at("@noble/curves/ed25519.js"));
const paseoVersion = JSON.parse(readFileSync(`${ROOT}/../package.json`, "utf8")).version;

const PROLOGUE = new TextEncoder().encode("paseo-relay-e2ee/1");
// The board offers no binary ciphertext: params [version 1, flags 0].
const BOARD_PARAMS = new Uint8Array([0x01, 0x00]);

const pair = (sk) => ({ secretKey: sk, publicKey: x25519.getPublicKey(sk) });
const daemonStatic = pair(fixed("farol-vector/daemon-static"));
const boardEphemeral = fixed("farol-vector/board-noise-ephemeral");
const boardRouting = fixed("farol-vector/board-routing-key");
const daemonEphemeral = fixed("farol-vector/daemon-noise-ephemeral");

// What the board sends: its e2ee_hello and, once open, its session hello.
const initiator = noise.createNoiseInitiator({
  remoteStatic: daemonStatic.publicKey,
  prologue: PROLOGUE,
  ephemeral: pair(boardEphemeral.slice()),
});
const msg1 = initiator.writeMessage(BOARD_PARAMS);
const helloText = JSON.stringify({
  type: "e2ee_hello",
  key: b64(x25519.getPublicKey(boardRouting)),
  capabilities: { binaryCiphertext: false },
  protected: { v: 1, message: b64(msg1) },
});

// The daemon side is Paseo's real secure channel, fed the board's hello.
const sent = [];
const daemonEnd = {
  onmessage: null,
  onclose: null,
  onerror: null,
  send(data) {
    sent.push(data);
    return Promise.resolve();
  },
  close(code, reason) {
    throw new Error(`daemon closed: ${code} ${reason}`);
  },
};
let received;
const got = new Promise((resolve) => (received = resolve));
draws.push(daemonEphemeral.slice());
const daemonOpen = e2ee.openDaemonSecureChannel(daemonEnd, {
  daemonKeyPair: daemonStatic,
  minimumMode: "protected",
  events: { onmessage: (m) => received(m) },
});
daemonEnd.onmessage({ data: helloText, isBinary: false });
const daemon = await daemonOpen;
assert.equal(daemon.securityMode(), "protected");
const readyText = sent[0];
const ready = JSON.parse(readyText);
assert.equal(ready.type, "e2ee_ready");

// The board finishes the handshake and seals its session hello.
const selected = initiator.readMessage(new Uint8Array(Buffer.from(ready.protected.message, "base64")));
assert.deepEqual(selected, BOARD_PARAMS);
const board = initiator.split();
const boardHello = JSON.stringify({ type: "hello", clientId: "farol-3a1f", clientType: "cli", protocolVersion: 1 });
const boardRecord = records.encodeProtectedRecord("text", new TextEncoder().encode(boardHello), board.send);
daemonEnd.onmessage({ data: b64(boardRecord), isBinary: false });
assert.equal(await got, boardHello);

// Two daemon frames, so the receive counter is exercised.
const serverInfo = JSON.stringify({
  type: "session",
  message: { type: "status", payload: { status: "server_info", serverId: "srv_AAAAAAAAAAAA", hostname: "studio", version: paseoVersion } },
});
const daemonFrames = [];
for (const text of [serverInfo, JSON.stringify({ type: "pong" })]) {
  const before = sent.length;
  await daemon.send(text);
  daemonFrames.push({ plaintext: text, frame: sent[before] });
}

// Pairing links, built by Paseo's own helpers.
const offer = await offerServer.createConnectionOfferV2({
  serverId: "srv_AAAAAAAAAAAA",
  daemonPublicKeyB64: b64(daemonStatic.publicKey),
  relay: { endpoint: "relay.paseo.sh:443", useTls: true },
});
const offerUrl = offerServer.encodeOfferToFragmentUrl({ offer, appBaseUrl: "https://app.paseo.sh" });
assert.deepEqual(offers.parseConnectionOfferFromUrl(offerUrl), offer);
const pinned = await offerServer.createConnectionOfferV2({ ...offer, minimumSecurityMode: "protected" });
const pinnedUrl = offerServer.encodeOfferToFragmentUrl({ offer: pinned, appBaseUrl: "https://app.paseo.sh" });
const relayUri = endpoints.serializeRelayConnectionUri({ offer: pinned, password: "pa ss+word&=" });
assert.equal(endpoints.parseRelayConnectionUri(relayUri).password, "pa ss+word&=");
const connectUrl = `https://app.paseo.sh/#connect=${encodeURIComponent(relayUri)}`;
assert.equal(endpoints.parseRelayConnectionUri(connectUrl).offer.serverId, "srv_AAAAAAAAAAAA");
const plainUri = endpoints.serializeRelayConnectionUri({
  offer: { ...offer, relay: { endpoint: "192.168.1.20:8080", useTls: false } },
});

console.log(
  JSON.stringify(
    {
      paseo: paseoVersion,
      daemonKeyB64: b64(daemonStatic.publicKey),
      boardEphemeralSecret: hex(boardEphemeral),
      boardRoutingSecret: hex(boardRouting),
      handshake: {
        helloText,
        msg1: b64(msg1),
        readyText,
        sendKey: hex(board.send.k),
        receiveKey: hex(board.receive.k),
      },
      boardToDaemon: { plaintext: boardHello, frame: b64(boardRecord) },
      daemonToBoard: daemonFrames,
      links: {
        offerUrl,
        pinnedUrl,
        relayUri,
        connectUrl,
        plainUri,
        serverId: offer.serverId,
        relayEndpoint: "relay.paseo.sh:443",
        relayWebSocketUrl: endpoints.buildRelayWebSocketUrl({
          endpoint: offer.relay.endpoint,
          useTls: true,
          serverId: offer.serverId,
          role: "client",
        }),
        password: "pa ss+word&=",
      },
    },
    null,
    2,
  ),
);
