"""Every case in tests/fixtures/day_verdicts.json, through the generated Python bindings.

Run with the directory that holds ankiquest_rules.py and the native library:
    python test_day_verdicts.py <bindings dir> <day_verdicts.json>
"""

import json
import sys
import unittest

BINDINGS, FIXTURE = sys.argv[1], sys.argv[2]
sys.path.insert(0, BINDINGS)

import ankiquest_rules as rules  # noqa: E402


def server(v):
    today = v["today"]
    return rules.ServerDay(
        streak=v["streak"],
        streak_state=rules.StreakState[v["streak_state"].upper()],
        day_ends_at=v["day_ends_at"],
        last_review_id=v["last_review_id"],
        today=rules.ServerToday(
            starts_at=today["starts_at"],
            reviews=today["reviews"],
            time_ms=today["time_ms"],
            minutes=today["minutes"],
            streak_minutes=today["streak_minutes"],
        ),
    )


def shape(verdict):
    """The verdict in the fixture's own shape: None for a day that has ended."""
    if verdict.is_ended():
        return None
    v = verdict.verdict
    return {
        "met": v.met,
        "streak_state": v.streak_state.name.lower() if v.streak_state else None,
        "streak": v.streak,
        "reviews": v.reviews,
        "time_ms": v.time_ms,
        "minutes": v.minutes,
        "streak_minutes": v.streak_minutes,
        "unsent": v.unsent,
        "label": v.label.name.lower(),
    }


class DayVerdicts(unittest.TestCase):
    def test_every_case(self):
        with open(FIXTURE, encoding="utf-8") as f:
            cases = json.load(f)["cases"]
        self.assertTrue(cases)
        for case in cases:
            with self.subTest(case["name"]):
                pending = [rules.PendingReview(id=r["id"], time_ms=r["time_ms"]) for r in case["pending"]]
                verdict = rules.today_verdict(server(case["server"]), pending, case["now_ms"])
                self.assertEqual(shape(verdict), case["expected"])


if __name__ == "__main__":
    unittest.main(argv=sys.argv[:1])
