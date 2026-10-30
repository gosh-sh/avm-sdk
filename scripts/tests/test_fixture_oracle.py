"""Independent byte assertions for the bounded packaged-lifecycle oracle.

Preimages are written out here from canonical-encoding.md, not obtained from
the stand or the oracle implementation. The event vector is the immutable
EXTERNAL_EVENT_HEX in accepted avm-event-capnp-codec build_event_log_vectors.rs.
"""

import copy
import unittest

from blake3 import blake3

from fixture_oracle import account_image, account_leaf, event_bytes, map_root, verify_contents


def h(data):
    return blake3(data).digest()


class FixtureOracleTests(unittest.TestCase):
    def test_empty_and_singleton_roots_use_the_selected_domains_and_maximal_path(self):
        for compact in (False, True):
            empty = b"AVM_MULTI_MAP_COMPACT_EMPTY_V1" if compact else b"AVM_MULTI_MAP_EMPTY"
            self.assertEqual(map_root({}, 1, compact), h(empty + b"\x01\0\0\0"))
        leaf = h(b"AVM_MULTI_MAP_COMPACT_LEAF_V1\x01\0\xab\x01\0\0\0\xcd")
        expected = h(b"AVM_MULTI_MAP_COMPACT_PATH_V1\x01\0\0\0\x02\0\xab" + leaf)
        self.assertEqual(map_root({b"\xab": b"\xcd"}, 1, True), expected)
        leaf = h(b"AVM_MULTI_MAP_LEAF\x01\0\xab\x01\0\0\0\xcd")
        child = h(b"AVM_MULTI_MAP_BRANCH\x01\0\x01\0\0\x08" + leaf)
        expected = h(b"AVM_MULTI_MAP_BRANCH\x01\0\0\0\0\x04" + child)
        self.assertEqual(map_root({b"\xab": b"\xcd"}, 1, False), expected)

    def test_compact_odd_path_and_branch_commit_each_key_and_value_byte(self):
        entries = {b"\xa1": b"first", b"\xa2": b"second"}
        first = h(b"AVM_MULTI_MAP_COMPACT_LEAF_V1\x01\0\xa1\x05\0\0\0first")
        second = h(b"AVM_MULTI_MAP_COMPACT_LEAF_V1\x01\0\xa2\x06\0\0\0second")
        branch = h(b"AVM_MULTI_MAP_COMPACT_BRANCH_V1\x01\0\x01\0\x06\0" + first + second)
        expected = h(b"AVM_MULTI_MAP_COMPACT_PATH_V1\x01\0\0\0\x01\0\xa0" + branch)
        self.assertEqual(map_root(entries, 1, True), expected)
        self.assertNotEqual(map_root(entries, 1, False), expected)
        for changed in ({b"\xb1": b"first", b"\xa2": b"second"},
                        {b"\xa1": b"First", b"\xa2": b"second"}):
            self.assertNotEqual(map_root(changed, 1, True), expected)
        with self.assertRaises(AssertionError):
            map_root({b"\x01\x02": b"bad width"}, 1, True)

    def test_full_account_body_and_index_leaf_have_independent_byte_layouts(self):
        address, selection = bytes(range(64)), bytes(range(32))
        image = b"\0" + selection + b"\x03\0\0\0\0\0"
        image += (37).to_bytes(16, "little")
        image += b"\x02\0\0\0" + (123).to_bytes(16, "little")
        image += b"\x07\0\0\0" + (11).to_bytes(16, "little") + b"\x03\0\0\0abc"
        self.assertEqual(account_image(selection, {0: 37, 2: 123, 7: 11}, b"abc"), image)
        leaf = address + h(image) + b"\x09\0\0\0\0\0\0\0\x01" + selection
        leaf += b"\0\0\0\0\x04\x03\x02\x01" + bytes(17)
        self.assertEqual(account_leaf(address, image, 9, 0x01020304), leaf)
        self.assertEqual(len(leaf), 162)

    def test_frozen_standalone_event_vector(self):
        raw = bytes.fromhex(
            "00000000040006004a420f000000000000f15365000000000000000000000000d131194cfe9ec9b2"
            "150000000201000021000000020100002d0000000201000039000000820000003d000000160000004d00000037000000"
            "fe6c0821e42a7ce7ba3b4cf2f39760b81da2634f847ab47d125dd4fea0abf665afb5003836c1ff0c78c26fd3721dd59f963e3b06b1278c31cc8bd693215a08bb41cf71f3a60c58a5b0161cb98b39677717451e57e2c77350149f146fb51cbeaa77"
            "616c6c65742e7472616e7366657200050000005a000000090000002200000061737365742f7368656c6c00000000000a000000000000000c00000000000200150000003a0000001400000002000000190000002a000000180000000100010029000000520000002c00000001000100616d6f756e74000002000000000000001a270000000000006d656d6f00000000070000000000000001000000c2000000d361503ac1eeffe15468ae5638e24d7836b19312084ca337726563697069656e74000000000000000800000000000000000000000000020005000000020100001100000002010000bf4b15f21a083c67d64954f5724c6b2fcd1d39e3507af41887d4eb56385ea8163d2add52b7ace38d22fbed4b7e4078d3e48c96d29d20412e63bb02fbbc60ad12"
        )
        self.assertEqual(h(raw).hex(), "4c33f52867e6c1466a4c9d7f2bad8badd802e6a7cc1090a105742bd850ec2f59")
        source = raw[88:152]
        tx_hash = raw[152:184]
        recipient = bytes.fromhex("bf4b15f21a083c67d64954f5724c6b2fcd1d39e3507af41887d4eb56385ea8163d2add52b7ace38d22fbed4b7e4078d3e48c96d29d20412e63bb02fbbc60ad12")
        # The owner fixture uses generated_bytes(event_index=10, len=24).
        # The next byte in its frozen wire is the 'r' of "recipient", not memo.
        memo = bytes.fromhex("d361503ac1eeffe15468ae5638e24d7836b19312084ca337")
        self.assertEqual(len(memo), 24)
        encoded = event_bytes(source, tx_hash, 1_000_010, 1_700_000_000, 0, 0,
                              0xb2c99efe4c1931d1, "wallet.transfer", [b"asset/shell", b"\x0a\0\0\0"],
                              [("amount", "u64", 10010),
                               ("memo", "data", memo),
                               ("recipient", "address", recipient)])
        self.assertEqual(encoded, raw)

    def test_zero_length_lists_keep_their_preorder_target_offsets(self):
        # https://capnproto.org/encoding.html#canonicalization specifies the
        # special offset -1 for empty structs only. Empty lists still point to
        # the next preorder object position, even though they allocate no words.
        empty = event_bytes(bytes(64), bytes(32), 0, 0, 0, 0, 1, "x", [],
                            [("a", "data", b"")])
        # Root: 1 pointer + 4 data + 6 pointer words, followed by 12 source/tx
        # words and one name word. The topics pointer is word 9, target word 24.
        self.assertEqual(empty[9 * 8:10 * 8], bytes.fromhex("3900000006000000"))
        topics = event_bytes(bytes(64), bytes(32), 0, 0, 0, 0, 1, "x", [b"", b"k"],
                             [("a", "data", b"")])
        # The two topic pointers occupy words 24/25. Empty Data at word 24
        # targets word 26, so its signed word offset is one, not zero.
        self.assertEqual(topics[24 * 8:25 * 8], bytes.fromhex("0500000002000000"))

    def test_each_complete_content_and_each_claimed_root_mutation_is_rejected(self):
        expected = {"accounts": {b"a" * 64: b"complete account image"},
                    "account_leaves": {b"a" * 64: b"complete index metadata"},
                    "registry": {b"r" * 33: b"complete registry record"},
                    "code": {b"c" * 32: b"code bytes"}, "abi": {b"d" * 32: b"ABI bytes"},
                    "events": [b"complete canonical event"],
                    "rings": {b"e" * 32: b"complete AIER record"}}
        for compact in (False, True):
            roots = {"account_state_root": map_root(expected["account_leaves"], 64, compact).hex(),
                     "registry_root": map_root(expected["registry"], 33, compact).hex(),
                     "internal_event_ring_root": map_root(expected["rings"], 32, compact).hex()}
            verify_contents(copy.deepcopy(expected), expected, roots, compact)
            for field in expected:
                actual = copy.deepcopy(expected)
                if isinstance(actual[field], list):
                    actual[field][0] += b"mutated"
                else:
                    key = next(iter(actual[field]))
                    actual[field][key] += b"mutated"
                with self.subTest(compact=compact, field=field), self.assertRaises(AssertionError):
                    verify_contents(actual, expected, roots, compact)
            for field in roots:
                changed = dict(roots, **{field: "00" * 32})
                with self.subTest(compact=compact, root=field), self.assertRaises(AssertionError):
                    verify_contents(expected, expected, changed, compact)


if __name__ == "__main__":
    unittest.main()
