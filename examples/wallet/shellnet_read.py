#!/usr/bin/env python3
"""Read an AVM account or AVM events from a shellnet Block Manager.

usage: shellnet_read.py account <reader-url> <dapp_id:account_id>
       shellnet_read.py events <reader-url> <account_id> <event_name>

`account` prints the account read back by `GET /v2/account` as JSON: its
state (`absent`, `no_code`, `active` or `frozen`), its balances by ECC id,
its code hash and its last transaction logical time. `events` prints the
`blockchain { avm_events }` rows of one source account and event name.
"""

import base64
import json
import sys
import urllib.parse
import urllib.request


def fetch(url, body=None):
    headers = {"Accept": "application/json", "User-Agent": "avm-sdk-example"}
    if body is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=body, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def decode_account(payload):
    """The AVM account payload of `/v2/account`: the tag 02000000, then fixed-width
    little-endian fields; an option is a 0 byte or a 1 byte and its value, a
    list is its u64 length and its items."""
    offset = 0

    def take(size):
        nonlocal offset
        if offset + size > len(payload):
            raise ValueError("truncated AVM account payload")
        offset += size
        return payload[offset - size:offset]

    def number(size):
        return int.from_bytes(take(size), "little")

    def option(read):
        return read() if number(1) else None

    if take(4) != b"\x02\x00\x00\x00":
        raise ValueError("not an AVM account payload")
    account_hash = take(32).hex()
    account_id = take(32).hex()
    storage_bytes = number(4)
    storage_last_paid = number(4)
    storage_due = option(lambda: number(16))
    last_trans_lt = number(8)
    balances = {str(number(4)): str(number(16)) for _ in range(number(8))}
    option(lambda: take(32))  # init code hash, equal to the code hash
    state = number(4)
    if state == 2:
        take(32)
    code_hash = option(lambda: take(32).hex())
    option(lambda: take(32))  # data hash
    data = option(lambda: take(number(8)))
    if offset != len(payload) or state not in (0, 1, 2):
        raise ValueError("malformed AVM account payload")
    return {
        "state": {0: "no_code", 1: "active", 2: "frozen"}[state],
        "account_hash": account_hash, "account_id": account_id,
        "balances": balances, "code_hash": code_hash,
        "last_trans_lt": last_trans_lt, "storage_bytes": storage_bytes,
        "storage_last_paid": storage_last_paid,
        "storage_due_payment": None if storage_due is None else str(storage_due),
        "data": None if data is None else data.hex(),
    }


def account(reader, address):
    dapp_id, account_id = address.split(":")
    query = urllib.parse.urlencode({"account_id": account_id, "dapp_id": dapp_id})
    status, body = fetch(f"{reader.rstrip('/')}/v2/account?{query}")
    if status == 404:
        return {"state": "absent"}
    if status != 200:
        raise SystemExit(f"GET /v2/account: HTTP {status}: {body[:200]!r}")
    payload = base64.b64decode(json.loads(body)["boc"])
    if not payload.startswith(b"\x02\x00\x00\x00"):
        # A TVM account of the same id answers when no AVM account exists.
        return {"state": "absent"}
    return decode_account(payload)


def events(reader, account_id, event_name):
    query = ("query($account: String, $name: String) { blockchain { avm_events("
             "src_account: $account, event_name: $name) { edges { node { chain_order "
             "time kind event_name event_class src_dapp_id src_account topics "
             "fields { name type value } } } } } }")
    body = json.dumps({"query": query, "variables": {"account": account_id, "name": event_name}})
    status, answer = fetch(f"{reader.rstrip('/')}/graphql", body.encode())
    if status != 200:
        raise SystemExit(f"POST /graphql: HTTP {status}: {answer[:200]!r}")
    answer = json.loads(answer)
    if answer.get("errors"):
        raise SystemExit(f"POST /graphql: {answer['errors']}")
    return [edge["node"] for edge in answer["data"]["blockchain"]["avm_events"]["edges"]]


def main(arguments):
    if len(arguments) == 3 and arguments[0] == "account":
        result = account(arguments[1], arguments[2])
    elif len(arguments) == 4 and arguments[0] == "events":
        result = events(*arguments[1:])
    else:
        raise SystemExit(__doc__)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main(sys.argv[1:])
