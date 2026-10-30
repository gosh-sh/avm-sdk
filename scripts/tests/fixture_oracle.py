"""Bounded independent oracle for the six public SDK qualification journeys.

Test code only. No production AVM crate, stand output encoder or trie is used.
The primitive encodings come from canonical-encoding.md,
addressing-and-account-state.md, code-library.md and event-log.md/.capnp.
This is neither a state backend nor a general transaction interpreter.
"""

import struct

from blake3 import blake3


def h(data):
    return blake3(data).digest()


def le(value, width):
    return value.to_bytes(width, "little")


def map_root(entries, width, compact):
    assert 0 < width <= 64 and len(entries) <= 32, "fixture bound exceeded"
    rows = sorted(entries.items())
    assert all(len(key) == width for key, _ in rows)
    prefix = b"AVM_MULTI_MAP_COMPACT_" if compact else b"AVM_MULTI_MAP_"
    suffix = b"_V1" if compact else b""

    def hashed(kind, data):
        return h(prefix + kind + suffix + le(width, 2) + data)

    def nibble(key, depth):
        return (key[depth // 2] >> (4 if depth % 2 == 0 else 0)) & 15

    def descend(items, depth):
        if depth == width * 2:
            assert len(items) == 1
            key, value = items[0]
            return hashed(b"LEAF", key + le(len(value), 4) + value)
        if compact:
            end = depth
            while end < width * 2 and nibble(items[0][0], end) == nibble(items[-1][0], end):
                end += 1
            if end != depth:
                path = bytearray((end - depth + 1) // 2)
                for index in range(end - depth):
                    path[index // 2] |= nibble(items[0][0], depth + index) << (4 if index % 2 == 0 else 0)
                return hashed(b"PATH", le(depth, 2) + le(end, 2) + path + descend(items, end))
        groups = {}
        for item in items:
            groups.setdefault(nibble(item[0], depth), []).append(item)
        bitmap = sum(1 << digit for digit in groups)
        return hashed(b"BRANCH", le(depth, 2) + le(bitmap, 2)
                      + b"".join(descend(groups[digit], depth + 1) for digit in sorted(groups)))

    return descend(rows, 0) if rows else hashed(b"EMPTY", bytes(2))


def account_image(selection, extra, data):
    assert len(selection) == 32 and len(data) <= 65540
    return (b"\0" + selection + le(len(extra), 2)
            + b"".join(le(key, 4) + le(value, 16) for key, value in sorted(extra.items()))
            + le(len(data), 4) + data)


def account_leaf(address, image, transaction_lt, storage_last_paid, due=0, unpaid=0):
    assert len(address) == 64
    return (address + h(image) + le(transaction_lt, 8) + b"\x01" + image[1:33]
            + le(image[0], 4) + le(storage_last_paid, 4) + le(due, 16) + le(unpaid, 1))


def event_bytes(source, tx_hash, lt, time, seq, kind, event_class, name, topics, fields):
    """Encode only the schema shapes used by these fixtures in canonical order.

    A word-list builder applies the documented raw canonical Cap'n Proto
    pointer offsets, struct trimming and composite-list layout independently.
    It has no parser, schema generation, RPC, far pointers or arbitrary types.
    """
    assert len(source) == 64 and len(tx_hash) == 32 and kind in (0, 1)
    assert len(fields) <= 7 and len(topics) <= 2
    assert [field[0] for field in fields] == sorted({field[0] for field in fields})

    def obj(data=(), children=()):
        data, children = list(data), list(children)
        while data and data[-1] == 0:
            data.pop()
        while children and children[-1] is None:
            children.pop()
        return ("struct", data, children)

    def blob(value, text=False):
        return ("bytes", value.encode() + b"\0" if text else value)

    def value(tag, data):
        if tag == "u64":
            return obj([2, data])
        if tag == "u128":
            return obj([4], [obj([data & ((1 << 64) - 1), data >> 64])])
        if tag == "data":
            return obj([7], [blob(data)])
        assert tag == "address" and len(data) == 64
        return obj([8], [obj([], [blob(data[:32]), blob(data[32:])])])

    members = [obj([], [blob(key, True), value(tag, data)]) for key, tag, data in fields]
    root = obj([lt, time | (seq << 32), kind, event_class],
               [blob(source[:32]), blob(source[32:]), blob(tx_hash), blob(name, True),
                ("pointers", [blob(topic) for topic in topics]), ("fields", members)])
    words = [0]

    def emit(pointer, item):
        if item is None:
            return
        start = len(words)
        offset = start - pointer - 1
        if item[0] == "bytes":
            data = item[1]
            words[pointer] = 1 | (offset << 2) | (2 << 32) | (len(data) << 35)
            padded = data + bytes((-len(data)) % 8)
            words.extend(int.from_bytes(padded[i:i + 8], "little") for i in range(0, len(padded), 8))
        elif item[0] == "struct":
            _, data, children = item
            if not data and not children:
                words[pointer] = 0xfffffffc  # canonical zero-sized struct
                return
            words[pointer] = (offset << 2) | (len(data) << 32) | (len(children) << 48)
            words.extend(data + [0] * len(children))
            for index, child in enumerate(children):
                emit(start + len(data) + index, child)
        elif item[0] == "pointers":
            children = item[1]
            words[pointer] = 1 | (offset << 2) | (6 << 32) | (len(children) << 35)
            words.extend([0] * len(children))
            for index, child in enumerate(children):
                emit(start + index, child)
        else:
            assert item[0] == "fields"
            children = item[1]
            words[pointer] = 1 | (offset << 2) | (7 << 32) | (len(children) * 2 << 35)
            words.append((len(children) << 2) | (2 << 48))
            words.extend([0] * (len(children) * 2))
            for index, (_, data, pointers) in enumerate(children):
                assert not data and len(pointers) == 2
                for slot, child in enumerate(pointers):
                    emit(start + 1 + index * 2 + slot, child)

    emit(0, root)
    return b"".join(le(word, 8) for word in words)


def decode_state(data):
    """Read only task-owned developer state; never expose its signing secrets."""
    assert data[:8] == b"AVMLSF01" and data[10:12] == bytes(2)
    position = 308

    def take(size):
        nonlocal position
        assert 0 <= size <= len(data) - position
        result = data[position:position + size]
        position += size
        return result

    def number(width=4):
        return int.from_bytes(take(width), "little")

    def vector():
        return take(number())

    def byte_map(width):
        count = number()
        assert count <= 32
        result = {}
        for _ in range(count):
            key, value = take(width), vector()
            assert key not in result
            result[key] = value
        return result

    contents = {"registry": byte_map(33), "code": byte_map(32), "abi": byte_map(32)}
    assert number() == 0, "all prefunded fixture accounts must have deployed"
    contents.update(accounts={}, account_leaves={})
    count = number()
    assert count == 2, "these journeys have exactly two complete accounts"
    for _ in range(count):
        address, claimed = take(64), take(32)
        lt, paid, due, unpaid = number(8), number(), number(16), number(1)
        image = vector()
        assert h(image) == claimed and address not in contents["accounts"]
        contents["accounts"][address] = image
        contents["account_leaves"][address] = account_leaf(address, image, lt, paid, due, unpaid)
    take(number() * 104)  # nonces: public replay also validates each signature/order
    assert number() == 0, "public fixture operations must drain the queue"
    receipts = number()
    take(receipts * 136)
    transcript = [vector() for _ in range(number())]
    assert len(transcript) == receipts
    times = [number() for _ in range(number())]
    assert len(times) == receipts
    assert number() == 0, "no private internal-message injection is allowed"
    contents["events"] = [vector() for _ in range(number())]
    contents["rings"] = {}
    for _ in range(number()):
        source, event_class, record = take(64), take(8), vector()
        key = h(b"AVM_INTERNAL_EVENT_RING_KEY" + source + event_class)
        assert key not in contents["rings"]
        contents["rings"][key] = record
    return contents, {"registry_root": data[172:204].hex(),
                      "internal_event_ring_root": data[204:236].hex()}, transcript, times


def verify_contents(actual, expected, claimed_roots, compact):
    assert actual.keys() == expected.keys()
    for field in expected:
        assert actual[field] == expected[field], "independent fixture content mismatch: " + field
    roots = {"account_state_root": map_root(expected["account_leaves"], 64, compact).hex(),
             "registry_root": map_root(expected["registry"], 33, compact).hex(),
             "internal_event_ring_root": map_root(expected["rings"], 32, compact).hex()}
    assert claimed_roots == roots, "independent fixture root mismatch"
    return roots


def expected_journey(bundle, label, identities, transcript, times, current):
    """Construct the complete expected leaves from public fixture inputs.

    Only the two signed deploy inputs supply their signature bytes for event
    message identities. Their complete unsigned headers/bodies are checked
    here; the public stand admission/replay checks signatures. No account,
    registry, event bytes, metadata or claimed root are used as an expectation.
    All times, transfers, rent sizes and transaction LTs below are fixed by the
    six journeys, not learned from a candidate or a saved successful run.
    """
    labels = {"version-upgrade", "contract-b", "contract-batch-patch",
              "trap", "out-of-fuel", "invalid-event"}
    assert label in labels
    version = label == "version-upgrade"
    batch = label == "contract-batch-patch"
    failure = label in {"trap", "out-of-fuel", "invalid-event"}
    second = "contract-a-v2" if version else "contract-b-failure" if failure else label
    names = ["contract-a-v1", second]
    artifacts = [{key: (bundle / "examples" / name / "target/avm" / ("contract." + key)).read_bytes()
                  for key in ("wasm", "acab", "descriptor")} for name in names]
    codes = [h(item["wasm"]) for item in artifacts]
    descriptors = [h(item["descriptor"]) for item in artifacts]
    abis = [h(b"AVM_ABI" + item["acab"]) for item in artifacts]
    package = h(b"AVM_CODE_PACKAGE" + codes[0])
    selections = [h(b"AVM_CODE_SELECTION" + package + code + descriptor)
                  for code, descriptor in zip(codes, descriptors)]
    user = bytes.fromhex(identities["user"])
    owner = bytes.fromhex(identities["package_owner"])
    developer = bytes.fromhex(identities["developer"])
    addresses = [b"\x11" * 32 + h(b"AVM_ACCOUNT_ID\0\0" + package + user
                 + le(len(name), 2) + name.encode() + h(b"")) for name in names]
    first, second_address = addresses
    expected = {"accounts": {}, "account_leaves": {}, "registry": {},
                "code": dict(zip(codes, [a["wasm"] for a in artifacts])),
                "abi": dict(zip(abis, [a["acab"] for a in artifacts])), "events": [], "rings": {}}
    registry = expected["registry"]
    registry[b"\x01" + package] = package + codes[0] + b"\x01" + owner + selections[0]
    for index, (selection, code, descriptor, abi, artifact) in enumerate(zip(selections, codes, descriptors, abis, artifacts)):
        count = (0 if index == 0 else 2) if version and current else 1
        registry[b"\x02" + selection] = selection + package + code + descriptor + artifact["descriptor"] + b"\0"
        registry[b"\x03" + selection] = selection + le(count, 8)
        registry[b"\x04" + code] = code + le(count, 8)
        registry[b"\x05" + selection] = selection + abi

    start = 1_800_000_000
    offsets = [1, 2, 4, 5, 6, 7, 8] if version else [1, 2, 3, 3, 4 if batch else 3]
    assert times == [start + offset for offset in offsets] and len(transcript) == len(offsets)

    def external(wire, index, pubkey, nonce, at, method, body, deploy=False, extras=()):
        prefix = struct.pack("<BBHIQ", 1, 3 if deploy else 1, len(extras), len(body), method)
        prefix += le(1, 16) + le(nonce, 8) + le(at, 4) + le(at + 3600, 4)
        prefix += addresses[index] + pubkey + le(64, 2) + bytes(46)
        prefix += b"".join(le(key, 4) + le(value, 16) for key, value in extras) + body
        assert wire[:-64] == prefix and len(wire) == len(prefix) + 64, "public external fixture input differs"
        assert wire[-64:] != bytes(64)

    for index, transcript_index in enumerate((0, 2 if version else 1)):
        artifact, name, address = artifacts[index], names[index], addresses[index]
        body = b"ACDP" + le(0x17 if index == 0 else 2, 2) + bytes(2)
        body += selections[index] + package + codes[index] + descriptors[index] + user + h(b"")
        body += (owner if index == 0 else bytes(32)) + abis[index]
        body += bytes(2) + le(len(name), 2) + bytes(4)
        body += le(len(artifact["wasm"]), 4) + le(len(artifact["descriptor"]), 4) + le(len(artifact["acab"]), 4) + bytes(4)
        body += name.encode() + artifact["wasm"] + artifact["descriptor"] + artifact["acab"]
        wire = transcript[transcript_index]
        external(wire, index, developer if index == 0 else owner, 1, times[transcript_index], 0, body, deploy=True)
        fields = [("account", "address", address), ("init_data_hash", "data", h(b"")),
                  ("owner", "data", user), ("package", "data", package),
                  ("salt", "data", name.encode()), ("selected_code", "data", selections[index]),
                  ("ui_name", "data", b"")]
        lt = 2 if index == 0 else 5 if version else 4
        expected["events"].append(event_bytes(address, h(b"AVM_EXTERNAL_MESSAGE_ID" + wire),
            lt, times[transcript_index], 0, 0, int.from_bytes(h(b"avm.account.init")[:8], "little"),
            "avm.account.init", [], fields))

    def account(index, selection, ecc0, shells, extra, data, lt, paid):
        currencies = {2: shells, **extra}
        if ecc0:
            currencies[0] = ecc0
        image = account_image(selection, currencies, data)
        expected["accounts"][addresses[index]] = image
        expected["account_leaves"][addresses[index]] = account_leaf(addresses[index], image, lt, start + paid)

    # Config prices: publication=1000, init=10, external delivery=1,
    # requested FUEL=1, callback=1, rent=canonical_image_bytes*elapsed_seconds.
    funded = 1_000_000 - (1000 + 10 + 1 + 1)
    if version:
        version_method = int.from_bytes(h(b"causal.a.version")[:8], "little")
        external(transcript[1], 0, user, 1, start + 2, version_method, b"")
        migration = b"avm-a-v2-migrate-state!!"
        for pos, pubkey, nonce, payload in ((3, user, 2, migration),
                                          (4, owner, 1, b"avm-a-v2-migrate-state!?"),
                                          (5, owner, 2, migration)):
            body = b"ACUG" + selections[1] + codes[1] + le(len(payload), 4) + payload
            external(transcript[pos], 0, pubkey, nonce, times[pos], 0xc2c6c51bc610249d, body)
        external(transcript[6], 0, user, 3, start + 8, version_method, b"")
        state = bytearray(72)
        if current:
            state[16:20] = b"V1MG"
            state[20:24] = le(24, 4)
            state[24:48] = migration
            state[48:72] = migration
        state[16] = 2 if current else 1
        account(0, selections[1] if current else selections[0], 0,
                funded - 5 * 2 - (39 + 20 + 72) * 7, {}, bytes(state), 9, 8)
        account(1, selections[1], 0, funded, {}, bytes(72), 5, 4)
        return expected

    method_name = "causal.batch.patch" if batch else "causal.b.drain" if label in {"trap", "out-of-fuel"} else "causal.b.receive"
    method = h(method_name.encode())[:8]
    handler = bytes(8) if batch else h(b"causal.a.callback")[:8] if not failure else (
        h(b"causal.a.version")[:8] if label == "out-of-fuel" else le(0xc2c6c51bc610249d, 8))
    forward = second_address + method + handler
    external(transcript[2], 0, user, 1, start + 3,
             int.from_bytes(h(b"causal.a.forward")[:8], "little"), forward, extras=[(0, 100), (7, 40)])
    delivered = struct.pack("<BBHI", 0, 0 if batch else 4, 2, 16) + method
    delivered += le(1 if batch else 0, 16) + le(6, 8) + le(start + 3, 4) + bytes(4)
    delivered += first + second_address + (b"" if batch else handler) + le(0, 4) + le(37, 16) + le(7, 4) + le(11, 16) + b"\x42" * 16
    assert transcript[3] == delivered, "complete VM-produced message differs"
    sender = bytearray(le(1, 8) + bytes(64))
    sender_shells = funded - (39 + 20 + 72) * 2 - 2 - 1
    if batch:
        external(transcript[4], 1, user, 1, start + 4, int.from_bytes(method, "little"), b"")
        receiver = le(65536, 4) + le(1, 8) + bytes(65528)
        account(0, selections[0], 63, sender_shells, {7: 29}, bytes(sender), 5, 3)
        account(1, selections[1], 37, funded - (39 + 20 + 65540) - (39 + 60 + 65540) - 2,
                {7: 11}, receiver, 8, 4)
    else:
        status = {"trap": 30, "out-of-fuel": 31, "invalid-event": 44}.get(label, 0)
        callback = struct.pack("<BBHI", 0, 1, 2 if failure else 0, 0 if failure else 16) + handler
        callback += le(1, 16) + le(8, 8) + le(start + 3, 4) + bytes(4)
        callback += second_address + first + h(delivered) + first + le(status, 4)
        callback += le(0, 4) + le(37, 16) + le(7, 4) + le(11, 16) if failure else b"\x99" * 16
        assert transcript[4] == callback, "complete VM-produced callback differs"
        if failure:
            receiver = bytes(128)
        else:
            sender[8:16], sender[20:24], sender[24:40], sender[40:72] = le(1, 8), le(16, 4), b"\x99" * 16, h(delivered)
            receiver = le(1, 8) + first + le(37, 16) + le(1, 4) + le(7, 4) + le(11, 16) + method + le(16, 4) + b"\x42" * 4
            for seq, kind, event_class, name, topics, fields in (
                (0, 0, 0x71228a3dc1799631, "causal.a.forwarded", [second_address[32:]],
                 [("count", "u64", 1), ("destination", "address", second_address), ("value_ecc0", "u128", 37)]),
                (1, 1, 0x127e58f93e9d40f8, "causal.a.state", [le(1, 8)],
                 [("count", "u64", 1), ("state", "data", le(1, 8) + bytes(64))]),
            ):
                event = event_bytes(second_address, h(delivered), 7, start + 3, seq, kind, event_class, name, topics, fields)
                expected["events"].append(event)
                if kind == 1:
                    key = h(b"AVM_INTERNAL_EVENT_RING_KEY" + second_address + le(event_class, 8))
                    expected["rings"][key] = b"AIER\x01\0\0\0" + le(len(event), 4) + event
        account(0, selections[0], 100 if failure else 63, sender_shells,
                {7: 40 if failure else 29}, bytes(sender), 9, 3)
        account(1, selections[1], 0 if failure else 37, funded - (39 + 20 + 128),
                {} if failure else {7: 11}, receiver, 7, 3)
    return expected
