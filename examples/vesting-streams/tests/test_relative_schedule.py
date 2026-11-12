"""Constructor offsets are independent of execution wall-clock time."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("vesting_journey", Path(__file__).resolve().parents[1] / "scripts/e2e.py")
journey = importlib.util.module_from_spec(spec)
spec.loader.exec_module(journey)

class RelativeJourneyEncodingTests(unittest.TestCase):
    def test_journey_config_contains_only_schedule_offsets(self):
        owner = "51" * 32
        address = "11" * 32 + ":" + "22" * 32
        recipients = [[address, address] for _ in range(3)]
        original = journey.BASE_TIME
        try:
            journey.BASE_TIME = 1000
            first = journey.config(owner, address, [address] * 3, recipients)
            journey.BASE_TIME = 2_000_000_000
            second = journey.config(owner, address, [address] * 3, recipients)
        finally:
            journey.BASE_TIME = original
        self.assertEqual(first, second)
        self.assertEqual(int.from_bytes(first[328:332], "little"), 100)

if __name__ == "__main__":
    unittest.main()
