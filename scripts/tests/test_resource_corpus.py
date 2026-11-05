"""Captured test coverage must be complete before it can select deployment sizes."""
import json
from pathlib import Path
import tempfile
import unittest

from measure_contract_resources import read_corpus


class ResourceCorpusTests(unittest.TestCase):
    def corpus(self, root, rows, observations):
        (root / 'builds.jsonl').write_text('{"code_hash":"own"}\n')
        vectors = root / 'test-vectors'
        vectors.mkdir()
        (vectors / 'stand-1.tsv').write_text('\n'.join(rows) + '\n')
        (vectors / 'stand-1.memory.jsonl').write_text(json.dumps({'complete': True, 'runs': observations}) + '\n')

    def test_lost_or_foreign_execution_cannot_be_used_for_measurement(self):
        row = 'code_hash=own\tresult=ok\tvm_status=0\tfuel_budget=100\toutput_cap=100'
        for observed in ([], [{'code_hash': 'other'}], [{'code_hash': 'own'}, {'code_hash': 'own'}]):
            with self.subTest(observed=observed), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                self.corpus(root, [row], observed)
                with self.assertRaisesRegex(ValueError, 'disagree'):
                    read_corpus(root)

    def test_incomplete_raw_execution_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.corpus(root, ['result=error'], [])
            with self.assertRaisesRegex(ValueError, 'incomplete execution'):
                read_corpus(root)

    def test_incomplete_memory_capture_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.corpus(root, ['code_hash=own\tresult=ok\tvm_status=0\tfuel_budget=100\toutput_cap=100'], [{'code_hash': 'own'}])
            (root / 'test-vectors/stand-1.memory.jsonl').write_text('{"complete":false,"runs":[]}\n')
            with self.assertRaisesRegex(ValueError, 'incomplete memory'):
                read_corpus(root)

    def test_memory_execution_without_raw_vectors_cannot_disappear(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.corpus(root, ['code_hash=own\tresult=ok\tvm_status=0\tfuel_budget=100\toutput_cap=100'], [{'code_hash': 'own'}])
            (root / 'test-vectors/stand-2.memory.jsonl').write_text('{"complete":true,"runs":[{"code_hash":"own"}]}\n')
            with self.assertRaisesRegex(ValueError, 'no raw test capture'):
                read_corpus(root)


if __name__ == '__main__':
    unittest.main()
