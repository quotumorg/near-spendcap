// Shared helpers: config, raw JSON-RPC, credentials (kept OUTSIDE the repo), links.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const NETWORK = "testnet"; // hard-coded on purpose: this demo never touches mainnet
export const RPC_URL = process.env.NEAR_RPC_URL || "https://test.rpc.fastnear.com"; // rpc.testnet.near.org now answers -429 "deprecated"
export const FAUCET_URL = process.env.NEAR_FAUCET_URL || "https://helper.testnet.near.org/account";
export const EXPLORER = "https://testnet.nearblocks.io";

if (/mainnet/i.test(RPC_URL)) throw new Error(`refusing mainnet RPC ${RPC_URL}`);

// Hard guard: the RPC must report chain_id "testnet" before anything is signed.
export async function assertTestnet() {
  const s = await rpc("status", []);
  if (s.chain_id !== "testnet") throw new Error(`RPC ${RPC_URL} reports chain_id ${s.chain_id}; this demo runs on testnet only`);
  return s;
}

// Throwaway testnet keys live here, never inside the repo.
export const CREDS_DIR =
  process.env.SPENDCAP_CREDS_DIR || path.join(os.homedir(), ".near-credentials-quotum-spendcap", NETWORK);

export const txLink = (h) => `${EXPLORER}/txns/${h}`;
export const accLink = (a) => `${EXPLORER}/address/${a}`;

export const YOCTO_PER_NEAR = 10n ** 24n;
export function near(s) {
  // decimal NEAR string -> yocto bigint
  const [i, f = ""] = String(s).split(".");
  return BigInt(i) * YOCTO_PER_NEAR + BigInt((f + "0".repeat(24)).slice(0, 24) || "0");
}
export function fmtNear(y, d = 6) {
  y = BigInt(y);
  const neg = y < 0n;
  if (neg) y = -y;
  const i = y / YOCTO_PER_NEAR;
  const f = (y % YOCTO_PER_NEAR).toString().padStart(24, "0").slice(0, d);
  return `${neg ? "-" : ""}${i}.${f}`;
}

let rpcId = 0;
export async function rpc(method, params, { retries = 4 } = {}) {
  for (let attempt = 0; ; attempt++) {
    try {
      const r = await fetch(RPC_URL, {
        method: "POST",
        signal: AbortSignal.timeout(30_000),
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: ++rpcId, method, params }),
      });
      if (r.status === 429 || r.status >= 500) throw new Error(`HTTP ${r.status}`);
      const j = await r.json();
      if (j.error && (j.error.code === -429 || /rate limit|too many/i.test(JSON.stringify(j.error)))) throw new Error(`rate limited: ${JSON.stringify(j.error).slice(0, 120)}`);
      if (j.error) {
        const e = new Error(JSON.stringify(j.error));
        e.rpc = j.error;
        throw e;
      }
      return j.result;
    } catch (e) {
      if (e.rpc || attempt >= retries) throw e;
      await sleep(1000 * 2 ** attempt);
    }
  }
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function viewAccessKey(accountId, publicKey, finality = "final") {
  try {
    return await rpc("query", {
      request_type: "view_access_key",
      finality,
      account_id: accountId,
      public_key: publicKey,
    });
  } catch (e) {
    if (String(e.message).includes("UNKNOWN_ACCESS_KEY") || String(e.message).includes("does not exist")) return null;
    throw e;
  }
}

export async function viewKeys(accountId) {
  const r = await rpc("query", { request_type: "view_access_key_list", finality: "final", account_id: accountId });
  return r.keys;
}

export async function viewAccount(accountId) {
  return rpc("query", { request_type: "view_account", finality: "final", account_id: accountId });
}

export async function viewCall(contractId, method, args = {}, finality = "final") {
  const r = await rpc("query", {
    request_type: "call_function",
    finality,
    account_id: contractId,
    method_name: method,
    args_base64: Buffer.from(JSON.stringify(args)).toString("base64"),
  });
  return JSON.parse(Buffer.from(r.result).toString());
}

export function allowanceOf(keyView) {
  const p = keyView?.permission;
  if (!p || p === "FullAccess") return null;
  return p.FunctionCall?.allowance ?? null;
}

export function saveCreds(accountId, keyPair, role) {
  fs.mkdirSync(CREDS_DIR, { recursive: true, mode: 0o700 });
  const file = path.join(CREDS_DIR, `${accountId}.${role}.json`);
  fs.writeFileSync(
    file,
    JSON.stringify(
      { account_id: accountId, role, public_key: keyPair.getPublicKey().toString(), private_key: keyPair.toString() },
      null,
      1,
    ),
    { mode: 0o600 },
  );
  return file;
}

export function loadCreds(accountId, role) {
  const file = path.join(CREDS_DIR, `${accountId}.${role}.json`);
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

// Parse NEP-297 events from all receipt outcomes of a tx result.
export function eventsOf(txResult) {
  const out = [];
  for (const ro of txResult?.receipts_outcome || []) {
    for (const l of ro.outcome.logs || []) {
      if (l.startsWith("EVENT_JSON:")) {
        try {
          out.push({ executor: ro.outcome.executor_id, ...JSON.parse(l.slice(11)) });
        } catch {}
      }
    }
  }
  return out;
}

export function failureOf(txResult) {
  const st = txResult?.status;
  if (st && typeof st === "object" && "Failure" in st) return st.Failure;
  for (const ro of txResult?.receipts_outcome || []) {
    const s = ro.outcome.status;
    if (s && typeof s === "object" && "Failure" in s) return s.Failure;
  }
  return null;
}

export function panicMsg(failure) {
  const s = JSON.stringify(failure || "");
  const m = s.match(/Smart contract panicked: ([^"\\]+)/);
  return m ? m[1] : s;
}

export function gasBurnt(txResult) {
  let g = BigInt(txResult?.transaction_outcome?.outcome?.gas_burnt || 0);
  let t = BigInt(txResult?.transaction_outcome?.outcome?.tokens_burnt || 0);
  for (const ro of txResult?.receipts_outcome || []) {
    g += BigInt(ro.outcome.gas_burnt || 0);
    t += BigInt(ro.outcome.tokens_burnt || 0);
  }
  return { gas: g, tokens: t };
}

export function nowIso() {
  return new Date().toISOString();
}
