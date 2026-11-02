// Tiny agent loop: "pays for inference" from its quotum spend-cap wallet until the cap stops it.
// The agent holds ONLY a function-call access key (receiver = its wallet, method = pay, allowance).
import { Account, JsonRpcProvider, actions } from "near-api-js";
import {
  RPC_URL, txLink, rpc, viewAccessKey, allowanceOf, failureOf, panicMsg, gasBurnt, fmtNear, nowIso, eventsOf,
} from "./lib.mjs";

const TGAS = 10n ** 12n;
// Errors that mean "the NEAR protocol refused to admit this tx" (vs. transient RPC failures).
export const PROTOCOL_REFUSAL =
  /NotEnoughAllowance|Not enough allowance|MethodNameMismatch|ReceiverMismatch|DepositWithFunctionCall|RequiresFullAccess|InvalidAccessKeyError|AccessKeyNotFound|does not have permission|method name|doesn't match|full access|deposit/i;

export class Agent {
  constructor({ walletId, privateKey, endpointUrl, payGasTgas = 10, log = console.log }) {
    this.walletId = walletId;
    this.provider = new JsonRpcProvider({ url: RPC_URL });
    this.account = new Account(walletId, this.provider, privateKey);
    this.publicKey = null;
    this.privateKey = privateKey;
    this.endpointUrl = endpointUrl;
    this.payGas = BigInt(payGasTgas) * TGAS;
    this.log = log;
  }

  async pubKey() {
    if (!this.publicKey) {
      const { KeyPair } = await import("near-api-js");
      this.publicKey = KeyPair.fromString(this.privateKey).getPublicKey().toString();
    }
    return this.publicKey;
  }

  async allowance() {
    return allowanceOf(await viewAccessKey(this.walletId, await this.pubKey(), "optimistic"));
  }

  async infer(requestId, prompt, payment) {
    const headers = { "content-type": "application/json" };
    if (payment) {
      headers["x-payment-tx"] = payment;
      headers["x-payment-wallet"] = this.walletId;
    }
    const r = await fetch(`${this.endpointUrl}/v1/infer`, {
      method: "POST",
      headers,
      body: JSON.stringify({ request_id: requestId, prompt }),
    });
    return { status: r.status, body: await r.json() };
  }

  // Sign and send one tx with the agent key. Returns {kind, hash?, ...}.
  //  kind = "ok" | "denied" (contract panic, on-chain failed tx) | "rejected" (protocol refused before inclusion)
  async send(actionList, label, receiverId = this.walletId) {
    const before = await this.allowance().catch(() => null);
    for (let attempt = 0; ; attempt++) {
      const r = await this.sendOnce(actionList, label, receiverId, before);
      // transient RPC trouble (rate limit, timeout) is NOT a protocol refusal: retry, then surface as "error"
      if (r.kind === "rejected" && !PROTOCOL_REFUSAL.test(r.reason)) {
        if (attempt < 4) { await new Promise((ok) => setTimeout(ok, 2000 * 2 ** attempt)); continue; }
        return { ...r, kind: "error" };
      }
      return r;
    }
  }

  async sendOnce(actionList, label, receiverId, before) {
    try {
      const r = await this.account.signAndSendTransaction({
        receiverId,
        actions: actionList,
        waitUntil: "EXECUTED_OPTIMISTIC",
        throwOnFailure: false,
        retries: 1,
      });
      const hash = r.transaction.hash;
      const f = failureOf(r);
      const { gas, tokens } = gasBurnt(r);
      await new Promise((ok) => setTimeout(ok, 1500)); // let the gas-refund receipt credit the allowance
      const after = await this.allowance().catch(() => null);
      return {
        label, kind: f ? "denied" : "ok", hash, link: txLink(hash), reason: f ? panicMsg(f) : null,
        events: eventsOf(r), gas_burnt: gas.toString(), tokens_burnt: tokens.toString(),
        allowance_before: before, allowance_after: after, at: nowIso(),
      };
    } catch (e) {
      const msg = String(e?.message || e);
      const type = e?.type || (msg.match(PROTOCOL_REFUSAL) || [""])[0] || e?.name || "Error";
      const m = msg.match(/allowance: (\d+), cost: (\d+)/);
      return {
        label, kind: "rejected", hash: null, reason: `${type}: ${msg.slice(0, 300)}`, allowance_before: before,
        ...(m ? { allowance_at_check: m[1], tx_cost_required: m[2] } : {}), at: nowIso(),
      };
    }
  }

  payAction(requestId, amount) {
    return actions.functionCall("pay", { request_id: requestId, amount: amount.toString() }, this.payGas, 0n);
  }

  // One full request: quote (402) -> pay on NEAR -> redeem.
  async request(requestId, prompt) {
    const q = await this.infer(requestId, prompt);
    if (q.status !== 402) return { requestId, step: "quote", error: `expected 402, got ${q.status}` };
    const price = BigInt(q.body.price);
    const pay = await this.send([this.payAction(requestId, price)], `pay ${requestId}`);
    if (pay.kind !== "ok") return { requestId, price: price.toString(), pay };
    const served = await this.infer(requestId, prompt, pay.hash);
    return { requestId, price: price.toString(), pay, served };
  }

  // Loop until the wallet refuses (soft cap) or maxRequests is reached.
  async runUntilDenied({ day, maxRequests = 20, prompts }) {
    const out = [];
    for (let i = 1; i <= maxRequests; i++) {
      const rid = `d${day}-r${i}-${Date.now().toString(36)}`;
      const r = await this.request(rid, prompts?.[i % prompts.length] || `prompt ${i}`);
      out.push(r);
      const p = r.pay;
      this.log(
        `[agent] ${rid}: ${p?.kind === "ok" ? `PAID ${fmtNear(r.price, 3)} NEAR  -> HTTP ${r.served?.status}` : `${p?.kind?.toUpperCase()} ${p?.reason}`}` +
          `${p?.hash ? `\n        tx ${p.link}` : ""}  allowance ${p?.allowance_after ? fmtNear(p.allowance_after) : "?"} NEAR`,
      );
      if (!p || p.kind !== "ok") break;
    }
    return out;
  }

  // Misbehaving agent: keep retrying a denied payment. Only the key allowance stops it.
  async retryStorm({ day, max = 40 }) {
    const out = [];
    let settledRetry = false;
    for (let i = 1; i <= max; i++) {
      const rid = `d${day}-storm-${i}`;
      const r = await this.send([this.payAction(rid, 10n ** 22n)], `storm ${i}`);
      out.push(r);
      this.log(
        `[agent] storm #${i}: ${r.kind.toUpperCase()} ${r.kind === "rejected" ? r.reason.slice(0, 120) : ""}${r.hash ? ` tx ${r.hash}` : ""}  allowance ${r.allowance_after ? fmtNear(r.allowance_after) : r.allowance_before ? fmtNear(r.allowance_before) : "?"}`,
      );
      if (r.kind === "error") break;
      if (r.kind === "rejected") {
        // A tx in flight holds its full prepaid-gas cost against the allowance until its refund lands.
        // So wait for refunds to settle and retry once; only a second refusal means the hard cap is reached.
        if (!settledRetry && /allowance/i.test(r.reason)) {
          settledRetry = true;
          this.log(`[agent]   (allowance momentarily short while a refund is in flight; waiting 5 s and retrying once)`);
          await new Promise((ok) => setTimeout(ok, 5000));
          continue;
        }
        break;
      }
      settledRetry = false;
    }
    return out;
  }

  // Things the key must NOT be able to do (protocol-level refusals, no tx is included).
  async probeForbidden(other) {
    const g = 2n * TGAS; // low prepaid gas so the allowance check passes and the real refusal reason surfaces
    const res = [];
    res.push(await this.send([actions.functionCall("set_policy", { daily_cap: "1000000000000000000000000000" }, g, 0n)], "call set_policy (owner method)"));
    res.push(await this.send([actions.functionCall("pay", { request_id: "with-deposit", amount: "1" }, g, 1n)], "pay with 1 yocto attached deposit"));
    res.push(await this.send([actions.transfer(1n)], "plain transfer (1 yocto)"));
    res.push(await this.send([actions.functionCall("pay", { request_id: "x", amount: "1" }, g, 0n)], `call pay on another receiver (${other})`, other));
    for (const r of res) this.log(`[agent] forbidden: ${r.label} -> ${r.kind.toUpperCase()} ${r.reason ? r.reason.slice(0, 140) : ""}`);
    return res;
  }
}
