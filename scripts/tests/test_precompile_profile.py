"""The shipped crypto profile has a closed import/table and coherent revisions."""
from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from release import parse_profile
PROFILE = Path(__file__).resolve().parents[2] / 'profiles/wasmtime-release-profile.v1'
class PrecompileProfileTests(unittest.TestCase):
    def test_complete_crypto_profile(self):
        fields = parse_profile(PROFILE.read_bytes())
        self.assertEqual(fields['fuel_semantics_revision'], '5')
        self.assertEqual(fields['instance_pre_abi_revision'], '2')
        self.assertEqual(fields['precompile_abi_revision'], '1')
        self.assertEqual(fields['precompile_import'], 'avm.precompile(i32,i32,i32,i32)->i32')
        self.assertEqual(fields['precompile_table'], '1:blake3:32:2048:8;2:sha256:32:2048:4;3:ed25519_verify_strict:1:300000:12')
    def test_incomplete_unknown_or_mixed_crypto_profiles_fail(self):
        original = PROFILE.read_bytes()
        for key in ('precompile_abi_revision', 'precompile_import', 'precompile_table'):
            line = next(line for line in original.splitlines(keepends=True) if line.startswith(key.encode()+b'='))
            for changed in (original.replace(line,b''), original+line, original.replace(line,line.rstrip()+b'_unknown\n')):
                with self.subTest(key=key, changed=changed), self.assertRaises(ValueError):
                    parse_profile(changed)
        for before, after in ((b'fuel_semantics_revision=5',b'fuel_semantics_revision=4'),
                              (b'instance_pre_abi_revision=2',b'instance_pre_abi_revision=1'),
                              (b'1:blake3:32:2048:8',b'1:blake3:32:2048:9')):
            with self.subTest(before=before), self.assertRaises(ValueError):
                parse_profile(original.replace(before,after))
if __name__ == '__main__':
    unittest.main()
