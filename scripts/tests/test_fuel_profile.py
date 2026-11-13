"""Closed operation-price grammar, checked against the runtime profile contract."""

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from release import parse_profile

PROFILE = Path(__file__).resolve().parents[2] / "profiles/wasmtime-release-profile.v1"
OPERATOR_COST = "operator_cost=op_prices_v1;Block:0,Br:1,BrIf:1,BrTable:8,Call:17,CallIndirect:31,DataDrop:1,Drop:0,ElemDrop:61,Else:0,End:0,F32Abs:1,F32Add:1,F32Ceil:1,F32Const:1,F32ConvertI32S:1,F32ConvertI32U:1,F32ConvertI64S:1,F32ConvertI64U:25,F32Copysign:1,F32DemoteF64:1,F32Div:255,F32Eq:1,F32Floor:1,F32Ge:1,F32Gt:1,F32Le:1,F32Load:9,F32Lt:1,F32Max:1,F32Min:1,F32Mul:255,F32Ne:1,F32Nearest:1,F32Neg:1,F32ReinterpretI32:1,F32Sqrt:255,F32Store:1,F32Sub:1,F32Trunc:1,F64Abs:1,F64Add:1,F64Ceil:1,F64Const:1,F64ConvertI32S:1,F64ConvertI32U:1,F64ConvertI64S:1,F64ConvertI64U:26,F64Copysign:1,F64Div:255,F64Eq:1,F64Floor:1,F64Ge:1,F64Gt:1,F64Le:1,F64Load:9,F64Lt:1,F64Max:1,F64Min:1,F64Mul:253,F64Ne:1,F64Nearest:1,F64Neg:1,F64PromoteF32:1,F64ReinterpretI64:1,F64Sqrt:255,F64Store:1,F64Sub:1,F64Trunc:1,GlobalGet:1,GlobalSet:8,I32Add:1,I32And:1,I32Clz:1,I32Const:1,I32Ctz:1,I32DivS:9,I32DivU:11,I32Eq:1,I32Eqz:1,I32Extend16S:1,I32Extend8S:1,I32GeS:1,I32GeU:1,I32GtS:1,I32GtU:1,I32LeS:1,I32LeU:1,I32Load:2,I32Load16S:9,I32Load16U:9,I32Load8S:9,I32Load8U:9,I32LtS:1,I32LtU:1,I32Mul:1,I32Ne:1,I32Or:1,I32Popcnt:1,I32ReinterpretF32:1,I32RemS:12,I32RemU:11,I32Rotl:1,I32Rotr:1,I32Shl:1,I32ShrS:1,I32ShrU:1,I32Store:9,I32Store16:9,I32Store8:9,I32Sub:1,I32TruncF32S:1,I32TruncF32U:10,I32TruncF64S:1,I32TruncF64U:10,I32WrapI64:1,I32Xor:1,I64Add:1,I64Add128:1,I64And:1,I64Clz:1,I64Const:1,I64Ctz:1,I64DivS:47,I64DivU:36,I64Eq:1,I64Eqz:1,I64Extend16S:1,I64Extend32S:1,I64Extend8S:1,I64ExtendI32S:1,I64ExtendI32U:1,I64GeS:1,I64GeU:1,I64GtS:1,I64GtU:1,I64LeS:1,I64LeU:1,I64Load:3,I64Load16S:1,I64Load16U:1,I64Load32S:1,I64Load32U:1,I64Load8S:1,I64Load8U:1,I64LtS:1,I64LtU:1,I64Mul:1,I64MulWideS:1,I64MulWideU:1,I64Ne:1,I64Or:1,I64Popcnt:1,I64ReinterpretF64:1,I64RemS:46,I64RemU:36,I64Rotl:1,I64Rotr:1,I64Shl:1,I64ShrS:1,I64ShrU:1,I64Store:9,I64Store16:9,I64Store32:9,I64Store8:9,I64Sub:1,I64Sub128:1,I64TruncF32S:1,I64TruncF32U:12,I64TruncF64S:1,I64TruncF64U:1,I64Xor:1,If:5,LocalGet:1,LocalSet:1,LocalTee:1,Loop:0,MemoryCopy:1,MemoryFill:1,MemoryGrow:255,MemoryInit:1,MemorySize:1,Nop:0,Return:0,Select:1,TableCopy:1,TableInit:1,Unreachable:0"
VARIABLE_COSTS = "variable_operator_costs=memory_copy_per_byte:5,memory_fill_per_byte:3,memory_grow_per_page:255,memory_init_per_byte:2,table_copy_per_element:28,table_init_per_element:4,others:0"


def priced_profile():
    replacements = {
        "operator_cost": OPERATOR_COST,
        "variable_operator_costs": VARIABLE_COSTS,
        "fuel_semantics_revision": "fuel_semantics_revision=4",
        "instance_pre_abi_revision": "instance_pre_abi_revision=1",
    }
    return ("\n".join(replacements.get(line.split("=", 1)[0], line)
                      for line in PROFILE.read_text().splitlines()
                      if not line.startswith("precompile_")) + "\n").encode()


class FuelProfileTests(unittest.TestCase):
    def test_complete_runtime_price_schedule_is_accepted(self):
        fields = parse_profile(priced_profile())
        self.assertEqual(fields["operator_cost"], OPERATOR_COST.split("=", 1)[1])
        self.assertEqual(fields["variable_operator_costs"], VARIABLE_COSTS.split("=", 1)[1])
        self.assertEqual(fields["fuel_semantics_revision"], "4")

    def test_legacy_flat_profile_remains_verifiable(self):
        # Flat profiles predate memory payment and never carry a page price.
        legacy = b"".join(line for line in priced_profile().splitlines(keepends=True)
                          if not line.startswith(b"memory_page_price="))
        legacy = legacy.replace(OPERATOR_COST.encode(), b"operator_cost=legacy_flat_v1")
        legacy = legacy.replace(VARIABLE_COSTS.encode(), b"variable_operator_costs=all_zero")
        legacy = legacy.replace(b"fuel_semantics_revision=4", b"fuel_semantics_revision=2")
        self.assertEqual(parse_profile(legacy)["operator_cost"], "legacy_flat_v1")

    def test_malformed_or_incomplete_prices_are_rejected(self):
        original = priced_profile()
        mutations = [
            (b"Block:0", b"Block:1"),
            (b"Br:1,", b""),
            (b"Br:1,", b"Br:1,Br:1,"),
            (b"Br:1,", b"MadeUp:1,"),
            (b"Br:1,", b"Br:256,"),
            (b"Br:1,", b"Br:-1,"),
            (b"Br:1,", b"Br:01,"),
            (b"op_prices_v1;", b"op_prices_v2;"),
            (b"memory_copy_per_byte:5", b"memory_copy_per_byte:256"),
            (b"memory_copy_per_byte:5,", b""),
            (b"memory_copy_per_byte:5,", b"memory_copy_per_byte:5,memory_copy_per_byte:5,"),
            (b"table_init_per_element:4", b"table_fill_per_element:4"),
            (b"others:0", b"others:1"),
            (b"fuel_semantics_revision=4", b"fuel_semantics_revision=2"),
        ]
        for before, after in mutations:
            with self.subTest(before=before, after=after), self.assertRaises(ValueError):
                parse_profile(original.replace(before, after))
        for before, after in [
            (OPERATOR_COST.encode(), b"operator_cost=legacy_flat_v1"),
            (VARIABLE_COSTS.encode(), b"variable_operator_costs=all_zero"),
        ]:
            with self.subTest(after=after), self.assertRaises(ValueError):
                parse_profile(original.replace(before, after))

    def test_memory_page_price_is_one_positive_decimal_line(self):
        lines = [line for line in priced_profile().decode().splitlines()
                 if not line.startswith("memory_page_price=")]
        position = next(index for index, line in enumerate(lines)
                        if line.startswith("variable_operator_costs=")) + 1

        def with_price(*prices):
            priced = lines[:position] + ["memory_page_price=" + price for price in prices]
            return ("\n".join(priced + lines[position:]) + "\n").encode()

        for price in ("1", "1753", "18446744073709551615"):
            self.assertEqual(parse_profile(with_price(price))["memory_page_price"], price)
        self.assertNotIn("memory_page_price", parse_profile(with_price()))
        for prices in [("0",), ("01",), ("+1",), ("-1",), ("1.0",), ("1_000",),
                       ("18446744073709551616",), ("1", "1")]:
            with self.subTest(prices=prices), self.assertRaises(ValueError):
                parse_profile(with_price(*prices))
        legacy = with_price("1753").replace(OPERATOR_COST.encode(), b"operator_cost=legacy_flat_v1")
        legacy = legacy.replace(VARIABLE_COSTS.encode(), b"variable_operator_costs=all_zero")
        legacy = legacy.replace(b"fuel_semantics_revision=4", b"fuel_semantics_revision=2")
        with self.assertRaises(ValueError):
            parse_profile(legacy)

    def test_memory_classes_are_ten_page_aligned_ascending_maxima(self):
        maxima = "131072,196608,262144,327680,458752,589824,1048576,2097152,4194304,33554432"
        selected = {"memory_reservation": "selected_memory_class_bytes",
                    "pooling_max_memory_size": "selected_memory_class_bytes"}
        lines = [line for line in priced_profile().decode().splitlines()
                 if not line.startswith(("memory_class_", "store_memory_limit="))]
        lines = [key + "=" + selected[key] if key in selected else line
                 for line in lines for key in [line.split("=", 1)[0]]]

        def with_classes(maxima, extra=("memory_class_selection=declared_maximum_first_enclosing",
                                        "memory_class_operator_cost=one_common_table",
                                        "store_memory_limit=config.wasm.memory_limit_bytes")):
            return ("\n".join(lines + ["memory_class_maxima_bytes=" + maxima, *extra]) + "\n").encode()

        fields = parse_profile(with_classes(maxima))
        self.assertEqual(fields["memory_class_maxima_bytes"], maxima)
        self.assertEqual(fields["memory_reservation"], "selected_memory_class_bytes")
        for changed in [maxima.replace("196608,", ""), maxima + ",67108864", maxima.replace("196608", "196609"),
                        maxima.replace("262144,327680", "327680,262144"), maxima.replace("131072", "65536"),
                        maxima.replace(",33554432", ",33619968"), maxima.replace("131072", "0131072")]:
            with self.subTest(maxima=changed), self.assertRaises(ValueError):
                parse_profile(with_classes(changed))
        for extra in [(), ("memory_class_selection=declared_maximum_first_enclosing",
                           "memory_class_operator_cost=one_common_table",
                           "store_memory_limit=min(config.wasm.memory_limit_bytes,8388608)")]:
            with self.subTest(extra=extra), self.assertRaises(ValueError):
                parse_profile(with_classes(maxima, extra))
        unselected = with_classes(maxima).replace(b"=selected_memory_class_bytes",
                                                  b"=config.wasm.memory_limit_bytes")
        with self.assertRaises(ValueError):
            parse_profile(unselected)


if __name__ == "__main__":
    unittest.main()
