#!/usr/bin/env python3
"""Independent verifier for a reproduce run (r2, gap G1).

Different method from the run itself: Python stdlib only (no near-api-js), a
different RPC (archival fastnear by default), and every claim re-derived from
chain data. The run JSON is used only as the list of claims (account ids and
tx hashes) to check.

usage: python3 -I verify/verify.py runs/<run-id>.json [path/to/local.wasm]
"""
import base64, hashlib, json, os, sys, urllib.request

RPC = os.environ.get("VERIFY_RPC", "https://archival-rpc.testnet.fastnear.com")
YOCTO = 10**24
ok_count = fail_count = 0


def rpc(method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(RPC, body, {"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        j = json.load(r)
    if "error" in j:
        raise RuntimeError(json.dumps(j["error"])[:300])
    return j["result"]


B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58(b):
    n = int.from_bytes(b, "big")
    s = ""
    while n:
        n, r = divmod(n, 58)
        s = B58[r] + s
    return "1" * (len(b) - len(b.lstrip(b"\0"))) + s


def check(cond, what, detail=""):
    global ok_count, fail_count
    if cond:
        ok_count += 1
        print(f"  PASS {what}")
    else:
        fail_count += 1
        print(f"  FAIL {what} {detail}")


def view(acct, method, args=None):
    r = rpc("query", {"request_type": "call_function", "finality": "final", "account_id": acct,
                      "method_name": method, "args_base64": base64.b64encode(json.dumps(args or {}).encode()).decode()})
    return json.loads(bytes(r["result"]))


def tx(h, sender):
    return rpc("EXPERIMENTAL_tx_status", {"tx_hash": h, "sender_account_id": sender, "wait_until": "FINAL"})


def logs_events(t):
    ev = []
    for ro in t["receipts_outcome"]:
        for l in ro["outcome"]["logs"]:
            if l.startswith("EVENT_JSON:"):
                e = json.loads(l[11:]); e["_executor"] = ro["outcome"]["executor_id"]; ev.append(e)
    return ev


def failure_text(t):
    return json.dumps(t["status"].get("Failure")) if "Failure" in t["status"] else ""


def main():
    run = json.load(open(sys.argv[1]))
    wasm = sys.argv[2] if len(sys.argv) > 2 else None
    steps = {s["name"]: s for s in run["steps"]}
    acc = run["accounts"]; W, P, O = acc["wallet"], acc["provider"], acc["owner"]
    price, cap = int(run["config"]["price"]), int(run["config"]["dailyCap"])
    print(f"verifying {run['id']} against {RPC}")

    st = rpc("status", [])
    check(st["chain_id"] == "testnet", "RPC chain_id is testnet", st["chain_id"])

    a = rpc("query", {"request_type": "view_account", "finality": "final", "account_id": W})
    want = b58(bytes.fromhex(run["config"]["wasm_sha256"]))
    check(a["code_hash"] == want, f"wallet code_hash == base58(sha256 recorded in run) = {want}", a["code_hash"])
    if wasm:
        local = b58(hashlib.sha256(open(wasm, "rb").read()).digest())
        check(a["code_hash"] == local, f"wallet code_hash == local wasm {wasm}", f"{a['code_hash']} vs {local}")

    keys = rpc("query", {"request_type": "view_access_key_list", "finality": "final", "account_id": W})["keys"]
    check(all(k["access_key"]["permission"] != "FullAccess" for k in keys), "wallet has NO full-access key (locked)", keys)
    fc = [k for k in keys if k["public_key"] == acc["agent_public_key"]]
    check(len(keys) == 1 and len(fc) == 1, "exactly one key on wallet: the agent key", [k["public_key"] for k in keys])
    if fc:
        perm = fc[0]["access_key"]["permission"]["FunctionCall"]
        check(perm["receiver_id"] == W and perm["method_names"] == ["pay"], "agent key: receiver = wallet, methods = [pay]", perm)
        check(int(perm["allowance"]) <= int(run["config"]["allowance"]), f"agent key allowance {int(perm['allowance'])/YOCTO:.6f} <= daily hard cap {int(run['config']['allowance'])/YOCTO}")

    s = view(W, "get_status")
    d0 = view(W, "get_day", {"day": 0})
    paid0 = [r for r in steps["day0_loop"]["requests"] if r["pay"]["kind"] == "ok"]
    paid1 = [r for r in steps["day1_loop"]["requests"] if r["pay"]["kind"] == "ok"]
    check(d0 and int(d0["spent"]) == cap and d0["requests"] == len(paid0) and int(d0["unspent"]) == 0,
          f"on-chain day-0 record: spent = cap = {cap/YOCTO}, requests = {len(paid0)}, unspent 0", d0)
    check(s["day"] == 1 and int(s["spent_today"]) == price * len(paid1), "on-chain status: day 1, spent_today = day-1 payments", s)
    check(int(s["total_paid"]) == price * (len(paid0) + len(paid1)), f"on-chain total_paid = {(len(paid0)+len(paid1))} x price", s["total_paid"])

    # provider never signs anything, so its balance = funding + payments exactly
    pa = rpc("query", {"request_type": "view_account", "finality": "final", "account_id": P})
    expect = int(run["config"]["providerFund"]) + price * (len(paid0) + len(paid1))
    check(int(pa["amount"]) == expect, f"provider balance == funding + payments = {expect/YOCTO} NEAR", int(pa["amount"]) / YOCTO)

    for r in paid0 + paid1:
        t = tx(r["pay"]["hash"], W)
        ev = [e for e in logs_events(t) if e["event"] == "paid" and e["_executor"] == W]
        rc = [x for x in t.get("receipts", []) if x["receiver_id"] == P and x["predecessor_id"] == W]
        dep = sum(int(a2.get("Transfer", {}).get("deposit", 0)) for x in rc for a2 in x["receipt"]["Action"]["actions"] if isinstance(a2, dict))
        check("SuccessValue" in t["status"] and t["transaction"]["signer_id"] == W
              and t["transaction"]["public_key"] == acc["agent_public_key"] and ev and ev[0]["data"][0]["request_id"] == r["requestId"]
              and dep == price, f"paid tx {r['pay']['hash'][:10]}…: success, signed by agent key, paid event, {price/YOCTO} NEAR to provider")
    denied = [r["pay"] for r in steps["day0_loop"]["requests"] if r["pay"]["kind"] == "denied"]
    denied += [a2 for a2 in steps["retry_storm"]["attempts"] if a2["kind"] == "denied"]
    for d in denied:
        t = tx(d["hash"], W)
        check("Failure" in t["status"] and "SPENDCAP_DENIED: over daily cap" in failure_text(t)
              and not [x for x in t.get("receipts", []) if x["receiver_id"] == P],
              f"denied tx {d['hash'][:10]}…: on-chain failure SPENDCAP_DENIED, no transfer receipt")
    sb = steps["stranger_bell_early"]
    t = tx(sb["hash"], acc.get("stranger", P))
    check("Failure" in t["status"] and "only the owner may ring" in failure_text(t), "stranger early bell failed on-chain")
    ob = steps["owner_bell"]
    t = tx(ob["hash"], O)
    bell = [e for e in logs_events(t) if e["event"] == "bell"]
    keyacts = [a2 for x in t.get("receipts", []) if x["receiver_id"] == W for a2 in x["receipt"].get("Action", {}).get("actions", [])]
    check("SuccessValue" in t["status"] and bell and bell[0]["data"][0]["day"] == 0
          and any("DeleteKey" in a2 for a2 in keyacts if isinstance(a2, dict)) and any("AddKey" in a2 for a2 in keyacts if isinstance(a2, dict)),
          "owner bell: success, bell event day 0, DeleteKey+AddKey (re-arm) receipt on wallet")

    # protocol refusals are, by construction, not on chain: the agent key nonce counts only included txs
    rej = [a2 for a2 in steps["retry_storm"]["attempts"] if a2["kind"] == "rejected"]
    check(rej and all(a2["hash"] is None for a2 in rej) and "allowance" in rej[-1]["reason"].lower(),
          f"storm ended with {len(rej)} protocol refusal(s) (NotEnoughAllowance), no tx hash")
    if fc:
        n = fc[0]["access_key"]["nonce"]
        check(n % 1_000_000 == len(paid1), f"agent key nonce {n}: exactly {len(paid1)} included tx since re-arm (refused txs left no trace)")
    print(f"\n{ok_count} passed, {fail_count} failed")
    sys.exit(1 if fail_count else 0)


if __name__ == "__main__":
    main()
