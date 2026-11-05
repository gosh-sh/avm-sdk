import importlib.util
from pathlib import Path
import unittest


SPEC = importlib.util.spec_from_file_location("contract_memory", Path(__file__).with_name("contract_memory.py"))
memory = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(memory)


class ContractMemoryTests(unittest.TestCase):
    def test_reads_the_memory_section_in_pages(self):
        module = b"\0asm\x01\0\0\0\x05\x04\x01\x01\x01\x20"
        self.assertEqual(memory.memory_limits(module), (65536, 2097152))

    def test_missing_maximum_is_retained_as_unbounded(self):
        module = b"\0asm\x01\0\0\0\x05\x03\x01\x00\x01"
        self.assertEqual(memory.memory_limits(module), (65536, None))

    def test_malformed_or_unsupported_memory_is_rejected(self):
        for module in [b"bad", b"\0asm\x01\0\0\0", b"\0asm\x01\0\0\0\x05\x03\x01\x01\x01", b"\0asm\x01\0\0\0\x05\x04\x01\x03\x01\x02"]:
            with self.subTest(module=module), self.assertRaises(ValueError):
                memory.memory_limits(module)

    def test_qualification_requires_actual_maximum_and_receipt(self):
        with self.assertRaises(ValueError):
            memory.check_built_memory(b"\0asm\x01\0\0\0\x05\x04\x01\x01\x01\x20", 4194304, "-Clink-arg=--max-memory=4194304")
        with self.assertRaises(ValueError):
            memory.check_built_memory(b"\0asm\x01\0\0\0\x05\x04\x01\x01\x01\x20", 2097152, "rustflags: none")


if __name__ == "__main__":
    unittest.main()
