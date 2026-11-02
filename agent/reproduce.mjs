// Reproduce the whole demo from zero on NEAR TESTNET:
//  1 fresh owner account from the testnet faucet   2 wallet + provider sub-accounts
//  3 deploy + init the spend-cap contract ON the wallet (adds the agent FC key with allowance)
//  4 lock the wallet (delete its only full-access key)   5 start the mock inference endpoint
//  6 day 0: agent pays until the soft cap denies   7 forbidden-action probes (protocol refusals)
//  8 a stranger tries to ring the bell early (denied)   9 retry storm until the hard cap (allowance) stops it
// 10 owner rings the bell -> next day: caps reset, agent pays again   11 write runs/<id>.json + .md
// Env: WASM, PRICE_NEAR, DAILY_CAP_NEAR, ALLOWANCE_NEAR, PAY_GAS_TGAS, WALLET_FUND_NEAR, LOCK_WALLET, STORM_MAX
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";
import { Account, JsonRpcProvider, KeyPair, actions } from "near-api-js";
import {
  RPC_URL, FAUCET_URL, NETWORK, CREDS_DIR, txLink, accLink, near, fmtNear, rpc, viewKeys, viewAccount, viewCall,
  viewAccessKey, allowanceOf, saveCreds, failureOf, panicMsg, eventsOf, sleep, nowIso, assertTestnet,
} from "./lib.mjs";
import { startMockEndpoint } from "./mock-endpoint.mjs";
import { Agent } from "./agent.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, "..");
const env = process.env;
const CFG = {
  wasm: env.WASM || path.join(ROOT, "contract/target/near/quotum_spendcap.wasm"),
  price: near(env.PRICE_NEAR || "0.01"),
  dailyCap: near(env.DAILY_CAP_NEAR || "0.05"),
  allowance: near(env.ALLOWANCE_NEAR || "0.008"),
  payGasTgas: Number(env.PAY_GAS_TGAS || 5),
  walletFund: near(env.WALLET_FUND_NEAR || "2.3"),
  providerFund: near("0.05"),
  lock: (env.LOCK_WALLET || "1") === "1",
  stormMax: Number(env.STORM_MAX || 40),
  dayLengthSec: Number(env.DAY_LENGTH_SEC || 86400),
};
const TGAS = 10n ** 12n;
const run = { id: `run-${new Date().toISOString().replace(/[:.]/g, "-")}`, network: NETWORK, rpc: RPC_URL, started: nowIso(), steps: [] };
const log = (...a) => console.log(...a);
function step(name, data) {
  run.steps.push({ name, at: nowIso(), ...data });
  if (data?.hash) log(`  tx ${txLink(data.hash)}`);
}

async function send(account, receiverId, acts, label) {
  const r = await account.signAndSendTransaction({ receiverId, actions: acts, waitUntil: "FINAL", throwOnFailure: false });
  const f = failureOf(r);
  const res = { label, hash: r.transaction.hash, link: txLink(r.transaction.hash), ok: !f, reason: f ? panicMsg(f) : null, events: eventsOf(r) };
  log(`- ${label}: ${f ? "FAILED " + res.reason : "ok"}`);
  return res;
}

async function faucetAccount() {
  if (env.OWNER_ACCOUNT_ID) {
    const c = JSON.parse(fs.readFileSync(path.join(CREDS_DIR, `${env.OWNER_ACCOUNT_ID}.owner.json`), "utf8"));
    return { id: env.OWNER_ACCOUNT_ID, kp: KeyPair.fromString(c.private_key), faucet: null };
  }
  const kp = KeyPair.fromRandom("ed25519");
  const id = `qsc-${Date.now().toString(36)}${crypto.randomBytes(2).toString("hex")}.testnet`;
  saveCreds(id, kp, "owner");
  for (let a = 0; a < 4; a++) {
    const r = await fetch(FAUCET_URL, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ newAccountId: id, newAccountPublicKey: kp.getPublicKey().toString() }),
    });
    const t = await r.text();
    if (r.ok) {
      let hash = null;
      try { hash = JSON.parse(t).transaction?.hash; } catch {}
      return { id, kp, faucet: { url: FAUCET_URL, status: r.status, hash } };
    }
    log(`faucet HTTP ${r.status}: ${t.slice(0, 200)}; retrying`);
    await sleep(2000 * 2 ** a);
  }
  throw new Error("faucet failed; set OWNER_ACCOUNT_ID with a funded testnet account (creds in CREDS_DIR)");
}

async function waitAccount(id) {
  for (let i = 0; i < 20; i++) {
    try { return await viewAccount(id); } catch { await sleep(1500); }
  }
  throw new Error(`account ${id} not visible`);
}

async function main() {
  if (!fs.existsSync(CFG.wasm)) {
    const alt = path.join(ROOT, "contract/target/wasm32-unknown-unknown/release/quotum_spendcap.wasm");
    if (fs.existsSync(alt)) CFG.wasm = alt;
    else throw new Error(`wasm not found at ${CFG.wasm}; run ./reproduce.sh (it builds the contract)`);
  }
  const wasm = fs.readFileSync(CFG.wasm);
  run.config = {
    ...Object.fromEntries(Object.entries(CFG).map(([k, v]) => [k, typeof v === "bigint" ? v.toString() : v])),
    wasm: path.relative(ROOT, CFG.wasm),
    wasm_sha256: crypto.createHash("sha256").update(wasm).digest("hex"),
    wasm_bytes: wasm.length,
    node: process.version,
    near_api_js: JSON.parse(fs.readFileSync(path.join(ROOT, "node_modules/near-api-js/package.json"), "utf8")).version,
  };
  const chain = await assertTestnet();
  run.chain = { chain_id: chain.chain_id, protocol_version: chain.protocol_version, node_version: chain.version };
  log(`== quotum spend caps on NEAR ${NETWORK} | ${run.id} | ${RPC_URL} chain_id=${chain.chain_id} protocol ${chain.protocol_version}`);
  log(`   price ${fmtNear(CFG.price, 3)} NEAR, soft cap ${fmtNear(CFG.dailyCap, 3)} NEAR/day, hard cap (key allowance) ${fmtNear(CFG.allowance)} NEAR/day`);
  const provider = new JsonRpcProvider({ url: RPC_URL });

  // 1. owner (seat holder) from faucet
  const owner = await faucetAccount();
  await waitAccount(owner.id);
  const ownerAcc = new Account(owner.id, provider, owner.kp.toString());
  step("owner_created", { account: owner.id, link: accLink(owner.id), faucet: owner.faucet, hash: owner.faucet?.hash || undefined });
  log(`1 owner ${owner.id} (${fmtNear((await viewAccount(owner.id)).amount, 2)} NEAR from faucet)`);

  // 2. sub-accounts
  const walletId = `agent.${owner.id}`;
  const providerId = `infer.${owner.id}`;
  const deployKp = KeyPair.fromRandom("ed25519");
  const providerKp = KeyPair.fromRandom("ed25519");
  const strangerId = `stranger.${owner.id}`;
  const strangerKp = KeyPair.fromRandom("ed25519");
  saveCreds(strangerId, strangerKp, "stranger");
  const agentKp = KeyPair.fromRandom("ed25519");
  saveCreds(walletId, deployKp, "deploy");
  saveCreds(providerId, providerKp, "provider");
  saveCreds(walletId, agentKp, "agent");
  const w = await send(ownerAcc, walletId, [actions.createAccount(), actions.transfer(CFG.walletFund), actions.addFullAccessKey(deployKp.getPublicKey())], `create wallet ${walletId}`);
  step("wallet_created", { account: walletId, ...w });
  const p = await send(ownerAcc, providerId, [actions.createAccount(), actions.transfer(CFG.providerFund), actions.addFullAccessKey(providerKp.getPublicKey())], `create provider ${providerId}`);
  step("provider_created", { account: providerId, ...p });
  const sc = await send(ownerAcc, strangerId, [actions.createAccount(), actions.transfer(CFG.providerFund), actions.addFullAccessKey(strangerKp.getPublicKey())], `create stranger ${strangerId}`);
  step("stranger_created", { account: strangerId, ...sc });

  // 3. deploy + init on the wallet itself (init adds the agent's FC key with the allowance)
  const walletDeployAcc = new Account(walletId, provider, deployKp.toString());
  const initArgs = {
    owner_id: owner.id,
    provider_id: providerId,
    agent_key: agentKp.getPublicKey().toString(),
    agent_allowance: CFG.allowance.toString(),
    daily_cap: CFG.dailyCap.toString(),
    max_price: CFG.price.toString(),
    day_length_sec: CFG.dayLengthSec,
  };
  const d = await send(walletDeployAcc, walletId, [actions.deployContract(wasm), actions.functionCall("new", initArgs, 100n * TGAS, 0n)], "deploy + init spend-cap contract on wallet");
  step("deploy_init", { initArgs, ...d });
  if (!d.ok) throw new Error("deploy failed");

  // 4. lock: delete the deploy full-access key; only the agent FC key remains
  if (CFG.lock) {
    const l = await send(walletDeployAcc, walletId, [actions.deleteKey(deployKp.getPublicKey())], "lock wallet (delete its full-access key)");
    step("lock_wallet", l);
  }
  await sleep(1500);
  const keys = await viewKeys(walletId);
  step("wallet_keys", { keys });
  log(`  wallet keys now: ${keys.map((k) => `${k.public_key.slice(0, 16)}… ${JSON.stringify(k.access_key.permission)}`).join(" | ")}`);
  const st0 = await viewCall(walletId, "get_status");
  step("status_day0_start", { status: st0 });

  // 5. mock endpoint
  const ep = await startMockEndpoint({ providerId, priceYocto: CFG.price, allowedWallets: [walletId] });
  const agent = new Agent({ walletId, privateKey: agentKp.toString(), endpointUrl: ep.url, payGasTgas: CFG.payGasTgas });
  const provBal0 = BigInt((await viewAccount(providerId)).amount);
  step("agent_allowance_start", { allowance: await agent.allowance() });

  try {
    // 6. day 0 loop until soft cap denial
    log(`\n6 DAY 0: agent pays for inference until the cap stops it`);
    const day0 = await agent.runUntilDenied({ day: 0, maxRequests: 12, prompts: ["summarise NEP-141", "what is an access key", "explain allowance", "draft a haiku", "translate 'cap'"] });
    step("day0_loop", { requests: day0 });

    // 7. forbidden actions (protocol refuses; nothing included on-chain)
    log(`\n7 forbidden actions with the agent key`);
    const forb = await agent.probeForbidden(owner.id);
    step("forbidden_probes", { results: forb });

    // 8. stranger rings bell early
    const strAcc = new Account(strangerId, provider, strangerKp.toString());
    const sb = await send(strAcc, walletId, [actions.functionCall("bell", {}, 30n * TGAS, 0n)], `stranger ${strangerId} rings bell early`);
    step("stranger_bell_early", sb);

    // 9. retry storm until allowance is exhausted
    log(`\n9 runaway agent: retry storm on a denied payment until the key allowance (hard cap) runs out`);
    const storm = await agent.retryStorm({ day: 0, max: CFG.stormMax });
    step("retry_storm", { attempts: storm });

    // after hard cap: even a valid-looking request is refused by the protocol
    const after = await agent.request(`d0-after-hardcap-${Date.now().toString(36)}`, "one more?");
    step("request_after_hardcap", after);
    log(`  request after hard cap: ${after.pay?.kind} ${after.pay?.reason?.slice(0, 100) || ""}`);

    // 10. bell -> next day
    log(`\n10 owner rings the daily bell -> day 1`);
    const stBefore = await viewCall(walletId, "get_status");
    const bell = await send(ownerAcc, walletId, [actions.functionCall("bell", {}, 50n * TGAS, 0n)], "owner rings bell");
    step("owner_bell", { status_before: stBefore, ...bell });
    await sleep(1500);
    const st1 = await viewCall(walletId, "get_status");
    const k1 = await viewAccessKey(walletId, agentKp.getPublicKey().toString());
    step("status_day1_start", { status: st1, agent_key_allowance: allowanceOf(k1), day0_record: await viewCall(walletId, "get_day", { day: 0 }) });
    log(`  day ${st1.day}, spent_today ${fmtNear(st1.spent_today)} NEAR, agent allowance re-armed to ${fmtNear(allowanceOf(k1))} NEAR`);
    const day1 = await agent.runUntilDenied({ day: 1, maxRequests: 1, prompts: ["good morning, new day"] });
    step("day1_loop", { requests: day1 });
  } finally {
    await ep.close();
  }

  // 11. summary
  const provBal1 = BigInt((await viewAccount(providerId)).amount);
  const stEnd = await viewCall(walletId, "get_status");
  run.accounts = { owner: owner.id, wallet: walletId, provider: providerId, stranger: strangerId, agent_public_key: agentKp.getPublicKey().toString() };
  run.result = {
    provider_received: (provBal1 - provBal0).toString(),
    endpoint_stats: ep.stats,
    final_status: stEnd,
  };
  run.finished = nowIso();
  const outDir = path.join(ROOT, "runs");
  fs.mkdirSync(outDir, { recursive: true });
  fs.writeFileSync(path.join(outDir, `${run.id}.json`), JSON.stringify(run, (k, v) => (typeof v === "bigint" ? v.toString() : v), 1));
  fs.writeFileSync(path.join(outDir, `${run.id}.md`), summaryMd());
  log(`\nwrote runs/${run.id}.json and .md`);
  log(summaryMd());
}

function summaryMd() {
  const L = [];
  const s = (n) => run.steps.find((x) => x.name === n);
  L.push(`# ${run.id} (NEAR ${NETWORK})`, "");
  L.push(`Accounts: owner [${run.accounts?.owner}](${accLink(run.accounts?.owner)}), wallet [${run.accounts?.wallet}](${accLink(run.accounts?.wallet)}), provider [${run.accounts?.provider}](${accLink(run.accounts?.provider)}).`);
  L.push(`Config: price ${fmtNear(CFG.price, 3)} NEAR, soft cap ${fmtNear(CFG.dailyCap, 3)} NEAR/day, hard cap allowance ${fmtNear(CFG.allowance)} NEAR/day, pay gas ${CFG.payGasTgas} Tgas, wasm sha256 ${run.config?.wasm_sha256}.`, "");
  L.push("| step | result | tx |", "|---|---|---|");
  const row = (a, b, h) => L.push(`| ${a} | ${String(b ?? "").replace(/\|/g, "/").slice(0, 160)} | ${h ? `[${h.slice(0, 10)}…](${txLink(h)})` : "(not included: protocol refused)"} |`);
  for (const n of ["wallet_created", "provider_created", "stranger_created", "deploy_init", "lock_wallet"]) if (s(n)) row(n, s(n).ok ? "ok" : s(n).reason, s(n).hash);
  for (const r of s("day0_loop")?.requests || []) row(`day0 ${r.requestId}`, r.pay?.kind === "ok" ? `PAID, endpoint HTTP ${r.served?.status}, allowance ${fmtNear(r.pay.allowance_after ?? 0)}` : `${r.pay?.kind}: ${r.pay?.reason}`, r.pay?.hash);
  for (const r of s("forbidden_probes")?.results || []) row(`forbidden: ${r.label}`, `${r.kind}: ${(r.reason || "").slice(0, 120)}`, r.hash);
  if (s("stranger_bell_early")) row("stranger rings bell early", s("stranger_bell_early").ok ? "ok (UNEXPECTED)" : s("stranger_bell_early").reason, s("stranger_bell_early").hash);
  for (const r of s("retry_storm")?.attempts || []) row(`storm ${r.label}`, `${r.kind}: ${(r.reason || "").slice(0, 120)}; allowance ${r.allowance_after ? fmtNear(r.allowance_after) : r.allowance_before ? fmtNear(r.allowance_before) : "?"}`, r.hash);
  if (s("request_after_hardcap")) row("request after hard cap", `${s("request_after_hardcap").pay?.kind}: ${(s("request_after_hardcap").pay?.reason || "").slice(0, 120)}`, s("request_after_hardcap").pay?.hash);
  if (s("owner_bell")) row("owner bell (day 0 -> 1)", s("owner_bell").ok ? `ok; ${JSON.stringify(s("owner_bell").events?.find((e) => e.event === "bell")?.data?.[0] || {})}` : s("owner_bell").reason, s("owner_bell").hash);
  for (const r of s("day1_loop")?.requests || []) row(`day1 ${r.requestId}`, r.pay?.kind === "ok" ? `PAID, endpoint HTTP ${r.served?.status}, allowance ${fmtNear(r.pay.allowance_after ?? 0)}` : `${r.pay?.kind}: ${r.pay?.reason}`, r.pay?.hash);
  L.push("", `Provider received ${fmtNear(run.result?.provider_received ?? 0)} NEAR; endpoint stats ${JSON.stringify(run.result?.endpoint_stats)}.`);
  return L.join("\n") + "\n";
}

main().catch((e) => {
  console.error("FAILED:", e);
  try {
    fs.mkdirSync(path.join(ROOT, "runs"), { recursive: true });
    run.error = String(e?.stack || e);
    fs.writeFileSync(path.join(ROOT, "runs", `${run.id}.failed.json`), JSON.stringify(run, (k, v) => (typeof v === "bigint" ? v.toString() : v), 1));
  } catch {}
  process.exit(1);
});
