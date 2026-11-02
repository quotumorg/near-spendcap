// Mock pay-per-request inference endpoint (fixed price).
// POST /v1/infer {request_id, prompt}
//   no X-Payment-Tx header  -> 402 {price, provider_id, wallet_method}
//   X-Payment-Tx: <tx hash> -> verifies the tx on NEAR testnet via RPC, then 200 {completion}
// Verification (all must hold): tx final + successful; signer = receiver = the paying wallet;
// the wallet emitted a quotum-spendcap "paid" event for this request_id; a Transfer receipt
// wallet -> provider with deposit >= price succeeded; tx hash not used before (replay guard).
import http from "node:http";
import { rpc, eventsOf, fmtNear } from "./lib.mjs";

export function startMockEndpoint({ port = 0, providerId, priceYocto, allowedWallets = null, log = console.log }) {
  const used = new Set();
  const stats = { served: 0, quoted: 0, rejected: 0 };

  async function verify(txHash, walletId, requestId) {
    const r = await rpc("EXPERIMENTAL_tx_status", { tx_hash: txHash, sender_account_id: walletId, wait_until: "FINAL" });
    if (!r.status || !("SuccessValue" in r.status)) return `tx not successful: ${JSON.stringify(r.status).slice(0, 200)}`;
    const t = r.transaction;
    if (t.signer_id !== walletId || t.receiver_id !== walletId) return "tx must be signed by and sent to the paying wallet";
    const ev = eventsOf(r).find(
      (e) => e.executor === walletId && e.standard === "quotum-spendcap" && e.event === "paid" && e.data?.[0]?.request_id === requestId,
    );
    if (!ev) return "no paid event for this request_id";
    if (ev.data[0].provider_id !== providerId) return "paid to a different provider";
    const outcomeById = new Map(r.receipts_outcome.map((o) => [o.id, o.outcome]));
    const transferOk = (r.receipts || []).some((rc) => {
      if (rc.receiver_id !== providerId || rc.predecessor_id !== walletId) return false;
      const acts = rc.receipt?.Action?.actions || [];
      const dep = acts.reduce((s, a) => s + BigInt(a?.Transfer?.deposit || 0), 0n);
      const st = outcomeById.get(rc.receipt_id)?.status;
      return dep >= priceYocto && st && ("SuccessValue" in st || "SuccessReceiptId" in st);
    });
    if (!transferOk) return "no successful transfer >= price to provider";
    return null;
  }

  const server = http.createServer(async (req, res) => {
    const send = (code, obj) => {
      res.writeHead(code, { "content-type": "application/json" });
      res.end(JSON.stringify(obj));
    };
    if (req.method === "GET" && req.url === "/health") return send(200, { ok: true, providerId, price: priceYocto.toString(), stats });
    if (req.method !== "POST" || req.url !== "/v1/infer") return send(404, { error: "not found" });
    let body = "";
    for await (const c of req) body += c;
    let j;
    try {
      j = JSON.parse(body || "{}");
    } catch {
      return send(400, { error: "bad json" });
    }
    const requestId = String(j.request_id || "");
    const wallet = String(req.headers["x-payment-wallet"] || "");
    const txHash = String(req.headers["x-payment-tx"] || "");
    if (!requestId) return send(400, { error: "request_id required" });
    if (!txHash) {
      stats.quoted++;
      return send(402, {
        error: "payment required",
        price: priceYocto.toString(),
        price_near: fmtNear(priceYocto),
        provider_id: providerId,
        how: "call pay({request_id, amount}) on your quotum spend-cap wallet, then retry with X-Payment-Tx and X-Payment-Wallet",
      });
    }
    if (allowedWallets && !allowedWallets.includes(wallet)) return send(403, { error: "unknown wallet" });
    if (used.has(txHash)) {
      stats.rejected++;
      return send(409, { error: "payment tx already used" });
    }
    used.add(txHash); // Reserve before verification yields to another redemption.
    let why;
    try {
      why = await verify(txHash, wallet, requestId);
    } catch (e) {
      why = `verify error: ${e.message.slice(0, 200)}`;
    }
    if (why) {
      used.delete(txHash);
      stats.rejected++;
      log(`[endpoint] reject ${requestId}: ${why}`);
      return send(402, { error: why });
    }
    stats.served++;
    const prompt = String(j.prompt || "");
    return send(200, {
      request_id: requestId,
      model: "mock-llm-0",
      completion: `mock answer #${stats.served} to: ${prompt.slice(0, 60)}`,
      paid_tx: txHash,
    });
  });

  return new Promise((resolve) => {
    server.listen(port, "127.0.0.1", () => {
      const url = `http://127.0.0.1:${server.address().port}`;
      log(`[endpoint] mock inference at ${url}/v1/infer, price ${fmtNear(priceYocto)} NEAR -> ${providerId}`);
      resolve({ url, server, stats, close: () => new Promise((r) => server.close(r)) });
    });
  });
}

// CLI: node agent/mock-endpoint.mjs <provider_id> <price_yocto> [port]
if (import.meta.url === `file://${process.argv[1]}`) {
  const [providerId, price, port] = process.argv.slice(2);
  if (!providerId || !price) {
    console.error("usage: node agent/mock-endpoint.mjs <provider_id> <price_yocto> [port]");
    process.exit(2);
  }
  const { assertTestnet } = await import("./lib.mjs");
  await assertTestnet();
  startMockEndpoint({ providerId, priceYocto: BigInt(price), port: Number(port || 8787) });
}
