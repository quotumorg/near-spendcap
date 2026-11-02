import test from "node:test";
import http from "node:http";
import { Readable } from "node:stream";
import assert from "node:assert/strict";
import { startMockEndpoint } from "./mock-endpoint.mjs";

// Exercise the real HTTP handler in process. No sockets or external network.
test("concurrent payment reuse serves exactly once; failed verification releases the hash", { timeout: 10_000 }, async (t) => {
  const wallet = "wallet.testnet";
  const provider = "provider.testnet";
  let handler;
  t.mock.method(http, "createServer", (callback) => {
    handler = callback;
    return {
      listen: (_port, _host, ready) => ready(),
      address: () => ({ port: 12345 }),
      close: (done) => done(),
    };
  });
  let entered;
  const verifying = new Promise((resolve) => { entered = resolve; });
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  let calls = 0;
  let mode = "success";
  t.mock.method(globalThis, "fetch", async (url, options) => {
    assert.match(String(url), /^https:\/\//);
    assert.equal(JSON.parse(options.body).method, "EXPERIMENTAL_tx_status");
    calls++;
    entered();
    await gate;
    if (mode === "throw") throw Object.assign(new Error("invalid payment"), { rpc: true });
    return Response.json({ result: {
      status: mode === "failure" ? { Failure: {} } : { SuccessValue: "" },
      transaction: { signer_id: wallet, receiver_id: wallet },
      receipts_outcome: [{ id: "receipt", outcome: {
        executor_id: wallet, status: { SuccessValue: "" },
        logs: ["EVENT_JSON:" + JSON.stringify({ standard: "quotum-spendcap", event: "paid", data: [{ request_id: "request", provider_id: provider }] })],
      } }],
      receipts: [{ receipt_id: "receipt", predecessor_id: wallet, receiver_id: provider,
        receipt: { Action: { actions: [{ Transfer: { deposit: "1" } }] } } }],
    } });
  });
  const ep = await startMockEndpoint({ providerId: provider, priceYocto: 1n, allowedWallets: [wallet], log: () => {} });
  const redeem = (hash) => new Promise((resolve, reject) => {
    const req = Readable.from([JSON.stringify({ request_id: "request", prompt: "hello" })]);
    Object.assign(req, { method: "POST", url: "/v1/infer", headers: {
      "x-payment-tx": hash, "x-payment-wallet": wallet,
    } });
    let status;
    const res = {
      writeHead: (code) => { status = code; },
      end: (body) => { JSON.parse(body); resolve(status); },
    };
    handler(req, res).catch(reject);
  });
  try {
    const first = redeem("same-hash");
    await verifying;
    const second = await redeem("same-hash");
    release();
    assert.deepEqual([await first, second].sort(), [200, 409]);
    assert.equal(calls, 1, "only the reserved request verifies");
    assert.equal(ep.stats.served, 1);
    assert.equal(await redeem("same-hash"), 409, "successful reservation persists");
    for (const failure of ["failure", "throw"]) {
      mode = failure;
      assert.equal(await redeem(failure), 402);
      mode = "success";
      assert.equal(await redeem(failure), 200, "failed verification permits a later retry");
      assert.equal(await redeem(failure), 409);
    }
    assert.equal(ep.stats.served, 3);
  } finally {
    release();
    await ep.close();
  }
});
